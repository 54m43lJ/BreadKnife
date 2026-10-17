//! `org.freedesktop.Notifications` 服务端方法应答。
//!
//! 规范条文见 `bnote/reference/notification-spec.clean.html §9.1`。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc::Sender};
use zbus::zvariant::OwnedValue;

use crate::hints;
use crate::signal;
use crate::types::{Capabilities, CloseReason, Notice, NoticeEvent, ServerInfo};

/// 协议层状态：仅 id 分配与在册 id 集合，不保存通知业务数据。
#[derive(Debug, Default)]
pub(crate) struct State {
    next_id: u32,
    active: HashSet<u32>,
}

impl State {
    fn allocate(&mut self) -> u32 {
        loop {
            let id = self.next_id.max(1);
            self.next_id = id.checked_add(1).unwrap_or(1);
            if self.active.insert(id) {
                return id;
            }
        }
    }

    fn track(&mut self, id: u32) {
        self.active.insert(id);
    }

    fn remove(&mut self, id: u32) -> bool {
        self.active.remove(&id)
    }
}

pub(crate) struct Server {
    state: Arc<Mutex<State>>,
    tx: Sender<NoticeEvent>,
    caps: Capabilities,
    info: ServerInfo,
}

impl Server {
    pub(crate) fn new(
        state: Arc<Mutex<State>>,
        tx: Sender<NoticeEvent>,
        caps: Capabilities,
        info: ServerInfo,
    ) -> Self {
        Self {
            state,
            tx,
            caps,
            info,
        }
    }

    /// 宿主回发关闭后同步协议层在册集合，使后续 `CloseNotification` 按规范报错。
    ///
    /// 返回该 id 是否仍在册；重复关闭返回 `false`，调用方不应重复发射信号。
    pub(crate) async fn mark_closed(&self, id: u32) -> bool {
        self.state.lock().await.remove(id)
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    async fn get_capabilities(&self) -> Vec<String> {
        self.caps.entries.clone()
    }

    #[zbus(out_args("id"))]
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let hint_set = hints::parse(&hints, &app_icon, &actions, expire_timeout);

        let id = {
            let mut state = self.state.lock().await;
            if replaces_id == 0 {
                state.allocate()
            } else {
                state.track(replaces_id);
                replaces_id
            }
        };

        let notice = Notice {
            id,
            app_name,
            icon: hint_set.icon,
            summary,
            body,
            actions: hint_set.actions,
            urgency: hint_set.urgency,
            expire_timeout_ms: hint_set.expire_timeout_ms,
        };
        let _ = self.tx.send(NoticeEvent::Notify { notice }).await;

        id
    }

    async fn close_notification(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        id: u32,
    ) -> zbus::fdo::Result<()> {
        let existed = self.state.lock().await.remove(id);
        if !existed {
            // 规范 §9.1.3：通知已不存在时返回空 D-Bus Error。
            return Err(zbus::fdo::Error::Failed(String::new()));
        }

        let _ = self.tx.send(NoticeEvent::Close { id }).await;
        signal::emit_closed(conn, id, CloseReason::CloseRequest)
            .await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;

        Ok(())
    }

    #[zbus(out_args("name", "vendor", "version", "spec_version"))]
    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            self.info.name.clone(),
            self.info.vendor.clone(),
            self.info.version.clone(),
            self.info.spec_version.clone(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn activation_token(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        token: &str,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_start_at_one_and_increment() {
        let mut state = State::default();
        assert_eq!(state.allocate(), 1);
        assert_eq!(state.allocate(), 2);
        assert_eq!(state.allocate(), 3);
    }

    #[test]
    fn allocation_wraps_without_returning_zero_or_reusing_active_ids() {
        let mut state = State {
            next_id: u32::MAX,
            active: HashSet::from([u32::MAX]),
        };
        assert_eq!(state.allocate(), 1);
        assert_eq!(state.next_id, 2);

        state.track(2);
        state.next_id = 2;
        assert_eq!(state.allocate(), 3);
    }

    #[test]
    fn replacement_tracks_and_close_removes() {
        let mut state = State::default();
        state.track(42);
        assert!(state.remove(42));
        assert!(!state.active.contains(&42));
        assert!(!state.remove(42));
    }
}
