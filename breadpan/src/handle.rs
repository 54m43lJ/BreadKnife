//! TrayConfig + TrayHandle 前端（指令受理 → 内部线程投递）。
//!
//! 公开面无 async fn：入口与交互方法均为同步签名；异步只出现在事件通道消费端
//! （`recv().await`，由消费方经 `glib::spawn_future_local` 驱动）。
//! tokio 运行时为库内私有，消费方无需装配。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::Receiver;

use crate::dbus::bus::{BusCenter, BusParts};
use crate::dbus::interact::{self, Input};
use crate::dbus::menu;
use crate::events::{EventQueue, TrayEvent};
use crate::props;
use crate::{InteractionOutcome, MenuSnapshot, TrayError, TrayItem, TrayItemId};

// ── 配置 ──

/// 服务发现/调用/现场读取超时与属性变更合并窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayConfig {
    /// 服务发现/调用/现场读取超时（ms），默认 2000。
    pub timeout_ms: u64,
    /// 属性变更合并窗口（ms），默认 50。
    pub debounce_ms: u64,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 2000,
            debounce_ms: 50,
        }
    }
}

// ── 内部共享（Handle 薄封装的本体）──

pub(crate) struct RuntimeShared {
    pub(crate) bus: BusCenter,
    /// 内部运行时线程（停机 join 用；在运行时线程自身上不得 join）。
    rt_thread: thread::ThreadId,
    join: Mutex<Option<thread::JoinHandle<()>>>,
}

impl RuntimeShared {
    /// 优雅停机：释放 Watcher 总线名、清空簿记、停内部运行时；幂等。
    fn shutdown(&self) {
        if self.bus.is_stopped() {
            return;
        }
        // 先置 Stopped（此后 Handle 方法一律返回 Stopped）、再标记停机路径
        // （名释放引发的 NameOwnerChanged 不得触发 Displaced）
        self.bus.set_stopped();
        self.bus.begin_stopping();
        self.bus.request_stop();

        // join 运行时线程收尾（释放名 / 清簿记 / 关通道）
        if thread::current().id() != self.rt_thread {
            if let Some(join) = self.join.lock().unwrap().take() {
                let _ = join.join();
            }
        }
    }
}

// ── 入口 ──

/// 一次启动，登记中心与逻辑 Host 同进程就位。
pub struct Tray;

impl Tray {
    /// 申请 Watcher 总线名（不可被替换）→ 反向发现在册 Item → 建立跟踪；
    /// 阻塞至完成（或超时）；通道首事件为快照，与后续事件原子衔接。
    pub fn start(config: TrayConfig) -> Result<(TrayHandle, Receiver<TrayEvent>), TrayError> {
        let (events, rx) = EventQueue::new();
        let parts = BusParts::new(config, events);
        let (mut ready_rx, join) = BusCenter::spawn_runtime(parts);

        // 引导总时限：宽松守护（各步内部已受 timeout_ms 约束）
        let overall = Duration::from_millis((config.timeout_ms * 10).max(10_000));
        let deadline = std::time::Instant::now() + overall;
        let bus = loop {
            if std::time::Instant::now() >= deadline {
                let _ = join.join();
                return Err(TrayError::Timeout {
                    ms: overall.as_millis() as u64,
                });
            }
            match ready_rx.try_recv() {
                Ok(Ok(bus)) => break bus,
                Ok(Err(e)) => {
                    let _ = join.join();
                    return Err(e);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    let _ = join.join();
                    return Err(TrayError::Protocol(
                        "internal thread ended during startup".to_string(),
                    ));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };

        let rt_thread = join.thread().id();
        let shared = Arc::new(RuntimeShared {
            bus,
            rt_thread,
            join: Mutex::new(Some(join)),
        });
        Ok((TrayHandle { shared }, rx))
    }
}

// ── Handle：逻辑 Host 的完整操作面 ──

/// 库操作面。Clone + Send + Sync + 'static，全部方法线程安全。
#[derive(Clone)]
pub struct TrayHandle {
    shared: Arc<RuntimeShared>,
}

impl TrayHandle {
    fn live(&self) -> Result<&BusCenter, TrayError> {
        if self.shared.bus.is_stopped() {
            Err(TrayError::Stopped)
        } else {
            Ok(&self.shared.bus)
        }
    }

    fn dispatch<T, F, Fut>(&self, f: F) -> Result<T, TrayError>
    where
        T: Send + 'static,
        F: FnOnce(BusCenter) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, TrayError>> + Send + 'static,
    {
        let bus = self.live()?.clone();
        let bus_for_cmd = bus.clone();
        bus_for_cmd.cmd(f(bus))
    }

    // ── 查询：现场读取（DBus 即时读取 → 原生结构；items() 为全量刷新操作）──

    pub fn items(&self) -> Vec<TrayItem> {
        let bus = &self.shared.bus;
        if bus.is_stopped() {
            return Vec::new();
        }
        let entries = bus.tracker.read().unwrap().all();
        let bus2 = bus.clone();
        bus.cmd(async move {
            // 并发现场读取（每项独立受 timeout_ms 约束）
            let mut joins = Vec::with_capacity(entries.len());
            for entry in entries {
                let bus3 = bus2.clone();
                joins.push(
                    bus2.rt
                        .spawn(async move { props::fetch_item(&bus3, &entry).await }),
                );
            }
            let mut items = Vec::new();
            for join in joins {
                if let Ok(Ok(item)) = join.await {
                    items.push(item);
                }
            }
            items.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(items)
        })
        .unwrap_or_default()
    }

    pub fn item(&self, id: &TrayItemId) -> Option<TrayItem> {
        let bus = &self.shared.bus;
        if bus.is_stopped() {
            return None;
        }
        let entry = bus.entry_of(id).ok()?;
        let bus2 = bus.clone();
        bus.cmd(async move { props::fetch_item(&bus2, &entry).await })
            .ok()
    }

    /// 登记簿记中的最近一次存活探测结论（零 DBus 流量；不在册 → false）。
    pub fn is_alive(&self, id: &TrayItemId) -> bool {
        self.shared
            .bus
            .tracker
            .read()
            .unwrap()
            .is_alive(id)
            .unwrap_or(false)
    }

    // ── 菜单：前置 AboutToShow + 零缓存现场拉取，逐层懒加载 ──

    /// 读取根层（parent = 0）。Item 无菜单或未就绪 → Err。
    pub fn menu(&self, id: &TrayItemId) -> Result<MenuSnapshot, TrayError> {
        let entry = self.live()?.entry_of(id)?;
        self.dispatch(move |bus| async move { menu::read_layer(&bus, &entry, 0).await })
    }

    /// 懒加载展开：读取某子菜单条目（children_display = true）的下一层。
    pub fn menu_expand(&self, id: &TrayItemId, entry_id: i32) -> Result<MenuSnapshot, TrayError> {
        let entry = self.live()?.entry_of(id)?;
        self.dispatch(move |bus| async move { menu::read_layer(&bus, &entry, entry_id).await })
    }

    // ── 交互：每个语义输入一个方法；缺省 x/y=(0,0)、steps=1 ──

    /// 左键："整项即菜单"（is_menu_only）的 Item 自动转入 Menu 流程，否则 Activate。
    pub fn left_click(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.left_click_at(id, 0, 0)
    }

    pub fn left_click_at(
        &self,
        id: &TrayItemId,
        x: i32,
        y: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::Left(x, y))
    }

    /// 中键：SecondaryActivate。
    pub fn middle_click(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.middle_click_at(id, 0, 0)
    }

    pub fn middle_click_at(
        &self,
        id: &TrayItemId,
        x: i32,
        y: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::Middle(x, y))
    }

    /// 右键：DBusMenu 在册 → Menu 流程；否则 ContextMenu fallback。
    pub fn right_click(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.right_click_at(id, 0, 0)
    }

    pub fn right_click_at(
        &self,
        id: &TrayItemId,
        x: i32,
        y: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::Right(x, y))
    }

    /// 滚轮四向：Scroll(∓steps, vertical / horizontal)。
    pub fn scroll_up(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.scroll_up_by(id, 1)
    }

    pub fn scroll_up_by(
        &self,
        id: &TrayItemId,
        steps: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::ScrollUp(steps))
    }

    pub fn scroll_down(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.scroll_down_by(id, 1)
    }

    pub fn scroll_down_by(
        &self,
        id: &TrayItemId,
        steps: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::ScrollDown(steps))
    }

    pub fn scroll_left(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.scroll_left_by(id, 1)
    }

    pub fn scroll_left_by(
        &self,
        id: &TrayItemId,
        steps: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::ScrollLeft(steps))
    }

    pub fn scroll_right(&self, id: &TrayItemId) -> Result<InteractionOutcome, TrayError> {
        self.scroll_right_by(id, 1)
    }

    pub fn scroll_right_by(
        &self,
        id: &TrayItemId,
        steps: i32,
    ) -> Result<InteractionOutcome, TrayError> {
        self.input(id, Input::ScrollRight(steps))
    }

    /// 延续方法：提交 Outcome::Menu 弹出中的条目点击（dbusmenu Event("clicked")）。
    /// 不是打开菜单的入口——打开与 DBusMenu / ContextMenu 的路由内置于
    /// right_click() / left_click()。
    pub fn menu_activate(&self, id: &TrayItemId, menu_item_id: i32) -> Result<(), TrayError> {
        let bus = self.live()?;
        interact::menu_activate(bus, id, menu_item_id)
    }

    // ── 停机 ──

    /// 优雅停机：释放 Watcher 总线名、清空簿记、停内部运行时；幂等；
    /// 最后一个实例 drop 时自动触发。
    pub fn shutdown(&self) {
        self.shared.shutdown();
    }

    fn input(&self, id: &TrayItemId, input: Input) -> Result<InteractionOutcome, TrayError> {
        let id = id.clone();
        self.dispatch(move |bus| async move { interact::route(&bus, &id, input).await })
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) == 1 {
            self.shared.shutdown();
        }
    }
}
