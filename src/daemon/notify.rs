//! org.freedesktop.Notifications 实现（通知守护角色）。
//! 名被占（会话已有通知守护）时降级：仅日志，不影响其余功能。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use zbus::interface;

use crate::daemon::DaemonEvent;
use crate::shared::model::NotificationRecord;

pub async fn serve(conn: &zbus::Connection, tx: Sender<DaemonEvent>) -> Result<(), zbus::Error> {
    let iface = Notifications { tx, next_id: AtomicU32::new(1) };
    conn.object_server()
        .at("/org/freedesktop/Notifications", iface)
        .await?;
    conn.request_name("org.freedesktop.Notifications").await?;
    eprintln!("[notify] org.freedesktop.Notifications registered");
    Ok(())
}

struct Notifications {
    tx: Sender<DaemonEvent>,
    next_id: AtomicU32,
}

#[interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        // replaces_id != 0：复用原 id（窗口侧按 id 覆盖更新）
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed).max(1)
        };
        let record = NotificationRecord { id, app_name, icon, summary, body };
        let _ = self.tx.send(DaemonEvent::Notification(record));
        id
    }

    async fn close_notification(&self, _id: u32) {
        // 通知中心语义：缓存不因超时移除，无动作
    }

    async fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "icon-static"]
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        ("bread".into(), "breadknife".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}
