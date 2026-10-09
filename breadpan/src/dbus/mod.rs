//! DBus 域出口（内部聚合，不直接对外）。
//!
//! 常量、启动编排（`run`）、报到受理与反向发现、全局信号订阅任务。

pub(crate) mod bus;
pub(crate) mod heartbeat;
pub(crate) mod iface;
pub(crate) mod interact;
pub(crate) mod menu;
pub(crate) mod watcher;

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::pin;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use zbus::export::futures_core::Stream;
use zbus::fdo;
use zbus::message::{Message, Type as MessageType};
use zbus::zvariant::{OwnedValue, Value};
use zbus::{MatchRule, MessageStream};

use crate::events::TrayEvent;
use crate::props;
use crate::tracker::ItemEntry;
use crate::{TrayError, TrayItem};

use bus::BusCenter;

pub(crate) const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
pub(crate) const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub(crate) const ITEM_IFACE_KDE: &str = "org.kde.StatusNotifierItem";
pub(crate) const ITEM_IFACE_FD: &str = "org.freedesktop.StatusNotifierItem";
pub(crate) const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
pub(crate) const DBUSMENU_IFACE: &str = "com.canonical.dbusmenu";
pub(crate) const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
pub(crate) const DBUS_SERVICE: &str = "org.freedesktop.DBus";
pub(crate) const DBUS_PATH: &str = "/";
pub(crate) const DBUS_IFACE: &str = "org.freedesktop.DBus";
pub(crate) const INTROSPECTABLE_IFACE: &str = "org.freedesktop.DBus.Introspectable";

/// 报到/发现参数解析：
/// - `"name"` → (name, /StatusNotifierItem)；
/// - `"name/path"` 就地切分；
/// - `"/path"`（Ayatana 风格纯对象路径）→ 服务名取注册方唯一名（由调用方提供）。
pub(crate) fn split_service_arg(arg: &str, sender: Option<&str>) -> Option<(String, String)> {
    if arg.starts_with('/') {
        // 纯对象路径（Ayatana 风格）：服务名 = 注册方唯一名
        let sender = sender?;
        return Some((sender.to_string(), arg.to_string()));
    }
    match arg.split_once('/') {
        Some((name, path)) if !name.is_empty() && !path.is_empty() => {
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            };
            Some((name.to_string(), path))
        }
        _ if !arg.is_empty() => Some((arg.to_string(), DEFAULT_ITEM_PATH.to_string())),
        _ => None,
    }
}

/// 报到受理：解析注册参数（含 Ayatana 纯路径风格）→ 探测路由标记（Menu / ItemIsMenu）
/// → 最小集入册 →（announce）协议面广播 + Added。已存在 → 原地刷新并返回 false（幂等）。
pub(crate) async fn register_item(
    bus: &BusCenter,
    service_arg: &str,
    sender: Option<&str>,
    announce: bool,
) -> Result<bool, TrayError> {
    let Some((service, path)) = split_service_arg(service_arg, sender) else {
        return Err(TrayError::Protocol(format!(
            "unusable RegisterStatusNotifierItem argument: {service_arg:?}"
        )));
    };
    let unique: String = bus
        .call(
            DBUS_SERVICE,
            DBUS_PATH,
            DBUS_IFACE,
            "GetNameOwner",
            &service,
        )
        .await
        .unwrap_or_else(|_| service.clone());
    let (menu, menu_only) = props::probe_routing(bus, &service, &path).await;
    let entry = ItemEntry {
        service: service.clone(),
        unique,
        path,
        alive: true,
        menu,
        menu_only,
    };
    let inserted = bus.tracker.write().unwrap().upsert(entry);
    // 快照栅栏：快照收集前入册的 Item 由快照承载（静默），其后才广播 + 入队 Added
    if inserted && announce && bus.is_snapshot_done() {
        iface::emit_registered(bus, &service).await;
        bus.events.send(TrayEvent::Added(service));
    }
    Ok(inserted)
}

/// 启动期反向发现：ListNames 命中 `org.kde.StatusNotifierItem-` 前缀 → Introspect 探测 →
/// 静默入册（不广播、不入队 Added——快照是首事件）。
/// 过程叙述以返回值交付，由调用方在快照之后投递。
async fn discover_existing(bus: &BusCenter) -> Vec<String> {
    let mut narrations = Vec::new();
    let names: Vec<String> = bus
        .call(DBUS_SERVICE, DBUS_PATH, DBUS_IFACE, "ListNames", &())
        .await
        .unwrap_or_else(|e| {
            narrations.push(format!("reverse discovery: ListNames failed: {e}"));
            Vec::new()
        });
    let candidates: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with("org.kde.StatusNotifierItem-"))
        .collect();
    narrations.push(format!(
        "reverse discovery: {} candidate name(s)",
        candidates.len()
    ));
    for name in candidates {
        match probe_item_iface(bus, &name).await {
            Ok(true) => {
                let _ = register_item(bus, &name, None, false).await;
            }
            Ok(false) => narrations.push(format!(
                "discovery skip {name}: no StatusNotifierItem interface at {DEFAULT_ITEM_PATH}"
            )),
            Err(e) => narrations.push(format!("discovery skip {name}: probe failed: {e}")),
        }
    }
    narrations
}

/// 存量 Item 探测：Introspect 接口名确认；不实现 Introspect 的实现
/// （Electron/Ayatana 等，返回空 XML）回退为直接属性读取确认。
async fn probe_item_iface(bus: &BusCenter, name: &str) -> Result<bool, TrayError> {
    if let Ok(xml) = bus
        .call::<String, _>(
            name,
            DEFAULT_ITEM_PATH,
            INTROSPECTABLE_IFACE,
            "Introspect",
            &(),
        )
        .await
    {
        if xml.contains(ITEM_IFACE_KDE) || xml.contains(ITEM_IFACE_FD) {
            return Ok(true);
        }
    }
    Ok(props::read_all(bus, name, DEFAULT_ITEM_PATH).await.is_ok())
}

/// 快照：逐个现场读取全部在册 Item（并发），入队 Snapshot——事件流起点。
/// 收集完毕置栅栏并冲刷启动期延迟的死亡清理（快照内的死亡才补投 Removed）。
/// 读取失败的过程叙述以返回值交付（快照先行）。
async fn queue_snapshot(bus: &BusCenter) -> Vec<String> {
    let entries = bus.tracker.read().unwrap().all();
    let mut joins = Vec::with_capacity(entries.len());
    for entry in entries {
        let bus2 = bus.clone();
        joins.push(
            bus.rt
                .spawn(async move { props::fetch_item(&bus2, &entry).await }),
        );
    }
    let mut items: Vec<TrayItem> = Vec::with_capacity(joins.len());
    let mut narrations = Vec::new();
    for join in joins {
        match join.await {
            Ok(Ok(item)) => items.push(item),
            Ok(Err(e)) => narrations.push(format!("snapshot fetch failed: {e}")),
            Err(e) => narrations.push(format!("snapshot task failed: {e}")),
        }
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    bus.events.send(TrayEvent::Snapshot { items });

    // 栅栏置位：其后的报到/事件走正常增量路径
    bus.mark_snapshot_done();
    // 启动期（发现项）死亡清理冲刷：仅快照内的 id 补投 Removed，其余按从未出现丢弃
    let snapshot_ids: std::collections::HashSet<String> = bus.tracker.read().unwrap().ids();
    for id in bus.take_deferred_removed() {
        if snapshot_ids.contains(&id) {
            bus.events.send(TrayEvent::Removed(id));
        }
    }
    narrations
}

/// 主 future：库内运行时线程的驻留体——bootstrap 完成后驻停，直至停机/终局。
pub(crate) async fn run(
    bus: BusCenter,
    mut stop_rx: watch::Receiver<bool>,
    mut fatal_rx: watch::Receiver<String>,
    mut displaced_rx: watch::Receiver<bool>,
    ready_tx: mpsc::Sender<Result<BusCenter, TrayError>>,
) {
    // ── bootstrap（失败即向 start 报错）──
    if let Err(e) = bootstrap(&bus).await {
        let _ = ready_tx.send(Err(e)).await;
        return;
    }
    if ready_tx.send(Ok(bus.clone())).await.is_err() {
        // 消费端放弃等待：直接转停机路径，避免悬挂线程
        graceful_shutdown(&bus).await;
        return;
    }

    // ── 驻停 ──
    tokio::select! {
        _ = stop_rx.changed() => graceful_shutdown(&bus).await,
        _ = fatal_rx.changed() => {
            let msg = fatal_rx.borrow_and_update().clone();
            let _ = watcher::release(&bus).await;
            bus.tracker.write().unwrap().clear();
            bus.set_stopped();
            bus.events.send(TrayEvent::Fatal(msg));
            bus.events.close();
        }
        _ = displaced_rx.changed() => {
            // 所有权异常丢失（§2.8）：清空簿记、服务转停止形态、终局报错；不重试、不夺回。
            bus.tracker.write().unwrap().clear();
            bus.set_stopped();
            bus.events.send(TrayEvent::Displaced);
            bus.events.close();
        }
    }
}

async fn bootstrap(bus: &BusCenter) -> Result<(), TrayError> {
    // 挂载协议面（尚不申请名，报到无从发生）
    watcher::mount_interfaces(bus).await?;

    // 全局信号订阅 + 周期任务
    spawn_signal_tasks(bus.clone());
    bus.spawn(heartbeat::heartbeat_task(bus.clone()));
    bus.spawn(debounce_task(bus.clone()));

    // 反向发现存量 Item（静默入册）
    let discovery_notes = discover_existing(bus).await;

    // 快照（首事件），其后补投启动过程叙述与延迟的死亡清理
    let snapshot_notes = queue_snapshot(bus).await;
    for note in discovery_notes.into_iter().chain(snapshot_notes) {
        bus.events.send(TrayEvent::Narration(note));
    }

    // 最后申请 Watcher 名：此后报到以增量事件衔接（快照已就位）
    watcher::WatcherService::start(bus).await?;
    Ok(())
}

async fn graceful_shutdown(bus: &BusCenter) {
    let _ = watcher::release(bus).await;
    bus.tracker.write().unwrap().clear();
    bus.set_stopped();
    bus.events.close();
}

fn spawn_signal_tasks(bus: BusCenter) {
    bus.spawn(name_owner_changed_task(bus.clone()));
    bus.spawn(item_signals_task(bus.clone(), ITEM_IFACE_KDE));
    bus.spawn(item_signals_task(bus.clone(), ITEM_IFACE_FD));
    bus.spawn(properties_changed_task(bus.clone(), ITEM_IFACE_KDE));
    bus.spawn(properties_changed_task(bus.clone(), ITEM_IFACE_FD));
    bus.spawn(dbusmenu_signals_task(bus.clone()));
}

/// 订阅并消费一条匹配规则下的信号流。
async fn pump_signal_stream(
    rule: MatchRule<'static>,
    bus: BusCenter,
    mut on_msg: impl FnMut(&BusCenter, Message),
) {
    let stream = match MessageStream::for_match_rule(rule, &bus.conn, None).await {
        Ok(s) => s,
        Err(e) => {
            bus.events.send(TrayEvent::Narration(format!(
                "signal subscription failed: {e}"
            )));
            return;
        }
    };
    let mut stream = pin!(stream);
    while let Some(msg) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        match msg {
            Ok(m) => on_msg(&bus, m),
            Err(_) => continue,
        }
    }
}

fn signal_source(msg: &Message) -> (Option<String>, Option<String>) {
    let header = msg.header();
    (
        header.sender().map(|s| s.to_string()),
        header.path().map(|p| p.to_string()),
    )
}

/// NameOwnerChanged：Item 死亡清理、同名换主（应用重启竞态）、Watcher 名异常丢失（Displaced）。
async fn name_owner_changed_task(bus: BusCenter) {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(DBUS_SERVICE)
        .unwrap()
        .interface(DBUS_IFACE)
        .unwrap()
        .member("NameOwnerChanged")
        .unwrap()
        .build();
    pump_signal_stream(rule, bus.clone(), move |bus, msg| {
        let Some(sig) = fdo::NameOwnerChanged::from_message(msg) else {
            return;
        };
        let Ok(args) = sig.args() else {
            return;
        };
        let name = args.name().to_string();
        let old = (**args.old_owner())
            .clone()
            .map(|o| o.to_string())
            .unwrap_or_default();
        let new = (**args.new_owner())
            .clone()
            .map(|o| o.to_string())
            .unwrap_or_default();

        if name == WATCHER_NAME {
            if !new.is_empty() || old.is_empty() || bus.is_stopping() || bus.is_stopped() {
                return;
            }
            bus.report_displaced();
            return;
        }

        if new.is_empty() {
            // 服务名消失（或属主唯一名消失）：死亡清理
            let removed = bus.tracker.write().unwrap().unregister(&name);
            for entry in removed {
                if bus.is_snapshot_done() {
                    bus.events.send(TrayEvent::Removed(entry.service.clone()));
                } else {
                    // 启动期死亡：快照投递后按快照内容冲刷
                    bus.defer_removed(entry.service.clone());
                }
                let bus2 = bus.clone();
                let service = entry.service;
                bus.spawn(async move {
                    iface::emit_unregistered(&bus2, &service).await;
                });
            }
        } else if !old.is_empty() {
            // 同名换主：旧条目移除，新属主按报到重新入册
            let stale = bus.tracker.write().unwrap().unregister(&name);
            for entry in stale {
                bus.events.send(TrayEvent::Removed(entry.service.clone()));
            }
            let bus2 = bus.clone();
            let name2 = name.clone();
            bus.spawn(async move {
                let _ = register_item(&bus2, &name2, None, true).await;
            });
        }
    })
    .await;
}

/// 官方 New* 信号（NewStatus / NewIcon / NewTitle / NewToolTip / NewOverlayIcon / NewAttentionIcon）
/// 与 PropertiesChanged 同为变更触发源；本任务只登记待通告事实，不带数据。
async fn item_signals_task(bus: BusCenter, iface: &'static str) {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(iface)
        .unwrap()
        .build();
    pump_signal_stream(rule, bus.clone(), |bus, msg| {
        let (Some(sender), Some(path)) = signal_source(&msg) else {
            return;
        };
        if let Some(id) = bus.tracker.read().unwrap().find_by_signal(&sender, &path) {
            bus.pending.lock().unwrap().insert(id);
        }
    })
    .await;
}

/// PropertiesChanged（限定 arg0 = SNI 接口）：路由标记就地更新 + 待通告登记。
async fn properties_changed_task(bus: BusCenter, iface: &'static str) {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(PROPS_IFACE)
        .unwrap()
        .member("PropertiesChanged")
        .unwrap()
        .arg(0, iface)
        .unwrap()
        .build();
    pump_signal_stream(rule, bus.clone(), |bus, msg| {
        let (Some(sender), Some(path)) = signal_source(&msg) else {
            return;
        };
        let body: (String, HashMap<String, OwnedValue>, Vec<String>) =
            match msg.body().deserialize() {
                Ok(b) => b,
                Err(_) => return,
            };
        let Some(id) = bus.tracker.read().unwrap().find_by_signal(&sender, &path) else {
            return;
        };
        // 路由标记（ItemIsMenu / Menu）随变更信号就地更新
        let menu = body.1.get("Menu").map(|v| {
            let path: Option<zbus::zvariant::ObjectPath<'static>> =
                Value::from(v.try_clone().ok()?).try_into().ok();
            path.map(|p| p.to_string())
        });
        let menu_only = body
            .1
            .get("ItemIsMenu")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| v.try_into().ok());
        bus.tracker
            .write()
            .unwrap()
            .update_routing(&id, menu, menu_only);
        bus.pending.lock().unwrap().insert(id);
    })
    .await;
}

/// dbusmenu LayoutUpdated / ItemsPropertiesUpdated → 仅转发 MenuChanged，不代为重拉。
/// 快照投递前丢弃（消费方尚无菜单可重取）。
async fn dbusmenu_signals_task(bus: BusCenter) {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(DBUSMENU_IFACE)
        .unwrap()
        .build();
    pump_signal_stream(rule, bus.clone(), |bus, msg| {
        let (Some(sender), Some(path)) = signal_source(&msg) else {
            return;
        };
        if !bus.is_snapshot_done() {
            return;
        }
        if let Some(id) = bus
            .tracker
            .read()
            .unwrap()
            .find_by_menu_signal(&sender, &path)
        {
            bus.events.send(TrayEvent::MenuChanged(id));
        }
    })
    .await;
}

/// 变更通告合并（固定窗口节流）：信号只入待通告集合，节拍到期统一投递。
/// 快照投递前不冲刷（首事件契约）。
async fn debounce_task(bus: BusCenter) {
    let period = Duration::from_millis(bus.config.debounce_ms.max(1));
    let mut tick = tokio::time::interval(period);
    loop {
        tick.tick().await;
        if !bus.is_snapshot_done() {
            continue;
        }
        let drained: Vec<crate::TrayItemId> = bus.pending.lock().unwrap().drain().collect();
        for id in drained {
            bus.events.send(TrayEvent::Changed(id));
        }
    }
}
