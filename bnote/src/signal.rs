//! 三个规范信号的发射：`NotificationClosed` / `ActionInvoked` / `ActivationToken`。
//!
//! 规范条文见 `bnote/reference/notification-spec.clean.html §9.2`。

use zbus::Connection;

use crate::types::CloseReason;
use crate::{Error, INTERFACE, PATH};

/// 发射 `NotificationClosed(id, reason)`（规范 §9.2.1）。
pub async fn emit_closed(conn: &Connection, id: u32, reason: CloseReason) -> Result<(), Error> {
    conn.emit_signal(
        None::<&str>,
        PATH,
        INTERFACE,
        "NotificationClosed",
        &(id, reason.code()),
    )
    .await?;
    Ok(())
}

/// 发射 `ActionInvoked(id, action_key)`（规范 §9.2.2）。
pub async fn emit_action_invoked(
    conn: &Connection,
    id: u32,
    action_key: &str,
) -> Result<(), Error> {
    conn.emit_signal(
        None::<&str>,
        PATH,
        INTERFACE,
        "ActionInvoked",
        &(id, action_key),
    )
    .await?;
    Ok(())
}

/// 发射 `ActivationToken(id, token)`（规范 §9.2.3）。
pub async fn emit_activation_token(conn: &Connection, id: u32, token: &str) -> Result<(), Error> {
    conn.emit_signal(
        None::<&str>,
        PATH,
        INTERFACE,
        "ActivationToken",
        &(id, token),
    )
    .await?;
    Ok(())
}
