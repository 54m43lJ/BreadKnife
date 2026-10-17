//! Item 登记簿记（最小集）：报到 / 反向发现 / 换主重绑定 / 死亡清理。
//!
//! 只存唯一名 + 对象路径 + 存活字段（alive）+ DBusMenu 支持/路径 + 整项即菜单标记；
//! SNI 详情不经手、不落册，由消费方现场读取（零详情缓存）。

use std::collections::{HashMap, HashSet};

use crate::TrayItemId;

/// 单个 Item 的登记条目（最小集）。
#[derive(Debug, Clone)]
pub(crate) struct ItemEntry {
    /// 总线服务名（= TrayItemId）。
    pub service: String,
    /// 该服务名的当前属主唯一名（信号按发送者唯一名匹配）。
    pub unique: String,
    /// Item 对象路径（交互寻址用）。
    pub path: String,
    /// 最近一次存活探测结论（报到时置 true，此后由周期探测覆盖）。
    pub alive: bool,
    /// dbusmenu 对象路径（注册时探测 Menu 属性；None = 无 DBusMenu，右键 fallback）。
    pub menu: Option<String>,
    /// 整项即菜单（左键路由依据；注册时探测，随变更信号就地更新）。
    pub menu_only: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ItemTracker {
    items: HashMap<TrayItemId, ItemEntry>,
}

/// upsert 结果：新条目 / 原地刷新 / id 升级（CODE §4.3.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Upsert {
    Inserted,
    Refreshed,
    Renamed { old: TrayItemId, new: TrayItemId },
}

impl ItemTracker {
    /// 报到入册：双注册并入的落点——字段匹配两轮 lookup 自愈并入
    /// （同一对象/连接不允许双条目）：①注册名 == service ②发送者唯一名 == unique；
    /// 命中 → 原地刷新（path/menu/menu_only/alive 以最新报到为准），两轮未命中 → 新建。
    /// 临时唯一名 id 被知名名报到命中且属主唯一名不变 → id 升级（重键）。
    pub(crate) fn upsert(&mut self, entry: ItemEntry) -> Upsert {
        // ① 注册名 == service（知名名同名重启竞态；唯一名形态 == 临时条目）
        if let Some(existing) = self.items.get_mut(&entry.service) {
            existing.unique = entry.unique;
            existing.path = entry.path;
            existing.alive = true;
            existing.menu = entry.menu;
            existing.menu_only = entry.menu_only;
            return Upsert::Refreshed;
        }
        // ② 发送者唯一名 == unique（同连接先前以唯一名临时注册）
        let via_unique = self
            .items
            .iter()
            .find(|(_, e)| e.unique == entry.unique)
            .map(|(id, _)| id.clone());
        if let Some(old_id) = via_unique {
            if old_id.starts_with(':') && !entry.service.starts_with(':') {
                // 临时唯一名 → 知名名：id 升级（重键，属主唯一名不变）
                self.items.remove(&old_id);
                self.items.insert(entry.service.clone(), entry.clone());
                return Upsert::Renamed {
                    old: old_id,
                    new: entry.service,
                };
            }
            // 已在册的知名名 id 被重复报到：原地刷新，id 不变
            let existing = self.items.get_mut(&old_id).unwrap();
            existing.unique = entry.unique;
            existing.path = entry.path;
            existing.alive = true;
            existing.menu = entry.menu;
            existing.menu_only = entry.menu_only;
            return Upsert::Refreshed;
        }
        self.items.insert(entry.service.clone(), entry);
        Upsert::Inserted
    }

    /// 同名换主：属主唯一名重绑定（条目保留、id 不变；不在册 → false，CODE §4.3.2）。
    pub(crate) fn rebind_owner(&mut self, name: &str, new_owner: &str) -> bool {
        match self.items.get_mut(name) {
            Some(entry) => {
                entry.unique = new_owner.to_string();
                true
            }
            None => false,
        }
    }

    /// 死亡清理：移除服务名或属主唯一名命中的全部条目，返回被移除者。
    pub(crate) fn unregister(&mut self, name: &str) -> Vec<ItemEntry> {
        let matches: Vec<TrayItemId> = self
            .items
            .iter()
            .filter(|(_, e)| e.service == name || e.unique == name)
            .map(|(id, _)| id.clone())
            .collect();
        matches
            .into_iter()
            .filter_map(|id| self.items.remove(&id))
            .collect()
    }

    /// 清空簿记（Displaced / 停机路径），返回被清空者。
    pub(crate) fn clear(&mut self) -> Vec<ItemEntry> {
        self.items.drain().map(|(_, e)| e).collect()
    }

    pub(crate) fn get(&self, id: &TrayItemId) -> Option<ItemEntry> {
        self.items.get(id).cloned()
    }

    pub(crate) fn all(&self) -> Vec<ItemEntry> {
        self.items.values().cloned().collect()
    }

    pub(crate) fn ids(&self) -> HashSet<TrayItemId> {
        self.items.keys().cloned().collect()
    }

    /// 存活结论（登记簿记的 alive 字段，零 DBus 流量）；不在册 → None。
    pub(crate) fn is_alive(&self, id: &TrayItemId) -> Option<bool> {
        self.items.get(id).map(|e| e.alive)
    }

    /// 覆盖存活结论（周期探测回写）。
    pub(crate) fn set_alive(&mut self, service: &str, alive: bool) {
        if let Some(e) = self.items.get_mut(service) {
            e.alive = alive;
        }
    }

    /// 就地更新路由标记（PropertiesChanged 携带 ItemIsMenu / Menu 时）。
    pub(crate) fn update_routing(
        &mut self,
        service: &str,
        menu: Option<Option<String>>,
        menu_only: Option<bool>,
    ) {
        if let Some(e) = self.items.get_mut(service) {
            if let Some(menu) = menu {
                e.menu = menu;
            }
            if let Some(menu_only) = menu_only {
                e.menu_only = menu_only;
            }
        }
    }

    /// 按信号发送者（唯一名 + 对象路径）定位 Item。
    pub(crate) fn find_by_signal(&self, sender: &str, path: &str) -> Option<TrayItemId> {
        self.items
            .values()
            .find(|e| e.unique == sender && e.path == path)
            .map(|e| e.service.clone())
    }

    /// 按 dbusmenu 信号（唯一名 + 菜单对象路径）定位 Item。
    pub(crate) fn find_by_menu_signal(&self, sender: &str, path: &str) -> Option<TrayItemId> {
        self.items
            .values()
            .find(|e| e.unique == sender && e.menu.as_deref() == Some(path))
            .map(|e| e.service.clone())
    }
}
