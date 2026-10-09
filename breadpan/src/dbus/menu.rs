//! dbusmenu 现场读取（零缓存·逐层懒加载）。

use std::collections::HashMap;
use std::ops::Deref;

use zbus::zvariant::{OwnedValue, Value};

use crate::dbus::bus::BusCenter;
use crate::dbus::DBUSMENU_IFACE;
use crate::tracker::ItemEntry;
use crate::{MenuItem, MenuItemType, MenuSnapshot, ToggleState, ToggleType, TrayError};

/// GetLayout 布局节点：(id, 属性集, 子树)。
pub(crate) type NodeRaw = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// 现场读取单层菜单（Menu 流程：interact 路由与 menu()/menu_expand() 共用）。
///
/// AboutToShow(parent) 前置（应答异常视为未变更）→ GetLayout(parent, recursionDepth=1)
/// → GetGroupProperties → 组装该层 MenuSnapshot。不落任何缓存。
pub(crate) async fn read_layer(
    bus: &BusCenter,
    entry: &ItemEntry,
    parent: i32,
) -> Result<MenuSnapshot, TrayError> {
    let Some(menu_path) = entry.menu.clone() else {
        return Err(TrayError::ItemNotFound(format!(
            "{} has no dbusmenu",
            entry.service
        )));
    };
    let service = entry.service.clone();

    // AboutToShow：常规前置，告知弹出在即，触发 Item 惰性构建/刷新；应答仅作诊断。
    let _: Result<bool, TrayError> = bus
        .call(&service, &menu_path, DBUSMENU_IFACE, "AboutToShow", &parent)
        .await;

    // GetLayout：懒加载，每次一层（recursionDepth = 1）。
    let (revision, node): (u32, NodeRaw) = bus
        .call(
            &service,
            &menu_path,
            DBUSMENU_IFACE,
            "GetLayout",
            &(parent, 1, Vec::<String>::new()),
        )
        .await?;

    // 子层节点（depth=1 时子树为空数组）
    let children: Vec<NodeRaw> = node
        .2
        .into_iter()
        .filter_map(|v| Value::from(v).try_into().ok())
        .collect();

    // GetGroupProperties：条目属性查询（布局属性为底、组属性覆盖）。
    let ids: Vec<i32> = children.iter().map(|(id, _, _)| *id).collect();
    let group: Vec<(i32, HashMap<String, OwnedValue>)> = if ids.is_empty() {
        Vec::new()
    } else {
        bus.call(
            &service,
            &menu_path,
            DBUSMENU_IFACE,
            "GetGroupProperties",
            &(ids, Vec::<String>::new()),
        )
        .await?
    };

    Ok(assemble(parent, revision, &children, &group))
}

/// 组装该层快照：助记符解析 + raw 逃生门 + 默认值补全。
pub(crate) fn assemble(
    parent_id: i32,
    revision: u32,
    children: &[NodeRaw],
    group: &[(i32, HashMap<String, OwnedValue>)],
) -> MenuSnapshot {
    let mut items = Vec::with_capacity(children.len());
    for (id, props, _) in children {
        let mut merged = props.clone();
        if let Some((_, group_props)) = group.iter().find(|(gid, _)| gid == id) {
            for (k, v) in group_props {
                merged.insert(k.clone(), v.clone());
            }
        }
        items.push(menu_item_from_props(*id, &merged));
    }
    MenuSnapshot {
        parent_id,
        revision,
        items,
    }
}

/// 条目属性 → MenuItem（官方缺省：仅非默认值的属性才会上报，组装时按默认值补全）。
pub(crate) fn menu_item_from_props(id: i32, p: &HashMap<String, OwnedValue>) -> MenuItem {
    let raw_label = str_of(p, "label").unwrap_or_default().to_string();
    MenuItem {
        id,
        item_type: if str_of(p, "type") == Some("separator") {
            MenuItemType::Separator
        } else {
            // 含缺省 "standard" 与 vendor x-* 值
            MenuItemType::Standard
        },
        label: parse_mnemonic(&raw_label),
        raw_label,
        enabled: bool_of(p, "enabled").unwrap_or(true),
        visible: bool_of(p, "visible").unwrap_or(true),
        children_display: str_of(p, "children-display") == Some("submenu"),
        toggle_type: match str_of(p, "toggle-type") {
            Some("checkmark") => Some(ToggleType::Checkmark),
            Some("radio") => Some(ToggleType::Radio),
            _ => None,
        },
        toggle_state: int_of(p, "toggle-state").map(|n| match n {
            0 => ToggleState::Off,
            1 => ToggleState::On,
            _ => ToggleState::Indeterminate,
        }),
        icon_name: str_of(p, "icon-name")
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        icon_data: p
            .get("icon-data")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| v.try_into().ok())
            .filter(|b: &Vec<u8>| !b.is_empty()),
    }
}

/// 助记符解析：`__` 显示为 `_`；剩余 `_` 不显示（第一个剩余 `_` 标记访问键——
/// 访问键字符本身保留在显示文本中，其定位信息由 raw_label 逃生门承载）。
pub(crate) fn parse_mnemonic(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            let mut lookahead = chars.clone();
            match lookahead.next() {
                Some('_') => {
                    out.push('_');
                    chars.next(); // 吞掉第二个 '_'
                }
                Some(_) => { /* 访问键标记：跳过，不显示 */ }
                None => { /* 末尾孤立下划线：不显示 */ }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn str_of<'a>(p: &'a HashMap<String, OwnedValue>, key: &str) -> Option<&'a str> {
    match p.get(key)?.deref() {
        Value::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

fn bool_of(p: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    p.get(key)?.try_clone().ok().and_then(|v| v.try_into().ok())
}

fn int_of(p: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
    p.get(key)?.try_clone().ok().and_then(|v| v.try_into().ok())
}
