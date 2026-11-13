//! 托盘模块：图标渲染直出 + 手势捕获 + 菜单弹窗（自绘：懒加载 / 激活 / 刷新）。
//! 交互契约见 ARCHITECTURE §4.4「daemon ↔ 窗口」；菜单刷新走 menu_changed 推送（§4.4）。
//! 弹窗经 PopupManager 托管（根层挂根窗口；子层挂根层弹窗，Wayland 嵌套，见 popup.rs）。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

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

/// 新弹层行的悬停静默期：合成器映射 xdg_popup 时可能附带坐标错误的
/// enter / motion，静默期内的行不得据此高亮或启动子层展开。
/// 只作用于新层自己的行，父菜单行不受影响，仍可立即切换。
const HOVER_SETTLE: Duration = Duration::from_millis(80);

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
    /// 已展开的子层：逐级挂在开启它的上级弹窗下，anchor 为所在行。
    subs: RefCell<Vec<SubMenu>>,
    /// 子层会话序号：用作组标签与 closed 除名，避免同 Item 的各层互相顶掉。
    next_sub_id: Cell<u64>,
    /// 当前高亮的菜单行：真实 enter/motion 驱动，同一时刻至多一行高亮。
    hovered_row: RefCell<Option<gtk::Button>>,
    /// 待展开条目登记：子层请求飞行中的行锚（item, entry) → row。
    pending_rows: RefCell<HashMap<(String, i32), gtk::Button>>,
}

struct MenuSession {
    item: String,
    popover: gtk::Popover,
}

struct SubMenu {
    id: u64,
    popover: gtk::Popover,
    anchor: gtk::Button,
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
                container
            },
            ipc,
            ui,
            icons: RefCell::new(HashMap::new()),
            keys: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
            menu: RefCell::new(None),
            subs: RefCell::new(Vec::new()),
            next_sub_id: Cell::new(0),
            hovered_row: RefCell::new(None),
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
            // 重复请求，或菜单重读已作废锚行：迟到回执静默丢弃。
            (Ok(Response::Menu(_)), None) => {}
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
                    // 行树重建：飞行中的展开回执与已展开子层都以旧锚行为准，
                    // 必须先作废，避免回执拿已替换的锚行去开子层。
                    self.inner.pending_rows.borrow_mut().clear();
                    clear_menu_hover(&self.inner);
                    self.inner.popup.close_prefix(&format!("tray:{item}:sub"));
                    // 内容替换不是新弹层：不存在合成 enter，立即允许悬停。
                    let content = build_menu_layer(&self.inner, item, &snapshot, Instant::now());
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
    let content = build_menu_layer(inner, item, &snapshot, Instant::now() + HOVER_SETTLE);
    popover.set_child(Some(&content));
    {
        let inner2 = inner.clone();
        popover.connect_closed(move |_| {
            *inner2.menu.borrow_mut() = None;
            inner2.pending_rows.borrow_mut().clear();
            clear_menu_hover(&inner2);
            // 根层可能被外部点击 / Escape 直接收回：子层一并清掉，避免残留
            // 会话条目在下次菜单打开时误参与悬停判定。
            let subs: Vec<gtk::Popover> = inner2
                .subs
                .borrow()
                .iter()
                .map(|sub| sub.popover.clone())
                .collect();
            for sub in subs.into_iter().rev() {
                sub.popdown();
            }
            inner2.subs.borrow_mut().clear();
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
    clear_menu_hover(inner);
    *inner.menu.borrow_mut() = None;
    inner.popup.close_prefix("tray:");
}

/// 清除当前高亮行；菜单链整体关闭时调用，避免持有已隐藏的旧行。
fn clear_menu_hover(inner: &Rc<TrayInner>) {
    if let Some(row) = inner.hovered_row.borrow_mut().take() {
        row.remove_css_class("menu-hover");
    }
}

/// 给可用的行打上高亮；同一时刻至多一行（新行着色前先清除旧行）。
fn highlight_row(inner: &Rc<TrayInner>, row: &gtk::Button, enabled: bool) {
    if !enabled {
        return;
    }
    let mut hovered = inner.hovered_row.borrow_mut();
    if let Some(prev) = hovered.take()
        && prev != *row
    {
        prev.remove_css_class("menu-hover");
    }
    row.add_css_class("menu-hover");
    *hovered = Some(row.clone());
}

/// 启动一次 150ms 驻留展开；已计时则不重复启动。
fn schedule_expand(
    inner: &Rc<TrayInner>,
    row: &gtk::Button,
    item: &str,
    entry: i32,
    dwell: &Rc<Cell<u64>>,
) {
    if dwell.get() != 0 {
        return;
    }
    dwell.set(1);
    let dwell = dwell.clone();
    let inner = inner.clone();
    let row = row.clone();
    let item = item.to_string();
    gtk::glib::timeout_add_local_once(popup::HOVER_DWELL, move || {
        if dwell.get() != 1 {
            return;
        }
        request_expand(&inner, &row, &item, entry);
    });
}

/// 单层菜单内容：分隔行 / 标准行（toggle、图标、助记符标签、子层箭头）。
/// `hover_after` 之前忽略该层行上的指针事件（新弹层的合成 enter 静默期）。
fn build_menu_layer(
    inner: &Rc<TrayInner>,
    item: &str,
    snapshot: &MenuSnapshot,
    hover_after: Instant,
) -> gtk::Box {
    // 每层只登记自己的行锚；清空 pending_rows 会误伤其它行的飞行中展开请求。
    // 菜单层与行都不接焦点：窗口焦点由栏根常驻持有，弹层打开/关闭都不改变焦点。
    let layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for entry in snapshot.items.iter().filter(|m| m.visible) {
        match entry.item_type {
            MenuItemType::Separator => {
                let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                sep.add_css_class("menu-sep");
                layer.append(&sep);
            }
            MenuItemType::Standard => {
                layer.append(&build_menu_row(inner, item, entry, hover_after));
            }
        }
    }
    layer
}

fn build_menu_row(
    inner: &Rc<TrayInner>,
    item: &str,
    entry: &MenuItem,
    hover_after: Instant,
) -> gtk::Button {
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
    // 不含本行"的子层（经典）；② 有子层且可用 → 驻留即展开（掠过不触发）。
    //
    // 父菜单切换必须继续用 enter：子菜单 popover 是 grab widget，GTK 会把
    // motion 重定向给它，父菜单行收不到 motion；enter/leave 在 picking 阶段
    // 处理，不受重定向影响。新层行在静默期内忽略 enter/motion，父层行不受影响。
    {
        let entry_id = entry.id;
        let submenu = entry.children_display;
        let enabled = entry.enabled;
        let row_handle = row.clone();
        // 0 = 无计时；非 0 = 本次驻留令牌。
        let dwell = Rc::new(Cell::new(0u64));
        let motion = gtk::EventControllerMotion::new();
        {
            let g = dwell.clone();
            let row2 = row_handle.clone();
            let inner2 = inner.clone();
            let item2 = item.to_string();
            motion.connect_enter(move |_c, _x, _y| {
                if Instant::now() < hover_after {
                    return;
                }
                highlight_row(&inner2, &row2, enabled);
                close_subs_not_at(&inner2, &row2);
                if !submenu || !enabled {
                    return;
                }
                schedule_expand(&inner2, &row2, &item2, entry_id, &g);
            });
        }
        {
            // motion 只补高亮 / 补展开：合成 enter 可能已经占住指针焦点，
            // 真实移入同一行时不再有 enter，需要 motion 兜底。
            let g = dwell.clone();
            let row2 = row_handle.clone();
            let inner2 = inner.clone();
            let item2 = item.to_string();
            motion.connect_motion(move |_c, _x, _y| {
                if Instant::now() < hover_after {
                    return;
                }
                highlight_row(&inner2, &row2, enabled);
                if submenu && enabled {
                    schedule_expand(&inner2, &row2, &item2, entry_id, &g);
                }
            });
        }
        {
            let g = dwell.clone();
            let inner2 = inner.clone();
            let row2 = row_handle.clone();
            motion.connect_leave(move |_c| {
                let mut hovered = inner2.hovered_row.borrow_mut();
                if hovered.as_ref() == Some(&row2) {
                    hovered.take();
                }
                row2.remove_css_class("menu-hover");
                g.set(0);
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
    if inner
        .subs
        .borrow()
        .iter()
        .any(|sub| sub.anchor == *row && sub.popover.is_visible())
    {
        return;
    }
    let key = (item.to_string(), entry);
    // 同条目已有展开请求在飞（悬停计时与点击同时到达）：只发一次。
    if inner.pending_rows.borrow().contains_key(&key) {
        return;
    }
    inner.pending_rows.borrow_mut().insert(key, row.clone());
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

/// 子层弹窗：挂在**所在行所属的上级弹窗**下（根层或更上一层，Wayland 嵌套）；
/// 锚定该行右侧展开。逐级嵌套保证进入更深一层时上级仍是祖先，不会被
/// `close_subs_not_at` 误收，也避免同级弹窗互相抢 grab / focus。
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
    // 上级弹窗按锚行的实际祖先确定；旧会话遗留的锚行不再展开。
    let Some(parent) = containing_popover(anchor.upcast_ref::<gtk::Widget>()) else {
        eprintln!("[tray] submenu anchor detached from any popover");
        return;
    };
    let parent_active =
        parent == root || inner.subs.borrow().iter().any(|sub| sub.popover == parent);
    if !parent_active
        || !parent.is_visible()
        || !widget_within(
            parent.upcast_ref::<gtk::Widget>(),
            root.upcast_ref::<gtk::Widget>(),
        )
    {
        eprintln!("[tray] stale submenu anchor ignored");
        return;
    }
    // 同锚子层已在显示：不重复展开（悬停展开与点击展开竞态）
    if inner
        .subs
        .borrow()
        .iter()
        .any(|sub| sub.anchor == *anchor && sub.popover.is_visible())
    {
        return;
    }
    // 展开新子层前按同一悬停规则清场（防异步回执竞态：先前行的子层不该残留）
    close_subs_not_at(inner, anchor);
    let popover = gtk::Popover::new();
    popover.add_css_class("menu-popover");
    popover.set_has_arrow(false);
    popup::attach(
        &popover,
        &parent,
        gtk::PositionType::Right,
        gtk::Align::Fill,
        gtk::Align::Start,
    );
    popup::point_at_expanded(&popover, &parent, anchor, MENU_PANEL_INSET);
    let content = build_menu_layer(inner, item, &snapshot, Instant::now() + HOVER_SETTLE);
    popover.set_child(Some(&content));
    // 每层独立组标签：同 Item 的各级子菜单不得在 open 时互相 close_group。
    let id = inner.next_sub_id.get().wrapping_add(1);
    inner.next_sub_id.set(id);
    {
        let inner2 = inner.clone();
        popover.connect_closed(move |_| {
            // 只捕获序号，避免 closure ↔ popover 自持引用导致子层永不释放。
            inner2.subs.borrow_mut().retain(|sub| sub.id != id);
        });
    }
    inner.subs.borrow_mut().push(SubMenu {
        id,
        popover: popover.clone(),
        anchor: anchor.clone(),
    });
    inner.popup.manage(&popover);
    inner
        .popup
        .clone()
        .open(&popover, format!("tray:{item}:sub:{id}"));
}

/// 行级悬停统一规则（经典菜单语义，根行与子行同规则）：
/// 进入行 R 时，收起所有"锚点不是 R 且不含 R"的子层——即
/// 悬停其它条目收起相关子层；回到父条目/菜单空白/离开菜单均保留。
fn close_subs_not_at(inner: &Rc<TrayInner>, hovered: &gtk::Button) {
    let hovered_widget = hovered.upcast_ref::<gtk::Widget>();
    let targets: Vec<gtk::Popover> = {
        let mut subs = inner.subs.borrow_mut();
        // 已隐藏但 closed 尚未到达的迟到条目不再参与祖先判定。
        subs.retain(|sub| sub.popover.is_visible());
        subs.iter()
            .filter(|sub| {
                sub.anchor != *hovered
                    && !widget_within(hovered_widget, sub.popover.upcast_ref::<gtk::Widget>())
            })
            .map(|sub| sub.popover.clone())
            .collect()
    };
    // 由深到浅收：后打开的子层先 popdown，父层收起时才不会留下未发 closed 的子层。
    for p in targets.into_iter().rev() {
        p.popdown(); // 同步派发 closed → 除名；此刻不得持有 subs 借用
    }
}

/// `widget` 是否为 `ancestor` 的后代（含自身）。
/// 自走 parent 链而不调用 `gtk_widget_is_ancestor`：GTK 对空 / 已脱离的
/// widget 会打印 CRITICAL，而这里允许拿到旧会话遗留的锚行。
fn widget_within(widget: &gtk::Widget, ancestor: &gtk::Widget) -> bool {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        if &w == ancestor {
            return true;
        }
        current = w.parent();
    }
    false
}

/// 行所属的弹窗（沿 parent 链找最近一层 GtkPopover）。
fn containing_popover(widget: &gtk::Widget) -> Option<gtk::Popover> {
    let mut current = widget.parent();
    while let Some(w) = current {
        if let Ok(popover) = w.clone().downcast::<gtk::Popover>() {
            return Some(popover);
        }
        current = w.parent();
    }
    None
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
