//! Public data model of tray items.

use std::fmt;

/// Globally unique key of a tray item: bus name + object path.
///
/// An application restarting under a new unique name yields a *different*
/// item id (the old one is reported as [`TrayEvent::Removed`](crate::TrayEvent::Removed),
/// the new one as [`TrayEvent::Added`](crate::TrayEvent::Added)).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrayItemId {
    /// Bus name the item was registered under (well-known or `:1.x` unique).
    pub bus_name: String,
    /// Object path of the `org.kde.StatusNotifierItem` interface.
    pub path: String,
}

impl fmt::Display for TrayItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.bus_name, self.path)
    }
}

impl Default for TrayItemId {
    fn default() -> Self {
        Self {
            bus_name: String::new(),
            path: String::new(),
        }
    }
}

/// SNI item category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCategory {
    /// The item describes the status of a generic application.
    Application,
    /// The item describes the status of communication oriented applications.
    Communications,
    /// The item describes services of the system not related to applications.
    SystemServices,
    /// The item describes the state of hardware devices.
    Hardware,
}

impl Default for TrayCategory {
    fn default() -> Self {
        Self::Application
    }
}

impl TrayCategory {
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "Communications" => Self::Communications,
            "SystemServices" => Self::SystemServices,
            "Hardware" => Self::Hardware,
            _ => Self::Application,
        }
    }
}

/// SNI item status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrayStatus {
    /// The item doesn't convey important information (default).
    #[default]
    Passive,
    /// The item is active and conveys information.
    Active,
    /// The item requires attention (rendered with the attention icon).
    NeedsAttention,
}

impl TrayStatus {
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::NeedsAttention,
            _ => Self::Active,
        }
    }
}

/// Raw icon bitmap delivered by SNI (`a(iiay)`).
///
/// Pixels are ARGB32 in network byte order; `argb.len() == width * height * 4`.
/// This library performs no color space conversion — rendering concerns live
/// in the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// ARGB32 bytes, network byte order, packed rows (`width * 4` stride).
    pub argb: Vec<u8>,
}

/// Icon source of a tray item.
///
/// Resolution rule (fixed by this library, consumers need not re-implement):
/// a non-empty `IconName` always wins (theme lookup is the consumer's job);
/// otherwise the largest pixmap is delivered; otherwise an empty name is
/// returned as a placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    /// Icon name to be resolved against the system icon theme.
    Name(String),
    /// Raw bitmap fallback.
    Pixmap(Pixmap),
}

impl Default for IconSource {
    fn default() -> Self {
        Self::Name(String::new())
    }
}

/// SNI tooltip: icon + title + description.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolTip {
    /// Optional icon of the tooltip.
    pub icon: Option<IconSource>,
    /// Title (bold text in most renderers).
    pub title: String,
    /// Description (regular text in most renderers).
    pub description: String,
}

/// Scroll orientation for [`TrayHandle::scroll`](crate::TrayHandle::scroll).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Horizontal scroll.
    Horizontal,
    /// Vertical scroll.
    Vertical,
}

impl Orientation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

/// A complete snapshot of one tray item.
#[derive(Debug, Clone, Default)]
pub struct TrayItem {
    /// Unique identity of the item.
    pub id: TrayItemId,
    /// Item category.
    pub category: TrayCategory,
    /// Item status (Passive / Active / NeedsAttention).
    pub status: TrayStatus,
    /// Human readable title, if provided.
    pub title: Option<String>,
    /// Human readable item identifier ("Id" property), if provided.
    pub description: Option<String>,
    /// Window id the item belongs to (usually 0).
    pub window_id: i32,
    /// Primary icon (theme name or bitmap, see [`IconSource`]).
    pub icon: IconSource,
    /// Overlay icon, if provided (e.g. emission indicator).
    pub overlay_icon: Option<IconSource>,
    /// Attention icon, used while status is `NeedsAttention`.
    pub attention_icon: Option<IconSource>,
    /// Tooltip data, if provided.
    pub tooltip: Option<ToolTip>,
    /// Whether the item exposes a DBusMenu (`Menu` property points at a
    /// valid menu object). Retrieve it with
    /// [`TrayHandle::menu`](crate::TrayHandle::menu).
    pub has_menu: bool,
}
