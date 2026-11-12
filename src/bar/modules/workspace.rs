//! 工作区条（渲染 + 手势捕获）。
//! 按钮形态为三条件分层（README「界面交互」）；色彩映射见 ARCHITECTURE §3.10。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;

use crate::bar::ipc::IpcClient;
use crate::shared::model::{WorkspaceState, WorkspaceWindow};
use crate::shared::protocol::Request;

pub struct WorkspaceWidget {
    pub container: gtk::Box,
    ipc: Arc<IpcClient>,
    buttons: Rc<RefCell<BTreeMap<i64, WsButton>>>,
}

/// 单个工作区按钮及其可原地更新的子部件。
/// 状态推送只改样式类与浮层数据，不销毁按钮——按下与松开之间若重建，
/// GtkButton 的 clicked 不再配对，快速连点会丢事件。
struct WsButton {
    button: gtk::Button,
    base: gtk::Box,
    indicator: gtk::Box,
    tip: Rc<RefCell<WsTip>>,
}

struct WsTip {
    name: String,
    windows: Vec<WorkspaceWindow>,
}

impl WorkspaceWidget {
    pub fn new(ipc: Arc<IpcClient>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 2);

        // 滚轮切换：上＝左侧工作区、下＝右侧工作区，滚轮左右自然对应
        let ipc_scroll = ipc.clone();
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::HORIZONTAL,
        );
        scroll.connect_scroll(move |_ctrl, dx, dy| {
            let target = if dy > 0.0 || dx > 0.0 { "m+1" } else { "m-1" };
            let ipc = ipc_scroll.clone();
            std::thread::spawn(move || {
                if let Err(e) = ipc.request(Request::WorkspaceSwitch {
                    target: target.into(),
                }) {
                    eprintln!("[workspace] scroll switch failed: {e}");
                }
            });
            gtk::glib::Propagation::Proceed
        });
        container.add_controller(scroll);

        Self {
            container,
            ipc,
            buttons: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    /// 增量对账：同 id 按钮原地更新，只增删工作区集合的差集。
    /// 不整条重建，避免状态推送在按下与松开之间销毁按钮而吞掉点击。
    pub fn rebuild(&self, states: &[WorkspaceState]) {
        let mut map = self.buttons.borrow_mut();
        let live: HashSet<i64> = states.iter().map(|ws| ws.id).collect();

        let stale: Vec<i64> = map
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in stale {
            if let Some(entry) = map.remove(&id) {
                self.container.remove(&entry.button);
            }
        }

        let mut prev: Option<gtk::Button> = None;
        // 仅当集合/顺序变化时重排；活跃态推送不移动按钮，避免打断进行中的按下。
        let reorder = !self.order_matches(states, &map);
        for ws in states {
            if let Some(entry) = map.get(&ws.id) {
                entry.update(ws);
            } else {
                let entry = self.make_button(ws);
                self.container.append(&entry.button);
                map.insert(ws.id, entry);
            }
            let button = map
                .get(&ws.id)
                .map(|entry| entry.button.clone())
                .expect("workspace button inserted above");
            if reorder {
                self.container.reorder_child_after(&button, prev.as_ref());
            }
            prev = Some(button);
        }
    }

    fn order_matches(&self, states: &[WorkspaceState], map: &BTreeMap<i64, WsButton>) -> bool {
        let mut child = self.container.first_child();
        for ws in states {
            let Some(current) = child else {
                return false;
            };
            let Some(entry) = map.get(&ws.id) else {
                return false;
            };
            if current != entry.button {
                return false;
            }
            child = current.next_sibling();
        }
        child.is_none()
    }

    /// 单按钮：基底 + 活跃叠加点。
    /// 四要素由四条件独立决定（README「界面交互」）：基底形态＝有窗口方形／无窗口
    /// 圆点；基底色＝紧急高亮／否则背景；叠加形态＝活跃圆点；叠加色＝高亮基底取
    /// 背景色（高对比）／否则对比色。按钮内容区会撑满，基底与叠加点各自居中保持
    /// 契约尺寸（4px 点 / 20×15 胶囊）。
    fn make_button(&self, ws: &WorkspaceState) -> WsButton {
        let button = gtk::Button::new();
        button.add_css_class("ws-btn");
        button.set_focusable(false);

        let base = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        base.add_css_class("ws-base");
        base.add_css_class(if ws.has_windows { "square" } else { "dot" });
        base.set_halign(gtk::Align::Center);
        base.set_valign(gtk::Align::Center);
        if ws.urgent && ws.has_windows {
            base.add_css_class("urgent");
        }

        let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        indicator.add_css_class("ws-ind");
        indicator.set_halign(gtk::Align::Center);
        indicator.set_valign(gtk::Align::Center);
        if ws.urgent && ws.has_windows {
            indicator.add_css_class("on-urgent");
        }
        indicator.set_visible(ws.active);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&base));
        overlay.add_overlay(&indicator);
        button.set_child(Some(&overlay));

        let tip = Rc::new(RefCell::new(WsTip {
            name: ws.name.clone(),
            windows: ws.windows.clone(),
        }));
        button.set_has_tooltip(true);
        {
            let tip = tip.clone();
            button.connect_query_tooltip(move |_, _, _, _, tooltip| {
                let tip = tip.borrow();
                let content = build_ws_tooltip(&tip.name, &tip.windows);
                tooltip.set_custom(Some(&content));
                true
            });
        }

        let ipc = self.ipc.clone();
        {
            let tip = tip.clone();
            button.connect_clicked(move |_| {
                let target = tip.borrow().name.clone();
                let ipc = ipc.clone();
                std::thread::spawn(move || {
                    if let Err(e) = ipc.request(Request::WorkspaceSwitch { target }) {
                        eprintln!("[workspace] switch failed: {e}");
                    }
                });
            });
        }

        WsButton {
            button,
            base,
            indicator,
            tip,
        }
    }
}

impl WsButton {
    /// 原地刷新四要素与浮层数据；类集合整体替换，去掉上一帧的形态/紧急态。
    fn update(&self, ws: &WorkspaceState) {
        let urgent = ws.urgent && ws.has_windows;
        let mut base_classes = vec!["ws-base", if ws.has_windows { "square" } else { "dot" }];
        if urgent {
            base_classes.push("urgent");
        }
        self.base.set_css_classes(&base_classes);
        self.base.set_halign(gtk::Align::Center);
        self.base.set_valign(gtk::Align::Center);

        let mut indicator_classes = vec!["ws-ind"];
        if urgent {
            indicator_classes.push("on-urgent");
        }
        self.indicator.set_css_classes(&indicator_classes);
        self.indicator.set_halign(gtk::Align::Center);
        self.indicator.set_valign(gtk::Align::Center);
        self.indicator.set_visible(ws.active);

        let mut tip = self.tip.borrow_mut();
        tip.name.clone_from(&ws.name);
        tip.windows.clone_from(&ws.windows);
    }
}

/// 工作区悬停浮层：首期只列窗口标题；最多 4 条，超出时第五条为省略号。
fn build_ws_tooltip(name: &str, windows: &[WorkspaceWindow]) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
    root.add_css_class("ws-tip");
    root.set_size_request(220, -1);

    let title = gtk::Label::new(Some(&format!("工作区 {name}")));
    title.set_xalign(0.0);
    title.add_css_class("ws-tip-title");
    root.append(&title);

    if windows.is_empty() {
        let empty = gtk::Label::new(Some("无窗口"));
        empty.set_xalign(0.0);
        empty.add_css_class("ws-tip-empty");
        root.append(&empty);
        return root;
    }

    for window in windows.iter().take(4) {
        let text = if window.title.trim().is_empty() {
            "未命名窗口"
        } else {
            window.title.trim()
        };
        let row = gtk::Label::new(Some(text));
        row.set_xalign(0.0);
        row.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.set_max_width_chars(36);
        row.add_css_class("ws-tip-row");
        root.append(&row);
    }

    if windows.len() > 4 {
        let more = gtk::Label::new(Some("..."));
        more.set_xalign(0.0);
        more.add_css_class("ws-tip-more");
        root.append(&more);
    }

    root
}
