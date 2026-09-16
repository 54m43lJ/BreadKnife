//! demo-tray (SPEC §10.5) — a simple GTK4 window that lists every tray
//! icon on the bus and lets you exercise the full interaction surface:
//!
//!   left-click a card  -> Activate (receipt arrives in the event log)
//!   right-click a card -> menu snapshot popup (AboutToShow); clicking an
//!                         entry fires MenuActivate
//!   scroll a card      -> Scroll
//!
//! Built-in test rig (from `tray::testing`): spawn a demo item and mutate
//! it (status / icon / pixmap / menu) to watch `Changed` / `MenuChanged`,
//! and take/release the watcher role to watch `WatcherChanged`. All
//! `TrayEvent`s are printed in the log at the bottom.
//!
//! ```bash
//! cargo run -p demo-tray
//! ```

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::{
    glib, prelude::*, Align, Application, ApplicationWindow, Button, EventControllerScroll,
    EventControllerScrollFlags, Frame, GestureClick, Image, Label, Orientation, Popover,
    PolicyType, PositionType, ScrolledWindow, Separator, TextView, Widget,
};
use tray::{
    testing::{spawn_demo_item, DemoItem, DemoWatcher},
    IconSource, MenuItem, MenuItemKind, Orientation as TrayOrientation, Tray, TrayConfig,
    TrayEvent, TrayItem, TrayItemId,
};

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("org.breadknife.demo_tray")
        .build();
    app.connect_activate(|app| {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("demo-tray — tray library demo host")
            .default_width(980)
            .default_height(660)
            .build();
        let ui = Ui::new(&window);
        ui.wire_callbacks();
        window.present();
    });
    app.run()
}

// ── UI ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Ui {
    handle: tray::TrayHandle,
    watcher_label: Label,
    watcher_btn: Button,
    spawn_btn: Button,
    demo_controls: gtk4::Box,
    cards: gtk4::Box,
    log: TextView,
}

impl Ui {
    /// Build the whole UI and start the tray runtime. Event delivery into
    /// the GTK main loop goes through a `MainContext` channel (the library
    /// callback contract forbids touching UI on its event thread).
    fn new(window: &ApplicationWindow) -> Rc<Self> {
        let root = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        window.set_child(Some(&root));

        // Control row.
        let controls = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .build();
        let watcher_label = Label::builder().label("watcher: …").halign(Align::Start).build();
        let watcher_btn = Button::with_label("take watcher");
        let spawn_btn = Button::with_label("spawn demo item");
        let demo_controls = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(4)
            .visible(false)
            .build();
        for label in [
            "status",
            "icon",
            "pixmap",
            "menu add",
            "menu toggle",
            "menu remove",
            "mount menu",
            "remove item",
        ] {
            demo_controls.append(&Button::with_label(label));
        }
        controls.append(&watcher_label);
        controls.append(&watcher_btn);
        controls.append(&spawn_btn);
        controls.append(&demo_controls);
        root.append(&controls);

        let hint = Label::builder()
            .label("left-click = Activate · right-click = menu popup · scroll = Scroll — all events appear below")
            .halign(Align::Start)
            .css_classes(["dim-label"])
            .build();
        root.append(&hint);

        // Item cards.
        let cards = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(6)
            .build();
        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .child(&cards)
            .build();
        root.append(&scroll);

        // Event log.
        let log = TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .height_request(150)
            .monospace(true)
            .build();
        let log_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .child(&log)
            .build();
        root.append(&Frame::builder().label("events").child(&log_scroll).build());

        // Tray runtime + event forwarding.
        let (handle, rx) = Tray::connect(TrayConfig::default()).expect("tray connect");
        let (tx, ev_rx) = async_channel::unbounded::<TrayEvent>();
        std::thread::spawn(move || {
            for ev in rx {
                if tx.send_blocking(ev).is_err() {
                    break;
                }
            }
        });
        let ui = Rc::new(Self {
            handle,
            watcher_label,
            watcher_btn,
            spawn_btn,
            demo_controls,
            cards,
            log,
        });
        {
            let ui = Rc::clone(&ui);
            glib::spawn_future_local(async move {
                while let Ok(ev) = ev_rx.recv().await {
                    ui.on_event(&ev);
                }
            });
        }
        ui
    }

    /// Connect interactive handlers (needs the `Rc` identity).
    fn wire_callbacks(self: &Rc<Self>) {
        {
            let ui = Rc::clone(self);
            self.watcher_btn.connect_clicked(move |_| toggle_watcher(&ui));
        }
        {
            let ui = Rc::clone(self);
            self.spawn_btn.connect_clicked(move |_| spawn_demo(&ui));
        }
        let actions: [(usize, DemoAction); 8] = [
            (0, DemoAction::Status),
            (1, DemoAction::Icon),
            (2, DemoAction::Pixmap),
            (3, DemoAction::MenuAdd),
            (4, DemoAction::MenuToggle),
            (5, DemoAction::MenuRemove),
            (6, DemoAction::MenuMount),
            (7, DemoAction::Remove),
        ];
        for (i, action) in actions {
            let btn = self
                .demo_controls
                .observe_children()
                .item(i as u32)
                .and_downcast::<Button>()
                .expect("demo action button");
            let ui = Rc::clone(self);
            btn.connect_clicked(move |_| run_demo_action(&ui, action));
        }
    }

    fn on_event(self: &Rc<Self>, ev: &TrayEvent) {
        match ev {
            TrayEvent::Added(item) => self.logline(format!("Added          {}", describe(item))),
            TrayEvent::Removed(id) => self.logline(format!("Removed        {id}")),
            TrayEvent::Changed { id, changed, item } => self.logline(format!(
                "Changed        {id} {changed:?} -> {}",
                describe(item)
            )),
            TrayEvent::MenuChanged(id) => self.logline(format!("MenuChanged    {id}")),
            TrayEvent::WatcherChanged(state) => {
                self.logline(format!("WatcherChanged {state:?}"));
                self.watcher_label.set_text(&format!("watcher: {state:?}"));
            }
            TrayEvent::InteractionResult { id, op, result } => self.logline(format!(
                "Receipt        {id} {op:?} -> {}",
                match result {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("err: {e}"),
                }
            )),
            TrayEvent::Fatal(e) => self.logline(format!("FATAL          {e}")),
        }
        self.refresh();
    }

    fn refresh(self: &Rc<Self>) {
        let owned = DEMO_WATCHER
            .with(|c| c.borrow().as_ref().map(DemoWatcher::is_owner))
            .unwrap_or(false);
        self.watcher_btn
            .set_label(if owned { "release watcher" } else { "take watcher" });

        while let Some(c) = self.cards.first_child() {
            self.cards.remove(&c);
        }
        let items = self.handle.items();
        if items.is_empty() {
            self.cards.append(
                &Label::builder()
                    .label("no tray items — spawn the demo item, or start a real tray app")
                    .halign(Align::Start)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        for item in &items {
            self.cards.append(&self.build_card(item));
        }
    }

    fn build_card(self: &Rc<Self>, item: &TrayItem) -> Widget {
        let frame = Frame::new(None);
        let card = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(4)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(10)
            .width_request(130)
            .build();
        frame.set_child(Some(&card));

        card.append(&build_icon(&item.icon));

        card.append(
            &Label::builder()
                .label(item.title.as_deref().unwrap_or("—"))
                .justify(gtk4::Justification::Center)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .width_chars(15)
                .max_width_chars(15)
                .build(),
        );
        card.append(
            &Label::builder()
                .label(format!("{:?}", item.status))
                .css_classes(["dim-label"])
                .build(),
        );
        if let Some(tt) = &item.tooltip {
            frame.set_tooltip_text(Some(&format!("{} — {}", tt.title, tt.description)));
        }

        // Clicks: left = Activate, right = menu popup.
        let click = GestureClick::new();
        {
            let handle = self.handle.clone();
            let id = item.id.clone();
            let ui = Rc::clone(self);
            let frame = frame.clone();
            click.connect_pressed(move |gesture, _n, x, y| match gesture.current_button() {
                1 => {
                    let _ = handle.activate(&id, x as i32, y as i32);
                }
                3 => popup_menu(&ui, &frame, &id, x as i32, y as i32),
                _ => {}
            });
        }
        frame.add_controller(click);

        // Scroll.
        let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        {
            let handle = self.handle.clone();
            let id = item.id.clone();
            scroll.connect_scroll(move |_, dx, dy| {
                let orient = if dy.abs() >= dx.abs() {
                    TrayOrientation::Vertical
                } else {
                    TrayOrientation::Horizontal
                };
                let delta = if dy != 0.0 { dy } else { dx };
                let _ = handle.scroll(&id, delta as i32, orient);
                glib::Propagation::Proceed
            });
        }
        frame.add_controller(scroll);
        frame.upcast()
    }

    fn logline(&self, msg: impl std::fmt::Display) {
        let buf = self.log.buffer();
        let mut end = buf.end_iter();
        buf.insert(&mut end, &format!("[{}] {}\n", timestamp(), msg));
        if let Some(adj) = self.log.vadjustment() {
            adj.set_value(adj.upper());
        }
    }
}

fn popup_menu(ui: &Rc<Ui>, parent: &impl IsA<Widget>, id: &TrayItemId, x: i32, y: i32) {
    let _ = ui.handle.menu_about_to_show(id, 0); // notify, best effort
    let Some(snapshot) = ui.handle.menu(id) else {
        ui.logline(format!("AboutToShow    {id} has no menu"));
        return;
    };
    ui.logline(format!(
        "AboutToShow    {id} -> revision {} ({} top-level entries)",
        snapshot.revision,
        snapshot.root.len()
    ));

    let pop = Popover::builder()
        .autohide(true)
        .position(PositionType::Bottom)
        .build();
    let list = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();
    for entry in &snapshot.root {
        append_menu_entry(&list, entry, 0, ui, id, &pop);
    }
    if snapshot.root.is_empty() {
        list.append(&Label::new(Some("(empty menu)")));
    }
    pop.set_child(Some(&list));
    pop.set_parent(parent);
    pop.set_offset(x, y);
    pop.popup();
}

fn append_menu_entry(
    list: &gtk4::Box,
    entry: &MenuItem,
    depth: usize,
    ui: &Rc<Ui>,
    id: &TrayItemId,
    pop: &Popover,
) {
    if !entry.visible {
        return;
    }
    if entry.kind == MenuItemKind::Separator {
        list.append(&Separator::new(Orientation::Horizontal));
        return;
    }
    let mut label = String::new();
    match entry.toggled {
        Some(true) => label.push_str("✓ "),
        Some(false) => label.push_str("    "),
        None => {}
    }
    label.push_str(&"    ".repeat(depth));
    label.push_str(&entry.label);
    if entry.kind == MenuItemKind::Submenu {
        label.push_str("  ▸");
    }
    let btn = Button::builder().label(label).css_classes(["flat"]).build();
    btn.set_sensitive(entry.enabled);
    if let Some(icon) = &entry.icon {
        btn.set_tooltip_text(Some(icon));
    }
    let handle = ui.handle.clone();
    let entry_id = id.clone();
    let menu_item = entry.id;
    let pop_ref = pop.clone();
    btn.connect_clicked(move |_| {
        let _ = handle.menu_activate(&entry_id, menu_item);
        pop_ref.popdown();
    });
    list.append(&btn);
    for child in &entry.children {
        append_menu_entry(list, child, depth + 1, ui, id, pop);
    }
}

fn build_icon(icon: &IconSource) -> Image {
    let image = match icon {
        IconSource::Name(name) => {
            let known = gtk4::IconTheme::default().has_icon(name);
            let name = if known { name } else { "image-missing" };
            Image::from_icon_name(name)
        }
        IconSource::Pixmap(p) => {
            // SNI pixmap: ARGB32, premultiplied, network byte order
            // (bytes A,R,G,B). GDK wants R8G8B8A8 premultiplied.
            let mut rgba = Vec::with_capacity(p.argb.len());
            for px in p.argb.chunks_exact(4) {
                let (a, r, g, b) = (px[0], px[1], px[2], px[3]);
                rgba.extend_from_slice(&[r, g, b, a]);
            }
            let texture = gtk4::gdk::MemoryTexture::new(
                p.width as i32,
                p.height as i32,
                gtk4::gdk::MemoryFormat::R8g8b8a8Premultiplied,
                &glib::Bytes::from_owned(rgba),
                p.width as usize * 4,
            );
            Image::from_paintable(Some(&texture))
        }
    };
    image.set_pixel_size(32);
    image
}

// ── demo rig (watcher toggle + demo item actions) ───────────────────────

thread_local! {
    static DEMO_WATCHER: std::cell::RefCell<Option<DemoWatcher>> =
        const { std::cell::RefCell::new(None) };
    static DEMO_ITEM: std::cell::RefCell<Option<DemoItem>> =
        const { std::cell::RefCell::new(None) };
    static STATUS_N: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static ICON_N: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static MENU_N: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static LAST_MENU_ID: std::cell::Cell<i32> = const { std::cell::Cell::new(1) };
    static PIXMAP_ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static MENU_MOUNTED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

const ICONS: [&str; 3] = ["audio-volume-high", "network-wireless", "demo"];

fn toggle_watcher(ui: &Rc<Ui>) {
    DEMO_WATCHER.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.is_none() {
            match DemoWatcher::spawn() {
                Ok(w) => *slot = Some(w),
                Err(e) => {
                    ui.logline(format!("watcher spawn failed: {e}"));
                    return;
                }
            }
        }
        let Some(w) = slot.as_ref() else {
            return;
        };
        if w.is_owner() {
            w.release();
            ui.logline("watcher role released (hosts: ExternalDown, respawn after 2×timeout)");
        } else {
            match w.take() {
                Ok(()) => ui.logline("watcher role taken (hosts: ExternalUp)"),
                Err(e) => ui.logline(format!("watcher take failed: {e}")),
            }
        }
    });
    ui.refresh();
}

fn spawn_demo(ui: &Rc<Ui>) {
    let running = DEMO_ITEM.with(|c| c.borrow().is_some());
    if running {
        ui.logline("demo item already running");
        return;
    }
    match spawn_demo_item(Default::default()) {
        Ok(d) => {
            ui.logline(format!("demo item spawned: {}", d.service()));
            DEMO_ITEM.with(|c| *c.borrow_mut() = Some(d));
            ui.demo_controls.set_visible(true);
        }
        Err(e) => ui.logline(format!("demo item spawn failed: {e}")),
    }
    ui.refresh();
}

fn with_demo(ui: &Rc<Ui>, f: impl FnOnce(&DemoItem) -> Result<(), tray::TrayError>) {
    let result = DEMO_ITEM.with(|c| {
        let b = c.borrow();
        match b.as_ref() {
            Some(d) => f(d),
            None => Err(tray::TrayError::Protocol("no demo item".into())),
        }
    });
    if let Err(e) = result {
        ui.logline(format!("demo action failed: {e}"));
    }
    ui.refresh();
}

#[derive(Debug, Clone, Copy)]
enum DemoAction {
    Status,
    Icon,
    Pixmap,
    MenuAdd,
    MenuToggle,
    MenuRemove,
    MenuMount,
    Remove,
}

fn run_demo_action(ui: &Rc<Ui>, action: DemoAction) {
    match action {
        DemoAction::Status => {
            let next = STATUS_N.with(|c| {
                let n = c.get();
                c.set((n + 1) % 3);
                n
            });
            let statuses = ["passive", "needs-attention", "active"];
            with_demo(ui, |d| d.set_status(statuses[next as usize % 3]))
        }
        DemoAction::Icon => {
            let idx = ICON_N.with(|c| {
                let n = c.get();
                c.set((n + 1) % ICONS.len() as u32);
                n as usize % ICONS.len()
            });
            PIXMAP_ON.with(|c| c.set(false));
            with_demo(ui, |d| d.set_icon_name(ICONS[idx]))
        }
        DemoAction::Pixmap => {
            let on = !PIXMAP_ON.with(|c| c.get());
            PIXMAP_ON.with(|c| c.set(on));
            with_demo(ui, |d| d.use_pixmap(on))
        }
        DemoAction::MenuAdd => {
            MENU_N.with(|c| c.set(c.get() + 1));
            with_demo(ui, |d| {
                let id = d.menu_add(&format!("entry {}", MENU_N.with(|c| c.get())))?;
                LAST_MENU_ID.with(|c| c.set(id));
                Ok(())
            })
        }
        DemoAction::MenuToggle => {
            let id = LAST_MENU_ID.with(|c| c.get());
            with_demo(ui, |d| d.menu_toggle(id))
        }
        DemoAction::MenuRemove => {
            let id = LAST_MENU_ID.with(|c| c.get());
            with_demo(ui, |d| d.menu_remove(id))
        }
        DemoAction::MenuMount => {
            let on = !MENU_MOUNTED.with(|c| c.get());
            MENU_MOUNTED.with(|c| c.set(on));
            with_demo(ui, |d| d.set_menu_enabled(on))
        }
        DemoAction::Remove => {
            let item = DEMO_ITEM.with(|c| c.borrow_mut().take());
            match item {
                Some(d) => {
                    if let Err(e) = d.remove() {
                        ui.logline(format!("demo remove failed: {e}"));
                    } else {
                        ui.logline("demo item removed");
                    }
                    ui.demo_controls.set_visible(false);
                    ui.refresh();
                }
                None => ui.logline("no demo item"),
            }
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

fn timestamp() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let s = d.as_secs();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        (s / 3600) % 24,
        (s / 60) % 60,
        s % 60,
        d.subsec_millis()
    )
}

fn describe(item: &TrayItem) -> String {
    let icon = match &item.icon {
        IconSource::Name(n) => format!("name:{n}"),
        IconSource::Pixmap(p) => format!("pixmap:{}x{}", p.width, p.height),
    };
    format!(
        "{:?} title={:?} icon={icon} menu={}",
        item.status, item.title, item.has_menu
    )
}
