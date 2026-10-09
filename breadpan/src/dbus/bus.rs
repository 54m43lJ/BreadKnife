//! 会话总线连接（zbus tokio 特性）与库内 tokio 运行时宿主。
//!
//! 运行约束：内部线程固定为 1——宿主 tokio CurrentThread 运行时，
//! DBus 收发、事件组装与投递全部在此异步线程完成。
//! 公开同步方法经 [`BusCenter::cmd`] 把指令投递进运行时受理（内部各 DBus 调用受
//! `timeout_ms` 约束）；本线程以 std 通道阻塞等待结果，绝不在运行时线程上自阻塞。

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::watch;
use zbus::export::serde::Serialize;
use zbus::zvariant::Type;

use crate::events::EventQueue;
use crate::tracker::{ItemEntry, ItemTracker};
use crate::{TrayConfig, TrayError, TrayItemId};

/// 随线程启动前装配的共享件（通道发送端 + 簿记容器）。
pub(crate) struct BusParts {
    pub(crate) tracker: Arc<RwLock<ItemTracker>>,
    pub(crate) events: Arc<EventQueue>,
    pub(crate) pending: Arc<Mutex<HashSet<TrayItemId>>>,
    pub(crate) config: TrayConfig,
    pub(crate) stopping: Arc<AtomicBool>,
    pub(crate) stopped: Arc<AtomicBool>,
    pub(crate) stop_tx: watch::Sender<bool>,
    pub(crate) fatal_tx: watch::Sender<String>,
    pub(crate) displaced_tx: watch::Sender<bool>,
}

impl BusParts {
    /// `events`：事件队列（其唯一 Receiver 由 `Tray::start` 持有并交付消费方）。
    pub(crate) fn new(config: TrayConfig, events: Arc<EventQueue>) -> Self {
        let (stop_tx, _stop_rx) = watch::channel(false);
        let (fatal_tx, _fatal_rx) = watch::channel(String::new());
        let (displaced_tx, _displaced_rx) = watch::channel(false);
        Self {
            tracker: Arc::new(RwLock::new(ItemTracker::default())),
            events,
            pending: Arc::new(Mutex::new(HashSet::new())),
            config,
            stopping: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            stop_tx,
            fatal_tx,
            displaced_tx,
        }
    }
}

/// 库内总线域句柄：连接 + 运行时入口 + 共享簿记。Clone 廉价。
#[derive(Clone)]
pub(crate) struct BusCenter {
    pub(crate) conn: zbus::Connection,
    pub(crate) rt: Handle,
    /// 逐调用超时（= config.timeout_ms）。
    pub(crate) timeout: Duration,
    pub(crate) tracker: Arc<RwLock<ItemTracker>>,
    pub(crate) events: Arc<EventQueue>,
    pub(crate) pending: Arc<Mutex<HashSet<TrayItemId>>>,
    pub(crate) config: TrayConfig,
    stopping: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    stop_tx: watch::Sender<bool>,
    fatal_tx: watch::Sender<String>,
    displaced_tx: watch::Sender<bool>,
}

impl BusCenter {
    pub(crate) fn new(parts: BusParts, conn: zbus::Connection, rt: Handle) -> Self {
        Self {
            timeout: Duration::from_millis(parts.config.timeout_ms.max(1)),
            conn,
            rt,
            tracker: parts.tracker,
            events: parts.events,
            pending: parts.pending,
            config: parts.config,
            stopping: parts.stopping,
            stopped: parts.stopped,
            stop_tx: parts.stop_tx,
            fatal_tx: parts.fatal_tx,
            displaced_tx: parts.displaced_tx,
        }
    }

    /// 内部线程上启动库内运行时，装配并引导至就绪。
    /// 返回 (就绪通道, 线程 JoinHandle)；就绪载荷为装配完成的 BusCenter 或启动错误。
    pub(crate) fn spawn_runtime(
        parts: BusParts,
    ) -> (
        tokio::sync::mpsc::Receiver<Result<BusCenter, TrayError>>,
        thread::JoinHandle<()>,
    ) {
        let (ready_tx, ready_rx) = tokio::sync::mpsc::channel(1);
        let stop_rx = parts.stop_tx.subscribe();
        let fatal_rx = parts.fatal_tx.subscribe();
        let displaced_rx = parts.displaced_tx.subscribe();

        let join = thread::Builder::new()
            .name("breadpan-rt".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build breadpan internal runtime");
                rt.block_on(async move {
                    let conn = match zbus::Connection::session().await {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = ready_tx.send(Err(TrayError::Bus(e.to_string()))).await;
                            return;
                        }
                    };
                    let bus = BusCenter::new(parts, conn, Handle::current());
                    crate::dbus::run(bus, stop_rx, fatal_rx, displaced_rx, ready_tx).await;
                });
                // rt 在此 drop：全部内部任务随之丢弃，阻塞中的 cmd 调用方以 Stopped 解除。
            })
            .expect("spawn breadpan internal thread");

        (ready_rx, join)
    }

    // ── DBus 调用（逐调用受 timeout_ms 约束）──

    /// 经会话总线调用方法并反序列化应答体。
    pub(crate) async fn call<T, B>(
        &self,
        dest: &str,
        path: &str,
        iface: &str,
        method: &str,
        body: &B,
    ) -> Result<T, TrayError>
    where
        T: zbus::export::serde::de::DeserializeOwned + Type,
        B: Serialize + Type,
    {
        let ms = self.timeout.as_millis() as u64;
        let reply = tokio::time::timeout(
            self.timeout,
            self.conn
                .call_method(Some(dest), path, Some(iface), method, body),
        )
        .await
        .map_err(|_| TrayError::Timeout { ms })?
        .map_err(TrayError::from)?;
        reply
            .body()
            .deserialize::<T>()
            .map_err(|e| TrayError::Protocol(format!("reply decode: {e}")))
    }

    // ── 同步桥：公开同步方法 → 运行时受理 ──

    /// 把异步指令投递进库内运行时执行，调用线程阻塞等待。
    /// 内部各 DBus 调用已带超时；此处另设宽松守护上限，防意外悬挂。
    pub(crate) fn cmd<T, F>(&self, f: F) -> Result<T, TrayError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, TrayError>> + Send + 'static,
    {
        if self.is_stopped() {
            return Err(TrayError::Stopped);
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<T, TrayError>>(1);
        self.rt.spawn(async move {
            let _ = tx.send(f.await);
        });
        let guard = self.timeout.mul_f64(4.0) + Duration::from_secs(10);
        match rx.recv_timeout(guard) {
            Ok(v) => v,
            // 通道断开 + 已停机 → Stopped；否则视为守护超时
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) if self.is_stopped() => {
                Err(TrayError::Stopped)
            }
            Err(_) => Err(TrayError::Timeout {
                ms: guard.as_millis() as u64,
            }),
        }
    }

    /// 运行时上派发后台任务（受理即忘）。
    pub(crate) fn spawn<F>(&self, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(f);
    }

    // ── 状态与信号 ──

    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub(crate) fn set_stopped(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// 停机路径主动置位（名释放引发的 NameOwnerChanged 不触发 Displaced）。
    pub(crate) fn begin_stopping(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    pub(crate) fn request_stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    pub(crate) fn report_fatal(&self, msg: impl Into<String>) {
        let _ = self.fatal_tx.send(msg.into());
    }

    pub(crate) fn report_displaced(&self) {
        let _ = self.displaced_tx.send(true);
    }

    /// 登记簿记条目快照（供 Handle 层校验在册）。
    pub(crate) fn entry_of(&self, id: &TrayItemId) -> Result<ItemEntry, TrayError> {
        self.tracker
            .read()
            .unwrap()
            .get(id)
            .ok_or_else(|| TrayError::ItemNotFound(id.clone()))
    }
}
