mod app;
mod bar;
mod daemon;
mod hyprland;

use std::io::Write;
use std::os::unix::net::UnixStream;

use clap::Parser;
use gtk::prelude::*;

use std::rc::Rc;

use daemon::Daemon;

const APP_ID: &str = "com.breadknife.shell";

const SOCKET_HELP: &str = "\
SOCKET CONTROL:
    The daemon listens on a Unix socket for runtime control commands.
    Daemon and client must agree on the same path (default: /tmp/breadknife.sock,
    override both sides with -s/--socket).

    Protocol: plain text, one command per line, terminated by a newline (\\n).

    Commands:
        list          List currently loaded app modules (reply on daemon's stderr)
        start <APP>   Load app module APP on every connected monitor
        stop <APP>    Unload app module APP from all monitors
        exit          Shut the daemon down gracefully

    Examples:
        bread list                  # ask the daemon what is loaded
        bread start bar             # load the \"bar\" module on all monitors
        bread stop bar              # unload the \"bar\" module
        bread -s /tmp/my.sock exit  # shut down a daemon on a custom socket

    Known app modules: bar

    Note: the daemon writes its replies/log lines to its own stderr, so run the
    daemon in a visible terminal (or journal) to see command output.
";

#[derive(Parser)]
#[command(
    name = "bread",
    about = "BreadKnife Hyprland shell: run as daemon when given no command, act as control client otherwise",
    after_help = SOCKET_HELP,
    after_long_help = SOCKET_HELP
)]
struct Args {
    /// Unix socket the daemon binds to and the client connects to
    #[arg(short, long, default_value = "/tmp/breadknife.sock", value_name = "PATH")]
    socket: String,

    /// App module(s) to load at startup (repeatable); omit for default ["bar"]
    #[arg(short, long, value_name = "APP")]
    app: Vec<String>,

    /// Send a command to a running daemon instead of starting one;
    /// see SOCKET CONTROL below for the command list
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

fn main() {
    // VMware SVGA / vmwgfx 的 dmabuf 导入不可用，合成器会拒绝 GTK GL 渲染器
    // 分配的 buffer 并触发协议错误导致崩溃；回退到 cairo (shm) 渲染。
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: 单线程启动阶段，GTK 尚未初始化
        unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
    }

    let args = Args::parse();

    // ── client mode: forward command to daemon ───────────────────
    if !args.cmd.is_empty() {
        let msg = args.cmd.join(" ");
        let mut stream = UnixStream::connect(&args.socket).unwrap_or_else(|e| {
            eprintln!("[breadknife] cannot connect to daemon at {}: {}", args.socket, e);
            std::process::exit(1);
        });
        stream.write_all(msg.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        eprintln!("[breadknife] sent: {}", msg);
        return;
    }

    // ── daemon mode ──────────────────────────────────────────────
    let app_ids: Vec<String> = if args.app.is_empty() {
        vec!["bar".into()]
    } else {
        args.app
    };
    let socket_path = args.socket;

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::empty())
        .build();

    app.connect_activate(move |app| {
        let hold = app.hold();
        let event_bus = Rc::new(hyprland::EventBus::start());
        let daemon = Daemon::new(app, app::registry(), &socket_path, hold, event_bus);
        for id in &app_ids {
            daemon.load_app(id);
        }
        eprintln!("[breadknife] daemon ready, loaded: {:?}", app_ids);
    });

    app.run_with_args(&[] as &[&str]);
}
