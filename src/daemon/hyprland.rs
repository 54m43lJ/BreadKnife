//! Hyprland IPC 只读监听：显示器 / 工作区 / 活动窗口。
//! 事件契约见 ARCHITECTURE.html §4.1；本文件只订阅不回复。

use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde::Deserialize;

use crate::daemon::DaemonEvent;

// ── hyprctl -j 同步命令 ──

#[derive(Debug, Deserialize, Clone)]
pub struct MonitorInfo {
    pub name: String,
    /// 该显示器上的活动工作区（新版本 focusedWorkspace / 旧版本 activeWorkspace）。
    #[serde(default, alias = "focusedWorkspace", alias = "activeWorkspace")]
    pub active_ws: Option<WorkspaceRef>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkspaceRef {
    pub id: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkspaceInfo {
    pub id: i64,
    pub name: String,
    pub monitor: String,
    #[serde(default)]
    pub windows: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClientInfo {
    pub address: String,
    pub workspace: WorkspaceRef,
    #[serde(default)]
    pub urgent: bool,
}

fn hyprctl(args: &[&str]) -> String {
    match std::process::Command::new("hyprctl").args(args).output() {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        Ok(out) => {
            eprintln!(
                "[hyprland] hyprctl {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            String::new()
        }
        Err(e) => {
            eprintln!("[hyprland] hyprctl {:?} exec failed: {}", args, e);
            String::new()
        }
    }
}

fn hyprctl_json<T: serde::de::DeserializeOwned>(args: &[&str]) -> Vec<T> {
    let resp = hyprctl(args);
    if resp.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[hyprland] hyprctl {:?} parse failed: {}", args, e);
            Vec::new()
        }
    }
}

pub fn monitors() -> Vec<MonitorInfo> {
    hyprctl_json(&["-j", "monitors"])
}

pub fn workspaces() -> Vec<WorkspaceInfo> {
    hyprctl_json(&["-j", "workspaces"])
}

pub fn clients() -> Vec<ClientInfo> {
    hyprctl_json(&["-j", "clients"])
}

/// 窗口地址（0x…）→ 所在工作区 id。
pub fn workspace_of_client(address: &str) -> Option<i64> {
    clients()
        .into_iter()
        .find(|c| c.address == address)
        .map(|c| c.workspace.id)
}

pub fn dispatch_workspace(target: &str) {
    hyprctl(&["dispatch", "workspace", target]);
}

// ── socket2 事件流（只读监听）──

/// 汇聚后的事件语义；触发全量重同步的事件合并为 `Resync`。
#[derive(Debug, Clone)]
pub enum HyprlandEvent {
    MonitorAdded(String),
    MonitorRemoved(String),
    ActiveWindow { class: String, title: String, addr: String },
    WindowTitle { addr: String, title: String },
    Urgent { addr: String },
    Resync,
}

fn event_socket_path() -> Option<String> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(format!("{dir}/hypr/{sig}/.socket2.sock"))
}

/// 建立只读监听线程；连接失败重试后放弃（降级：无事件驱动）。
pub fn watch(tx: Sender<DaemonEvent>) {
    std::thread::Builder::new()
        .name("hyprland-events".into())
        .spawn(move || {
            let Some(path) = event_socket_path() else {
                eprintln!("[hyprland] HYPRLAND_INSTANCE_SIGNATURE missing; no event stream");
                return;
            };
            let stream = loop {
                match std::os::unix::net::UnixStream::connect(&path) {
                    Ok(s) => break s,
                    Err(e) => {
                        eprintln!("[hyprland] socket2 connect error: {}", e);
                        if !retry_wait() {
                            return;
                        }
                    }
                }
            };
            eprintln!("[hyprland] listening on socket2 ({})", path);
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Some((name, data)) = line.split_once(">>") else {
                    continue;
                };
                if let Some(ev) = parse_event(name, data) {
                    if tx.send(DaemonEvent::Hyprland(ev)).is_err() {
                        return; // daemon 已退出
                    }
                }
            }
            eprintln!("[hyprland] socket2 disconnected");
        })
        .expect("spawn hyprland-events");
}

/// 连接重试等待；返回 false = 放弃。
fn retry_wait() -> bool {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(1));
    }
    true
}

fn parse_event(name: &str, data: &str) -> Option<HyprlandEvent> {
    match name {
        "monitoraddedv2" | "monitoradded" => {
            Some(HyprlandEvent::MonitorAdded(first_token(data)))
        }
        "monitorremovedv2" | "monitorremoved" => {
            Some(HyprlandEvent::MonitorRemoved(first_token(data)))
        }
        "activewindowv2" => {
            // class,title,addr（title 可含逗号：取首尾，其余归 title）
            let parts: Vec<&str> = data.split(',').collect();
            if parts.len() >= 3 {
                let title = parts[1..parts.len() - 1].join(",");
                Some(HyprlandEvent::ActiveWindow {
                    class: parts[0].to_string(),
                    title,
                    addr: parts[parts.len() - 1].to_string(),
                })
            } else if parts.len() == 2 {
                Some(HyprlandEvent::ActiveWindow {
                    class: parts[0].to_string(),
                    title: parts[1].to_string(),
                    addr: String::new(),
                })
            } else {
                None
            }
        }
        "activewindow" => {
            let (class, title) = data.split_once(',')?;
            Some(HyprlandEvent::ActiveWindow {
                class: class.to_string(),
                title: title.to_string(),
                addr: String::new(),
            })
        }
        "windowtitlev2" => {
            let (addr, title) = data.split_once(',')?;
            Some(HyprlandEvent::WindowTitle { addr: addr.to_string(), title: title.to_string() })
        }
        "urgent" | "urgentdecode" => {
            Some(HyprlandEvent::Urgent { addr: first_token(data) })
        }
        // 触发全量重同步的事件（显示器 / 工作区 / 窗口归属 / 配置重载）
        "workspacev2" | "workspace" | "focusedmonv2" | "focusedmon" | "openwindow"
        | "closewindow" | "movewindow" | "movewindowv2" | "createworkspace" | "createworkspacev2"
        | "destroyworkspace" | "destroyworkspacev2" | "changefloatingmode" | "configreloaded"
        | "activespecialworkspace" => Some(HyprlandEvent::Resync),
        _ => None, // 未消费的事件静默忽略
    }
}

fn first_token(data: &str) -> String {
    data.split(',').next().unwrap_or_default().to_string()
}
