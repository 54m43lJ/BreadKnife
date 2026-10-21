//! Host-facing session, handle and event receiver.
//!
//! `Session::start` owns a dedicated thread and a current-thread Tokio
//! runtime.  The host-facing methods are synchronous as required by the
//! contract; only [`NoticeReceiver::recv`] is asynchronous.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, ThreadId};

use tokio::sync::mpsc;

use crate::bus::Bus;
use crate::events::EventQueue;
use crate::types::{CloseReason, NoticeEvent, SessionConfig};
use crate::{Error, signal};

/// Entry point for assembling the notification service.
pub struct Session;

impl Session {
    /// Connect to the session bus, register
    /// `org.freedesktop.Notifications`, and return the host handle and event
    /// receiver.
    ///
    /// This call blocks until the service is ready.
    pub fn start(config: SessionConfig) -> Result<(SessionHandle, NoticeReceiver), Error> {
        validate_config(&config)?;

        let (events, receiver) = EventQueue::new();
        let (ready_rx, join) = Bus::spawn_runtime(config, Arc::clone(&events))?;

        let bus = match ready_rx.recv() {
            Ok(Ok(bus)) => bus,
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(_) => {
                let _ = join.join();
                return Err(Error::Bus(
                    "bnote runtime terminated during startup".to_owned(),
                ));
            }
        };

        let thread_id = join.thread().id();
        let shared = Arc::new(RuntimeShared {
            bus: Mutex::new(Some(bus)),
            join: Mutex::new(Some(join)),
            thread_id,
            handles: AtomicUsize::new(1),
        });

        Ok((SessionHandle { shared }, receiver))
    }
}

/// Cloneable command handle.  The session stops when the last clone is
/// dropped, or earlier when [`SessionHandle::shutdown`] is called.
pub struct SessionHandle {
    shared: Arc<RuntimeShared>,
}

impl Clone for SessionHandle {
    fn clone(&self) -> Self {
        self.shared.handles.fetch_add(1, Ordering::SeqCst);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        if self.shared.handles.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.shared.shutdown();
        }
    }
}

impl SessionHandle {
    /// Close a notification and emit `NotificationClosed` with `reason`.
    ///
    /// The identifier is invalidated before the signal is sent.  Calling this
    /// for an inactive id returns [`Error::InvalidArgs`].
    pub fn close(&self, id: u32, reason: CloseReason) -> Result<(), Error> {
        let bus = self.shared.bus()?;
        bus.cmd(move |bus| async move {
            let mut state = bus.state.lock().await;
            if !state.remove(id) {
                return Err(Error::InvalidArgs(format!(
                    "notification id {id} is not active"
                )));
            }
            signal::emit_closed(&bus, id, reason).await
        })
    }

    /// Emit `ActionInvoked` for the given notification and action key.
    ///
    /// The id must still be active; the action key is currently not checked
    /// against the notification's action list.
    pub fn invoke_action(&self, id: u32, action_key: &str) -> Result<(), Error> {
        let bus = self.shared.bus()?;
        let action_key = action_key.to_owned();
        bus.cmd(move |bus| async move {
            let state = bus.state.lock().await;
            if !state.contains(id) {
                return Err(Error::InvalidArgs(format!(
                    "notification id {id} is not active"
                )));
            }
            signal::emit_action_invoked(&bus, id, &action_key).await
        })
    }

    /// Emit `ActivationToken` for the given notification.
    ///
    /// The id must still be active.
    pub fn activation_token(&self, id: u32, token: &str) -> Result<(), Error> {
        let bus = self.shared.bus()?;
        let token = token.to_owned();
        bus.cmd(move |bus| async move {
            let state = bus.state.lock().await;
            if !state.contains(id) {
                return Err(Error::InvalidArgs(format!(
                    "notification id {id} is not active"
                )));
            }
            signal::emit_activation_token(&bus, id, &token).await
        })
    }

    /// Stop the session.  This is idempotent and blocks until the internal
    /// runtime has been joined.
    pub fn shutdown(&self) {
        self.shared.shutdown();
    }
}

/// Single-consumer event stream.
pub struct NoticeReceiver {
    rx: mpsc::UnboundedReceiver<NoticeEvent>,
}

impl NoticeReceiver {
    pub(crate) fn new(rx: mpsc::UnboundedReceiver<NoticeEvent>) -> Self {
        Self { rx }
    }

    /// Wait for the next event.  Returns `None` after the session stops or
    /// after a [`NoticeEvent::Fatal`] event.
    pub async fn recv(&mut self) -> Option<NoticeEvent> {
        self.rx.recv().await
    }
}

pub(crate) struct RuntimeShared {
    bus: Mutex<Option<Bus>>,
    join: Mutex<Option<JoinHandle<()>>>,
    thread_id: ThreadId,
    handles: AtomicUsize,
}

impl RuntimeShared {
    fn bus(&self) -> Result<Bus, Error> {
        self.bus
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(Error::Stopped)
    }

    pub(crate) fn shutdown(&self) {
        {
            let guard = self
                .bus
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(bus) = guard.as_ref() {
                bus.shutdown();
            }
        }

        // A handle must never join its own runtime thread.
        if std::thread::current().id() == self.thread_id {
            return;
        }

        let join = self
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(join) = join {
            let _ = join.join();
        }

        // Drop the connection so the bus name and object are released.
        let bus = self
            .bus
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(bus);
    }
}

fn validate_config(config: &SessionConfig) -> Result<(), Error> {
    let mut icon_static = false;
    let mut icon_multi = false;

    for entry in &config.capabilities.entries {
        if entry.is_empty()
            || !entry
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            return Err(Error::InvalidArgs(format!(
                "invalid capability name: {entry:?}"
            )));
        }

        match entry.as_str() {
            "icon-static" => icon_static = true,
            "icon-multi" => icon_multi = true,
            _ => {}
        }
    }

    if icon_static && icon_multi {
        return Err(Error::InvalidArgs(
            "capabilities icon-static and icon-multi are mutually exclusive".to_owned(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Capabilities, ServerInfo};

    fn config(capabilities: &[&str]) -> SessionConfig {
        SessionConfig {
            capabilities: Capabilities::new(capabilities.iter().copied()),
            server_info: ServerInfo {
                name: "bnote-test".into(),
                vendor: "breadknife".into(),
                version: "0.1.0".into(),
                spec_version: "1.2".into(),
            },
            command_timeout_ms: 1000,
        }
    }

    #[test]
    fn capability_names_are_validated() {
        assert!(validate_config(&config(&["actions", "body-markup"])).is_ok());
        assert!(validate_config(&config(&["bad cap"])).is_err());
        assert!(validate_config(&config(&[""])).is_err());
    }

    #[test]
    fn icon_capabilities_are_mutually_exclusive() {
        assert!(validate_config(&config(&["icon-static"])).is_ok());
        assert!(validate_config(&config(&["icon-multi"])).is_ok());
        assert!(validate_config(&config(&["icon-static", "icon-multi"])).is_err());
    }

    #[test]
    fn public_handles_have_the_required_auto_traits() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_send<T: Send>() {}

        assert_send_sync::<Session>();
        assert_send_sync::<SessionHandle>();
        assert_send::<NoticeReceiver>();
    }
}
