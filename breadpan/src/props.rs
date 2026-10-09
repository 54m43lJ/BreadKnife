//! 属性即时读取（zvariant/serde → 原生结构）。
//!
//! `fetch_item` 是 items()/item()/Snapshot 的统一取数路径：DBus 即时读取、零缓存、交付即最新。

use std::collections::HashMap;
use std::ops::Deref;

use zbus::zvariant::{OwnedValue, Value};

use crate::dbus::{bus::BusCenter, ITEM_IFACE_FD, ITEM_IFACE_KDE, PROPS_IFACE};
use crate::tracker::ItemEntry;
use crate::{IconSource, Pixmap, ToolTip, TrayCategory, TrayError, TrayItem, TrayStatus};

/// a(iiay) 的元组形态。
pub(crate) type PixmapTuple = (i32, i32, Vec<u8>);

/// 现场读取单个 Item 全部官方属性，反序列化为 TrayItem。
pub(crate) async fn fetch_item(bus: &BusCenter, entry: &ItemEntry) -> Result<TrayItem, TrayError> {
    let props = read_all(bus, &entry.service, &entry.path).await?;
    Ok(tray_item_from_props(entry, &props))
}

/// GetAll（org.kde 前缀优先，失败回退 org.freedesktop 前缀）。
pub(crate) async fn read_all(
    bus: &BusCenter,
    service: &str,
    path: &str,
) -> Result<HashMap<String, OwnedValue>, TrayError> {
    let kde = ITEM_IFACE_KDE.to_string();
    match bus
        .call::<HashMap<String, OwnedValue>, _>(service, path, PROPS_IFACE, "GetAll", &kde)
        .await
    {
        Ok(props) => Ok(props),
        Err(_) => {
            let fd = ITEM_IFACE_FD.to_string();
            bus.call::<HashMap<String, OwnedValue>, _>(service, path, PROPS_IFACE, "GetAll", &fd)
                .await
        }
    }
}

/// 注册期路由探测：Menu 属性（对象路径）与 ItemIsMenu。读取失败按无菜单/非菜单处理。
pub(crate) async fn probe_routing(
    bus: &BusCenter,
    service: &str,
    path: &str,
) -> (Option<String>, bool) {
    let menu = read_prop(bus, service, path, "Menu")
        .await
        .ok()
        .flatten()
        .and_then(|v| {
            let p: zbus::zvariant::ObjectPath<'_> = v.try_into().ok()?;
            Some(p.to_string())
        });
    let menu_only = read_prop(bus, service, path, "ItemIsMenu")
        .await
        .ok()
        .flatten()
        .and_then(|v| as_bool(&v))
        .unwrap_or(false);
    (menu, menu_only)
}

/// 单属性读取（Get）：读取失败 → None。
async fn read_prop(
    bus: &BusCenter,
    service: &str,
    path: &str,
    name: &str,
) -> Result<Option<OwnedValue>, TrayError> {
    let body = (ITEM_IFACE_KDE, name);
    let v: OwnedValue = bus.call(service, path, PROPS_IFACE, "Get", &body).await?;
    Ok(Some(v))
}

/// 属性集 → TrayItem（ARCHITECTURE §3.3 字段表）。
pub(crate) fn tray_item_from_props(
    entry: &ItemEntry,
    props: &HashMap<String, OwnedValue>,
) -> TrayItem {
    let str_of = |key: &str| props.get(key).and_then(as_string);
    let pixmap_of = |key: &str| props.get(key).and_then(as_pixmaps);

    let icon_name = str_of("IconName");
    let icon_pixmaps = pixmap_of("IconPixmap");
    let overlay_name = str_of("OverlayIconName");
    let overlay_pixmaps = pixmap_of("OverlayIconPixmap");
    let attention_name = str_of("AttentionIconName");
    let attention_pixmaps = pixmap_of("AttentionIconPixmap");

    TrayItem {
        id: entry.service.clone(),
        category: str_of("Category")
            .and_then(|s| category_from(&s))
            .unwrap_or(TrayCategory::ApplicationStatus),
        status: str_of("Status")
            .and_then(|s| status_from(&s))
            .unwrap_or(TrayStatus::Passive),
        alive: entry.alive,
        ident: str_of("Id").unwrap_or_default(),
        title: str_of("Title").filter(|s| !s.is_empty()),
        is_menu_only: props.get("ItemIsMenu").and_then(as_bool).unwrap_or(false),
        window_id: props.get("WindowId").and_then(as_u32).unwrap_or(0),
        icon: icon_source(icon_name.as_deref(), icon_pixmaps.as_deref()),
        overlay_icon: optional_icon(overlay_name.as_deref(), overlay_pixmaps.as_deref()),
        attention_icon: optional_icon(attention_name.as_deref(), attention_pixmaps.as_deref()),
        tooltip: props.get("ToolTip").and_then(tooltip_from),
        menu: props
            .get("Menu")
            .and_then(|v| as_object_path(v).or_else(|| as_string(v)))
            .filter(|s| !s.is_empty()),
    }
}

fn category_from(s: &str) -> Option<TrayCategory> {
    match s {
        "Communications" => Some(TrayCategory::Communications),
        "SystemServices" => Some(TrayCategory::SystemServices),
        "Hardware" => Some(TrayCategory::Hardware),
        "ApplicationStatus" => Some(TrayCategory::ApplicationStatus),
        _ => None,
    }
}

fn status_from(s: &str) -> Option<TrayStatus> {
    match s {
        "Active" => Some(TrayStatus::Active),
        "NeedsAttention" => Some(TrayStatus::NeedsAttention),
        "Passive" => Some(TrayStatus::Passive),
        _ => None,
    }
}

/// 二选一规则固定（SNI §3.1.6）：Name 非空 → Name，否则 → Pixmap（双空 → 空位图）。
pub(crate) fn icon_source(name: Option<&str>, pixmaps: Option<&[PixmapTuple]>) -> IconSource {
    if let Some(name) = name {
        if !name.is_empty() {
            return IconSource::Name(name.to_string());
        }
    }
    IconSource::Pixmap(pick_pixmap(pixmaps))
}

fn optional_icon(name: Option<&str>, pixmaps: Option<&[PixmapTuple]>) -> Option<IconSource> {
    let has_name = name.is_some_and(|s| !s.is_empty());
    let has_pixmap = pixmaps.is_some_and(|p| !p.is_empty());
    if !has_name && !has_pixmap {
        return None;
    }
    Some(icon_source(name, pixmaps))
}

/// 取面积最大的一张位图（多分辨率交付时的确定性选择）。
pub(crate) fn pick_pixmap(pixmaps: Option<&[PixmapTuple]>) -> Pixmap {
    let best = pixmaps
        .and_then(|list| {
            list.iter()
                .filter(|(w, h, _)| *w > 0 && *h > 0)
                .max_by_key(|(w, h, _)| (*w as u64) * (*h as u64))
        })
        .map(|(w, h, bytes)| (*w as u32, *h as u32, bytes.clone()));
    match best {
        Some((width, height, argb)) => Pixmap {
            width,
            height,
            argb,
        },
        None => Pixmap {
            width: 0,
            height: 0,
            argb: Vec::new(),
        },
    }
}

/// ToolTip 四元组 (STRING, a(iiay), STRING, STRING) 的映射。
fn tooltip_from(v: &OwnedValue) -> Option<ToolTip> {
    let raw: (String, Vec<PixmapTuple>, String, String) =
        Value::from(v.try_clone().ok()?).try_into().ok()?;
    let icon = optional_icon(Some(raw.0.as_str()), Some(&raw.1));
    Some(ToolTip {
        icon,
        title: raw.2,
        description: raw.3,
    })
}

// ── zvariant 宽容取值（规范外的类型波动不 panic、按缺省处理）──

fn as_string(v: &OwnedValue) -> Option<String> {
    v.try_clone().ok().and_then(|v| v.try_into().ok())
}

fn as_bool(v: &OwnedValue) -> Option<bool> {
    v.try_clone().ok().and_then(|v| v.try_into().ok())
}

fn as_u32(v: &OwnedValue) -> Option<u32> {
    match v.deref() {
        Value::U32(n) => Some(*n),
        Value::I32(n) => u32::try_from(*n).ok(),
        _ => None,
    }
}

fn as_object_path(v: &OwnedValue) -> Option<String> {
    let path: zbus::zvariant::ObjectPath<'static> = v.try_clone().ok()?.try_into().ok()?;
    Some(path.to_string())
}

fn as_pixmaps(v: &OwnedValue) -> Option<Vec<PixmapTuple>> {
    v.try_clone().ok().and_then(|v| v.try_into().ok())
}
