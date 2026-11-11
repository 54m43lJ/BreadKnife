//! Shared data types exposed to the host.
//!
//! Definitions and field semantics are documented in `bnote/USER.html §4`.

use std::fmt;

/// A notification delivered to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Server-assigned identifier. Always non-zero.
    pub id: u32,
    pub app_name: String,
    pub icon: NoticeIcon,
    pub summary: String,
    pub body: String,
    pub actions: Vec<Action>,
    /// `action-icons` hint: render action keys as icon names.
    pub action_icons: bool,
    pub urgency: Urgency,
    /// `None` means the host chooses the timeout, `Some(0)` means the
    /// notification never expires, and any other value is a duration in
    /// milliseconds.
    pub expire_timeout_ms: Option<u32>,
    /// `transient` hint: bypass host persistence.
    pub transient: bool,
    /// `resident` hint: keep the notification after an action fires.
    pub resident: bool,
    /// `sound-file` hint: notification sound file path.
    pub sound_file: Option<String>,
    /// `sound-name` hint: notification sound theme name.
    pub sound_name: Option<String>,
    /// `suppress-sound` hint: suppress notification sound.
    pub suppress_sound: bool,
}

/// An event delivered asynchronously to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeEvent {
    /// A new notification, or a replacement carrying the same [`Notice::id`].
    Notify { notice: Notice },
    /// The client called `CloseNotification`.  The library has already sent
    /// `NotificationClosed`; the host only has to remove the UI.
    Close { id: u32 },
    /// The session bus disconnected or the service stopped unexpectedly.
    ///
    /// The library closes the event channel after this event; the next
    /// [`crate::NoticeReceiver::recv`] returns `None`.
    Fatal { message: String },
}

/// Reason encoded in the `NotificationClosed` signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// `1`: the notification expired.
    Expired,
    /// `2`: the user dismissed the notification.
    Dismissed,
    /// `3`: the client called `CloseNotification`.
    CloseRequest,
}

impl CloseReason {
    pub(crate) fn code(self) -> u32 {
        match self {
            Self::Expired => 1,
            Self::Dismissed => 2,
            Self::CloseRequest => 3,
        }
    }
}

/// Capability names returned by `GetCapabilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub entries: Vec<String>,
}

impl Capabilities {
    /// Build a capability list from any string-like iterator.
    pub fn new<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            entries: entries.into_iter().map(Into::into).collect(),
        }
    }
}

/// Values returned by `GetServerInformation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub spec_version: String,
}

/// Configuration used to assemble a [`crate::Session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub capabilities: Capabilities,
    pub server_info: ServerInfo,
    /// Upper bound for each synchronous [`crate::SessionHandle`] command.
    pub command_timeout_ms: u64,
}

/// Normalized urgency of a notification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl Urgency {
    pub(crate) fn from_byte(value: u8) -> Self {
        match value {
            0 => Self::Low,
            1 => Self::Normal,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

/// One actionable entry of a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
}

/// Normalized notification icon.
#[derive(Clone, PartialEq, Eq)]
pub enum NoticeIcon {
    /// The client did not provide an icon.
    None,
    /// A theme icon name or a `file://` URI, preserved as sent by the client.
    Name(String),
    /// Tightly packed RGBA8 pixels.
    Pixels {
        width: i32,
        height: i32,
        rgba: Vec<u8>,
    },
}

impl fmt::Debug for NoticeIcon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Name(name) => f.debug_tuple("Name").field(name).finish(),
            Self::Pixels {
                width,
                height,
                rgba,
            } => f
                .debug_struct("Pixels")
                .field("width", width)
                .field("height", height)
                .field("rgba", &format_args!("<{} bytes>", rgba.len()))
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_reason_codes_match_the_specification() {
        assert_eq!(CloseReason::Expired.code(), 1);
        assert_eq!(CloseReason::Dismissed.code(), 2);
        assert_eq!(CloseReason::CloseRequest.code(), 3);
    }

    #[test]
    fn urgency_defaults_to_normal_and_unknown_values_fall_back() {
        assert_eq!(Urgency::default(), Urgency::Normal);
        assert_eq!(Urgency::from_byte(0), Urgency::Low);
        assert_eq!(Urgency::from_byte(1), Urgency::Normal);
        assert_eq!(Urgency::from_byte(2), Urgency::Critical);
        assert_eq!(Urgency::from_byte(9), Urgency::Normal);
    }

    #[test]
    fn capabilities_accept_string_iterators() {
        let caps = Capabilities::new(["actions", "body"]);
        assert_eq!(caps.entries, ["actions", "body"]);
    }

    #[test]
    fn pixel_debug_does_not_dump_the_buffer() {
        let icon = NoticeIcon::Pixels {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        };
        assert_eq!(
            format!("{icon:?}"),
            "Pixels { width: 1, height: 1, rgba: <4 bytes> }"
        );
    }
}
