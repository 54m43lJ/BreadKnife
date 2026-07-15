pub mod icon;
pub mod item;
pub mod menu;
pub mod watcher;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, atomic::AtomicBool, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use gtk::glib;

use crate::sni::item::{SniEvent, SniItemState, UpdateKind};
use crate::sni::menu::{MenuRequest, SniRequest};

// ── global state ─────────────────────────────────────────────────

/// Items map — accessed only from GTK main thread; Mutex for static Sync requirement
static ITEMS: OnceLock<Mutex<HashMap<String, SniItemState>>> = OnceLock::new();

/// Request channel sender to background thread
static REQUEST_TX: OnceLock<mpsc::Sender<SniRequest>> = OnceLock::new();

// Callbacks — stored in thread-local since GTK types are !Send
thread_local! {
    static CB_ADDED: RefCell<Option<Box<dyn Fn(&str)>>> = RefCell::new(None);
    static CB_REMOVED: RefCell<Option<Box<dyn Fn(&str)>>> = RefCell::new(None);
    static CB_UPDATED: RefCell<Option<Box<dyn Fn(&str, UpdateKind)>>> = RefCell::new(None);
}

// ── public API ───────────────────────────────────────────────────

pub fn init_sni_host() {
    eprintln!("[sni::init] starting");

    ITEMS.get_or_init(|| Mutex::new(HashMap::new()));

    let (event_tx, event_rx) = mpsc::channel::<SniEvent>();
    let (req_tx, req_rx) = mpsc::channel::<SniRequest>();
    REQUEST_TX.set(req_tx).ok();

    let shutdown = Arc::new(AtomicBool::new(false));

    thread::spawn({
        let shutdown = shutdown.clone();
        move || {
            eprintln!("[sni::bg] thread spawned, entering run_watcher");
            watcher::run_watcher(event_tx, req_rx, shutdown);
            eprintln!("[sni::bg] run_watcher returned");
        }
    });

    // GTK timer: drain events
    glib::timeout_add_local(Duration::from_millis(100), move || {
        loop {
            match event_rx.try_recv() {
                Ok(event) => {
                    eprintln!("[sni::main] event: {:?}", event);
                    handle_event(event)
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[sni::main] event channel disconnected");
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });

    eprintln!("[sni::init] done");
}

fn handle_event(event: SniEvent) {
    let items = ITEMS.get().expect("SNI not initialized");

    match event {
        SniEvent::ItemAdded { ref id, .. } => {
            eprintln!("[sni::main] ItemAdded {id} — calling cb");
            let state = match &event {
                SniEvent::ItemAdded { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            items.lock().unwrap().insert(id.clone(), state);
            CB_ADDED.with(|cb| {
                if let Some(ref cb) = *cb.borrow() {
                    cb(id);
                } else {
                    eprintln!("[sni::main] ItemAdded — no callback registered");
                }
            });
        }
        SniEvent::ItemRemoved { ref id } => {
            eprintln!("[sni::main] ItemRemoved {id}");
            items.lock().unwrap().remove(id.as_str());
            CB_REMOVED.with(|cb| {
                if let Some(ref cb) = *cb.borrow() {
                    cb(id);
                }
            });
        }
        SniEvent::ItemUpdated { ref id, ref kind, .. } => {
            let state = match &event {
                SniEvent::ItemUpdated { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            items.lock().unwrap().insert(id.clone(), state);
            CB_UPDATED.with(|cb| {
                if let Some(ref cb) = *cb.borrow() {
                    cb(id, kind.clone());
                }
            });
        }
    }
}

// ── callbacks ────────────────────────────────────────────────────

pub fn set_item_added_cb<F: Fn(&str) + 'static>(cb: F) {
    CB_ADDED.with(|c| *c.borrow_mut() = Some(Box::new(cb)));
}

pub fn set_item_removed_cb<F: Fn(&str) + 'static>(cb: F) {
    CB_REMOVED.with(|c| *c.borrow_mut() = Some(Box::new(cb)));
}

pub fn set_item_updated_cb<F: Fn(&str, UpdateKind) + 'static>(cb: F) {
    CB_UPDATED.with(|c| *c.borrow_mut() = Some(Box::new(cb)));
}

// ── queries ──────────────────────────────────────────────────────

pub fn get_item_state(id: &str) -> Option<SniItemState> {
    ITEMS.get()?.lock().unwrap().get(id).cloned()
}

pub fn item_ids() -> Vec<String> {
    ITEMS
        .get()
        .map(|m| m.lock().unwrap().keys().cloned().collect())
        .unwrap_or_default()
}

// ── actions ──────────────────────────────────────────────────────

/// Send Activate D-Bus call (fire-and-forget)
pub fn activate_item(id: &str, x: i32, y: i32) {
    let state = match get_item_state(id) {
        Some(s) => s,
        None => return,
    };
    if let Some(tx) = REQUEST_TX.get() {
        let _ = tx.send(SniRequest::Activate {
            bus_name: state.bus_name,
            object_path: state.object_path,
            x,
            y,
        });
    }
}

/// Send SecondaryActivate D-Bus call (fire-and-forget)
pub fn secondary_activate_item(id: &str, x: i32, y: i32) {
    let state = match get_item_state(id) {
        Some(s) => s,
        None => return,
    };
    if let Some(tx) = REQUEST_TX.get() {
        let _ = tx.send(SniRequest::SecondaryActivate {
            bus_name: state.bus_name,
            object_path: state.object_path,
            x,
            y,
        });
    }
}

/// Fetch dbusmenu for item. Blocks briefly (≤500ms) for background thread response.
/// Call from GTK main thread only.
pub fn request_menu<F>(item_id: &str, on_ready: F)
where
    F: FnOnce(Option<menu::MenuResult>) + 'static,
{
    let state = match get_item_state(item_id) {
        Some(s) => s,
        None => {
            on_ready(None);
            return;
        }
    };

    if state.menu_path.is_empty() {
        on_ready(None);
        return;
    }

    let req_tx = match REQUEST_TX.get() {
        Some(tx) => tx.clone(),
        None => {
            on_ready(None);
            return;
        }
    };

    let (reply_tx, reply_rx) = mpsc::channel();

    let req = SniRequest::Menu(MenuRequest {
        bus_name: state.bus_name.clone(),
        menu_path: state.menu_path.clone(),
        reply: reply_tx,
    });

    if req_tx.send(req).is_err() {
        on_ready(None);
        return;
    }

    let result = reply_rx.recv_timeout(Duration::from_millis(500)).ok();
    on_ready(result);
}
