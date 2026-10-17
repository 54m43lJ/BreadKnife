//! 活动窗口图标 / 标题 + 通知中心弹窗。

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::bar::modules::popup::{self, PopupManager};
use crate::shared::model::{ActiveWindow, NotificationRecord};

/// 窗口侧留存的通知条数（弹出列表长度上限）。
const LIST_KEEP: usize = 20;

pub struct WindowInfoWidget {
    pub container: gtk::Box,
    icon: gtk::Image,
    label: gtk::Label,
    records: Rc<RefCell<Vec<NotificationRecord>>>,
    list_box: Rc<gtk::Box>,
}

impl WindowInfoWidget {
    pub fn new(window: &gtk::Window, popup: Rc<PopupManager>) -> Self {
        let icon = gtk::Image::new();
        icon.set_pixel_size(16);
        icon.add_css_class("title-icon");

        let label = gtk::Label::new(None);
        label.add_css_class("title-label");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(40);

        let records = Rc::new(RefCell::new(Vec::new()));

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        container.add_css_class("title-box");

        // 点击标题区域弹出通知列表（面板挂根窗口，不参与容器布局）
        let (list_box, popover) = Self::build_popover();
        popup::attach(
            &popover,
            window,
            gtk::PositionType::Bottom,
            gtk::Align::Fill,
            gtk::Align::Fill,
        );
        popup.manage(&popover);
        let list_box = Rc::new(list_box);
        let click = gtk::GestureClick::new();
        let records_click = records.clone();
        let list_handle = list_box.clone();
        let pop_handle = popover.clone();
        let popup_click = popup.clone();
        let window_handle = window.clone();
        let anchor_handle = container.clone();
        click.connect_pressed(move |_g, _n, _x, _y| {
            Self::fill_list(&list_handle, &records_click.borrow());
            // toggle：本次按下刚关掉自己 → 不重开
            if popup_click.press_closed_contains("notif") {
                return;
            }
            popup::point_at(&pop_handle, &window_handle, &anchor_handle);
            popup_click.clone().open(&pop_handle, "notif".into());
        });
        container.add_controller(click);

        container.append(&icon);
        container.append(&label);

        Self { container, icon, label, records, list_box }
    }

    fn build_popover() -> (gtk::Box, gtk::Popover) {
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&list_box));
        scrolled.set_max_content_height(320);
        scrolled.set_propagate_natural_height(true);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.add_css_class("notif-popover");
        content.append(&scrolled);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        (list_box, popover)
    }

    pub fn set_active(&self, active: &ActiveWindow) {
        self.label.set_text(&active.title);
        // 标题截断显示，悬停看全文（README「界面交互」）
        self.label
            .set_tooltip_text((!active.title.is_empty()).then_some(active.title.as_str()));
        set_image_from_theme(&self.icon, &active.class.to_lowercase());
    }

    pub fn notify(&self, record: NotificationRecord) {
        {
            let mut records = self.records.borrow_mut();
            if let Some(existing) = records.iter_mut().find(|n| n.id == record.id) {
                *existing = record;
            } else {
                records.push(record);
                while records.len() > LIST_KEEP {
                    records.remove(0);
                }
            }
        }
        Self::fill_list(&self.list_box, &self.records.borrow());
    }

    fn fill_list(list: &gtk::Box, records: &[NotificationRecord]) {
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        if records.is_empty() {
            let empty = gtk::Label::new(Some("暂无通知"));
            empty.add_css_class("notif-empty");
            list.append(&empty);
            return;
        }
        for record in records.iter().rev() {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 1);
            row.add_css_class("notif-row");

            let summary = gtk::Label::new(Some(&record.summary));
            summary.add_css_class("notif-summary");
            summary.set_halign(gtk::Align::Start);
            summary.set_ellipsize(gtk::pango::EllipsizeMode::End);

            let body = gtk::Label::new(Some(&record.body));
            body.add_css_class("notif-body");
            body.set_halign(gtk::Align::Start);
            body.set_wrap(true);
            body.set_xalign(0.0);

            let app = gtk::Label::new(Some(&record.app_name));
            app.add_css_class("notif-app");
            app.set_halign(gtk::Align::End);

            row.append(&summary);
            if !record.body.is_empty() {
                row.append(&body);
            }
            row.append(&app);
            list.append(&row);
        }
    }
}

/// 主题命中 → 上屏；未命中 → 隐藏（缺失降级）。
fn set_image_from_theme(image: &gtk::Image, name: &str) {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let theme = gtk::IconTheme::for_display(&display);
    if !theme.has_icon(name) {
        image.clear();
        image.set_visible(false);
        return;
    }
    let paintable = theme.lookup_icon(
        name,
        &[],
        32,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    image.set_paintable(Some(&paintable));
    image.set_visible(true);
}
