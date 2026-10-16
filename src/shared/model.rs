//! daemon / 窗口共用数据模型（窗口切片、状态增量）。
//! 类型契约见 ARCHITECTURE.html §4.5；IPC 消息壳见 protocol.rs。

use serde::{Deserialize, Serialize};

// ── 工作区（形态见 README「界面交互」；色彩映射见 ARCHITECTURE §3.10）──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceState {
    pub id: i64,
    pub name: String,
    pub urgent: bool,
    pub active: bool,
    pub has_windows: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ActiveWindow {
    pub class: String,
    pub title: String,
}

// ── 托盘 Item 视图（daemon → 窗口；渲染直出为可绘制对象）──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IconView {
    /// Freedesktop 图标名。
    Name(String),
    /// 文件路径图标（Ayatana 生态事实扩展）。
    Path(String),
    /// ARGB32 位图（网络字节序 [A,R,G,B]，行跨度 = width × 4）。
    Pixmap {
        width: u32,
        height: u32,
        #[serde(with = "b64_vec")]
        argb: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrayItemView {
    pub id: String,
    pub title: Option<String>,
    /// 悬停提示（title 与 description 预拼接）。
    pub tooltip: Option<String>,
    pub icon: IconView,
    /// SNI NeedsAttention 状态。
    pub attention: bool,
    /// 存活标记：false → 窗口置灰（移除只由 Removed 驱动）。
    pub alive: bool,
    pub has_menu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TrayUpdate {
    Added(TrayItemView),
    Removed(String),
    Changed(TrayItemView),
}

// ── 菜单快照（懒加载，每次一层）──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MenuSnapshot {
    pub parent_id: i32,
    pub revision: u32,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MenuItem {
    pub id: i32,
    pub item_type: MenuItemType,
    /// 已解析助记符（GTK use-underline 直接可用）。
    pub label: String,
    pub raw_label: String,
    pub enabled: bool,
    pub visible: bool,
    pub children_display: bool,
    pub toggle_type: Option<ToggleType>,
    pub toggle_state: Option<ToggleState>,
    pub icon_name: Option<String>,
    /// PNG 字节流。
    #[serde(with = "b64_opt", default)]
    pub icon_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MenuItemType {
    Standard,
    Separator,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToggleType {
    Checkmark,
    Radio,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToggleState {
    Off,
    On,
    Indeterminate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InteractionOutcome {
    /// 已路由为 Item 的 VOID 调用（含 ContextMenu 回退：Item 自绘菜单，宿主不渲染）。
    Dispatched,
    Menu(MenuSnapshot),
}

// ── 系统状态 ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SystemState {
    pub battery: Option<BatteryState>,
    pub network: Option<NetworkState>,
    pub volume: Option<VolumeState>,
    pub brightness: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatteryState {
    pub percent: u32,
    pub charging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkState {
    /// ethernet / wifi / …；无连接时为 "none"。
    pub kind: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VolumeState {
    pub percent: u32,
    pub muted: bool,
}

// ── 通知记录（应用名、图标、摘要、正文、通知 ID）──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationRecord {
    pub id: u32,
    pub app_name: String,
    pub icon: String,
    pub summary: String,
    pub body: String,
}

// ── 窗口切片与状态增量 ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowSnapshot {
    pub monitor: String,
    pub workspaces: Vec<WorkspaceState>,
    pub active: ActiveWindow,
    pub tray: Vec<TrayItemView>,
    pub notifications: Vec<NotificationRecord>,
    pub system: SystemState,
    pub css_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StateDelta {
    /// 本屏工作区切片（定向推送）。
    pub workspaces: Option<Vec<WorkspaceState>>,
    pub active: Option<ActiveWindow>,
    pub system: Option<SystemState>,
}

// ── base64 编解码（位图 / PNG 跨 JSON 线格式；fd 传递为文档留出的升级路径）──

mod b64_vec {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

mod b64_opt {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(bytes) => s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}
