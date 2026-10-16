//! 控制中心入口与弹出面板。
//! 入口：电池（无电池设备自动隐藏）、网络状态、实时时钟；
//! 面板：媒体控制、音量/亮度滑块、日历。缺失降级见 ARCHITECTURE §2。

use std::cell::Cell;
use std::sync::Arc;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::bar::ipc::IpcClient;
use crate::shared::model::SystemState;
use crate::shared::protocol::{MediaOp, Request};

/// 滑块发送节流：拖动期间限频投递。
const SLIDER_THROTTLE: Duration = Duration::from_millis(200);

pub struct CenterWidget {
    pub container: gtk::Box,
    net: gtk::Label,
    battery: gtk::Label,
    volume: gtk::Scale,
    brightness: gtk::Scale,
    volume_row: gtk::Box,
    brightness_row: gtk::Box,
}

impl CenterWidget {
    pub fn new(ipc: Arc<IpcClient>) -> Self {
        let net = gtk::Label::new(None);
        net.add_css_class("status-label");

        let battery = gtk::Label::new(None);
        battery.add_css_class("status-label");

        let clock = gtk::Label::new(Some(&format_time()));
        clock.add_css_class("clock-label");
        {
            let clock = clock.clone();
            glib::timeout_add_seconds_local(1, move || {
                clock.set_text(&format_time());
                glib::ControlFlow::Continue
            });
        }
        let entry = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        entry.add_css_class("center-entry");
        entry.append(&net);
        entry.append(&battery);
        entry.append(&clock);

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.append(&entry);

        // ── 弹出面板 ──
        let (volume, volume_row) = Self::make_slider("音量");
        let (brightness, brightness_row) = Self::make_slider("亮度");

        let media_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        media_row.add_css_class("center-media");
        for (glyph, op) in [
            ("⏮", MediaOp::Prev),
            ("⏯", MediaOp::PlayPause),
            ("⏭", MediaOp::Next),
        ] {
            let button = gtk::Button::with_label(glyph);
            button.add_css_class("center-btn");
            button.set_focusable(false);
            let ipc = ipc.clone();
            button.connect_clicked(move |_| {
                let ipc = ipc.clone();
                std::thread::spawn(move || {
                    if let Err(e) = ipc.request(Request::MediaControl { op }) {
                        eprintln!("[center] media control failed: {e}");
                    }
                });
            });
            media_row.append(&button);
        }

        let calendar = gtk::Calendar::new();
        calendar.add_css_class("center-calendar");

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        panel.add_css_class("center-popover");
        panel.append(&media_row);
        panel.append(&volume_row);
        panel.append(&brightness_row);
        panel.append(&calendar);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&panel));
        popover.set_autohide(true);
        popover.set_parent(&entry);

        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_g, _n, _x, _y| {
            popover.popup();
        });
        entry.add_controller(click);

        let widget = Self {
            container,
            net,
            battery,
            volume,
            brightness,
            volume_row,
            brightness_row,
        };

        widget.wire_sliders(ipc);
        widget
    }

    fn make_slider(title: &str) -> (gtk::Scale, gtk::Box) {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);

        let label = gtk::Label::new(Some(title));
        label.add_css_class("center-slider-label");

        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.append(&label);
        row.append(&scale);
        (scale, row)
    }

    /// 滑块 → daemon（节流投递；拖动标记避免系统回推时回发）。
    fn wire_sliders(&self, ipc: Arc<IpcClient>) {
        let dragging = Rc::new(Cell::new(false));
        let last_send = Rc::new(Cell::new(Instant::now() - SLIDER_THROTTLE));

        for (kind, scale) in [("volume", &self.volume), ("brightness", &self.brightness)] {
            let gesture = gtk::GestureClick::new();
            let d_press = dragging.clone();
            gesture.connect_pressed(move |_g, _n, _x, _y| {
                d_press.set(true);
            });
            let d_release = dragging.clone();
            gesture.connect_released(move |_g, _n, _x, _y| {
                d_release.set(false);
            });
            scale.add_controller(gesture);

            let ipc = ipc.clone();
            let kind = kind.to_string();
            let dragging = dragging.clone();
            let last_send = last_send.clone();
            scale.connect_value_changed(move |scale| {
                if !dragging.get() {
                    return; // 非拖动期的值变化 = 系统状态回推，不回发
                }
                let now = Instant::now();
                if now.saturating_duration_since(last_send.get()) < SLIDER_THROTTLE {
                    return;
                }
                last_send.set(now);
                let value = scale.value().round() as u32;
                let request = if kind == "volume" {
                    Request::VolumeSet { value }
                } else {
                    Request::BrightnessSet { value }
                };
                let ipc = ipc.clone();
                std::thread::spawn(move || {
                    if let Err(e) = ipc.request(request) {
                        eprintln!("[center] slider set failed: {e}");
                    }
                });
            });
        }
    }

    pub fn set_system(&self, system: &SystemState) {
        // 网络：服务缺席 → 隐藏；无连接 → 警示
        match &system.network {
            None => self.net.set_visible(false),
            Some(n) => {
                self.net.set_visible(true);
                if n.connected {
                    let glyph = match n.kind.as_str() {
                        "ethernet" => "⏚",
                        "wifi" => "ᯤ",
                        _ => "◎",
                    };
                    self.net.set_text(glyph);
                } else {
                    self.net.set_text("⚠");
                }
            }
        }

        // 电池：无电池设备自动隐藏
        match &system.battery {
            None => self.battery.set_visible(false),
            Some(b) => {
                self.battery.set_visible(true);
                let bolt = if b.charging { "⚡" } else { "" };
                self.battery.set_text(&format!("{}{}%", bolt, b.percent));
            }
        }

        // 滑块：服务缺席隐藏整行；回推经 value_changed 的拖动判定过滤
        if let Some(v) = &system.volume {
            self.volume_row.set_visible(true);
            if (self.volume.value().round() as u32) != v.percent {
                self.volume.set_value(v.percent as f64);
            }
        } else {
            self.volume_row.set_visible(false);
        }
        if let Some(b) = &system.brightness {
            self.brightness_row.set_visible(true);
            if (self.brightness.value().round() as u32) != *b {
                self.brightness.set_value(*b as f64);
            }
        } else {
            self.brightness_row.set_visible(false);
        }
    }
}

fn format_time() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%H:%M"))
        .unwrap_or_default()
        .to_string()
}
