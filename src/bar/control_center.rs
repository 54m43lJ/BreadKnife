use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::sni;

#[allow(unused)]
pub struct ControlCenter {
    pub container: gtk::Box,
    tray_box: gtk::Box,
    right_box: gtk::Box,
    network_icon: gtk::Label,
    volume_icon: gtk::Label,
    battery_icon: gtk::Label,
    clock: gtk::Label,
    tray_items: Rc<RefCell<HashMap<String, gtk::Image>>>,
}

impl ControlCenter {
    pub fn new() -> Self {
        let tray_items = Rc::new(RefCell::new(HashMap::new()));
        let tray_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);

        // ── tray: load existing + subscribe ──────────────────────
        {
            let tb = tray_box.clone();
            let ti = tray_items.clone();

            // initial items
            let ids = sni::item_ids();
            eprintln!("[cc] initial tray items: {} ({:?})", ids.len(), ids);
            for id in &ids {
                if let Some(state) = sni::get_item_state(id) {
                    eprintln!("[cc] creating tray icon for {id} icon={}", state.icon_name);
                    let icon = make_tray_icon(id, &state);
                    tb.append(&icon);
                    ti.borrow_mut().insert(id.clone(), icon);
                }
            }

            // item added
            {
                let tb = tb.clone();
                let ti = ti.clone();
                sni::set_item_added_cb(move |id: &str| {
                    eprintln!("[cc] cb: item added {id}");
                    let id = id.to_string();
                    if let Some(state) = sni::get_item_state(&id) {
                        eprintln!("[cc] cb: creating tray icon for {id}");
                        let icon = make_tray_icon(&id, &state);
                        tb.append(&icon);
                        ti.borrow_mut().insert(id, icon);
                    }
                });
            }

            // item removed
            {
                let ti = ti.clone();
                let tb = tb.clone();
                sni::set_item_removed_cb(move |id: &str| {
                    if let Some(icon) = ti.borrow_mut().remove(id) {
                        tb.remove(&icon);
                    }
                });
            }

            // item updated (icon changes)
            {
                let ti = ti.clone();
                sni::set_item_updated_cb(move |id: &str, _kind: sni::item::UpdateKind| {
                    let id = id.to_string();
                    if let Some(state) = sni::get_item_state(&id) {
                        if let Ok(items) = ti.try_borrow() {
                            if let Some(icon) = items.get(&id) {
                                sni::icon::update_tray_image(icon, &state);
                            }
                        }
                    }
                });
            }
        }

        // ── system icons + clock ──────────────────────────────────

        let network_icon = gtk::Label::new(Some("\u{f05e}"));
        network_icon.add_css_class("system-icon");
        network_icon.set_halign(gtk::Align::End);

        let volume_icon = gtk::Label::new(Some("\u{f026}"));
        volume_icon.add_css_class("system-icon");
        volume_icon.set_halign(gtk::Align::End);

        let battery_icon = gtk::Label::new(Some("\u{f1e6}"));
        battery_icon.add_css_class("system-icon");
        battery_icon.set_halign(gtk::Align::End);

        let clock = gtk::Label::new(Some(&format_time()));
        clock.add_css_class("clock-label");
        clock.set_halign(gtk::Align::End);

        // right_box: horizontal icon + clock row
        let right_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        right_box.append(&network_icon);
        right_box.append(&volume_icon);
        right_box.append(&battery_icon);
        right_box.append(&clock);

        // outer container
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        container.append(&tray_box);
        container.append(&right_box);

        // ── timers ────────────────────────────────────────────────

        // clock — 1s
        {
            let clock = clock.clone();
            glib::timeout_add_seconds_local(1, move || {
                clock.set_text(&format_time());
                glib::ControlFlow::Continue
            });
        }

        // network — 5s, nmcli
        {
            let network_icon = network_icon.clone();
            glib::timeout_add_seconds_local(5, move || {
                network_icon.set_text(poll_network());
                glib::ControlFlow::Continue
            });
        }

        // battery — 30s, sysfs
        {
            let battery_icon = battery_icon.clone();
            glib::timeout_add_seconds_local(30, move || {
                let (cap, charging) = poll_battery();
                battery_icon.set_text(battery_icon_text(cap, charging));
                glib::ControlFlow::Continue
            });
        }

        // initial battery poll immediately
        {
            let battery_icon = battery_icon.clone();
            let (cap, charging) = poll_battery();
            battery_icon.set_text(battery_icon_text(cap, charging));
        }

        // volume — event-driven via pactl subscribe
        {
            let (vol, muted) = query_pactl_volume();
            volume_icon.set_text(volume_icon_text(vol, muted));
        }
        spawn_pactl_listener(volume_icon.clone());

        ControlCenter {
            container,
            tray_box,
            right_box,
            network_icon,
            volume_icon,
            battery_icon,
            clock,
            tray_items,
        }
    }
}

// ── tray icon helpers ────────────────────────────────────────────

fn make_tray_icon(
    id: &str,
    state: &sni::item::SniItemState,
) -> gtk::Image {
    let icon = sni::icon::make_tray_image(state);
    icon.add_css_class("tray-icon");

    // left click
    {
        let id = id.to_string();
        let left = gtk::GestureClick::new();
        left.set_button(1);
        left.connect_pressed(move |_gesture, _n, x, y| {
            sni::activate_item(&id, x as i32, y as i32);
        });
        icon.add_controller(left);
    }

    // right click — menu or secondary activate
    {
        let id_for_menu = id.to_string();
        let id_for_fallback = id.to_string();
        let icon_weak = icon.clone();
        let right = gtk::GestureClick::new();
        right.set_button(3);
        right.connect_pressed(move |_gesture, _n, x, y| {
            let icon_weak = icon_weak.clone();
            let id_fb = id_for_fallback.clone();
            sni::request_menu(&id_for_menu, move |result| {
                if let Some(result) = result {
                    if !result.items.is_empty() {
                        let menu = build_menu_from_result(&result);
                        let popover = gtk::PopoverMenu::from_model(Some(&menu));
                        popover.set_parent(&icon_weak);
                        popover.popup();
                        return;
                    }
                }
                sni::secondary_activate_item(&id_fb, x as i32, y as i32);
            });
        });
        icon.add_controller(right);
    }

    icon
}

fn build_menu_from_result(result: &sni::menu::MenuResult) -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    for item in &result.items {
        build_menu_item(&menu, item);
    }
    menu
}

fn build_menu_item(menu: &gtk::gio::Menu, item: &sni::menu::MenuItem) {
    if item.is_separator {
        // gtk::gio::Menu doesn't have a direct separator; use empty label item
        let mi = gtk::gio::MenuItem::new(Some(""), None);
        menu.append_item(&mi);
        return;
    }

    if item.is_submenu {
        let sub = gtk::gio::Menu::new();
        for child in &item.children {
            build_menu_item(&sub, child);
        }
        let mi = gtk::gio::MenuItem::new(Some(&item.label), None);
        mi.set_submenu(Some(&sub));
        menu.append_item(&mi);
    } else if !item.action.is_empty() {
        let mi = gtk::gio::MenuItem::new(
            Some(&item.label),
            Some(&format!("app.{}", item.action)),
        );
        menu.append_item(&mi);
    } else if !item.label.is_empty() {
        let mi = gtk::gio::MenuItem::new(Some(&item.label), None);
        menu.append_item(&mi);
    }
}

// ── clock ────────────────────────────────────────────────────────

fn format_time() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%H:%M"))
        .unwrap_or_default()
        .to_string()
}

// ── network ──────────────────────────────────────────────────────

fn poll_network() -> &'static str {
    let output = match Command::new("nmcli")
        .args(["-t", "-f", "DEVICE,TYPE,STATE", "device", "status"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return "\u{f05e}",
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut wifi_up = false;
    let mut eth_up = false;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() < 3 {
            continue;
        }
        if parts[0] == "lo" {
            continue;
        }
        if !parts[2].starts_with("connected") {
            continue;
        }
        match parts[1] {
            "wifi" => wifi_up = true,
            "ethernet" => eth_up = true,
            _ => {}
        }
    }

    if wifi_up {
        "\u{f1eb}"
    } else if eth_up {
        "\u{e796}"
    } else {
        "\u{f05e}"
    }
}

// ── volume ───────────────────────────────────────────────────────

fn query_pactl_volume() -> (u32, bool) {
    let vol = Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.split('%')
                .next()
                .and_then(|before| before.rsplit(' ').next())
                .and_then(|num| num.parse::<u32>().ok())
        })
        .unwrap_or(0);

    let muted = Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
        .ok()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("yes")
        })
        .unwrap_or(false);

    (vol, muted)
}

fn volume_icon_text(vol: u32, muted: bool) -> &'static str {
    if muted {
        "\u{eee8}"
    } else if vol > 66 {
        "\u{f028}"
    } else if vol > 33 {
        "\u{f027}"
    } else {
        "\u{f026}"
    }
}

fn spawn_pactl_listener(volume_icon: gtk::Label) {
    let (tx, rx) = mpsc::channel::<()>();

    thread::spawn(move || {
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg("pactl subscribe 2>/dev/null | grep --line-buffered sink")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => return,
        };

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if line.is_ok() {
                if tx.send(()).is_err() {
                    break;
                }
            }
        }
    });

    glib::timeout_add_local(Duration::from_millis(500), move || {
        let mut changed = false;
        while rx.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            let (vol, muted) = query_pactl_volume();
            volume_icon.set_text(volume_icon_text(vol, muted));
        }
        glib::ControlFlow::Continue
    });
}

// ── battery ──────────────────────────────────────────────────────

fn poll_battery() -> (Option<u8>, bool) {
    let capacity = std::fs::read_to_string("/sys/class/power_supply/BAT0/capacity")
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok());

    let charging = std::fs::read_to_string("/sys/class/power_supply/BAT0/status")
        .ok()
        .map(|s| s.trim() == "Charging")
        .unwrap_or(false);

    (capacity, charging)
}

fn battery_icon_text(capacity: Option<u8>, charging: bool) -> &'static str {
    match capacity {
        None => "\u{f1e6}",
        Some(cap) => {
            if charging {
                return "\u{f0e7}";
            }
            if cap >= 90 {
                "\u{f240}"
            } else if cap >= 60 {
                "\u{f241}"
            } else if cap >= 30 {
                "\u{f242}"
            } else if cap >= 10 {
                "\u{f243}"
            } else {
                "\u{f244}"
            }
        }
    }
}
