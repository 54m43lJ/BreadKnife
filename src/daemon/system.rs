//! UPower / NetworkManager / BlueZ / MPRIS 消费（zbus）；pamixer / brightnessctl / ddcutil 调起。
//! 总线口径见 ARCHITECTURE §3.2：UPower / NetworkManager / BlueZ 在 system bus，
//! 通知守护与 MPRIS 在 session bus。缺失降级：服务 / 工具缺席时对应字段为 None
//! （入口组件隐藏，面板操作项灰置，见 ARCHITECTURE §2）。
//! zbus 互动运行在库外私有 tokio 运行时线程上，与 breadpan 的内部运行时互不相干。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::daemon::notify;
use crate::daemon::DaemonEvent;
use crate::shared::model::{BatteryState, BluetoothState, NetworkState, SystemState, VolumeState};
use crate::shared::protocol::MediaOp;

const FAST_POLL: Duration = Duration::from_secs(5); // 音量 / 亮度
const SLOW_POLL: Duration = Duration::from_secs(30); // 电池 / 网络 / 蓝牙

/// 媒体控制下发通道：请求线程 → system 运行时（zbus 连接绑定其创建的运行时）。
static MEDIA_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<MediaOp>> = OnceLock::new();

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
                let session = match zbus::Connection::session().await {
                    Ok(c) => Some(c),
                    Err(e) => {
                        eprintln!("[system] session bus unavailable: {e}");
                        None
                    }
                };
                let system = match zbus::Connection::system().await {
                    Ok(c) => Some(c),
                    Err(e) => {
                        eprintln!("[system] system bus unavailable: {e}");
                        None
                    }
                };
                if let Some(conn) = &session {
                    if let Err(e) = notify::serve(conn, tx.clone()).await {
                        eprintln!("[system] notification daemon degraded: {e}");
                    }
                    spawn_media_worker(conn);
                }
                let state = Arc::new(Mutex::new(SystemState::default()));
                let mut tasks = Vec::new();
                if let Some(conn) = &system {
                    tasks.push(tokio::spawn(poll_battery(
                        conn.clone(),
                        state.clone(),
                        tx.clone(),
                    )));
                    tasks.push(tokio::spawn(poll_network(
                        conn.clone(),
                        state.clone(),
                        tx.clone(),
                    )));
                    tasks.push(tokio::spawn(poll_bluetooth(
                        conn.clone(),
                        state.clone(),
                        tx.clone(),
                    )));
                }
                tasks.push(tokio::spawn(poll_blocking(
                    FAST_POLL,
                    state.clone(),
                    tx.clone(),
                    read_volume_brightness,
                )));
                for task in tasks {
                    let _ = task.await;
                }
            });
        })
        .expect("spawn system");
}

/// 通知记录缓存的窗口侧上限之外的守护侧留存上限。
pub const NOTIFICATION_KEEP: usize = 50;

// ── 媒体（MPRIS，session bus）──

fn spawn_media_worker(conn: &zbus::Connection) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MediaOp>();
    if MEDIA_TX.set(tx).is_err() {
        return; // 已就位（理论不达：start 单次）
    }
    let conn = conn.clone();
    tokio::spawn(async move {
        while let Some(op) = rx.recv().await {
            if let Err(e) = media_dispatch(&conn, op).await {
                eprintln!("[system] media control failed: {e}");
            }
        }
    });
}

/// 请求线程入口：投递到 system 运行时执行（连接不跨运行时使用）。
pub fn media_control(op: MediaOp) {
    match MEDIA_TX.get() {
        Some(tx) => {
            let _ = tx.send(op);
        }
        None => eprintln!("[system] media control unavailable: no session bus"),
    }
}

/// MPRIS 消费：取总线上的播放器（优先非瞬态名），对 Player 接口下发操作。
async fn media_dispatch(conn: &zbus::Connection, op: MediaOp) -> Result<(), String> {
    let dbus = zbus::Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .map_err(|e| e.to_string())?;
    let names: Vec<String> = dbus
        .call("ListNames", &())
        .await
        .map_err(|e| e.to_string())?;
    let mut players: Vec<String> = names
        .iter()
        .filter_map(|n| n.strip_prefix("org.mpris.MediaPlayer2."))
        .map(String::from)
        .collect();
    players.sort();
    // .instance 后缀为瞬态名，等价玩家的影子注册——优先唯一名
    let suffix = players
        .iter()
        .find(|s| !s.contains(".instance"))
        .or_else(|| players.first());
    let Some(suffix) = suffix else {
        return Err("no mpris player on the bus".into());
    };
    let player = zbus::Proxy::new(
        conn,
        format!("org.mpris.MediaPlayer2.{suffix}"),
        "/org/mpris/MediaPlayer2",
        "org.mpris.MediaPlayer2.Player",
    )
    .await
    .map_err(|e| e.to_string())?;
    let method = match op {
        MediaOp::PlayPause => "PlayPause",
        MediaOp::Next => "Next",
        MediaOp::Prev => "Previous",
    };
    let _: () = player.call(method, &()).await.map_err(|e| e.to_string())?;
    Ok(())
}

// ── 电池（UPower，system bus）──

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

// ── 网络（NetworkManager，system bus）──

async fn poll_network(
    conn: zbus::Connection,
    state: Arc<Mutex<SystemState>>,
    tx: Sender<DaemonEvent>,
) {
    loop {
        let network = network_read(&conn).await;
        let changed = {
            let mut g = state.lock().unwrap();
            if g.network != network {
                g.network = network;
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

/// 主连接类型 → 状态；无主连接 = 未连接；服务缺席 = None。
async fn network_read(conn: &zbus::Connection) -> Option<NetworkState> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    .ok()?;
    let ctype: String = proxy.get_property("PrimaryConnectionType").await.ok()?;
    let kind = match ctype.as_str() {
        "" => "none",
        "802-3-ethernet" => "ethernet",
        "802-11-wireless" | "wifi" => "wifi",
        _ => "other",
    };
    Some(NetworkState { kind: kind.into(), connected: !ctype.is_empty() })
}

// ── 蓝牙（BlueZ，system bus）──

async fn poll_bluetooth(
    conn: zbus::Connection,
    state: Arc<Mutex<SystemState>>,
    tx: Sender<DaemonEvent>,
) {
    loop {
        let bluetooth = bluetooth_read(&conn).await;
        let changed = {
            let mut g = state.lock().unwrap();
            if g.bluetooth != bluetooth {
                g.bluetooth = bluetooth;
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

/// ObjectManager 全量清点：适配器 Powered + 已连接设备数；BlueZ 缺席 = None。
async fn bluetooth_read(conn: &zbus::Connection) -> Option<BluetoothState> {
    let om = zbus::Proxy::new(conn, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager")
        .await
        .ok()?;
    let tree: std::collections::HashMap<
        zbus::zvariant::OwnedObjectPath,
        std::collections::HashMap<String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>>,
    > = om.call("GetManagedObjects", &()).await.ok()?;
    let mut powered = false;
    let mut connected = 0u32;
    for ifaces in tree.values() {
        for (iface, props) in ifaces {
            match iface.as_str() {
                "org.bluez.Adapter1" => {
                    powered |= props
                        .get("Powered")
                        .and_then(|v| v.downcast_ref::<bool>().ok())
                        .unwrap_or(false);
                }
                "org.bluez.Device1" => {
                    if props
                        .get("Connected")
                        .and_then(|v| v.downcast_ref::<bool>().ok())
                        .unwrap_or(false)
                    {
                        connected += 1;
                    }
                }
                _ => {}
            }
        }
    }
    Some(BluetoothState { powered, connected })
}

// ── 音量 / 亮度（pamixer / brightnessctl / ddcutil，daemon 调起）──

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

    // brightnessctl -m: "device,api,level,max,percent%"（内置面板）
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

/// 亮度：brightnessctl 管内置面板，ddcutil 经 DDC/CI 管外接显示器（ARCHITECTURE §3.2）。
pub fn set_brightness(value: u32) {
    run_quiet(&["brightnessctl", "set", &format!("{value}%")]);
    run_quiet(&["ddcutil", "setvcp", "10", &value.to_string()]);
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
