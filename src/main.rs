use gtk::prelude::*;
use gtk::{gdk, Application, ApplicationWindow, CssProvider};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

const APP_ID: &str = "com.breadknife.shell";

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

        let monitors = display.monitors();
        for i in 0..monitors.n_items() {
            let monitor = monitors
                .item(i)
                .unwrap()
                .downcast::<gdk::Monitor>()
                .unwrap();

            let geom = monitor.geometry();

            let window = ApplicationWindow::builder()
                .application(app)
                .default_width(geom.width())
                .default_height(30)
                .decorated(false)
                .resizable(false)
                .build();

            window.init_layer_shell();
            window.set_layer(Layer::Top);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Top, true);
            window.set_monitor(Some(&monitor));
            window.auto_exclusive_zone_enable();

            window.present();
        }
    });

    app.run();
}
