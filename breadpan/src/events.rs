//! 事件与单事件通道（tokio mpsc；`start` 交付唯一 Receiver，无追加订阅）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::TrayItem;

/// 事件通道容量：库内定值。
pub(crate) const CHANNEL_CAPACITY: usize = 256;

/// 事件流消息。公开语义见 ARCHITECTURE §3.2（事件清单与投递语义）。
#[derive(Debug, Clone, PartialEq)]
pub enum TrayEvent {
    /// 事件流起点：全部 Item 记录（启动时逐个现场读取），与后续事件原子衔接。
    Snapshot { items: Vec<TrayItem> },
    /// 出现：数据现场读取（`item(id)`）。
    Added(crate::TrayItemId),
    /// 服务名消失；同名重现（应用重启）视为新 Item。
    Removed(crate::TrayItemId),
    /// 变更通告（窗口合并，不携带数据）：重读 `item()`/`items()` 取最新。
    Changed(crate::TrayItemId),
    /// dbusmenu 布局/属性变更信号到达；不代为重拉，是否重取由消费方决定。
    MenuChanged(crate::TrayItemId),
    /// 交互调用的异步回执。
    InteractionResult {
        id: crate::TrayItemId,
        op: InteractionOp,
        result: Result<(), String>,
    },
    /// 登记中心所有权丢失（被其它 Watcher 取代，ARCHITECTURE §2.8）：
    /// 极罕见边缘路径；终局报错，待遇同 Fatal；不重试不夺回。
    Displaced,
    /// 内部过程叙述（反向发现、重扫描等）。
    Narration(String),
    /// 终止宣告：最后一个事件，此后 Handle 方法一律返回 Stopped。
    Fatal(String),
}

/// 各交互方法路由到的具体 DBus 交互
/// （SNI §3.2 Activate / SecondaryActivate / ContextMenu / Scroll + dbusmenu Event）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionOp {
    Activate,
    SecondaryActivate,
    ContextMenu,
    Scroll,
    MenuActivate,
}

/// 单实例单通道事件队列：满则丢弃并计数（有损交付，不阻塞内部线程）。
pub(crate) struct EventQueue {
    tx: Mutex<Option<mpsc::Sender<TrayEvent>>>,
    dropped: AtomicU64,
}

impl EventQueue {
    /// 建队：(队列, 唯一 Receiver)；此后库内不再发放 Receiver。
    pub(crate) fn new() -> (Arc<Self>, mpsc::Receiver<TrayEvent>) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        (
            Arc::new(Self {
                tx: Mutex::new(Some(tx)),
                dropped: AtomicU64::new(0),
            }),
            rx,
        )
    }

    /// try_send：满则丢弃该事件并递增 dropped。
    pub(crate) fn send(&self, ev: TrayEvent) {
        let guard = self.tx.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            if tx.try_send(ev).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 丢弃计数（背压告警，仅供内部诊断）。
    #[allow(dead_code)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 关闭通道：消费端 `recv()` 此后返回 `None`（停机路径）。
    pub(crate) fn close(&self) {
        self.tx.lock().unwrap().take();
    }
}
