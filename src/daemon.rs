use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, Application};

use crate::app::{AppModule, PerMonitor};

struct Inner {
    /// monitor → app_id → PerMonitor
    instances: RefCell<HashMap<gdk::Monitor, HashMap<String, PerMonitor>>>,
    loaded: RefCell<Vec<String>>,
    hold: RefCell<Option<gtk::gio::ApplicationHoldGuard>>,
}

#[derive(Clone)]
pub struct Daemon {
    application: Application,
    modules: Rc<HashMap<String, Box<dyn AppModule>>>,
    inner: Rc<Inner>,
}

impl Daemon {
    pub fn new(
        application: &Application,
        modules: Vec<Box<dyn AppModule>>,
        socket_path: &str,
        hold: gtk::gio::ApplicationHoldGuard,
    ) -> Self {
        // Load CSS from all modules
        let display = gdk::Display::default().unwrap();
        for m in &modules {
            if let Some(css) = m.css() {
                let provider = gtk::CssProvider::new();
                provider.load_from_string(css);
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        }

        let module_map: HashMap<String, Box<dyn AppModule>> =
            modules.into_iter().map(|m| (m.id().to_string(), m)).collect();

        let daemon = Daemon {
            application: application.clone(),
            modules: Rc::new(module_map),
            inner: Rc::new(Inner {
                instances: RefCell::new(HashMap::new()),
                loaded: RefCell::new(Vec::new()),
                hold: RefCell::new(Some(hold)),
            }),
        };

        daemon.setup_hotplug();
        daemon.start_socket(socket_path);

        daemon
    }

    // ── app lifecycle ──────────────────────────────────────────

    pub fn load_app(&self, id: &str) {
        if self.inner.loaded.borrow().contains(&id.to_string()) {
            eprintln!("[daemon] app '{}' already loaded", id);
            return;
        }

        let module = match self.modules.get(id) {
            Some(m) => m,
            None => {
                eprintln!("[daemon] unknown app: {}", id);
                return;
            }
        };

        let display = gdk::Display::default().unwrap();
        let monitors = display.monitors();
        let mut instances = self.inner.instances.borrow_mut();

        for i in 0..monitors.n_items() {
            let monitor = monitors
                .item(i)
                .unwrap()
                .downcast::<gdk::Monitor>()
                .unwrap();
            let pm = module.create(&self.application, &monitor);
            pm.window.present();

            instances.entry(monitor).or_default().insert(id.to_string(), pm);
        }

        self.inner.loaded.borrow_mut().push(id.to_string());
        eprintln!(
            "[daemon] loaded app '{}' on {} monitors",
            id,
            monitors.n_items()
        );
    }

    pub fn unload_app(&self, id: &str) {
        let mut instances = self.inner.instances.borrow_mut();

        let mut cleaned = 0usize;
        for (_monitor, apps) in instances.iter_mut() {
            if let Some(pm) = apps.remove(id) {
                for handler in pm.signal_handlers {
                    // signal handlers are disconnected when the object is destroyed,
                    // but explicit disconnect is cleaner
                    drop(handler);
                }
                pm.window.close();
                cleaned += 1;
            }
        }

        self.inner.loaded.borrow_mut().retain(|a| a != id);
        eprintln!(
            "[daemon] unloaded app '{}' ({} instances cleaned)",
            id, cleaned
        );
    }

    // ── monitor hotplug ────────────────────────────────────────

    fn setup_hotplug(&self) {
        let display = gdk::Display::default().unwrap();
        let monitors = display.monitors();

        let daemon = self.clone();
        monitors.connect_items_changed(move |list, _position, _removed, _added| {
            let current: Vec<gdk::Monitor> = (0..list.n_items())
                .filter_map(|i| list.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
                .collect();

            let mut instances = daemon.inner.instances.borrow_mut();
            let loaded = daemon.inner.loaded.borrow();

            // remove monitors that disappeared
            let removed: Vec<gdk::Monitor> = instances
                .keys()
                .filter(|m| !current.contains(m))
                .cloned()
                .collect();
            for monitor in &removed {
                if let Some(apps) = instances.remove(monitor) {
                    eprintln!(
                        "[daemon] monitor removed: {:?} ({} app instances)",
                        monitor.connector(),
                        apps.len()
                    );
                    for (_app_id, pm) in apps {
                        for handler in pm.signal_handlers {
                            monitor.disconnect(handler);
                        }
                        pm.window.close();
                    }
                }
            }

            // add new monitors
            for monitor in &current {
                if !instances.contains_key(monitor) {
                    let mut per_app = HashMap::new();
                    for app_id in loaded.iter() {
                        if let Some(module) = daemon.modules.get(app_id) {
                            per_app.insert(
                                app_id.clone(),
                                module.create(&daemon.application, monitor),
                            );
                        }
                    }
                    for pm in per_app.values() {
                        pm.window.present();
                    }
                    instances.insert(monitor.clone(), per_app);
                    eprintln!("[daemon] new monitor: {:?}", monitor.connector());
                }
            }

            eprintln!(
                "[daemon] monitors: {} total, {} tracked",
                current.len(),
                instances.len()
            );
        });
    }

    // ── unix socket ────────────────────────────────────────────

    fn handle_command(&self, cmd: &str) {
        let cmd = cmd.trim();
        match cmd {
            "list" => {
                let loaded = self.inner.loaded.borrow();
                eprintln!("[daemon] loaded apps: {:?}", *loaded);
            }
            _ if cmd.starts_with("start ") => {
                let name = &cmd[6..].trim();
                self.load_app(name);
            }
            _ if cmd.starts_with("stop ") => {
                let name = &cmd[5..].trim();
                self.unload_app(name);
            }
            "exit" => {
                eprintln!("[daemon] shutting down...");
                let app_ids: Vec<String> = self.inner.loaded.borrow().clone();
                for id in &app_ids {
                    self.unload_app(id);
                }
                self.inner.hold.borrow_mut().take();
                self.application.quit();
            }
            _ => {
                eprintln!("[daemon] unknown command: {}", cmd);
            }
        }
    }

    fn start_socket(&self, socket_path: &str) {
        let path = socket_path.to_owned();
        let _ = std::fs::remove_file(&path);

        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[daemon] failed to bind unix socket {}: {}", path, e);
                return;
            }
        };
        listener.set_nonblocking(true).ok();

        let fd = listener.as_raw_fd();
        let listener = Rc::new(RefCell::new(listener));
        let daemon = self.clone();

        glib::unix_fd_add_local(fd, glib::IOCondition::IN, move |_fd, _cond| {
            loop {
                match listener.borrow().accept() {
                    Ok((stream, _addr)) => {
                        stream.set_nonblocking(true).ok();
                        let mut reader = BufReader::new(stream);
                        let mut line = String::new();
                        loop {
                            match reader.read_line(&mut line) {
                                Ok(0) => break, // EOF
                                Ok(_) => {
                                    daemon.handle_command(line.trim());
                                    line.clear();
                                }
                                Err(ref e)
                                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                                {
                                    break;
                                }
                                Err(e) => {
                                    eprintln!("[daemon] socket read error: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        eprintln!("[daemon] socket accept error: {}", e);
                        break;
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        eprintln!("[daemon] unix socket listening on {}", path);
    }
}
