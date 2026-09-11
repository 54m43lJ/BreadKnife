//! Interactive demo tray item (SPEC §10.2) — a fake SNI application you can
//! drive from stdin to exercise every library code path without a real
//! tray app.
//!
//! ```bash
//! cargo run --example demo-item --features testing
//! ```
//!
//! Commands (one per line):
//! ```text
//!   status <passive|active|needs-attention>
//!   icon <icon-name>
//!   pixmap [off]
//!   tooltip <title> <description>
//!   menu add <label> | menu toggle <id> | menu remove <id>
//!   toggle-menu
//!   remove
//! ```

use std::io::{BufRead, Write};

use tray::testing::{spawn_demo_item, DemoItemConfig};

fn main() {
    let config = DemoItemConfig::default();
    let demo = spawn_demo_item(config.clone()).expect("spawn demo item");
    let mut menu_on = config.menu;
    println!("demo item online: {}", demo.service());
    println!("type commands, empty line or `remove` to exit");

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let r: Result<(), tray::TrayError> = match (cmd, rest) {
            ("status", arg) => demo.set_status(arg),
            ("icon", arg) => demo.set_icon_name(arg),
            ("pixmap", arg) => demo.use_pixmap(arg != "off"),
            ("tooltip", rest) => {
                let (t, d) = rest.split_once(' ').unwrap_or((rest, ""));
                demo.set_tooltip(t, d)
            }
            ("menu", rest) => {
                let (sub, arg) = rest.split_once(' ').unwrap_or((rest, ""));
                match (sub, arg) {
                    ("add", label) => demo.menu_add(label).map(|id| {
                        println!("  added menu id {id}");
                    }),
                    ("toggle", id) => demo.menu_toggle(id.parse().unwrap_or(-1)),
                    ("remove", id) => demo.menu_remove(id.parse().unwrap_or(-1)),
                    _ => Err(tray::TrayError::Protocol("menu <add|toggle|remove>".into())),
                }
            }
            ("toggle-menu", _) => {
                menu_on = !menu_on;
                demo.set_menu_enabled(menu_on)
            }
            ("remove", _) => break,
            _ => Err(tray::TrayError::Protocol(format!(
                "unknown command: {line}"
            ))),
        };
        if let Err(e) = r {
            println!("error: {e}");
        }
        let _ = out.flush();
    }
    let _ = demo.remove();
    println!("demo item removed");
}
