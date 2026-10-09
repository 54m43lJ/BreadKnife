//! org.kde.StatusNotifierWatcher 协议面（ARCHITECTURE §3.4）。
//!
//! 监听与应答对 `org.freedesktop.*`（原文拼写）与 `org.kde.*` 两套前缀等价处理：
//! 两个接口名挂在同一路径上；信号广播走事实规范前缀 org.kde。

use zbus::object_server::SignalEmitter;

use crate::dbus::bus::BusCenter;
use crate::dbus::{self, WATCHER_PATH};

fn registered_list(bus: &BusCenter) -> Vec<String> {
    let mut ids: Vec<String> = bus.tracker.read().unwrap().ids().into_iter().collect();
    ids.sort();
    ids
}

/// org.kde 前缀（事实规范）协议面。
pub(crate) struct WatcherIface {
    pub(crate) bus: BusCenter,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherIface {
    /// Item 报到：登记总线唯一名 + 对象路径；报到即拉取跟踪；重复报到幂等。
    async fn register_status_notifier_item(&self, service: &str) {
        if let Err(e) = dbus::register_item(&self.bus, service, true).await {
            self.bus
                .events
                .send(crate::events::TrayEvent::Narration(format!(
                    "register {service} failed: {e}"
                )));
        }
    }

    /// 查询：当前在册 Item 清单。
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        registered_list(&self.bus)
    }

    /// 兼容桩：受理即忘——自包含设计不对进程外 Host 负责。
    fn register_status_notifier_host(&self, _service: &str) {}

    /// 兼容桩：恒真——逻辑 Host 进程内在位。
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(signal)]
    pub async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;
}

/// org.freedesktop 前缀（规范原文拼写）协议面——应答等价处理。
pub(crate) struct FreedesktopWatcherIface {
    pub(crate) bus: BusCenter,
}

#[zbus::interface(name = "org.freedesktop.StatusNotifierWatcher")]
impl FreedesktopWatcherIface {
    async fn register_status_notifier_item(&self, service: &str) {
        if let Err(e) = dbus::register_item(&self.bus, service, true).await {
            self.bus
                .events
                .send(crate::events::TrayEvent::Narration(format!(
                    "register {service} failed: {e}"
                )));
        }
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        registered_list(&self.bus)
    }

    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
}

/// 广播：Item 报到（协议面保真，供总线观察方）。
pub(crate) async fn emit_registered(bus: &BusCenter, service: &str) {
    if let Ok(emitter) = SignalEmitter::new(&bus.conn, WATCHER_PATH) {
        let _ = WatcherIface::status_notifier_item_registered(&emitter, service).await;
    }
}

/// 广播：Item 离开（死亡清理）。
pub(crate) async fn emit_unregistered(bus: &BusCenter, service: &str) {
    if let Ok(emitter) = SignalEmitter::new(&bus.conn, WATCHER_PATH) {
        let _ = WatcherIface::status_notifier_item_unregistered(&emitter, service).await;
    }
}
