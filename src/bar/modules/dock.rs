//! 最小化托盘：底部触发条 + 窗口条目（独立 layer-shell，无 grab）。
//! 交互与几何契约见 USER.html §5.2.7 与 CORE.html §4.14。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::bar::ipc::IpcClient;
use crate::shared::model::MinimizedWindow;
use crate::shared::protocol::Request;

pub const DOCK_TRIGGER_HEIGHT: i32 = 2;
pub const DOCK_HIDE_DELAY_MS: u64 = 300;
pub const DOCK_ITEM_WIDTH: i32 = 160;
pub const DOCK_MAX_WIDTH_RATIO: f64 = 0.88;

const DOCK_PANEL_HEIGHT: i32 = 64;
const DOCK_PADDING: i32 = 12;

pub struct DockWidget {
    trigger: gtk::Window,
    panel: gtk::Window,
    items: gtk::Box,
    monitor: String,
    ipc: Arc<IpcClient>,
    data: RefCell<Vec<MinimizedWindow>>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    screen_width: Cell<i32>,
    labels: RefCell<HashMap<String, gtk::Label>>,
}

impl DockWidget {
    pub fn new(ipc: Arc<IpcClient>, monitor: &str) -> Rc<Self> {
        let screen_width = find_monitor(monitor)
            .map(|m| m.geometry().width())
            .unwrap_or(1920);

        // 触发条：2px 全宽，常驻底边。
        let trigger = gtk::Window::new();
        trigger.add_css_class("bread-dock-trigger");
        trigger.init_layer_shell();
        trigger.set_layer(Layer::Top);
        trigger.set_anchor(Edge::Bottom, true);
        trigger.set_anchor(Edge::Left, true);
        trigger.set_anchor(Edge::Right, true);
        trigger.set_namespace(Some("bread-dock-trigger"));
        trigger.set_keyboard_mode(KeyboardMode::None);
        trigger.set_exclusive_zone(0);
        trigger.set_default_size(-1, DOCK_TRIGGER_HEIGHT);
        trigger.set_resizable(false);
        trigger.set_decorated(false);
        if let Some(monitor) = find_monitor(monitor) {
            trigger.set_monitor(Some(&monitor));
        }

        // 面板：动态宽度、水平居中、底边距 8px。
        let panel = gtk::Window::new();
        panel.add_css_class("bread-dock");
        panel.init_layer_shell();
        panel.set_layer(Layer::Top);
        panel.set_anchor(Edge::Bottom, true);
        panel.set_anchor(Edge::Left, true);
        panel.set_namespace(Some("bread-dock"));
        panel.set_keyboard_mode(KeyboardMode::None);
        panel.set_exclusive_zone(0);
        panel.set_default_size(DOCK_ITEM_WIDTH + DOCK_PADDING, DOCK_PANEL_HEIGHT);
        panel.set_resizable(false);
        panel.set_decorated(false);
        panel.set_margin(Edge::Bottom, 8);
        if let Some(monitor) = find_monitor(monitor) {
            panel.set_monitor(Some(&monitor));
        }

        let items = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        items.add_css_class("dock-items");
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&items));
        scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        scrolled.set_propagate_natural_width(true);
        scrolled.add_css_class("dock-scroll");
        panel.set_child(Some(&scrolled));

        let widget = Rc::new(Self {
            trigger,
            panel,
            items,
            monitor: monitor.to_string(),
            ipc,
            data: RefCell::new(Vec::new()),
            hide_timer: RefCell::new(None),
            screen_width: Cell::new(screen_width),
            labels: RefCell::new(HashMap::new()),
        });

        widget.install_hover();
        widget.trigger.set_visible(false);
        widget.panel.set_visible(false);
        widget
    }

    fn install_hover(self: &Rc<Self>) {
        {
            let enter = self.clone();
            let leave = self.clone();
            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter(move |_c, _x, _y| {
                enter.cancel_hide();
                enter.show_panel();
            });
            motion.connect_leave(move |_c| leave.schedule_hide());
            self.trigger.add_controller(motion);
        }
        {
            let enter = self.clone();
            let leave = self.clone();
            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter(move |_c, _x, _y| {
                enter.cancel_hide();
                enter.show_panel();
            });
            motion.connect_leave(move |_c| leave.schedule_hide());
            self.panel.add_controller(motion);
        }
    }

    pub fn set_items(self: &Rc<Self>, items: &[MinimizedWindow]) {
        let filtered: Vec<MinimizedWindow> = items
            .iter()
            .filter(|item| item.monitor == self.monitor)
            .cloned()
            .collect();
        let same_layout = {
            let old = self.data.borrow();
            old.len() == filtered.len()
                && old
                    .iter()
                    .zip(filtered.iter())
                    .all(|(a, b)| a.address == b.address)
        };
        if same_layout {
            // 标题 / 图标类变化原位更新，只有增删与顺序变化才重建。
            for (old, new) in self.data.borrow().iter().zip(filtered.iter()) {
                if old.title != new.title
                    && let Some(label) = self.labels.borrow().get(&new.address)
                {
                    label.set_text(&new.title);
                }
            }
            *self.data.borrow_mut() = filtered;
            if !self.data.borrow().is_empty() {
                self.trigger.set_visible(true);
            }
            return;
        }
        *self.data.borrow_mut() = filtered;
        self.rebuild();
        if self.data.borrow().is_empty() {
            self.hide_panel();
            self.trigger.set_visible(false);
            self.panel.set_visible(false);
        } else {
            self.trigger.set_visible(true);
        }
    }

    fn rebuild(self: &Rc<Self>) {
        if let Some(width) = find_monitor(&self.monitor).map(|m| m.geometry().width()) {
            self.screen_width.set(width);
        }
        self.labels.borrow_mut().clear();
        while let Some(child) = self.items.first_child() {
            self.items.remove(&child);
        }
        for window in self.data.borrow().iter() {
            self.items.append(&self.build_item(window));
        }
        let count = self.data.borrow().len() as i32;
        let wanted = count * DOCK_ITEM_WIDTH + DOCK_PADDING;
        let max_width = (self.screen_width.get() as f64 * DOCK_MAX_WIDTH_RATIO) as i32;
        let width = wanted.min(max_width).max(DOCK_ITEM_WIDTH);
        self.panel.set_default_size(width, DOCK_PANEL_HEIGHT);
        let left = ((self.screen_width.get() - width) / 2).max(0);
        self.panel.set_margin(Edge::Left, left);
    }

    fn build_item(self: &Rc<Self>, window: &MinimizedWindow) -> gtk::Box {
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        item.add_css_class("dock-item");
        item.set_size_request(DOCK_ITEM_WIDTH - 12, 40);
        let icon = dock_icon(&window.class);
        icon.set_size_request(32, 32);
        item.append(&icon);
        let label = gtk::Label::new(Some(if window.title.trim().is_empty() {
            "未命名窗口"
        } else {
            window.title.trim()
        }));
        label.add_css_class("dock-title");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(18);
        label.set_hexpand(true);
        label.set_xalign(0.0);
        self.labels
            .borrow_mut()
            .insert(window.address.clone(), label.clone());
        item.append(&label);

        let close = gtk::Button::with_label("✕");
        close.add_css_class("dock-close");
        close.set_focusable(false);
        {
            let widget = self.clone();
            let address = window.address.clone();
            close.connect_clicked(move |_| widget.on_close(&address));
        }
        item.append(&close);

        for button in [1u32, 2] {
            let gesture = gtk::GestureClick::new();
            gesture.set_button(button);
            let widget = self.clone();
            let address = window.address.clone();
            gesture.connect_pressed(move |_g, _n, _x, _y| {
                if button == 2 {
                    widget.on_close(&address);
                } else {
                    widget.on_restore(&address);
                }
            });
            item.add_controller(gesture);
        }

        item
    }

    fn on_restore(&self, address: &str) {
        send_request(
            &self.ipc,
            Request::MinimizedRestore {
                address: address.to_string(),
            },
        );
    }

    fn on_close(&self, address: &str) {
        send_request(
            &self.ipc,
            Request::MinimizedClose {
                address: address.to_string(),
            },
        );
    }

    fn show_panel(&self) {
        if self.data.borrow().is_empty() {
            return;
        }
        self.panel.present();
        self.panel.set_visible(true);
    }

    fn hide_panel(&self) {
        self.panel.set_visible(false);
    }

    fn schedule_hide(self: &Rc<Self>) {
        self.cancel_hide();
        let widget = self.clone();
        let source =
            glib::timeout_add_local_once(Duration::from_millis(DOCK_HIDE_DELAY_MS), move || {
                widget.hide_panel();
            });
        *self.hide_timer.borrow_mut() = Some(source);
    }

    fn cancel_hide(&self) {
        if let Some(source) = self.hide_timer.borrow_mut().take() {
            source.remove();
        }
    }
}

fn send_request(ipc: &Arc<IpcClient>, request: Request) {
    let ipc = ipc.clone();
    std::thread::spawn(move || {
        if let Err(e) = ipc.request(request) {
            eprintln!("[dock] request failed: {e}");
        }
    });
}

fn dock_icon(class: &str) -> gtk::Image {
    let name = class.to_lowercase();
    let display = gtk::gdk::Display::default();
    if let Some(display) = display {
        let theme = gtk::IconTheme::for_display(&display);
        let candidates = [name.as_str(), "application-x-executable"];
        for candidate in candidates {
            if candidate.is_empty() || !theme.has_icon(candidate) {
                continue;
            }
            let paintable = theme.lookup_icon(
                candidate,
                &[],
                32,
                1,
                gtk::TextDirection::Ltr,
                gtk::IconLookupFlags::empty(),
            );
            let image = gtk::Image::from_paintable(Some(&paintable));
            image.set_pixel_size(32);
            return image;
        }
    }
    let image = gtk::Image::from_icon_name("application-x-executable");
    image.set_pixel_size(32);
    image
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
