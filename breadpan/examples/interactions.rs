//! 交互式宿主示例：持续消费事件流（模拟真实托盘程序的数据面）+ 简单 REPL。
//!
//! 结构与真实宿主同构——
//! - 后台线程：消费事件流，实时打印事件并维护 Item 卡片册
//!   （快照重建 / Added 建卡 / Removed 删卡 / Changed 重读刷新）；
//! - 主线程 REPL：以「编号」或「id 前缀」引用 Item，发起查询与全部交互；
//!   菜单互动现场读取并打印返回数据。
//!
//! 命令（help 可随时查看）：
//!   list                                列出在册 Item
//!   info     <编号|id前缀>              查看属性（现场读取，零缓存）
//!   menu     <编号|id前缀>              读取根层菜单
//!   expand   <编号|id前缀> <条目id>     展开子菜单层
//!   click    <编号|id前缀> [x y]        左键（Activate 或整项即菜单路由）
//!   middle   <编号|id前缀>              中键（SecondaryActivate）
//!   right    <编号|id前缀>              右键（Menu 流程或 ContextMenu fallback）
//!   scroll   <编号|id前缀> <方向> [步数] 滚轮（up/down/left/right）
//!   mclick   <编号|id前缀> <条目id>     菜单条目点击提交（menu_activate）
//!   alive    <编号|id前缀>              存活探测结论（零 DBus 流量）
//!   quit                                优雅停机退出（Ctrl-D 同）

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use breadpan::{
    InteractionOutcome, MenuSnapshot, Tray, TrayConfig, TrayEvent, TrayHandle, TrayItem,
};

/// 卡片册：id → 最近数据（None = 已报到但现场读取尚不可用）。
type Cards = Arc<Mutex<BTreeMap<String, Option<TrayItem>>>>;

fn main() {
    let (handle, mut rx) = match Tray::start(TrayConfig::default()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[interactions] tray start failed: {e}");
            std::process::exit(1);
        }
    };
    println!("[interactions] tray started; commands: 'help', 'quit'");

    let cards: Cards = Arc::new(Mutex::new(BTreeMap::new()));
    let snapshot_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 事件消费线程（真实宿主的渲染驱动面）
    {
        let cards = cards.clone();
        let handle = handle.clone();
        let snapshot_seen = snapshot_seen.clone();
        thread::spawn(move || {
            while let Some(ev) = rx.blocking_recv() {
                if matches!(ev, TrayEvent::Snapshot { .. }) {
                    snapshot_seen.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                handle_event(&handle, &cards, ev);
            }
        });
    }

    // 启动栅栏：等首事件（快照）就绪后再出提示符
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !snapshot_seen.load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() < deadline
        {
            thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // REPL
    let stdin = std::io::stdin();
    loop {
        print!("tray> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => break, // EOF / 读失败
            Ok(_) => {}
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        if repl(&handle, &cards, &tokens) {
            break;
        }
    }
    handle.shutdown();
    println!("[interactions] bye");
}

// ── 事件面 ──

fn handle_event(handle: &TrayHandle, cards: &Cards, ev: TrayEvent) {
    match ev {
        TrayEvent::Snapshot { items } => {
            let mut c = cards.lock().unwrap();
            c.clear();
            for item in items {
                c.insert(item.id.clone(), Some(item));
            }
            println!("\n[event] snapshot: {} item(s)", c.len());
        }
        TrayEvent::Added(id) => {
            let fetched = handle.item(&id);
            match &fetched {
                Some(item) => println!("\n[event] added: {} (title={:?})", item.id, item.title),
                None => println!("\n[event] added: {id} (data unavailable yet)"),
            }
            cards.lock().unwrap().insert(id, fetched);
        }
        TrayEvent::Removed(id) => {
            cards.lock().unwrap().remove(&id);
            println!("\n[event] removed: {id}");
        }
        TrayEvent::Changed(id) => {
            // 真实宿主在此重读刷新卡片
            let fetched = handle.item(&id);
            let old = cards.lock().unwrap().get(&id).cloned().flatten();
            let title = fetched.as_ref().and_then(|i| i.title.clone());
            println!(
                "\n[event] changed: {id} (re-read title: {:?}, was {:?})",
                title,
                old.and_then(|i| i.title)
            );
            if fetched.is_some() {
                cards.lock().unwrap().insert(id, fetched);
            }
        }
        TrayEvent::MenuChanged(id) => {
            println!("\n[event] menu-changed: {id} (use 'menu' to re-pull if desired)");
        }
        TrayEvent::InteractionResult { id, op, result } => {
            println!("\n[event] interaction: {id} {op:?} => {result:?}");
        }
        TrayEvent::Displaced => {
            println!("\n[event] displaced: watcher ownership lost; exiting");
            handle.shutdown();
            std::process::exit(0);
        }
        TrayEvent::Narration(msg) => println!("\n[event] narration: {msg}"),
        TrayEvent::Fatal(msg) => {
            println!("\n[event] fatal: {msg}; exiting");
            handle.shutdown();
            std::process::exit(0);
        }
    }
}

// ── REPL ──

/// 返回 true 表示退出。
fn repl(handle: &TrayHandle, cards: &Cards, t: &[&str]) -> bool {
    match t[0] {
        "help" | "h" => print_help(),
        "list" | "l" => list_items(cards),
        "info" => with_target(handle, cards, t.get(1), |id| match handle.item(id) {
            Some(item) => print_item(&item),
            None => println!("no data for {id}"),
        }),
        "menu" => with_target(handle, cards, t.get(1), |id| match handle.menu(id) {
            Ok(menu) => print_menu(&menu),
            Err(e) => println!("menu({id}) failed: {e}"),
        }),
        "expand" => with_target(handle, cards, t.get(1), |id| {
            let Some(entry_id) = t.get(2).and_then(|s| s.parse::<i32>().ok()) else {
                println!("usage: expand <item> <entry-id>");
                return;
            };
            match handle.menu_expand(id, entry_id) {
                Ok(menu) => print_menu(&menu),
                Err(e) => println!("menu_expand({id}, {entry_id}) failed: {e}"),
            }
        }),
        "click" => with_target(handle, cards, t.get(1), |id| {
            let (x, y) = coords(t.get(2), t.get(3));
            match handle.left_click_at(id, x, y) {
                Ok(outcome) => print_outcome("left_click", outcome),
                Err(e) => println!("left_click({id}) failed: {e}"),
            }
        }),
        "middle" => with_target(handle, cards, t.get(1), |id| {
            match handle.middle_click(id) {
                Ok(outcome) => print_outcome("middle_click", outcome),
                Err(e) => println!("middle_click({id}) failed: {e}"),
            }
        }),
        "right" => with_target(handle, cards, t.get(1), |id| match handle.right_click(id) {
            Ok(outcome) => print_outcome("right_click", outcome),
            Err(e) => println!("right_click({id}) failed: {e}"),
        }),
        "scroll" => with_target(handle, cards, t.get(1), |id| {
            let Some(dir) = t.get(2) else {
                println!("usage: scroll <item> <up|down|left|right> [steps]");
                return;
            };
            let steps = t.get(3).and_then(|s| s.parse::<i32>().ok()).unwrap_or(1);
            let result = match *dir {
                "up" => handle.scroll_up_by(id, steps),
                "down" => handle.scroll_down_by(id, steps),
                "left" => handle.scroll_left_by(id, steps),
                "right" => handle.scroll_right_by(id, steps),
                other => {
                    println!("unknown direction: {other}");
                    return;
                }
            };
            match result {
                Ok(outcome) => print_outcome("scroll", outcome),
                Err(e) => println!("scroll({id}) failed: {e}"),
            }
        }),
        "mclick" => with_target(handle, cards, t.get(1), |id| {
            let Some(entry_id) = t.get(2).and_then(|s| s.parse::<i32>().ok()) else {
                println!("usage: mclick <item> <entry-id>");
                return;
            };
            match handle.menu_activate(id, entry_id) {
                Ok(()) => {
                    println!("menu_activate({id}, {entry_id}) accepted (receipt arrives as event)")
                }
                Err(e) => println!("menu_activate({id}, {entry_id}) failed: {e}"),
            }
        }),
        "alive" => with_target(handle, cards, t.get(1), |id| {
            println!("is_alive({id}) = {}", handle.is_alive(id));
        }),
        "quit" | "q" => return true,
        other => println!("unknown command: {other}; 'help' for usage"),
    }
    false
}

/// 解析目标 token（编号 / 精确 id / 唯一 id 前缀）后执行动作。
fn with_target(_handle: &TrayHandle, cards: &Cards, tok: Option<&&str>, f: impl FnOnce(&String)) {
    let Some(tok) = tok else {
        println!("missing <item> argument (see 'help')");
        return;
    };
    let tok = *tok;
    let cards = cards.lock().unwrap();
    let resolved: Option<String> = if let Ok(n) = tok.parse::<usize>() {
        (n >= 1).then(|| cards.keys().nth(n - 1).cloned()).flatten()
    } else if cards.contains_key(tok) {
        Some(tok.to_string())
    } else {
        let hits: Vec<&String> = cards.keys().filter(|k| k.starts_with(tok)).collect();
        match hits.len() {
            1 => Some(hits[0].clone()),
            0 => None,
            _ => {
                println!("ambiguous prefix {tok:?}: {} matches", hits.len());
                return;
            }
        }
    };
    match resolved {
        Some(id) => f(&id),
        None => println!("no item matches {tok:?} (see 'list')"),
    }
}

fn coords(x: Option<&&str>, y: Option<&&str>) -> (i32, i32) {
    let x = x.and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
    let y = y.and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
    (x, y)
}

// ── 打印 ──

fn print_help() {
    println!(
        "commands:\n  \
         list                               list registered items\n  \
         info     <item>                    show item properties (live read)\n  \
         menu     <item>                    read root menu\n  \
         expand   <item> <entry-id>         expand a submenu layer\n  \
         click    <item> [x y]              left click (Activate or menu routing)\n  \
         middle   <item>                    middle click (SecondaryActivate)\n  \
         right    <item>                    right click (Menu flow or ContextMenu fallback)\n  \
         scroll   <item> <dir> [steps]      scroll up/down/left/right\n  \
         mclick   <item> <entry-id>         submit menu entry click (menu_activate)\n  \
         alive    <item>                    liveness verdict (no dbus traffic)\n  \
         quit                               graceful shutdown\n<item> = 1-based index from 'list', exact id, or unique id prefix"
    );
}

fn list_items(cards: &Cards) {
    let cards = cards.lock().unwrap();
    if cards.is_empty() {
        println!("no items registered");
        return;
    }
    println!(
        "{:<3} {:<44} {:<14} {:<8} alive",
        "#", "id", "title", "status"
    );
    for (n, (id, item)) in cards.iter().enumerate() {
        let (title, status, alive) = match item {
            Some(i) => (
                i.title.clone().unwrap_or_else(|| "-".into()),
                format!("{:?}", i.status),
                i.alive,
            ),
            None => ("-".into(), "?".into(), false),
        };
        println!(
            "{:<3} {:<44} {:<14} {:<8} {}",
            n + 1,
            id,
            title,
            status,
            alive
        );
    }
}

fn icon_desc(icon: &breadpan::IconSource) -> String {
    match icon {
        breadpan::IconSource::Name(n) => format!("Name({n})"),
        breadpan::IconSource::Pixmap(p) => format!("Pixmap({}x{})", p.width, p.height),
    }
}

fn print_item(item: &TrayItem) {
    let opt_icon =
        |o: &Option<breadpan::IconSource>| o.as_ref().map(icon_desc).unwrap_or_else(|| "-".into());
    println!("  id             : {}", item.id);
    println!("  category       : {:?}", item.category);
    println!("  status         : {:?}", item.status);
    println!("  alive          : {}", item.alive);
    println!("  ident          : {}", item.ident);
    println!("  title          : {:?}", item.title);
    println!("  is_menu_only   : {}", item.is_menu_only);
    println!("  window_id      : {}", item.window_id);
    println!("  icon           : {}", icon_desc(&item.icon));
    println!("  overlay_icon   : {}", opt_icon(&item.overlay_icon));
    println!("  attention_icon : {}", opt_icon(&item.attention_icon));
    if let Some(t) = &item.tooltip {
        println!("  tooltip        : {:?} / {:?}", t.title, t.description);
    } else {
        println!("  tooltip        : -");
    }
    println!("  menu           : {:?}", item.menu);
}

fn print_menu(menu: &MenuSnapshot) {
    println!(
        "menu: revision={} parent={} entries={}",
        menu.revision,
        menu.parent_id,
        menu.items.len()
    );
    for e in &menu.items {
        let kind = if e.item_type == breadpan::MenuItemType::Separator {
            "<separator>"
        } else {
            e.label.as_str()
        };
        let toggle = match (e.toggle_type, e.toggle_state) {
            (Some(t), Some(s)) => format!(" {t:?}={s:?}"),
            _ => String::new(),
        };
        let flags = [
            (!e.enabled).then_some("disabled"),
            (!e.visible).then_some("hidden"),
            e.children_display.then_some("submenu"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",");
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{flags}]")
        };
        let icon = e
            .icon_name
            .as_ref()
            .map(|n| format!(" icon={n}"))
            .unwrap_or_default();
        println!("  [{:>3}] {:?}{}{}{}", e.id, kind, toggle, flags, icon);
        if e.label != e.raw_label && e.item_type != breadpan::MenuItemType::Separator {
            println!("         raw label: {:?}", e.raw_label);
        }
    }
}

fn print_outcome(op: &str, outcome: InteractionOutcome) {
    match outcome {
        InteractionOutcome::Dispatched => {
            println!("{op} -> dispatched (receipt arrives as InteractionResult event)")
        }
        InteractionOutcome::Menu(menu) => {
            println!("{op} -> menu flow:");
            print_menu(&menu);
        }
    }
}
