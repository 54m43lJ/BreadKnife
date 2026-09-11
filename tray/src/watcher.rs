//! Fallback `org.kde.StatusNotifierWatcher` implementation.
//!
//! Exported by the library when no external watcher is present (see
//! `FallbackPolicy::Auto`). Method calls from item applications are queued
//! into the shared runtime state; the generated SNI signals serve both the
//! bus and as the wake-up for the worker thread.

use std::sync::Weak;

use zbus::object_server::SignalEmitter;
use zbus::interface;

use crate::runtime::{self, Cmd, Shared};

/// The exported watcher object. Holds only a weak reference so the object
/// server never keeps the runtime alive.
pub(crate) struct WatcherIface {
    pub shared: Weak<Shared>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherIface {
    async fn register_status_notifier_item(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        service: String,
    ) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        {
            // Make the registration visible to `RegisteredStatusNotifierItems`
            // before this method returns: bus clients may enumerate right
            // away, while the worker materializes the item asynchronously.
            let mut st = shared.state.lock().unwrap();
            if shared.stopping.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            if !st.pending_registered.contains(&service) {
                st.pending_registered.push(service.clone());
            }
            st.pending.push_back(Cmd::ItemRegistered(service.clone()));
        }
        runtime::emit_wake(&shared.conn);
        let _ = Self::status_notifier_item_registered(&ctxt, &service).await;
    }

    async fn register_status_notifier_host(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[allow(unused_variables)] service: String,
    ) {
        let _ = Self::status_notifier_host_registered(&ctxt).await;
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        let Some(shared) = self.shared.upgrade() else {
            return Vec::new();
        };
        let st = shared.state.lock().unwrap();
        let mut items: Vec<String> = st
            .items
            .values()
            .map(|e| format!("{}{}", e.bus, e.path))
            .collect();
        items.extend(st.pending_registered.iter().cloned());
        items
    }

    #[zbus(signal)]
    pub async fn status_notifier_item_registered(
        ctxt: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn status_notifier_item_unregistered(
        ctxt: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn status_notifier_host_registered(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}
