//! Full-lifecycle integration test (SPEC §11).
//!
//! Runs inside a private `dbus-run-session` if no session bus is present,
//! so `cargo test` works on any Linux machine with dbus installed.

use std::time::Duration;

use tray::{
    testing::{spawn_demo_item, wait_for, DemoItemConfig},
    IconSource, ItemField, Tray, TrayConfig, TrayEvent, TrayStatus, WatcherState,
};

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn full_lifecycle() {
    ensure_session_bus("full_lifecycle");

    let config = TrayConfig {
        timeout_ms: 1000,
        debounce_ms: 20,
        ..Default::default()
    };
    let (handle, rx) = Tray::connect(config).expect("tray connect");

    // Bare dbus-run-session has no watcher: we must have become the fallback.
    let watcher = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::WatcherChanged(s) => Some(*s),
        _ => None,
    });
    assert_eq!(watcher, Some(WatcherState::FallbackActive));

    // Spawn a demo item -> Added with sensible defaults.
    let demo = spawn_demo_item(DemoItemConfig::default()).expect("spawn demo item");
    let added = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::Added(item) => Some(item.clone()),
        _ => None,
    });
    let added = added.expect("Added event");
    assert_eq!(added.id, demo.id());
    assert_eq!(added.status, TrayStatus::Active);
    assert_eq!(added.icon, IconSource::Name("demo".into()));
    assert!(added.has_menu);

    // Snapshot APIs agree.
    assert_eq!(handle.item(&demo.id()).unwrap().id, demo.id());
    assert!(handle.menu(&demo.id()).is_some());
    assert_eq!(handle.items().len(), 1);

    // Status change -> Changed([Status]).
    demo.set_status("passive").unwrap();
    let changed = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::Changed { changed, item, .. } => Some((changed.clone(), item.clone())),
        _ => None,
    });
    let (fields, item) = changed.expect("Changed event");
    assert!(fields.contains(&ItemField::Status), "fields: {fields:?}");
    assert_eq!(item.status, TrayStatus::Passive);

    // Menu change -> MenuChanged + readable snapshot with the new entry.
    let hello = demo.menu_add("Hello").unwrap();
    let menu = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::MenuChanged(_) => Some(()),
        _ => None,
    });
    menu.expect("MenuChanged event");
    let menu = handle.menu(&demo.id()).expect("menu snapshot");
    let hello_item = menu
        .root
        .iter()
        .find(|e| e.id == hello)
        .expect("menu entry present");
    assert_eq!(hello_item.label, "Hello");

    // AboutToShow pre-show contract: sync refresh + readable snapshot.
    let _need = handle.menu_about_to_show(&demo.id(), 0).unwrap();
    assert!(handle.menu(&demo.id()).is_some());

    // Menu activation reaches the demo item and yields a receipt.
    handle.menu_activate(&demo.id(), hello).unwrap();
    let receipt = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::InteractionResult { result, .. } => Some(result.clone()),
        _ => None,
    });
    assert_eq!(receipt, Some(Ok(())));
    assert_eq!(demo.last_click(), Some(hello));

    // Pixmap icon -> Icon change with parsed bitmap.
    demo.use_pixmap(true).unwrap();
    let pixmap = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::Changed { item, .. } => Some(item.icon.clone()),
        _ => None,
    });
    match pixmap.expect("icon change") {
        IconSource::Pixmap(p) => {
            assert_eq!((p.width, p.height), (4, 2));
            assert_eq!(p.argb.len(), 32);
        }
        other => panic!("expected pixmap icon, got {other:?}"),
    }

    // Menu unmount -> HasMenu change.
    demo.set_menu_enabled(false).unwrap();
    let has_menu = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::Changed { changed, .. } => Some(changed.contains(&ItemField::HasMenu)),
        _ => None,
    });
    assert_eq!(has_menu, Some(true));

    // Synchronous AboutToShow on a menu-less item fails cleanly.
    assert!(handle.menu_about_to_show(&demo.id(), hello).is_err());

    // Removal -> Removed.
    let demo_id = demo.id();
    demo.remove().unwrap();
    let removed = wait_for(&rx, WAIT, |ev| match ev {
        TrayEvent::Removed(id) => Some(id.clone()),
        _ => None,
    });
    assert_eq!(removed, Some(demo_id.clone()));
    assert!(handle.items().is_empty());

    // Shutdown stops the runtime; further calls fail with Stopped.
    handle.shutdown();
    std::thread::sleep(Duration::from_millis(200));
    assert!(matches!(
        handle.activate(&demo_id, 0, 0),
        Err(tray::TrayError::Stopped)
    ));
}

/// A second host connecting later must enumerate items that registered
/// before it (exercises the property-based `RegisteredStatusNotifierItems`
/// read against our own watcher).
#[test]
fn second_host_enumerates_existing_item() {
    ensure_session_bus("second_host_enumerates_existing_item");

    let config = TrayConfig {
        timeout_ms: 1000,
        debounce_ms: 20,
        ..Default::default()
    };
    let (host_a, _rx) = Tray::connect(config.clone()).expect("host A connect");
    let demo = spawn_demo_item(DemoItemConfig::default()).expect("spawn demo item");

    // Host B never provides a watcher; the external one (host A) is present.
    let (host_b, rx_b) = Tray::connect(TrayConfig {
        fallback: tray::FallbackPolicy::Never,
        ..config
    })
    .expect("host B connect");

    let added = wait_for(&rx_b, WAIT, |ev| match ev {
        TrayEvent::Added(item) => Some(item.clone()),
        _ => None,
    });
    assert_eq!(added.expect("host B sees the item").id, demo.id());
    assert_eq!(host_b.items().len(), 1);

    host_b.shutdown();
    demo.remove().unwrap();
    host_a.shutdown();
}

/// Re-exec *this test alone* under a private `dbus-run-session`: the tests
/// depend on a *bare* session (no external StatusNotifierWatcher) and on
/// total isolation from each other, so each test gets its own bus.
fn ensure_session_bus(test: &'static str) {
    if std::env::var_os("TRAY_TEST_UNDER_DBUS").is_some() {
        return;
    }
    let exe = std::env::current_exe().expect("current exe");
    let status = std::process::Command::new("dbus-run-session")
        .arg("--")
        .arg(&exe)
        .arg("--exact")
        .arg(test)
        .arg("--nocapture")
        .env("TRAY_TEST_UNDER_DBUS", "1")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .status()
        .expect("spawn dbus-run-session (is dbus installed?)");
    std::process::exit(status.code().unwrap_or(1));
}
