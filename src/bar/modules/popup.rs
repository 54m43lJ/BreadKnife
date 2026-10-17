//! 弹窗会话托管：统一打开 / 定位 / 关闭。
//! 关闭面三条：① 任务栏内的按下一律先关全部弹窗——事件继续送达目标部件（无缝切换，
//! 含 menu ↔ submenu ↔ 其它入口）；② 任务栏外的按下由合成器经 xdg_popup grab 收回
//! 弹窗并吞掉（点击外部自动关闭，不落到底下的应用）——grab 由 GtkPopover 的
//! autohide 建立，各弹窗不得改为无 grab 自管，否则会失去 ②；③ Escape 兜底。
//!
//! 定位纪律：弹窗挂父部件——根窗口（不参与容器布局，避免撑宽右对齐元素）或上级
//! 弹窗（Wayland 嵌套弹窗，锚点矩形落在上级弹窗几何内）；pointing_to 在每次打开前
//! 按当前布局重算（锚点坐标随内容变化，装配期尚无分配）；右对齐 / 顶对齐用
//! halign / valign 表达，GTK 据此摆放。
//!
//! 菜单跟踪悬停切换（macOS 菜单栏式）：弹窗抓取期间合成器把指针焦点钉在弹窗
//! 表面上，任务栏窗口收不到 motion——GTK 事件路径不可用，锚点悬停改由
//! 「合成器指针轮询」驱动（仅弹窗打开期间轮询），见 hover_switch。
//!
//! 借用纪律：popdown 同步派发 closed 信号，闭包会重入各模块的 RefCell；
//! 任何持有 RefCell 借用的代码路径不得跨越 popdown——本结构一律
//! 「先收集目标、释放借用、再逐个关闭」。

use std::cell::{Cell, RefCell};
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

pub struct PopupManager {
    window: gtk::Window,
    /// 打开中的弹窗（组标签：cc / notif / tray:<item> / tray:<item>:sub）。
    open: RefCell<Vec<(gtk::Popover, String)>>,
    /// 本次按下经捕获阶段关闭的组标签；每次按下先清空（锚点 toggle 判定）。
    press_closed: RefCell<Vec<String>>,
    /// 悬停切换锚点（托盘图标；注册即参与轮询判定）。
    hover_anchors: RefCell<Vec<HoverAnchor>>,
    /// 悬停切换判定状态。
    hover_state: RefCell<HoverState>,
    /// 悬停世代：命中锚点变化即递增；异步切换回执据此校验（离开即作废）。
    hover_generation: Cell<u64>,
    /// 指针轮询线程是否在跑。
    polling: Cell<bool>,
}

struct HoverAnchor {
    widget: gtk::Widget,
    switch: Rc<dyn Fn(u64)>,
}

#[derive(Default)]
struct HoverState {
    anchor: Option<usize>,
    deadline: Option<Instant>,
    fired: bool,
}

impl PopupManager {
    /// 窗口级装配：捕获阶段按下、Escape 两类关闭源。
    pub fn install(window: &gtk::Window) -> Rc<Self> {
        let mgr = Rc::new(Self {
            window: window.clone(),
            open: RefCell::new(Vec::new()),
            press_closed: RefCell::new(Vec::new()),
            hover_anchors: RefCell::new(Vec::new()),
            hover_state: RefCell::new(HoverState::default()),
            hover_generation: Cell::new(0),
            polling: Cell::new(false),
        });

        // 任务栏内按下：先关全部弹窗，交互本身照常送达目标部件
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.set_button(0);
        let m = mgr.clone();
        click.connect_pressed(move |_g, _n, _x, _y| {
            m.press_closed.borrow_mut().clear();
            let closed: Vec<String> =
                m.open.borrow().iter().map(|(_, g)| g.clone()).collect();
            m.close_all();
            *m.press_closed.borrow_mut() = closed;
        });
        window.add_controller(click);

        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let m = mgr.clone();
        key.connect_key_pressed(move |_k, keyval, _code, _mods| {
            if keyval == gtk::gdk::Key::Escape && m.close_all() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key);

        mgr
    }

    /// 建造期登记：closed 信号自动除名（每个 popover 恰一次）。
    pub fn manage(self: &Rc<Self>, popover: &gtk::Popover) {
        let mgr = self.clone();
        let watch = popover.clone();
        popover.connect_closed(move |_| mgr.unlist(&watch));
    }

    /// 打开：同组旧实例先收（托盘菜单每次新建 popover）→ 登记 → popup。
    pub fn open(self: &Rc<Self>, popover: &gtk::Popover, group: String) {
        self.close_group(&group);
        self.open.borrow_mut().push((popover.clone(), group));
        popover.popup();
        if !self.hover_anchors.borrow().is_empty() {
            self.ensure_hover_poll();
        }
    }

    /// 关闭指定组。
    pub fn close_group(&self, group: &str) {
        self.close_by(|g| g == group);
    }

    /// 关闭组标签以 prefix 开头的弹窗（如 "tray:" 收根菜单与全部子层）。
    pub fn close_prefix(&self, prefix: &str) {
        self.close_by(|g| g.starts_with(prefix));
    }

    /// 关闭全部；返回是否有弹窗被关（Escape 拦截判定用）。
    pub fn close_all(&self) -> bool {
        let targets: Vec<gtk::Popover> =
            self.open.borrow().iter().map(|(p, _)| p.clone()).collect();
        let had = !targets.is_empty();
        for p in targets {
            p.popdown(); // 同步派发 closed → 除名；此刻不得持有 open 借用
        }
        had
    }

    /// 本 press 的捕获阶段是否关闭过指定组（锚点 toggle：跳过重开与重复交互）。
    pub fn press_closed_contains(&self, group: &str) -> bool {
        self.press_closed.borrow().iter().any(|g| g == group)
    }

    /// 是否有弹窗正在显示（菜单跟踪悬停切换的门槛）。
    pub fn any_open(&self) -> bool {
        !self.open.borrow().is_empty()
    }

    /// 悬停世代（异步切换回执校验用）。
    pub fn hover_generation(&self) -> u64 {
        self.hover_generation.get()
    }

    /// 确保合成器指针轮询在跑（弹窗打开期间启动；全部关闭后自行收停）。
    fn ensure_hover_poll(self: &Rc<Self>) {
        if self.polling.replace(true) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<(f64, f64)>();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("hover-poll".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if let Some(pos) = cursor_pos() {
                            if tx.send(pos).is_err() {
                                break;
                            }
                        }
                        std::thread::sleep(HOVER_POLL);
                    }
                })
                .ok();
        }
        let m = self.clone();
        glib::timeout_add_local(HOVER_POLL, move || {
            let mut latest = None;
            while let Ok(pos) = rx.try_recv() {
                latest = Some(pos);
            }
            if let Some(pos) = latest {
                m.hover_tick(pos);
            }
            if !m.any_open() {
                m.polling.set(false);
                *m.hover_state.borrow_mut() = HoverState::default();
                stop.store(true, Ordering::Relaxed);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    /// 一次悬停判定：命中锚点变化 → 记录驻留期限；驻留到期且未触发 → 回调切换。
    fn hover_tick(&self, (px, py): (f64, f64)) {
        let hit = {
            let anchors = self.hover_anchors.borrow();
            anchors.iter().position(|a| {
                a.widget
                    .compute_bounds(&self.window)
                    .map(|b| {
                        let (bx, by) = (b.x() as f64, b.y() as f64);
                        let (bw, bh) = (b.width() as f64, b.height() as f64);
                        px >= bx && px < bx + bw && py >= by && py < by + bh
                    })
                    .unwrap_or(false)
            })
        };
        let now = Instant::now();
        let mut st = self.hover_state.borrow_mut();
        if st.anchor != hit {
            st.anchor = hit;
            st.fired = false;
            st.deadline = hit.map(|_| now + HOVER_DWELL);
            drop(st);
            self.hover_generation
                .set(self.hover_generation.get().wrapping_add(1));
            return;
        }
        let Some(deadline) = st.deadline else { return };
        if st.fired || now < deadline {
            return;
        }
        st.fired = true;
        let token = self.hover_generation.get();
        drop(st);
        let switch = hit.and_then(|idx| {
            self.hover_anchors
                .borrow()
                .get(idx)
                .map(|a| a.switch.clone())
        });
        if let Some(switch) = switch {
            switch(token);
        }
    }

    fn close_by(&self, f: impl Fn(&str) -> bool) {
        let targets: Vec<gtk::Popover> = self
            .open
            .borrow()
            .iter()
            .filter(|(_, g)| f(g))
            .map(|(p, _)| p.clone())
            .collect();
        for p in targets {
            p.popdown();
        }
    }

    fn unlist(&self, popover: &gtk::Popover) {
        self.open.borrow_mut().retain(|(p, _)| p != popover);
    }
}

/// 弹窗静态装配：挂父部件并声明方位与对齐。
/// `parent` 为根窗口时是顶层弹窗；为上级 popover 时是 Wayland 嵌套弹窗。
pub fn attach(
    popover: &gtk::Popover,
    parent: &impl IsA<gtk::Widget>,
    position: gtk::PositionType,
    halign: gtk::Align,
    valign: gtk::Align,
) {
    popover.set_position(position);
    popover.set_halign(halign);
    popover.set_valign(valign);
    popover.set_parent(parent);
}

/// 打开前定位：指向矩形取锚点在父部件坐标系下的当前布局。
pub fn point_at(
    popover: &gtk::Popover,
    parent: &impl IsA<gtk::Widget>,
    anchor: &impl IsA<gtk::Widget>,
) {
    let Some(b) = anchor.compute_bounds(parent) else {
        eprintln!("[popup] anchor bounds unavailable; popover left unanchored");
        return;
    };
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        b.x().round() as i32,
        b.y().round() as i32,
        b.width().round() as i32,
        b.height().round() as i32,
    )));
}

/// 悬停切换驻留时长：防指针掠过误切换。
pub const HOVER_DWELL: Duration = Duration::from_millis(150);

/// 合成器指针轮询周期。
const HOVER_POLL: Duration = Duration::from_millis(50);

/// 菜单跟踪悬停切换（macOS 菜单栏式）登记：弹窗打开期间轮询合成器指针位置，
/// 锚点悬停驻留后调用 `switch(token)`——token 为命中世代值，调用方据此校验异步
/// 切换的迟到回执；指针离开锚点或移到别的锚点即递增世代，未到期与已过期作废。
/// 无弹窗显示时悬停不产生任何动作。
pub fn hover_switch<F: Fn(u64) + 'static>(
    manager: &Rc<PopupManager>,
    anchor: &impl IsA<gtk::Widget>,
    switch: F,
) {
    manager.hover_anchors.borrow_mut().push(HoverAnchor {
        widget: anchor.clone().upcast(),
        switch: Rc::new(switch),
    });
}

/// 合成器指针位置（Hyprland 命令套接字 `j/cursorpos`；轮询专用，失败返回 None）。
fn cursor_pos() -> Option<(f64, f64)> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let mut sock = std::os::unix::net::UnixStream::connect(format!(
        "{runtime}/hypr/{sig}/.socket.sock"
    ))
    .ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
    sock.write_all(b"j/cursorpos").ok()?;
    let mut resp = String::new();
    sock.read_to_string(&mut resp).ok()?;
    let v: serde_json::Value = serde_json::from_str(&resp).ok()?;
    Some((v.get("x")?.as_f64()?, v.get("y")?.as_f64()?))
}
