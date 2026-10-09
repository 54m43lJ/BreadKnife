use gtk::{gdk, glib::signal::SignalHandlerId, Application, ApplicationWindow};

use crate::hyprland::EventBus;

/// Resources owned by one app instance on one monitor.
pub struct PerMonitor {
    pub window: ApplicationWindow,
    pub signal_handlers: Vec<SignalHandlerId>,
}

/// An application module that can be loaded and unloaded per-monitor.
pub trait AppModule {
    /// Unique short identifier, e.g. "bar".
    fn id(&self) -> &'static str;

    /// Create a window (and any related resources) for a single monitor.
    fn create(
        &self,
        app: &Application,
        monitor: &gdk::Monitor,
        event_bus: &EventBus,
    ) -> PerMonitor;

    /// Optional CSS to load into the global display provider when the app is first loaded.
    fn css(&self) -> Option<&str> {
        None
    }
}

/// Hardcoded registry of all known app modules.
pub fn registry() -> Vec<Box<dyn AppModule>> {
    vec![Box::new(crate::bar::BarApp::new())]
}
