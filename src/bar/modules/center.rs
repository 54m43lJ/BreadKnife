//! 控制中心入口与弹出面板。
//! 入口：电池（无电池设备自动隐藏）、网络状态、实时时钟；
//! 面板：媒体控制、音量/亮度滑块、网络/蓝牙状态、日历。
//! 缺失降级见 ARCHITECTURE §2：入口隐藏，面板操作项灰置并提示。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::bar::ipc::IpcClient;
use crate::bar::modules::popup::{self, PopupManager};
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
    net_value: gtk::Label,
    bt_value: gtk::Label,
    play_pause_icon: gtk::Image,
    playing: Rc<Cell<bool>>,
}

impl CenterWidget {
    pub fn new(ipc: Arc<IpcClient>, window: &gtk::Window, popup: Rc<PopupManager>) -> Self {
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

        // 媒体控制：符号图标（非 emoji），均分整行；播放/暂停更大且随状态切换
        let playing = Rc::new(Cell::new(false));
        let media_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        media_row.add_css_class("center-media");
        media_row.set_homogeneous(true);
        media_row.set_hexpand(true);
        media_row.set_valign(gtk::Align::End);

        let prev = Self::media_button("media-skip-backward-symbolic", 16);
        let next = Self::media_button("media-skip-forward-symbolic", 16);

        let play_pause_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
        play_pause_icon.set_pixel_size(22);
        play_pause_icon.set_halign(gtk::Align::Center);
        play_pause_icon.set_valign(gtk::Align::End);
        let play_pause = gtk::Button::new();
        play_pause.add_css_class("center-btn");
        play_pause.set_focusable(false);
        play_pause.set_hexpand(true);
        play_pause.set_valign(gtk::Align::End);
        play_pause.set_child(Some(&play_pause_icon));

        {
            let ipc = ipc.clone();
            prev.connect_clicked(move |_| Self::send_media(ipc.clone(), MediaOp::Prev));
        }
        {
            let ipc = ipc.clone();
            next.connect_clicked(move |_| Self::send_media(ipc.clone(), MediaOp::Next));
        }
        {
            let ipc = ipc.clone();
            let playing = playing.clone();
            let icon = play_pause_icon.clone();
            play_pause.connect_clicked(move |_| {
                // 乐观切换图标，daemon 的 MPRIS 轮询随后校正
                let now = !playing.get();
                playing.set(now);
                icon.set_icon_name(Some(play_pause_icon_name(now)));
                Self::send_media(ipc.clone(), MediaOp::PlayPause);
            });
        }

        media_row.append(&prev);
        media_row.append(&play_pause);
        media_row.append(&next);

        let (net_value, net_row) = Self::make_status_row("网络");
        let (bt_value, bt_row) = Self::make_status_row("蓝牙");

        let calendar = gtk::Calendar::new();
        calendar.add_css_class("center-calendar");

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        panel.add_css_class("center-popover");
        panel.append(&media_row);
        panel.append(&volume_row);
        panel.append(&brightness_row);
        panel.append(&net_row);
        panel.append(&bt_row);
        panel.append(&calendar);

        // 面板挂根窗口（不参与容器布局），右缘与入口对齐；开关经 PopupManager
        let popover = gtk::Popover::new();
        popover.set_child(Some(&panel));
        popup::attach(
            &popover,
            window,
            gtk::PositionType::Bottom,
            gtk::Align::End,
            gtk::Align::Fill,
        );
        popup.manage(&popover);

        let pop_handle = popover.clone();
        let popup_click = popup.clone();
        let window_handle = window.clone();
        let entry_handle = entry.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_g, _n, _x, _y| {
            // toggle：本次按下刚关掉自己 → 不重开
            if popup_click.press_closed_contains("cc") {
                return;
            }
            popup::point_at(&pop_handle, &window_handle, &entry_handle);
            popup_click.clone().open(&pop_handle, "cc".into());
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
            net_value,
            bt_value,
            play_pause_icon,
            playing,
        };

        widget.wire_sliders(ipc);
        widget
    }

    /// 媒体按钮：符号图标按设备像素尺寸渲染，底部对齐，横向均分由父容器负责。
    fn media_button(icon: &str, pixel_size: i32) -> gtk::Button {
        let image = gtk::Image::from_icon_name(icon);
        image.set_pixel_size(pixel_size);
        image.set_halign(gtk::Align::Center);
        image.set_valign(gtk::Align::End);

        let button = gtk::Button::new();
        button.add_css_class("center-btn");
        button.set_focusable(false);
        button.set_hexpand(true);
        button.set_valign(gtk::Align::End);
        button.set_child(Some(&image));
        button
    }

    fn send_media(ipc: Arc<IpcClient>, op: MediaOp) {
        std::thread::spawn(move || {
            if let Err(e) = ipc.request(Request::MediaControl { op }) {
                eprintln!("[center] media control failed: {e}");
            }
        });
    }

    fn make_slider(title: &str) -> (gtk::Scale, gtk::Box) {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_valign(gtk::Align::Center);

        let label = gtk::Label::new(Some(title));
        label.add_css_class("center-slider-label");
        label.set_xalign(0.0);
        label.set_valign(gtk::Align::Center);
        label.set_size_request(30, -1);

        // 文字在左、滑块在右，同一行
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("center-slider-row");
        row.append(&label);
        row.append(&scale);
        (scale, row)
    }

    /// 面板状态行：标题 + 值（缺失降级由 set_system 切换灰置与提示）。
    fn make_status_row(title: &str) -> (gtk::Label, gtk::Box) {
        let label = gtk::Label::new(Some(title));
        label.add_css_class("center-slider-label");

        let value = gtk::Label::new(None);
        value.add_css_class("status-label");
        value.set_halign(gtk::Align::End);
        value.set_hexpand(true);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&label);
        row.append(&value);
        (value, row)
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

        // 滑块：服务缺席灰置并提示（面板操作项不隐藏，ARCHITECTURE §2）；
        // 回推经 value_changed 的拖动判定过滤
        match &system.volume {
            Some(_) => self.set_row_off(&self.volume_row, false, None),
            None => self.set_row_off(&self.volume_row, true, Some("pamixer 缺席，音量调节不可用")),
        }
        if let Some(v) = &system.volume
            && (self.volume.value().round() as u32) != v.percent
        {
            self.volume.set_value(v.percent as f64);
        }
        match &system.brightness {
            Some(_) => self.set_row_off(&self.brightness_row, false, None),
            None => self.set_row_off(
                &self.brightness_row,
                true,
                Some("brightnessctl 缺席，亮度调节不可用"),
            ),
        }
        if let Some(b) = &system.brightness
            && (self.brightness.value().round() as u32) != *b
        {
            self.brightness.set_value(*b as f64);
        }

        // 面板网络状态
        match &system.network {
            None => self.set_status(&self.net_value, "缺席", Some("NetworkManager 缺席")),
            Some(n) if !n.connected => self.set_status(&self.net_value, "未连接", None),
            Some(n) => {
                let text = match n.kind.as_str() {
                    "ethernet" => "以太网",
                    "wifi" => "Wi-Fi",
                    _ => "已连接",
                };
                self.set_status(&self.net_value, text, None);
            }
        }

        // 面板蓝牙状态
        match &system.bluetooth {
            None => self.set_status(&self.bt_value, "缺席", Some("BlueZ 缺席")),
            Some(b) if !b.powered => self.set_status(&self.bt_value, "已关闭", None),
            Some(b) if b.connected == 0 => self.set_status(&self.bt_value, "已开启", None),
            Some(b) => {
                self.set_status(&self.bt_value, &format!("{} 台已连接", b.connected), None);
            }
        }

        // 媒体状态：Playing → 暂停图标，其余（暂停 / 停止 / 无播放器）→ 播放图标
        let playing = system.media.as_ref().is_some_and(|m| m.playing);
        if self.playing.get() != playing {
            self.playing.set(playing);
            self.play_pause_icon
                .set_icon_name(Some(play_pause_icon_name(playing)));
        }
    }

    /// 面板操作项降级：灰置 + 提示（tooltip），恢复时解除。
    fn set_row_off(&self, row: &gtk::Box, off: bool, hint: Option<&str>) {
        row.set_sensitive(!off);
        if off {
            row.add_css_class("row-off");
            row.set_tooltip_text(hint);
        } else {
            row.remove_css_class("row-off");
            row.set_tooltip_text(None);
        }
    }

    /// 面板状态值：文本 + 可选提示。
    fn set_status(&self, label: &gtk::Label, text: &str, hint: Option<&str>) {
        label.set_text(text);
        if hint.is_some() {
            label.add_css_class("row-off");
        } else {
            label.remove_css_class("row-off");
        }
        label.set_tooltip_text(hint);
    }
}

fn play_pause_icon_name(playing: bool) -> &'static str {
    if playing {
        "media-playback-pause-symbolic"
    } else {
        "media-playback-start-symbolic"
    }
}

fn format_time() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%H:%M"))
        .unwrap_or_default()
        .to_string()
}
