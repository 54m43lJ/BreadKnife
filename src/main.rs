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

#[derive(Parser)]
struct Args {
    /// Unix socket path for runtime control
    #[arg(short, long, default_value = "/tmp/breadknife.sock")]
    socket: String,

    /// Apps to load at startup (repeatable); omit for defaults
    #[arg(short, long)]
    app: Vec<String>,

    /// Command to send to a running daemon (e.g. "start bar", "list", "exit")
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

fn main() {
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
