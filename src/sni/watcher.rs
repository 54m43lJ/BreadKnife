use std::sync::{mpsc, Arc, atomic::AtomicBool};

use zbus::connection;
use zbus::interface;
use zbus::zvariant;

use crate::sni::icon::IconPixmap;
use crate::sni::item::{SniEvent, SniItemState};
use crate::sni::menu::{MenuResult, SniRequest};

// ── watcher D-Bus interface ──────────────────────────────────────

struct Watcher {
    tx: mpsc::Sender<SniEvent>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        service: String,
    ) {
        let (bus_name, object_path) = parse_service(&service);

        // Ayatana compat: if bus_name is empty (service was a path like /org/ayatana/...),
        // use the sender's unique bus name from the D-Bus message header.
        let bus_name = if bus_name.is_empty() {
            hdr.sender()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default()
        } else {
            bus_name
        };

        let id = format!("{bus_name}{object_path}");

        eprintln!("[sni::bg] RegisterStatusNotifierItem service={service} bus={bus_name} path={object_path}");

        match fetch_item_state(conn, &bus_name, &object_path).await {
            Ok(state) => {
                eprintln!("[sni::bg] item state fetched ok — icon={:?}", state.icon_name);
                let _ = self.tx.send(SniEvent::ItemAdded { id, state });
            }
            Err(e) => {
                eprintln!("[sni::bg] FAILED fetch state {bus_name}{object_path}: {e}");
            }
        }
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }
}

// ── background thread ────────────────────────────────────────────

pub fn run_watcher(
    tx: mpsc::Sender<SniEvent>,
    request_rx: mpsc::Receiver<SniRequest>,
    _shutdown: Arc<AtomicBool>,
) {
    eprintln!("[sni::bg] run_watcher entry");
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => {
            eprintln!("[sni::bg] tokio runtime created");
            rt
        }
        Err(e) => {
            eprintln!("[sni::bg] tokio runtime FAILED: {e}");
            return;
        }
    };

    eprintln!("[sni::bg] entering block_on");
    rt.block_on(async {
        match watcher_main(tx, request_rx).await {
            Ok(()) => eprintln!("[sni::bg] watcher_main returned Ok"),
            Err(e) => eprintln!("[sni::bg] watcher_main ERROR: {e}"),
        }
    });
}

async fn watcher_main(
    tx: mpsc::Sender<SniEvent>,
    request_rx: mpsc::Receiver<SniRequest>,
) -> zbus::Result<()> {
    eprintln!("[sni::bg] watcher_main — connecting to session bus...");
    let conn = connection::Builder::session()?
        .name("org.kde.StatusNotifierWatcher")?
        .serve_at("/StatusNotifierWatcher", Watcher { tx: tx.clone() })?
        .build()
        .await?;

    eprintln!("[sni::bg] watcher registered on session bus, name acquired");

    // Scan for already-running SNI items and register them
    discover_existing_items(&conn, &tx).await;

    process_requests(&conn, request_rx).await;

    eprintln!("[sni::bg] process_requests exited");
    Ok(())
}

async fn process_requests(conn: &zbus::Connection, rx: mpsc::Receiver<SniRequest>) {
    eprintln!("[sni::bg] process_requests loop started");
    loop {
        match rx.recv() {
            Ok(SniRequest::Activate {
                bus_name,
                object_path,
                x,
                y,
            }) => {
                eprintln!("[sni::bg] Activate {bus_name}{object_path} ({x},{y})");
                if let Ok(proxy) = build_item_proxy(conn, &bus_name, &object_path).await {
                    let _ = proxy.activate(x, y).await;
                }
            }
            Ok(SniRequest::SecondaryActivate {
                bus_name,
                object_path,
                x,
                y,
            }) => {
                eprintln!("[sni::bg] SecondaryActivate {bus_name}{object_path}");
                if let Ok(proxy) = build_item_proxy(conn, &bus_name, &object_path).await {
                    let _ = proxy.secondary_activate(x, y).await;
                }
            }
            Ok(SniRequest::Menu(req)) => {
                eprintln!("[sni::bg] Menu request {}/{}", req.bus_name, req.menu_path);
                let result = fetch_menu_impl(conn, &req.bus_name, &req.menu_path).await;
                let _ = req.reply.send(result);
            }
            Err(mpsc::RecvError) => {
                eprintln!("[sni::bg] process_requests channel closed");
                break;
            }
        }
    }
}

async fn build_item_proxy<'a>(
    conn: &'a zbus::Connection,
    bus_name: &'a str,
    object_path: &'a str,
) -> zbus::Result<StatusNotifierItemProxy<'a>> {
    StatusNotifierItemProxy::builder(conn)
        .destination(bus_name)?
        .path(object_path)?
        .build()
        .await
}

/// After acquiring the watcher name, scan for already-running SNI items
/// that registered with the previous watcher instance.
async fn discover_existing_items(conn: &zbus::Connection, tx: &mpsc::Sender<SniEvent>) {
    eprintln!("[sni::bg] discover_existing_items start");
    let dbus = match zbus::fdo::DBusProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[sni::bg] DBusProxy failed: {e}");
            return;
        }
    };
    let names = match dbus.list_names().await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("[sni::bg] ListNames failed: {e}");
            return;
        }
    };

    for name in &names {
        let s = name.as_str();
        if s.starts_with(':')
            || s == "org.kde.StatusNotifierWatcher"
            || s == "org.freedesktop.DBus"
        {
            continue;
        }
        // skip system bus names that never have SNI items
        if s.starts_with("org.freedesktop")
            || s.starts_with("org.gtk")
            || s.starts_with("org.gnome")
            || s.starts_with("org.pulse")
            || s.starts_with("org.pipe")
            || s.starts_with("org.mpris")
            || s.starts_with("ca.desrt")
            || s.starts_with("org.a11y")
        {
            continue;
        }

        let (bus, path) = (s, "/StatusNotifierItem");
        match fetch_item_state(conn, bus, path).await {
            Ok(state) => {
                // only add if it has meaningful properties (at minimum an icon or title)
                if state.icon_name.is_empty() && state.title.is_empty()
                    && state.icon_pixmaps.is_empty()
                {
                    eprintln!("[sni::bg] discover {bus}{path}: empty state, skipping");
                    continue;
                }
                let id = format!("{bus}{path}");
                eprintln!("[sni::bg] discovered existing item: {id} icon={}", state.icon_name);
                let _ = tx.send(SniEvent::ItemAdded { id, state });
            }
            Err(e) => {
                eprintln!("[sni::bg] discover {bus}{path}: {e}");
            }
        }
    }
    eprintln!("[sni::bg] discover_existing_items done");
}

async fn fetch_menu_impl(
    _conn: &zbus::Connection,
    _bus_name: &str,
    _menu_path: &str,
) -> MenuResult {
    // TODO: dbusmenu GetLayout (phase 5)
    MenuResult { items: Vec::new() }
}

// ── service parsing ──────────────────────────────────────────────

fn parse_service(service: &str) -> (String, String) {
    if let Some(pos) = service.find('/') {
        (service[..pos].to_string(), service[pos..].to_string())
    } else {
        (service.to_string(), "/StatusNotifierItem".to_string())
    }
}

// ── item state fetching ──────────────────────────────────────────

async fn fetch_item_state(
    conn: &zbus::Connection,
    bus_name: &str,
    object_path: &str,
) -> zbus::Result<SniItemState> {
    let id = format!("{}{}", bus_name, object_path);
    eprintln!("[sni::bg] fetch_item_state {bus_name}{object_path}");
    let mut state = SniItemState::empty(&id, bus_name, object_path);

    let proxy = match build_item_proxy(conn, bus_name, object_path).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[sni::bg] fetch_item_state proxy build failed {bus_name}{object_path}: {e}");
            return Err(e);
        }
    };

    macro_rules! prop {
        ($field:ident, $method:ident, $default:expr) => {
            match proxy.$method().await {
                Ok(v) => state.$field = v,
                Err(e) => {
                    eprintln!("[sni::bg] fetch {bus_name}{object_path}.{}: {e}", stringify!($method));
                    state.$field = $default;
                }
            }
        };
        ($field:ident, $method:ident, map $default:expr) => {
            match proxy.$method().await {
                Ok(v) => state.$field = v,
                Err(e) => {
                    eprintln!("[sni::bg] fetch {bus_name}{object_path}.{}: {e}", stringify!($method));
                    state.$field = $default;
                }
            }
        };
    }

    prop!(category, category, String::new());
    prop!(title, title, String::new());
    prop!(status, status, String::new());
    prop!(icon_name, icon_name, String::new());
    prop!(icon_theme_path, icon_theme_path, String::new());
    prop!(attention_icon_name, attention_icon_name, String::new());
    prop!(overlay_icon_name, overlay_icon_name, String::new());
    state.item_is_menu = proxy.item_is_menu().await.unwrap_or(false);
    state.menu_path = proxy
        .menu()
        .await
        .map(|p| {
            let s = p.to_string();
            if !s.is_empty() && s != "/" {
                eprintln!("[sni::bg] fetch {bus_name}{object_path}.menu = {s}");
            }
            s
        })
        .unwrap_or_default();
    prop!(window_id, window_id, 0);

    if let Ok(pixmaps) = proxy.icon_pixmap().await {
        state.icon_pixmaps = convert_pixmaps(&pixmaps);
    }
    if let Ok(pixmaps) = proxy.attention_icon_pixmap().await {
        state.attention_icon_pixmaps = convert_pixmaps(&pixmaps);
    }
    if let Ok(pixmaps) = proxy.overlay_icon_pixmap().await {
        state.overlay_icon_pixmaps = convert_pixmaps(&pixmaps);
    }

    Ok(state)
}

fn convert_pixmaps(raw: &[zvariant::OwnedValue]) -> Vec<IconPixmap> {
    raw.iter()
        .filter_map(|v| {
            let s: &zvariant::Structure = v.downcast_ref().ok()?;
            let fields = s.fields();
            let w: i32 = i32::try_from(fields.get(0)?).ok()?;
            let h: i32 = i32::try_from(fields.get(1)?).ok()?;
            // byte array (ay) → zvariant::Array → Vec<u8>
            let array: &zvariant::Array = fields.get(2)?.downcast_ref().ok()?;
            let bytes: Vec<u8> = array.iter().filter_map(|v| u8::try_from(v).ok()).collect();
            Some(IconPixmap {
                width: w,
                height: h,
                bytes,
            })
        })
        .collect()
}

// ── SNI proxy ────────────────────────────────────────────────────

#[zbus::proxy(
    interface = "org.kde.StatusNotifierItem",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierItem"
)]
trait StatusNotifierItem {
    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<zvariant::OwnedValue>>;

    #[zbus(property)]
    fn attention_icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> zbus::Result<Vec<zvariant::OwnedValue>>;

    #[zbus(property)]
    fn overlay_icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> zbus::Result<Vec<zvariant::OwnedValue>>;

    #[zbus(property)]
    fn item_is_menu(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn menu(&self) -> zbus::Result<zbus::zvariant::ObjectPath<'_>>;

    #[zbus(property)]
    fn window_id(&self) -> zbus::Result<u32>;

    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;
}
