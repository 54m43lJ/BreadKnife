//! UPower / NetworkManager 消费；pamixer / brightnessctl / playerctl 调起。
//! 缺失降级：服务 / 工具缺席时对应字段为 None（入口组件隐藏，见 ARCHITECTURE §2）。
//! zbus 互动运行在库外私有 tokio 运行时线程上，与 breadpan 的内部运行时互不相干。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::daemon::notify;
use crate::daemon::DaemonEvent;
use crate::shared::model::{BatteryState, NetworkState, SystemState, VolumeState};
use crate::shared::protocol::MediaOp;

const FAST_POLL: Duration = Duration::from_secs(5); // 音量 / 亮度
const SLOW_POLL: Duration = Duration::from_secs(30); // 电池 / 网络

pub fn start(tx: Sender<DaemonEvent>) {
    std::thread::Builder::new()
        .name("system".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                let conn = match zbus::Connection::session().await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[system] session bus unavailable: {e}");
                        return;
                    }
                };
                if let Err(e) = notify::serve(&conn, tx.clone()).await {
                    eprintln!("[system] notification daemon degraded: {e}");
                }
                let state = Arc::new(Mutex::new(SystemState::default()));
                let battery = poll_battery(conn, state.clone(), tx.clone());
                let network = poll_blocking(SLOW_POLL, state.clone(), tx.clone(), read_network);
                let vb = poll_blocking(FAST_POLL, state.clone(), tx.clone(), read_volume_brightness);
                tokio::join!(battery, network, vb);
            });
        })
        .expect("spawn system");
}

/// 通知记录缓存的窗口侧上限之外的守护侧留存上限。
pub const NOTIFICATION_KEEP: usize = 50;

async fn poll_battery(
    conn: zbus::Connection,
    state: Arc<Mutex<SystemState>>,
    tx: Sender<DaemonEvent>,
) {
    loop {
        let battery = battery_read(&conn).await;
        let changed = {
            let mut g = state.lock().unwrap();
            if g.battery != battery {
                g.battery = battery;
                true
            } else {
                false
            }
        };
        if changed {
            let _ = tx.send(DaemonEvent::System(state.lock().unwrap().clone()));
        }
        tokio::time::sleep(SLOW_POLL).await;
    }
}

async fn battery_read(conn: &zbus::Connection) -> Option<BatteryState> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
    )
    .await
    .ok()?;
    let percent: f64 = proxy.get_property("Percentage").await.ok()?;
    let state: u32 = proxy.get_property("State").await.unwrap_or(0);
    Some(BatteryState { percent: percent.round() as u32, charging: state == 1 })
}

async fn poll_blocking(
    period: Duration,
    state: Arc<Mutex<SystemState>>,
    tx: Sender<DaemonEvent>,
    read: fn(&mut SystemState) -> bool,
) {
    loop {
        let s = state.clone();
        let changed =
            tokio::task::spawn_blocking(move || {
                let mut g = s.lock().unwrap();
                read(&mut g)
            })
            .await
            .unwrap_or(false);
        if changed {
            let _ = tx.send(DaemonEvent::System(state.lock().unwrap().clone()));
        }
        tokio::time::sleep(period).await;
    }
}

fn read_network(g: &mut SystemState) -> bool {
    let network = run_capture(&["nmcli", "-t", "-f", "TYPE,STATE", "device"], |stdout| {
        let mut best: Option<NetworkState> = None;
        for line in stdout.lines() {
            let Some((kind, state)) = line.split_once(':') else { continue };
            if state.starts_with("connected") {
                // 有线优先展示
                if best.as_ref().is_none_or(|n| n.kind != "ethernet") {
                    best = Some(NetworkState { kind: kind.to_string(), connected: true });
                }
            }
        }
        best.or_else(|| Some(NetworkState { kind: "none".into(), connected: false }))
    });
    if g.network != network {
        g.network = network;
        true
    } else {
        false
    }
}

fn read_volume_brightness(g: &mut SystemState) -> bool {
    let mut changed = false;

    let percent = run_capture(&["pamixer", "--get-volume"], |s| s.trim().parse::<u32>().ok());
    if let Some(pct) = percent {
        // 音量可读才更新（pamixer 缺席 → 保持 None，入口隐藏）
        let muted = run_capture(&["pamixer", "--get-mute"], |s| Some(s.trim() == "true"))
            .unwrap_or(false);
        let volume = VolumeState { percent: pct, muted };
        if g.volume.as_ref() != Some(&volume) {
            g.volume = Some(volume);
            changed = true;
        }
    }

    // brightnessctl -m: "device,api,level,max,percent%"
    let brightness = run_capture(&["brightnessctl", "-m"], |s| {
        s.lines().next()?.rsplit(',').next()?.trim().trim_end_matches('%').parse::<u32>().ok()
    });
    if g.brightness != brightness {
        g.brightness = brightness;
        changed = true;
    }
    changed
}

// ── 执行面（窗口请求 → daemon 线程调用）──

pub fn set_volume(value: u32) {
    run_quiet(&["pamixer", "--set-volume", &value.to_string()]);
}

pub fn set_brightness(value: u32) {
    run_quiet(&["brightnessctl", "set", &format!("{value}%")]);
}

pub fn media_control(op: MediaOp) {
    let op = match op {
        MediaOp::PlayPause => "play-pause",
        MediaOp::Next => "next",
        MediaOp::Prev => "previous",
    };
    run_quiet(&["playerctl", op]);
}

// ── 子进程工具 ──

fn run_quiet(argv: &[&str]) {
    match std::process::Command::new(argv[0]).args(&argv[1..]).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!(
                "[system] {} failed: {}",
                argv[0],
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        Err(e) => eprintln!("[system] {} exec failed: {}", argv[0], e),
    }
}

fn run_capture<T>(argv: &[&str], f: impl FnOnce(&str) -> Option<T>) -> Option<T> {
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    f(&String::from_utf8_lossy(&out.stdout))
}
