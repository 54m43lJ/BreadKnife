use gtk::prelude::*;
use gtk::{gdk, Application, ApplicationWindow, CssProvider};

const APP_ID: &str = "com.breadknife.shell";

fn main() {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        let display = gdk::Display::default().unwrap();
        let monitor = display.monitors().item(0).unwrap().downcast::<gdk::Monitor>().unwrap();
        let geom = monitor.geometry();

        let window = ApplicationWindow::builder()
            .application(app)
            .default_width(geom.width())
            .default_height(30)
            .decorated(false)
            .resizable(false)
            .build();

        let css = CssProvider::new();
        css.load_from_string("window { background-color: rgba(0, 0, 0, 0.85); }");
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        window.connect_destroy(|_| {
            std::process::exit(0);
        });

        window.present();
    });

    app.run();
}
