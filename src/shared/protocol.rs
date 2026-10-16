//! IPC 消息类型（请求 / 应答 / 推送）与线格式（帧编码）。
//! 消息契约见 ARCHITECTURE.html §4.4；路径契约见 §4.2 与 §3.3。

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::shared::model::{
    InteractionOutcome, MenuSnapshot, NotificationRecord, StateDelta, TrayUpdate,
    WindowSnapshot,
};

/// 单帧上限：位图图标经 base64 跨 JSON 时的护栏。
const MAX_FRAME: usize = 16 * 1024 * 1024;

// ── 消息体 ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InteractInput {
    Left { x: i32, y: i32 },
    Middle { x: i32, y: i32 },
    Right { x: i32, y: i32 },
    ScrollUp { steps: i32 },
    ScrollDown { steps: i32 },
    ScrollLeft { steps: i32 },
    ScrollRight { steps: i32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaOp {
    PlayPause,
    Next,
    Prev,
}

/// 窗口 → daemon（请求，均有应答）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Request {
    /// 窗口接入握手：登记连接归属的显示器，daemon 随即推送全量快照。
    Hello { monitor: String },
    TrayInteract { item: String, input: InteractInput },
    TrayMenuRead { item: String },
    TrayMenuExpand { item: String, entry: i32 },
    TrayMenuActivate { item: String, entry: i32 },
    WorkspaceSwitch { target: String },
    MediaControl { op: MediaOp },
    VolumeSet { value: u32 },
    BrightnessSet { value: u32 },
}

/// daemon → 窗口（请求应答）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Response {
    Ok,
    Menu(MenuSnapshot),
    Outcome(InteractionOutcome),
    Error(String),
}

/// daemon → 窗口（推送）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Push {
    /// 首连全量快照。
    Snapshot(WindowSnapshot),
    /// 增量：工作区（定向）/ 活动窗口 / 系统状态。
    State(StateDelta),
    /// 托盘项增 / 删 / 改。
    Tray(TrayUpdate),
    /// 弹窗打开期间的菜单刷新触发（弹窗未开该 Item 的窗口忽略）。
    MenuChanged { item: String },
    Notification(NotificationRecord),
    /// 按 daemon 跟踪的路径重读 CSS。
    Reload,
}

/// 控制指令（§4.2 行协议的服务端解析结果）。
#[derive(Debug, Clone, PartialEq)]
pub enum ControlCommand {
    Open { name: String, monitor: Option<String> },
    Close { name: String, monitor: Option<String> },
    OpenAll,
    CloseAll,
    Exit,
    Reload,
}

// ── 线格式：4 字节小端长度 + JSON ──

/// 统一帧：请求 / 应答携带关联 id（推送 id 恒 0）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Request { id: u64, request: Request },
    Response { id: u64, response: Response },
    Push { push: Push },
}

pub fn write_frame(stream: &mut UnixStream, frame: &Frame) -> io::Result<()> {
    let body = serde_json::to_vec(frame).map_err(|e| io::Error::other(e.to_string()))?;
    stream.write_all(&(body.len() as u32).to_le_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

pub fn read_frame(stream: &mut UnixStream) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame length out of range"));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::other(e.to_string()))
}

// ── 路径（§4.2 控制接口、§3.3 进程形态）──

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir())
}

pub fn default_control_socket() -> PathBuf {
    runtime_dir().join("bread.sock")
}

/// 窗口 IPC 套接字由控制套接字派生（同目录、同词干）。
pub fn window_socket_for(control: &Path) -> PathBuf {
    let stem = control
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bread");
    control.with_file_name(format!("{stem}-windows.sock"))
}

pub fn pid_file() -> PathBuf {
    runtime_dir().join("bread.pid")
}

/// daemon 存活探测：读 PID 文件后核对 /proc（窗口自主退出路径用）。
pub fn daemon_alive() -> bool {
    let Ok(pid) = std::fs::read_to_string(pid_file()) else {
        return false;
    };
    let pid: u32 = match pid.trim().parse() {
        Ok(p) if p > 0 => p,
        _ => return false,
    };
    Path::new(&format!("/proc/{pid}")).exists()
}

// ── 控制行协议解析（§4.2：一行一条指令）──

pub fn parse_control_line(line: &str) -> Option<ControlCommand> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let monitor = |tokens: &[&str]| -> Option<Option<String>> {
        match tokens {
            [.., "-m", mon] => Some(Some(mon.to_string())),
            _ if tokens.iter().any(|t| *t == "-m") => None, // -m 缺参
            _ => Some(None),
        }
    };
    match tokens.as_slice() {
        ["open-all"] => Some(ControlCommand::OpenAll),
        ["close-all"] => Some(ControlCommand::CloseAll),
        ["exit"] => Some(ControlCommand::Exit),
        ["reload"] => Some(ControlCommand::Reload),
        ["open", name, rest @ ..] => monitor(rest)
            .map(|m| ControlCommand::Open { name: name.to_string(), monitor: m }),
        ["close", name, rest @ ..] => monitor(rest)
            .map(|m| ControlCommand::Close { name: name.to_string(), monitor: m }),
        _ => None,
    }
}
