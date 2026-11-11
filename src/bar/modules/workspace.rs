//! 工作区条（渲染 + 手势捕获）。
//! 按钮形态为三条件分层（README「界面交互」）；色彩映射见 ARCHITECTURE §3.10。

use std::cell::RefCell;
use std::sync::Arc;
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk::prelude::*;

use crate::bar::ipc::IpcClient;
use crate::shared::model::{WorkspaceState, WorkspaceWindow};
use crate::shared::protocol::Request;

pub struct WorkspaceWidget {
    pub container: gtk::Box,
    ipc: Arc<IpcClient>,
    buttons: Rc<RefCell<BTreeMap<i64, gtk::Button>>>,
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
                if let Err(e) = ipc.request(Request::WorkspaceSwitch { target: target.into() }) {
                    eprintln!("[workspace] scroll switch failed: {e}");
                }
            });
            gtk::glib::Propagation::Proceed
        });
        container.add_controller(scroll);

        Self { container, ipc, buttons: Rc::new(RefCell::new(BTreeMap::new())) }
    }

    pub fn rebuild(&self, states: &[WorkspaceState]) {
        let buttons = self.buttons.clone();
        {
            let mut map = buttons.borrow_mut();
            map.clear();
            while let Some(child) = self.container.first_child() {
                self.container.remove(&child);
            }
        }
        for ws in states {
            let button = self.make_button(ws);
            buttons.borrow_mut().insert(ws.id, button.clone());
            self.container.append(&button);
        }
    }

    /// 单按钮：基底 + 活跃叠加点。
    /// 四要素由四条件独立决定（README「界面交互」）：基底形态＝有窗口方形／无窗口
    /// 圆点；基底色＝紧急高亮／否则背景；叠加形态＝活跃圆点；叠加色＝高亮基底取
    /// 背景色（高对比）／否则对比色。按钮内容区会撑满，基底与叠加点各自居中保持
    /// 契约尺寸（4px 点 / 20×15 胶囊）。
    fn make_button(&self, ws: &WorkspaceState) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("ws-btn");
        button.set_focusable(false);

        let base = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        base.add_css_class("ws-base");
        base.add_css_class(if ws.has_windows { "square" } else { "dot" });
        base.set_halign(gtk::Align::Center);
        base.set_valign(gtk::Align::Center);
        if ws.urgent {
            base.add_css_class("urgent");
        }

        let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        indicator.add_css_class("ws-ind");
        indicator.set_halign(gtk::Align::Center);
        indicator.set_valign(gtk::Align::Center);
        if ws.urgent {
            indicator.add_css_class("on-urgent");
        }
        indicator.set_visible(ws.active);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&base));
        overlay.add_overlay(&indicator);
        button.set_child(Some(&overlay));

        let windows = ws.windows.clone();
        let ws_name = ws.name.clone();
        button.set_has_tooltip(true);
        button.connect_query_tooltip(move |_, _, _, _, tooltip| {
            tooltip.set_custom(Some(&build_ws_tooltip(&ws_name, &windows)));
            true
        });

        let ipc = self.ipc.clone();
        let target = ws.name.clone();
        button.connect_clicked(move |_| {
            let ipc = ipc.clone();
            let target = target.clone();
            std::thread::spawn(move || {
                if let Err(e) = ipc.request(Request::WorkspaceSwitch { target }) {
                    eprintln!("[workspace] switch failed: {e}");
                }
            });
        });

        button
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
