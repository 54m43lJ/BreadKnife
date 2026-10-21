//! `org.freedesktop.Notifications` method handling.
//!
//! The four methods mirror the specification one-to-one; protocol details are
//! authoritative in `reference/9.dbus-protocol.html`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;
use zbus::zvariant::OwnedValue;

use crate::events::EventQueue;
use crate::hints::{self, HintSet};
use crate::signal;
use crate::types::{Action, Capabilities, CloseReason, Notice, NoticeEvent, ServerInfo};

/// Errors produced by the D-Bus service side.
///
/// These are deliberately separate from [`crate::Error`]: the public error
/// type describes host-facing API failures, while this one maps directly to
/// the two D-Bus error names in `PROVIDER §4.4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServerError {
    InvalidArgs(String),
    Failed(String),
}

impl From<ServerError> for zbus::fdo::Error {
    fn from(error: ServerError) -> Self {
        match error {
            ServerError::InvalidArgs(message) => zbus::fdo::Error::InvalidArgs(message),
            ServerError::Failed(message) => zbus::fdo::Error::Failed(message),
        }
    }
}

/// Protocol bookkeeping: id allocation and the set of live ids.
#[derive(Debug, Default)]
pub(crate) struct State {
    next_id: u32,
    active: HashSet<u32>,
}

impl State {
    pub(crate) fn allocate(&mut self) -> u32 {
        loop {
            let id = if self.next_id == 0 { 1 } else { self.next_id };
            self.next_id = if id == u32::MAX { 1 } else { id + 1 };
            if self.active.insert(id) {
                return id;
            }
        }
    }

    pub(crate) fn track(&mut self, id: u32) {
        if id != 0 {
            self.active.insert(id);
        }
    }

    pub(crate) fn contains(&self, id: u32) -> bool {
        self.active.contains(&id)
    }

    pub(crate) fn remove(&mut self, id: u32) -> bool {
        self.active.remove(&id)
    }
}

/// Raw `Notify` arguments after D-Bus deserialization.
pub(crate) struct NotifyParams {
    pub(crate) app_name: String,
    pub(crate) replaces_id: u32,
    pub(crate) app_icon: String,
    pub(crate) summary: String,
    pub(crate) body: String,
    pub(crate) actions: Vec<String>,
    pub(crate) hints: HashMap<String, OwnedValue>,
    pub(crate) expire_timeout: i32,
}

/// The object registered at `/org/freedesktop/Notifications`.
pub(crate) struct Server {
    caps: Capabilities,
    info: ServerInfo,
    events: Arc<EventQueue>,
    state: Arc<Mutex<State>>,
    stopped: Arc<AtomicBool>,
}

impl Server {
    pub(crate) fn new(
        caps: Capabilities,
        info: ServerInfo,
        events: Arc<EventQueue>,
        state: Arc<Mutex<State>>,
        stopped: Arc<AtomicBool>,
    ) -> Self {
        Self {
            caps,
            info,
            events,
            state,
            stopped,
        }
    }

    async fn notify_impl(&self, params: NotifyParams) -> Result<u32, ServerError> {
        if self.stopped.load(Ordering::SeqCst) || self.events.is_closed() {
            return Err(ServerError::Failed("session stopped".to_owned()));
        }

        validate_actions(&params.actions)?;
        let hint_set = hints::parse(&params.hints, &params.app_icon);

        // Hold the protocol lock across id assignment and event delivery so a
        // concurrent CloseNotification cannot overtake a Notify event.
        let mut state = self.state.lock().await;
        let id = if params.replaces_id == 0 {
            state.allocate()
        } else {
            state.track(params.replaces_id);
            params.replaces_id
        };

        let notice = build_notice(params, hint_set, id);
        if !self.events.send(NoticeEvent::Notify { notice }).await {
            state.remove(id);
            return Err(ServerError::Failed("event channel is closed".to_owned()));
        }

        Ok(id)
    }

    async fn close_impl(&self, conn: &zbus::Connection, id: u32) -> Result<(), ServerError> {
        if self.stopped.load(Ordering::SeqCst) || self.events.is_closed() {
            return Err(ServerError::Failed("session stopped".to_owned()));
        }

        // Keep the id invalidated and the signal ordered with respect to a
        // concurrent replacement of the same id.
        let mut state = self.state.lock().await;
        if !state.remove(id) {
            return Err(ServerError::InvalidArgs(String::new()));
        }

        let emitted = signal::emit_closed_on(conn, id, CloseReason::CloseRequest)
            .await
            .map_err(|error| ServerError::Failed(error.to_string()));
        let _ = self.events.send(NoticeEvent::Close { id }).await;
        emitted
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
    ) -> zbus::fdo::Result<u32> {
        self.notify_impl(NotifyParams {
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
        })
        .await
        .map_err(Into::into)
    }

    async fn close_notification(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        id: u32,
    ) -> zbus::fdo::Result<()> {
        self.close_impl(conn, id).await.map_err(Into::into)
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

fn validate_actions(actions: &[String]) -> Result<(), ServerError> {
    if !actions.len().is_multiple_of(2) {
        return Err(ServerError::InvalidArgs(
            "actions must contain key/label pairs".to_owned(),
        ));
    }
    Ok(())
}

fn parse_expire_timeout(expire_timeout: i32) -> Option<u32> {
    if expire_timeout < 0 {
        None
    } else {
        Some(expire_timeout as u32)
    }
}

fn build_notice(params: NotifyParams, hints: HintSet, id: u32) -> Notice {
    let actions = params
        .actions
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Action {
            key: pair[0].clone(),
            label: pair[1].clone(),
        })
        .collect();

    Notice {
        id,
        app_name: params.app_name,
        icon: hints.icon,
        summary: params.summary,
        body: params.body,
        actions,
        urgency: hints.urgency,
        expire_timeout_ms: parse_expire_timeout(params.expire_timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NoticeIcon, Urgency};

    #[test]
    fn ids_start_at_one_and_increment() {
        let mut state = State::default();
        assert_eq!(state.allocate(), 1);
        assert_eq!(state.allocate(), 2);
        assert_eq!(state.allocate(), 3);
    }

    #[test]
    fn allocation_wraps_without_returning_zero_or_reusing_live_ids() {
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
    fn replacement_tracking_and_removal() {
        let mut state = State::default();
        state.track(42);
        assert!(state.remove(42));
        assert!(!state.remove(42));
    }

    #[test]
    fn actions_must_be_pairs() {
        assert!(validate_actions(&["default".into(), "Open".into()]).is_ok());
        assert!(validate_actions(&["default".into()]).is_err());
    }

    #[test]
    fn expiration_timeout_follows_the_specification() {
        assert_eq!(parse_expire_timeout(-1), None);
        assert_eq!(parse_expire_timeout(-100), None);
        assert_eq!(parse_expire_timeout(0), Some(0));
        assert_eq!(parse_expire_timeout(5000), Some(5000));
    }

    #[test]
    fn build_notice_keeps_value_fields_and_pairs() {
        let params = NotifyParams {
            app_name: "app".into(),
            replaces_id: 9,
            app_icon: "".into(),
            summary: "summary".into(),
            body: "body".into(),
            actions: vec![
                "default".into(),
                "Open".into(),
                "dismiss".into(),
                "Close".into(),
            ],
            hints: HashMap::new(),
            expire_timeout: 3000,
        };
        let notice = build_notice(
            params,
            HintSet {
                urgency: Urgency::Critical,
                icon: NoticeIcon::Name("icon".into()),
            },
            9,
        );

        assert_eq!(notice.id, 9);
        assert_eq!(notice.app_name, "app");
        assert_eq!(notice.summary, "summary");
        assert_eq!(notice.body, "body");
        assert_eq!(
            notice.actions,
            vec![
                Action {
                    key: "default".into(),
                    label: "Open".into(),
                },
                Action {
                    key: "dismiss".into(),
                    label: "Close".into(),
                },
            ]
        );
        assert_eq!(notice.urgency, Urgency::Critical);
        assert_eq!(notice.expire_timeout_ms, Some(3000));
    }
}
