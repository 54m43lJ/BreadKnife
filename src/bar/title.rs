use gtk::prelude::*;

use crate::hyprland::{self, EventBus, Subscription};

#[allow(unused)]
pub struct TitleWidget {
    pub container: gtk::Box,
    label: gtk::Label,
    _subs: Vec<Subscription>,
}

impl TitleWidget {
    pub fn new(event_bus: &EventBus) -> Self {
        let label = gtk::Label::new(None);
        label.add_css_class("title-label");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        // init
        if let Some(title) = hyprland::get_active_window_title() {
            label.set_text(&title);
        }

        // activewindow: WINDOWCLASS,WINDOWTITLE
        // windowtitlev2: WINDOWADDRESS,WINDOWTITLE
        let label_weak = label.clone();
        let update = move |data: &str| {
            let title = data.split(',').nth(1).unwrap_or("");
            label_weak.set_text(title);
        };

        let sub1 = event_bus.subscribe("activewindow", update.clone());
        let sub2 = event_bus.subscribe("windowtitlev2", update);

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.append(&label);

        TitleWidget {
            container,
            label,
            _subs: vec![sub1, sub2],
        }
    }
}
