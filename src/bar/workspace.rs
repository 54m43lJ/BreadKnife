use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::gdk;

use crate::hyprland::{self, EventBus, Subscription};

pub struct WorkspaceWidget {
    pub container: gtk::Box,
    labels: Rc<RefCell<HashMap<String, gtk::Label>>>,
    active: Rc<RefCell<Option<String>>>,
    _subs: Vec<Subscription>,
}

impl WorkspaceWidget {
    pub fn new(monitor: &gdk::Monitor, event_bus: &EventBus) -> Self {
        let connector = monitor
            .connector()
            .unwrap_or_default()
            .to_string();

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 2);

        let labels = Rc::new(RefCell::new(build_labels(&container, &connector)));
        let active = Rc::new(RefCell::new(
            hyprland::get_active_workspace().map(|w| w.name),
        ));

        // initial highlight
        if let Some(ref name) = *active.borrow() {
            if let Some(label) = labels.borrow().get(name) {
                label.add_css_class("workspace-active");
            }
        }

        let mut subs = Vec::new();

        // workspacev2: ID,NAME
        {
            let labels = labels.clone();
            let active = active.clone();
            subs.push(event_bus.subscribe("workspacev2", move |data| {
                let id = data.split(',').next().unwrap_or("");
                println!("[workspace] workspace changed to {}", id);
                update_active(&labels, &active, id);
            }));
        }

        // focusedmonv2: MONNAME,WORKSPACEID
        {
            let labels = labels.clone();
            let active = active.clone();
            let _connector = connector.clone();
            subs.push(event_bus.subscribe("focusedmonv2", move |data| {
                let mut parts = data.split(',');
                let _mon = parts.next().unwrap_or("");
                let id = parts.next().unwrap_or("");
                if _mon == _connector {
                    update_active(&labels, &active, id);
                }
            }));
        }

        // monitoraddedv2 / monitorremovedv2: rebind
        {
            let labels = labels.clone();
            let active = active.clone();
            let container = container.clone();
            let connector = connector.clone();
            let rebind = move || {
                // clear
                while let Some(child) = container.first_child() {
                    container.remove(&child);
                }
                *labels.borrow_mut() = build_labels(&container, &connector);
                // re-check highlight
                if let Some(ref name) = *active.borrow() {
                    if let Some(label) = labels.borrow().get(name) {
                        label.add_css_class("workspace-active");
                    }
                }
            };
            subs.push(event_bus.subscribe("monitoraddedv2", {
                let rebind = rebind.clone();
                move |_| rebind()
            }));
            subs.push(event_bus.subscribe("monitorremovedv2", move |_| rebind()));
        }

        WorkspaceWidget {
            container,
            labels,
            active,
            _subs: subs,
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────

fn build_labels(
    container: &gtk::Box,
    connector: &str,
) -> HashMap<String, gtk::Label> {
    let map: HashMap<String, gtk::Label> = hyprland::get_workspace_rules()
        .into_iter()
        .filter(|r| r.monitor == connector)
        .map(|r| {
            let label = gtk::Label::new(Some(&r.workspace_string));
            label.add_css_class("workspace-label");

            // click
            let target = r.workspace_string.clone();
            let click = gtk::GestureClick::new();
            click.connect_pressed(move |_gesture, _n, _x, _y| {
                eprintln!("[workspace] click -> {}", target);
                hyprland::dispatch_workspace(&target);
            });
            label.add_controller(click);

            // scroll
            {
                let label_weak = label.clone();
                let scroll = gtk::EventControllerScroll::new(
                    gtk::EventControllerScrollFlags::VERTICAL,
                );
                scroll.connect_scroll(move |_ctrl, _dx, dy| {
                    cycle_workspace(&label_weak, dy);
                    gtk::glib::Propagation::Proceed
                });
                label.add_controller(scroll);
            }

            (r.workspace_string, label)
        })
        .collect();

    // sort by numeric parse, then append
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by(|a, b| {
        let na: i64 = a.parse().unwrap_or(i64::MAX);
        let nb: i64 = b.parse().unwrap_or(i64::MAX);
        na.cmp(&nb).then_with(|| a.cmp(b))
    });
    for k in keys {
        container.append(&map[k]);
    }

    eprintln!(
        "[workspace] bound for {:?}: {:?}",
        connector,
        map.keys().collect::<Vec<_>>()
    );

    map
}

fn update_active(
    labels: &RefCell<HashMap<String, gtk::Label>>,
    active: &RefCell<Option<String>>,
    key: &str,
) {
    let labels = labels.borrow();
    if !labels.contains_key(key) {
        return; // not our monitor's workspace
    }

    // clear old
    if let Some(ref old) = *active.borrow() {
        if let Some(label) = labels.get(old) {
            label.remove_css_class("workspace-active");
        }
    }

    // set new
    if let Some(label) = labels.get(key) {
        label.add_css_class("workspace-active");
        *active.borrow_mut() = Some(key.to_string());
        eprintln!("[workspace] active -> {}", key);
    }
}

fn cycle_workspace(label: &gtk::Label, dy: f64) {
    // walk up to container
    let container = match label
        .parent()
        .and_then(|p| p.downcast::<gtk::Box>().ok())
    {
        Some(b) => b,
        None => return,
    };

    // collect siblings in display order
    let mut siblings: Vec<gtk::Label> = Vec::new();
    let mut child = container.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        if let Ok(l) = c.downcast::<gtk::Label>() {
            siblings.push(l);
        }
    }

    let n = siblings.len();
    if n == 0 {
        return;
    }

    let cur = siblings.iter().position(|l| l == label).unwrap_or(0);
    let new = if dy > 0.0 {
        (cur + 1) % n
    } else {
        (cur + n - 1) % n
    };

    let target = siblings[new].text();
    eprintln!("[workspace] scroll -> {}", target);
    hyprland::dispatch_workspace(&target);
}
