//! 窗口进程生命周期（spawn / kill / 崩溃重启）；显示器 → 窗口映射。

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::daemon::DaemonEvent;
use crate::shared::protocol::Frame;

/// 崩溃重启上限（连续窗口期内）与窗口期长度。
pub const RESTART_LIMIT: u32 = 5;
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);
/// 崩溃后延时重启的等待。
const RESPAWN_DELAY: Duration = Duration::from_secs(1);
const EXIT_POLL: Duration = Duration::from_millis(300);

pub struct WindowProc {
    pub child: std::process::Child,
    /// 连接就绪（Hello）后登记的推送通道；None = 尚未接入 / 已断开。
    pub out: Option<Sender<Frame>>,
    pub killing: bool,
    pub started: Instant,
}

/// 为显示器 spawn 一个窗口进程（bread __window --monitor M --socket S）。
/// 探测线程轮询 /proc 判定退出（Child 留在 daemon 侧供 kill 使用；
/// 退出进程在 kernel 侧以僵尸态留存，stat 的 state 位变为 Z 即视为退出），
/// 判定后延时上报——自然形成崩溃重启间隔。
pub fn spawn(tx: &Sender<DaemonEvent>, monitor: &str, window_socket: &str) -> Option<WindowProc> {
    let exe = std::env::current_exe().ok()?;
    let mut command = std::process::Command::new(exe);
    command
        .args(["__window", "--monitor", monitor, "--socket", window_socket])
        .stdout(std::process::Stdio::null());
    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[windows] spawn window for {monitor} failed: {e}");
            return None;
        }
    };
    let pid = child.id();
    let tx = tx.clone();
    let mon = monitor.to_string();
    std::thread::Builder::new()
        .name(format!("wait-{mon}"))
        .spawn(move || {
            while process_alive(pid) {
                std::thread::sleep(EXIT_POLL);
            }
            std::thread::sleep(RESPAWN_DELAY);
            let _ = tx.send(DaemonEvent::ChildExited { monitor: mon });
        })
        .ok()?;
    eprintln!("[windows] spawned window for {monitor} (pid {pid})");
    Some(WindowProc { child, out: None, killing: false, started: Instant::now() })
}

/// 进程仍在运行（或为僵尸——僵尸视为已退出，返回 false）。
fn process_alive(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // stat: pid (comm) state ...——comm 可含空格，取末括号后首字符
    let Some(after) = stat.rsplit(')').next() else { return false };
    let state = after.trim().chars().next();
    !matches!(state, Some('Z') | Some('X'))
}
