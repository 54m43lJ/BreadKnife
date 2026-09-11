//! `tray-inspect` — zero-code observation CLI for the tray library
//! (SPEC §10.1).
//!
//! ```text
//! tray-inspect list                     print all items as a table, exit
//! tray-inspect watch                    live event stream
//! tray-inspect menu <bus> <path>        print a menu tree
//! tray-inspect activate <bus> <path> [x y]   fire Activate, print receipt
//! tray-inspect dump-json                full snapshot as JSON
//! ```

use std::process::ExitCode;
use std::time::Duration;

use tray::{
    IconSource, MenuItem, MenuItemKind, Tray, TrayConfig, TrayEvent, TrayItemId,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => cmd_list(),
        Some("watch") => cmd_watch(),
        Some("menu") => with_bus_path(&args[1..], cmd_menu),
        Some("activate") => with_bus_path(&args[1..], |h, id, rest| {
            let (x, y) = match (rest.first(), rest.get(1)) {
                (Some(x), Some(y)) => (x.parse().unwrap_or(0), y.parse().unwrap_or(0)),
                _ => (0, 0),
            };
            cmd_activate(h, id, x, y)
        }),
        Some("dump-json") => cmd_dump_json(),
        _ => {
            eprintln!("{}", USAGE);
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage: tray-inspect <list|watch|menu <bus> <path>|activate <bus> <path> [x y]|dump-json>";

fn with_bus_path(
    args: &[String],
    f: impl FnOnce(&tray::TrayHandle, TrayItemId, &[String]) -> Result<(), String>,
) -> Result<(), String> {
    let (bus, path) = match (args.first(), args.get(1)) {
        (Some(b), Some(p)) => (b.clone(), p.clone()),
        _ => return Err(USAGE.to_string()),
    };
    let (handle, _rx) =
        Tray::connect(TrayConfig::default()).map_err(|e| e.to_string())?;
    let id = TrayItemId {
        bus_name: bus,
        path,
    };
    let out = f(&handle, id, &args[2.min(args.len())..]);
    handle.shutdown();
    out
}

fn settle(handle: &tray::TrayHandle) {
    // Give the worker a moment to report existing items.
    for _ in 0..20 {
        if !handle.items().is_empty() {
            std::thread::sleep(Duration::from_millis(200));
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn cmd_list() -> Result<(), String> {
    let (handle, _rx) = Tray::connect(TrayConfig::default()).map_err(|e| e.to_string())?;
    settle(&handle);
    let items = handle.items();
    if items.is_empty() {
        println!("(no tray items)");
    }
    println!(
        "{:<32} {:<15} {:<20} {:<24} menu",
        "ID", "STATUS", "TITLE", "ICON"
    );
    for it in &items {
        let icon = match &it.icon {
            IconSource::Name(n) => format!("name:{n}"),
            IconSource::Pixmap(p) => format!("pixmap {}x{}", p.width, p.height),
        };
        println!(
            "{:<32} {:<15} {:<20} {:<24} {}",
            it.id,
            format!("{:?}", it.status),
            it.title.as_deref().unwrap_or("-"),
            icon,
            if it.has_menu { "yes" } else { "no" }
        );
    }
    handle.shutdown();
    Ok(())
}

fn cmd_watch() -> Result<(), String> {
    let (handle, rx) = Tray::connect(TrayConfig::default()).map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
    println!("watching tray events (Ctrl-C to exit)…");
    for ev in rx {
        let stamp = format!("[+{:.3}s]", start.elapsed().as_secs_f64());
        match ev {
            TrayEvent::Added(item) => println!("{stamp} Added {} ({:?})", item.id, item.status),
            TrayEvent::Removed(id) => println!("{stamp} Removed {id}"),
            TrayEvent::Changed { id, changed, item } => println!(
                "{stamp} Changed {} {:?} -> status={:?} icon={:?}",
                id, changed, item.status, item.icon
            ),
            TrayEvent::MenuChanged(id) => println!("{stamp} MenuChanged {id}"),
            TrayEvent::WatcherChanged(state) => println!("{stamp} WatcherChanged {state:?}"),
            TrayEvent::InteractionResult { id, op, result } => match result {
                Ok(()) => println!("{stamp} InteractionResult {id} {op:?} ok"),
                Err(e) => println!("{stamp} InteractionResult {id} {op:?} error: {e}"),
            },
            TrayEvent::Fatal(e) => {
                println!("{stamp} Fatal {e}");
                break;
            }
        }
    }
    handle.shutdown();
    Ok(())
}

fn wait_item(handle: &tray::TrayHandle, id: &TrayItemId) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if handle.item(id).is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("item {id} did not appear within 5s"))
}

fn cmd_menu(handle: &tray::TrayHandle, id: TrayItemId, _rest: &[String]) -> Result<(), String> {
    wait_item(handle, &id)?;
    // Pre-show contract: refresh via AboutToShow before reading the tree.
    handle
        .menu_about_to_show(&id, 0)
        .map_err(|e| e.to_string())?;
    let Some(menu) = handle.menu(&id) else {
        return Err(format!("no menu for {id}"));
    };
    println!("menu revision {}:", menu.revision);
    print_items(&menu.root, 1);
    handle.shutdown();
    Ok(())
}

fn print_items(items: &[MenuItem], depth: usize) {
    for it in items {
        let pad = "  ".repeat(depth);
        match it.kind {
            MenuItemKind::Separator => println!("{pad}---"),
            _ => {
                let marks = match (it.enabled, it.toggled) {
                    (false, _) => " [disabled]",
                    (true, Some(true)) => " [x]",
                    (true, Some(false)) => " [ ]",
                    (true, None) => "",
                };
                let sub = if it.kind == MenuItemKind::Submenu {
                    " >"
                } else {
                    ""
                };
                println!("{pad}#{} {}{marks}{sub}", it.id, it.label);
                if !it.children.is_empty() {
                    print_items(&it.children, depth + 1);
                }
            }
        }
    }
}

fn cmd_activate(
    handle: &tray::TrayHandle,
    id: TrayItemId,
    x: i32,
    y: i32,
) -> Result<(), String> {
    wait_item(handle, &id)?;
    handle.activate(&id, x, y).map_err(|e| e.to_string())?;
    let rx = handle.subscribe();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut receipt = false;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(TrayEvent::InteractionResult { id: i, .. }) if i == id => {
                receipt = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    match receipt {
        true => println!("Activate({x}, {y}) on {id}: delivered, item acknowledged"),
        false => println!("Activate sent; no receipt within 5s"),
    }
    handle.shutdown();
    Ok(())
}

fn cmd_dump_json() -> Result<(), String> {
    let (handle, _rx) = Tray::connect(TrayConfig::default()).map_err(|e| e.to_string())?;
    settle(&handle);
    let items = handle.items();
    let mut out = String::from("[");
    for (n, it) in items.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        out.push_str(&item_json(&handle, it));
    }
    out.push(']');
    println!("{out}");
    handle.shutdown();
    Ok(())
}

fn item_json(handle: &tray::TrayHandle, it: &tray::TrayItem) -> String {
    let mut out = format!(
        "{{\"id\":{},\"category\":\"{:?}\",\"status\":\"{:?}\",",
        json_string(&it.id.to_string()),
        it.category,
        it.status
    );
    out.push_str(&format!(
        "\"title\":{},\"description\":{},\"window_id\":{},",
        opt_json(it.title.as_ref().map(|s| json_string(s))),
        opt_json(it.description.as_ref().map(|s| json_string(s))),
        it.window_id
    ));
    out.push_str(&format!("\"icon\":{},", icon_json(&it.icon)));
    out.push_str(&format!(
        "\"tooltip\":{},\"has_menu\":{}",
        opt_json(it.tooltip.as_ref().map(|t| {
            format!(
                "{{\"title\":{},\"description\":{}}}",
                json_string(&t.title),
                json_string(&t.description)
            )
        })),
        it.has_menu
    ));
    if it.has_menu {
        if let Some(menu) = handle.menu(&it.id) {
            out.push_str(",\"menu\":");
            out.push_str(&menu_json(&menu));
        }
    }
    out.push('}');
    out
}

fn menu_json(menu: &tray::MenuSnapshot) -> String {
    let items: Vec<String> = menu.root.iter().map(menu_item_json).collect();
    format!(
        "{{\"revision\":{},\"root\":[{}]}}",
        menu.revision,
        items.join(",")
    )
}

fn menu_item_json(it: &MenuItem) -> String {
    let mut out = format!(
        "{{\"id\":{},\"kind\":\"{:?}\",\"label\":{},\"enabled\":{},\"visible\":{}",
        it.id,
        it.kind,
        json_string(&it.label),
        it.enabled,
        it.visible
    );
    if let Some(t) = it.toggled {
        out.push_str(&format!(",\"toggled\":{t}"));
    }
    if let Some(i) = &it.icon {
        out.push_str(&format!(",\"icon\":{}", json_string(i)));
    }
    if !it.children.is_empty() {
        let children: Vec<String> = it.children.iter().map(menu_item_json).collect();
        out.push_str(&format!(",\"children\":[{}]", children.join(",")));
    }
    out.push('}');
    out
}

fn icon_json(icon: &IconSource) -> String {
    match icon {
        IconSource::Name(n) => format!("{{\"name\":{}}}", json_string(n)),
        IconSource::Pixmap(p) => format!(
            "{{\"pixmap\":{{\"width\":{},\"height\":{},\"argb_len\":{}}}}}",
            p.width,
            p.height,
            p.argb.len()
        ),
    }
}

fn opt_json(v: Option<String>) -> String {
    match v {
        Some(s) => s,
        None => "null".to_string(),
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
