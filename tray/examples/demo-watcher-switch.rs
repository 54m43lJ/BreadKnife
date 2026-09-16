//! Watcher role switcher (SPEC §10.6) — manufactures
//! `TrayEvent::WatcherChanged` transitions for a running tray host.
//!
//! ```bash
//! cargo run --example demo-watcher-switch --features testing
//! ```
//!
//! Press <Enter> to toggle: take over `org.kde.StatusNotifierWatcher`
//! (a running host reports `ExternalUp`), or yield again (a host that was
//! fallback becomes `ExternalDown`, respawns as `FallbackActive` after
//! `2 * timeout_ms`). Type `q<Enter>` to quit.

use std::io::{BufRead, Write};

use tray::testing::DemoWatcher;

fn main() {
    let conn = zbus::blocking::Connection::session().expect("connect to session bus");
    let watcher = DemoWatcher::new(conn);

    println!("demo watcher switch online ({}).", watcher.unique_name());
    println!("Enter = toggle watcher role, q<Enter> = quit");
    print_state(&watcher, false);

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        match line.trim() {
            "q" | "quit" => break,
            "" => {
                if watcher.is_owner() {
                    watcher.release();
                } else if let Err(e) = watcher.take() {
                    println!("take failed: {e}");
                }
                print_state(&watcher, true);
            }
            other => println!("unknown input: {other:?} (Enter toggles, q quits)"),
        }
        let _ = out.flush();
    }
    watcher.release();
    println!("bye");
}

fn print_state(watcher: &DemoWatcher, changed: bool) {
    let role = if watcher.is_owner() {
        "ON — we own org.kde.StatusNotifierWatcher (hosts see ExternalUp)"
    } else {
        "off — name released (hosts see ExternalDown / respawn FallbackActive)"
    };
    println!("{}watcher role: {role}", if changed { "-> " } else { "" });
}
