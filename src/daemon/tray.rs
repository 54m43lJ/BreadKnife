//! breadpan 组装（Tray::start）+ 事件泵 + 交互执行 + 菜单读取。
//! 组装契约见 ARCHITECTURE.html §4.3；全部托盘调用在 daemon 内执行，
//! 同步阻塞只发生于调用线程，不冻结任何窗口。

use std::sync::mpsc::Sender;

use breadpan::{
    IconSource, InteractionOutcome as PanOutcome, MenuSnapshot as PanMenu, Pixmap, Tray,
    TrayConfig, TrayHandle, TrayItem,
};

use crate::daemon::DaemonEvent;
use crate::shared::model::{
    IconView, InteractionOutcome, MenuItem, MenuItemType, MenuSnapshot, ToggleState, ToggleType,
    TrayItemView,
};

/// 进程级组装一次；失败降级为空托盘（返回 None，仅日志）。
pub fn start(tx: Sender<DaemonEvent>) -> Option<TrayHandle> {
    match Tray::start(TrayConfig::default()) {
        Ok((handle, mut rx)) => {
            std::thread::Builder::new()
                .name("tray-events".into())
                .spawn(move || {
                    // 事件泵：库内 Receiver 唯一，daemon 是唯一消费方
                    while let Some(ev) = rx.blocking_recv() {
                        if tx.send(DaemonEvent::Tray(ev)).is_err() {
                            return;
                        }
                    }
                    eprintln!("[tray] event channel closed");
                })
                .expect("spawn tray-events");
            Some(handle)
        }
        Err(e) => {
            eprintln!("[tray] start failed, tray degraded: {e}");
            None
        }
    }
}

// ── breadpan 类型 → 共享视图（跨 IPC 载荷）──

pub fn view_of(item: TrayItem) -> TrayItemView {
    let tooltip = item.tooltip.map(|t| {
        if t.description.is_empty() {
            t.title
        } else {
            format!("{}\n{}", t.title, t.description)
        }
    });
    TrayItemView {
        id: item.id,
        title: item.title,
        tooltip,
        icon: icon_view_of(item.icon),
        attention: matches!(item.status, breadpan::TrayStatus::NeedsAttention),
        alive: item.alive,
        has_menu: item.menu.is_some(),
    }
}

fn icon_view_of(icon: IconSource) -> IconView {
    match icon {
        IconSource::Name(name) => IconView::Name(name),
        IconSource::Path(path) => IconView::Path(path),
        IconSource::Pixmap(Pixmap { width, height, argb }) => {
            IconView::Pixmap { width, height, argb }
        }
    }
}

fn menu_of(menu: PanMenu) -> MenuSnapshot {
    MenuSnapshot {
        parent_id: menu.parent_id,
        revision: menu.revision,
        items: menu
            .items
            .into_iter()
            .map(|i| MenuItem {
                id: i.id,
                item_type: match i.item_type {
                    breadpan::MenuItemType::Standard => MenuItemType::Standard,
                    breadpan::MenuItemType::Separator => MenuItemType::Separator,
                },
                label: i.label,
                raw_label: i.raw_label,
                enabled: i.enabled,
                visible: i.visible,
                children_display: i.children_display,
                toggle_type: i.toggle_type.map(|t| match t {
                    breadpan::ToggleType::Checkmark => ToggleType::Checkmark,
                    breadpan::ToggleType::Radio => ToggleType::Radio,
                }),
                toggle_state: i.toggle_state.map(|s| match s {
                    breadpan::ToggleState::Off => ToggleState::Off,
                    breadpan::ToggleState::On => ToggleState::On,
                    breadpan::ToggleState::Indeterminate => ToggleState::Indeterminate,
                }),
                icon_name: i.icon_name,
                icon_data: i.icon_data,
            })
            .collect(),
    }
}

pub fn outcome_of(outcome: PanOutcome) -> InteractionOutcome {
    match outcome {
        PanOutcome::Dispatched => InteractionOutcome::Dispatched,
        PanOutcome::Menu(menu) => InteractionOutcome::Menu(menu_of(menu)),
    }
}

// ── 交互与菜单执行（在专用线程上调用；阻塞的是该线程）──

pub fn interact(
    handle: &TrayHandle,
    item: &str,
    input: &crate::shared::protocol::InteractInput,
) -> Result<InteractionOutcome, breadpan::TrayError> {
    use crate::shared::protocol::InteractInput::*;
    let id = String::from(item);
    let outcome = match *input {
        Left { x, y } => handle.left_click_at(&id, x, y)?,
        Middle { x, y } => handle.middle_click_at(&id, x, y)?,
        Right { x, y } => handle.right_click_at(&id, x, y)?,
        ScrollUp { steps } => handle.scroll_up_by(&id, steps)?,
        ScrollDown { steps } => handle.scroll_down_by(&id, steps)?,
        ScrollLeft { steps } => handle.scroll_left_by(&id, steps)?,
        ScrollRight { steps } => handle.scroll_right_by(&id, steps)?,
    };
    Ok(outcome_of(outcome))
}

pub fn menu_read(handle: &TrayHandle, item: &str) -> Result<MenuSnapshot, breadpan::TrayError> {
    handle.menu(&String::from(item)).map(menu_of)
}

pub fn menu_expand(
    handle: &TrayHandle,
    item: &str,
    entry: i32,
) -> Result<MenuSnapshot, breadpan::TrayError> {
    handle.menu_expand(&String::from(item), entry).map(menu_of)
}

pub fn menu_activate(
    handle: &TrayHandle,
    item: &str,
    entry: i32,
) -> Result<InteractionOutcome, breadpan::TrayError> {
    handle.menu_activate(&String::from(item), entry).map(outcome_of)
}
