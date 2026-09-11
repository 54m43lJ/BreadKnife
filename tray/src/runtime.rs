//! Internal runtime: worker thread, wake/timer machinery and all DBus
//! plumbing. Nothing here is part of the public API.

use std::{
    collections::{BinaryHeap, HashMap, VecDeque},
    process,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Condvar, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use zbus::{
    blocking::{fdo::DBusProxy, Connection},
    fdo::{RequestNameFlags, RequestNameReply},
    message::{Message, Type as MsgType},
    MatchRule,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use crate::{
    config::{FallbackPolicy, TrayConfig},
    error::TrayError,
    event::{ItemField, TrayEvent},
    menu::{fetch_menu, MenuSnapshot},
    model::{Orientation, TrayItem, TrayItemId},
    sni::{
        build_item, get_item_properties, parse_service, SNI_HOST_PREFIX, SNI_ITEM_IFACE,
        SNI_WATCHER, SNI_WATCHER_IFACE, SNI_WATCHER_PATH, MENU_IFACE,
    },
    watcher::WatcherIface,
    InteractionOp, WatcherState,
};

/// Internal broadcast signal used to wake the worker thread. Any thread may
/// emit it; the worker's catch-all signal match rule receives it.
const WAKE_IFACE: &str = "org.breadknife.tray.Internal";
const WAKE_PATH: &str = "/org/breadknife/tray";
const WAKE_MEMBER: &str = "Wake";

static HOST_COUNTER: AtomicU32 = AtomicU32::new(0);

pub(crate) struct Shared {
    pub conn: Connection,
    pub config: TrayConfig,
    pub state: Mutex<State>,
    pub timers: TimerWheel,
    pub stopping: AtomicBool,
    pub host_id: Mutex<String>,
}

pub(crate) struct State {
    pub items: HashMap<TrayItemId, ItemEntry>,
    pub watcher: WatcherState,
    pub pending: VecDeque<Cmd>,
    /// Registrations accepted by the fallback watcher whose items have not
    /// been materialized yet. Kept separate so the
    /// `RegisteredStatusNotifierItems` property is consistent the moment
    /// `RegisterStatusNotifierItem` returns (bus clients may enumerate
    /// immediately afterwards).
    pub pending_registered: Vec<String>,
    /// (deadline, work) pairs; the ticker only *wakes* us, due entries are
    /// processed here.
    pub timers: Vec<(Instant, TimerKind)>,
    pub next_subscriber_id: u64,
    pub subscribers: Vec<(u64, mpsc::Sender<TrayEvent>)>,
    pub callbacks: Vec<Arc<dyn Fn(&TrayEvent) + Send + Sync>>,
    pub dropped_events: u64,
}

pub(crate) struct ItemEntry {
    pub bus: String,
    /// Resolved unique bus name, used to attribute signals to items.
    pub unique: String,
    pub path: String,
    pub menu_path: Option<String>,
    pub snapshot: TrayItem,
    pub menu: Option<MenuSnapshot>,
}

#[derive(Debug, Clone)]
pub(crate) enum TimerKind {
    RefetchItem(TrayItemId),
    RefetchMenu(TrayItemId),
    WatcherRespawn,
}

pub(crate) enum Cmd {
    ItemRegistered(String),
    Interact {
        id: TrayItemId,
        op: InteractionOp,
        x: i32,
        y: i32,
        delta: i32,
        orient: Orientation,
    },
    MenuActivate {
        id: TrayItemId,
        menu_item: i32,
    },
    Shutdown,
}

/// Condvar-based wake scheduler. Values here are only used to unblock the
/// ticker thread at the right time; the authoritative timer list lives in
/// [`State::timers`].
#[derive(Default)]
pub(crate) struct TimerWheel {
    deadlines: Mutex<BinaryHeap<std::cmp::Reverse<Instant>>>,
    cv: Condvar,
}

impl TimerWheel {
    fn wake_at(&self, at: Instant) {
        self.deadlines.lock().unwrap().push(std::cmp::Reverse(at));
        self.cv.notify_all();
    }

    fn stop(&self) {
        self.cv.notify_all();
    }

    /// Block until some deadline is due (returns them) or `stopping` is set
    /// (returns empty).
    fn wait(&self, stopping: &AtomicBool) -> Vec<Instant> {
        let mut guard = self.deadlines.lock().unwrap();
        loop {
            if stopping.load(Ordering::Acquire) {
                return Vec::new();
            }
            let now = Instant::now();
            let mut due = Vec::new();
            while let Some(std::cmp::Reverse(at)) = guard.peek().copied() {
                if at <= now {
                    guard.pop();
                    due.push(at);
                } else {
                    break;
                }
            }
            if !due.is_empty() {
                return due;
            }
            match guard.peek().copied() {
                Some(std::cmp::Reverse(at)) => {
                    let (g, timeout) = self
                        .cv
                        .wait_timeout(guard, at.saturating_duration_since(now))
                        .unwrap();
                    guard = g;
                    if timeout.timed_out() {
                        continue;
                    }
                }
                None => {
                    guard = self.cv.wait(guard).unwrap();
                }
            }
        }
    }
}

/// Enqueue a command for the worker and wake it.
pub(crate) fn enqueue(shared: &Shared, cmd: Cmd) -> Result<(), TrayError> {
    {
        let mut st = shared.state.lock().unwrap();
        if shared.stopping.load(Ordering::Acquire) {
            return Err(TrayError::Stopped);
        }
        st.pending.push_back(cmd);
    }
    emit_wake(&shared.conn);
    Ok(())
}

/// Synchronous part of `Tray::connect`: bus connect + watcher probe + host
/// registration. Then spawns the worker and ticker threads and returns the
/// default event channel.
pub(crate) fn start(
    config: TrayConfig,
    callbacks: Vec<Arc<dyn Fn(&TrayEvent) + Send + Sync>>,
) -> Result<(Arc<Shared>, mpsc::Receiver<TrayEvent>), TrayError> {
    let conn = Connection::session().map_err(|e| TrayError::Bus(e.to_string()))?;
    let shared = Arc::new(Shared {
        conn,
        config,
        state: Mutex::new(State {
            items: HashMap::new(),
            watcher: WatcherState::ExternalDown,
            pending: VecDeque::new(),
            pending_registered: Vec::new(),
            timers: Vec::new(),
            next_subscriber_id: 0,
            subscribers: Vec::new(),
            callbacks,
            dropped_events: 0,
        }),
        timers: TimerWheel::default(),
        stopping: AtomicBool::new(false),
        host_id: Mutex::new(String::new()),
    });

    let (tx, rx) = mpsc::channel();
    {
        let mut st = shared.state.lock().unwrap();
        let id = st.next_subscriber_id;
        st.next_subscriber_id += 1;
        st.subscribers.push((id, tx));
    }

    setup_watcher_and_host(&shared)?;
    spawn_threads(&shared)?;
    Ok((shared, rx))
}

/// Spawn all runtime threads.
fn spawn_threads(shared: &Arc<Shared>) -> Result<(), TrayError> {
    // The catch-all signal rule is installed BEFORE initial enumeration so
    // that registration signals racing with the enumeration are buffered.
    let rule = MatchRule::builder().msg_type(MsgType::Signal).build();
    let iter = zbus::blocking::MessageIterator::for_match_rule(rule, &shared.conn, Some(1024))?;

    let wshared = Arc::downgrade(shared);
    thread::Builder::new()
        .name("tray-worker".into())
        .spawn(move || worker_main(wshared, iter))
        .map_err(|e| TrayError::Protocol(e.to_string()))?;

    let tshared = Arc::downgrade(shared);
    thread::Builder::new()
        .name("tray-ticker".into())
        .spawn(move || ticker_main(tshared))
        .map_err(|e| TrayError::Protocol(e.to_string()))?;
    Ok(())
}

fn ticker_main(shared: Weak<Shared>) {
    loop {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if shared.stopping.load(Ordering::Acquire) {
            return;
        }
        let due = shared.timers.wait(&shared.stopping);
        if due.is_empty() {
            return; // stopped
        }
        emit_wake(&shared.conn);
    }
}

fn worker_main(shared: Weak<Shared>, mut iter: zbus::blocking::MessageIterator) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    enumerate_items(&shared);
    loop {
        process_pending(&shared);
        process_due_timers(&shared);
        if shared.stopping.load(Ordering::Acquire) {
            break;
        }
        match iter.next() {
            Some(Ok(msg)) => handle_message(&shared, msg),
            Some(Err(err)) => {
                fatal(&shared, err);
                break;
            }
            None => {
                fatal(&shared, TrayError::Protocol("signal stream closed".into()));
                break;
            }
        }
    }
    shutdown_cleanup(&shared);
}

pub(crate) fn emit_wake(conn: &Connection) {
    let _ = conn.emit_signal(None::<&'static str>, WAKE_PATH, WAKE_IFACE, WAKE_MEMBER, &());
}

fn dispatch(shared: &Shared, ev: TrayEvent) {
    let (callbacks, subscribers) = {
        let st = shared.state.lock().unwrap();
        (st.callbacks.clone(), st.subscribers.clone())
    };
    for cb in &callbacks {
        cb(&ev);
    }
    let mut dead_ids = Vec::new();
    for (id, sub) in &subscribers {
        if sub.send(ev.clone()).is_err() {
            dead_ids.push(*id);
        }
    }
    if !dead_ids.is_empty() {
        let mut st = shared.state.lock().unwrap();
        st.dropped_events += dead_ids.len() as u64;
        st.subscribers.retain(|(id, _)| !dead_ids.contains(id));
    }
}

fn fatal(shared: &Shared, err: impl std::fmt::Display) {
    shared.stopping.store(true, Ordering::Release);
    shared.timers.stop();
    dispatch(shared, TrayEvent::Fatal(err.to_string()));
}

fn shutdown_cleanup(shared: &Shared) {
    let was_fallback = shared.state.lock().unwrap().watcher == WatcherState::FallbackActive;
    if was_fallback {
        yield_watcher(shared);
    }
    let host = shared.host_id.lock().unwrap().clone();
    if !host.is_empty() {
        let _ = shared.conn.release_name(host);
    }
    shared.stopping.store(true, Ordering::Release);
    shared.timers.stop();
}

// ── watcher / host setup ────────────────────────────────────────────────

fn dbus_proxy(conn: &Connection) -> Result<DBusProxy<'_>, TrayError> {
    DBusProxy::new(conn).map_err(TrayError::from)
}

fn our_unique_name(shared: &Shared) -> String {
    shared
        .conn
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default()
}

/// Probe for the watcher and establish host registration. Emits the initial
/// `WatcherChanged` event.
fn setup_watcher_and_host(shared: &Arc<Shared>) -> Result<(), TrayError> {
    let dbus = dbus_proxy(&shared.conn)?;
    let mode = if dbus.get_name_owner(SNI_WATCHER.try_into().expect("static name")).is_ok() {
        register_host(shared)?;
        WatcherState::ExternalUp
    } else {
        match shared.config.fallback {
            FallbackPolicy::Auto => {
                let became = become_watcher(shared)?;
                if became {
                    register_host(shared)?;
                    WatcherState::FallbackActive
                } else {
                    // Lost the race: someone took the name in between.
                    register_host(shared)?;
                    WatcherState::ExternalUp
                }
            }
            FallbackPolicy::Never => WatcherState::ExternalDown,
        }
    };
    shared.state.lock().unwrap().watcher = mode;
    dispatch(shared, TrayEvent::WatcherChanged(mode));
    Ok(())
}

fn become_watcher(shared: &Arc<Shared>) -> Result<bool, TrayError> {
    let reply = shared.conn.request_name_with_flags(
        SNI_WATCHER,
        RequestNameFlags::DoNotQueue | RequestNameFlags::ReplaceExisting,
    )?;
    let owned = matches!(
        reply,
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner
    );
    if owned {
        shared.conn.object_server().at(
            SNI_WATCHER_PATH,
            WatcherIface {
                shared: Arc::downgrade(shared),
            },
        )?;
    }
    Ok(owned)
}

fn yield_watcher(shared: &Shared) {
    let _ = shared
        .conn
        .object_server()
        .remove::<WatcherIface, _>(SNI_WATCHER_PATH);
    let _ = shared.conn.release_name(SNI_WATCHER);
}

fn register_host(shared: &Shared) -> Result<(), TrayError> {
    let mut host = shared.host_id.lock().unwrap();
    if host.is_empty() {
        let candidate = format!(
            "{}{}-{}",
            SNI_HOST_PREFIX,
            process::id(),
            HOST_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        *host = match shared.conn.request_name_with_flags(
            candidate.as_str(),
            RequestNameFlags::DoNotQueue.into(),
        ) {
            Ok(_) => candidate,
            Err(_) => our_unique_name(shared),
        };
    }
    let host_id = host.clone();
    drop(host);

    watcher_proxy(shared)?.call::<_, _, ()>("RegisterStatusNotifierHost", &(&host_id,))?;
    Ok(())
}

fn watcher_proxy(shared: &Shared) -> Result<zbus::blocking::Proxy<'static>, TrayError> {
    zbus::blocking::Proxy::new_owned(
        shared.conn.clone(),
        SNI_WATCHER,
        SNI_WATCHER_PATH,
        SNI_WATCHER_IFACE,
    )
    .map_err(TrayError::from)
}

// ── item lifecycle ──────────────────────────────────────────────────────

fn enumerate_items(shared: &Shared) {
    let list: Vec<String> = enumerate_via_property(shared)
        .or_else(|_| enumerate_via_method(shared))
        .unwrap_or_default();
    for service in list {
        add_item(shared, service);
    }
}

/// `RegisteredStatusNotifierItems` is a *property* on the watcher interface
/// (the spec-correct access path).
fn enumerate_via_property(shared: &Shared) -> Result<Vec<String>, TrayError> {
    let props = zbus::blocking::fdo::PropertiesProxy::new(
        &shared.conn,
        SNI_WATCHER,
        SNI_WATCHER_PATH,
    )?;
    let value = props.get(
        SNI_WATCHER_IFACE.try_into()?,
        "RegisteredStatusNotifierItems".try_into()?,
    )?;
    Ok(Vec::<String>::try_from(value)?)
}

/// Some watcher implementations expose it as a method instead; try that as
/// a fallback.
fn enumerate_via_method(shared: &Shared) -> Result<Vec<String>, TrayError> {
    watcher_proxy(shared)?
        .call("RegisteredStatusNotifierItems", &())
        .map_err(TrayError::from)
}

fn add_item(shared: &Shared, service: String) {
    let Some((bus, path)) = parse_service(&service) else {
        return;
    };
    let id = TrayItemId {
        bus_name: bus.clone(),
        path: path.clone(),
    };
    {
        let st = shared.state.lock().unwrap();
        if st.items.contains_key(&id) {
            drop(st);
            schedule_refetch_item(shared, id);
            return;
        }
    }
    let unique = resolve_unique(shared, &bus);
    let Ok(props) = get_item_properties(&shared.conn, &bus, &path) else {
        return; // died before we could look at it
    };
    let snapshot = build_item(id.clone(), &props);
    let menu = snapshot
        .menu_path
        .as_ref()
        .and_then(|mp| fetch_menu(&shared.conn, &bus, mp, shared.config.menu_max_depth).ok());
    let mut st = shared.state.lock().unwrap();
    if st.items.contains_key(&id) {
        return; // raced with a signal echo
    }
    st.items.insert(
        id,
        ItemEntry {
            bus,
            unique,
            path,
            menu_path: snapshot.menu_path.clone(),
            snapshot: snapshot.item.clone(),
            menu,
        },
    );
    drop(st);
    dispatch(shared, TrayEvent::Added(snapshot.item));
}

fn resolve_unique(shared: &Shared, bus: &str) -> String {
    dbus_proxy(&shared.conn)
        .and_then(|p| p.get_name_owner(bus.try_into().expect("valid name")).map_err(TrayError::from))
        .map(|n| n.to_string())
        .unwrap_or_else(|_| bus.to_string())
}

fn remove_item(shared: &Shared, id: &TrayItemId) {
    let existed = shared.state.lock().unwrap().items.remove(id).is_some();
    if existed {
        dispatch(shared, TrayEvent::Removed(id.clone()));
    }
}

fn schedule_refetch_item(shared: &Shared, id: TrayItemId) {
    schedule_timer(
        shared,
        Instant::now() + Duration::from_millis(shared.config.debounce_ms),
        TimerKind::RefetchItem(id),
    );
}

fn schedule_refetch_menu(shared: &Shared, id: TrayItemId) {
    schedule_timer(
        shared,
        Instant::now() + Duration::from_millis(shared.config.debounce_ms),
        TimerKind::RefetchMenu(id),
    );
}

fn refetch_item(shared: &Shared, id: TrayItemId) {
    let Some((bus, old_menu_path, old)) = ({
        let st = shared.state.lock().unwrap();
        st.items.get(&id).map(|e| {
            (
                e.bus.clone(),
                e.menu_path.clone(),
                e.snapshot.clone(),
            )
        })
    }) else {
        return;
    };
    let Ok(props) = get_item_properties(&shared.conn, &bus, &id.path) else {
        // Unreachable or broken: drop it. NameOwnerChanged handling covers
        // the normal death paths; this is just belt and braces.
        remove_item(shared, &id);
        return;
    };
    let snapshot = build_item(id.clone(), &props);
    let changed = diff_fields(&old, &snapshot.item);
    let menu_path_changed = old.has_menu != snapshot.item.has_menu
        || old_menu_path != snapshot.menu_path;

    {
        let mut st = shared.state.lock().unwrap();
        if let Some(entry) = st.items.get_mut(&id) {
            entry.snapshot = snapshot.item.clone();
            if menu_path_changed {
                entry.menu_path = snapshot.menu_path.clone();
                entry.menu = None;
            }
        }
    }
    if menu_path_changed {
        if let Some(mp) = &snapshot.menu_path {
            refetch_menu(shared, &id, &bus, mp);
        }
    }
    if !changed.is_empty() {
        dispatch(
            shared,
            TrayEvent::Changed {
                id,
                changed,
                item: snapshot.item,
            },
        );
    }
}

fn refetch_menu_for(shared: &Shared, id: &TrayItemId) {
    let Some((bus, menu_path)) = ({
        let st = shared.state.lock().unwrap();
        st.items
            .get(id)
            .and_then(|e| e.menu_path.as_ref().map(|mp| (e.bus.clone(), mp.clone())))
    }) else {
        return;
    };
    refetch_menu(shared, id, &bus, &menu_path);
}

/// Synchronous menu refresh on the caller's thread (used by
/// `TrayHandle::menu_about_to_show`, where consumers need a fresh snapshot
/// *before* presenting a menu — lazy dbusmenu providers only populate their
/// layout in response to AboutToShow).
pub(crate) fn refresh_menu_sync(shared: &Shared, id: &TrayItemId) -> Option<MenuSnapshot> {
    let location = {
        let st = shared.state.lock().unwrap();
        if shared.stopping.load(Ordering::Acquire) {
            return None;
        }
        st.items
            .get(id)
            .and_then(|e| e.menu_path.as_ref().map(|mp| (e.bus.clone(), mp.clone())))
    };
    let (bus, menu_path) = location?;

    let mut changed = false;
    let mut snapshot = None;
    if let Ok(fresh) = fetch_menu(&shared.conn, &bus, &menu_path, shared.config.menu_max_depth) {
        let mut st = shared.state.lock().unwrap();
        if let Some(entry) = st.items.get_mut(id) {
            if entry.menu.as_ref() != Some(&fresh) {
                entry.menu = Some(fresh);
                changed = true;
            }
            snapshot = entry.menu.clone();
        }
    }
    if changed {
        dispatch(shared, TrayEvent::MenuChanged(id.clone()));
    }
    snapshot
}

fn refetch_menu(shared: &Shared, id: &TrayItemId, bus: &str, menu_path: &str) {
    let Ok(fresh) = fetch_menu(&shared.conn, bus, menu_path, shared.config.menu_max_depth) else {
        return;
    };
    let mut changed = false;
    {
        let mut st = shared.state.lock().unwrap();
        if let Some(entry) = st.items.get_mut(id) {
            if entry.menu.as_ref() != Some(&fresh) {
                entry.menu = Some(fresh);
                changed = true;
            }
        }
    }
    if changed {
        dispatch(shared, TrayEvent::MenuChanged(id.clone()));
    }
}

fn diff_fields(old: &TrayItem, new: &TrayItem) -> Vec<ItemField> {
    use ItemField::*;
    let mut out = Vec::new();
    if old.status != new.status {
        out.push(Status);
    }
    if old.title != new.title {
        out.push(Title);
    }
    if old.description != new.description {
        out.push(Description);
    }
    if old.icon != new.icon {
        out.push(Icon);
    }
    if old.overlay_icon != new.overlay_icon {
        out.push(OverlayIcon);
    }
    if old.attention_icon != new.attention_icon {
        out.push(AttentionIcon);
    }
    if old.tooltip != new.tooltip {
        out.push(Tooltip);
    }
    if old.window_id != new.window_id {
        out.push(WindowId);
    }
    if old.category != new.category {
        out.push(Category);
    }
    if old.has_menu != new.has_menu {
        out.push(HasMenu);
    }
    out
}

// ── interactions ────────────────────────────────────────────────────────

fn do_interact(
    shared: &Shared,
    id: TrayItemId,
    op: InteractionOp,
    x: i32,
    y: i32,
    delta: i32,
    orient: Orientation,
) {
    let location = {
        let st = shared.state.lock().unwrap();
        if shared.stopping.load(Ordering::Acquire) {
            return;
        }
        st.items.get(&id).map(|e| (e.bus.clone(), e.path.clone()))
    };
    let Some((bus, path)) = location else {
        dispatch(
            shared,
            TrayEvent::InteractionResult {
                id,
                op,
                result: Err("item not found".into()),
            },
        );
        return;
    };
    let result: Result<(), TrayError> = (|| {
        let proxy = zbus::blocking::Proxy::new(
            &shared.conn,
            bus,
            ObjectPath::try_from(path.as_str())?,
            SNI_ITEM_IFACE,
        )?;
        match op {
            InteractionOp::Activate => proxy.call::<_, _, ()>("Activate", &(x, y))?,
            InteractionOp::SecondaryActivate => {
                proxy.call::<_, _, ()>("SecondaryActivate", &(x, y))?
            }
            InteractionOp::Scroll => proxy.call::<_, _, ()>("Scroll", &(delta, orient.as_str()))?,
            InteractionOp::MenuActivate | InteractionOp::MenuAboutToShow => {
                return Err(TrayError::Protocol("unsupported op".into()))
            }
        }
        Ok(())
    })();
    dispatch(
        shared,
        TrayEvent::InteractionResult {
            id,
            op,
            result: result.map_err(|e| e.to_string()),
        },
    );
}

fn do_menu_activate(shared: &Shared, id: TrayItemId, menu_item: i32) {
    let location = {
        let st = shared.state.lock().unwrap();
        if shared.stopping.load(Ordering::Acquire) {
            return;
        }
        st.items
            .get(&id)
            .and_then(|e| e.menu_path.as_ref().map(|mp| (e.bus.clone(), mp.clone())))
    };
    let op = InteractionOp::MenuActivate;
    let Some((bus, menu_path)) = location else {
        dispatch(
            shared,
            TrayEvent::InteractionResult {
                id,
                op,
                result: Err("item has no menu".into()),
            },
        );
        return;
    };
    let result: Result<(), TrayError> = (|| {
        let proxy = zbus::blocking::Proxy::new(
            &shared.conn,
            bus,
            ObjectPath::try_from(menu_path.as_str())?,
            MENU_IFACE,
        )?;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        proxy.call::<_, _, ()>("Event", &(menu_item, "clicked", Value::I32(0), ts))?;
        Ok(())
    })();
    dispatch(
        shared,
        TrayEvent::InteractionResult {
            id,
            op,
            result: result.map_err(|e| e.to_string()),
        },
    );
}

// ── command & timer processing ──────────────────────────────────────────

fn process_pending(shared: &Shared) {
    loop {
        let cmd = {
            let mut st = shared.state.lock().unwrap();
            if shared.stopping.load(Ordering::Acquire) {
                return;
            }
            st.pending.pop_front()
        };
        match cmd {
            None => return,
            Some(Cmd::ItemRegistered(service)) => {
                add_item(shared, service.clone());
                let mut st = shared.state.lock().unwrap();
                st.pending_registered.retain(|s| s != &service);
            }
            Some(Cmd::Interact {
                id,
                op,
                x,
                y,
                delta,
                orient,
            }) => do_interact(shared, id, op, x, y, delta, orient),
            Some(Cmd::MenuActivate { id, menu_item }) => do_menu_activate(shared, id, menu_item),
            Some(Cmd::Shutdown) => {
                shared.stopping.store(true, Ordering::Release);
                return;
            }
        }
    }
}

fn same_target(a: &TimerKind, b: &TimerKind) -> bool {
    matches!(
        (a, b),
        (TimerKind::RefetchItem(x), TimerKind::RefetchItem(y)) if x == y
    ) || matches!(
        (a, b),
        (TimerKind::RefetchMenu(x), TimerKind::RefetchMenu(y)) if x == y
    )
}

fn schedule_timer(shared: &Shared, at: Instant, kind: TimerKind) {
    let mut st = shared.state.lock().unwrap();
    st.timers.retain(|(_, k)| !same_target(k, &kind));
    st.timers.push((at, kind));
    drop(st);
    shared.timers.wake_at(at);
}

fn process_due_timers(shared: &Arc<Shared>) {
    let now = Instant::now();
    let due: Vec<TimerKind> = {
        let mut st = shared.state.lock().unwrap();
        let mut due = Vec::new();
        st.timers.retain(|(at, kind)| {
            if *at <= now {
                due.push(kind.clone());
                false
            } else {
                true
            }
        });
        due
    };
    for kind in due {
        match kind {
            TimerKind::RefetchItem(id) => refetch_item(shared, id),
            TimerKind::RefetchMenu(id) => refetch_menu_for(shared, &id),
            TimerKind::WatcherRespawn => watcher_respawn(shared),
        }
    }
}

// ── watcher transitions ─────────────────────────────────────────────────

fn on_watcher_down(shared: &Shared) {
    let was = {
        let mut st = shared.state.lock().unwrap();
        let was = st.watcher;
        if was == WatcherState::ExternalUp {
            st.watcher = WatcherState::ExternalDown;
        }
        was
    };
    if was != WatcherState::ExternalUp {
        return;
    }
    if shared.config.fallback == FallbackPolicy::Auto {
        schedule_timer(
            shared,
            Instant::now() + Duration::from_millis(2 * shared.config.timeout_ms),
            TimerKind::WatcherRespawn,
        );
    } else {
        dispatch(shared, TrayEvent::WatcherChanged(WatcherState::ExternalDown));
    }
}

fn on_watcher_up(shared: &Shared) {
    {
        let st = shared.state.lock().unwrap();
        if st.watcher == WatcherState::FallbackActive {
            drop(st);
            yield_watcher(shared);
        }
    }
    if register_host(shared).is_ok() {
        let mut st = shared.state.lock().unwrap();
        st.watcher = WatcherState::ExternalUp;
        drop(st);
        dispatch(shared, TrayEvent::WatcherChanged(WatcherState::ExternalUp));
    }
}

fn watcher_respawn(shared: &Arc<Shared>) {
    if shared.config.fallback != FallbackPolicy::Auto {
        return;
    }
    let watcher = shared.state.lock().unwrap().watcher;
    if watcher != WatcherState::ExternalDown {
        return;
    }
    let present = dbus_proxy(&shared.conn)
        .and_then(|p| p.get_name_owner(SNI_WATCHER.try_into().expect("static name")).map_err(TrayError::from))
        .is_ok();
    if present {
        return; // racing with on_watcher_up
    }
    if become_watcher(shared).unwrap_or(false) {
        let _ = register_host(shared);
        let mut st = shared.state.lock().unwrap();
        st.watcher = WatcherState::FallbackActive;
        drop(st);
        dispatch(shared, TrayEvent::WatcherChanged(WatcherState::FallbackActive));
    }
}

// ── signal dispatch ─────────────────────────────────────────────────────

fn handle_message(shared: &Shared, msg: Message) {
    let hdr = msg.header();
    let iface = hdr.interface().map(|s| s.as_str()).unwrap_or_default();
    let member = hdr.member().map(|s| s.as_str()).unwrap_or_default();
    let sender = hdr
        .sender()
        .map(|s| s.as_str().to_string())
        .unwrap_or_default();
    let path = hdr.path().map(|s| s.as_str().to_string()).unwrap_or_default();

    match iface {
        "org.freedesktop.DBus" => {
            if member == "NameOwnerChanged" {
                handle_name_owner_changed(shared, &msg);
            }
        }
        SNI_WATCHER_IFACE => match member {
            "StatusNotifierItemRegistered" => {
                if let Ok(service) = msg.body().deserialize::<String>() {
                    add_item(shared, service);
                }
            }
            "StatusNotifierItemUnregistered" => {
                if let Ok(service) = msg.body().deserialize::<String>() {
                    if let Some((bus, path)) = parse_service(&service) {
                        remove_item(
                            shared,
                            &TrayItemId {
                                bus_name: bus,
                                path,
                            },
                        );
                    }
                }
            }
            _ => {}
        },
        "org.freedesktop.DBus.Properties" => {
            if member != "PropertiesChanged" {
                return;
            }
            let Ok((arg_iface, _, _)) = msg.body().deserialize::<(
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            )>() else {
                return;
            };
            match arg_iface.as_str() {
                SNI_ITEM_IFACE => {
                    if let Some(id) = item_by_unique(shared, &sender) {
                        schedule_refetch_item(shared, id);
                    }
                }
                MENU_IFACE => {
                    if let Some(id) = item_by_menu_signal(shared, &sender, &path) {
                        schedule_refetch_menu(shared, id);
                    }
                }
                _ => {}
            }
        }
        SNI_ITEM_IFACE => {
            // Legacy SNI change signals: NewStatus, NewIcon, NewTitle, ...
            if member.starts_with("New") {
                if let Some(id) = item_by_unique(shared, &sender) {
                    schedule_refetch_item(shared, id);
                }
            }
        }
        MENU_IFACE => {
            if matches!(
                member,
                "LayoutUpdated" | "ItemsPropertiesUpdated" | "AboutToShow"
            ) {
                if let Some(id) = item_by_menu_signal(shared, &sender, &path) {
                    schedule_refetch_menu(shared, id);
                }
            }
        }
        WAKE_IFACE => {} // pure wake-up, nothing to do
        _ => {}
    }
}

fn item_by_unique(shared: &Shared, unique: &str) -> Option<TrayItemId> {
    let st = shared.state.lock().unwrap();
    st.items
        .iter()
        .find(|(_, e)| e.unique == unique)
        .map(|(id, _)| id.clone())
}

fn item_by_menu_signal(shared: &Shared, unique: &str, path: &str) -> Option<TrayItemId> {
    let st = shared.state.lock().unwrap();
    st.items
        .iter()
        .find(|(_, e)| e.unique == unique && e.menu_path.as_deref() == Some(path))
        .map(|(id, _)| id.clone())
}

fn handle_name_owner_changed(shared: &Shared, msg: &Message) {
    let Ok((name, old, new)) = msg.body().deserialize::<(String, String, String)>() else {
        return;
    };
    if name == SNI_WATCHER {
        let our = our_unique_name(shared);
        if new == our {
            // Our own acquisition — state already handled by become_watcher().
            return;
        }
        if old == our && new.is_empty() {
            // Our own release during yield.
            return;
        }
        if new.is_empty() {
            on_watcher_down(shared);
        } else {
            on_watcher_up(shared);
        }
        return;
    }

    // Item bus name lifecycle.
    let affected: Vec<TrayItemId> = {
        let st = shared.state.lock().unwrap();
        st.items
            .iter()
            .filter(|(_, e)| e.bus == name || e.unique == name)
            .map(|(id, _)| id.clone())
            .collect()
    };
    if new.is_empty() {
        for id in affected {
            remove_item(shared, &id);
        }
    } else if !affected.is_empty() {
        {
            let mut st = shared.state.lock().unwrap();
            for id in &affected {
                if let Some(entry) = st.items.get_mut(id) {
                    entry.unique = new.clone();
                }
            }
        }
        for id in affected {
            schedule_refetch_item(shared, id);
        }
    }
}
