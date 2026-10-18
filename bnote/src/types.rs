//! 共享数据模型：宿主可见的通知事件与归一后的通知数据。
//!
//! 契约见 `bnote/ARCHITECTURE.html §4.2–4.3`。

use std::fmt;

/// 宿主收到的通知事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeEvent {
    /// 新通知，或同 id 的替换通知。
    Notify { notice: Notice },
    /// 客户端经 `CloseNotification` 请求关闭指定通知。
    ///
    /// 库按规范回发 `NotificationClosed`（原因 [`CloseReason::CloseRequest`]）；
    /// 宿主只需从自己的展示与缓存中移除该条。
    Close { id: u32 },
}

/// 关闭原因，对应规范 §9.2.1 的 reason 数值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// 1：通知超时。
    Expired,
    /// 2：用户关闭。
    Dismissed,
    /// 3：客户端调用 `CloseNotification`。
    CloseRequest,
}

impl CloseReason {
    /// 规范中的 UINT32 reason 值。
    pub fn code(self) -> u32 {
        match self {
            Self::Expired => 1,
            Self::Dismissed => 2,
            Self::CloseRequest => 3,
        }
    }
}

impl fmt::Display for CloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Expired => "expired",
            Self::Dismissed => "dismissed",
            Self::CloseRequest => "request",
        })
    }
}

/// `GetCapabilities` 应答中的能力清单。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub entries: Vec<String>,
}

impl Capabilities {
    /// 由任意字符串迭代器构造能力清单。
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

/// `GetServerInformation` 应答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub spec_version: String,
}

/// 通知服务会话在总线上的可用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// 已持有 `org.freedesktop.Notifications`，通知可用。
    Available,
    /// 总线名被其他服务持有；当前排队，等待其退出后接管。
    Queued,
}

/// 归一后的通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: u32,
    pub app_name: String,
    pub icon: NoticeIcon,
    pub summary: String,
    pub body: String,
    pub actions: Vec<Action>,
    pub urgency: Urgency,
    /// 请求的自动关闭时间。
    ///
    /// `None` 表示客户端未指定（规范 `-1`），由宿主决定；
    /// `Some(0)` 表示常驻（规范 `0`）；`Some(ms)` 为毫秒数。
    pub expire_timeout_ms: Option<u32>,
}

/// 通知紧急度，对应 hint `urgency`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl fmt::Display for Urgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        })
    }
}

/// 通知携带的动作：键用于 `ActionInvoked` 回发，标签用于展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
}

/// 归一后的通知图标。
#[derive(Clone, PartialEq, Eq)]
pub enum NoticeIcon {
    /// 客户未提供图标。
    None,
    /// 主题图标名或文件路径（`image-path` 系列 hint 与 `app_icon` 的原文）。
    Name(String),
    /// 已归一为紧密排列的 RGBA8 位图。
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
    fn close_reason_codes_match_spec() {
        assert_eq!(CloseReason::Expired.code(), 1);
        assert_eq!(CloseReason::Dismissed.code(), 2);
        assert_eq!(CloseReason::CloseRequest.code(), 3);
    }

    #[test]
    fn capabilities_accept_string_slices() {
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
