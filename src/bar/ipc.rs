//! IPC 客户端：快照 / 推送订阅、请求投递。
//! 推送与 UI 回执汇入统一 UI 任务队列（std mpsc），由窗口主线程排空执行；
//! 请求在专用线程上阻塞往返（daemon 侧托盘调用可达数秒，不得在 GTK 主线程等待）。

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::bar::UiTask;
use crate::shared::protocol::{read_frame, write_frame, Frame, Request, Response};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(35);

type PendingMap = Arc<Mutex<HashMap<u64, std::sync::mpsc::Sender<Frame>>>>;

pub struct IpcClient {
    path: PathBuf,
    monitor: String,
    ui_tx: std::sync::mpsc::Sender<UiTask>,
    stream: Mutex<Option<UnixStream>>,
    pending: PendingMap,
    next_id: AtomicU64,
}

impl IpcClient {
    /// 连接 + Hello 握手；全量快照由 daemon 在握手后经推送通道送达。
    pub fn connect(
        path: &Path,
        monitor: &str,
        ui_tx: std::sync::mpsc::Sender<UiTask>,
    ) -> Result<Arc<Self>, String> {
        let client = Arc::new(Self {
            path: path.to_path_buf(),
            monitor: monitor.to_string(),
            ui_tx,
            stream: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
        });
        client.establish()?;
        Ok(client)
    }

    /// 阻塞请求（只在专用线程调用，不在 GTK 主线程）：失败重连一次后重试。
    pub fn request(&self, request: Request) -> Result<Response, String> {
        let mut last_err = String::new();
        for attempt in 0..2 {
            if attempt == 1 && self.reconnect().is_err() {
                return Err(format!("request failed (reconnect failed): {last_err}"));
            }
            match self.attempt(&request) {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    eprintln!("[bar-ipc] request failed (attempt {}): {}", attempt + 1, e);
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    fn attempt(&self, request: &Request) -> Result<Response, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel::<Frame>();
        self.pending.lock().unwrap().insert(id, tx);

        let sent = {
            let mut guard = self.stream.lock().unwrap();
            match guard.as_mut() {
                Some(stream) => {
                    write_frame(stream, &Frame::Request { id, request: request.clone() })
                        .map_err(|e| e.to_string())
                }
                None => Err("not connected".into()),
            }
        };
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }

        match rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(Frame::Response { response, .. }) => Ok(response),
            Ok(other) => Err(format!("unexpected frame: {other:?}")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err("response timeout".into())
            }
        }
    }

    /// 建立连接：连接套接字、起 reader 线程（推送 → UI 队列；应答 → 等待者）、Hello 握手。
    fn establish(&self) -> Result<(), String> {
        let stream = UnixStream::connect(&self.path).map_err(|e| e.to_string())?;
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;

        let mut read_half = stream.try_clone().map_err(|e| e.to_string())?;
        let pending = self.pending.clone();
        let ui_tx = self.ui_tx.clone();
        std::thread::Builder::new()
            .name("ipc-reader".into())
            .spawn(move || {
                while let Ok(frame) = read_frame(&mut read_half) {
                    match frame {
                        Frame::Push { push } => {
                            if ui_tx.send(UiTask::Push(push)).is_err() {
                                return; // UI 队列已无人消费
                            }
                        }
                        Frame::Response { id, .. } => {
                            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                let _ = tx.send(frame);
                            }
                        }
                        other => eprintln!("[bar-ipc] unexpected frame: {other:?}"),
                    }
                }
                // EOF：daemon 侧连接断开。存活探测由窗口的周期检查承担；
                // 后续请求失败会走重连路径（重连后按首连流程再取全量快照）。
            })
            .map_err(|e| e.to_string())?;

        *self.stream.lock().unwrap() = Some(stream);

        // Hello 握手（同步应答；全量快照随后经 UI 队列到达并整体重建）
        match self.attempt(&Request::Hello { monitor: self.monitor.clone() })? {
            Response::Ok => Ok(()),
            Response::Error(e) => Err(format!("hello rejected: {e}")),
            other => Err(format!("unexpected hello reply: {other:?}")),
        }
    }

    fn reconnect(&self) -> Result<(), String> {
        // 丢弃旧连接与滞留等待者
        self.pending.lock().unwrap().clear();
        *self.stream.lock().unwrap() = None;
        self.establish()
    }
}
