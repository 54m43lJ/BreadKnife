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

/// 运行 hyprctl 子命令；失败时错误消息取 stderr（hyprctl 的错误也可能走 stdout）。
fn hyprctl(args: &[&str]) -> Result<String, String> {
    match std::process::Command::new("hyprctl").args(args).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if out.status.success() {
                Ok(stdout)
            } else if stderr.is_empty() {
                Err(stdout)
            } else {
                Err(stderr)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn hyprctl_json<T: serde::de::DeserializeOwned>(args: &[&str]) -> Vec<T> {
    let resp = match hyprctl(args) {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("[hyprland] hyprctl {:?} failed: {}", args, e);
            return Vec::new();
        }
    };
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

/// 工作区切换。Hyprland ≥0.55（Lua 配置）走 `hl.dsp.focus`；
/// 旧版（hyprlang）Lua 表达式必然失败，回退经典 `dispatch workspace <target>`。
pub fn dispatch_workspace(target: &str) {
    if hyprctl(&["dispatch", &lua_focus_expr(target)]).is_ok() {
        return;
    }
    if let Err(e) = hyprctl(&["dispatch", "workspace", target]) {
        eprintln!("[hyprland] dispatch workspace {target:?} failed: {e}");
    }
}

/// Hyprland Lua dispatch 表达式（`hl.dsp.focus({ workspace = "…" })`）。
fn lua_focus_expr(target: &str) -> String {
    format!("hl.dsp.focus({{ workspace = {} }})", lua_string(target))
}

/// Lua 字符串字面量（转义反斜杠与双引号）。
fn lua_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── socket2 事件流（只读监听）──

/// 汇聚后的事件语义；触发全量重同步的事件合并为 `Resync`。
#[derive(Debug, Clone, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── socket2 事件行解析（fixture 防上游事件名 / 载荷漂移）──

    #[test]
    fn parse_event_monitor_lifecycle() {
        assert!(matches!(
            parse_event("monitoraddedv2", "DP-1,Some Monitor,0"),
            Some(HyprlandEvent::MonitorAdded(n)) if n == "DP-1"
        ));
        assert!(matches!(
            parse_event("monitoradded", "DP-1"),
            Some(HyprlandEvent::MonitorAdded(n)) if n == "DP-1"
        ));
        assert!(matches!(
            parse_event("monitorremovedv2", "DP-1,Some Monitor,0"),
            Some(HyprlandEvent::MonitorRemoved(n)) if n == "DP-1"
        ));
    }

    #[test]
    fn parse_event_active_window() {
        // v2 载荷：class,title,addr（title 可含逗号）
        assert!(matches!(
            parse_event("activewindowv2", "firefox,T,with,commas,0xdeadbeef"),
            Some(HyprlandEvent::ActiveWindow { ref class, ref title, ref addr })
                if class == "firefox" && title == "T,with,commas" && addr == "0xdeadbeef"
        ));
        // 空桌面：class 与 title 皆空、无地址
        assert!(matches!(
            parse_event("activewindowv2", ","),
            Some(HyprlandEvent::ActiveWindow { ref class, ref title, ref addr })
                if class.is_empty() && title.is_empty() && addr.is_empty()
        ));
        // 旧版事件：class,title
        assert!(matches!(
            parse_event("activewindow", "kitty,zsh"),
            Some(HyprlandEvent::ActiveWindow { ref class, ref title, .. })
                if class == "kitty" && title == "zsh"
        ));
    }

    #[test]
    fn parse_event_title_and_urgent() {
        assert!(matches!(
            parse_event("windowtitlev2", "0xdeadbeef,new title"),
            Some(HyprlandEvent::WindowTitle { ref addr, ref title })
                if addr == "0xdeadbeef" && title == "new title"
        ));
        assert!(matches!(
            parse_event("urgent", "0xdeadbeef"),
            Some(HyprlandEvent::Urgent { ref addr }) if addr == "0xdeadbeef"
        ));
        assert!(matches!(
            parse_event("urgentdecode", "0xdeadbeef,extra"),
            Some(HyprlandEvent::Urgent { ref addr }) if addr == "0xdeadbeef"
        ));
    }

    #[test]
    fn parse_event_resync_triggers() {
        let cases = [
            ("workspacev2", "2"),
            ("workspace", "2"),
            ("focusedmonv2", "DP-1,2"),
            ("focusedmon", "DP-1,2"),
            ("openwindow", "0xa,1,kitty,zsh"),
            ("closewindow", "0xa"),
            ("movewindow", "0xa,2"),
            ("movewindowv2", "0xa,2,1"),
            ("createworkspace", "3"),
            ("createworkspacev2", "3"),
            ("destroyworkspace", "3"),
            ("destroyworkspacev2", "3"),
            ("changefloatingmode", "0xa,1"),
            ("configreloaded", ""),
            ("activespecialworkspace", ""),
        ];
        for (name, data) in cases {
            assert!(
                matches!(parse_event(name, data), Some(HyprlandEvent::Resync)),
                "{name}>>{data} 应触发 Resync"
            );
        }
    }

    #[test]
    fn parse_event_unknown_ignored() {
        assert_eq!(parse_event("somerandomevent", "x"), None);
        assert_eq!(parse_event("tolongline", "a>>b"), None);
    }

    // ── 工作区切换命令构造（Lua dispatch 表达式）──

    #[test]
    fn lua_focus_expr_shape() {
        assert_eq!(lua_focus_expr("2"), r#"hl.dsp.focus({ workspace = "2" })"#);
        assert_eq!(lua_focus_expr("m+1"), r#"hl.dsp.focus({ workspace = "m+1" })"#);
    }

    #[test]
    fn lua_string_escapes() {
        assert_eq!(lua_string("special:magic"), r#""special:magic""#);
        assert_eq!(lua_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    // ── hyprctl -j 反序列化（fixture 防字段漂移）──

    #[test]
    fn monitors_json_workspace_ref_alias() {
        // 新版本字段名
        let focused = r#"[{"id":1,"name":"DP-1","focusedWorkspace":{"id":3,"name":"3"}}]"#;
        let parsed: Vec<MonitorInfo> = serde_json::from_str(focused).unwrap();
        assert_eq!(parsed[0].active_ws.as_ref().unwrap().id, 3);
        // 旧版本字段名
        let legacy = r#"[{"id":1,"name":"DP-1","activeWorkspace":{"id":2,"name":"2"}}]"#;
        let parsed: Vec<MonitorInfo> = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed[0].active_ws.as_ref().unwrap().id, 2);
        // 两字段皆缺（理论不达，防手滑改坏 Option）
        let none = r#"[{"id":1,"name":"DP-1"}]"#;
        let parsed: Vec<MonitorInfo> = serde_json::from_str(none).unwrap();
        assert!(parsed[0].active_ws.is_none());
    }

    #[test]
    fn workspaces_json_windows_count() {
        let data = r#"[{"id":1,"name":"1","monitor":"DP-1","monitorID":0,"windows":2,"hasfullscreen":false,"lastwindow":"0x1"},{"id":2,"name":"2","monitor":"DP-1","windows":0}]"#;
        let parsed: Vec<WorkspaceInfo> = serde_json::from_str(data).unwrap();
        assert_eq!(parsed[0].windows, 2);
        assert_eq!(parsed[1].windows, 0);
        assert_eq!(parsed[1].monitor, "DP-1");
        assert_eq!(parsed[1].id, 2);
    }

    #[test]
    fn clients_json_urgent_flag() {
        let full = r#"[{"address":"0x55f0aa","mapped":true,"hidden":false,"at":[0,0],"size":[800,600],"workspace":{"id":2,"name":"2"},"floating":false,"monitor":0,"class":"firefox","title":"F","initialClass":"firefox","initialTitle":"F","pid":123,"xwayland":false,"pinned":false,"fullscreen":false,"fullscreenMode":0,"fakefullscreen":false,"grouped":[],"swallowCount":0,"swallowing":"0x0","focusHistoryID":0,"urgent":true}]"#;
        let parsed: Vec<ClientInfo> = serde_json::from_str(full).unwrap();
        assert!(parsed[0].urgent);
        assert_eq!(parsed[0].workspace.id, 2);
        // 旧版本输出缺 urgent 字段 → false
        let minimal = r#"[{"address":"0x1","workspace":{"id":1,"name":"1"}}]"#;
        let parsed: Vec<ClientInfo> = serde_json::from_str(minimal).unwrap();
        assert!(!parsed[0].urgent);
    }
}
