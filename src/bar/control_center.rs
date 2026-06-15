use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib::{self, SignalHandlerId};
use gtk::prelude::*;

use astal_tray::prelude::{TrayExt, TrayItemExt};
use astal_tray::{Tray, TrayItem};

#[allow(unused)]
pub struct ControlCenter {
    pub container: gtk::Box,
    tray_box: gtk::Box,
    clock: gtk::Label,
    _items: Rc<RefCell<HashMap<String, gtk::Image>>>,
    _tray: Tray,
    _handlers: Rc<RefCell<Vec<SignalHandlerId>>>,
}

impl ControlCenter {
    pub fn new() -> Self {
        let tray = Tray::default();
        let items = Rc::new(RefCell::new(HashMap::new()));
        let handlers = Rc::new(RefCell::new(Vec::new()));

        let tray_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);

        // initial tray items
        for item in tray.items() {
            let icon = make_tray_icon(&item, &handlers);
            tray_box.append(&icon);
            items.borrow_mut().insert(item.item_id().to_string(), icon);
        }

        // item added
        {
            let tray_box = tray_box.clone();
            let items = items.clone();
            let handlers = handlers.clone();
            let tray_ref = tray.clone();
            tray.connect_item_added(move |_tray, item_id| {
                let item = tray_ref.item(item_id);
                let icon = make_tray_icon(&item, &handlers);
                tray_box.append(&icon);
                items.borrow_mut().insert(item_id.to_string(), icon);
            });
        }

        // item removed
        {
            let tray_box = tray_box.clone();
            let items = items.clone();
            tray.connect_item_removed(move |_tray, item_id| {
                if let Some(icon) = items.borrow_mut().remove(item_id) {
                    tray_box.remove(&icon);
                }
            });
        }

        // clock
        let clock = gtk::Label::new(Some(&format_time()));
        clock.add_css_class("clock-label");

        {
            let clock = clock.clone();
            glib::timeout_add_seconds_local(1, move || {
                clock.set_text(&format_time());
                glib::ControlFlow::Continue
            });
        }

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        container.append(&tray_box);
        container.append(&clock);

        ControlCenter {
            container,
            tray_box,
            clock,
            _items: items,
            _tray: tray,
            _handlers: handlers,
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────

fn make_tray_icon(item: &TrayItem, handlers: &Rc<RefCell<Vec<SignalHandlerId>>>) -> gtk::Image {
    let icon = gtk::Image::from_gicon(&item.gicon());
    icon.add_css_class("tray-icon");

    // icon changed — tray items update their gicon dynamically
    {
        let icon = icon.clone();
        let item = item.clone();
        let h = item.connect_gicon_notify(move |item| {
            icon.set_from_gicon(&item.gicon());
        });
        handlers.borrow_mut().push(h);
    }

    // left click
    {
        let item = item.clone();
        let left = gtk::GestureClick::new();
        left.set_button(1);
        left.connect_pressed(move |_gesture, _n, x, y| {
            item.activate(x as i32, y as i32);
        });
        icon.add_controller(left);
    }

    // right click — show menu or secondary activate
    {
        let item = item.clone();
        let right = gtk::GestureClick::new();
        right.set_button(3);
        let icon_weak = icon.clone();
        right.connect_pressed(move |_gesture, _n, x, y| {
            if let Some(menu) = item.menu_model() {
                item.about_to_show();

                let popover = gtk::PopoverMenu::from_model(Some(&menu));
                popover.set_parent(&icon_weak);

                if let Some(ag) = item.action_group() {
                    popover.insert_action_group("dbusmenu", Some(&ag));
                }

                popover.popup();
            } else {
                item.secondary_activate(x as i32, y as i32);
            }
        });
        icon.add_controller(right);
    }

    icon
}

fn format_time() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%H:%M"))
        .unwrap_or_default()
        .to_string()
}
