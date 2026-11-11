//! bar 窗口进程（View，GTK4）：装配与 GTK 主循环；层壳窗口实例；数据分发。
//! 启动调用栈见 CODE.html §4.1.2。

pub mod ipc;
pub mod modules;
pub mod style;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use gtk::prelude::*;
use gtk::{gdk, glib, Application};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::shared::model::WindowSnapshot;
use crate::shared::protocol as wire;
use ipc::IpcClient;
use modules::center::CenterWidget;
use modules::popup::PopupManager;
use modules::tray::TrayWidget;
use modules::window_info::WindowInfoWidget;
use modules::workspace::WorkspaceWidget;

const BAR_HEIGHT: i32 = 30;
/// UI 任务队列排空周期（推送与交互回执经此汇入 GTK 主线程）。
const UI_DRAIN: std::time::Duration = std::time::Duration::from_millis(40);

/// 窗口进程入口：连接 IPC → 取快照 → GTK 主循环。
pub fn run(monitor: &str, socket: &Path) -> Result<(), String> {
    let app = Application::new(
        Some(crate::APP_ID),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    let monitor = monitor.to_string();
    let socket = socket.to_path_buf();
    {
        let app = app.clone();
        app.connect_activate(move |app| {
            if let Err(e) = WindowApp::create(app, &monitor, &socket) {
                eprintln!("[window] init failed: {e}");
                app.quit();
            }
        });
    }
    app.run_with_args(&[] as &[&str]);
    Ok(())
}

/// UI 任务：推送 + 异步回执（工作线程投递，GTK 主线程执行）。
/// 只携带纯数据（锚定部件由 TrayInner 的登记表按 item/entry 反查）。
pub enum UiTask {
    Push(wire::Push),
    TrayInteractDone { item: String, result: Result<wire::Response, String> },
    TrayExpandDone { item: String, entry: i32, result: Result<wire::Response, String> },
    TrayActivateDone { item: String, result: Result<wire::Response, String> },
    TrayMenuReadDone { item: String, result: Result<wire::Response, String> },
    /// 菜单跟踪悬停切换：token 为悬停驻留世代（过期回执丢弃）。
    TrayHoverMenu { item: String, token: u64, result: Result<wire::Response, String> },
}

pub struct WindowApp {
    _app: Application,
    css_path: RefCell<String>,
    workspace: WorkspaceWidget,
    info: WindowInfoWidget,
    tray: TrayWidget,
    center: CenterWidget,
}

impl WindowApp {
    pub fn create(app: &Application, monitor: &str, socket: &Path) -> Result<Rc<Self>, String> {
        // UI 任务队列：工作线程 → GTK 主线程
        let (ui_tx, ui_rx) = mpsc::channel::<UiTask>();
        let ipc = IpcClient::connect(socket, monitor, ui_tx.clone())?;

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .decorated(false)
            .resizable(false)
            .default_height(BAR_HEIGHT)
            .build();

        // 层壳窗口：锚定 Top|Left|Right
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_anchor(Edge::Top, true);
        window.set_namespace(Some("bread"));
        window.auto_exclusive_zone_enable();
        if let Some(m) = find_monitor(monitor) {
            window.set_monitor(Some(&m));
            let geom = m.geometry();
            window.set_default_size(geom.width(), BAR_HEIGHT);
            // 几何跟踪（分辨率变更）
            let window2 = window.clone();
            m.connect_geometry_notify(move |mon| {
                let g = mon.geometry();
                window2.set_default_size(g.width(), BAR_HEIGHT);
            });
        }

        // 弹窗会话：窗口级关闭面先于模块装配（各弹窗经其托管）
        let popup = PopupManager::install(window.upcast_ref());

        let workspace = WorkspaceWidget::new(ipc.clone());
        let info = WindowInfoWidget::new(window.upcast_ref(), popup.clone());
        let tray = TrayWidget::new(ipc.clone(), ui_tx.clone(), window.upcast_ref(), popup.clone());
        let center = CenterWidget::new(ipc.clone(), window.upcast_ref(), popup.clone());
        let layout = gtk::CenterBox::new();
        layout.set_start_widget(Some(&workspace.container));
        layout.set_center_widget(Some(&info.container));
        // 托盘与控制中心入口同侧收尾
        let tail = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tail.append(&tray.container());
        tail.append(&center.container);
        layout.set_end_widget(Some(&tail));
        window.set_child(Some(&layout));
        window.present();

        let state = Rc::new(Self {
            _app: app.clone(),
            css_path: RefCell::new(String::new()),
            workspace,
            info,
            tray,
            center,
        });

        // UI 任务排空（GTK 主线程周期驱动）
        let state_dispatch = state.clone();
        glib::timeout_add_local(UI_DRAIN, move || {
            while let Ok(task) = ui_rx.try_recv() {
                state_dispatch.handle_task(task);
            }
            glib::ControlFlow::Continue
        });

        // daemon 存活周期检查：确认已退出 → 自主退出（退出路径②，CODE §4.2.3）
        let app_watch = app.clone();
        glib::timeout_add_seconds_local(2, move || {
            if !wire::daemon_alive() {
                eprintln!("[window] daemon gone, exiting");
                app_watch.quit();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });

        Ok(state)
    }

    fn handle_task(&self, task: UiTask) {
        match task {
            UiTask::Push(push) => self.handle_push(push),
            UiTask::TrayInteractDone { item, result } => {
                self.tray.on_interact_done(&item, result)
            }
            UiTask::TrayExpandDone { item, entry, result } => {
                self.tray.on_expand_done(&item, entry, result)
            }
            UiTask::TrayActivateDone { item, result } => {
                self.tray.on_activate_done(&item, result)
            }
            UiTask::TrayMenuReadDone { item, result } => {
                self.tray.on_menu_read_done(&item, result)
            }
            UiTask::TrayHoverMenu { item, token, result } => {
                self.tray.on_hover_menu_done(&item, token, result)
            }
        }
    }

    fn handle_push(&self, push: wire::Push) {
        use wire::Push::*;
        match push {
            Snapshot(snapshot) => self.apply_snapshot(&snapshot),
            State(delta) => {
                if let Some(workspaces) = &delta.workspaces {
                    self.workspace.rebuild(workspaces);
                }
                if let Some(active) = &delta.active {
                    self.info.set_active(active);
                }
                if let Some(system) = &delta.system {
                    self.center.set_system(system);
                }
            }
            Tray(update) => self.tray.apply(update),
            MenuChanged { item } => self.tray.menu_changed(&item),
            Notification(record) => self.info.notify(record),
            Reload => {
                let path = self.css_path.borrow().clone();
                if !path.is_empty() {
                    if let Err(e) = style::reload(&path) {
                        eprintln!("[window] css reload failed: {e}");
                    }
                }
            }
        }
    }

    fn apply_snapshot(&self, snapshot: &WindowSnapshot) {
        self.workspace.rebuild(&snapshot.workspaces);
        self.info.set_active(&snapshot.active);
        self.tray.rebuild(&snapshot.tray);
        self.center.set_system(&snapshot.system);
        for record in &snapshot.notifications {
            self.info.notify(record.clone());
        }
        let css = self.css_path.borrow().clone();
        if css != snapshot.css_path {
            *self.css_path.borrow_mut() = snapshot.css_path.clone();
            if let Err(e) = style::reload(&snapshot.css_path) {
                eprintln!("[window] css load failed: {e}");
            }
        }
    }
}

fn find_monitor(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        let monitor = monitors.item(i)?.downcast::<gdk::Monitor>().ok()?;
        if monitor.connector().as_deref() == Some(name) {
            return Some(monitor);
        }
    }
    eprintln!("[window] monitor {name} not found; falling back to default");
    None
}
