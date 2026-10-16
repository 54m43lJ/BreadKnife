//! bread 入口：子命令分发（daemon / open / close / …）。
//! 同一二进制承载三种角色：daemon、窗口（由 daemon spawn）、客户端（指令转发）。

mod bar;
mod daemon;
mod shared;

#[cfg(test)]
mod tests;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use clap::Parser;

use shared::protocol;

const APP_ID: &str = "com.breadknife.shell";

const SOCKET_HELP: &str = "\
CONTROL COMMANDS (forwarded to the daemon over its Unix socket):
    bread                          Start the daemon in the foreground
    bread daemon                   Start the daemon in the background (auto-daemonize)
    bread open-all                 Open bar windows on every connected monitor
    bread close-all                Close all bar windows
    bread open bar [-m MONITOR]    Open the bar window (default: all monitors)
    bread close bar [-m MONITOR]   Close the bar window (default: all monitors)
    bread reload                   Hot-reload the CSS theme (path tracked by the daemon)
    bread exit                     Shut the daemon down gracefully

The daemon listens on $XDG_RUNTIME_DIR/bread.sock by default; override both
sides with -s/--socket. The protocol is plain text, one command per line.
";

#[derive(Parser)]
#[command(
    name = "bread",
    about = "BreadKnife Hyprland shell",
    after_help = SOCKET_HELP,
    after_long_help = SOCKET_HELP
)]
struct Args {
    /// Unix socket the daemon binds to and the client connects to
    #[arg(short, long, value_name = "PATH")]
    socket: Option<String>,

    /// Command to run (omitted → start the daemon in the background)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

#[derive(Parser)]
struct WindowArgs {
    #[arg(long)]
    monitor: String,
    #[arg(long)]
    socket: String,
}

fn main() {
    let args = Args::parse();
    let control = args
        .socket
        .clone()
        .unwrap_or_else(|| protocol::default_control_socket().to_string_lossy().into());

    if args.cmd.is_empty() {
        // bread 前台启动 daemon（README「运行管理」/ ARCHITECTURE §3.3）
        if let Err(e) = daemon::run(&control) {
            eprintln!("[breadknife] daemon failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    let cmd = args.cmd;
    match cmd[0].as_str() {
        "__daemon" => {
            if let Err(e) = daemon::run(&control) {
                eprintln!("[breadknife] daemon failed: {e}");
                std::process::exit(1);
            }
        }
        "daemon" => start_daemon(&control),
        "__window" => {
            let wargs = WindowArgs::parse_from(
                std::iter::once("bread").chain(cmd[1..].iter().map(|s| s.as_str())),
            );
            // VMware SVGA / vmwgfx 的 dmabuf 导入不可用：回退 cairo (shm) 渲染，
            // 规避 Wayland dmabuf 协议错误崩溃
            if std::env::var_os("GSK_RENDERER").is_none() {
                // SAFETY: 单线程启动阶段，GTK 尚未初始化
                unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
            }
            let socket = PathBuf::from(&wargs.socket);
            if let Err(e) = bar::run(&wargs.monitor, &socket) {
                eprintln!("[breadknife] window failed: {e}");
                std::process::exit(1);
            }
        }
        "open" | "close" if cmd.len() >= 2 => client_send(&control, &cmd.join(" ")),
        "open-all" | "close-all" | "reload" | "exit" => client_send(&control, &cmd.join(" ")),
        other => {
            eprintln!("[breadknife] unknown command: {other}\n{SOCKET_HELP}");
            std::process::exit(2);
        }
    }
}

/// 后台化命令 → 后台 daemon（bread daemon 自动后台化，ARCHITECTURE §3.3）。
fn start_daemon(control: &str) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[breadknife] cannot resolve own executable: {e}");
            std::process::exit(1);
        }
    };
    let mut command = std::process::Command::new(exe);
    command
        .arg("__daemon")
        .arg("--socket")
        .arg(control)
        .stdout(std::process::Stdio::null());
    // stderr 保持继承：日志仍可达启动终端
    match command.spawn() {
        Ok(child) => {
            eprintln!("[breadknife] daemon started (pid {})", child.id());
        }
        Err(e) => {
            eprintln!("[breadknife] cannot start daemon: {e}");
            std::process::exit(1);
        }
    }
}

fn client_send(control: &str, line: &str) {
    let mut stream = UnixStream::connect(control).unwrap_or_else(|e| {
        eprintln!("[breadknife] cannot connect to daemon at {control}: {e}");
        std::process::exit(1);
    });
    let _ = stream.write_all(line.as_bytes());
    let _ = stream.write_all(b"\n");
    eprintln!("[breadknife] sent: {line}");
}
