//! 交互与菜单：activate / 滚轮 / menu()·menu_expand 现场读取 / menu_activate。
//!
//! 对首个在册 Item 逐一演示交互方法；无 Item 时打印提示后退出。

use breadpan::{InteractionOutcome, Tray, TrayConfig, TrayEvent};

fn main() {
    let (handle, mut rx) = match Tray::start(TrayConfig::default()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[interactions] tray start failed: {e}");
            std::process::exit(1);
        }
    };

    // 首事件即快照（原子衔接起点）
    let Some(TrayEvent::Snapshot { items }) = rx.blocking_recv() else {
        eprintln!("[interactions] event stream ended before snapshot");
        handle.shutdown();
        return;
    };
    println!("[interactions] snapshot: {} item(s)", items.len());

    let Some(target) = items.first().map(|i| i.id.clone()) else {
        println!("[interactions] no tray items on the bus; nothing to interact with");
        handle.shutdown();
        return;
    };
    println!("[interactions] target item: {target}");

    // ── 交互：左键（Activate 或整项即菜单路由）──
    match handle.left_click(&target) {
        Ok(InteractionOutcome::Dispatched) => {
            println!("left_click -> dispatched (receipt arrives as InteractionResult event)")
        }
        Ok(InteractionOutcome::Menu(menu)) => {
            println!(
                "left_click -> menu flow (item is menu-only): {} entry(ies)",
                menu.items.len()
            )
        }
        Err(e) => println!("left_click -> error: {e}"),
    }

    // ── 滚轮（steps 缺省 1；_by 变体传显式值）──
    match handle.scroll_up_by(&target, 3) {
        Ok(_) => println!("scroll_up_by(3) -> dispatched"),
        Err(e) => println!("scroll_up_by(3) -> error: {e}"),
    }
    let _ = handle.scroll_down(&target);

    // ── 菜单：右键路由（DBusMenu 在册 → Menu 流程；否则 ContextMenu fallback）──
    let mut submenu_entry: Option<i32> = None;
    match handle.right_click(&target) {
        Ok(InteractionOutcome::Menu(menu)) => {
            println!("right_click -> menu (revision {}):", menu.revision);
            for entry in &menu.items {
                println!(
                    "  [{}] label={:?} raw={:?} enabled={} toggle={:?} submenu={}",
                    entry.id,
                    entry.label,
                    entry.raw_label,
                    entry.enabled,
                    entry.toggle_type,
                    entry.children_display
                );
                if entry.children_display {
                    submenu_entry = Some(entry.id);
                }
            }
        }
        Ok(InteractionOutcome::Dispatched) => {
            println!("right_click -> ContextMenu fallback dispatched")
        }
        Err(e) => println!("right_click -> error: {e}"),
    }

    // ── 子层展开：menu_expand 逐层懒加载 ──
    if let Some(entry_id) = submenu_entry {
        match handle.menu_expand(&target, entry_id) {
            Ok(menu) => {
                println!(
                    "menu_expand({entry_id}) -> {} child entry(ies)",
                    menu.items.len()
                );
                // 子层首个可用条目的点击提交
                if let Some(child) = menu.items.iter().find(|e| e.enabled && e.visible) {
                    match handle.menu_activate(&target, child.id) {
                        Ok(()) => println!("menu_activate({}) -> accepted", child.id),
                        Err(e) => println!("menu_activate({}) -> error: {e}", child.id),
                    }
                }
            }
            Err(e) => println!("menu_expand({entry_id}) -> error: {e}"),
        }
    }

    // ── 消费回执与后续事件，直至停机 ──
    handle.shutdown();
    while let Some(ev) = rx.blocking_recv() {
        println!("[event] {ev:?}");
    }
    println!("[interactions] done");
}
