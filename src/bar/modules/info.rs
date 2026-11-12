//! 活动窗口图标 / 标题 + 通知中心 popover + 贴栏通知窗口 + 动作激活令牌。
//! 几何、生命周期与交互契约见 USER.html §5.2.5–§5.2.6 与 CORE.html §4.13。

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::bar::ipc::IpcClient;
use crate::bar::modules::popup::{self, PopupManager};
use crate::shared::model::{
    ActiveWindow, NotificationAction, NotificationIcon, NotificationRecord, NotificationStatus,
    NotificationUrgency,
};
use crate::shared::protocol::{NotificationDismissReason, Request};

pub const POPUP_VISIBLE_MAX: usize = 10;
pub const GROUP_PREVIEW_COUNT: usize = 3;
pub const POPUP_LOW_TIMEOUT_MS: u64 = 30_000;

const POPUP_WIDTH: i32 = 300;
/// 贴栏通知与任务栏底边的空白（层壳 top margin，相对任务栏让出的可用区）。
const POPUP_TOP_GAP: i32 = 5;

type RetryCallback = Rc<dyn Fn()>;
type RetrySlot = Rc<RefCell<Option<RetryCallback>>>;

pub struct InfoCenterWidget {
    pub container: gtk::Box,
    ipc: Arc<IpcClient>,
    icon: gtk::Image,
    title_label: gtk::Label,
    #[allow(dead_code)]
    center: gtk::Popover,
    center_content: gtk::Box,
    center_scroll: gtk::ScrolledWindow,
    center_max_height: i32,
    records: Rc<RefCell<Vec<NotificationRecord>>>,
    status: Rc<RefCell<NotificationStatus>>,
    retry: RetrySlot,
    collapsed: Rc<RefCell<HashSet<String>>>,
    expanded: Rc<RefCell<Option<String>>>,
    notification_window: Rc<NotificationWindow>,
    #[allow(dead_code)]
    popup_manager: Rc<PopupManager>,
}

impl InfoCenterWidget {
    pub fn new(
        ipc: Arc<IpcClient>,
        monitor: &str,
        window: &gtk::Window,
        popup: Rc<PopupManager>,
    ) -> Self {
        let icon = gtk::Image::new();
        icon.set_pixel_size(16);
        icon.add_css_class("title-icon");

        let title_label = gtk::Label::new(None);
        title_label.add_css_class("title-label");
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_label.set_max_width_chars(40);

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        container.add_css_class("title-box");
        container.append(&icon);
        container.append(&title_label);

        let records = Rc::new(RefCell::new(Vec::new()));
        let status = Rc::new(RefCell::new(NotificationStatus::Ready));
        let retry = Rc::new(RefCell::new(None));
        let collapsed = Rc::new(RefCell::new(HashSet::new()));
        let expanded = Rc::new(RefCell::new(None));

        let notification_window = NotificationWindow::new(ipc.clone(), monitor, window, &container);

        // ── 通知中心 popover ──
        let center_content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        center_content.add_css_class("nc-panel");
        let center_max_height = screen_height(window).max(240);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&center_content));
        scrolled.set_max_content_height(center_max_height);
        scrolled.set_propagate_natural_height(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let center = gtk::Popover::new();
        center.add_css_class("nc-popover");
        center.set_has_arrow(false);
        center.set_child(Some(&scrolled));
        let width = (screen_width(window) as f64 * 0.4).round() as i32;
        center.set_size_request(width.max(280), -1);
        popup::attach(
            &center,
            window,
            gtk::PositionType::Bottom,
            gtk::Align::Center,
            gtk::Align::Fill,
        );
        popup.manage(&center);
        {
            let nw = notification_window.clone();
            center.connect_closed(move |_| nw.set_center_open(false));
        }

        // 标题区域点击：toggle 通知中心。
        {
            let center = center.clone();
            let manager = popup.clone();
            let nw = notification_window.clone();
            let anchor = container.clone();
            let parent = window.clone();
            let click = gtk::GestureClick::new();
            click.connect_pressed(move |_g, _n, _x, _y| {
                if manager.press_closed_contains("info") {
                    return;
                }
                if center.is_visible() {
                    manager.close_group("info");
                    return;
                }
                popup::point_at(&center, &parent, &anchor);
                manager.clone().open(&center, "info".into());
                nw.set_center_open(true);
            });
            container.add_controller(click);
        }

        let widget = Self {
            container,
            ipc: ipc.clone(),
            icon,
            title_label,
            center,
            center_content,
            center_scroll: scrolled.clone(),
            center_max_height,
            records,
            status,
            retry,
            collapsed,
            expanded,
            notification_window,
            popup_manager: popup,
        };
        widget.rebuild_center();
        widget
    }

    pub fn set_active(&self, active: &ActiveWindow) {
        self.title_label.set_text(&active.title);
        self.title_label
            .set_tooltip_text((!active.title.is_empty()).then_some(active.title.as_str()));
        set_image_from_theme(&self.icon, &active.class.to_lowercase(), 32);
    }

    #[allow(dead_code)]
    pub fn toggle_center(&self) {
        if self.center.is_visible() {
            self.popup_manager.close_group("info");
        } else {
            popup::point_at(
                &self.center,
                &self.center.parent().unwrap(),
                &self.container,
            );
            self.popup_manager.clone().open(&self.center, "info".into());
            self.notification_window.set_center_open(true);
        }
    }

    pub fn set_notification_status(&self, status: &NotificationStatus) {
        *self.status.borrow_mut() = status.clone();
        self.notification_window.set_status(status);
        self.rebuild_center();
    }

    pub fn set_records(&self, records: &[NotificationRecord]) {
        *self.records.borrow_mut() = records.to_vec();
        self.rebuild_center();
    }

    pub fn upsert_center(&self, record: NotificationRecord) {
        {
            let mut records = self.records.borrow_mut();
            records.retain(|item| item.id != record.id);
            records.insert(0, record);
        }
        self.rebuild_center();
    }

    pub fn remove_notification(&self, id: u32) {
        self.records.borrow_mut().retain(|item| item.id != id);
        self.notification_window.remove(id);
        self.rebuild_center();
    }

    pub fn clear_notifications(&self, app_name: Option<&str>) {
        match app_name {
            Some(app) => self
                .records
                .borrow_mut()
                .retain(|item| item.app_name != app),
            None => self.records.borrow_mut().clear(),
        }
        self.notification_window.clear(app_name);
        self.rebuild_center();
    }

    pub fn push_popup(&self, record: NotificationRecord) {
        self.notification_window.push(record);
    }

    /// dismiss：只收贴栏卡片，通知中心条目保留到清理。
    pub fn dismiss_popup(&self, id: u32) {
        self.notification_window.remove(id);
    }

    pub fn set_notification_retry(&self, retry: RetryCallback) {
        *self.retry.borrow_mut() = Some(retry);
        self.rebuild_center();
    }

    #[allow(dead_code)]
    fn rebuild_center(&self) {
        CenterView {
            content: self.center_content.clone(),
            popover: self.center.clone(),
            scroll: self.center_scroll.clone(),
            max_height: self.center_max_height,
            records: self.records.clone(),
            status: self.status.clone(),
            retry: self.retry.clone(),
            collapsed: self.collapsed.clone(),
            expanded: self.expanded.clone(),
            ipc: self.ipc.clone(),
        }
        .rebuild();
    }
}

/// 通知中心视图状态：分组、收缩与展开都只作用于本地视图。
#[derive(Clone)]
struct CenterView {
    content: gtk::Box,
    popover: gtk::Popover,
    scroll: gtk::ScrolledWindow,
    max_height: i32,
    records: Rc<RefCell<Vec<NotificationRecord>>>,
    status: Rc<RefCell<NotificationStatus>>,
    retry: RetrySlot,
    collapsed: Rc<RefCell<HashSet<String>>>,
    expanded: Rc<RefCell<Option<String>>>,
    ipc: Arc<IpcClient>,
}

impl CenterView {
    fn rebuild(&self) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }

        if let NotificationStatus::Unavailable { message } = &*self.status.borrow() {
            self.content.append(&self.unavailable_panel(message));
            self.fit_height();
            return;
        }

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let clear = gtk::Button::with_label("全部清理");
        clear.add_css_class("nc-clear");
        clear.set_focusable(false);
        let ipc = self.ipc.clone();
        clear.connect_clicked(move |_| {
            send_request(&ipc, Request::NotificationClear);
        });
        header.append(&spacer);
        header.append(&clear);
        self.content.append(&header);

        let records = self.records.borrow().clone();
        if records.is_empty() {
            let empty = gtk::Label::new(Some("暂无通知"));
            empty.add_css_class("nc-empty");
            self.content.append(&empty);
            self.fit_height();
            return;
        }

        let expanded_now = self.expanded.borrow().clone();
        for (app, items) in group_records(&records) {
            if let Some(expanded) = &expanded_now
                && expanded != &app
            {
                continue;
            }
            let is_expanded = expanded_now.as_deref() == Some(app.as_str());
            let open = !self.collapsed.borrow().contains(&app);
            self.content.append(&self.group_header(&app, open));
            if !open {
                continue;
            }
            let limit = if is_expanded {
                items.len()
            } else {
                GROUP_PREVIEW_COUNT
            };
            for record in items.iter().take(limit) {
                self.content
                    .append(&notification_card(&self.ipc, record, true));
            }
            if items.len() > limit {
                let more = gtk::Button::with_label(&format!("… 还有 {} 条", items.len() - limit));
                more.add_css_class("nc-more");
                more.set_focusable(false);
                let view = self.clone();
                let app = app.clone();
                more.connect_clicked(move |_| {
                    *view.expanded.borrow_mut() = Some(app.clone());
                    view.rebuild();
                });
                self.content.append(&more);
            }
        }

        // 分组收起/展开会改变内容高度；显式测量并同步滚动区高度，让 popover 跟随收缩/增高。
        self.fit_height();
    }

    /// 内容自然高度 → 滚动区最小高度（上限 100% 屏高）。
    /// 仅靠 `propagate_natural_height` 时 popover 在可见状态下可能保持旧高度；
    /// 重新 present 才会按新的自然尺寸更新弹窗表面。
    fn fit_height(&self) {
        let (_, natural, _, _) = self.content.measure(gtk::Orientation::Vertical, -1);
        self.scroll
            .set_min_content_height(natural.min(self.max_height));
        self.scroll.queue_resize();
        if self.popover.is_visible() {
            self.popover.present();
        }
    }

    fn group_header(&self, app: &str, open: bool) -> gtk::Box {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("nc-group");
        let arrow = gtk::Label::new(Some(if open { "▾" } else { "▸" }));
        arrow.add_css_class("nc-arrow");
        let name = gtk::Label::new(Some(app));
        name.add_css_class("nc-app");
        name.set_hexpand(true);
        name.set_xalign(0.0);
        let close = gtk::Button::with_label("✕");
        close.add_css_class("nc-group-close");
        close.set_focusable(false);

        let ipc = self.ipc.clone();
        let app_name = app.to_string();
        close.connect_clicked(move |_| {
            send_request(
                &ipc,
                Request::NotificationClearGroup {
                    app_name: app_name.clone(),
                },
            );
        });

        let view = self.clone();
        let app_name = app.to_string();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_g, _n, _x, _y| {
            if view.expanded.borrow().as_deref() == Some(app_name.as_str()) {
                *view.expanded.borrow_mut() = None;
                view.collapsed.borrow_mut().insert(app_name.clone());
            } else {
                let mut set = view.collapsed.borrow_mut();
                if !set.remove(&app_name) {
                    set.insert(app_name.clone());
                }
            }
            view.rebuild();
        });
        header.add_controller(click);

        header.append(&arrow);
        header.append(&name);
        header.append(&close);
        header
    }

    fn unavailable_panel(&self, message: &str) -> gtk::Box {
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
        panel.add_css_class("nc-unavail");
        panel.set_halign(gtk::Align::Center);
        panel.set_valign(gtk::Align::Center);
        let icon = gtk::Label::new(Some("!"));
        icon.add_css_class("nc-unavail-ic");
        icon.set_halign(gtk::Align::Center);
        let title = gtk::Label::new(Some("通知中心暂时不可用"));
        title.add_css_class("nc-unavail-t");
        title.set_halign(gtk::Align::Center);
        let body = gtk::Label::new(Some(message));
        body.add_css_class("nc-unavail-b");
        body.set_wrap(true);
        body.set_justify(gtk::Justification::Center);
        body.set_xalign(0.5);
        let retry = gtk::Button::with_label("↻ 重试");
        retry.add_css_class("nc-retry");
        retry.set_focusable(false);
        retry.set_halign(gtk::Align::Center);
        let callback = self.retry.borrow().clone();
        retry.connect_clicked(move |_| {
            if let Some(callback) = &callback {
                callback();
            }
        });
        panel.append(&icon);
        panel.append(&title);
        panel.append(&body);
        panel.append(&retry);
        panel
    }
}

struct NotificationWindow {
    window: gtk::Window,
    container: gtk::Box,
    records: RefCell<Vec<NotificationRecord>>,
    timers: RefCell<HashMap<u32, glib::SourceId>>,
    anchor: gtk::Widget,
    bar_window: gtk::Window,
    center_open: Cell<bool>,
    status: RefCell<NotificationStatus>,
    ipc: Arc<IpcClient>,
}

impl NotificationWindow {
    fn new(
        ipc: Arc<IpcClient>,
        monitor: &str,
        bar_window: &gtk::Window,
        anchor: &impl IsA<gtk::Widget>,
    ) -> Rc<Self> {
        let window = gtk::Window::new();
        window.add_css_class("bread-notify");
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_namespace(Some("bread-notify"));
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_exclusive_zone(0);
        window.set_default_size(POPUP_WIDTH, -1);
        window.set_margin(Edge::Top, POPUP_TOP_GAP);
        window.set_resizable(false);

        if let Some(monitor) = find_monitor(monitor) {
            window.set_monitor(Some(&monitor));
        }

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("notif-stack");
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&container));
        scrolled.set_max_content_height(screen_height(bar_window));
        scrolled.set_propagate_natural_height(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        window.set_child(Some(&scrolled));

        Rc::new(Self {
            window,
            container,
            records: RefCell::new(Vec::new()),
            timers: RefCell::new(HashMap::new()),
            anchor: anchor.clone().upcast(),
            bar_window: bar_window.clone(),
            center_open: Cell::new(false),
            status: RefCell::new(NotificationStatus::Ready),
            ipc,
        })
    }

    fn push(self: &Rc<Self>, record: NotificationRecord) {
        let id = record.id;
        if record.dismissed {
            self.remove(id);
            return;
        }
        {
            let mut records = self.records.borrow_mut();
            if let Some(existing) = records.iter_mut().find(|item| item.id == id) {
                *existing = record;
            } else {
                records.insert(0, record);
            }
        }
        self.reset_timer(id);
        self.rebuild();
        self.show();
    }

    fn remove(&self, id: u32) {
        if let Some(timer) = self.timers.borrow_mut().remove(&id) {
            timer.remove();
        }
        self.records.borrow_mut().retain(|item| item.id != id);
        self.rebuild();
        self.show();
    }

    fn clear(&self, app_name: Option<&str>) {
        let ids: Vec<u32> = {
            let records = self.records.borrow();
            records
                .iter()
                .filter(|item| app_name.is_none_or(|app| item.app_name == app))
                .map(|item| item.id)
                .collect()
        };
        for id in ids {
            if let Some(timer) = self.timers.borrow_mut().remove(&id) {
                timer.remove();
            }
        }
        match app_name {
            Some(app) => self
                .records
                .borrow_mut()
                .retain(|item| item.app_name != app),
            None => self.records.borrow_mut().clear(),
        }
        self.rebuild();
        self.show();
    }

    fn reset_timer(self: &Rc<Self>, id: u32) {
        if let Some(timer) = self.timers.borrow_mut().remove(&id) {
            timer.remove();
        }
        let Some(record) = self
            .records
            .borrow()
            .iter()
            .find(|item| item.id == id)
            .cloned()
        else {
            return;
        };
        let timeout_ms = match record.expire_timeout_ms {
            Some(0) => return,
            Some(ms) => ms as u64,
            None if record.urgency == NotificationUrgency::Low => POPUP_LOW_TIMEOUT_MS,
            None => return,
        };
        let weak: Weak<Self> = Rc::downgrade(self);
        let source =
            glib::timeout_add_local_once(std::time::Duration::from_millis(timeout_ms), move || {
                if let Some(this) = weak.upgrade() {
                    this.expire(id);
                }
            });
        self.timers.borrow_mut().insert(id, source);
    }

    fn expire(&self, id: u32) {
        self.timers.borrow_mut().remove(&id);
        self.records.borrow_mut().retain(|item| item.id != id);
        self.rebuild();
        self.show();
        send_request(
            &self.ipc,
            Request::NotificationDismiss {
                id,
                reason: NotificationDismissReason::Expired,
            },
        );
    }

    fn set_status(&self, status: &NotificationStatus) {
        *self.status.borrow_mut() = status.clone();
        match status {
            NotificationStatus::Ready => self.show(),
            NotificationStatus::Unavailable { .. } => self.hide(),
        }
    }

    fn set_center_open(&self, open: bool) {
        self.center_open.set(open);
        if open {
            self.hide();
        } else {
            self.show();
        }
    }

    fn show(&self) {
        if self.center_open.get() || self.records.borrow().is_empty() {
            self.hide();
            return;
        }
        if matches!(
            &*self.status.borrow(),
            NotificationStatus::Unavailable { .. }
        ) {
            self.hide();
            return;
        }
        if let Some(bounds) = self.anchor.compute_bounds(&self.bar_window) {
            let center = bounds.x() + bounds.width() / 2.0;
            let left = (center - POPUP_WIDTH as f32 / 2.0).round() as i32;
            self.window.set_margin(Edge::Left, left.max(0));
        }
        if !self.window.is_visible() {
            self.window.present();
        }
    }

    fn hide(&self) {
        self.window.set_visible(false);
    }

    fn rebuild(&self) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        let records = self.records.borrow();
        let active: Vec<&NotificationRecord> =
            records.iter().filter(|record| !record.dismissed).collect();
        for record in active.iter().take(POPUP_VISIBLE_MAX) {
            self.container
                .append(&notification_card(&self.ipc, record, false));
        }
        if active.len() > POPUP_VISIBLE_MAX {
            let hidden = gtk::Label::new(Some(&format!(
                "还有 {} 条通知",
                active.len() - POPUP_VISIBLE_MAX
            )));
            hidden.add_css_class("notif-hidden");
            self.container.append(&hidden);
        }
    }
}

fn group_records(records: &[NotificationRecord]) -> Vec<(String, Vec<NotificationRecord>)> {
    let mut groups: Vec<(String, Vec<NotificationRecord>)> = Vec::new();
    for record in records {
        if let Some((_, items)) = groups.iter_mut().find(|(app, _)| app == &record.app_name) {
            items.push(record.clone());
        } else {
            groups.push((record.app_name.clone(), vec![record.clone()]));
        }
    }
    groups
}

fn notification_card(ipc: &Arc<IpcClient>, record: &NotificationRecord, center: bool) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("notif-card");
    card.set_hexpand(true);
    if record.urgency == NotificationUrgency::Critical {
        card.add_css_class("crit");
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if center {
        let title = gtk::Label::new(None);
        set_label_text(&title, &record.summary, true);
        title.add_css_class("notif-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(36);
        title.set_tooltip_text(Some(&record.summary));
        header.append(&title);
    } else {
        let app = gtk::Label::new(Some(&record.app_name));
        app.add_css_class("notif-app");
        app.set_hexpand(true);
        app.set_xalign(0.0);
        header.append(&app);
    }

    let close = gtk::Button::with_label("✕");
    close.add_css_class("notif-close");
    close.set_focusable(false);
    {
        let ipc = ipc.clone();
        let id = record.id;
        close.connect_clicked(move |_| {
            let request = if center {
                Request::NotificationRemove { id }
            } else {
                Request::NotificationDismiss {
                    id,
                    reason: NotificationDismissReason::Dismissed,
                }
            };
            send_request(&ipc, request);
        });
    }
    header.append(&close);
    card.append(&header);

    if !center {
        let title = gtk::Label::new(None);
        set_label_text(&title, &record.summary, true);
        title.add_css_class("notif-title");
        title.set_justify(gtk::Justification::Center);
        title.set_xalign(0.5);
        title.set_lines(2);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        title.set_tooltip_text(Some(&record.summary));
        card.append(&title);
    }

    let body_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    if let Some(image) = notification_image(&record.icon, 32) {
        image.set_valign(gtk::Align::Start);
        body_row.append(&image);
    }
    let body = notification_body(&record.body, center);
    body.set_hexpand(true);
    body.set_tooltip_text((!record.body.is_empty()).then_some(record.body.as_str()));
    if !record.body.is_empty() {
        body_row.append(&body);
    } else {
        body_row.append(&gtk::Box::new(gtk::Orientation::Horizontal, 0));
    }
    card.append(&body_row);

    if !record.actions.is_empty() {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.add_css_class("notif-actions");
        for action in &record.actions {
            actions.append(&action_button(ipc, record, action));
        }
        card.append(&actions);
    }

    // 卡片点击：左键默认动作 / 中键 dismiss / 弹窗右键全部 dismiss。
    for button in [1u32, 2, 3] {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(button);
        let ipc = ipc.clone();
        let id = record.id;
        gesture.connect_pressed(move |_g, _n, _x, _y| match button {
            2 => {
                let request = if center {
                    Request::NotificationRemove { id }
                } else {
                    Request::NotificationDismiss {
                        id,
                        reason: NotificationDismissReason::Dismissed,
                    }
                };
                send_request(&ipc, request);
            }
            3 if !center => send_request(&ipc, Request::NotificationDismissAll),
            1 => send_request(
                &ipc,
                Request::NotificationActivate {
                    id,
                    action_key: None,
                    activation_token: activation_token(),
                },
            ),
            _ => {}
        });
        card.add_controller(gesture);
    }

    if !center {
        card.set_valign(gtk::Align::Start);
    }
    card
}

fn action_button(
    ipc: &Arc<IpcClient>,
    record: &NotificationRecord,
    action: &NotificationAction,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("notif-action");
    button.set_focusable(false);
    let image = record
        .action_icons
        .then(|| theme_image(&action.key, 16))
        .flatten();
    if let Some(image) = image {
        button.set_child(Some(&image));
        button.set_tooltip_text(Some(&action.label));
        button.update_property(&[gtk::accessible::Property::Label(&action.label)]);
    } else {
        button.set_label(&action.label);
    }
    let ipc = ipc.clone();
    let id = record.id;
    let key = action.key.clone();
    button.connect_clicked(move |_| {
        send_request(
            &ipc,
            Request::NotificationActivate {
                id,
                action_key: Some(key.clone()),
                activation_token: activation_token(),
            },
        );
    });
    button
}

fn send_request(ipc: &Arc<IpcClient>, request: Request) {
    let ipc = ipc.clone();
    std::thread::spawn(move || {
        if let Err(e) = ipc.request(request) {
            eprintln!("[info] request failed: {e}");
        }
    });
}

/// 动作点击时现场生成激活令牌；不缓存、不复用。
fn activation_token() -> Option<String> {
    let display = gtk::gdk::Display::default()?;
    let context = display.app_launch_context();
    context
        .startup_notify_id(None::<&gio::AppInfo>, &[])
        .map(|id| id.to_string())
}

fn set_label_text(label: &gtk::Label, text: &str, markup: bool) {
    if markup && gtk::pango::parse_markup(text, '\0').is_ok() {
        label.set_markup(text);
    } else {
        label.set_text(text);
    }
}

fn set_body_text(label: &gtk::Label, text: &str) {
    set_label_text(label, text, true);
    label.connect_activate_link(|_label, uri| {
        if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:") {
            let _ = std::process::Command::new("xdg-open").arg(uri).spawn();
        }
        glib::Propagation::Stop
    });
}

fn notification_image(icon: &NotificationIcon, size: i32) -> Option<gtk::Image> {
    match icon {
        NotificationIcon::None => None,
        NotificationIcon::Name(name) => {
            if let Some(path) = name.strip_prefix("file://") {
                file_image(path, size)
            } else {
                theme_image(name, size)
            }
        }
        NotificationIcon::Pixels {
            width,
            height,
            rgba,
        } => {
            if *width <= 0
                || *height <= 0
                || rgba.len() != (*width as usize) * (*height as usize) * 4
            {
                return None;
            }
            let bytes = glib::Bytes::from(rgba);
            let texture = gtk::gdk::MemoryTexture::new(
                *width,
                *height,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (*width as usize) * 4,
            );
            let image = gtk::Image::from_paintable(Some(&texture));
            image.set_pixel_size(size);
            Some(image)
        }
    }
}

fn theme_image(name: &str, size: i32) -> Option<gtk::Image> {
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    if !theme.has_icon(name) {
        return None;
    }
    let paintable = theme.lookup_icon(
        name,
        &[],
        size,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    let image = gtk::Image::from_paintable(Some(&paintable));
    image.set_pixel_size(size);
    Some(image)
}

enum BodySegment {
    Text(String),
    Image { src: String, alt: String },
}

/// 正文渲染：纯文本走单个标签；含本地图片时按段落竖向排列。
fn notification_body(body: &str, center: bool) -> gtk::Widget {
    if !body.contains("<img") {
        return body_label(body, center).upcast();
    }
    let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
    container.add_css_class("notif-body");
    for segment in split_body(body) {
        match segment {
            BodySegment::Text(text) => {
                if !text.trim().is_empty() {
                    container.append(&body_label(&text, center));
                }
            }
            BodySegment::Image { src, alt } => {
                if let Some(picture) = body_image(&src) {
                    container.append(&picture);
                } else if !alt.is_empty() {
                    container.append(&body_label(&alt, center));
                }
            }
        }
    }
    container.upcast()
}

fn body_label(text: &str, center: bool) -> gtk::Label {
    let label = gtk::Label::new(None);
    set_body_text(&label, text);
    label.add_css_class("notif-body");
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_lines(if center { 2 } else { 8 });
    label.set_ellipsize(if center {
        gtk::pango::EllipsizeMode::End
    } else {
        gtk::pango::EllipsizeMode::Middle
    });
    label
}

fn split_body(body: &str) -> Vec<BodySegment> {
    let mut segments = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<img") {
        if start > 0 {
            segments.push(BodySegment::Text(rest[..start].to_string()));
        }
        let Some(end) = rest[start..].find('>').map(|offset| start + offset + 1) else {
            segments.push(BodySegment::Text(rest[start..].to_string()));
            return segments;
        };
        let tag = &rest[start..end];
        let src = attribute(tag, "src").unwrap_or_default();
        let alt = attribute(tag, "alt").unwrap_or_default();
        segments.push(BodySegment::Image { src, alt });
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        segments.push(BodySegment::Text(rest.to_string()));
    }
    segments
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let pattern = format!("{name}=");
    let start = tag.find(&pattern)? + pattern.len();
    let rest = tag[start..].trim_start();
    let (quote, rest) = match rest.chars().next()? {
        '"' => ('"', &rest[1..]),
        '\'' => ('\'', &rest[1..]),
        _ => return None,
    };
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// body-images 只接受本地 file:// 与图标主题名，不访问网络。
fn body_image(src: &str) -> Option<gtk::Picture> {
    if src.starts_with("http://") || src.starts_with("https://") {
        return None;
    }
    let paintable = if let Some(path) = src.strip_prefix("file://") {
        let texture = gtk::gdk::Texture::from_filename(path).ok()?;
        texture.upcast::<gtk::gdk::Paintable>()
    } else {
        let display = gtk::gdk::Display::default()?;
        let theme = gtk::IconTheme::for_display(&display);
        if !theme.has_icon(src) {
            return None;
        }
        theme
            .lookup_icon(
                src,
                &[],
                128,
                1,
                gtk::TextDirection::Ltr,
                gtk::IconLookupFlags::empty(),
            )
            .upcast::<gtk::gdk::Paintable>()
    };
    let picture = gtk::Picture::for_paintable(&paintable);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_size_request(0, -1);
    Some(picture)
}

fn file_image(path: &str, size: i32) -> Option<gtk::Image> {
    let texture = gtk::gdk::Texture::from_filename(path).ok()?;
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(size);
    Some(image)
}

fn set_image_from_theme(image: &gtk::Image, name: &str, size: i32) {
    if let Some(fresh) = theme_image(name, size) {
        image.set_paintable(fresh.paintable().as_ref());
        image.set_visible(true);
    } else {
        image.clear();
        image.set_visible(false);
    }
}

fn monitor_size(window: &gtk::Window) -> (i32, i32) {
    let Some(display) = gtk::gdk::Display::default() else {
        return (1920, 1080);
    };
    let monitor = window
        .surface()
        .and_then(|surface| display.monitor_at_surface(&surface))
        .or_else(|| {
            display
                .monitors()
                .item(0)
                .and_then(|item| item.downcast::<gtk::gdk::Monitor>().ok())
        });
    monitor
        .map(|monitor| (monitor.geometry().width(), monitor.geometry().height()))
        .unwrap_or((1920, 1080))
}

fn screen_width(window: &gtk::Window) -> i32 {
    monitor_size(window).0
}

fn screen_height(window: &gtk::Window) -> i32 {
    monitor_size(window).1
}

fn find_monitor(name: &str) -> Option<gtk::gdk::Monitor> {
    let display = gtk::gdk::Display::default()?;
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        let monitor = monitors.item(i)?.downcast::<gtk::gdk::Monitor>().ok()?;
        if monitor.connector().as_deref() == Some(name) {
            return Some(monitor);
        }
    }
    None
}
