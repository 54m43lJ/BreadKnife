//! 登记中心服务：名持有（不可替换）、所有权丢失上报（经 NameOwnerChanged 任务）、停机。

use zbus::fdo::{RequestNameFlags, RequestNameReply};

use crate::dbus::bus::BusCenter;
use crate::dbus::iface::{FreedesktopWatcherIface, WatcherIface};
use crate::dbus::{WATCHER_NAME, WATCHER_PATH};
use crate::TrayError;

pub(crate) struct WatcherService;

/// 挂载协议面（不申请名）——供 bootstrap 在快照就绪前置位。
pub(crate) async fn mount_interfaces(bus: &BusCenter) -> Result<(), TrayError> {
    bus.conn
        .object_server()
        .at(WATCHER_PATH, WatcherIface { bus: bus.clone() })
        .await?;
    bus.conn
        .object_server()
        .at(WATCHER_PATH, FreedesktopWatcherIface { bus: bus.clone() })
        .await?;
    Ok(())
}

impl WatcherService {
    /// 申请 Watcher 名（不声明 allow-replacement，持有期不可被替换）；
    /// 名字已被他人持有 → NameTaken（多 Watcher 不能共存）。
    /// 协议面须已挂载（bootstrap 在快照就绪前完成挂载，名最后申请——
    /// 申请前置会让 Item 报到与快照收集竞态，破坏"首事件即快照"契约）。
    pub(crate) async fn start(bus: &BusCenter) -> Result<Self, TrayError> {
        let reply = bus
            .conn
            .request_name_with_flags(WATCHER_NAME, RequestNameFlags::DoNotQueue.into())
            .await?;
        match reply {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(Self),
            other => Err(TrayError::NameTaken(format!(
                "{WATCHER_NAME} already owned elsewhere (reply: {other:?})"
            ))),
        }
    }
}

/// 释放名字（shutdown 路径）；幂等。受 timeout 约束，总线异常时不拖死停机路径。
pub(crate) async fn release(bus: &BusCenter) {
    let _ = tokio::time::timeout(bus.timeout, bus.conn.release_name(WATCHER_NAME)).await;
}
