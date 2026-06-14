use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib::signal::SignalHandlerId;
use gtk::prelude::*;
use gtk::{gdk, Application, ApplicationWindow, CssProvider};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

const APP_ID: &str = "com.breadknife.shell";
const BAR_HEIGHT: i32 = 30;

struct Bar {
    window: ApplicationWindow,
    _geometry_handler: SignalHandlerId,
}

fn create_bar(app: &Application, monitor: &gdk::Monitor) -> Bar {
    let geom = monitor.geometry();

    let window = ApplicationWindow::builder()
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
        "[breadknife] + bar for monitor: {:?}  {}x{}",
        monitor.connector(),
        geom.width(),
        geom.height()
    );

    let window_clone = window.clone();
    let geometry_handler = monitor.connect_geometry_notify(move |m| {
        let g = m.geometry();
        window_clone.set_default_size(g.width(), BAR_HEIGHT);
        eprintln!(
            "[breadknife] geometry changed for {:?}: {}x{}",
            m.connector(),
            g.width(),
            g.height()
        );
    });

    Bar {
        window,
        _geometry_handler: geometry_handler,
    }
}

fn main() {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        let display = gdk::Display::default().unwrap();

        let css = CssProvider::new();
        css.load_from_string("window { background-color: black; }");
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let bars: Rc<RefCell<HashMap<gdk::Monitor, Bar>>> =
            Rc::new(RefCell::new(HashMap::new()));

        let monitors = display.monitors();

        // initial population
        for i in 0..monitors.n_items() {
            let monitor = monitors
                .item(i)
                .unwrap()
                .downcast::<gdk::Monitor>()
                .unwrap();
            let bar = create_bar(app, &monitor);
            bar.window.present();
            bars.borrow_mut().insert(monitor, bar);
        }

        eprintln!(
            "[breadknife] startup: {} monitors, {} bars",
            monitors.n_items(),
            bars.borrow().len()
        );

        // hotplug
        let bars_hp = bars.clone();
        let app_hp = app.clone();
        monitors.connect_items_changed(move |list, _position, _removed, _added| {
            let mut bars = bars_hp.borrow_mut();

            let current: Vec<gdk::Monitor> = (0..list.n_items())
                .filter_map(|i| list.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
                .collect();

            // remove bars for monitors that disappeared
            let mut removed = Vec::new();
            for (monitor, _bar) in bars.iter() {
                if !current.contains(monitor) {
                    removed.push(monitor.clone());
                }
            }
            for monitor in &removed {
                if let Some(bar) = bars.remove(monitor) {
                    eprintln!(
                        "[breadknife] - bar for monitor: {:?}",
                        monitor.connector()
                    );
                    monitor.disconnect(bar._geometry_handler);
                    bar.window.close();
                }
            }

            // add bars for new monitors
            for monitor in &current {
                if !bars.contains_key(monitor) {
                    let bar = create_bar(&app_hp, monitor);
                    bar.window.present();
                    bars.insert(monitor.clone(), bar);
                }
            }

            eprintln!(
                "[breadknife] monitors: {} total, {} bars",
                current.len(),
                bars.len()
            );
        });
    });

    app.run();
}
