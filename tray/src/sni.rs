//! SNI (StatusNotifierItem) protocol helpers: constants, service string
//! parsing and property -> model conversion.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::{
    error::TrayError,
    model::{IconSource, Pixmap, ToolTip, TrayCategory, TrayItem, TrayStatus, TrayItemId},
};

pub(crate) const SNI_WATCHER: &str = "org.kde.StatusNotifierWatcher";
pub(crate) const SNI_WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub(crate) const SNI_WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";
pub(crate) const SNI_ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
pub(crate) const SNI_HOST_PREFIX: &str = "_org.kde.StatusNotifierHost.";
pub(crate) const MENU_IFACE: &str = "com.canonical.dbusmenu";

/// Parse an SNI service string into (bus name, object path).
///
/// Two forms exist: `"bus.name"` (implies `/StatusNotifierItem`) and
/// `"bus.name/object/path"`.
pub(crate) fn parse_service(service: &str) -> Option<(String, String)> {
    let service = service.trim();
    if service.is_empty() {
        return None;
    }
    match service.find('/') {
        Some(idx) => {
            let (bus, path) = service.split_at(idx);
            if bus.is_empty() || path.len() < 2 {
                return None;
            }
            Some((bus.to_string(), path.to_string()))
        }
        None => Some((service.to_string(), "/StatusNotifierItem".to_string())),
    }
}

/// Fetch all properties of an SNI item object.
pub(crate) fn get_item_properties(
    conn: &Connection,
    bus: &str,
    path: &str,
) -> Result<HashMap<String, OwnedValue>, TrayError> {
    let props = zbus::blocking::fdo::PropertiesProxy::new(
        conn,
        bus,
        ObjectPath::try_from(path)?,
    )?;
    Ok(props.get_all(SNI_ITEM_IFACE.try_into()?)?)
}

/// Assemble an item snapshot (and its menu path) from raw properties.
///
/// Missing properties degrade to defaults per SPEC §8; this function never
/// fails on partial data.
pub(crate) fn build_item(id: TrayItemId, props: &HashMap<String, OwnedValue>) -> TrayItemSnapshot {
    let category = props
        .get("Category")
        .and_then(as_string)
        .map(|s| TrayCategory::parse(&s))
        .unwrap_or_default();
    let status = props
        .get("Status")
        .and_then(as_string)
        .map(|s| TrayStatus::parse(&s))
        .unwrap_or(TrayStatus::Active);
    let title = non_empty(props.get("Title").and_then(as_string));
    let description = non_empty(props.get("Id").and_then(as_string));
    let window_id = props
        .get("WindowId")
        .and_then(as_i32)
        .unwrap_or_default();

    let icon_name = props
        .get("IconName")
        .and_then(as_string)
        .unwrap_or_default();
    let icon_pixmap = props.get("IconPixmap").and_then(parse_pixmap);
    let overlay = icon_pair(
        props.get("OverlayIconName").and_then(as_string),
        props.get("OverlayIconPixmap").and_then(parse_pixmap),
    );
    let attention = icon_pair(
        props.get("AttentionIconName").and_then(as_string),
        props.get("AttentionIconPixmap").and_then(parse_pixmap),
    );
    let tooltip = props.get("ToolTip").and_then(parse_tooltip);

    let menu_path = props
        .get("Menu")
        .and_then(as_object_path)
        .filter(|p| p != "/" && !p.is_empty());

    TrayItemSnapshot {
        item: TrayItem {
            id,
            category,
            status,
            title,
            description,
            window_id,
            icon: icon_pair(Some(icon_name), icon_pixmap).unwrap_or(IconSource::Name(String::new())),
            overlay_icon: overlay,
            attention_icon: attention,
            tooltip,
            has_menu: menu_path.is_some(),
        },
        menu_path,
    }
}

/// A parsed item snapshot plus the menu object path, if any.
pub(crate) struct TrayItemSnapshot {
    pub item: TrayItem,
    pub menu_path: Option<String>,
}

fn icon_pair(name: Option<String>, pixmap: Option<Pixmap>) -> Option<IconSource> {
    icon_source(name.unwrap_or_default(), pixmap)
}

/// Fixed icon resolution rule (SPEC §5.3): non-empty name wins, else the
/// largest pixmap, else an empty name placeholder.
fn icon_source(name: String, pixmap: Option<Pixmap>) -> Option<IconSource> {
    if !name.is_empty() {
        Some(IconSource::Name(name))
    } else {
        pixmap.map(IconSource::Pixmap)
    }
}

fn parse_pixmap(value: &OwnedValue) -> Option<Pixmap> {
    parse_pixmap_value(&Value::from(value.clone()))
}

fn parse_pixmap_value(value: &Value) -> Option<Pixmap> {
    let Value::Array(entries) = value else {
        return None;
    };
    let mut best: Option<Pixmap> = None;
    for i in 0..entries.len() {
        let Ok(Some(entry)) = entries.get::<Value>(i) else {
            continue;
        };
        let Value::Structure(fields) = entry else {
            continue;
        };
        let fields = fields.fields();
        if fields.len() != 3 {
            continue;
        }
        let (Some(w), Some(h), Some(data)) = (
            as_i32_value(&fields[0]),
            as_i32_value(&fields[1]),
            as_bytes(&fields[2]),
        ) else {
            continue;
        };
        if w <= 0 || h <= 0 {
            continue;
        }
        let (w, h) = (w as u32, h as u32);
        if data.len() < (w * h * 4) as usize {
            continue;
        }
        let pixmap = Pixmap {
            width: w,
            height: h,
            argb: data[..(w * h * 4) as usize].to_vec(),
        };
        if best.as_ref().is_none_or(|b| w * h > b.width * b.height) {
            best = Some(pixmap);
        }
    }
    best
}

fn parse_tooltip(value: &OwnedValue) -> Option<ToolTip> {
    let Value::Structure(fields) = Value::from(value.clone()) else {
        return None;
    };
    let fields = fields.fields();
    if fields.len() != 4 {
        return None;
    }
    let icon_name = as_string_value(&fields[0]).unwrap_or_default();
    let icon_pixmap = parse_pixmap_value(&fields[1]);
    let title = as_string_value(&fields[2]).unwrap_or_default();
    let description = as_string_value(&fields[3]).unwrap_or_default();
    if title.is_empty() && description.is_empty() {
        return None;
    }
    Some(ToolTip {
        icon: icon_source(icon_name, icon_pixmap),
        title,
        description,
    })
}


fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

pub(crate) fn as_string(value: &OwnedValue) -> Option<String> {
    String::try_from(value.clone()).ok()
}

fn as_i32(value: &OwnedValue) -> Option<i32> {
    i32::try_from(value.clone()).ok()
}

fn as_object_path(value: &OwnedValue) -> Option<String> {
    if let Ok(path) = ObjectPath::try_from(value.clone()) {
        return Some(path.to_string());
    }
    // Some providers smuggle the path in as a plain string.
    as_string(value).filter(|s| s.starts_with('/'))
}

fn as_string_value(value: &Value) -> Option<String> {
    match value {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

fn as_i32_value(value: &Value) -> Option<i32> {
    match value {
        Value::I32(v) => Some(*v),
        Value::U32(v) => i32::try_from(*v).ok(),
        _ => None,
    }
}

fn as_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                match arr.get::<u8>(i) {
                    Ok(Some(b)) => out.push(b),
                    _ => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn parses_plain_service() {
        let parsed = parse_service("org.example.App").unwrap();
        assert_eq!(parsed.0, "org.example.App");
        assert_eq!(parsed.1, "/StatusNotifierItem");
    }

    #[test]
    fn parses_unique_name_with_path() {
        let parsed = parse_service(":1.42/org/kde/item").unwrap();
        assert_eq!(parsed.0, ":1.42");
        assert_eq!(parsed.1, "/org/kde/item");
    }

    #[test]
    fn rejects_bad_service() {
        assert!(parse_service("").is_none());
        assert!(parse_service("/only/path").is_none());
        assert!(parse_service("bus/").is_none());
    }

    #[test]
    fn icon_rule_prefers_name_then_pixmap() {
        let p = Pixmap {
            width: 1,
            height: 1,
            argb: vec![0, 0, 0, 0],
        };
        assert_eq!(
            icon_source("foo".into(), Some(p.clone())),
            Some(IconSource::Name("foo".into()))
        );
        assert_eq!(icon_source(String::new(), Some(p.clone())), Some(IconSource::Pixmap(p)));
        assert_eq!(icon_source(String::new(), None), None);
    }

    #[test]
    fn builds_item_with_defaults() {
        let id = TrayItemId {
            bus_name: ":1.1".into(),
            path: "/StatusNotifierItem".into(),
        };
        let snapshot = build_item(id.clone(), &HashMap::new());
        assert_eq!(snapshot.item.id, id);
        assert_eq!(snapshot.item.status, TrayStatus::Active);
        assert_eq!(snapshot.item.category, TrayCategory::Application);
        assert!(!snapshot.item.has_menu);
        assert!(snapshot.menu_path.is_none());
    }

    #[test]
    fn builds_item_with_menu_and_pixmap() {
        use zbus::zvariant::Structure;
        let id = TrayItemId {
            bus_name: "org.a".into(),
            path: "/item".into(),
        };
        let owned = |v: Value| v.try_to_owned().unwrap();
        let pixmap_entry = Value::Structure(Structure::from((
            4i32,
            2i32,
            vec![1u8; 32],
        )));
        let mut props = HashMap::new();
        props.insert("Menu".into(), owned(Value::from("/menu")));
        props.insert(
            "IconPixmap".into(),
            owned(Value::from(vec![pixmap_entry])),
        );
        let snapshot = build_item(id, &props);
        assert!(snapshot.item.has_menu);
        assert_eq!(snapshot.menu_path.as_deref(), Some("/menu"));
        match snapshot.item.icon {
            IconSource::Pixmap(p) => {
                assert_eq!((p.width, p.height), (4, 2));
                assert_eq!(p.argb.len(), 32);
            }
            other => panic!("expected pixmap, got {other:?}"),
        }
    }
}
