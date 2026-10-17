//! 协议与模型测试：帧回环、控制行解析、base64 载荷。

use std::os::unix::net::UnixStream;

use crate::shared::model::*;
use crate::shared::protocol::*;

fn frame_roundtrip(frame: Frame) -> Frame {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    write_frame(&mut a, &frame).unwrap();
    read_frame(&mut b).unwrap()
}

fn sample_menu() -> MenuSnapshot {
    MenuSnapshot {
        parent_id: 0,
        revision: 7,
        items: vec![
            MenuItem {
                id: 1,
                item_type: MenuItemType::Standard,
                label: "打_开".into(),
                raw_label: "打__开".into(),
                enabled: true,
                visible: true,
                children_display: false,
                toggle_type: Some(ToggleType::Checkmark),
                toggle_state: Some(ToggleState::On),
                icon_name: Some("media-playback-start".into()),
                icon_data: Some(vec![0x89, 0x50, 0x4E, 0x47]),
            },
            MenuItem {
                id: 2,
                item_type: MenuItemType::Separator,
                label: String::new(),
                raw_label: String::new(),
                enabled: true,
                visible: true,
                children_display: false,
                toggle_type: None,
                toggle_state: None,
                icon_name: None,
                icon_data: None,
            },
            MenuItem {
                id: 3,
                item_type: MenuItemType::Standard,
                label: "子菜单".into(),
                raw_label: "子菜单".into(),
                enabled: false,
                visible: false,
                children_display: true,
                toggle_type: None,
                toggle_state: None,
                icon_name: None,
                icon_data: None,
            },
        ],
    }
}

fn sample_snapshot() -> WindowSnapshot {
    WindowSnapshot {
        monitor: "DP-1".into(),
        workspaces: vec![WorkspaceState {
            id: 1,
            name: "1".into(),
            urgent: true,
            active: false,
            has_windows: true,
        }],
        active: ActiveWindow { class: "firefox".into(), title: "Test".into() },
        tray: vec![TrayItemView {
            id: "org.example.Item".into(),
            title: Some("Example".into()),
            tooltip: Some("Example\n详情".into()),
            icon: IconView::Pixmap {
                width: 2,
                height: 2,
                argb: vec![0xFF, 0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF, 0x77, 0x88, 0x99, 0xFF, 0xAA, 0xBB, 0xCC],
            },
            attention: false,
            alive: true,
            has_menu: true,
        }],
        notifications: vec![NotificationRecord {
            id: 1,
            app_name: "app".into(),
            icon: "icon".into(),
            summary: "s".into(),
            body: "b".into(),
        }],
        system: SystemState::default(),
        css_path: "/tmp/bread.css".into(),
    }
}

#[test]
fn frame_push_snapshot_roundtrip() {
    let frame = Frame::Push { push: Push::Snapshot(sample_snapshot()) };
    let Frame::Push { push: Push::Snapshot(got) } = frame_roundtrip(frame) else {
        panic!("wrong frame");
    };
    assert_eq!(got, sample_snapshot());
}

#[test]
fn frame_request_response_roundtrip() {
    let req = Frame::Request {
        id: 42,
        request: Request::TrayInteract {
            item: "org.example.Item".into(),
            input: InteractInput::ScrollLeft { steps: 3 },
        },
    };
    assert_eq!(frame_roundtrip(req.clone()), req);

    let resp = Frame::Response {
        id: 42,
        response: Response::Outcome(InteractionOutcome::Menu(sample_menu())),
    };
    assert_eq!(frame_roundtrip(resp.clone()), resp);
}

#[test]
fn frame_length_guard() {
    use std::io::Write;
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(&(u32::MAX.to_le_bytes())).unwrap();
    drop(a);
    assert!(read_frame(&mut b).is_err());
}

#[test]
fn control_line_parsing() {
    use ControlCommand::*;
    assert_eq!(
        parse_control_line("open bar"),
        Some(Open { name: "bar".into(), monitor: None })
    );
    assert_eq!(
        parse_control_line("open bar -m DP-1"),
        Some(Open { name: "bar".into(), monitor: Some("DP-1".into()) })
    );
    assert_eq!(parse_control_line("close-all"), Some(CloseAll));
    assert_eq!(parse_control_line("reload"), Some(Reload));
    assert_eq!(parse_control_line("exit"), Some(Exit));
    assert_eq!(parse_control_line("frobnicate"), None);
    assert_eq!(parse_control_line("open bar -m"), None); // -m 缺参
}

#[test]
fn daemon_alive_without_pid_file() {
    // 测试环境不落盘：仅当默认 PID 路径恰好缺失时成立
    if !crate::shared::protocol::pid_file().exists() {
        assert!(!daemon_alive());
    }
}
