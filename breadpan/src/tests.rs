//! 单元测试（纯函数）+ 生命周期测试（需 DBus session bus，进程内 mock Item）。
//!
//! 生命周期测试串行执行（Watcher 总线名全总线唯一）；
//! 总线不可用或已有他人 Watcher 时跳过（`dbus-run-session cargo test` 可全量运行）。

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

use crate::dbus::menu::{assemble, menu_item_from_props, parse_mnemonic, NodeRaw};
use crate::dbus::split_service_arg;
use crate::events::{InteractionOp, TrayEvent};
use crate::props::{icon_source, tray_item_from_props};
use crate::tracker::ItemEntry;
use crate::{
    IconSource, InteractionOutcome, MenuItemType, ToggleState, ToggleType, Tray, TrayConfig,
    TrayError, TrayItem, TrayStatus,
};

const MOCK_ID: &str = "org.kde.StatusNotifierItem-99999-1";
const MOCK_ITEM_PATH: &str = "/StatusNotifierItem";
const MOCK_MENU_PATH: &str = "/org/mock/Menu";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";

// ── 单元测试 ──

#[test]
fn config_defaults() {
    let c = TrayConfig::default();
    assert_eq!(c.timeout_ms, 2000);
    assert_eq!(c.debounce_ms, 50);
}

#[test]
fn split_service_arg_variants() {
    let (n, p) = split_service_arg("org.kde.StatusNotifierItem-1-2");
    assert_eq!(n, "org.kde.StatusNotifierItem-1-2");
    assert_eq!(p, "/StatusNotifierItem");

    let (n, p) = split_service_arg("org.foo/Custom/Path");
    assert_eq!(n, "org.foo");
    assert_eq!(p, "/Custom/Path");

    let (n, p) = split_service_arg(":1.42");
    assert_eq!(n, ":1.42");
    assert_eq!(p, "/StatusNotifierItem");
}

#[test]
fn mnemonic_parsing() {
    // "__" → "_"；剩余 "_" 为访问键标记，不显示
    assert_eq!(parse_mnemonic("E_xit"), "Exit");
    assert_eq!(parse_mnemonic("__File"), "_File");
    assert_eq!(parse_mnemonic("A__B_C"), "A_BC");
    assert_eq!(parse_mnemonic("Plain"), "Plain");
    assert_eq!(parse_mnemonic("Trailing_"), "Trailing");
    assert_eq!(parse_mnemonic(""), "");
    assert_eq!(parse_mnemonic("___"), "_");
}

#[test]
fn menu_assembly_defaults_and_mapping() {
    let sv = |s: &str| OwnedValue::try_from(Value::from(s)).unwrap();

    let mut m1 = HashMap::new();
    m1.insert("label".to_string(), sv("E_xit"));
    let mut m2 = HashMap::new();
    m2.insert("label".to_string(), sv("__File"));
    m2.insert(
        "enabled".to_string(),
        OwnedValue::try_from(Value::from(false)).unwrap(),
    );
    let mut m3 = HashMap::new();
    m3.insert("label".to_string(), sv("Sub"));
    m3.insert("children-display".to_string(), sv("submenu"));
    m3.insert("toggle-type".to_string(), sv("radio"));
    m3.insert(
        "toggle-state".to_string(),
        OwnedValue::try_from(Value::from(1i32)).unwrap(),
    );
    let mut m4 = HashMap::new();
    m4.insert("type".to_string(), sv("separator"));
    m4.insert(
        "toggle-state".to_string(),
        OwnedValue::try_from(Value::from(-1i32)).unwrap(),
    );

    let children: Vec<NodeRaw> = vec![
        (1, m1.clone(), vec![]),
        (2, m2.clone(), vec![]),
        (3, m3.clone(), vec![]),
        (4, m4.clone(), vec![]),
    ];
    let snap = assemble(0, 7, &children, &[]);
    assert_eq!(snap.parent_id, 0);
    assert_eq!(snap.revision, 7);
    assert_eq!(snap.items.len(), 4);

    let e1 = &snap.items[0];
    assert_eq!(
        (e1.id, e1.label.as_str(), e1.raw_label.as_str()),
        (1, "Exit", "E_xit")
    );
    assert!(e1.enabled && e1.visible && !e1.children_display);
    assert_eq!(e1.item_type, MenuItemType::Standard);
    assert_eq!(e1.toggle_type, None);
    assert_eq!(e1.toggle_state, None);

    let e2 = &snap.items[1];
    assert_eq!(e2.label, "_File");
    assert!(!e2.enabled);

    let e3 = &snap.items[2];
    assert!(e3.children_display);
    assert_eq!(e3.toggle_type, Some(ToggleType::Radio));
    assert_eq!(e3.toggle_state, Some(ToggleState::On));

    let e4 = &snap.items[3];
    assert_eq!(e4.item_type, MenuItemType::Separator);
    assert_eq!(e4.toggle_state, Some(ToggleState::Indeterminate));

    // 单条目组装 + 组属性覆盖
    let mut group_props = HashMap::new();
    group_props.insert(
        "visible".to_string(),
        OwnedValue::try_from(Value::from(false)).unwrap(),
    );
    let snap2 = assemble(3, 9, &[(31, m1, vec![])], &[(31, group_props)]);
    assert_eq!(snap2.parent_id, 3);
    assert!(!snap2.items[0].visible);
    let _ = menu_item_from_props(31, &HashMap::new()); // 全缺省不 panic
}

#[test]
fn icon_source_selection() {
    let big = vec![
        (8, 8, vec![0u8; 8 * 8 * 4]),
        (32, 32, vec![0u8; 32 * 32 * 4]),
    ];
    let empty: Vec<(i32, i32, Vec<u8>)> = vec![];

    // Name 非空 → Name
    assert_eq!(
        icon_source(Some("edit-copy"), Some(&big)),
        IconSource::Name("edit-copy".into())
    );
    // Name 空且有位图 → 最大位图
    match icon_source(None, Some(&big)) {
        IconSource::Pixmap(p) => assert_eq!((p.width, p.height), (32, 32)),
        other => panic!("expected pixmap, got {other:?}"),
    }
    // 双空 → 空位图
    match icon_source(None, Some(&empty)) {
        IconSource::Pixmap(p) => assert_eq!((p.width, p.height, p.argb.len()), (0, 0, 0)),
        other => panic!("expected empty pixmap, got {other:?}"),
    }
}

#[test]
fn tray_item_from_props_mapping() {
    let entry = |service: &str, alive: bool| ItemEntry {
        service: service.to_string(),
        unique: ":1.1".to_string(),
        path: "/StatusNotifierItem".to_string(),
        alive,
        menu: None,
        menu_only: false,
    };
    let sv = |s: &str| OwnedValue::try_from(Value::from(s)).unwrap();

    let mut props = HashMap::new();
    props.insert("Category".to_string(), sv("SystemServices"));
    props.insert("Status".to_string(), sv("NeedsAttention"));
    props.insert("Id".to_string(), sv("mock"));
    props.insert("Title".to_string(), sv("Mock"));
    props.insert(
        "ItemIsMenu".to_string(),
        OwnedValue::try_from(Value::from(true)).unwrap(),
    );
    props.insert(
        "WindowId".to_string(),
        OwnedValue::try_from(Value::from(7u32)).unwrap(),
    );
    props.insert("IconName".to_string(), sv("edit-copy"));
    props.insert(
        "Menu".to_string(),
        OwnedValue::try_from(Value::new(ObjectPath::from_static_str_unchecked(
            "/org/mock/Menu",
        )))
        .unwrap(),
    );
    let item: TrayItem = tray_item_from_props(&entry("svc", true), &props);
    assert_eq!(item.id, "svc");
    assert_eq!(item.status, TrayStatus::NeedsAttention);
    assert!(item.is_menu_only);
    assert_eq!(item.window_id, 7);
    assert_eq!(item.icon, IconSource::Name("edit-copy".into()));
    assert_eq!(item.menu.as_deref(), Some("/org/mock/Menu"));
    assert!(item.alive);

    // 未知枚举值回退缺省；WindowId 以 I32 送达也可解析；标题空串 → None
    let mut props2 = HashMap::new();
    props2.insert("Status".to_string(), sv("Weird"));
    props2.insert(
        "WindowId".to_string(),
        OwnedValue::try_from(Value::from(9i32)).unwrap(),
    );
    props2.insert("Title".to_string(), sv(""));
    let item2 = tray_item_from_props(&entry("svc", false), &props2);
    assert_eq!(item2.status, TrayStatus::Passive);
    assert_eq!(item2.window_id, 9);
    assert_eq!(item2.title, None);
    assert!(!item2.alive);
}

// ── mock Item（进程内、同一 session bus）──

#[derive(Default)]
struct MockCalls(Mutex<Vec<String>>);

impl MockCalls {
    fn record(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }

    fn contains(&self, s: &str) -> bool {
        self.0.lock().unwrap().iter().any(|c| c == s)
    }
}

struct MockItem {
    calls: Arc<MockCalls>,
    title: Arc<Mutex<String>>,
    menu_only: Arc<Mutex<bool>>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl MockItem {
    #[zbus(property)]
    fn category(&self) -> String {
        "ApplicationStatus".into()
    }
    #[zbus(property)]
    fn id(&self) -> String {
        "mock".into()
    }
    #[zbus(property)]
    fn title(&self) -> String {
        self.title.lock().unwrap().clone()
    }
    #[zbus(property)]
    fn status(&self) -> String {
        "Active".into()
    }
    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        *self.menu_only.lock().unwrap()
    }
    #[zbus(property)]
    fn icon_name(&self) -> String {
        "edit-copy".into()
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        ObjectPath::from_static_str_unchecked(MOCK_MENU_PATH).into()
    }

    async fn activate(&self, x: i32, y: i32) {
        self.calls.record(format!("activate {x} {y}"));
    }
    async fn secondary_activate(&self, x: i32, y: i32) {
        self.calls.record(format!("secondary {x} {y}"));
    }
    async fn context_menu(&self, x: i32, y: i32) {
        self.calls.record(format!("context {x} {y}"));
    }
    async fn scroll(&self, delta: i32, orientation: &str) {
        self.calls.record(format!("scroll {delta} {orientation}"));
    }

    #[zbus(signal)]
    pub async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

type MenuNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn menu_db() -> HashMap<i32, Vec<i32>> {
    HashMap::from([(0, vec![1, 2, 3]), (3, vec![31, 32])])
}

fn menu_props(id: i32) -> HashMap<String, OwnedValue> {
    let sv = |s: &str| OwnedValue::try_from(Value::from(s)).unwrap();
    let mut p = HashMap::new();
    match id {
        1 => {
            p.insert("label".into(), sv("E_xit"));
        }
        2 => {
            p.insert("label".into(), sv("__File"));
            p.insert(
                "enabled".into(),
                OwnedValue::try_from(Value::from(false)).unwrap(),
            );
        }
        3 => {
            p.insert("label".into(), sv("Sub"));
            p.insert("children-display".into(), sv("submenu"));
        }
        31 => {
            p.insert("label".into(), sv("Sub_A"));
            p.insert("toggle-type".into(), sv("radio"));
            p.insert(
                "toggle-state".into(),
                OwnedValue::try_from(Value::from(1i32)).unwrap(),
            );
        }
        32 => {
            p.insert("type".into(), sv("separator"));
        }
        _ => {}
    }
    p
}

fn child_node(id: i32) -> OwnedValue {
    let node: MenuNode = (id, menu_props(id), vec![]);
    OwnedValue::try_from(Value::Structure(node.into())).unwrap()
}

struct MockMenu {
    calls: Arc<MockCalls>,
}

#[interface(name = "com.canonical.dbusmenu")]
impl MockMenu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn status(&self) -> String {
        "normal".into()
    }

    async fn about_to_show(&self, id: i32) -> bool {
        self.calls.record(format!("about_to_show {id}"));
        false
    }

    async fn get_layout(&self, parent: i32, _depth: i32, _props: Vec<String>) -> (u32, MenuNode) {
        let children_ids = menu_db().get(&parent).cloned().unwrap_or_default();
        let children: Vec<OwnedValue> = children_ids.iter().map(|id| child_node(*id)).collect();
        (7, (parent, menu_props(parent), children))
    }

    async fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _props: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        ids.into_iter().map(|id| (id, menu_props(id))).collect()
    }

    async fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _ts: u32) {
        self.calls.record(format!("event {id} {event_id}"));
    }

    #[zbus(signal)]
    pub async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

// ── 测试用运行时（驱动 mock 的 zbus 连接）──

struct TestRt {
    handle: tokio::runtime::Handle,
}

impl TestRt {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            tx.send(rt.handle().clone()).unwrap();
            rt.block_on(std::future::pending::<()>());
        });
        Self {
            handle: rx.recv().unwrap(),
        }
    }

    fn block_on<T: Send + 'static>(&self, f: impl Future<Output = T> + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.handle.spawn(async move {
            let _ = tx.send(f.await);
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("test rt op")
    }
}

fn recv_within(rx: &mut tokio::sync::mpsc::Receiver<TrayEvent>, ms: u64) -> Option<TrayEvent> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        match rx.try_recv() {
            Ok(ev) => return Some(ev),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return None,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn eventually_recv(
    rx: &mut tokio::sync::mpsc::Receiver<TrayEvent>,
    ms: u64,
    pred: impl Fn(&TrayEvent) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        match rx.try_recv() {
            Ok(ev) => {
                if pred(&ev) {
                    return true;
                }
                // 跳过无关事件（如总线上的其他真实 Item 活动）
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return false,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn is_mock_event(ev: &TrayEvent, kind: &str) -> bool {
    match (kind, ev) {
        ("added", TrayEvent::Added(id))
        | ("removed", TrayEvent::Removed(id))
        | ("changed", TrayEvent::Changed(id))
        | ("menu_changed", TrayEvent::MenuChanged(id)) => id == MOCK_ID,
        _ => false,
    }
}

/// 轮询等待条件成立（fire-and-forget 调用回执的竞态护栏）。
fn wait_until(ms: u64, pred: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if pred() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ── 生命周期测试（串行单测内完成全部阶段）──

#[test]
fn lifecycle() {
    let Some((handle, mut rx)) = try_start() else {
        return;
    };

    // 快照：首事件（可能含总线上其他真实 Item）
    match recv_within(&mut rx, 3000) {
        Some(TrayEvent::Snapshot { items }) => {
            println!("[lifecycle] snapshot: {} item(s)", items.len());
        }
        other => panic!("expected Snapshot as first event, got {other:?}"),
    }

    // mock Item 上线
    let rt = TestRt::new();
    let calls = Arc::new(MockCalls::default());
    let title = Arc::new(Mutex::new("Mock Tray Item".to_string()));
    let menu_only = Arc::new(Mutex::new(false));
    let calls_for_item = calls.clone();
    let calls_for_menu = calls.clone();
    let title_for_item = title.clone();
    let menu_only_for_item = menu_only.clone();
    let conn = rt.block_on(async move {
        let conn = Connection::session().await.expect("session bus");
        conn.object_server()
            .at(
                MOCK_ITEM_PATH,
                MockItem {
                    calls: calls_for_item,
                    title: title_for_item,
                    menu_only: menu_only_for_item,
                },
            )
            .await
            .unwrap();
        conn.object_server()
            .at(
                MOCK_MENU_PATH,
                MockMenu {
                    calls: calls_for_menu,
                },
            )
            .await
            .unwrap();
        conn.request_name(MOCK_ID).await.unwrap();
        conn
    });

    let reg_conn = conn.clone();
    rt.block_on(async move {
        reg_conn
            .call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some(WATCHER_IFACE),
                "RegisterStatusNotifierItem",
                &MOCK_ID,
            )
            .await
            .unwrap();
    });

    // Added（数据现场读取）
    assert!(
        eventually_recv(&mut rx, 3000, |ev| is_mock_event(ev, "added")),
        "expected Added({MOCK_ID})"
    );

    // 协议面：在册清单包含 mock；Host 桩恒真
    let props_conn = conn.clone();
    let watcher_props: HashMap<String, OwnedValue> = rt.block_on(async move {
        props_conn
            .call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &WATCHER_IFACE,
            )
            .await
            .unwrap()
            .body()
            .deserialize()
            .unwrap()
    });
    let listed: Vec<String> = watcher_props
        .get("RegisteredStatusNotifierItems")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| v.try_into().ok())
        .unwrap_or_default();
    assert!(
        listed.contains(&MOCK_ID.to_string()),
        "watcher property must list mock"
    );
    let host_registered: bool = watcher_props
        .get("IsStatusNotifierHostRegistered")
        .and_then(|v| v.try_into().ok())
        .unwrap_or(false);
    assert!(host_registered, "host stub must answer true");

    // 现场读取：item() 全字段
    let item = handle.item(&MOCK_ID.to_string()).expect("item read");
    assert_eq!(item.id, MOCK_ID);
    assert_eq!(item.ident, "mock");
    assert_eq!(item.title.as_deref(), Some("Mock Tray Item"));
    assert_eq!(item.status, TrayStatus::Active);
    assert!(!item.is_menu_only);
    assert_eq!(item.icon, IconSource::Name("edit-copy".into()));
    assert_eq!(item.menu.as_deref(), Some(MOCK_MENU_PATH));
    assert!(item.alive);
    assert!(handle.is_alive(&MOCK_ID.to_string()));
    assert!(!handle.is_alive(&"org.kde.StatusNotifierItem-0-0".to_string()));
    assert_eq!(handle.item(&"nope".to_string()), None);
    assert!(matches!(
        handle.left_click(&"nope".to_string()),
        Err(TrayError::ItemNotFound(_))
    ));

    // 右键 → Menu 流程（现场快照）
    match handle.right_click(&MOCK_ID.to_string()) {
        Ok(InteractionOutcome::Menu(menu)) => {
            assert_eq!(menu.parent_id, 0);
            assert_eq!(menu.revision, 7);
            assert_eq!(menu.items.len(), 3);
            assert_eq!(menu.items[0].label, "Exit");
            assert_eq!(menu.items[1].label, "_File");
            assert!(!menu.items[1].enabled);
            assert!(menu.items[2].children_display);
            assert!(
                calls.contains("about_to_show 0"),
                "AboutToShow must precede GetLayout"
            );
        }
        other => panic!("right_click should route to Menu flow, got {other:?}"),
    }

    // 子层懒加载
    match handle.menu_expand(&MOCK_ID.to_string(), 3) {
        Ok(menu) => {
            assert_eq!(menu.parent_id, 3);
            assert_eq!(menu.items.len(), 2);
            assert_eq!(menu.items[0].label, "SubA");
            assert_eq!(menu.items[0].toggle_state, Some(ToggleState::On));
            assert_eq!(menu.items[1].item_type, MenuItemType::Separator);
        }
        Err(e) => panic!("menu_expand failed: {e}"),
    }

    // menu() 直读根层（与 click 路由共用流程）
    assert!(matches!(
        handle.menu(&MOCK_ID.to_string()),
        Ok(ref m) if m.items.len() == 3
    ));
    assert!(matches!(
        handle.menu(&"nope".to_string()),
        Err(TrayError::ItemNotFound(_))
    ));

    // menu_activate：受理 + 回执
    handle.menu_activate(&MOCK_ID.to_string(), 31).unwrap();
    assert!(eventually_recv(&mut rx, 3000, |ev| matches!(
        ev,
        TrayEvent::InteractionResult { id, op: InteractionOp::MenuActivate, result: Ok(()) } if id == MOCK_ID
    )));
    assert!(calls.contains("event 31 clicked"));

    // 左键（非 menu_only）→ Activate 路由
    match handle.left_click(&MOCK_ID.to_string()) {
        Ok(InteractionOutcome::Dispatched) => {}
        other => panic!("left_click should dispatch Activate, got {other:?}"),
    }
    handle.left_click_at(&MOCK_ID.to_string(), 10, 20).unwrap();
    assert!(eventually_recv(&mut rx, 3000, |ev| matches!(
        ev,
        TrayEvent::InteractionResult { id, op: InteractionOp::Activate, result: Ok(()) } if id == MOCK_ID
    )));
    assert!(calls.contains("activate 0 0"));
    assert!(calls.contains("activate 10 20"));

    // 中键 / 滚轮（up/left 为负）；回执与调用记录均需等待到达
    handle.middle_click(&MOCK_ID.to_string()).unwrap();
    handle.scroll_up_by(&MOCK_ID.to_string(), 3).unwrap();
    handle.scroll_right(&MOCK_ID.to_string()).unwrap();
    assert!(wait_until(3000, || {
        calls.contains("secondary 0 0") && calls.contains("scroll -3 vertical")
    }));
    assert!(wait_until(3000, || calls.contains("scroll 1 horizontal")));

    // 变更通告：New* 信号与 PropertiesChanged 同窗口合并为单次 Changed
    *title.lock().unwrap() = "Mock v2".to_string();
    let sig_conn = conn.clone();
    rt.block_on(async move {
        let emitter = SignalEmitter::new(&sig_conn, MOCK_ITEM_PATH).unwrap();
        MockItem::new_icon(&emitter).await.unwrap();
        zbus::fdo::Properties::properties_changed(
            &emitter,
            "org.kde.StatusNotifierItem".try_into().unwrap(),
            HashMap::from([("Title", Value::from("Mock v2"))]),
            std::borrow::Cow::Borrowed(&[]),
        )
        .await
        .unwrap();
    });
    assert!(
        eventually_recv(&mut rx, 3000, |ev| is_mock_event(ev, "changed")),
        "expected Changed after New* / PropertiesChanged"
    );
    let updated = handle.item(&MOCK_ID.to_string()).unwrap();
    assert_eq!(updated.title.as_deref(), Some("Mock v2"));

    // ItemIsMenu 随 PropertiesChanged 就地更新：左键路由转入 Menu 流程
    *menu_only.lock().unwrap() = true;
    let mi_conn = conn.clone();
    rt.block_on(async move {
        let emitter = SignalEmitter::new(&mi_conn, MOCK_ITEM_PATH).unwrap();
        zbus::fdo::Properties::properties_changed(
            &emitter,
            "org.kde.StatusNotifierItem".try_into().unwrap(),
            HashMap::from([("ItemIsMenu", Value::from(true))]),
            std::borrow::Cow::Borrowed(&[]),
        )
        .await
        .unwrap();
    });
    assert!(
        eventually_recv(&mut rx, 3000, |ev| is_mock_event(ev, "changed")),
        "expected Changed after ItemIsMenu update"
    );
    assert!(wait_until(3000, || {
        matches!(
            handle.left_click(&MOCK_ID.to_string()),
            Ok(InteractionOutcome::Menu(_))
        )
    }));

    // 菜单变更信号 → MenuChanged（不代为重拉）
    let menu_sig_conn = conn.clone();
    rt.block_on(async move {
        let emitter = SignalEmitter::new(&menu_sig_conn, MOCK_MENU_PATH).unwrap();
        MockMenu::layout_updated(&emitter, 8, 0).await.unwrap();
    });
    assert!(
        eventually_recv(&mut rx, 3000, |ev| is_mock_event(ev, "menu_changed")),
        "expected MenuChanged after LayoutUpdated"
    );

    // 单实例 Watcher：二次启动 → NameTaken
    assert!(matches!(
        Tray::start(TrayConfig::default()),
        Err(TrayError::NameTaken(_))
    ));

    // mock 下线（服务名消失）→ Removed
    drop(conn);
    assert!(
        eventually_recv(&mut rx, 3000, |ev| is_mock_event(ev, "removed")),
        "expected Removed after mock leaves the bus"
    );
    assert_eq!(handle.item(&MOCK_ID.to_string()), None);

    // 停机：幂等；此后一律 Stopped；通道关闭（recv → None）
    handle.shutdown();
    handle.shutdown();
    assert!(matches!(
        handle.menu(&MOCK_ID.to_string()),
        Err(TrayError::Stopped)
    ));
    assert!(matches!(
        handle.left_click(&MOCK_ID.to_string()),
        Err(TrayError::Stopped)
    ));
    assert_eq!(handle.items(), Vec::<TrayItem>::new());
    assert!(
        recv_within(&mut rx, 1000).is_none(),
        "channel must close after shutdown"
    );
}

/// 总线不可用或 Watcher 名已被他人持有时返回 None（跳过生命周期测试）。
fn try_start() -> Option<(crate::TrayHandle, tokio::sync::mpsc::Receiver<TrayEvent>)> {
    match Tray::start(TrayConfig::default()) {
        Ok(v) => Some(v),
        Err(TrayError::NameTaken(_)) => {
            eprintln!("[lifecycle] SKIPPED: another watcher owns the bus name");
            None
        }
        Err(TrayError::Bus(e)) => {
            eprintln!("[lifecycle] SKIPPED: no session bus ({e})");
            None
        }
        Err(e) => panic!("unexpected start failure: {e}"),
    }
}
