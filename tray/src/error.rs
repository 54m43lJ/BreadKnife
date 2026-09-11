//! Crate-wide error type.

use crate::TrayItemId;

/// Errors surfaced by the tray library.
#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    /// Failed to connect to the session bus.
    #[error("session bus connect failed: {0}")]
    Bus(String),
    /// An operation timed out after the configured duration.
    #[error("operation timed out after {ms} ms")]
    Timeout {
        /// Timeout in milliseconds.
        ms: u64,
    },
    /// The referenced item does not exist (or is gone).
    #[error("item {0} not found")]
    ItemNotFound(String),
    /// A DBus protocol call failed.
    #[error("dbus call failed: {0}")]
    Protocol(String),
    /// The tray runtime has stopped and can no longer serve requests.
    #[error("tray runtime stopped")]
    Stopped,
}

impl TrayError {
    pub(crate) fn item_not_found(id: &TrayItemId) -> Self {
        TrayError::ItemNotFound(id.to_string())
    }
}

impl From<zbus::Error> for TrayError {
    fn from(err: zbus::Error) -> Self {
        TrayError::Protocol(err.to_string())
    }
}

impl From<zbus::fdo::Error> for TrayError {
    fn from(err: zbus::fdo::Error) -> Self {
        TrayError::Protocol(err.to_string())
    }
}

impl From<zbus::names::Error> for TrayError {
    fn from(err: zbus::names::Error) -> Self {
        TrayError::Protocol(err.to_string())
    }
}

impl From<zbus::zvariant::Error> for TrayError {
    fn from(err: zbus::zvariant::Error) -> Self {
        TrayError::Protocol(err.to_string())
    }
}
