//! Public event model.

use crate::{
    model::{TrayItem, TrayItemId},
    InteractionOp, WatcherState,
};

/// A single field of [`TrayItem`] that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemField {
    /// `Status` property.
    Status,
    /// `Title` property.
    Title,
    /// `Id` property.
    Description,
    /// Primary icon (`IconName`/`IconPixmap`).
    Icon,
    /// Overlay icon.
    OverlayIcon,
    /// Attention icon.
    AttentionIcon,
    /// `ToolTip` property.
    Tooltip,
    /// `WindowId` property.
    WindowId,
    /// `Category` property.
    Category,
    /// `Menu` property presence.
    HasMenu,
}

/// Events delivered to consumers via callbacks and/or channels.
///
/// Ordering: events of one item are strictly ordered; no global order across
/// items is guaranteed.
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// A new item appeared (existing items are reported as `Added` during
    /// startup, indistinguishable from runtime additions).
    Added(TrayItem),
    /// An item disappeared (unregistered, process exited, or lost during
    /// watcher replacement).
    Removed(TrayItemId),
    /// Item properties changed. `changed` lists the fields that actually
    /// differ (debounce-merged union); `item` carries the fresh snapshot.
    Changed {
        /// Item that changed.
        id: TrayItemId,
        /// Fields that changed.
        changed: Vec<ItemField>,
        /// Fresh snapshot of the item.
        item: TrayItem,
    },
    /// The item's menu content changed; call
    /// [`TrayHandle::menu`](crate::TrayHandle::menu) to fetch a new snapshot.
    MenuChanged(TrayItemId),
    /// Watcher state transition (external watcher up/down, fallback active).
    WatcherChanged(WatcherState),
    /// Asynchronous receipt of an interaction call.
    InteractionResult {
        /// Item the call was directed at.
        id: TrayItemId,
        /// Which operation this receipt belongs to.
        op: InteractionOp,
        /// Outcome on the item side.
        result: Result<(), String>,
    },
    /// Unrecoverable internal error (e.g. bus disconnect). This is the last
    /// event ever delivered; the runtime is stopped afterwards.
    Fatal(String),
}
