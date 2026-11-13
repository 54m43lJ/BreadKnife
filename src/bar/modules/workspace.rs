//! 工作区条（渲染 + 手势捕获）。
//! 按钮形态为三条件分层（README「界面交互」）；色彩映射见 ARCHITECTURE §3.10。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use gtk::prelude::*;

use crate::bar::ipc::IpcClient;
use crate::shared::model::{WorkspaceState, WorkspaceWindow};
use crate::shared::protocol::Request;

/// 同向滚动的合并窗口：从首次放行的事件起算，窗口内的同向事件直接忽略。
const SCROLL_MERGE: Duration = Duration::from_millis(20);

pub struct WorkspaceWidget {
    pub container: gtk::Box,
    switches: Rc<SwitchQueue>,
    buttons: Rc<RefCell<BTreeMap<i64, WsButton>>>,
}

/// 工作区切换队列：所有点击 / 滚轮共用一条工作线程，按提交顺序执行。
/// 逐事件 `spawn` 会让相对切换（m±1）在线程调度中乱序，后发的请求先到，
/// 表现为切换被吞或方向反了；串行队列同时避免高频滚动堆出并发请求。
struct SwitchQueue {
    tx: mpsc::Sender<String>,
}

impl SwitchQueue {
    fn new(ipc: Arc<IpcClient>) -> Self {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("workspace-switch".into())
            .spawn(move || {
                while let Ok(target) = rx.recv() {
                    if let Err(e) = ipc.request(Request::WorkspaceSwitch { target }) {
                        eprintln!("[workspace] switch failed: {e}");
                    }
                }
            })
            .expect("spawn workspace-switch thread");
        Self { tx }
    }

    fn submit(&self, target: impl Into<String>) {
        if self.tx.send(target.into()).is_err() {
            eprintln!("[workspace] switch queue closed");
        }
    }
}

/// 滚轮方向：`+1` 为下一工作区（向下 / 向右），`-1` 为上一工作区（向上 / 向左）。
/// 双轴均有位移时取绝对值更大的轴，避免斜向滚动的正负分量被 `||` 误判。
fn scroll_direction(dx: f64, dy: f64) -> Option<i8> {
    let (delta, positive) = if dy.abs() >= dx.abs() {
        (dy, dy > 0.0)
    } else {
        (dx, dx > 0.0)
    };
    if delta == 0.0 {
        None
    } else if positive {
        Some(1)
    } else {
        Some(-1)
    }
}

/// 滚轮合并器：只作用于滚动事件，点击等输入不做任何去抖。
#[derive(Default)]
struct ScrollGate {
    /// 最近一次放行的事件时刻与方向；被忽略的事件不刷新窗口。
    last: Option<(Instant, i8)>,
}

impl ScrollGate {
    /// 首个事件立即放行；合并窗口内的同向事件忽略；反向事件立即放行并重新计时。
    fn accept(&mut self, direction: i8, now: Instant) -> bool {
        if let Some((at, last_direction)) = self.last
            && last_direction == direction
            && now.saturating_duration_since(at) < SCROLL_MERGE
        {
            return false;
        }
        self.last = Some((now, direction));
        true
    }
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
        let switches = Rc::new(SwitchQueue::new(ipc));

        // 滚轮切换：上＝左侧工作区、下＝右侧工作区，滚轮左右自然对应。
        // 合并窗口只吞同向的连续滚动事件，反向滚动立即生效并重新计时。
        let gate = Rc::new(RefCell::new(ScrollGate::default()));
        let scroll_switches = switches.clone();
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::HORIZONTAL,
        );
        scroll.connect_scroll(move |_ctrl, dx, dy| {
            let Some(direction) = scroll_direction(dx, dy) else {
                return gtk::glib::Propagation::Proceed;
            };
            if gate.borrow_mut().accept(direction, Instant::now()) {
                let target = if direction > 0 { "m+1" } else { "m-1" };
                scroll_switches.submit(target);
            }
            gtk::glib::Propagation::Proceed
        });
        container.add_controller(scroll);

        Self {
            container,
            switches,
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

        {
            let switches = self.switches.clone();
            let tip = tip.clone();
            button.connect_clicked(move |_| {
                switches.submit(tip.borrow().name.clone());
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{SCROLL_MERGE, ScrollGate, scroll_direction};

    #[test]
    fn scroll_direction_prefers_dominant_axis() {
        assert_eq!(scroll_direction(0.0, 0.0), None);
        assert_eq!(scroll_direction(0.0, 1.0), Some(1));
        assert_eq!(scroll_direction(0.0, -1.0), Some(-1));
        assert_eq!(scroll_direction(2.0, 0.0), Some(1));
        assert_eq!(scroll_direction(-2.0, 0.0), Some(-1));
        // 斜向滚动取绝对值更大的轴，而不是正负分量相或。
        assert_eq!(scroll_direction(-5.0, 1.0), Some(-1));
        assert_eq!(scroll_direction(1.0, -2.0), Some(-1));
    }

    #[test]
    fn scroll_gate_merges_same_direction_within_window() {
        let start = Instant::now();
        let mut gate = ScrollGate::default();

        assert!(gate.accept(1, start));
        assert!(!gate.accept(1, start + SCROLL_MERGE / 2));
        // 窗口从首次放行的事件起算，被忽略的事件不刷新计时。
        assert!(gate.accept(1, start + SCROLL_MERGE));
        assert!(!gate.accept(1, start + SCROLL_MERGE + Duration::from_millis(1)));
    }

    #[test]
    fn scroll_gate_accepts_reverse_and_restarts_window() {
        let start = Instant::now();
        let mut gate = ScrollGate::default();

        assert!(gate.accept(-1, start));
        // 反方向立即放行，并以该事件重新开始计时。
        assert!(gate.accept(1, start + Duration::from_millis(10)));
        assert!(!gate.accept(1, start + Duration::from_millis(15)));
        assert!(gate.accept(-1, start + Duration::from_millis(25)));
        assert!(!gate.accept(-1, start + Duration::from_millis(30)));
    }
}
