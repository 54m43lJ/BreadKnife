//! bread daemon（Model，无 GTK）：装配与事件循环；停机。
//! 启动 / 退出调用栈见 CODE.html §4.1.1 与 §4.3。

pub mod hyprland;
pub mod ipc;
pub mod notify;
pub mod system;
pub mod tray;
pub mod windows;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use breadpan::{TrayEvent, TrayHandle};

use crate::shared::model::{
    MinimizedWindow, NotificationRecord, NotificationStatus, StateDelta, SystemState, TrayItemView,
    WindowSnapshot, WorkspaceState, WorkspaceWindow,
};
use crate::shared::protocol::{Frame, NotificationDismissReason, Push, Request, Response};

/// 事件循环滴答：超时档兼作重同步去抖窗口（事件风暴在若干个滴答内合并为一次全量重读）。
/// 50ms：工作区切换的更新延迟不超过一帧量级，同时仍能合并连续的窗口生命周期事件。
const TICK: Duration = Duration::from_millis(50);

/// 最小化集合工作区。
pub const MINIMIZED_WORKSPACE: &str = "special:minimized";

/// 汇聚线程 → 主循环的统一事件面。
pub enum DaemonEvent {
    Hyprland(hyprland::HyprlandEvent),
    Tray(TrayEvent),
    WindowMessage {
        id: u64,
        request: Request,
        out: Sender<Frame>,
    },
    Control(crate::shared::protocol::ControlCommand),
    System(SystemState),
    Notification(NotificationRecord),
    NotificationClosed(u32),
    NotificationStatus(NotificationStatus),
    ChildExited {
        monitor: String,
    },
    /// 现场重读完成（Added / Changed 的数据面）。
    TrayView {
        id: String,
        view: TrayItemView,
    },
    /// 现场重读失败通告（Item 尚不可读，仅日志）。
    TrayViewGone {
        id: String,
    },
}

pub struct Daemon {
    tx: Sender<DaemonEvent>,
    rx: Receiver<DaemonEvent>,
    tray: Option<TrayHandle>,
    tray_registry: HashMap<String, TrayItemView>,
    /// 图标显示序（首见序）。
    tray_order: Vec<String>,
    notification: Option<notify::NotificationService>,
    notification_status: NotificationStatus,
    notifications: VecDeque<NotificationRecord>,
    minimized: Vec<MinimizedWindow>,
    monitors: Vec<hyprland::MonitorInfo>,
    workspaces: Vec<hyprland::WorkspaceInfo>,
    /// 工作区规则：本屏工作区清单 = 现存工作区 ∪ 绑定到本屏的静态规则。
    ws_rules: Vec<hyprland::WorkspaceRuleInfo>,
    slices: HashMap<String, Vec<WorkspaceState>>,
    /// 紧急位（持久）：Urgent 事件置位；仅"切换到该工作区"或"活跃时点击该工作区"清除。
    urgent: HashSet<i64>,
    /// 各屏上次的活跃工作区 id（活跃跃迁 → 清除新活跃工作区的紧急位）。
    last_active: HashMap<String, i64>,
    active_window: crate::shared::model::ActiveWindow,
    active_addr: String,
    system: SystemState,
    css_path: String,
    windows: HashMap<String, windows::WindowProc>,
    /// 应存在窗口的显示器集合（open/open-all/插拔维护；close 移除）。
    desired: HashSet<String>,
    /// 首次重同步是否已把显示器清单种入 desired（§3.3：启动即逐屏 spawn）。
    seeded: bool,
    restarts: HashMap<String, (u32, Instant)>,
    pending_resync: bool,
    control_path: String,
    window_path: String,
}

pub fn run(control_path: &str) -> Result<(), String> {
    // PID 文件：活体检测 + 占位（窗口自主退出路径依赖）
    let pid_path = crate::shared::protocol::pid_file();
    if let Ok(existing) = std::fs::read_to_string(&pid_path) {
        let pid = existing.trim();
        if !pid.is_empty() && Path::new(&format!("/proc/{pid}")).exists() {
            return Err(format!("daemon already running (pid {pid})"));
        }
    }
    std::fs::write(&pid_path, std::process::id().to_string())
        .map_err(|e| format!("write pid file: {e}"))?;

    let window_path = crate::shared::protocol::window_socket_for(Path::new(control_path));
    let css_path = resolve_css();

    let (tx, rx) = mpsc::channel::<DaemonEvent>();

    hyprland::watch(tx.clone());
    let tray = tray::start(tx.clone());
    let (notification, notification_status) = match notify::start(tx.clone()) {
        Ok(service) => (Some(service), NotificationStatus::Ready),
        Err(error) => (
            None,
            NotificationStatus::Unavailable {
                message: notify::unavailable_message(error),
            },
        ),
    };
    ipc::serve_control(control_path, tx.clone())
        .map_err(|e| format!("bind control socket: {e}"))?;
    ipc::serve_windows(window_path.to_string_lossy().as_ref(), tx.clone())
        .map_err(|e| format!("bind window socket: {e}"))?;
    system::start(tx.clone());

    let mut daemon = Daemon {
        tx,
        rx,
        tray,
        tray_registry: HashMap::new(),
        tray_order: Vec::new(),
        notification,
        notification_status,
        notifications: VecDeque::new(),
        minimized: Vec::new(),
        monitors: Vec::new(),
        workspaces: Vec::new(),
        ws_rules: Vec::new(),
        slices: HashMap::new(),
        urgent: HashSet::new(),
        last_active: HashMap::new(),
        active_window: crate::shared::model::ActiveWindow::default(),
        active_addr: String::new(),
        system: SystemState::default(),
        css_path,
        windows: HashMap::new(),
        desired: HashSet::new(),
        seeded: false,
        restarts: HashMap::new(),
        pending_resync: false,
        control_path: control_path.to_string(),
        window_path: window_path.to_string_lossy().into(),
    };
    daemon.resync(); // 首次：显示器清单 + 种子窗口
    daemon.run_loop();
    daemon.shutdown();
    Ok(())
}

/// CSS 路径解析：BREAD_CSS 覆盖，否则把内置主题落到运行时目录（daemon 跟踪该路径）。
fn resolve_css() -> String {
    if let Some(p) = std::env::var_os("BREAD_CSS") {
        return p.to_string_lossy().into();
    }
    let dir = crate::shared::protocol::runtime_dir().join("bread");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("bread.css");
    let _ = std::fs::write(&path, include_str!("../assets/bread.css"));
    path.to_string_lossy().into()
}

impl Daemon {
    // ── 事件循环 ──

    fn run_loop(&mut self) {
        loop {
            match self.rx.recv_timeout(TICK) {
                Ok(ev) => {
                    if !self.handle(ev) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.pending_resync {
                        self.pending_resync = false;
                        self.resync();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// 返回 false = 退出事件循环。
    fn handle(&mut self, ev: DaemonEvent) -> bool {
        match ev {
            DaemonEvent::Hyprland(hev) => self.on_hyprland(hev),
            DaemonEvent::Tray(tev) => self.on_tray(tev),
            DaemonEvent::WindowMessage { id, request, out } => {
                self.on_window_request(id, request, out)
            }
            DaemonEvent::Control(cmd) => self.on_control(cmd),
            DaemonEvent::System(state) => {
                if self.system != state {
                    self.system = state;
                    self.broadcast(Push::State(StateDelta {
                        system: Some(self.system.clone()),
                        ..Default::default()
                    }));
                }
            }
            DaemonEvent::Notification(record) => self.on_notification(record),
            DaemonEvent::NotificationClosed(id) => self.on_notification_closed(id),
            DaemonEvent::NotificationStatus(status) => self.on_notification_status(status),
            DaemonEvent::ChildExited { monitor } => self.on_child_exited(monitor),
            DaemonEvent::TrayView { id, view } => {
                let fresh = !self.tray_registry.contains_key(&id);
                self.tray_registry.insert(id.clone(), view.clone());
                if fresh {
                    self.tray_order.push(id);
                }
                let update = if fresh {
                    crate::shared::model::TrayUpdate::Added(view)
                } else {
                    crate::shared::model::TrayUpdate::Changed(view)
                };
                self.broadcast(Push::Tray(update));
            }
            DaemonEvent::TrayViewGone { id } => {
                eprintln!("[tray] item {id} data unavailable (fresh read failed)");
            }
        }
        true
    }

    // ── Hyprland 事件面 ──

    fn on_hyprland(&mut self, ev: hyprland::HyprlandEvent) {
        use hyprland::HyprlandEvent::*;
        match ev {
            MonitorAdded(name) => {
                eprintln!("[daemon] monitor added: {name}");
                self.desired.insert(name);
                self.pending_resync = true;
            }
            MonitorRemoved(name) => {
                eprintln!("[daemon] monitor removed: {name}");
                self.desired.remove(&name);
                self.kill_window(&name);
                self.pending_resync = true;
            }
            ActiveWindow { class, title } => {
                // v1 载荷（class,title）；地址由随后的 activewindowv2 更新
                self.active_window = crate::shared::model::ActiveWindow { class, title };
                self.broadcast(Push::State(StateDelta {
                    active: Some(self.active_window.clone()),
                    ..Default::default()
                }));
            }
            ActiveWindowAddr(addr) => {
                self.active_addr = hyprland::normalize_addr(&addr);
                if self.active_addr.is_empty() {
                    self.active_window = crate::shared::model::ActiveWindow::default();
                    self.broadcast(Push::State(StateDelta {
                        active: Some(self.active_window.clone()),
                        ..Default::default()
                    }));
                }
            }
            WindowTitle { addr, title } => {
                if !addr.is_empty() {
                    self.update_workspace_window_title(&addr, &title);
                    self.update_minimized_title(&addr, &title);
                }
                if !addr.is_empty() && hyprland::normalize_addr(&addr) == self.active_addr {
                    self.active_window.title = title;
                    self.broadcast(Push::State(StateDelta {
                        active: Some(self.active_window.clone()),
                        ..Default::default()
                    }));
                }
            }
            Urgent { addr } => {
                // 即时上条：置位后立即重建并推送切片，不等下一次重同步
                if let Some(ws) = hyprland::workspace_of_client(&addr)
                    && self.urgent.insert(ws)
                {
                    self.rebuild_workspace_slices();
                    self.push_workspace_slices();
                }
            }
            MoveWindow { .. } | OpenWindow { .. } | CloseWindow(_) => {
                self.resync_minimized();
                // 窗口归属变化同样影响工作区悬停列表。
                self.pending_resync = true;
            }
            Minimized { addr, minimized } => {
                if minimized {
                    self.minimize_window(&addr);
                } else {
                    self.restore_minimized(&addr);
                    self.resync_minimized();
                }
            }
            Resync => self.pending_resync = true,
        }
    }

    fn clear_urgent_on_active_transition(&mut self) {
        clear_urgent_on_active_transition(&mut self.urgent, &mut self.last_active, &self.monitors);
    }

    /// 全量重同步：显示器清单、工作区映射（活动 / 占用 / 紧急）、窗口进程对齐。
    /// 首次执行把显示器清单种入 desired——启动即逐屏 spawn（§3.3）；
    /// 此后 desired 只经 open/close/插拔事件变更（用户关闭的不因重同步复活）。
    fn resync(&mut self) {
        self.monitors = hyprland::monitors();
        self.workspaces = hyprland::workspaces();
        self.ws_rules = hyprland::workspace_rules();
        // 启动 / 重同步主动读取焦点窗口：仅靠 activewindow 事件会漏掉启动前的焦点。
        if let Some(active) = hyprland::active_window() {
            self.active_addr = hyprland::normalize_addr(&active.address);
            self.active_window = crate::shared::model::ActiveWindow {
                class: active.class,
                title: active.title,
            };
        }
        // 重同步是无状态刷新：即使内容未变也重发 active，修复窗口侧漏推/空白后
        // 仅靠事件永远等不到下一次更新的问题（如移动窗口到别的工作区）。
        self.broadcast(Push::State(StateDelta {
            active: Some(self.active_window.clone()),
            ..Default::default()
        }));
        if !self.seeded {
            self.seeded = true;
            for m in &self.monitors {
                self.desired.insert(m.name.clone());
            }
        }

        // 紧急位（持久）：仅由 Urgent 事件置位；此处只执行"切换到该工作区"的清除。
        // 工作区销毁（空置回收）后清掉幽灵位，防复现时旧紧急位复活。
        self.urgent
            .retain(|id| self.workspaces.iter().any(|w| w.id == *id));
        // 清除路径①：切换到该工作区（活跃跃迁）
        self.clear_urgent_on_active_transition();

        self.rebuild_workspace_slices();
        self.resync_minimized();

        // 窗口进程对齐：desired ∩ 现存显示器 − 已在册 = spawn；消失的显示器 = kill
        let current: HashSet<String> = self.monitors.iter().map(|m| m.name.clone()).collect();
        let want: Vec<String> = self.desired.intersection(&current).cloned().collect();
        for mon in want {
            if !self.windows.contains_key(&mon) {
                self.spawn_window(&mon);
            }
        }
        let stale: Vec<String> = self
            .windows
            .keys()
            .filter(|m| !current.contains(*m))
            .cloned()
            .collect();
        for mon in stale {
            self.kill_window(&mon);
        }

        self.push_workspace_slices();
    }

    /// 工作区切片重建：显示器 × 工作区清单（现存 ∪ 规则声明）。
    /// 标题事件只改对应条目，避免为非活跃窗口标题触发 clients 全量查询。
    fn update_workspace_window_title(&mut self, addr: &str, title: &str) {
        let addr = hyprland::normalize_addr(addr);
        let mut changed = false;
        for slice in self.slices.values_mut() {
            for workspace in slice {
                for window in &mut workspace.windows {
                    if window.title != title && hyprland::normalize_addr(&window.address) == addr {
                        window.title = title.to_owned();
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.push_workspace_slices();
        }
    }

    fn rebuild_workspace_slices(&mut self) {
        let clients = hyprland::clients();
        let mut slices: HashMap<String, Vec<WorkspaceState>> = HashMap::new();
        for m in &self.monitors {
            slices.insert(
                m.name.clone(),
                workspace_slice(m, &self.workspaces, &self.ws_rules, &self.urgent, &clients),
            );
        }
        self.slices = slices;
    }

    /// 推送全部显示器的最新工作区切片（diff 后定向）。
    fn push_workspace_slices(&mut self) {
        let slices = self.slices.clone();
        let names: Vec<String> = self.monitors.iter().map(|m| m.name.clone()).collect();
        for mon in names {
            if let Some(slice) = slices.get(&mon) {
                self.push_to(
                    &mon,
                    Push::State(StateDelta {
                        workspaces: Some(slice.clone()),
                        ..Default::default()
                    }),
                );
            }
        }
    }

    // ── 窗口进程面 ──

    fn spawn_window(&mut self, monitor: &str) {
        if self.windows.contains_key(monitor) {
            return;
        }
        if let Some(proc) = windows::spawn(&self.tx, monitor, &self.window_path) {
            self.windows.insert(monitor.to_string(), proc);
        }
    }

    fn kill_window(&mut self, monitor: &str) {
        if let Some(mut entry) = self.windows.remove(monitor) {
            entry.killing = true;
            if let Some(out) = entry.out.take() {
                drop(out);
            }
            let mut child = entry.child;
            let _ = child.kill();
            // 收割：独立线程 wait，避免僵尸残留至 daemon 退出
            std::thread::Builder::new()
                .name(format!("reap-{monitor}"))
                .spawn(move || {
                    let _ = child.wait();
                })
                .ok();
            eprintln!("[windows] killed window for {monitor}");
        }
    }

    fn on_child_exited(&mut self, monitor: String) {
        let Some(entry) = self.windows.remove(&monitor) else {
            return; // kill 路径已在册移除；非崩溃退出
        };
        if entry.killing {
            return;
        }
        eprintln!(
            "[windows] window for {monitor} crashed (uptime {:?})",
            entry.started.elapsed()
        );
        if !self.desired.contains(&monitor) {
            return;
        }
        // 连续崩溃窗口期计数
        let (count, since) = self
            .restarts
            .get(&monitor)
            .copied()
            .unwrap_or((0, Instant::now()));
        let (count, since) = if since.elapsed() > windows::RESTART_WINDOW {
            (1, Instant::now())
        } else {
            (count + 1, since)
        };
        self.restarts.insert(monitor.clone(), (count, since));
        if count >= windows::RESTART_LIMIT {
            eprintln!("[windows] giving up on {monitor}: restart limit exceeded");
            self.desired.remove(&monitor);
            return;
        }
        self.spawn_window(&monitor);
    }

    // ── 托盘事件面 ──

    fn on_tray(&mut self, ev: TrayEvent) {
        match ev {
            TrayEvent::Snapshot { items } => {
                self.tray_registry.clear();
                self.tray_order.clear();
                for item in items {
                    let view = tray::view_of(item);
                    self.tray_order.push(view.id.clone());
                    self.tray_registry.insert(view.id.clone(), view.clone());
                    // 常规时序下窗口尚未接入（spawn 晚于 tray start）；
                    // 若已接入则按 Added 幂等补投
                    self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Added(view)));
                }
                eprintln!("[tray] snapshot: {} item(s)", self.tray_registry.len());
            }
            TrayEvent::Added(id) => self.reread_tray_view(id),
            TrayEvent::Changed(id) => self.reread_tray_view(id),
            TrayEvent::Removed(id) => {
                if self.tray_registry.remove(&id).is_some() {
                    self.tray_order.retain(|item| item != &id);
                    self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Removed(id)));
                }
            }
            TrayEvent::MenuChanged(id) => {
                self.broadcast(Push::MenuChanged { item: id });
            }
            TrayEvent::NameChanged { old, new } => {
                // 同一对象 id 重键：原位换键，不补 Removed / Added。
                if let Some(view) = self.tray_registry.remove(&old) {
                    if let Some(slot) = self.tray_order.iter_mut().find(|item| **item == old) {
                        *slot = new.clone();
                    }
                    self.tray_registry.insert(new.clone(), view);
                }
                self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Renamed {
                    old,
                    new: new.clone(),
                }));
                self.reread_tray_view(new);
            }
            TrayEvent::OwnerChanged(id) => {
                eprintln!("[tray] owner changed for {id} (informational)");
            }
            TrayEvent::InteractionResult { id, op, result } => {
                if let Err(e) = result {
                    eprintln!("[tray] interaction {op:?} on {id} failed: {e}");
                }
            }
            TrayEvent::Narration(msg) => {
                eprintln!("[tray] {msg}");
            }
            TrayEvent::Displaced => {
                eprintln!("[tray] displaced by another watcher — terminal, degrading");
                self.degrade_tray();
            }
            TrayEvent::Fatal(msg) => {
                eprintln!("[tray] fatal: {msg} — terminal, degrading");
                self.degrade_tray();
            }
        }
    }

    /// 现场重读上独立线程（item() 阻塞 2–4s，不得卡事件泵）。
    fn reread_tray_view(&mut self, id: String) {
        let Some(handle) = self.tray.clone() else {
            return;
        };
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("tray-reread".into())
            .spawn(move || match handle.item(&id) {
                Some(item) => {
                    let _ = tx.send(DaemonEvent::TrayView {
                        id,
                        view: tray::view_of(item),
                    });
                }
                None => {
                    let _ = tx.send(DaemonEvent::TrayViewGone { id });
                }
            })
            .expect("spawn tray-reread");
    }

    fn degrade_tray(&mut self) {
        self.tray = None;
        let ids: Vec<String> = self.tray_registry.keys().cloned().collect();
        self.tray_registry.clear();
        self.tray_order.clear();
        for id in ids {
            self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Removed(id)));
        }
    }

    // ── 窗口请求面 ──

    fn on_window_request(&mut self, id: u64, request: Request, out: Sender<Frame>) {
        match request {
            Request::Hello { monitor } => {
                if let Some(entry) = self.windows.get_mut(&monitor) {
                    entry.out = Some(out.clone());
                }
                ipc::hello_reply(&out, id, self.snapshot_for(&monitor));
            }
            Request::TrayInteract { item, input } => {
                self.exec_tray(&out, id, move |h| match tray::interact(h, &item, &input) {
                    Ok(o) => Response::Outcome(o),
                    Err(e) => Response::Error(e.to_string()),
                });
            }
            Request::TrayMenuRead { item } => {
                self.exec_tray(&out, id, move |h| match tray::menu_read(h, &item) {
                    Ok(m) => Response::Menu(m),
                    Err(e) => Response::Error(e.to_string()),
                });
            }
            Request::TrayMenuExpand { item, entry } => {
                self.exec_tray(&out, id, move |h| {
                    match tray::menu_expand(h, &item, entry) {
                        Ok(m) => Response::Menu(m),
                        Err(e) => Response::Error(e.to_string()),
                    }
                });
            }
            Request::TrayMenuActivate { item, entry } => {
                self.exec_tray(&out, id, move |h| {
                    match tray::menu_activate(h, &item, entry) {
                        Ok(_) => Response::Ok,
                        Err(e) => Response::Error(e.to_string()),
                    }
                });
            }
            Request::WorkspaceSwitch { target } => {
                // 清除路径②：点击该工作区（活跃且紧急时的解除动作；点击非活跃工作区
                // 等价于"切换到该工作区"）。相对目标（m±1）不按名解析，由跃迁路径清除。
                if let Some(ws_id) = workspace_id_by_name(&self.workspaces, &target)
                    && self.urgent.remove(&ws_id)
                {
                    self.rebuild_workspace_slices();
                    self.push_workspace_slices();
                }
                std::thread::spawn(move || {
                    hyprland::dispatch_workspace(&target);
                });
                ipc::reply(&out, id, Response::Ok);
            }
            Request::MinimizedRestore { address } => {
                self.restore_minimized(&address);
                ipc::reply(&out, id, Response::Ok);
            }
            Request::MinimizedClose { address } => {
                self.close_minimized(&address);
                ipc::reply(&out, id, Response::Ok);
            }
            Request::MediaControl { op } => {
                std::thread::spawn(move || system::media_control(op));
                ipc::reply(&out, id, Response::Ok);
            }
            Request::VolumeSet { value } => {
                std::thread::spawn(move || system::set_volume(value));
                ipc::reply(&out, id, Response::Ok);
            }
            Request::BrightnessSet { value } => {
                std::thread::spawn(move || system::set_brightness(value));
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationRetry => {
                self.retry_notification();
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationDismiss { id: nid, reason } => {
                self.dismiss_notification(nid, reason);
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationRemove { id: nid } => {
                self.remove_notification(nid);
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationDismissAll => {
                self.dismiss_all_notifications();
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationClearGroup { app_name } => {
                self.clear_notification_group(&app_name);
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationClear => {
                self.clear_notifications();
                ipc::reply(&out, id, Response::Ok);
            }
            Request::NotificationActivate {
                id: nid,
                action_key,
                activation_token,
            } => {
                self.activate_notification(nid, action_key.as_deref(), activation_token.as_deref());
                ipc::reply(&out, id, Response::Ok);
            }
        }
    }

    /// 托盘调用上专用线程：同步阻塞发生在该线程，不冻结事件循环。
    fn exec_tray(
        &self,
        out: &Sender<Frame>,
        id: u64,
        f: impl FnOnce(&TrayHandle) -> Response + Send + 'static,
    ) {
        match self.tray.clone() {
            Some(handle) => {
                let out = out.clone();
                std::thread::Builder::new()
                    .name("tray-exec".into())
                    .spawn(move || ipc::reply(&out, id, f(&handle)))
                    .expect("spawn tray-exec");
            }
            None => {
                ipc::reply(out, id, Response::Error("tray unavailable".into()));
            }
        }
    }

    // ── 控制指令面（§4.2）──

    fn on_control(&mut self, cmd: crate::shared::protocol::ControlCommand) {
        use crate::shared::protocol::ControlCommand::*;
        match cmd {
            Open { name, monitor } => {
                self.open_windows(name, monitor);
            }
            Close { name, monitor } => {
                if name != "bar" {
                    eprintln!("[daemon] unknown window kind: {name}");
                    return;
                }
                match monitor {
                    Some(mon) => {
                        self.desired.remove(&mon);
                        self.kill_window(&mon);
                    }
                    None => {
                        let mons: Vec<String> = self.desired.iter().cloned().collect();
                        for mon in mons {
                            self.desired.remove(&mon);
                            self.kill_window(&mon);
                        }
                    }
                }
            }
            OpenAll => {
                let mons: Vec<String> = self.monitors.iter().map(|m| m.name.clone()).collect();
                for mon in mons {
                    self.desired.insert(mon);
                }
                self.pending_resync = true;
            }
            CloseAll => {
                let mons: Vec<String> = self.desired.iter().cloned().collect();
                for mon in mons {
                    self.desired.remove(&mon);
                    self.kill_window(&mon);
                }
            }
            Reload => {
                eprintln!("[daemon] reload: broadcasting css re-read");
                self.broadcast(Push::Reload);
            }
            Exit => {
                eprintln!("[daemon] shutting down...");
                for mon in self.windows.keys().cloned().collect::<Vec<_>>() {
                    self.kill_window(&mon);
                }
                if let Some(handle) = &self.tray {
                    handle.shutdown();
                }
                if let Some(service) = &self.notification {
                    service.shutdown();
                }
                self.shutdown_files();
                std::process::exit(0);
            }
        }
    }

    fn open_windows(&mut self, name: String, monitor: Option<String>) {
        if name != "bar" {
            eprintln!("[daemon] unknown window kind: {name}");
            return;
        }
        match monitor {
            Some(mon) => {
                self.desired.insert(mon);
                self.pending_resync = true;
            }
            None => {
                let mons: Vec<String> = self.monitors.iter().map(|m| m.name.clone()).collect();
                for mon in mons {
                    self.desired.insert(mon);
                }
                self.pending_resync = true;
            }
        }
    }

    // ── 通知面 ──

    fn on_notification(&mut self, record: NotificationRecord) {
        self.notifications.retain(|n| n.id != record.id);
        self.notifications.push_front(record.clone());
        notify::play_sound(&record);
        self.broadcast(Push::Notification(record.clone()));
        if let Some(monitor) = hyprland::current_monitor() {
            self.push_to(&monitor, Push::NotificationPopup(record));
        }
    }

    /// 发送方 `CloseNotification`：视为 dismiss；贴栏卡片移除，条目保留在中心。
    fn on_notification_closed(&mut self, id: u32) {
        let Some(pos) = self.notifications.iter().position(|n| n.id == id) else {
            return;
        };
        if self.notifications[pos].transient {
            let _ = self.notifications.remove(pos);
            self.broadcast(Push::NotificationRemoved(id));
            return;
        }
        if self.notifications[pos].dismissed {
            return;
        }
        self.notifications[pos].dismissed = true;
        self.broadcast(Push::NotificationDismissed(id));
    }

    fn on_notification_status(&mut self, status: NotificationStatus) {
        if self.notification_status != status {
            self.notification_status = status.clone();
            self.broadcast(Push::State(StateDelta {
                notification: Some(status),
                ..Default::default()
            }));
        }
    }

    /// 用户 dismiss / 超时：贴栏卡片消失，条目默认保留在通知中心等待清理；
    /// `transient` 条目按时限直接删除。
    fn dismiss_notification(&mut self, id: u32, reason: NotificationDismissReason) {
        let Some(pos) = self.notifications.iter().position(|n| n.id == id) else {
            return;
        };
        let mut record = self.notifications[pos].clone();
        let bnote_reason = match reason {
            NotificationDismissReason::Dismissed => bnote::CloseReason::Dismissed,
            NotificationDismissReason::Expired => bnote::CloseReason::Expired,
        };
        if record.transient {
            let _ = self.notifications.remove(pos);
            if let Some(service) = &self.notification {
                service.close(id, bnote_reason);
            }
            self.broadcast(Push::NotificationRemoved(id));
            return;
        }
        if record.dismissed {
            return;
        }
        if let Some(service) = &self.notification {
            service.close(id, bnote_reason);
        }
        record.dismissed = true;
        self.notifications[pos] = record;
        self.broadcast(Push::NotificationDismissed(id));
    }

    /// 贴栏弹窗全部 dismiss：中心条目全部保留，等待用户清理。
    fn dismiss_all_notifications(&mut self) {
        let ids: Vec<u32> = self
            .notifications
            .iter()
            .filter(|n| !n.dismissed)
            .map(|n| n.id)
            .collect();
        for id in ids {
            self.dismiss_notification(id, NotificationDismissReason::Dismissed);
        }
    }

    /// 通知中心清理单条：彻底移除；仍活跃的条目顺带向发送方 dismiss。
    fn remove_notification(&mut self, id: u32) {
        let Some(pos) = self.notifications.iter().position(|n| n.id == id) else {
            return;
        };
        let Some(record) = self.notifications.remove(pos) else {
            return;
        };
        if !record.dismissed
            && let Some(service) = &self.notification
        {
            service.close(id, bnote::CloseReason::Dismissed);
        }
        self.broadcast(Push::NotificationRemoved(id));
    }

    fn clear_notification_group(&mut self, app_name: &str) {
        let ids: Vec<u32> = self
            .notifications
            .iter()
            .filter(|n| n.app_name == app_name)
            .map(|n| n.id)
            .collect();
        if let Some(service) = &self.notification {
            for id in &ids {
                if self
                    .notifications
                    .iter()
                    .any(|n| n.id == *id && !n.dismissed)
                {
                    service.close(*id, bnote::CloseReason::Dismissed);
                }
            }
        }
        self.notifications.retain(|n| n.app_name != app_name);
        self.broadcast(Push::NotificationCleared(Some(app_name.to_string())));
    }

    fn clear_notifications(&mut self) {
        if let Some(service) = &self.notification {
            for record in &self.notifications {
                if !record.dismissed {
                    service.close(record.id, bnote::CloseReason::Dismissed);
                }
            }
        }
        self.notifications.clear();
        self.broadcast(Push::NotificationCleared(None));
    }

    fn activate_notification(
        &mut self,
        id: u32,
        action_key: Option<&str>,
        activation_token: Option<&str>,
    ) {
        let Some(record) = self.notifications.iter().find(|n| n.id == id).cloned() else {
            return;
        };
        let key = action_key.map(str::to_string).or_else(|| {
            record
                .actions
                .iter()
                .find(|action| action.key == "default")
                .or_else(|| (record.actions.len() == 1).then(|| &record.actions[0]))
                .map(|action| action.key.clone())
        });
        if let Some(key) = key {
            if let Some(service) = &self.notification {
                service.invoke_action(id, &key, activation_token);
            }
            if record.resident {
                return;
            }
        }
        self.dismiss_notification(id, NotificationDismissReason::Dismissed);
    }

    fn retry_notification(&mut self) {
        if let Some(service) = self.notification.take() {
            service.shutdown();
        }
        match notify::start(self.tx.clone()) {
            Ok(service) => {
                self.notification = Some(service);
                self.on_notification_status(NotificationStatus::Ready);
            }
            Err(error) => {
                self.on_notification_status(NotificationStatus::Unavailable {
                    message: notify::unavailable_message(error),
                });
            }
        }
    }

    // ── 最小化托盘面 ──

    fn resync_minimized(&mut self) {
        let clients: Vec<hyprland::ClientInfo> = hyprland::clients()
            .into_iter()
            .filter(|client| client.workspace.name == MINIMIZED_WORKSPACE)
            .collect();

        let mut next: Vec<MinimizedWindow> = Vec::with_capacity(clients.len());
        // 保留既有顺序，原位更新字段。
        for existing in &self.minimized {
            if let Some(client) = clients
                .iter()
                .find(|client| hyprland::normalize_addr(&client.address) == existing.address)
            {
                next.push(MinimizedWindow {
                    address: hyprland::normalize_addr(&client.address),
                    class: client.class.clone(),
                    title: client.title.clone(),
                    monitor: client.monitor.clone(),
                });
            }
        }
        // 新窗口按 clients() 顺序追加右侧。
        for client in &clients {
            let address = hyprland::normalize_addr(&client.address);
            if next.iter().all(|window| window.address != address) {
                next.push(MinimizedWindow {
                    address,
                    class: client.class.clone(),
                    title: client.title.clone(),
                    monitor: client.monitor.clone(),
                });
            }
        }

        if self.minimized != next {
            self.minimized = next;
            self.push_minimized();
        }
    }

    fn minimize_window(&mut self, address: &str) {
        hyprland::move_to_workspace(address, MINIMIZED_WORKSPACE);
        self.resync_minimized();
    }

    fn restore_minimized(&mut self, address: &str) {
        let address = hyprland::normalize_addr(address);
        let Some(window) = self
            .minimized
            .iter()
            .find(|window| window.address == address)
            .cloned()
        else {
            return;
        };
        if let Some(target) = hyprland::active_workspace_name(&window.monitor) {
            hyprland::move_to_workspace(&address, &target);
            hyprland::focus_window(&address);
        }
    }

    fn close_minimized(&mut self, address: &str) {
        let address = hyprland::normalize_addr(address);
        if self
            .minimized
            .iter()
            .any(|window| window.address == address)
        {
            hyprland::close_window(&address);
        }
    }

    fn update_minimized_title(&mut self, addr: &str, title: &str) {
        let addr = hyprland::normalize_addr(addr);
        let mut changed = false;
        for window in &mut self.minimized {
            if window.address == addr && window.title != title {
                window.title = title.to_string();
                changed = true;
            }
        }
        if changed {
            self.push_minimized();
        }
    }

    fn push_minimized(&self) {
        self.broadcast(Push::State(StateDelta {
            minimized: Some(self.minimized.clone()),
            ..Default::default()
        }));
    }

    // ── 推送 ──

    fn snapshot_for(&self, monitor: &str) -> WindowSnapshot {
        let tray: Vec<TrayItemView> = self
            .tray_order
            .iter()
            .filter_map(|id| self.tray_registry.get(id).cloned())
            .collect();
        WindowSnapshot {
            monitor: monitor.to_string(),
            workspaces: self.slices.get(monitor).cloned().unwrap_or_default(),
            active: self.active_window.clone(),
            tray,
            notifications: self.notifications.iter().cloned().collect(),
            notification: self.notification_status.clone(),
            minimized: self.minimized.clone(),
            system: self.system.clone(),
            css_path: self.css_path.clone(),
        }
    }

    fn push_to(&self, monitor: &str, push: Push) {
        if let Some(entry) = self.windows.get(monitor)
            && let Some(out) = &entry.out
        {
            ipc::push(out, push);
        }
    }

    fn broadcast(&self, push: Push) {
        for entry in self.windows.values() {
            if let Some(out) = &entry.out {
                ipc::push(out, push.clone());
            }
        }
    }

    // ── 停机 ──

    fn shutdown(&mut self) {
        for mon in self.windows.keys().cloned().collect::<Vec<_>>() {
            self.kill_window(&mon);
        }
        if let Some(handle) = &self.tray {
            handle.shutdown();
        }
        if let Some(service) = self.notification.take() {
            service.shutdown();
        }
        self.shutdown_files();
    }

    fn shutdown_files(&self) {
        crate::shared::protocol::shutdown_files();
        let _ = std::fs::remove_file(crate::shared::protocol::pid_file());
        let _ = std::fs::remove_file(&self.control_path);
        let _ = std::fs::remove_file(&self.window_path);
    }
}

/// 活跃跃迁清除：本屏活跃工作区变化时，清除新活跃工作区的紧急位
/// （紧急清除规则之"切换到该工作区"；首次见屏只记基线，不清除）。
fn clear_urgent_on_active_transition(
    urgent: &mut HashSet<i64>,
    last_active: &mut HashMap<String, i64>,
    monitors: &[hyprland::MonitorInfo],
) {
    last_active.retain(|name, _| monitors.iter().any(|m| &m.name == name));
    for m in monitors {
        let Some(ws) = &m.active_ws else { continue };
        if let Some(prev) = last_active.insert(m.name.clone(), ws.id)
            && prev != ws.id
        {
            urgent.remove(&ws.id);
        }
    }
}

/// 目标名 → 工作区 id（点击清除用；相对目标 m±1 不在此解析）。
fn workspace_id_by_name(ws_all: &[hyprland::WorkspaceInfo], target: &str) -> Option<i64> {
    ws_all.iter().find(|w| w.name == target).map(|w| w.id)
}

/// 仅规则、尚未创建的工作区的合成分拣 id 基准：Hyprland 给命名工作区分配负 id，
/// 取深负区间避免与真实 id 相撞（数值规则直接用其 id）。
const RULE_ONLY_ID_BASE: i64 = -100_000;

/// 本屏工作区清单：现存工作区 ∪ 绑定到本屏的静态工作区规则（按名去重 + id 排序）。
/// 仅规则条目＝非活跃工作区：无窗口、不活跃、不紧急；规则按显示器名绑定，动态
/// 选择器（special:* / empty*）不产生常驻条目。
fn workspace_slice(
    monitor: &hyprland::MonitorInfo,
    ws_all: &[hyprland::WorkspaceInfo],
    rules: &[hyprland::WorkspaceRuleInfo],
    urgent: &HashSet<i64>,
    clients: &[hyprland::ClientInfo],
) -> Vec<WorkspaceState> {
    let mut windows_by_ws: HashMap<i64, Vec<(i32, WorkspaceWindow)>> = HashMap::new();
    for client in clients {
        windows_by_ws.entry(client.workspace.id).or_default().push((
            client.focus_history_id,
            WorkspaceWindow {
                address: hyprland::normalize_addr(&client.address),
                title: client.title.clone(),
                class: client.class.clone(),
            },
        ));
    }
    for windows in windows_by_ws.values_mut() {
        windows.sort_by_key(|(focus_history_id, _)| *focus_history_id);
    }

    let mut slice: Vec<WorkspaceState> = ws_all
        .iter()
        .filter(|ws| ws.monitor == monitor.name)
        .map(|ws| WorkspaceState {
            id: ws.id,
            name: ws.name.clone(),
            urgent: urgent.contains(&ws.id),
            active: monitor.active_ws.as_ref().is_some_and(|a| a.id == ws.id),
            has_windows: ws.windows > 0,
            windows: windows_by_ws
                .remove(&ws.id)
                .map(|windows| windows.into_iter().map(|(_, window)| window).collect())
                .unwrap_or_default(),
        })
        .collect();

    // 规则补充：按名对全量现存工作区去重（工作区被移动到其它屏时不造幽灵）
    let existing: HashSet<&str> = ws_all.iter().map(|ws| ws.name.as_str()).collect();
    let mut synthetic = 0i64;
    for rule in rules {
        if !rule.enabled || rule.monitor != monitor.name {
            continue;
        }
        let Some((name, id)) = hyprland::rule_workspace(rule) else {
            continue;
        };
        if existing.contains(name.as_str()) {
            continue;
        }
        let id = match id {
            Some(id) => id,
            None => {
                synthetic += 1;
                RULE_ONLY_ID_BASE - synthetic
            }
        };
        slice.push(WorkspaceState {
            id,
            name,
            urgent: false,
            active: false,
            has_windows: false,
            windows: Vec::new(),
        });
    }

    slice.sort_by_key(|ws| ws.id);
    slice
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::hyprland::{ClientInfo, MonitorInfo, WorkspaceInfo, WorkspaceRef};

    fn mon(name: &str, active: Option<i64>) -> MonitorInfo {
        MonitorInfo {
            id: 0,
            name: name.into(),
            focused: false,
            active_ws: active.map(|id| WorkspaceRef {
                id,
                name: id.to_string(),
            }),
        }
    }

    #[test]
    fn active_transition_clears_only_on_change() {
        let mut urgent: HashSet<i64> = [1, 2, 3].into_iter().collect();
        let mut last = HashMap::new();
        // 首次见屏：只记基线，不清除
        let mons = [mon("DP-1", Some(2))];
        clear_urgent_on_active_transition(&mut urgent, &mut last, &mons);
        assert!(urgent.contains(&2));
        // 同工作区重复重同步：不清除（活跃期间迟到的 Urgent 事件得以保留）
        clear_urgent_on_active_transition(&mut urgent, &mut last, &mons);
        assert!(urgent.contains(&2));
        // 跃迁到 1：清除 1，其余不动
        let mons = [mon("DP-1", Some(1))];
        clear_urgent_on_active_transition(&mut urgent, &mut last, &mons);
        assert!(!urgent.contains(&1));
        assert!(urgent.contains(&2) && urgent.contains(&3));
        // 显示器消失：基线随之移除
        clear_urgent_on_active_transition(&mut urgent, &mut last, &[]);
        assert!(last.is_empty());
    }

    #[test]
    fn workspace_slice_merges_rules_and_dedupes() {
        let m = mon("DP-2", Some(1));
        let ws_all = vec![
            WorkspaceInfo {
                id: 1,
                name: "1".into(),
                monitor: "DP-2".into(),
                windows: 2,
            },
            WorkspaceInfo {
                id: 2,
                name: "2".into(),
                monitor: "DP-2".into(),
                windows: 0,
            },
            WorkspaceInfo {
                id: 3,
                name: "3".into(),
                monitor: "DP-1".into(),
                windows: 1,
            },
        ];
        let rules = vec![
            rule("1", "DP-2", true),
            rule("2", "DP-2", true),
            rule("3", "DP-2", true),             // 现存于 DP-1：不造幽灵
            rule("5", "DP-2", true),             // 仅规则：非活跃工作区
            rule("6", "DP-2", false),            // 禁用：跳过
            rule("7", "DP-1", true),             // 他屏：不出现
            rule("9", "", true),                 // 未绑定：跳过
            rule("name:web", "DP-2", true),      // 命名规则：合成 id
            rule("special:magic", "DP-2", true), // 动态选择器：跳过
        ];
        let urgent: HashSet<i64> = [1].into_iter().collect();
        let slice = workspace_slice(&m, &ws_all, &rules, &urgent, &[]);

        let names: Vec<&str> = slice.iter().map(|s| s.name.as_str()).collect();
        // 命名工作区（合成负 id）排最前，其后按 id：1、2、5
        assert_eq!(names, ["web", "1", "2", "5"]);
        assert_eq!(
            slice.iter().map(|s| s.id).collect::<Vec<_>>(),
            [RULE_ONLY_ID_BASE - 1, 1, 2, 5]
        );

        let one = &slice[1];
        assert!(one.active && one.has_windows && one.urgent);
        let two = &slice[2];
        assert!(!two.active && !two.has_windows);
        let five = &slice[3];
        assert_eq!(five.id, 5);
        assert!(!five.active && !five.has_windows && !five.urgent);
        let web = &slice[0];
        assert!(!web.active && !web.has_windows && !web.urgent);
    }

    #[test]
    fn workspace_slice_groups_clients_and_sorts_by_focus_history() {
        let m = mon("DP-2", Some(1));
        let ws_all = vec![
            WorkspaceInfo {
                id: 1,
                name: "1".into(),
                monitor: "DP-2".into(),
                windows: 2,
            },
            WorkspaceInfo {
                id: 2,
                name: "2".into(),
                monitor: "DP-2".into(),
                windows: 1,
            },
        ];
        let clients = vec![
            ClientInfo {
                address: "0xaaa".into(),
                title: "older".into(),
                class: "app-a".into(),
                focus_history_id: 3,
                monitor: "DP-2".into(),
                workspace: WorkspaceRef {
                    id: 1,
                    name: "1".into(),
                },
            },
            ClientInfo {
                address: "0xbbb".into(),
                title: "newest".into(),
                class: "app-b".into(),
                focus_history_id: 0,
                monitor: "DP-2".into(),
                workspace: WorkspaceRef {
                    id: 1,
                    name: "1".into(),
                },
            },
            ClientInfo {
                address: "0xccc".into(),
                title: "other".into(),
                class: "app-c".into(),
                focus_history_id: 1,
                monitor: "DP-2".into(),
                workspace: WorkspaceRef {
                    id: 2,
                    name: "2".into(),
                },
            },
        ];

        let slice = workspace_slice(&m, &ws_all, &[], &HashSet::new(), &clients);
        assert_eq!(slice[0].windows.len(), 2);
        assert_eq!(
            slice[0]
                .windows
                .iter()
                .map(|window| window.title.as_str())
                .collect::<Vec<_>>(),
            ["newest", "older"]
        );
        assert_eq!(slice[1].windows.len(), 1);
        assert_eq!(slice[1].windows[0].title, "other");
    }

    fn rule(selector: &str, monitor: &str, enabled: bool) -> hyprland::WorkspaceRuleInfo {
        hyprland::WorkspaceRuleInfo {
            workspace_string: selector.into(),
            enabled,
            monitor: monitor.into(),
        }
    }

    #[test]
    fn workspace_id_by_name_exact_match() {
        let ws = vec![
            WorkspaceInfo {
                id: 2,
                name: "2".into(),
                monitor: "DP-1".into(),
                windows: 0,
            },
            WorkspaceInfo {
                id: 7,
                name: "web".into(),
                monitor: "DP-1".into(),
                windows: 1,
            },
        ];
        assert_eq!(workspace_id_by_name(&ws, "2"), Some(2));
        assert_eq!(workspace_id_by_name(&ws, "web"), Some(7));
        assert_eq!(workspace_id_by_name(&ws, "m+1"), None);
    }
}
