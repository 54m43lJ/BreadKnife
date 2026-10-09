//! 最小消费方：start → 消费事件流 → 打印快照与全部事件。

use breadpan::{Tray, TrayConfig, TrayEvent};

fn main() {
    let (handle, mut rx) = match Tray::start(TrayConfig::default()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[minimal] tray start failed: {e}");
            std::process::exit(1);
        }
    };
    println!("[minimal] tray started; consuming events (Ctrl-C to quit)");

    // 事件流消费：消费方可以 recv().await（经 glib::spawn_future_local 驱动），
    // 此处为无 UI 示例，直接阻塞消费。
    while let Some(ev) = rx.blocking_recv() {
        match ev {
            TrayEvent::Snapshot { items } => {
                println!("[snapshot] {} item(s)", items.len());
                for item in items {
                    println!(
                        "  - {} (title={:?}, status={:?})",
                        item.id, item.title, item.status
                    );
                }
            }
            TrayEvent::Added(id) => println!("[added] {id}"),
            TrayEvent::Removed(id) => println!("[removed] {id}"),
            TrayEvent::Changed(id) => println!("[changed] {id} (re-read item() for fresh data)"),
            TrayEvent::MenuChanged(id) => println!("[menu-changed] {id}"),
            TrayEvent::InteractionResult { id, op, result } => {
                println!("[interaction] {id} {op:?} => {result:?}")
            }
            TrayEvent::Displaced => {
                println!("[displaced] watcher ownership lost; terminal");
                break;
            }
            TrayEvent::Narration(msg) => println!("[narration] {msg}"),
            TrayEvent::Fatal(msg) => {
                println!("[fatal] {msg}");
                break;
            }
        }
    }

    handle.shutdown();
}
