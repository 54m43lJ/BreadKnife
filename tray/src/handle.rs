//! Public entry points: `Tray`, `TrayBuilder` and `TrayHandle`.

use std::sync::{mpsc, Arc};

use zbus::zvariant::ObjectPath;

use crate::{
    config::TrayConfig,
    error::TrayError,
    event::TrayEvent,
    menu::MenuSnapshot,
    model::{Orientation, TrayItem, TrayItemId},
    runtime::{self, Cmd, Shared},
    InteractionOp, MENU_IFACE,
};

/// Builder-style entry point (callback primitive, SPEC §5.1).
pub struct TrayBuilder {
    config: TrayConfig,
    callbacks: Vec<Arc<dyn Fn(&TrayEvent) + Send + Sync>>,
}

impl TrayBuilder {
    /// Register an event callback. May be called multiple times; callbacks
    /// are invoked serially on the library's internal event thread.
    pub fn on_event(mut self, cb: impl Fn(&TrayEvent) + Send + Sync + 'static) -> Self {
        self.callbacks.push(Arc::new(cb));
        self
    }

    /// Assemble and start the runtime. Blocks until host registration
    /// completes (or fails).
    pub fn spawn(self) -> Result<TrayHandle, TrayError> {
        let (handle, _) = self.spawn_with_channel()?;
        Ok(handle)
    }

    /// Like [`TrayBuilder::spawn`] but also returns the default event
    /// channel (what [`Tray::connect`] returns).
    pub fn spawn_with_channel(
        self,
    ) -> Result<(TrayHandle, mpsc::Receiver<TrayEvent>), TrayError> {
        let (handle, rx) = runtime::start(self.config, self.callbacks)?;
        Ok((TrayHandle { shared: handle }, rx))
    }
}

/// Static entry point of the library.
pub struct Tray;

impl Tray {
    /// Connect to the session bus and start the runtime.
    ///
    /// Blocks until the host is registered with the watcher (or the fallback
    /// watcher is active). Returns the handle plus the default event channel.
    pub fn connect(
        config: TrayConfig,
    ) -> Result<(TrayHandle, mpsc::Receiver<TrayEvent>), TrayError> {
        TrayBuilder {
            config,
            callbacks: Vec::new(),
        }
        .spawn_with_channel()
    }

    /// Build with callback-based event delivery only.
    pub fn builder(config: TrayConfig) -> TrayBuilder {
        TrayBuilder {
            config,
            callbacks: Vec::new(),
        }
    }
}

/// Consumer-facing handle. `Clone + Send + Sync`.
#[derive(Clone)]
pub struct TrayHandle {
    pub(crate) shared: Arc<Shared>,
}

impl TrayHandle {
    /// Snapshot of all current items.
    pub fn items(&self) -> Vec<TrayItem> {
        let st = self.shared.state.lock().unwrap();
        st.items.values().map(|e| e.snapshot.clone()).collect()
    }

    /// Snapshot of one item.
    pub fn item(&self, id: &TrayItemId) -> Option<TrayItem> {
        let st = self.shared.state.lock().unwrap();
        st.items.get(id).map(|e| e.snapshot.clone())
    }

    /// Menu snapshot of one item, if it has a menu and it has been fetched.
    pub fn menu(&self, id: &TrayItemId) -> Option<MenuSnapshot> {
        let st = self.shared.state.lock().unwrap();
        st.items.get(id).and_then(|e| e.menu.clone())
    }

    /// Fire `Activate(x, y)` at the item. Result arrives as an
    /// [`TrayEvent::InteractionResult`] receipt.
    pub fn activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError> {
        runtime::enqueue(
            &self.shared,
            Cmd::Interact {
                id: id.clone(),
                op: InteractionOp::Activate,
                x,
                y,
                delta: 0,
                orient: Orientation::Horizontal,
            },
        )
    }

    /// Fire `SecondaryActivate(x, y)` (typically the right-click action).
    pub fn secondary_activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError> {
        runtime::enqueue(
            &self.shared,
            Cmd::Interact {
                id: id.clone(),
                op: InteractionOp::SecondaryActivate,
                x,
                y,
                delta: 0,
                orient: Orientation::Horizontal,
            },
        )
    }

    /// Fire a scroll event at the item.
    pub fn scroll(
        &self,
        id: &TrayItemId,
        delta: i32,
        orient: Orientation,
    ) -> Result<(), TrayError> {
        runtime::enqueue(
            &self.shared,
            Cmd::Interact {
                id: id.clone(),
                op: InteractionOp::Scroll,
                x: 0,
                y: 0,
                delta,
                orient,
            },
        )
    }

    /// Trigger a menu entry (dbusmenu `Event(id, "clicked", ...)`). The
    /// receipt arrives as an `InteractionResult`.
    pub fn menu_activate(&self, id: &TrayItemId, menu_item_id: i32) -> Result<(), TrayError> {
        runtime::enqueue(
            &self.shared,
            Cmd::MenuActivate {
                id: id.clone(),
                menu_item: menu_item_id,
            },
        )
    }

    /// Notify the item that a menu is about to be shown (SPEC-correct
    /// pre-show call for `menu_item_id`, 0 = root).
    ///
    /// Synchronous. After the call the library refreshes the menu snapshot
    /// for this item, because lazy DBusMenu providers only populate their
    /// layout in response to AboutToShow — read `menu()` right after this
    /// returns to present up-to-date content. The boolean is the item's
    /// "layout changed" hint.
    pub fn menu_about_to_show(&self, id: &TrayItemId, menu_item_id: i32) -> Result<bool, TrayError> {
        let (bus, menu_path) = {
            let st = self.shared.state.lock().unwrap();
            if self.shared.stopping.load(std::sync::atomic::Ordering::Acquire) {
                return Err(TrayError::Stopped);
            }
            let entry = st.items.get(id).ok_or_else(|| TrayError::item_not_found(id))?;
            match &entry.menu_path {
                Some(mp) => (entry.bus.clone(), mp.clone()),
                None => return Err(TrayError::Protocol("item has no menu".into())),
            }
        };
        let proxy = zbus::blocking::Proxy::new(
            &self.shared.conn,
            bus,
            ObjectPath::try_from(menu_path.as_str())?,
            MENU_IFACE,
        )?;
        let need_update = proxy.call::<_, _, bool>("AboutToShow", &(menu_item_id,))?;
        runtime::refresh_menu_sync(&self.shared, id);
        Ok(need_update)
    }

    /// Subscribe an additional event channel.
    pub fn subscribe(&self) -> mpsc::Receiver<TrayEvent> {
        let (tx, rx) = mpsc::channel();
        let mut st = self.shared.state.lock().unwrap();
        let id = st.next_subscriber_id;
        st.next_subscriber_id += 1;
        st.subscribers.push((id, tx));
        rx
    }

    /// Graceful shutdown: unregister host, release fallback watcher, stop
    /// all internal threads. Idempotent.
    pub fn shutdown(&self) {
        let _ = runtime::enqueue(&self.shared, Cmd::Shutdown);
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        // Last handle leaving triggers an automatic shutdown (SPEC §9).
        if Arc::strong_count(&self.shared) == 1 {
            self.shutdown();
        }
    }
}
