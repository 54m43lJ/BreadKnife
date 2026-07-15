use crate::sni::icon::IconPixmap;

// ── event types ──────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum UpdateKind {
    Icon,
    ToolTip,
    Status,
    Title,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum SniEvent {
    ItemAdded { id: String, state: SniItemState },
    ItemRemoved { id: String },
    ItemUpdated { id: String, kind: UpdateKind, state: SniItemState },
}

// ── item state ───────────────────────────────────────────────────

/// Snapshot of an SNI item's properties at a point in time
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SniItemState {
    pub id: String,
    pub bus_name: String,
    pub object_path: String,
    pub category: String,
    pub title: String,
    pub status: String,
    pub icon_name: String,
    pub icon_theme_path: String,
    pub icon_pixmaps: Vec<IconPixmap>,
    pub attention_icon_name: String,
    pub attention_icon_pixmaps: Vec<IconPixmap>,
    pub overlay_icon_name: String,
    pub overlay_icon_pixmaps: Vec<IconPixmap>,
    pub item_is_menu: bool,
    pub menu_path: String,
    pub window_id: u32,
}

impl SniItemState {
    pub fn empty(id: &str, bus_name: &str, object_path: &str) -> Self {
        SniItemState {
            id: id.to_string(),
            bus_name: bus_name.to_string(),
            object_path: object_path.to_string(),
            category: String::new(),
            title: String::new(),
            status: String::new(),
            icon_name: String::new(),
            icon_theme_path: String::new(),
            icon_pixmaps: Vec::new(),
            attention_icon_name: String::new(),
            attention_icon_pixmaps: Vec::new(),
            overlay_icon_name: String::new(),
            overlay_icon_pixmaps: Vec::new(),
            item_is_menu: false,
            menu_path: String::new(),
            window_id: 0,
        }
    }
}
