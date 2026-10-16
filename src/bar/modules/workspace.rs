//! 工作区条（渲染 + 手势捕获）。
//! 按钮形态为三条件分层（README「界面交互」）；色彩映射见 ARCHITECTURE §3.10。

use std::cell::RefCell;
use std::sync::Arc;
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk::prelude::*;

use crate::bar::ipc::IpcClient;
use crate::shared::model::WorkspaceState;
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

    /// 单按钮：Overlay(基底形状 + 活跃小圆点)。
    /// 有窗口：大圆角方形（紧急→高亮色）；无窗口：单个小圆点；
    /// 活跃叠加对比色小圆点（紧急时方形变色、小点不变色，保住活跃判断）。
    /// 按钮内容区会撑满，基底与小圆点各自居中保持契约尺寸（4px 点 / 20×15 胶囊）。
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
        if !ws.has_windows && ws.active && !ws.urgent {
            base.add_css_class("active");
        }

        let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        indicator.add_css_class("ws-ind");
        indicator.set_halign(gtk::Align::Center);
        indicator.set_valign(gtk::Align::Center);
        indicator.set_visible(ws.has_windows && ws.active);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&base));
        overlay.add_overlay(&indicator);
        button.set_child(Some(&overlay));

        let ipc = self.ipc.clone();
        let target = ws.name.clone();
        button.set_tooltip_text(Some(&format!("workspace {}", ws.name)));
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
