//! DBusMenu client (`com.canonical.dbusmenu`): read-only menu snapshots.

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::{error::TrayError, sni::MENU_IFACE};

/// A read-only snapshot of a tray item's menu.
///
/// Fetch a fresh snapshot whenever a [`TrayEvent::MenuChanged`](crate::TrayEvent::MenuChanged)
/// arrives; the library re-pulls affected subtrees and hands out complete,
/// consistent snapshots (consumers never touch the dbusmenu protocol).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MenuSnapshot {
    /// Menu revision as reported by the item.
    pub revision: u32,
    /// Top-level entries, truncated at the configured maximum depth.
    pub root: Vec<MenuItem>,
}

/// One entry of a menu snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MenuItem {
    /// dbusmenu item id (needed for activation).
    pub id: i32,
    /// Kind of the entry.
    pub kind: MenuItemKind,
    /// Label with mnemonic underscores resolved (`_File` -> `File`).
    pub label: String,
    /// Whether the entry is enabled (false = greyed out).
    pub enabled: bool,
    /// Whether the entry is visible.
    pub visible: bool,
    /// Check/radio state, if the entry is toggleable.
    pub toggled: Option<bool>,
    /// Optional icon (theme name).
    pub icon: Option<String>,
    /// Child entries (empty beyond the configured depth).
    pub children: Vec<MenuItem>,
}

/// Kind of a menu entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MenuItemKind {
    /// Regular action entry.
    #[default]
    Standard,
    /// Visual separator.
    Separator,
    /// Container with children.
    Submenu,
}

/// Fetch a menu layout snapshot over DBusMenu.
pub(crate) fn fetch_menu(
    conn: &Connection,
    bus: &str,
    path: &str,
    max_depth: u8,
) -> Result<MenuSnapshot, TrayError> {
    let proxy = zbus::blocking::Proxy::new(
        conn,
        bus,
        ObjectPath::try_from(path)?,
        MENU_IFACE,
    )?;
    let (revision, layout): (u32, OwnedValue) =
        proxy.call("GetLayout", &(0i32, max_depth as i32, Vec::<String>::new()))?;
    let value = Value::from(layout);
    // Layout root is itself a node: (id, props, children).
    let root = match &value {
        Value::Structure(fields) if fields.fields().len() == 3 => {
            parse_children(&fields.fields()[2], max_depth as i32)
        }
        _ => Vec::new(),
    };
    Ok(MenuSnapshot { revision, root })
}

/// Parse the `av` items of a layout node into menu entries.
fn parse_children(layout: &Value, depth: i32) -> Vec<MenuItem> {
    let mut out = Vec::new();
    if depth < 0 {
        return out;
    }
    let Value::Array(items) = layout else {
        return out;
    };
    for i in 0..items.len() {
        let Ok(Some(Value::Value(inner))) = items.get::<Value>(i) else {
            continue;
        };
        if let Some(entry) = parse_node(&inner, depth) {
            out.push(entry);
        }
    }
    out
}

/// Parse one `(ia{sv}av)` layout node.
fn parse_node(value: &Value, depth: i32) -> Option<MenuItem> {
    let Value::Structure(fields) = value else {
        return None;
    };
    let fields = fields.fields();
    if fields.len() != 3 {
        return None;
    }
    let id = match &fields[0] {
        Value::I32(id) => *id,
        _ => return None,
    };
    let props = prop_lookup(&fields[1]);
    let kind = if prop_str(&props, "type").as_deref() == Some("separator") {
        MenuItemKind::Separator
    } else if prop_str(&props, "children-display").as_deref() == Some("submenu") {
        MenuItemKind::Submenu
    } else {
        MenuItemKind::Standard
    };
    let label = parse_mnemonic(&prop_str(&props, "label").unwrap_or_default());
    let enabled = prop_bool(&props, "enabled").unwrap_or(true);
    let visible = prop_bool(&props, "visible").unwrap_or(true);
    let toggle_type = prop_str(&props, "toggle-type").unwrap_or_default();
    let toggle_state = prop_i32(&props, "toggle-state");
    let toggled = if toggle_type.is_empty() {
        None
    } else {
        toggle_state.filter(|s| *s >= 0).map(|s| s == 1)
    };
    let children = if depth > 0 {
        parse_children(&fields[2], depth - 1)
    } else {
        Vec::new()
    };
    Some(MenuItem {
        id,
        kind,
        label,
        enabled,
        visible,
        toggled,
        icon: prop_str(&props, "icon-name"),
        children,
    })
}

type PropMap = Vec<(String, PropValue)>;

#[derive(Debug, Clone)]
enum PropValue {
    Str(String),
    Bool(bool),
    Int(i32),
}

fn prop_lookup(value: &Value) -> PropMap {
    let mut out = Vec::new();
    if let Value::Dict(dict) = value {
        for (k, v) in dict.iter() {
            let key = match k {
                Value::Str(s) => s.to_string(),
                _ => continue,
            };
            // Property values are variants (`a{sv}`): unwrap them first.
            let v = match v {
                Value::Value(inner) => inner.as_ref(),
                other => other,
            };
            let prop = match v {
                Value::Str(s) => PropValue::Str(s.to_string()),
                Value::Bool(b) => PropValue::Bool(*b),
                Value::I32(i) => PropValue::Int(*i),
                Value::U32(u) => PropValue::Int(*u as i32),
                _ => continue,
            };
            out.push((key, prop));
        }
    }
    out
}

fn prop_str(props: &PropMap, key: &str) -> Option<String> {
    props.iter().find_map(|(k, v)| match v {
        PropValue::Str(s) if k == key => Some(s.clone()),
        _ => None,
    })
}

fn prop_bool(props: &PropMap, key: &str) -> Option<bool> {
    props.iter().find_map(|(k, v)| match v {
        PropValue::Bool(b) if k == key => Some(*b),
        _ => None,
    })
}

fn prop_i32(props: &PropMap, key: &str) -> Option<i32> {
    props.iter().find_map(|(k, v)| match v {
        PropValue::Int(i) if k == key => Some(*i),
        _ => None,
    })
}

/// Resolve dbusmenu label mnemonics: `__` is a literal underscore, `_X`
/// marks the mnemonic `X` (dropped in plain-text labels).
pub(crate) fn parse_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            match chars.next() {
                Some('_') => out.push('_'),
                Some(next) => out.push(next),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_are_resolved() {
        assert_eq!(parse_mnemonic("_File"), "File");
        assert_eq!(parse_mnemonic("Open__System"), "Open_System");
        assert_eq!(parse_mnemonic("trailing_"), "trailing");
        assert_eq!(parse_mnemonic("plain"), "plain");
    }

    #[test]
    fn parses_nested_layout() {
        use zbus::zvariant::Structure;
        use std::collections::HashMap;
        let node = |id: i32,
                    props: HashMap<String, Value<'static>>,
                    children: Vec<Value<'static>>|
         -> Value<'static> {
            Value::Value(Box::new(Value::Structure(Structure::from((
                id, props, children,
            )))))
        };
        // Root: submenu with a standard entry and a separator.
        let std_item = node(2, HashMap::new(), Vec::new());
        let sep_item = node(
            3,
            [("type".to_string(), Value::from("separator"))].into(),
            Vec::new(),
        );
        let root = Value::Structure(Structure::from((
            0i32,
            HashMap::from([
                ("label".to_string(), Value::from("_Root")),
                ("children-display".to_string(), Value::from("submenu")),
            ]),
            vec![std_item, sep_item],
        )));

        let root_node = parse_node(&root, 8).expect("root node parses");
        assert_eq!(root_node.label, "Root");
        assert_eq!(root_node.kind, MenuItemKind::Submenu);
        assert_eq!(root_node.children.len(), 2);
        assert_eq!(root_node.children[0].kind, MenuItemKind::Standard);
        assert_eq!(root_node.children[1].kind, MenuItemKind::Separator);
    }

    #[test]
    fn toggled_follows_toggle_state() {
        use zbus::zvariant::Structure;
        use std::collections::HashMap;
        let make = |toggle_type: Option<&str>, state: i32| -> Value {
            let mut props: HashMap<String, Value> = HashMap::new();
            if let Some(t) = toggle_type {
                props.insert("toggle-type".to_string(), Value::from(t.to_string()));
                props.insert("toggle-state".to_string(), Value::from(state));
            }
            Value::Structure(Structure::from((1i32, props, Vec::<Value>::new())))
        };
        let node = parse_node(&make(Some("checkmark"), 1), 2).unwrap();
        assert_eq!(node.toggled, Some(true));
        let node = parse_node(&make(Some("radio"), 0), 2).unwrap();
        assert_eq!(node.toggled, Some(false));
        let node = parse_node(&make(Some("checkmark"), -1), 2).unwrap();
        assert_eq!(node.toggled, None);
        let node = parse_node(&make(None, 1), 2).unwrap();
        assert_eq!(node.toggled, None);
    }
}
