use std::sync::mpsc;

// ── request types (GTK → background) ────────────────────────────

/// Requests sent from GTK thread to the zbus background thread
pub enum SniRequest {
    Activate {
        bus_name: String,
        object_path: String,
        x: i32,
        y: i32,
    },
    SecondaryActivate {
        bus_name: String,
        object_path: String,
        x: i32,
        y: i32,
    },
    Menu(MenuRequest),
}

pub struct MenuRequest {
    pub bus_name: String,
    pub menu_path: String,
    pub reply: mpsc::Sender<MenuResult>,
}

// ── menu result types ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MenuResult {
    pub items: Vec<MenuItem>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MenuItem {
    #[allow(unused)]
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub is_separator: bool,
    pub is_submenu: bool,
    pub children: Vec<MenuItem>,
    #[allow(unused)]
    pub toggle_state: i32,
    pub action: String,
}
