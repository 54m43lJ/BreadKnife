//! org.freedesktop.Notifications 守护角色：bnote 装配 + NameTaken 重试 + 提示音。
//! 事件映射与命令线程契约见 CORE.html §4.6 / §5.5–§5.7。

use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use crate::daemon::DaemonEvent;
use crate::shared::model::{
    NotificationAction, NotificationIcon, NotificationRecord, NotificationUrgency,
};

/// SessionHandle 同步命令等待上限。
pub const NOTIFICATION_COMMAND_TIMEOUT_MS: u64 = 5_000;
/// 提示音后端。
pub const SOUND_BACKEND: &str = "canberra-gtk-play";

pub struct NotificationService {
    session: bnote::SessionHandle,
    command_tx: Sender<NotificationCommand>,
    event_thread: Mutex<Option<JoinHandle<()>>>,
    command_thread: Mutex<Option<JoinHandle<()>>>,
}

pub enum NotificationCommand {
    Close {
        id: u32,
        reason: bnote::CloseReason,
    },
    InvokeAction {
        id: u32,
        key: String,
        activation_token: Option<String>,
    },
    Shutdown,
}

/// 同步建立 bnote 会话；失败时由 daemon 记录为 Unavailable。
pub fn start(tx: Sender<DaemonEvent>) -> Result<NotificationService, bnote::Error> {
    let (session, mut receiver) = bnote::Session::start(session_config())?;
    let (command_tx, command_rx) = mpsc::channel::<NotificationCommand>();

    let event_tx = tx.clone();
    let event_thread = std::thread::Builder::new()
        .name("notify-events".into())
        .spawn(move || {
            while let Some(event) = futures::executor::block_on(receiver.recv()) {
                let daemon_event = match event {
                    bnote::NoticeEvent::Notify { notice } => {
                        DaemonEvent::Notification(to_record(notice))
                    }
                    bnote::NoticeEvent::Close { id } => DaemonEvent::NotificationClosed(id),
                    bnote::NoticeEvent::Fatal { message } => DaemonEvent::NotificationStatus(
                        crate::shared::model::NotificationStatus::Unavailable { message },
                    ),
                };
                if event_tx.send(daemon_event).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| bnote::Error::Bus(format!("spawn notification event thread: {e}")))?;

    let command_session = session.clone();
    let status_tx = tx.clone();
    let command_thread = std::thread::Builder::new()
        .name("notify-commands".into())
        .spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    NotificationCommand::Close { id, reason } => {
                        if !report_command(&status_tx, command_session.close(id, reason)) {
                            break;
                        }
                    }
                    NotificationCommand::InvokeAction {
                        id,
                        key,
                        activation_token,
                    } => {
                        let token_ok = activation_token.is_none_or(|token| {
                            report_command(&status_tx, command_session.activation_token(id, &token))
                        });
                        if !token_ok {
                            break;
                        }
                        if !report_command(&status_tx, command_session.invoke_action(id, &key)) {
                            break;
                        }
                    }
                    NotificationCommand::Shutdown => break,
                }
            }
            command_session.shutdown();
        })
        .map_err(|e| bnote::Error::Bus(format!("spawn notification command thread: {e}")))?;

    Ok(NotificationService {
        session,
        command_tx,
        event_thread: Mutex::new(Some(event_thread)),
        command_thread: Mutex::new(Some(command_thread)),
    })
}

impl NotificationService {
    pub fn close(&self, id: u32, reason: bnote::CloseReason) {
        let _ = self
            .command_tx
            .send(NotificationCommand::Close { id, reason });
    }

    pub fn invoke_action(&self, id: u32, key: &str, activation_token: Option<&str>) {
        let _ = self.command_tx.send(NotificationCommand::InvokeAction {
            id,
            key: key.to_string(),
            activation_token: activation_token.map(str::to_string),
        });
    }

    pub fn shutdown(&self) {
        let _ = self.command_tx.send(NotificationCommand::Shutdown);
        if let Some(join) = self
            .command_thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = join.join();
        }
        self.session.shutdown();
        if let Some(join) = self
            .event_thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = join.join();
        }
    }
}

/// 命令失败处置：Timeout 记日志（可重试/重建），Stopped 上报不可用并终止命令线程。
fn report_command(tx: &Sender<DaemonEvent>, result: Result<(), bnote::Error>) -> bool {
    match result {
        Ok(()) => true,
        Err(bnote::Error::Stopped) => {
            let _ = tx.send(DaemonEvent::NotificationStatus(
                crate::shared::model::NotificationStatus::Unavailable {
                    message: "通知服务已停止，请点击刷新重建会话。".to_string(),
                },
            ));
            false
        }
        Err(error) => {
            eprintln!("[notify] command failed: {error}");
            true
        }
    }
}

fn session_config() -> bnote::SessionConfig {
    let mut capabilities = vec![
        "actions",
        "action-icons",
        "body",
        "body-hyperlinks",
        "body-images",
        "body-markup",
        "icon-static",
        "persistence",
    ];
    if sound_available() {
        capabilities.push("sound");
    }
    bnote::SessionConfig {
        capabilities: bnote::Capabilities::new(capabilities),
        server_info: bnote::ServerInfo {
            name: "bread".to_string(),
            vendor: "BreadKnife".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            spec_version: "1.2".to_string(),
        },
        command_timeout_ms: NOTIFICATION_COMMAND_TIMEOUT_MS,
    }
}

pub fn unavailable_message(err: bnote::Error) -> String {
    match err {
        bnote::Error::NameTaken(_) => {
            "另一个通知服务正在运行，bread 无法接管 org.freedesktop.Notifications。".to_string()
        }
        other => other.to_string(),
    }
}

fn to_record(notice: bnote::Notice) -> NotificationRecord {
    let icon = match notice.icon {
        bnote::NoticeIcon::None => NotificationIcon::None,
        bnote::NoticeIcon::Name(name) => NotificationIcon::Name(name),
        bnote::NoticeIcon::Pixels {
            width,
            height,
            rgba,
        } => NotificationIcon::Pixels {
            width,
            height,
            rgba,
        },
    };
    let urgency = match notice.urgency {
        bnote::Urgency::Low => NotificationUrgency::Low,
        bnote::Urgency::Normal => NotificationUrgency::Normal,
        bnote::Urgency::Critical => NotificationUrgency::Critical,
    };
    NotificationRecord {
        id: notice.id,
        app_name: notice.app_name,
        icon,
        summary: notice.summary,
        body: notice.body,
        actions: notice
            .actions
            .into_iter()
            .map(|action| NotificationAction {
                key: action.key,
                label: action.label,
            })
            .collect(),
        action_icons: notice.action_icons,
        urgency,
        expire_timeout_ms: notice.expire_timeout_ms,
        transient: notice.transient,
        resident: notice.resident,
        dismissed: false,
        sound_file: notice.sound_file,
        sound_name: notice.sound_name,
        suppress_sound: notice.suppress_sound,
    }
}

/// PATH 中是否存在提示音后端。
pub fn sound_available() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(SOUND_BACKEND))
        .any(|candidate| candidate.is_file())
}

/// 到达时播放一次；失败只记日志，不阻塞事件循环。
pub fn play_sound(record: &NotificationRecord) {
    if record.suppress_sound || !sound_available() {
        return;
    }
    let (flag, value) = if let Some(file) = &record.sound_file {
        ("-f", file.clone())
    } else if let Some(name) = &record.sound_name {
        ("-i", name.clone())
    } else {
        return;
    };
    std::thread::Builder::new()
        .name("notify-sound".into())
        .spawn(move || {
            let _ = std::process::Command::new(SOUND_BACKEND)
                .arg(flag)
                .arg(value)
                .status();
        })
        .ok();
}
