use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::glib::{self, ControlFlow, SignalHandlerId};
use gtk::prelude::*;
use gtk::{gdk, gio};
use tray::{IconSource, ItemField, Tray, TrayConfig, TrayEvent, TrayHandle, TrayItemId};

#[allow(unused)]
pub struct ControlCenter {
    pub container: gtk::Box,
    tray_box: gtk::Box,
    clock: gtk::Label,
    items: Rc<RefCell<HashMap<TrayItemId, gtk::Image>>>,
    _handlers: Rc<RefCell<Vec<SignalHandlerId>>>,
    _event_join: glib::JoinHandle<()>,
}

impl ControlCenter {
    pub fn new() -> Self {
        let handle = shared_tray();
        let items: Rc<RefCell<HashMap<TrayItemId, gtk::Image>>> = Rc::new(RefCell::new(HashMap::new()));
        let handlers = Rc::new(RefCell::new(Vec::new()));

        let tray_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);

        // initial snapshot
        for item in handle.items() {
            let icon = make_tray_icon(&handle, &item);
            tray_box.append(&icon);
            items.borrow_mut().insert(item.id.clone(), icon);
        }

        // bridge tray events (library thread) into the GTK main loop
        let (ev_tx, ev_rx) = async_channel::unbounded::<TrayEvent>();
        {
            let sub = handle.subscribe();
            std::thread::spawn(move || {
                while let Ok(ev) = sub.recv() {
                    if ev_tx.send_blocking(ev).is_err() {
                        break;
                    }
                }
            });
        }

        let event_join = {
            let tray_box = tray_box.clone();
            let items = items.clone();
            glib::spawn_future_local(async move {
                while let Ok(ev) = ev_rx.recv().await {
                    handle_tray_event(&tray_box, &items, ev);
                }
            })
        };

        // clock
        let clock = gtk::Label::new(Some(&format_time()));
        clock.add_css_class("clock-label");

        {
            let clock = clock.clone();
            glib::timeout_add_seconds_local(1, move || {
                clock.set_text(&format_time());
                ControlFlow::Continue
            });
        }

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        container.append(&tray_box);
        container.append(&clock);

        ControlCenter {
            container,
            tray_box,
            clock,
            items,
            _handlers: handlers,
            _event_join: event_join,
        }
    }
}

/// Process-wide tray runtime (SPEC §5.1: one Tray instance, many consumers).
fn shared_tray() -> TrayHandle {
    static TRAY: OnceLock<TrayHandle> = OnceLock::new();
    TRAY
        .get_or_init(|| {
            let (handle, rx) =
                Tray::connect(TrayConfig::default()).expect("tray: connect to session bus");
            // keep the default channel drained; monitors use subscribe()
            std::thread::spawn(move || {
                for _ in rx {}
            });
            handle
        })
        .clone()
}

fn handle_tray_event(
    tray_box: &gtk::Box,
    items: &Rc<RefCell<HashMap<TrayItemId, gtk::Image>>>,
    ev: TrayEvent,
) {
    let handle = shared_tray();
    match ev {
        TrayEvent::Added(item) => {
            if items.borrow().contains_key(&item.id) {
                return;
            }
            let icon = make_tray_icon(&handle, &item);
            tray_box.append(&icon);
            items.borrow_mut().insert(item.id, icon);
        }
        TrayEvent::Removed(id) => {
            if let Some(icon) = items.borrow_mut().remove(&id) {
                tray_box.remove(&icon);
            }
        }
        TrayEvent::Changed { changed, item, .. } => {
            let map = items.borrow();
            let Some(icon) = map.get(&item.id) else {
                return;
            };
            if changed.contains(&ItemField::Icon) {
                render_icon(icon, &item.icon);
            }
            if changed.contains(&ItemField::Tooltip) {
                let tip = item.tooltip.as_ref().map(|t| {
                    if t.description.is_empty() {
                        t.title.clone()
                    } else {
                        format!("{}\n{}", t.title, t.description)
                    }
                });
                icon.set_tooltip_text(tip.as_deref());
            }
        }
        TrayEvent::MenuChanged(_) => {} // menus are read on right-click
        TrayEvent::WatcherChanged(_) => {}
        TrayEvent::InteractionResult { .. } => {}
        TrayEvent::Fatal(e) => eprintln!("[tray] fatal: {e}"),
    }
}

// ── item widget ──────────────────────────────────────────────────

fn make_tray_icon(handle: &TrayHandle, item: &tray::TrayItem) -> gtk::Image {
    let icon = gtk::Image::new();
    render_icon(&icon, &item.icon);
    icon.add_css_class("tray-icon");
    if let Some(t) = &item.tooltip {
        icon.set_tooltip_text(Some(&tooltip_text(&t.title, &t.description)));
    }

    // left click → Activate
    {
        let handle = handle.clone();
        let id = item.id.clone();
        let left = gtk::GestureClick::new();
        left.set_button(1);
        left.connect_pressed(move |_gesture, _n, x, y| {
            let _ = handle.activate(&id, x as i32, y as i32);
        });
        icon.add_controller(left);
    }

    // right click → ContextMenu (or SecondaryActivate fallback)
    {
        let handle = handle.clone();
        let id = item.id.clone();
        let icon_weak = icon.clone();
        let right = gtk::GestureClick::new();
        right.set_button(3);
        right.connect_pressed(move |_gesture, _n, x, y| {
            // DBusMenu contract: AboutToShow(0) *before* presenting. Lazy
            // providers populate their layout only in response to this, and
            // it also sync-refreshes our snapshot.
            let _ = handle.menu_about_to_show(&id, 0);
            let snapshot = handle.menu(&id).filter(|m| !m.root.is_empty());
            match snapshot {
                Some(s) => popup_menu(&handle, &id, &s, &icon_weak),
                None => {
                    // No usable menu: SNI says use the secondary action.
                    let _ = handle.secondary_activate(&id, x as i32, y as i32);
                }
            }
        });
        icon.add_controller(right);
    }

    icon
}

fn render_icon(image: &gtk::Image, source: &IconSource) {
    match source {
        IconSource::Name(name) if !name.is_empty() => {
            image.set_icon_name(Some(name));
        }
        IconSource::Pixmap(p) => match pixmap_to_texture(p) {
            Some(tex) => image.set_paintable(Some(&tex)),
            None => image.set_icon_name(Some("image-missing")),
        },
        _ => image.set_icon_name(Some("image-missing")),
    }
}

/// SNI pixmaps are ARGB32 in network byte order; GDK wants R8G8B8A8.
fn pixmap_to_texture(p: &tray::Pixmap) -> Option<gdk::MemoryTexture> {
    if p.width == 0 || p.height == 0 || p.argb.len() < (p.width * p.height * 4) as usize {
        return None;
    }
    let mut rgba = Vec::with_capacity(p.argb.len());
    for px in p.argb.chunks_exact(4) {
        let [a, r, g, b] = [px[0], px[1], px[2], px[3]];
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Some(gdk::MemoryTexture::new(
        p.width as i32,
        p.height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(rgba),
        (p.width * 4) as usize,
    ))
}

fn tooltip_text(title: &str, description: &str) -> String {
    if description.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{description}")
    }
}

// ── context menu (DBusMenu snapshot → gio::Menu) ─────────────────

fn popup_menu(
    handle: &TrayHandle,
    id: &TrayItemId,
    snapshot: &tray::MenuSnapshot,
    parent: &gtk::Image,
) {
    let actions = gio::SimpleActionGroup::new();
    let menu = build_menu(handle, id, &snapshot.root, &actions);

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(parent);
    popover.insert_action_group("tray", Some(&actions));
    popover.popup();

    let popover = popover.clone();
    popover.connect_closed(move |p| {
        p.unparent();
    });
}

fn build_menu(
    handle: &TrayHandle,
    id: &TrayItemId,
    entries: &[tray::MenuItem],
    actions: &gio::SimpleActionGroup,
) -> gio::Menu {
    let menu = gio::Menu::new();
    append_entries(handle, id, entries, &menu, actions);
    menu
}

fn append_entries(
    handle: &TrayHandle,
    id: &TrayItemId,
    entries: &[tray::MenuItem],
    menu: &gio::Menu,
    actions: &gio::SimpleActionGroup,
) {
    for entry in entries {
        if !entry.visible {
            continue;
        }
        match entry.kind {
            tray::MenuItemKind::Separator => {
                menu.append_section(None, &gio::Menu::new());
            }
            tray::MenuItemKind::Submenu => {
                let sub = gio::Menu::new();
                append_entries(handle, id, &entry.children, &sub, actions);
                menu.append_submenu(Some(&entry.label), &sub);
            }
            tray::MenuItemKind::Standard => {
                let action_name = format!("item-{}", entry.id);
                let full = format!("tray.{action_name}");
                let item = gio::MenuItem::new(Some(&entry.label), Some(&full));

                if !entry.enabled {
                    item.set_action_and_target_value(Some(&full), None);
                    item.set_attribute_value("enabled", Some(&false.to_variant()));
                }
                if let Some(icon) = &entry.icon {
                    item.set_icon(&gio::ThemedIcon::new(icon));
                }

                let action = match entry.toggled {
                    Some(state) => {
                        let a = gio::SimpleAction::new_stateful(
                            &action_name,
                            None,
                            &state.to_variant(),
                        );
                        a
                    }
                    None => gio::SimpleAction::new(&action_name, None),
                };
                {
                    let handle = handle.clone();
                    let item_id = id.clone();
                    let menu_id = entry.id;
                    let toggled = entry.toggled;
                    let action = action.clone();
                    action.connect_activate(move |a, _param| {
                        if let Some(cur) = toggled {
                            // flip local checkmark and inform the item
                            let next = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(cur);
                            a.set_state(&next.to_variant());
                        }
                        let _ = handle.menu_activate(&item_id, menu_id);
                    });
                }
                actions.add_action(&action);
                menu.append_item(&item);
            }
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────

fn format_time() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%H:%M"))
        .unwrap_or_default()
        .to_string()
}
