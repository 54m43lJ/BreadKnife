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
    /// 窗口级装配：捕获阶段按下、Escape 两类关闭源。
    pub fn install(window: &gtk::Window) -> Rc<Self> {
        let mgr = Rc::new(Self {
            open: RefCell::new(Vec::new()),
            press_closed: RefCell::new(Vec::new()),
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
