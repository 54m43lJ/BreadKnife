//! Programmable fake SNI item ("demo item") for development and CI.
//!
//! Feature-gated behind `testing`. It registers a fully protocol-compliant
//! `org.kde.StatusNotifierItem` (plus a `com.canonical.dbusmenu` object) on
//! its own bus connection, so the tray library can be exercised without any
//! real tray application present (SPEC §10.4).

use std::sync::{mpsc, Arc, Mutex};

use zbus::{
    blocking::Connection,
    interface,
    object_server::{ObjectServer, SignalEmitter},
    zvariant::{ObjectPath, OwnedValue, Structure, Value},
};

use crate::{error::TrayError, model::TrayItemId};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";

/// Configuration for [`spawn_demo_item`].
#[derive(Debug, Clone)]
pub struct DemoItemConfig {
    /// Object path of the demo item. Default: `/org/tray/demo/Item`.
    pub item_path: String,
    /// Whether the item exposes a menu. Default: `true`.
    pub menu: bool,
}

impl Default for DemoItemConfig {
    fn default() -> Self {
        Self {
            item_path: "/org/tray/demo/Item".to_string(),
            menu: true,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct DemoMenuEntry {
    id: i32,
    label: String,
    toggleable: bool,
    toggled: bool,
}

#[derive(Debug)]
struct DemoState {
    status: String,
    icon_name: String,
    use_pixmap: bool,
    tooltip: Option<(String, String)>,
    menu_enabled: bool,
    menu_items: Vec<DemoMenuEntry>,
    menu_revision: u32,
    next_menu_id: i32,
    last_click: Option<i32>,
    activations: u32,
}

impl Default for DemoState {
    fn default() -> Self {
        Self {
            status: "Active".to_string(),
            icon_name: "demo".to_string(),
            use_pixmap: false,
            tooltip: Some(("Demo tooltip".to_string(), "From demo-item".to_string())),
            menu_enabled: true,
            menu_items: vec![DemoMenuEntry {
                id: 1,
                label: "About demo".to_string(),
                ..Default::default()
            }],
            menu_revision: 1,
            next_menu_id: 2,
            last_click: None,
            activations: 0,
        }
    }
}

type Shared = Arc<Mutex<DemoState>>;

/// A running demo item. Call [`DemoItem::remove`] to unregister; dropping
/// also unregisters as a best effort.
pub struct DemoItem {
    conn: Connection,
    control: zbus::blocking::Proxy<'static>,
    item_path: String,
    state: Shared,
}

impl DemoItem {
    /// Bus id under which this item is registered.
    pub fn id(&self) -> TrayItemId {
        TrayItemId {
            bus_name: self
                .conn
                .unique_name()
                .map(|n| n.to_string())
                .unwrap_or_default(),
            path: self.item_path.clone(),
        }
    }

    /// Service string (bus name + item path) as watchers see it.
    pub fn service(&self) -> String {
        format!("{}{}", self.id().bus_name, self.item_path)
    }

    fn apply(&self, msg: String) -> Result<i32, TrayError> {
        Ok(self.control.call::<_, _, i32>("Apply", &(&msg,))?)
    }

    /// Switch item status: `passive` | `active` | `needs-attention`.
    pub fn set_status(&self, status: &str) -> Result<(), TrayError> {
        let status = match status.to_ascii_lowercase().as_str() {
            "passive" => "Passive",
            "needs-attention" => "NeedsAttention",
            _ => "Active",
        };
        self.apply(format!("status {status}")).map(|_| ())
    }

    /// Switch to a theme icon name.
    pub fn set_icon_name(&self, name: &str) -> Result<(), TrayError> {
        self.apply(format!("icon {name}")).map(|_| ())
    }

    /// Toggle between theme icon and an embedded 4x2 bitmap.
    pub fn use_pixmap(&self, on: bool) -> Result<(), TrayError> {
        self.apply(format!("pixmap {}", if on { "on" } else { "off" }))
            .map(|_| ())
    }

    /// Set tooltip (both empty -> clear).
    pub fn set_tooltip(&self, title: &str, description: &str) -> Result<(), TrayError> {
        self.apply(format!("tooltip {title}|{description}"))
            .map(|_| ())
    }

    /// Append a menu entry; returns its dbusmenu id.
    pub fn menu_add(&self, label: &str) -> Result<i32, TrayError> {
        self.apply(format!("menu_add {label}"))
    }

    /// Make an entry toggleable and flip its state.
    pub fn menu_toggle(&self, id: i32) -> Result<(), TrayError> {
        self.apply(format!("menu_toggle {id}")).map(|_| ())
    }

    /// Remove a menu entry.
    pub fn menu_remove(&self, id: i32) -> Result<(), TrayError> {
        self.apply(format!("menu_remove {id}")).map(|_| ())
    }

    /// Mount/unmount the Menu object from the item.
    pub fn set_menu_enabled(&self, on: bool) -> Result<(), TrayError> {
        self.apply(format!("menu {}", if on { "on" } else { "off" }))
            .map(|_| ())
    }

    /// Last menu entry clicked via `menu_activate`, if any.
    pub fn last_click(&self) -> Option<i32> {
        self.state.lock().unwrap().last_click
    }

    /// Number of Activate/SecondaryActivate calls received.
    pub fn activations(&self) -> u32 {
        self.state.lock().unwrap().activations
    }

    /// Unregister the item and close its bus connection.
    pub fn remove(self) -> Result<(), TrayError> {
        let result = self.control.call_noreply("Exit", &());
        std::mem::forget(self); // Drop would exit again
        result.map_err(TrayError::from)
    }
}

impl Drop for DemoItem {
    fn drop(&mut self) {
        let _ = self.control.call_noreply("Exit", &());
    }
}

/// Spawn a demo item on its own bus connection and register it with the
/// watcher (retrying briefly while the watcher shows up).
pub fn spawn_demo_item(config: DemoItemConfig) -> Result<DemoItem, TrayError> {
    let conn = Connection::session().map_err(|e| TrayError::Bus(e.to_string()))?;
    let menu_path = format!("{}/menu", config.item_path.trim_end_matches('/'));
    let state: Shared = Arc::new(Mutex::new(DemoState {
        menu_enabled: config.menu,
        ..Default::default()
    }));

    conn.object_server().at(
        config.item_path.as_str(),
        DemoItemIface {
            state: Arc::clone(&state),
            menu_path: menu_path.clone(),
        },
    )?;
    conn.object_server()
        .at(menu_path.as_str(), DemoMenuIface { state: Arc::clone(&state) })?;
    conn.object_server().at(
        config.item_path.as_str(),
        DemoControlIface {
            state: Arc::clone(&state),
            item_path: config.item_path.clone(),
            menu_path: menu_path.clone(),
        },
    )?;

    let unique = conn
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let service = format!("{}{}", unique, config.item_path);

    let control: zbus::blocking::Proxy<'static> = zbus::blocking::proxy::Builder::new(&conn)
        .destination(unique.clone())?
        .path(config.item_path.clone())?
        .interface("org.tray.demo.Control")?
        .build()?;
    let _ = control.call_noreply("Spawned", &());

    // Register with the watcher; retry while it shows up.
    let watcher = zbus::blocking::Proxy::new(&conn, WATCHER, WATCHER_PATH, WATCHER_IFACE)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(&service,)) {
            Ok(()) => break,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(TrayError::from(e)),
        }
    }

    Ok(DemoItem {
        conn,
        control,
        item_path: config.item_path,
        state,
    })
}

// ── exported interfaces ─────────────────────────────────────────────────

/// The SNI item object.
struct DemoItemIface {
    state: Shared,
    menu_path: String,
}

/// Private control channel used by [`DemoItem`] methods to mutate state on
/// the interface's executor thread (where change notifications must be
/// emitted).
struct DemoControlIface {
    state: Shared,
    item_path: String,
    menu_path: String,
}

/// The dbusmenu object.
struct DemoMenuIface {
    state: Shared,
}

#[interface(name = "org.tray.demo.Control")]
impl DemoControlIface {
    async fn spawned(&self) {}

    /// Apply a state mutation and emit the matching change notifications.
    async fn apply(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        msg: String,
    ) -> i32 {
        let (cmd, arg) = msg.split_once(' ').unwrap_or((msg.as_str(), ""));
        let menu_changed = matches!(cmd, "menu_add" | "menu_toggle" | "menu_remove" | "menu");
        let mut menu_add_id = -1;
        {
            let mut st = self.state.lock().unwrap();
            match cmd {
                "status" => st.status = arg.to_string(),
                "icon" => {
                    st.icon_name = arg.to_string();
                    st.use_pixmap = false;
                }
                "pixmap" => st.use_pixmap = arg == "on",
                "tooltip" => {
                    let (t, d) = arg.split_once('|').unwrap_or(("", ""));
                    st.tooltip = if t.is_empty() && d.is_empty() {
                        None
                    } else {
                        Some((t.to_string(), d.to_string()))
                    };
                }
                "menu_add" => {
                    let id = st.next_menu_id;
                    st.next_menu_id += 1;
                    st.menu_items.push(DemoMenuEntry {
                        id,
                        label: arg.to_string(),
                        ..Default::default()
                    });
                    menu_add_id = id;
                }
                "menu_toggle" => {
                    if let Ok(id) = arg.parse::<i32>() {
                        if let Some(e) = st.menu_items.iter_mut().find(|e| e.id == id) {
                            e.toggleable = true;
                            e.toggled = !e.toggled;
                        }
                    }
                }
                "menu_remove" => {
                    if let Ok(id) = arg.parse::<i32>() {
                        st.menu_items.retain(|e| e.id != id);
                    }
                }
                "menu" => st.menu_enabled = arg == "on",
                _ => {}
            }
        }

        // Emit SNI property change notifications.
        let item_ref = server
            .interface::<_, DemoItemIface>(self.item_path.as_str())
            .await
            .ok();
        if let Some(item_ref) = item_ref {
            let item = item_ref.get().await;
            let emitter = item_ref.signal_emitter();
            let _ = item.status_changed(emitter).await;
            let _ = item.icon_name_changed(emitter).await;
            let _ = item.icon_pixmap_changed(emitter).await;
            let _ = item.tool_tip_changed(emitter).await;
            let _ = item.menu_changed(emitter).await;
        }

        if menu_changed {
            let revision = self.state.lock().unwrap().menu_revision;
            if let Ok(menu_ref) = server
                .interface::<_, DemoMenuIface>(self.menu_path.as_str())
                .await
            {
                let _ =
                    DemoMenuIface::layout_updated(menu_ref.signal_emitter(), revision, 0).await;
            }
        }
        menu_add_id
    }

    async fn exit(&self, #[zbus(signal_context)] ctxt: SignalEmitter<'_>) {
        let _ = ctxt.connection().clone().close().await;
    }
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl DemoItemIface {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "demo"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Demo Item"
    }

    #[zbus(property)]
    fn status(&self) -> String {
        self.state.lock().unwrap().status.clone()
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        let st = self.state.lock().unwrap();
        if st.use_pixmap {
            String::new()
        } else {
            st.icon_name.clone()
        }
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        let st = self.state.lock().unwrap();
        if st.use_pixmap {
            // 4x2 checkerboard, ARGB32 network byte order.
            let mut argb = Vec::with_capacity(32);
            for y in 0..2u32 {
                for x in 0..4u32 {
                    let on = (x + y) % 2 == 0;
                    argb.extend_from_slice(&[
                        255,
                        if on { 220 } else { 30 },
                        30,
                        if on { 30 } else { 220 },
                    ]);
                }
            }
            vec![(4, 2, argb)]
        } else {
            Vec::new()
        }
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<(i32, i32, Vec<u8>)>, String, String) {
        match &self.state.lock().unwrap().tooltip {
            Some((t, d)) => (String::new(), Vec::new(), t.clone(), d.clone()),
            None => (String::new(), Vec::new(), String::new(), String::new()),
        }
    }

    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'static> {
        let enabled = self.state.lock().unwrap().menu_enabled;
        ObjectPath::try_from(if enabled {
            self.menu_path.as_str()
        } else {
            "/"
        })
        .unwrap()
        .to_owned()
    }

    fn activate(&self, _x: i32, _y: i32) {
        self.state.lock().unwrap().activations += 1;
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {
        self.state.lock().unwrap().activations += 1;
    }

    fn scroll(&self, _delta: i32, _orientation: String) {}
}

#[interface(name = "com.canonical.dbusmenu")]
impl DemoMenuIface {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> (u32, OwnedValue) {
        let st = self.state.lock().unwrap();
        let children: Vec<Value> = st
            .menu_items
            .iter()
            .map(|e| {
                let mut props = std::collections::HashMap::new();
                props.insert("label".to_string(), Value::from(e.label.as_str()));
                props.insert("enabled".to_string(), Value::from(true));
                props.insert("visible".to_string(), Value::from(true));
                if e.toggleable {
                    props.insert("toggle-type".to_string(), Value::from("checkmark"));
                    props.insert(
                        "toggle-state".to_string(),
                        Value::from(if e.toggled { 1i32 } else { 0i32 }),
                    );
                }
                Value::Value(Box::new(Value::Structure(Structure::from((
                    e.id,
                    props,
                    Vec::<Value>::new(),
                )))))
            })
            .collect();
        let layout = Value::Structure(Structure::from((
            0i32,
            std::collections::HashMap::<String, Value>::new(),
            children,
        )));
        (st.menu_revision, layout.try_to_owned().unwrap())
    }

    fn get_group_properties(
        &self,
        _ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, std::collections::HashMap<String, OwnedValue>)> {
        Vec::new()
    }

    fn get_property(&self, _id: i32, _name: String) -> OwnedValue {
        OwnedValue::from(0u32)
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id == "clicked" {
            self.state.lock().unwrap().last_click = Some(id);
        }
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    #[zbus(signal)]
    pub async fn layout_updated(
        ctxt: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// Wait (bounded) on an event receiver until a predicate matches, draining
/// everything else.
pub fn wait_for<T>(
    rx: &mpsc::Receiver<crate::TrayEvent>,
    timeout: std::time::Duration,
    mut pred: impl FnMut(&crate::TrayEvent) -> Option<T>,
) -> Option<T> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match rx.recv_timeout(remaining) {
            Ok(ev) => {
                if let Some(v) = pred(&ev) {
                    return Some(v);
                }
            }
            Err(_) => return None,
        }
    }
}
