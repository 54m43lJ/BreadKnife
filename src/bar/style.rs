//! 按 daemon 跟踪的路径加载 CSS；reload 重读。
//! CSS 路径由 daemon 控制与跟踪（ARCHITECTURE §3.10）。

use std::cell::RefCell;

thread_local! {
    static CURRENT: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

pub fn load(path: &str) -> Result<(), String> {
    let display = gtk::gdk::Display::default().ok_or("no display")?;
    let provider = gtk::CssProvider::new();
    provider.load_from_path(path);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    CURRENT.with(|c| *c.borrow_mut() = Some(provider));
    Ok(())
}

/// reload 指令路径：先卸载旧 provider 再按同一路径重读（改色即见）。
pub fn reload(path: &str) -> Result<(), String> {
    CURRENT.with(|c| {
        if let Some(old) = c.borrow_mut().take() {
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_remove_provider_for_display(&display, &old);
            }
        }
    });
    load(path)
}
