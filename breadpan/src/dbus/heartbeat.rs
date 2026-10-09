//! 周期存活探测：结果写入登记簿记（alive 字段），不产生事件。

use std::time::Duration;

use crate::dbus::bus::BusCenter;
use crate::dbus::{DBUS_IFACE, DBUS_PATH, DBUS_SERVICE, ITEM_IFACE_KDE, PROPS_IFACE};

/// 探测周期（库内定值）。
pub(crate) const HEARTBEAT_PERIOD: Duration = Duration::from_secs(5);

/// 存活探测循环：
/// - 总线自身断连 → 上报 Fatal（终局）；
/// - 对每个在册 Item 做轻量属性读取（超时 timeout_ms）→ 覆盖 tracker 的 alive 字段。
pub(crate) async fn heartbeat_task(bus: BusCenter) {
    let mut tick = tokio::time::interval(HEARTBEAT_PERIOD);
    loop {
        tick.tick().await;

        // 总线存活探测： GetNameOwner(自身唯一名)
        let self_unique = bus.conn.unique_name().map(|n| n.to_string());
        let Some(self_unique) = self_unique else {
            continue;
        };
        let ping = tokio::time::timeout(
            bus.timeout,
            bus.conn.call_method(
                Some(DBUS_SERVICE),
                DBUS_PATH,
                Some(DBUS_IFACE),
                "GetNameOwner",
                &self_unique,
            ),
        )
        .await;
        match ping {
            Err(_) => continue, // 超时：视作繁忙，跳过本拍
            Ok(Err(zbus::Error::InputOutput(e))) => {
                bus.report_fatal(format!("session bus connection lost: {e}"));
                return;
            }
            Ok(Err(zbus::Error::Handshake(e))) => {
                bus.report_fatal(format!("session bus connection lost: {e}"));
                return;
            }
            Ok(Err(zbus::Error::Connection(e, _))) => {
                bus.report_fatal(format!("session bus connection lost: {e}"));
                return;
            }
            Ok(_) => {}
        }

        // 逐 Item 轻量读取（并发），结果仅写入登记簿记
        let entries = bus.tracker.read().unwrap().all();
        let mut joins = Vec::with_capacity(entries.len());
        for entry in entries {
            let bus2 = bus.clone();
            joins.push(bus.rt.spawn(async move {
                let ok = bus2
                    .call::<zbus::zvariant::OwnedValue, _>(
                        &entry.service,
                        &entry.path,
                        PROPS_IFACE,
                        "Get",
                        &(ITEM_IFACE_KDE, "Status"),
                    )
                    .await
                    .is_ok();
                (entry.service.clone(), ok)
            }));
        }
        for join in joins {
            if let Ok((service, ok)) = join.await {
                bus.tracker.write().unwrap().set_alive(&service, ok);
            }
        }
    }
}
