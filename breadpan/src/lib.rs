//! breadpan：SNI 托盘协议栈单库。
//!
//! 登记中心（Watcher）内置，开箱即用。消费方只面对五件事——
//! Item 数据打包获取、Item 事件流、Item 交互、菜单读取、打包生成可渲染的 GTK 对象。
//!
//! 公开面契约见 `breadpan/ARCHITECTURE.html §3`；本文件承载公开数据模型与错误，
//! 入口与操作面在 [`handle`] 模块（经本文件再导出）。

pub mod events;
mod handle;
mod props;
mod tracker;

mod dbus;
#[cfg(feature = "gtk")]
pub mod gtk;

#[cfg(test)]
mod tests;

pub use events::{InteractionOp, TrayEvent};
pub use handle::{Tray, TrayConfig, TrayHandle};

/// Item 唯一键：值即 Item 注册到 Watcher 的总线服务名。
pub type TrayItemId = String;

/// 一个托盘项的即时投影（现场读取，零缓存）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayItem {
    pub id: TrayItemId,
    pub category: TrayCategory,
    pub status: TrayStatus,
    pub alive: bool,
    pub ident: String,
    pub title: Option<String>,
    pub is_menu_only: bool,
    pub window_id: u32,
    pub icon: IconSource,
    pub overlay_icon: Option<IconSource>,
    pub attention_icon: Option<IconSource>,
    pub tooltip: Option<ToolTip>,
    /// SNI `Menu` 属性原文：com.canonical.dbusmenu 对象路径；`None` = 无 DBusMenu。
    pub menu: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCategory {
    ApplicationStatus,
    Communications,
    SystemServices,
    Hardware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    Passive,
    Active,
    NeedsAttention,
}

/// 悬停提示：官方四元组 (STRING, a(iiay), STRING, STRING) 的映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTip {
    pub icon: Option<IconSource>,
    pub title: String,
    pub description: String,
}

/// 图标来源：Name 非空 → Name，否则 → Pixmap（二选一规则固定，SNI §3.1.6）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    /// Freedesktop 图标名；渲染直出时按主题规范查主题。
    Name(String),
    /// 主题名缺失时交付的位图；转纹理的色彩处理在渲染直出路径内完成。
    Pixmap(Pixmap),
}

/// ARGB32 位图：网络字节序，行跨度 = width × 4（SNI §6，签名 a(iiay)）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

/// 单层菜单快照：懒加载，每次一层；深层经 `menu_expand` 按需展开。
#[derive(Debug, Clone, PartialEq)]
pub struct MenuSnapshot {
    /// 本次加载层的父条目 id（根层 0）＝ GetLayout parentId。
    pub parent_id: i32,
    /// 布局版本号，与 dbusmenu `LayoutUpdated.revision` 对应。
    pub revision: u32,
    /// parent 的直接子条目。
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub id: i32,
    /// 对应 dbusmenu 的 `type`（type 为 Rust 保留字，就近取名）。
    pub item_type: MenuItemType,
    /// 已解析助记符：`__` 显示为 `_`，第一个剩余 `_` 后字符为访问键。
    pub label: String,
    /// label 原始值（逃生门）。
    pub raw_label: String,
    pub enabled: bool,
    pub visible: bool,
    /// `children-display`="submenu" → true；空/其它值 → false。
    pub children_display: bool,
    pub toggle_type: Option<ToggleType>,
    pub toggle_state: Option<ToggleState>,
    pub icon_name: Option<String>,
    /// PNG 字节流（与 SNI 的 ARGB32 位图不同格式）。
    pub icon_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItemType {
    Standard,
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleType {
    Checkmark,
    Radio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleState {
    Off,
    On,
    Indeterminate,
}

/// 交互受理结果：`Dispatched` = 已路由为 Item 的 VOID 调用；`Menu` = 菜单流程现场快照。
#[derive(Debug, Clone, PartialEq)]
pub enum InteractionOutcome {
    Dispatched,
    Menu(MenuSnapshot),
}

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("session bus connect failed: {0}")]
    Bus(String),
    #[error("watcher bus name already owned elsewhere: {0}")]
    NameTaken(String),
    #[error("operation timed out after {ms} ms")]
    Timeout { ms: u64 },
    #[error("item {0} not found")]
    ItemNotFound(String),
    #[error("dbus call failed: {0}")]
    Protocol(String),
    #[error("tray runtime stopped")]
    Stopped,
}

impl From<zbus::Error> for TrayError {
    fn from(e: zbus::Error) -> Self {
        match e {
            zbus::Error::NameTaken => {
                TrayError::NameTaken("name already taken on the bus".to_string())
            }
            other => TrayError::Protocol(other.to_string()),
        }
    }
}
