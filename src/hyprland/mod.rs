//! Hyprland IPC: sync commands + async event bus.
//!
//! Tokio reads socket2 events on a dedicated thread and dispatches them
//! to the GTK main thread through an async channel (glib >= 0.22 dropped
//! the old self-pipe + fd watch approach).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde::Deserialize;

// ── types ───────────────────────────────────────────────────────

#[allow(unused)]
#[derive(Deserialize, Debug, Clone)]
pub struct ActiveWorkspace {
    pub id: i64,
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WorkspaceRule {
    #[serde(rename = "workspaceString")]
    pub workspace_string: String,
    pub monitor: String,
}

// ── socket paths ────────────────────────────────────────────────

fn hyprland_dir() -> String {
    format!(
        "{}/hypr/{}",
        std::env::var("XDG_RUNTIME_DIR").unwrap(),
        std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap()
    )
}

fn event_socket() -> String {
    format!("{}/.socket2.sock", hyprland_dir())
}

// ── sync commands (via hyprctl subprocess) ──────────────────────

fn hyprctl(args: &[&str]) -> String {
    match std::process::Command::new("hyprctl")
        .args(args)
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => {
            eprintln!("[hyprland] hyprctl {:?} failed: {}", args, e);
            String::new()
        }
    }
}

pub fn get_workspace_rules() -> Vec<WorkspaceRule> {
    let resp = hyprctl(&["-j", "workspacerules"]);
    serde_json::from_str(&resp).unwrap_or_default()
}

pub fn get_active_workspace() -> Option<ActiveWorkspace> {
    let resp = hyprctl(&["-j", "activeworkspace"]);
    serde_json::from_str(&resp).ok()
}

pub fn get_active_window_title() -> Option<String> {
    let resp = hyprctl(&["-j", "activewindow"]);
    let json: serde_json::Value = serde_json::from_str(&resp).ok()?;
    json.get("title")?.as_str().map(|s| s.to_string())
}

pub fn dispatch_workspace(name: &str) {
    let cmd = format!("hl.dsp.focus({{ workspace = \"{}\" }})", name);
    hyprctl(&["dispatch", &cmd]);
}

// ── EventBus ────────────────────────────────────────────────────

type Callback = Box<dyn Fn(&str)>;

struct EventBusInner {
    subscribers: RefCell<HashMap<String, Vec<(u64, Callback)>>>,
    next_id: RefCell<u64>,
}

impl EventBusInner {
    fn dispatch(&self, event: &str, data: &str) {
        // eprintln!("[hyprland] event '{}' data='{}'", event, data);
        if let Some(subs) = self.subscribers.borrow().get(event) {
            for (_id, cb) in subs {
                cb(data);
            }
        }
    }
}

pub struct Subscription {
    id: u64,
    event: String,
    inner: Rc<EventBusInner>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.inner
            .subscribers
            .borrow_mut()
            .get_mut(&self.event)
            .map(|subs| subs.retain(|(id, _)| *id != self.id));
    }
}

pub struct EventBus {
    inner: Rc<EventBusInner>,
}

impl EventBus {
    pub fn start() -> Self {
        let inner = Rc::new(EventBusInner {
            subscribers: RefCell::new(HashMap::new()),
            next_id: RefCell::new(0),
        });

        let (tx, rx) = async_channel::unbounded::<(String, String)>();

        // main-loop consumer
        {
            let inner_dispatch = inner.clone();
            gtk::glib::spawn_future_local(async move {
                while let Ok((event, data)) = rx.recv().await {
                    inner_dispatch.dispatch(&event, &data);
                }
            });
        }

        // socket2 reader thread
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let path = event_socket();
                let stream = match tokio::net::UnixStream::connect(&path).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[hyprland] socket2 connect error: {}", e);
                        return;
                    }
                };
                eprintln!("[hyprland] listening on socket2 ({})", path);

                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stream);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some((event, data)) = parse_event(&line) {
                        if tx.send_blocking((event, data)).is_err() {
                            break;
                        }
                    }
                }
                eprintln!("[hyprland] socket2 disconnected");
            });
        });

        EventBus { inner }
    }

    pub fn subscribe(&self, event: &str, f: impl Fn(&str) + 'static) -> Subscription {
        let mut subs = self.inner.subscribers.borrow_mut();
        let mut next = self.inner.next_id.borrow_mut();
        let id = *next;
        *next += 1;
        subs.entry(event.to_string())
            .or_default()
            .push((id, Box::new(f)));
        Subscription {
            id,
            event: event.to_string(),
            inner: self.inner.clone(),
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────

fn parse_event(line: &str) -> Option<(String, String)> {
    let (event, data) = line.split_once(">>")?;
    Some((event.to_string(), data.to_string()))
}
