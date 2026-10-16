//! 弹窗会话托管：无 grab popover 的统一打开 / 定位 / 关闭。
//! 背景：GtkPopover 默认 autohide 抢占输入 grab——外部点击只用于关闭、不落到
//! 下方部件（无缝交互缺失）；且挂在普通容器上的 popover 显示期参与容器布局，
//! 曾把控制中心入口撑宽导致整行重排（右侧对齐漂移）。
//! 对策：弹窗一律挂根窗口、pointing_to 定位、autohide/modal 双关；
//! 关闭自管——捕获阶段窗口外按下先行关闭（交互照常送达目标）、Escape、窗口失活。
//!
//! 借用纪律：popdown 同步派发 closed 信号，闭包会重入各模块的 RefCell；
//! 任何持有 RefCell 借用的代码路径不得跨越 popdown——本结构一律
//! 「先收集目标、释放借用、再逐个关闭」。

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

pub struct PopupManager {
    /// 打开中的弹窗（组标签：cc / notif / tray:<item> / tray:<item>:sub）。
    open: RefCell<Vec<(gtk::Popover, String)>>,
    /// 本次按下经捕获阶段关闭的组标签；每次按下先清空（锚点 toggle 判定）。
    press_closed: RefCell<Vec<String>>,
}

impl PopupManager {
    /// 窗口级装配：捕获阶段点击、Escape、失活三类关闭源。
    pub fn install(window: &gtk::Window) -> Rc<Self> {
        let mgr = Rc::new(Self {
            open: RefCell::new(Vec::new()),
            press_closed: RefCell::new(Vec::new()),
        });

        // 任意键型的按下：菜单外的按下先关菜单，交互本身照常送达目标部件
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.set_button(0);
        let m = mgr.clone();
        click.connect_pressed(move |_g, _n, x, y| {
            m.press_closed.borrow_mut().clear();
            if !m.point_inside_any(x, y) {
                let closed: Vec<String> =
                    m.open.borrow().iter().map(|(_, g)| g.clone()).collect();
                m.close_all();
                *m.press_closed.borrow_mut() = closed;
            }
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

        // 点到其它应用 / 其它屏的 bar（层壳窗口通常不参与激活，此路为兜底）
        let m = mgr.clone();
        window.connect_is_active_notify(move |w| {
            if !w.is_active() {
                m.close_all();
            }
        });

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

    /// 命中判定：按下点是否落在任一打开弹窗的 popup 面内。
    /// popup surface 的位置相对其父 surface（即根窗口），与手势坐标同坐标系。
    fn point_inside_any(&self, x: f64, y: f64) -> bool {
        self.open.borrow().iter().any(|(p, _)| {
            if !p.is_visible() {
                return false;
            }
            let Some(surface) = p.surface() else { return false };
            let Ok(popup) = surface.downcast::<gtk::gdk::Popup>() else { return false };
            let px = f64::from(popup.position_x());
            let py = f64::from(popup.position_y());
            let (w, h) = (f64::from(p.width()), f64::from(p.height()));
            x >= px && x < px + w && y >= py && y < py + h
        })
    }
}

/// 弹窗改造：挂根窗口、指向锚点矩形、无 grab（autohide 关闭即去 grab）。
pub fn attach(
    popover: &gtk::Popover,
    window: &gtk::Window,
    anchor: &impl IsA<gtk::Widget>,
    position: gtk::PositionType,
) {
    popover.set_autohide(false);
    if let Some(rect) = anchor_rect(window, anchor) {
        popover.set_pointing_to(Some(&rect));
    }
    popover.set_position(position);
    popover.set_parent(window);
}

/// 锚点在根窗口坐标系下的矩形（pointing_to 载荷）。
fn anchor_rect(
    window: &gtk::Window,
    anchor: &impl IsA<gtk::Widget>,
) -> Option<gtk::gdk::Rectangle> {
    let b = anchor.compute_bounds(window)?;
    Some(gtk::gdk::Rectangle::new(
        b.x().round() as i32,
        b.y().round() as i32,
        b.width().round() as i32,
        b.height().round() as i32,
    ))
}
