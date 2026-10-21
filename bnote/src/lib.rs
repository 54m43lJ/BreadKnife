//! `org.freedesktop.Notifications` protocol library.
//!
//! A host starts a [`Session`] once, then consumes [`NoticeEvent`]s from the
//! returned [`NoticeReceiver`] and sends close / action / activation-token
//! commands through the [`SessionHandle`].  The library owns the D-Bus
//! connection and its runtime; the host never sees protocol details.
//!
//! The public contract is documented in `bnote/USER.html`; the protocol
//! surface is documented in `bnote/PROVIDER.html`.

#![forbid(unsafe_code)]

mod bus;
mod events;
mod handle;
mod hints;
mod server;
mod signal;
mod types;

pub use handle::{NoticeReceiver, Session, SessionHandle};
pub use types::{
    Action, Capabilities, CloseReason, Notice, NoticeEvent, NoticeIcon, ServerInfo, SessionConfig,
    Urgency,
};

use std::fmt;

pub(crate) const BUS_NAME: &str = "org.freedesktop.Notifications";
pub(crate) const PATH: &str = "/org/freedesktop/Notifications";
pub(crate) const INTERFACE: &str = "org.freedesktop.Notifications";

/// Library error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The session bus could not be reached, the object could not be
    /// registered, or a signal could not be sent.
    Bus(String),
    /// `org.freedesktop.Notifications` is already owned by another process.
    NameTaken(String),
    /// A `SessionHandle` synchronous command did not complete within
    /// `command_timeout_ms`.
    ///
    /// The command is aborted best-effort; a D-Bus signal already submitted to
    /// the bus cannot be recalled.
    Timeout { ms: u64 },
    /// A host parameter is invalid, such as an invalid capability list or an
    /// unknown/expired notification id.
    InvalidArgs(String),
    /// Reserved fallback for unexpected internal protocol/type failures.
    ///
    /// No normal execution path currently produces this variant; it is kept
    /// for future exceptional cases.
    Protocol(String),
    /// The session has already been stopped.
    Stopped,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(message) => write!(f, "D-Bus error: {message}"),
            Self::NameTaken(name) => write!(f, "bus name is already taken: {name}"),
            Self::Timeout { ms } => write!(f, "command timed out after {ms} ms"),
            Self::InvalidArgs(message) => write!(f, "invalid arguments: {message}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::Stopped => f.write_str("session stopped"),
        }
    }
}

impl std::error::Error for Error {}

impl From<zbus::Error> for Error {
    fn from(error: zbus::Error) -> Self {
        match error {
            zbus::Error::NameTaken => Self::NameTaken(BUS_NAME.to_owned()),
            other => Self::Bus(other.to_string()),
        }
    }
}
