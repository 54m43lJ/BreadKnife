//! Protocol integration tests.
//!
//! These tests need an isolated D-Bus session bus.  Run them with:
//!
//! ```text
//! dbus-run-session -- env BNOTE_TEST_DBUS=1 cargo test --test protocol
//! ```
//!
//! Without `BNOTE_TEST_DBUS` they are skipped, so an ordinary `cargo test`
//! never talks to the developer's desktop bus.

#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;

use bnote::{
    Action, Capabilities, CloseReason, Error, NoticeEvent, NoticeIcon, ServerInfo, Session,
    SessionConfig, Urgency,
};
use futures_util::StreamExt;
use zbus::Connection;
use zbus::zvariant::{Array, OwnedValue, StructureBuilder, Value};

static BUS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled() -> bool {
    std::env::var_os("BNOTE_TEST_DBUS").is_some()
        && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
}

fn config() -> SessionConfig {
    SessionConfig {
        capabilities: Capabilities::new(["actions", "body", "body-markup"]),
        server_info: ServerInfo {
            name: "bnote-test".into(),
            vendor: "breadknife".into(),
            version: "0.1.0".into(),
            spec_version: "1.2".into(),
        },
        command_timeout_ms: 2000,
    }
}

fn image_data() -> OwnedValue {
    let structure = StructureBuilder::new()
        .add_field(2i32)
        .add_field(1i32)
        .add_field(8i32)
        .add_field(false)
        .add_field(8i32)
        .add_field(3i32)
        .add_field(Array::from(vec![255u8, 0, 0, 0, 255, 0, 9, 9]))
        .build()
        .unwrap();
    OwnedValue::try_from(structure).unwrap()
}

fn text(value: &str) -> OwnedValue {
    OwnedValue::try_from(Value::from(value.to_owned())).unwrap()
}

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;

    fn get_server_information(&self) -> zbus::Result<(String, String, String, String)>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn activation_token(&self, id: u32, token: &str) -> zbus::Result<()>;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn methods_events_and_host_signals() {
    if !enabled() {
        return;
    }
    let _guard = BUS_LOCK.lock().await;

    let (handle, mut events) = Session::start(config()).unwrap();
    let connection = Connection::session().await.unwrap();
    let proxy = NotificationsProxy::new(&connection).await.unwrap();

    assert_eq!(
        proxy.get_capabilities().await.unwrap(),
        ["actions", "body", "body-markup"]
    );
    assert_eq!(
        proxy.get_server_information().await.unwrap(),
        (
            "bnote-test".to_owned(),
            "breadknife".to_owned(),
            "0.1.0".to_owned(),
            "1.2".to_owned(),
        )
    );

    let mut closed = proxy.receive_notification_closed().await.unwrap();
    let mut invoked = proxy.receive_action_invoked().await.unwrap();
    let mut tokens = proxy.receive_activation_token().await.unwrap();

    let mut hints = HashMap::new();
    hints.insert("image-data".to_owned(), image_data());
    hints.insert("urgency".to_owned(), OwnedValue::from(2u8));
    hints.insert("image-path".to_owned(), text("ignored-icon"));

    let id = proxy
        .notify(
            "test-app",
            0,
            "app-icon",
            "Summary",
            "Body",
            vec!["default".into(), "Open".into()],
            hints,
            5000,
        )
        .await
        .unwrap();
    assert!(id > 0);

    let notice = match events.recv().await.unwrap() {
        NoticeEvent::Notify { notice } => notice,
        other => panic!("unexpected event: {other:?}"),
    };
    assert_eq!(notice.id, id);
    assert_eq!(notice.app_name, "test-app");
    assert_eq!(notice.summary, "Summary");
    assert_eq!(notice.body, "Body");
    assert_eq!(notice.urgency, Urgency::Critical);
    assert_eq!(
        notice.actions,
        vec![Action {
            key: "default".into(),
            label: "Open".into(),
        }]
    );
    assert_eq!(notice.expire_timeout_ms, Some(5000));
    assert_eq!(
        notice.icon,
        NoticeIcon::Pixels {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        }
    );

    let replaced = proxy
        .notify(
            "test-app",
            id,
            "",
            "Updated",
            "",
            Vec::new(),
            HashMap::new(),
            0,
        )
        .await
        .unwrap();
    assert_eq!(replaced, id);
    let notice = match events.recv().await.unwrap() {
        NoticeEvent::Notify { notice } => notice,
        other => panic!("unexpected event: {other:?}"),
    };
    assert_eq!(notice.id, id);
    assert_eq!(notice.summary, "Updated");
    assert!(notice.actions.is_empty());
    assert_eq!(notice.expire_timeout_ms, Some(0));

    handle.invoke_action(id, "default").unwrap();
    let signal = invoked.next().await.unwrap();
    let args = signal.args().unwrap();
    assert_eq!(args.id, id);
    assert_eq!(args.action_key, "default");

    handle.activation_token(id, "token-1").unwrap();
    let signal = tokens.next().await.unwrap();
    let args = signal.args().unwrap();
    assert_eq!(args.id, id);
    assert_eq!(args.token, "token-1");

    handle.close(id, CloseReason::Expired).unwrap();
    let signal = closed.next().await.unwrap();
    let args = signal.args().unwrap();
    assert_eq!(args.id, id);
    assert_eq!(args.reason, 1);

    assert!(matches!(
        handle.close(id, CloseReason::Expired),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.invoke_action(id, "default"),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.activation_token(id, "token-2"),
        Err(Error::InvalidArgs(_))
    ));

    handle.shutdown();
    assert_eq!(events.recv().await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_close_invalidates_the_id_and_emits_reason_three() {
    if !enabled() {
        return;
    }
    let _guard = BUS_LOCK.lock().await;

    let (handle, mut events) = Session::start(config()).unwrap();
    let connection = Connection::session().await.unwrap();
    let proxy = NotificationsProxy::new(&connection).await.unwrap();
    let mut closed = proxy.receive_notification_closed().await.unwrap();

    let id = proxy
        .notify(
            "test-app",
            0,
            "",
            "Summary",
            "",
            Vec::new(),
            HashMap::new(),
            -1,
        )
        .await
        .unwrap();
    let notice = match events.recv().await.unwrap() {
        NoticeEvent::Notify { notice } => notice,
        other => panic!("unexpected event: {other:?}"),
    };
    assert_eq!(notice.expire_timeout_ms, None);

    proxy.close_notification(id).await.unwrap();
    assert_eq!(events.recv().await, Some(NoticeEvent::Close { id }));

    let signal = closed.next().await.unwrap();
    let args = signal.args().unwrap();
    assert_eq!(args.id, id);
    assert_eq!(args.reason, 3);

    match proxy.close_notification(id).await {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.InvalidArgs");
        }
        other => panic!("expected an InvalidArgs D-Bus error, got {other:?}"),
    }

    assert!(matches!(
        handle.close(id, CloseReason::Dismissed),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.invoke_action(id, "default"),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.activation_token(id, "token-2"),
        Err(Error::InvalidArgs(_))
    ));

    handle.shutdown();
    assert_eq!(events.recv().await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_actions_are_rejected_and_second_session_reports_name_taken() {
    if !enabled() {
        return;
    }
    let _guard = BUS_LOCK.lock().await;

    let (handle, _events) = Session::start(config()).unwrap();
    let connection = Connection::session().await.unwrap();
    let proxy = NotificationsProxy::new(&connection).await.unwrap();

    match proxy
        .notify(
            "test-app",
            0,
            "",
            "Summary",
            "",
            vec!["orphan".into()],
            HashMap::new(),
            0,
        )
        .await
    {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.InvalidArgs");
        }
        other => panic!("expected an InvalidArgs D-Bus error, got {other:?}"),
    }

    match Session::start(config()) {
        Err(Error::NameTaken(name)) => assert_eq!(name, "org.freedesktop.Notifications"),
        Err(other) => panic!("expected NameTaken, got {other:?}"),
        Ok(_) => panic!("expected NameTaken, got a second session"),
    }

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_can_be_rebuilt_after_shutdown() {
    if !enabled() {
        return;
    }
    let _guard = BUS_LOCK.lock().await;

    for _ in 0..5 {
        let (handle, mut events) = Session::start(config()).unwrap();
        drop(handle);
        assert_eq!(events.recv().await, None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_ids_are_rejected_by_host_commands() {
    if !enabled() {
        return;
    }
    let _guard = BUS_LOCK.lock().await;

    let (handle, _events) = Session::start(config()).unwrap();
    assert!(matches!(
        handle.close(999, CloseReason::Expired),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.invoke_action(999, "default"),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.activation_token(999, "token"),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.invoke_action(0, "default"),
        Err(Error::InvalidArgs(_))
    ));
    assert!(matches!(
        handle.activation_token(0, "token"),
        Err(Error::InvalidArgs(_))
    ));
    handle.shutdown();
    assert!(matches!(
        handle.close(1, CloseReason::Expired),
        Err(Error::Stopped)
    ));
}
