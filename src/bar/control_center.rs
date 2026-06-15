use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gtk::glib::{self, SignalHandlerId};
use gtk::prelude::*;

use astal_tray::prelude::{TrayExt, TrayItemExt};
use astal_tray::{Tray, TrayItem};

#[allow(unused)]
pub struct ControlCenter {
    pub container: gtk::Box,
    tray_box: gtk::Box,
    right_box: gtk::Box,
    network_icon: gtk::Label,
    volume_icon: gtk::Label,
    battery_icon: gtk::Label,
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
            items
                .borrow_mut()
                .insert(item.item_id().to_string(), icon);
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
        let right_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        right_box.append(&network_icon);
        right_box.append(&volume_icon);
        right_box.append(&battery_icon);
        right_box.append(&clock);

        // outer container
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
        // initial state
        {
            let (vol, muted) = query_pactl_volume();
            volume_icon.set_text(volume_icon_text(vol, muted));
        }

        // spawn background listener
        spawn_pactl_listener(volume_icon.clone());

        ControlCenter {
            container,
            tray_box,
            right_box,
            network_icon,
            volume_icon,
            battery_icon,
            clock,
            _items: items,
            _tray: tray,
            _handlers: handlers,
        }
    }
}

// ── tray icon helpers ────────────────────────────────────────────

fn make_tray_icon(item: &TrayItem, handlers: &Rc<RefCell<Vec<SignalHandlerId>>>) -> gtk::Image {
    let icon = gtk::Image::from_gicon(&item.gicon());
    icon.add_css_class("tray-icon");

    // icon changed
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

    // right click — menu or secondary activate
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
        let dev = parts[0];
        let typ = parts[1];
        let state = parts[2];

        if dev == "lo" {
            continue;
        }
        if !state.starts_with("connected") {
            continue;
        }

        match typ {
            "wifi" => wifi_up = true,
            "ethernet" => eth_up = true,
            _ => {}
        }
    }

    if wifi_up {
        "\u{f1eb}" // nf-fa-wifi
    } else if eth_up {
        "\u{e796}" // nf-dev-ethernet
    } else {
        "\u{f05e}" // nf-fa-ban
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
            // extract first "  NN%" → the number before '%'
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
        "\u{eee8}" // nf-fa-volume_xmark
    } else if vol > 66 {
        "\u{f028}" // nf-fa-volume_up
    } else if vol > 33 {
        "\u{f027}" // nf-fa-volume_down
    } else {
        "\u{f026}" // nf-fa-volume_off
    }
}

fn spawn_pactl_listener(volume_icon: gtk::Label) {
    let (tx, rx) = mpsc::channel::<()>();

    // background thread: spawn pactl subscribe, forward events via mpsc
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
                // any sink event → signal main thread to re-query
                if tx.send(()).is_err() {
                    break; // receiver dropped
                }
            }
        }
        // child died or pipe closed → tx dropped → timeout loop exits via recv error
    });

    // main thread: poll mpsc every 500ms, drain events, re-query volume
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
        None => "\u{f1e6}", // nf-fa-plug (desktop, no battery)
        Some(cap) => {
            if charging {
                return "\u{f0e7}"; // nf-fa-bolt
            }
            if cap >= 90 {
                "\u{f240}" // nf-fa-battery_4
            } else if cap >= 60 {
                "\u{f241}" // nf-fa-battery_3
            } else if cap >= 30 {
                "\u{f242}" // nf-fa-battery_2
            } else if cap >= 10 {
                "\u{f243}" // nf-fa-battery_1
            } else {
                "\u{f244}" // nf-fa-battery_0
            }
        }
    }
}
