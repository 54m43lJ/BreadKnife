mod title;
mod workspace;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, Application};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::app::{AppModule, PerMonitor};
use crate::hyprland::EventBus;

const BAR_HEIGHT: i32 = 30;

pub struct BarApp {
    workspaces: Rc<RefCell<HashMap<String, workspace::WorkspaceWidget>>>,
    titles: Rc<RefCell<HashMap<String, title::TitleWidget>>>,
}

impl BarApp {
    pub fn new() -> Self {
        BarApp {
            workspaces: Rc::new(RefCell::new(HashMap::new())),
            titles: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

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
        let connector = monitor
            .connector()
            .unwrap_or_default()
            .to_string();

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
        let tw = title::TitleWidget::new(event_bus);

        self.workspaces.borrow_mut().insert(connector.clone(), ws);
        self.titles.borrow_mut().insert(connector.clone(), tw);

        // clean up when window destroyed
        {
            let workspaces = self.workspaces.clone();
            let titles = self.titles.clone();
            let conn = connector.clone();
            window.connect_destroy(move |_| {
                workspaces.borrow_mut().remove(&conn);
                titles.borrow_mut().remove(&conn);
            });
        }

        // reference from maps
        let workspaces = self.workspaces.borrow();
        let titles = self.titles.borrow();
        let ws = &workspaces[&connector];
        let tw = &titles[&connector];

        let layout = gtk::CenterBox::new();
        layout.set_start_widget(Some(&ws.container));
        layout.set_center_widget(Some(&tw.container));
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
