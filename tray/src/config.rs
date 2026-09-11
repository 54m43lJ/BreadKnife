//! Library configuration.

/// Fallback watcher strategy.
///
/// The SNI ecosystem requires an `org.kde.StatusNotifierWatcher` service on
/// the session bus. Bare sessions (e.g. plain Hyprland) usually have none;
/// with [`FallbackPolicy::Auto`] this library steps in and provides one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FallbackPolicy {
    /// Act as watcher when no external watcher is present (default).
    /// Yield automatically when an external watcher appears.
    #[default]
    Auto,
    /// Never provide a watcher; act purely as host/item client.
    Never,
}

/// Runtime configuration of a [`Tray`](crate::Tray) instance.
#[derive(Debug, Clone)]
pub struct TrayConfig {
    /// Service discovery and watcher respawn grace, in milliseconds.
    /// A dead external watcher is replaced after `2 * timeout_ms`.
    pub timeout_ms: u64,
    /// Property change coalescing window in milliseconds (event debounce).
    pub debounce_ms: u64,
    /// Maximum depth of menu layout fetches.
    pub menu_max_depth: u8,
    /// Fallback watcher strategy.
    pub fallback: FallbackPolicy,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 2000,
            debounce_ms: 50,
            menu_max_depth: 8,
            fallback: FallbackPolicy::Auto,
        }
    }
}
