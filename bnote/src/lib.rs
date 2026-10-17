//! bnote：`org.freedesktop.Notifications` 协议库。
//!
//! 宿主装配 [`Session`] 后从事件通道接收 [`NoticeEvent`]；关闭、动作与激活令牌
//! 通过 [`close`] / [`invoke_action`] / [`activation_token`] 显式回发。
//! 协议权威文本见 `reference/notification-spec.clean.html`。

pub mod hints;
mod server;
pub mod signal;
mod types;

use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc::Sender};
use zbus::Connection;

pub use signal::{emit_action_invoked, emit_activation_token, emit_closed};
pub use types::{
    Action, Capabilities, CloseReason, Notice, NoticeEvent, NoticeIcon, ServerInfo, Urgency,
};

pub(crate) const BUS_NAME: &str = "org.freedesktop.Notifications";
pub(crate) const PATH: &str = "/org/freedesktop/Notifications";
pub(crate) const INTERFACE: &str = "org.freedesktop.Notifications";

/// 库错误。
#[derive(Debug)]
pub enum Error {
    /// D-Bus 连接、对象注册或信号发射失败。
    Bus(zbus::Error),
    /// 宿主传入的参数或当前装配状态不合法。
    InvalidArgs(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(error) => write!(f, "D-Bus error: {error}"),
            Self::InvalidArgs(message) => write!(f, "invalid arguments: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bus(error) => Some(error),
            Self::InvalidArgs(_) => None,
        }
    }
}

impl From<zbus::Error> for Error {
    fn from(error: zbus::Error) -> Self {
        Self::Bus(error)
    }
}

/// 已注册的通知服务会话。
pub struct Session {
    conn: Connection,
}

impl Session {
    /// 在 `conn` 上注册 `/org/freedesktop/Notifications` 并占用
    /// `org.freedesktop.Notifications` 总线名。
    ///
    /// 返回后服务即可接收方法调用；事件经 `tx` 流向宿主。
    pub async fn start(
        conn: &Connection,
        tx: Sender<NoticeEvent>,
        caps: Capabilities,
        info: ServerInfo,
    ) -> Result<Rc<Self>, Error> {
        validate_capabilities(&caps)?;

        let state = Arc::new(Mutex::new(server::State::default()));
        let server = server::Server::new(state, tx, caps, info);

        if !conn.object_server().at(PATH, server).await? {
            return Err(Error::InvalidArgs(format!(
                "interface already registered at {PATH}"
            )));
        }

        if let Err(error) = conn.request_name(BUS_NAME).await {
            let _ = conn.object_server().remove::<server::Server, _>(PATH).await;
            return Err(error.into());
        }

        Ok(Rc::new(Self { conn: conn.clone() }))
    }

    /// 本会话使用的总线连接。
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// 回发 `NotificationClosed`，并从协议层在册集合移除 `id`。
///
/// 宿主在超时、用户关闭或删除通知后调用；`reason` 对应实际原因。
/// 重复关闭同一 id 不会重复发射信号。
pub async fn close(conn: &Connection, id: u32, reason: CloseReason) -> Result<(), Error> {
    let tracked = if let Ok(interface) = conn
        .object_server()
        .interface::<_, server::Server>(PATH)
        .await
    {
        interface.get().await.mark_closed(id).await
    } else {
        false
    };

    if tracked {
        signal::emit_closed(conn, id, reason).await?;
    }

    Ok(())
}

/// 回发 `ActionInvoked`。
pub async fn invoke_action(conn: &Connection, id: u32, action_key: &str) -> Result<(), Error> {
    signal::emit_action_invoked(conn, id, action_key).await
}

/// 回发 `ActivationToken`。
pub async fn activation_token(conn: &Connection, id: u32, token: &str) -> Result<(), Error> {
    signal::emit_activation_token(conn, id, token).await
}

/// 能力名限于字母数字与连字符（规范 §9.1.1）；`icon-static` / `icon-multi` 互斥。
fn validate_capabilities(caps: &Capabilities) -> Result<(), Error> {
    let mut icon_static = false;
    let mut icon_multi = false;

    for entry in &caps.entries {
        if entry.is_empty() || !entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::InvalidArgs(format!(
                "invalid capability name: {entry:?}"
            )));
        }

        match entry.as_str() {
            "icon-static" => icon_static = true,
            "icon-multi" => icon_multi = true,
            _ => {}
        }
    }

    if icon_static && icon_multi {
        return Err(Error::InvalidArgs(
            "capabilities icon-static and icon-multi are mutually exclusive".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_names_are_validated() {
        assert!(validate_capabilities(&Capabilities::new(["actions", "body-markup"])).is_ok());
        assert!(validate_capabilities(&Capabilities::new(["bad cap"])).is_err());
        assert!(validate_capabilities(&Capabilities::new([""])).is_err());
    }

    #[test]
    fn icon_capabilities_are_mutually_exclusive() {
        assert!(validate_capabilities(&Capabilities::new(["icon-static"])).is_ok());
        assert!(validate_capabilities(&Capabilities::new(["icon-multi"])).is_ok());
        assert!(validate_capabilities(&Capabilities::new(["icon-static", "icon-multi"])).is_err());
    }
}
