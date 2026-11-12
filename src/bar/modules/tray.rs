//! 托盘模块：图标渲染直出 + 手势捕获 + 菜单弹窗（自绘：懒加载 / 激活 / 刷新）。
//! 交互契约见 ARCHITECTURE §4.4「daemon ↔ 窗口」；菜单刷新走 menu_changed 推送（§4.4）。
//! 弹窗经 PopupManager 托管（根层挂根窗口；子层挂根层弹窗，Wayland 嵌套，见 popup.rs）。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use gtk::prelude::*;

use crate::bar::UiTask;
use crate::bar::ipc::IpcClient;
use crate::bar::modules::popup::{self, PopupManager};
use crate::shared::model::{
    IconView, InteractionOutcome, MenuItem, MenuItemType, MenuSnapshot, ToggleState, ToggleType,
    TrayItemView, TrayUpdate,
};
use crate::shared::protocol::{InteractInput, Request, Response};

/// 图标渲染直出尺寸（逻辑像素）。
const ICON_SIZE: i32 = 16;

pub struct TrayWidget {
    inner: Rc<TrayInner>,
}

struct TrayInner {
    window: gtk::Window,
    popup: Rc<PopupManager>,
    container: gtk::Box,
    ipc: Arc<IpcClient>,
    ui: Sender<UiTask>,
    icons: RefCell<HashMap<String, gtk::Button>>,
    /// 每个图标当前 id（Renamed 后手势仍指向新 id）。
    keys: RefCell<HashMap<String, Rc<RefCell<String>>>>,
    /// 图标显示序（首见序）。
    order: RefCell<Vec<String>>,
    /// 当前打开的根菜单会话（一个窗口同时至多一个根弹窗）。
    menu: RefCell<Option<MenuSession>>,
    /// 已展开的子层：(弹窗, 锚行)——行级悬停去留判定（经典菜单语义，见 close_subs_not_at）。
    subs: RefCell<Vec<(gtk::Popover, gtk::Button)>>,
    /// 待展开条目登记：子层请求飞行中的行锚（item, entry) → row。
    pending_rows: RefCell<HashMap<(String, i32), gtk::Button>>,
}

struct MenuSession {
    item: String,
    popover: gtk::Popover,
}

impl TrayWidget {
    pub fn new(
        ipc: Arc<IpcClient>,
        ui: Sender<UiTask>,
        window: &gtk::Window,
        popup: Rc<PopupManager>,
    ) -> Self {
        let inner = Rc::new(TrayInner {
            window: window.clone(),
            popup,
            container: {
                let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                container.add_css_class("tray-box");
                // 背景块按自然高度居中，上下 margin 不受父容器拉伸影响
                container.set_valign(gtk::Align::Center);
                container
            },
            ipc,
            ui,
            icons: RefCell::new(HashMap::new()),
            keys: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
            menu: RefCell::new(None),
            subs: RefCell::new(Vec::new()),
            pending_rows: RefCell::new(HashMap::new()),
        });
        Self { inner }
    }

    pub fn container(&self) -> gtk::Box {
        self.inner.container.clone()
    }

    pub fn rebuild(&self, views: &[TrayItemView]) {
        {
            let mut icons = self.inner.icons.borrow_mut();
            icons.clear();
            self.inner.order.borrow_mut().clear();
            self.inner.keys.borrow_mut().clear();
            while let Some(child) = self.inner.container.first_child() {
                self.inner.container.remove(&child);
            }
        }
        for view in views {
            add_icon(&self.inner, view);
        }
    }

    pub fn apply(&self, update: TrayUpdate) {
        match update {
            TrayUpdate::Added(view) => {
                if !self.inner.icons.borrow().contains_key(&view.id) {
                    add_icon(&self.inner, &view);
                } else {
                    update_icon(&self.inner, &view);
                }
            }
            TrayUpdate::Changed(view) => update_icon(&self.inner, &view),
            TrayUpdate::Removed(id) => remove_icon(&self.inner, &id),
            TrayUpdate::Renamed { old, new } => rename_icon(&self.inner, &old, &new),
        }
    }

    pub fn menu_changed(&self, item: &str) {
        menu_refresh(&self.inner, item);
    }

    // ── 异步回执（UI 队列汇入，GTK 主线程执行）──

    pub fn on_interact_done(&self, item: &str, result: Result<Response, String>) {
        match result {
            // 左键 / 右键命中菜单流程（含 menu_only 项自动转菜单）
            Ok(Response::Outcome(InteractionOutcome::Menu(snapshot))) => {
                if let Some(anchor) = self.inner.icons.borrow().get(item).cloned() {
                    show_menu(&self.inner, &anchor, item, snapshot);
                }
            }
            // ContextMenu 回退：Item 自绘菜单，宿主不渲染
            Ok(Response::Outcome(InteractionOutcome::Dispatched)) => {}
            Ok(Response::Ok) => {}
            Ok(other) => eprintln!("[tray] unexpected interact reply: {other:?}"),
            Err(e) => eprintln!("[tray] interact failed: {e}"),
        }
    }

    pub fn on_expand_done(&self, item: &str, entry: i32, result: Result<Response, String>) {
        let key = (item.to_string(), entry);
        let anchor = self.inner.pending_rows.borrow_mut().remove(&key);
        match (result, anchor) {
            (Ok(Response::Menu(snapshot)), Some(anchor)) => {
                open_sub_popover(&self.inner, &anchor, item, snapshot)
            }
            (Err(e), _) => eprintln!("[tray] menu expand failed: {e}"),
            (other, _) => eprintln!("[tray] unexpected expand reply: {other:?}"),
        }
    }

    pub fn on_activate_done(&self, _item: &str, result: Result<Response, String>) {
        if let Err(e) = &result {
            eprintln!("[tray] menu activate failed: {e}");
        }
        close_menu(&self.inner);
    }

    pub fn on_menu_read_done(&self, item: &str, result: Result<Response, String>) {
        if !self.inner.menu_open_for(item) {
            return;
        }
        match result {
            Ok(Response::Menu(snapshot)) => {
                // 借用即刻释放：widget 调用不得在任何 RefCell 借用之下
                let popover = self.inner.menu.borrow().as_ref().map(|s| s.popover.clone());
                if let Some(popover) = popover {
                    // 行树重建：已展开子层先收（锚行随即被替换）
                    self.inner.popup.close_prefix(&format!("tray:{item}:sub"));
                    let content = build_menu_layer(&self.inner, item, &snapshot);
                    popover.set_child(Some(&content));
                }
            }
            Err(e) => eprintln!("[tray] menu re-read failed: {e}"),
            other => eprintln!("[tray] unexpected menu read reply: {other:?}"),
        }
    }

    /// 悬停切换回执：世代未过期、仍有弹窗显示、且该 item 菜单尚未显示时才切换；
    /// 读取失败或无可见条目（视作无菜单）保持现状。
    pub fn on_hover_menu_done(&self, item: &str, token: u64, result: Result<Response, String>) {
        if self.inner.popup.hover_generation() != token || !self.inner.popup.any_open() {
            return;
        }
        if self.inner.menu_open_for(item) {
            return;
        }
        let Ok(Response::Menu(snapshot)) = result else {
            return;
        };
        if !snapshot.items.iter().any(|m| m.visible) {
            return;
        }
        if let Some(anchor) = self.inner.icons.borrow().get(item).cloned() {
            show_menu(&self.inner, &anchor, item, snapshot);
        }
    }
}

// ── 图标增删改 ──

impl TrayInner {
    /// 该 Item 的根菜单是否正在显示。
    fn menu_open_for(&self, item: &str) -> bool {
        self.menu
            .borrow()
            .as_ref()
            .is_some_and(|s| s.item == item && s.popover.is_visible())
    }
}

fn add_icon(inner: &Rc<TrayInner>, view: &TrayItemView) {
    let button = gtk::Button::new();
    button.add_css_class("tray-btn");
    let key = Rc::new(RefCell::new(view.id.clone()));
    button.set_focusable(false);

    let paintable = icon_paintable_of(&view.icon);
    let image = gtk::Image::from_paintable(Some(&paintable));
    image.set_pixel_size(ICON_SIZE);
    button.set_child(Some(&image));
    if let Some(tooltip) = &view.tooltip {
        button.set_tooltip_text(Some(tooltip));
    }
    if !view.alive {
        button.add_css_class("tray-dead"); // 失联置灰（不移除）
    }
    if view.attention {
        button.add_css_class("tray-attention");
    }

    inner.keys.borrow_mut().insert(view.id.clone(), key.clone());
    attach_gestures(inner, &button, key.clone());

    // 菜单跟踪悬停切换（macOS 菜单栏式）：有弹窗显示时驻留即读该 item 菜单并替换
    {
        let inner2 = inner.clone();
        let item = view.id.clone();
        popup::hover_switch(&inner.popup, &button, move |token| {
            if inner2.menu_open_for(&item) {
                return; // 该项菜单已在显示
            }
            let ipc = inner2.ipc.clone();
            let ui = inner2.ui.clone();
            let item = item.clone();
            std::thread::spawn(move || {
                let result = ipc.request(Request::TrayMenuRead { item: item.clone() });
                let _ = ui.send(UiTask::TrayHoverMenu {
                    item,
                    token,
                    result,
                });
            });
        });
    }

    inner.container.append(&button);
    inner.icons.borrow_mut().insert(view.id.clone(), button);
    inner.order.borrow_mut().push(view.id.clone());
}

fn remove_icon(inner: &Rc<TrayInner>, id: &str) {
    // 借用即刻释放：container.remove 可能触发菜单 closed 等同步信号重入
    let button = inner.icons.borrow_mut().remove(id);
    inner.keys.borrow_mut().remove(id);
    inner.order.borrow_mut().retain(|item| item != id);
    if let Some(button) = button {
        inner.container.remove(&button);
    }
}

/// 原位更新：只换内容与状态，不改变图标在容器中的位置。
fn update_icon(inner: &Rc<TrayInner>, view: &TrayItemView) {
    let Some(button) = inner.icons.borrow().get(&view.id).cloned() else {
        add_icon(inner, view);
        return;
    };
    let paintable = icon_paintable_of(&view.icon);
    let image = gtk::Image::from_paintable(Some(&paintable));
    image.set_pixel_size(ICON_SIZE);
    button.set_child(Some(&image));
    button.set_tooltip_text(view.tooltip.as_deref());
    if view.alive {
        button.remove_css_class("tray-dead");
    } else {
        button.add_css_class("tray-dead");
    }
    if view.attention {
        button.add_css_class("tray-attention");
    } else {
        button.remove_css_class("tray-attention");
    }
}

/// 同一 Item id 重键：原位换键，不移动位置。
fn rename_icon(inner: &Rc<TrayInner>, old: &str, new: &str) {
    if let Some(button) = inner.icons.borrow_mut().remove(old) {
        inner.icons.borrow_mut().insert(new.to_string(), button);
        if let Some(slot) = inner
            .order
            .borrow_mut()
            .iter_mut()
            .find(|item| item.as_str() == old)
        {
            *slot = new.to_string();
        }
    }
    if let Some(key) = inner.keys.borrow_mut().remove(old) {
        *key.borrow_mut() = new.to_string();
        inner.keys.borrow_mut().insert(new.to_string(), key);
    }
    if inner.menu.borrow().as_ref().is_some_and(|s| s.item == old) {
        close_menu(inner);
    }
}

// ── 手势 → IPC → daemon ──

fn attach_gestures(inner: &Rc<TrayInner>, button: &gtk::Button, key: Rc<RefCell<String>>) {
    // 左 / 中 / 右键（坐标取部件局部，SNI 常规做法）
    for button_number in [1u32, 2, 3] {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(button_number);
        let inner2 = inner.clone();
        let key = key.clone();
        gesture.connect_pressed(move |g, _n, x, y| {
            let id = key.borrow().clone();
            // toggle：本次按下刚在捕获阶段关掉自己的菜单 → 不再发起重复交互
            if inner2.popup.press_closed_contains(&format!("tray:{id}"))
                || inner2.menu_open_for(&id)
            {
                return;
            }
            let input = match g.current_button() {
                2 => InteractInput::Middle {
                    x: x as i32,
                    y: y as i32,
                },
                3 => InteractInput::Right {
                    x: x as i32,
                    y: y as i32,
                },
                _ => InteractInput::Left {
                    x: x as i32,
                    y: y as i32,
                },
            };
            let ipc = inner2.ipc.clone();
            let ui = inner2.ui.clone();
            let item = id.clone();
            std::thread::spawn(move || {
                let result = ipc.request(Request::TrayInteract {
                    item: item.clone(),
                    input,
                });
                let _ = ui.send(UiTask::TrayInteractDone { item, result });
            });
        });
        button.add_controller(gesture);
    }

    // 四向滚轮（垂直 + 水平；Shift+滚轮在 GTK 侧表现为水平增量）
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::HORIZONTAL,
    );
    let inner2 = inner.clone();
    let key = key.clone();
    scroll.connect_scroll(move |_ctrl, dx, dy| {
        let input = if dy.abs() >= dx.abs() {
            if dy > 0.0 {
                InteractInput::ScrollDown { steps: 1 }
            } else {
                InteractInput::ScrollUp { steps: 1 }
            }
        } else if dx > 0.0 {
            InteractInput::ScrollRight { steps: 1 }
        } else {
            InteractInput::ScrollLeft { steps: 1 }
        };
        let ipc = inner2.ipc.clone();
        let item = key.borrow().clone();
        std::thread::spawn(move || {
            if let Err(e) = ipc.request(Request::TrayInteract { item, input }) {
                eprintln!("[tray] scroll failed: {e}");
            }
        });
        gtk::glib::Propagation::Proceed
    });
    button.add_controller(scroll);
}

// ── 菜单弹窗（自绘；由 PopupManager 托管）──

fn show_menu(inner: &Rc<TrayInner>, anchor: &gtk::Button, item: &str, snapshot: MenuSnapshot) {
    close_menu(inner);

    let popover = gtk::Popover::new();
    popover.add_css_class("menu-popover");
    popover.set_has_arrow(false);
    popup::attach(
        &popover,
        &inner.window,
        gtk::PositionType::Bottom,
        gtk::Align::Fill,
        gtk::Align::Fill,
    );
    popup::point_at(&popover, &inner.window, anchor);
    let content = build_menu_layer(inner, item, &snapshot);
    popover.set_child(Some(&content));
    {
        let inner2 = inner.clone();
        popover.connect_closed(move |_| {
            *inner2.menu.borrow_mut() = None;
            inner2.pending_rows.borrow_mut().clear();
        });
    }
    inner.popup.manage(&popover);
    inner.popup.clone().open(&popover, format!("tray:{item}"));
    *inner.menu.borrow_mut() = Some(MenuSession {
        item: item.to_string(),
        popover,
    });
}

/// 关闭本 Item 的菜单链（根 + 全部子层）。
/// 借用纪律：先清会话并释放借用，再经 manager popdown——popdown 同步派发
/// closed 信号，闭包会重入 menu / pending_rows（RefCell 跨 popdown 曾致崩溃）。
fn close_menu(inner: &Rc<TrayInner>) {
    inner.pending_rows.borrow_mut().clear();
    *inner.menu.borrow_mut() = None;
    inner.popup.close_prefix("tray:");
}

/// 单层菜单内容：分隔行 / 标准行（toggle、图标、助记符标签、子层箭头）。
fn build_menu_layer(inner: &Rc<TrayInner>, item: &str, snapshot: &MenuSnapshot) -> gtk::Box {
    inner.pending_rows.borrow_mut().clear();
    let layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for entry in snapshot.items.iter().filter(|m| m.visible) {
        match entry.item_type {
            MenuItemType::Separator => {
                let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                sep.add_css_class("menu-sep");
                layer.append(&sep);
            }
            MenuItemType::Standard => {
                layer.append(&build_menu_row(inner, item, entry));
            }
        }
    }
    layer
}

fn build_menu_row(inner: &Rc<TrayInner>, item: &str, entry: &MenuItem) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("menu-row");
    row.set_focusable(false);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    // toggle 指示（忠实透传，无乐观更新——刷新靠 MenuChanged 重读）
    let toggle_label = gtk::Label::new(Some(toggle_glyph(entry)));
    toggle_label.add_css_class("menu-toggle");
    hbox.append(&toggle_label);

    if let Some(image) = menu_entry_icon(entry) {
        hbox.append(&image);
    }

    // 已解析助记符直接交给 GTK 下划线解析
    let label = gtk::Label::with_mnemonic(&entry.label);
    label.add_css_class("menu-label");
    label.set_halign(gtk::Align::Start);
    hbox.append(&label);

    if entry.children_display {
        let arrow = gtk::Label::new(Some("▸"));
        arrow.add_css_class("menu-arrow");
        hbox.append(&arrow);
    }

    row.set_child(Some(&hbox));
    if !entry.enabled {
        row.set_sensitive(false);
    }

    // 行级悬停（macOS 同级悬停）：进入任意层级的行 → ① 收起"锚点非本行且
    // 不含本行"的子层（经典）；② 有子层且可用 → 驻留即展开（掠过不触发）
    {
        let inner2 = inner.clone();
        let item2 = item.to_string();
        let entry_id = entry.id;
        let submenu = entry.children_display;
        let enabled = entry.enabled;
        let row_handle = row.clone();
        let generation = Rc::new(Cell::new(0u64));
        let motion = gtk::EventControllerMotion::new();
        {
            let g = generation.clone();
            let row2 = row_handle.clone();
            motion.connect_enter(move |_c, _x, _y| {
                close_subs_not_at(&inner2, &row2);
                if !submenu || !enabled {
                    return;
                }
                g.set(g.get().wrapping_add(1));
                let token = g.get();
                let g2 = g.clone();
                let inner3 = inner2.clone();
                let row3 = row2.clone();
                let item3 = item2.clone();
                gtk::glib::timeout_add_local_once(popup::HOVER_DWELL, move || {
                    if g2.get() != token {
                        return;
                    }
                    request_expand(&inner3, &row3, &item3, entry_id);
                });
            });
        }
        {
            let g = generation.clone();
            motion.connect_leave(move |_c| {
                g.set(g.get().wrapping_add(1));
            });
        }
        row.add_controller(motion);
    }

    let inner2 = inner.clone();
    let item = item.to_string();
    let entry_id = entry.id;
    let submenu = entry.children_display;
    let row_handle = row.clone();
    row.connect_clicked(move |_| {
        if submenu {
            request_expand(&inner2, &row_handle, &item, entry_id);
        } else {
            // 条目激活：menu_activate（VOID 受理即返回），随后关闭菜单
            let ipc = inner2.ipc.clone();
            let ui = inner2.ui.clone();
            let item = item.clone();
            std::thread::spawn(move || {
                let result = ipc.request(Request::TrayMenuActivate {
                    item: item.clone(),
                    entry: entry_id,
                });
                let _ = ui.send(UiTask::TrayActivateDone { item, result });
            });
        }
    });

    row
}

/// 子层展开请求（懒加载）：登记行锚（飞行中的回执据此回锚）；
/// 同锚子层已在显示则不重复（悬停展开与点击展开的竞态）。
fn request_expand(inner: &Rc<TrayInner>, row: &gtk::Button, item: &str, entry: i32) {
    if inner.subs.borrow().iter().any(|(_, a)| a == row) {
        return;
    }
    inner
        .pending_rows
        .borrow_mut()
        .insert((item.to_string(), entry), row.clone());
    let ipc = inner.ipc.clone();
    let ui = inner.ui.clone();
    let item = item.to_string();
    std::thread::spawn(move || {
        let result = ipc.request(Request::TrayMenuExpand {
            item: item.clone(),
            entry,
        });
        let _ = ui.send(UiTask::TrayExpandDone {
            item,
            entry,
            result,
        });
    });
}

/// 父面板内衬（bread.css `.menu-popover contents` padding 4px + border 1px）：
/// 子菜单锚点矩形按此左右外扩，实现与父面板边缘相接的贴边对齐。
const MENU_PANEL_INSET: f64 = 4.0;

/// 子层弹窗：挂根菜单弹窗之下（Wayland 嵌套弹窗），锚定该行右侧展开；
/// 换层时旧子层先收，根层收回时子层随之收回。
fn open_sub_popover(
    inner: &Rc<TrayInner>,
    anchor: &gtk::Button,
    item: &str,
    snapshot: MenuSnapshot,
) {
    // 借用即刻释放：widget 调用不得在任何 RefCell 借用之下
    let root = inner.menu.borrow().as_ref().map(|s| s.popover.clone());
    let Some(root) = root else {
        eprintln!("[tray] submenu reply without root menu session");
        return;
    };
    // 同锚子层已在显示：不重复展开（悬停展开与点击展开竞态）
    if inner.subs.borrow().iter().any(|(_, a)| a == anchor) {
        return;
    }
    // 展开新子层前按同一悬停规则清场（防异步回执竞态：先前行的子层不该残留）
    close_subs_not_at(inner, anchor);
    let popover = gtk::Popover::new();
    popover.add_css_class("menu-popover");
    popover.set_has_arrow(false);
    popup::attach(
        &popover,
        &root,
        gtk::PositionType::Right,
        gtk::Align::Fill,
        gtk::Align::Start,
    );
    popup::point_at_expanded(&popover, &root, anchor, MENU_PANEL_INSET);
    let content = build_menu_layer(inner, item, &snapshot);
    popover.set_child(Some(&content));
    // 子层登记（锚行）供行级悬停去留判定；收场随 closed 自动除名
    {
        let inner2 = inner.clone();
        let p = popover.clone();
        popover.connect_closed(move |_| {
            inner2.subs.borrow_mut().retain(|(sp, _)| sp != &p);
        });
    }
    inner
        .subs
        .borrow_mut()
        .push((popover.clone(), anchor.clone()));
    inner.popup.manage(&popover);
    inner
        .popup
        .clone()
        .open(&popover, format!("tray:{item}:sub"));
}

/// 行级悬停统一规则（经典菜单语义，根行与子行同规则）：
/// 进入行 R 时，收起所有"锚点不是 R 且不含 R"的子层——即
/// 悬停其它条目收起相关子层；回到父条目/菜单空白/离开菜单均保留。
fn close_subs_not_at(inner: &Rc<TrayInner>, hovered: &gtk::Button) {
    let targets: Vec<gtk::Popover> = inner
        .subs
        .borrow()
        .iter()
        .filter(|(p, anchor)| anchor != hovered && !hovered.is_ancestor(p))
        .map(|(p, _)| p.clone())
        .collect();
    for p in targets {
        p.popdown(); // 同步派发 closed → 除名；此刻不得持有 subs 借用
    }
}

/// MenuChanged 重取刷新：根层或已展开路径——首版重取根层并重建
/// （已展开子层先收，用户按需再展开）。
fn menu_refresh(inner: &Rc<TrayInner>, item: &str) {
    let open = inner
        .menu
        .borrow()
        .as_ref()
        .is_some_and(|s| s.item == item && s.popover.is_visible());
    if !open {
        return;
    }
    let ipc = inner.ipc.clone();
    let ui = inner.ui.clone();
    let item = item.to_string();
    std::thread::spawn(move || {
        let result = ipc.request(Request::TrayMenuRead { item: item.clone() });
        let _ = ui.send(UiTask::TrayMenuReadDone { item, result });
    });
}

// ── 渲染辅助 ──

/// 共享视图 → breadpan 图标源 → 渲染直出（须在 GTK 主线程）。
fn icon_paintable_of(view: &IconView) -> gtk::gdk::Paintable {
    let source = match view {
        IconView::Name(name) => breadpan::IconSource::Name(name.clone()),
        IconView::Path(path) => breadpan::IconSource::Path(path.clone()),
        IconView::Pixmap {
            width,
            height,
            argb,
        } => breadpan::IconSource::Pixmap(breadpan::Pixmap {
            width: *width,
            height: *height,
            argb: argb.clone(),
        }),
    };
    breadpan::gtk::icon_paintable(&source)
}

fn menu_entry_icon(entry: &MenuItem) -> Option<gtk::Image> {
    if let Some(name) = &entry.icon_name {
        let paintable = breadpan::gtk::icon_paintable(&breadpan::IconSource::Name(name.clone()));
        let image = gtk::Image::from_paintable(Some(&paintable));
        image.set_pixel_size(ICON_SIZE);
        return Some(image);
    }
    if let Some(png) = &entry.icon_data {
        let bytes = gtk::glib::Bytes::from_owned(png.clone());
        if let Ok(texture) = gtk::gdk::Texture::from_bytes(&bytes) {
            let image = gtk::Image::from_paintable(Some(&texture));
            image.set_pixel_size(ICON_SIZE);
            return Some(image);
        }
    }
    None
}

fn toggle_glyph(entry: &MenuItem) -> &'static str {
    match (entry.toggle_type, entry.toggle_state) {
        (Some(ToggleType::Checkmark), Some(ToggleState::On)) => "✓",
        (Some(ToggleType::Checkmark), Some(ToggleState::Indeterminate)) => "▬",
        (Some(ToggleType::Radio), Some(ToggleState::On)) => "●",
        (Some(ToggleType::Radio), Some(ToggleState::Indeterminate)) => "◍",
        _ => "",
    }
}
