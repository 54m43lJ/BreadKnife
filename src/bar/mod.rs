mod workspace;

use gtk::prelude::*;
use gtk::{gdk, Application};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::app::{AppModule, PerMonitor};
use crate::hyprland::EventBus;

const BAR_HEIGHT: i32 = 30;

pub struct BarApp;

impl AppModule for BarApp {
    fn id(&self) -> &'static str {
        "bar"
    }

    fn css(&self) -> Option<&str> {
        Some(include_str!("style.css"))
    }

    fn create(
        &self,
        app: &Application,
        monitor: &gdk::Monitor,
        event_bus: &EventBus,
    ) -> PerMonitor {
        let geom = monitor.geometry();

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .default_width(geom.width())
            .default_height(BAR_HEIGHT)
            .decorated(false)
            .resizable(false)
            .build();

        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_anchor(Edge::Top, true);
        window.set_monitor(Some(monitor));
        window.auto_exclusive_zone_enable();

        eprintln!(
            "[bar] + {:?}  {}x{}",
            monitor.connector(),
            geom.width(),
            geom.height()
        );

        // ── workspace widget ─────────────────────────────────────
        let ws = workspace::WorkspaceWidget::new(monitor, event_bus);

        let layout = gtk::CenterBox::new();
        layout.set_center_widget(Some(&ws.container));
        window.set_child(Some(&layout));

        // ── geometry tracking ────────────────────────────────────
        let window_clone = window.clone();
        let geometry_handler = monitor.connect_geometry_notify(move |m| {
            let g = m.geometry();
            window_clone.set_default_size(g.width(), BAR_HEIGHT);
            eprintln!(
                "[bar] geometry changed for {:?}: {}x{}",
                m.connector(),
                g.width(),
                g.height()
            );
        });

        PerMonitor {
            window,
            signal_handlers: vec![geometry_handler],
        }
    }
}
