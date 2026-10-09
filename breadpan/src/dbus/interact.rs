//! 语义输入路由（ARCHITECTURE §2.7）+ InteractionResult 回执。

use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::dbus::bus::BusCenter;
use crate::dbus::menu;
use crate::dbus::{DBUSMENU_IFACE, ITEM_IFACE_FD, ITEM_IFACE_KDE};
use crate::events::{InteractionOp, TrayEvent};
use crate::tracker::ItemEntry;
use crate::{InteractionOutcome, TrayError};

/// 路由前归一的语义输入（_at/_by 变体已在 Handle 层折入）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Input {
    Left(i32, i32),
    Middle(i32, i32),
    Right(i32, i32),
    ScrollUp(i32),
    ScrollDown(i32),
    ScrollLeft(i32),
    ScrollRight(i32),
}

/// 按路由表分发（依据登记簿记 menu_only / menu，零查询）：
/// - left_click：`menu_only` 且 DBusMenu 在册 → Menu 流程；否则 Activate(x,y)
/// - middle_click：SecondaryActivate(x,y)
/// - right_click：DBusMenu 在册 → Menu 流程；否则 ContextMenu(x,y)（SNI 原生 fallback）
/// - scroll_*：Scroll(∓steps, vertical / horizontal)，符号约定 up/left 为负
///
/// Menu 流程现场取数并同步返回快照；VOID 调用受理即返回（回执经事件流）。
pub(crate) async fn route(
    bus: &BusCenter,
    id: &str,
    input: Input,
) -> Result<InteractionOutcome, TrayError> {
    let entry = bus.entry_of(&id.to_string())?;

    let menu_hit = match input {
        Input::Left(..) => entry.menu_only && entry.menu.is_some(),
        Input::Right(..) => entry.menu.is_some(),
        _ => false,
    };
    if menu_hit {
        let snapshot = menu::read_layer(bus, &entry, 0).await?;
        return Ok(InteractionOutcome::Menu(snapshot));
    }

    let op = match input {
        Input::Left(..) => InteractionOp::Activate,
        Input::Middle(..) => InteractionOp::SecondaryActivate,
        Input::Right(..) => InteractionOp::ContextMenu,
        Input::ScrollUp(..)
        | Input::ScrollDown(..)
        | Input::ScrollLeft(..)
        | Input::ScrollRight(..) => InteractionOp::Scroll,
    };
    dispatch_void(
        bus,
        entry,
        id.to_string(),
        op,
        move |bus, entry| async move {
            match input {
                Input::Left(x, y) => invoke_xy(&bus, &entry, "Activate", x, y).await,
                Input::Middle(x, y) => invoke_xy(&bus, &entry, "SecondaryActivate", x, y).await,
                Input::Right(x, y) => invoke_xy(&bus, &entry, "ContextMenu", x, y).await,
                Input::ScrollUp(n) => invoke_scroll(&bus, &entry, -n, "vertical").await,
                Input::ScrollDown(n) => invoke_scroll(&bus, &entry, n, "vertical").await,
                Input::ScrollLeft(n) => invoke_scroll(&bus, &entry, -n, "horizontal").await,
                Input::ScrollRight(n) => invoke_scroll(&bus, &entry, n, "horizontal").await,
            }
        },
    );

    Ok(InteractionOutcome::Dispatched)
}

/// 延续方法：提交 Outcome::Menu 弹出中的条目点击（dbusmenu Event("clicked")）。
/// 受理即返回 Ok；调用结果经 InteractionResult 事件交付。
pub(crate) fn menu_activate(bus: &BusCenter, id: &str, menu_item_id: i32) -> Result<(), TrayError> {
    let entry = bus.entry_of(&id.to_string())?;
    let Some(menu_path) = entry.menu.clone() else {
        return Err(TrayError::ItemNotFound(format!(
            "{} has no dbusmenu",
            entry.service
        )));
    };
    dispatch_void(
        bus,
        entry,
        id.to_string(),
        InteractionOp::MenuActivate,
        move |bus, entry| async move {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u32)
                .unwrap_or(0);
            bus.call::<(), _>(
                &entry.service,
                &menu_path,
                DBUSMENU_IFACE,
                "Event",
                &(menu_item_id, "clicked", zbus::zvariant::Value::I32(0), ts),
            )
            .await
        },
    );
    Ok(())
}

/// 派发一次 VOID 交互（运行时后台执行，回执入队 InteractionResult）。
fn dispatch_void<F, Fut>(bus: &BusCenter, entry: ItemEntry, id: String, op: InteractionOp, f: F)
where
    F: FnOnce(BusCenter, ItemEntry) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), TrayError>> + Send + 'static,
{
    let bus2 = bus.clone();
    bus.spawn(async move {
        let result = f(bus2.clone(), entry).await.map_err(|e| e.to_string());
        bus2.events
            .send(TrayEvent::InteractionResult { id, op, result });
    });
}

/// SNI 定位方法（Activate / SecondaryActivate / ContextMenu）：org.kde 前缀优先，
/// 失败回退 org.freedesktop 前缀。
async fn invoke_xy(
    bus: &BusCenter,
    entry: &ItemEntry,
    method: &str,
    x: i32,
    y: i32,
) -> Result<(), TrayError> {
    match bus
        .call::<(), _>(&entry.service, &entry.path, ITEM_IFACE_KDE, method, &(x, y))
        .await
    {
        Ok(()) => Ok(()),
        Err(_) => {
            bus.call::<(), _>(&entry.service, &entry.path, ITEM_IFACE_FD, method, &(x, y))
                .await
        }
    }
}

/// Scroll(delta, orientation)：同前缀回退。
async fn invoke_scroll(
    bus: &BusCenter,
    entry: &ItemEntry,
    delta: i32,
    orient: &str,
) -> Result<(), TrayError> {
    match bus
        .call::<(), _>(
            &entry.service,
            &entry.path,
            ITEM_IFACE_KDE,
            "Scroll",
            &(delta, orient),
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(_) => {
            bus.call::<(), _>(
                &entry.service,
                &entry.path,
                ITEM_IFACE_FD,
                "Scroll",
                &(delta, orient),
            )
            .await
        }
    }
}
