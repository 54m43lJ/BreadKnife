//! Minimal embedding example (SPEC §10.3) — the authoritative starting
//! point for secondary developers.
//!
//! ```bash
//! cargo run --example minimal
//! ```

use tray::{Tray, TrayConfig, TrayEvent};

fn main() {
    let (handle, rx) = Tray::connect(TrayConfig::default()).expect("connect to session bus");
    println!("tray host online; watching items (Ctrl-C to exit)…");
    for ev in rx {
        match ev {
            TrayEvent::Added(item) => println!(
                "added: {} title={:?} icon={:?}",
                item.id, item.title, item.icon
            ),
            TrayEvent::Removed(id) => println!("removed: {id}"),
            TrayEvent::Changed { id, changed, .. } => println!("changed: {id} fields={changed:?}"),
            TrayEvent::MenuChanged(id) => println!("menu changed: {id}"),
            TrayEvent::WatcherChanged(state) => println!("watcher: {state:?}"),
            TrayEvent::InteractionResult { id, op, result } => {
                println!("receipt: {id} {op:?} -> {result:?}")
            }
            TrayEvent::Fatal(e) => {
                eprintln!("fatal: {e}");
                break;
            }
        }
    }
    handle.shutdown();
}
