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
    NotificationRecord, StateDelta, SystemState, TrayItemView, WindowSnapshot, WorkspaceState,
};
use crate::shared::protocol::{Frame, Push, Request, Response};

/// 事件循环滴答：超时档兼作重同步去抖窗口（事件风暴在若干个滴答内合并为一次全量重读）。
const TICK: Duration = Duration::from_millis(200);

/// 汇聚线程 → 主循环的统一事件面。
pub enum DaemonEvent {
    Hyprland(hyprland::HyprlandEvent),
    Tray(TrayEvent),
    WindowMessage { id: u64, request: Request, out: Sender<Frame> },
    Control(crate::shared::protocol::ControlCommand),
    System(SystemState),
    Notification(NotificationRecord),
    ChildExited { monitor: String },
    /// 现场重读完成（Added / Changed 的数据面）。
    TrayView { id: String, view: TrayItemView },
    /// 现场重读失败通告（Item 尚不可读，仅日志）。
    TrayViewGone { id: String },
}

struct WindowProcEntry {
    proc: windows::WindowProc,
}

pub struct Daemon {
    tx: Sender<DaemonEvent>,
    rx: Receiver<DaemonEvent>,
    tray: Option<TrayHandle>,
    tray_registry: HashMap<String, TrayItemView>,
    monitors: Vec<hyprland::MonitorInfo>,
    ws_all: Vec<hyprland::WorkspaceInfo>,
    /// 工作区规则：本屏工作区清单 = 现存工作区 ∪ 绑定到本屏的静态规则。
    ws_rules: Vec<hyprland::WorkspaceRuleInfo>,
    workspaces: HashMap<String, Vec<WorkspaceState>>,
    /// 紧急位（持久）：Urgent 事件置位；仅"切换到该工作区"或"活跃时点击该工作区"清除。
    urgent: HashSet<i64>,
    /// 各屏上次的活跃工作区 id（活跃跃迁 → 清除新活跃工作区的紧急位）。
    last_active: HashMap<String, i64>,
    active_window: crate::shared::model::ActiveWindow,
    active_addr: String,
    notifications: VecDeque<NotificationRecord>,
    system: SystemState,
    css_path: String,
    windows: HashMap<String, WindowProcEntry>,
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

    let window_path =
        crate::shared::protocol::window_socket_for(Path::new(control_path));
    let css_path = resolve_css();

    let (tx, rx) = mpsc::channel::<DaemonEvent>();

    hyprland::watch(tx.clone());
    let tray = tray::start(tx.clone());
    ipc::serve_control(control_path, tx.clone()).map_err(|e| format!("bind control socket: {e}"))?;
    ipc::serve_windows(
        window_path.to_string_lossy().as_ref(),
        tx.clone(),
    )
    .map_err(|e| format!("bind window socket: {e}"))?;
    system::start(tx.clone());

    let mut daemon = Daemon {
        tx,
        rx,
        tray,
        tray_registry: HashMap::new(),
        monitors: Vec::new(),
        ws_all: Vec::new(),
        ws_rules: Vec::new(),
        workspaces: HashMap::new(),
        urgent: HashSet::new(),
        last_active: HashMap::new(),
        active_window: crate::shared::model::ActiveWindow::default(),
        active_addr: String::new(),
        notifications: VecDeque::new(),
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
                        workspaces: None,
                        active: None,
                        system: Some(self.system.clone()),
                    }));
                }
            }
            DaemonEvent::Notification(record) => {
                let known = self.notifications.iter().any(|n| n.id == record.id);
                if known {
                    if let Some(n) = self.notifications.iter_mut().find(|n| n.id == record.id) {
                        *n = record.clone();
                    }
                } else {
                    self.notifications.push_back(record.clone());
                    while self.notifications.len() > system::NOTIFICATION_KEEP {
                        self.notifications.pop_front();
                    }
                }
                self.broadcast(Push::Notification(record));
            }
            DaemonEvent::ChildExited { monitor } => self.on_child_exited(monitor),
            DaemonEvent::TrayView { id, view } => {
                let fresh = !self.tray_registry.contains_key(&id);
                self.tray_registry.insert(id.clone(), view.clone());
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
                    workspaces: None,
                    active: Some(self.active_window.clone()),
                    system: None,
                }));
            }
            ActiveWindowAddr(addr) => {
                self.active_addr = hyprland::normalize_addr(&addr);
            }
            WindowTitle { addr, title } => {
                if !addr.is_empty() && hyprland::normalize_addr(&addr) == self.active_addr {
                    self.active_window.title = title;
                    self.broadcast(Push::State(StateDelta {
                        workspaces: None,
                        active: Some(self.active_window.clone()),
                        system: None,
                    }));
                }
            }
            Urgent { addr } => {
                // 即时上条：置位后立即重建并推送切片，不等下一次重同步
                if let Some(ws) = hyprland::workspace_of_client(&addr) {
                    if self.urgent.insert(ws) {
                        self.rebuild_workspace_slices();
                        self.push_workspace_slices();
                    }
                }
            }
            Resync => self.pending_resync = true,
        }
    }

    /// 全量重同步：显示器清单、工作区映射（活动 / 占用 / 紧急）、窗口进程对齐。
    /// 首次执行把显示器清单种入 desired——启动即逐屏 spawn（§3.3）；
    /// 此后 desired 只经 open/close/插拔事件变更（用户关闭的不因重同步复活）。
    fn resync(&mut self) {
        self.monitors = hyprland::monitors();
        self.ws_all = hyprland::workspaces();
        self.ws_rules = hyprland::workspace_rules();
        if !self.seeded {
            self.seeded = true;
            for m in &self.monitors {
                self.desired.insert(m.name.clone());
            }
        }

        // 紧急位（持久）：仅由 Urgent 事件置位；此处只执行"切换到该工作区"的清除。
        // 工作区销毁（空置回收）后清掉幽灵位，防复现时旧紧急位复活。
        self.urgent.retain(|id| self.ws_all.iter().any(|w| w.id == *id));
        // 清除路径①：切换到该工作区（活跃跃迁）
        clear_urgent_on_active_transition(&mut self.urgent, &mut self.last_active, &self.monitors);

        self.rebuild_workspace_slices();

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
    fn rebuild_workspace_slices(&mut self) {
        let mut slices: HashMap<String, Vec<WorkspaceState>> = HashMap::new();
        for m in &self.monitors {
            slices.insert(
                m.name.clone(),
                workspace_slice(m, &self.ws_all, &self.ws_rules, &self.urgent),
            );
        }
        self.workspaces = slices;
    }

    /// 推送全部显示器的最新工作区切片（diff 后定向）。
    fn push_workspace_slices(&mut self) {
        let slices = self.workspaces.clone();
        let names: Vec<String> = self.monitors.iter().map(|m| m.name.clone()).collect();
        for mon in names {
            if let Some(slice) = slices.get(&mon) {
                self.push_to(&mon, Push::State(StateDelta {
                    workspaces: Some(slice.clone()),
                    active: None,
                    system: None,
                }));
            }
        }
    }

    // ── 窗口进程面 ──

    fn spawn_window(&mut self, monitor: &str) {
        if self.windows.contains_key(monitor) {
            return;
        }
        if let Some(proc) = windows::spawn(&self.tx, monitor, &self.window_path) {
            self.windows.insert(monitor.to_string(), WindowProcEntry { proc });
        }
    }

    fn kill_window(&mut self, monitor: &str) {
        if let Some(mut entry) = self.windows.remove(monitor) {
            entry.proc.killing = true;
            if let Some(out) = entry.proc.out.take() {
                drop(out);
            }
            let mut child = entry.proc.child;
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
        if entry.proc.killing {
            return;
        }
        eprintln!("[windows] window for {monitor} crashed (uptime {:?})", entry.proc.started.elapsed());
        if !self.desired.contains(&monitor) {
            return;
        }
        // 连续崩溃窗口期计数
        let (count, since) = self.restarts.get(&monitor).copied().unwrap_or((0, Instant::now()));
        let (count, since) = if since.elapsed() > windows::RESTART_WINDOW {
            (1, Instant::now())
        } else {
            (count + 1, since)
        };
        self.restarts.insert(monitor.clone(), (count, since));
        if count > windows::RESTART_LIMIT {
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
                for item in items {
                    let view = tray::view_of(item);
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
                    self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Removed(id)));
                }
            }
            TrayEvent::MenuChanged(id) => {
                self.broadcast(Push::MenuChanged { item: id });
            }
            TrayEvent::NameChanged { old, new } => {
                // 双注册并入：重键（旧 id 不出 Removed，对象生死不变）
                self.tray_registry.remove(&old);
                self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Removed(old)));
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
        let Some(handle) = self.tray.clone() else { return };
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("tray-reread".into())
            .spawn(move || match handle.item(&id) {
                Some(item) => {
                    let _ = tx.send(DaemonEvent::TrayView { id, view: tray::view_of(item) });
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
        for id in ids {
            self.broadcast(Push::Tray(crate::shared::model::TrayUpdate::Removed(id)));
        }
    }

    // ── 窗口请求面 ──

    fn on_window_request(&mut self, id: u64, request: Request, out: Sender<Frame>) {
        match request {
            Request::Hello { monitor } => {
                if let Some(entry) = self.windows.get_mut(&monitor) {
                    entry.proc.out = Some(out.clone());
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
                if let Some(ws_id) = workspace_id_by_name(&self.ws_all, &target) {
                    if self.urgent.remove(&ws_id) {
                        self.rebuild_workspace_slices();
                        self.push_workspace_slices();
                    }
                }
                std::thread::spawn(move || {
                    hyprland::dispatch_workspace(&target);
                });
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

    // ── 推送 ──

    fn snapshot_for(&self, monitor: &str) -> WindowSnapshot {
        let mut tray: Vec<TrayItemView> = self.tray_registry.values().cloned().collect();
        tray.sort_by(|a, b| a.id.cmp(&b.id));
        WindowSnapshot {
            monitor: monitor.to_string(),
            workspaces: self.workspaces.get(monitor).cloned().unwrap_or_default(),
            active: self.active_window.clone(),
            tray,
            notifications: self.notifications.iter().cloned().collect(),
            system: self.system.clone(),
            css_path: self.css_path.clone(),
        }
    }

    fn push_to(&self, monitor: &str, push: Push) {
        if let Some(entry) = self.windows.get(monitor) {
            if let Some(out) = &entry.proc.out {
                ipc::push(out, push);
            }
        }
    }

    fn broadcast(&self, push: Push) {
        for entry in self.windows.values() {
            if let Some(out) = &entry.proc.out {
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
        self.shutdown_files();
    }

    fn shutdown_files(&self) {
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
        if let Some(prev) = last_active.insert(m.name.clone(), ws.id) {
            if prev != ws.id {
                urgent.remove(&ws.id);
            }
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
) -> Vec<WorkspaceState> {
    let mut slice: Vec<WorkspaceState> = ws_all
        .iter()
        .filter(|ws| ws.monitor == monitor.name)
        .map(|ws| WorkspaceState {
            id: ws.id,
            name: ws.name.clone(),
            urgent: urgent.contains(&ws.id),
            active: monitor.active_ws.as_ref().is_some_and(|a| a.id == ws.id),
            has_windows: ws.windows > 0,
        })
        .collect();

    // 规则补充：按名对全量现存工作区去重（工作区被移动到其它屏时不造幽灵）
    let existing: HashSet<&str> = ws_all.iter().map(|ws| ws.name.as_str()).collect();
    let mut synthetic = 0i64;
    for rule in rules {
        if !rule.enabled || rule.monitor != monitor.name {
            continue;
        }
        let Some((name, id)) = hyprland::rule_workspace(rule) else { continue };
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
        });
    }

    slice.sort_by_key(|ws| ws.id);
    slice
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::hyprland::{MonitorInfo, WorkspaceInfo, WorkspaceRef};

    fn mon(name: &str, active: Option<i64>) -> MonitorInfo {
        MonitorInfo {
            name: name.into(),
            active_ws: active.map(|id| WorkspaceRef { id }),
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
        let slice = workspace_slice(&m, &ws_all, &rules, &urgent);

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
