//! `tray` — headless Linux system tray data layer.
//!
//! This crate implements the client side of the StatusNotifierItem (SNI)
//! protocol plus a DBusMenu client, with an embedded fallback watcher for
//! sessions that lack one. It is UI-agnostic: no GTK/GLib dependency, raw
//! data + event delivery only.
//!
//! Entry points: [`Tray::connect`] / [`Tray::builder`]. See
//! `docs/SPEC.md` for the authoritative specification.
//!
//! ```no_run
//! use std::time::Duration;
//! use tray::{Tray, TrayConfig, TrayEvent};
//!
//! let config = TrayConfig::default();
//! let (handle, rx) = Tray::connect(config).unwrap();
//! for ev in rx {
//!     match ev {
//!         TrayEvent::Added(item) => println!("item: {:?}", item.id),
//!         TrayEvent::Removed(id) => println!("gone: {id}"),
//!         _ => {}
//!     }
//! }
//! ```

mod config;
mod error;
mod event;
mod handle;
mod menu;
mod model;
mod runtime;
mod sni;
mod watcher;

#[cfg(feature = "testing")]
pub mod testing;

pub use config::{FallbackPolicy, TrayConfig};
pub use error::TrayError;
pub use event::{ItemField, TrayEvent};
pub use handle::{Tray, TrayBuilder, TrayHandle};
pub use menu::{MenuItem, MenuItemKind, MenuSnapshot};
pub use model::{
    IconSource, Orientation, Pixmap, ToolTip, TrayCategory, TrayItem, TrayItemId, TrayStatus,
};

/// Watcher presence state (delivered via
/// [`TrayEvent::WatcherChanged`](crate::TrayEvent::WatcherChanged)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherState {
    /// An external watcher is online and we registered with it.
    ExternalUp,
    /// No watcher on the bus and we are not providing one.
    ExternalDown,
    /// This library is providing the watcher service.
    FallbackActive,
}

/// Which interaction a [`TrayEvent::InteractionResult`](crate::TrayEvent::InteractionResult)
/// receipt belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionOp {
    /// Left-click activation.
    Activate,
    /// Secondary (right-click) activation.
    SecondaryActivate,
    /// Scroll.
    Scroll,
    /// Menu entry click.
    MenuActivate,
    /// Menu about-to-show.
    MenuAboutToShow,
}

pub(crate) use sni::MENU_IFACE;
