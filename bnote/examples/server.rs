//! 交互式通知服务器测试器（宿主 API 契约见 USER §4）。
//!
//! 以常驻 notification server 形式运行：占用会话总线的
//! `org.freedesktop.Notifications`；名称已被占用时直接退出（bnote 不排队、
//! 不替换已有守护）。stdout 打印全部库事件；stdin 命令驱动宿主侧行为：
//!
//!   list                    列出在册通知
//!   close <id> [reason]     宿主侧关闭并回发 NotificationClosed
//!                           （expired|dismissed|request，默认 dismissed）
//!   action <id> <key>       回发 ActionInvoked
//!   token <id> <token>      回发 ActivationToken
//!   info                    打印服务器信息与能力
//!   quit                    退出
//!
//! 外部客户端驱动示例：
//!
//!   notify-send [-u critical] [-r <replaces_id>] "标题" "正文"
//!   dunstify -A "default,Open" "标题"      # 客户端阻塞等 ActionInvoked
//!   gdbus call --session -d org.freedesktop.Notifications \
//!     -o /org/freedesktop/Notifications \
//!     -m org.freedesktop.Notifications.CloseNotification <id>
//!
//! 运行：
//!
//!   dbus-run-session -- cargo run --example server -- --caps actions,body,body-markup
//!
//! 示例采用同步宿主结构：主线程直接调用 SessionHandle 的阻塞命令，事件接收
//! 放在独立线程的 current-thread runtime 中。

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use bnote::{
    Capabilities, CloseReason, Error, Notice, NoticeEvent, NoticeReceiver, ServerInfo, Session,
    SessionConfig,
};

const HELP: &str = "\
命令：
  list                    列出在册通知
  close <id> [reason]     关闭并回发 NotificationClosed（expired|dismissed|request，默认 dismissed）
  action <id> <key>       回发 ActionInvoked
  token <id> <token>      回发 ActivationToken
  info                    打印服务器信息与能力
  quit                    退出";

enum Message {
    Event(NoticeEvent),
    Closed,
    Line(String),
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut name = "bnote-server".to_string();
    let mut vendor = "breadknife".to_string();
    let mut caps: Vec<String> = vec![
        "actions".into(),
        "body".into(),
        "body-markup".into(),
        "icon-static".into(),
    ];

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => name = args.next().unwrap_or(name),
            "--vendor" => vendor = args.next().unwrap_or(vendor),
            "--caps" => {
                caps = args
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            other => eprintln!("未知参数 {other}（支持 --name / --vendor / --caps）"),
        }
    }

    let info = ServerInfo {
        name,
        vendor,
        version: env!("CARGO_PKG_VERSION").into(),
        spec_version: "1.2".into(),
    };

    let config = SessionConfig {
        capabilities: Capabilities {
            entries: caps.clone(),
        },
        server_info: info.clone(),
        command_timeout_ms: 2000,
    };

    let (handle, receiver) = match Session::start(config) {
        Ok(pair) => pair,
        Err(Error::NameTaken(name)) => {
            eprintln!("无法启动：{name} 已被其他进程占用（bnote 不排队、不替换）");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("无法启动：{error}");
            return ExitCode::FAILURE;
        }
    };

    println!("通知服务器已就绪（org.freedesktop.Notifications）");
    println!("{HELP}");

    let (tx, rx) = mpsc::channel();
    let event_thread = spawn_event_thread(receiver, tx.clone());
    spawn_stdin_thread(tx.clone());
    drop(tx);

    let mut notices: BTreeMap<u32, Notice> = BTreeMap::new();

    while let Ok(message) = rx.recv() {
        match message {
            Message::Event(NoticeEvent::Notify { notice }) => {
                print_notice(&notice);
                notices.insert(notice.id, notice);
            }
            Message::Event(NoticeEvent::Close { id }) => {
                println!("← 客户端请求关闭 #{id}");
                notices.remove(&id);
            }
            Message::Event(NoticeEvent::Fatal { message }) => {
                println!("← 会话终止：{message}");
                break;
            }
            Message::Closed => {
                println!("← 事件流已结束");
                break;
            }
            Message::Line(line) => match handle_line(&handle, &mut notices, &info, &caps, &line) {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => println!("! {error}"),
            },
        }

        let _ = std::io::stdout().flush();
    }

    handle.shutdown();
    let _ = event_thread.join();
    println!("已停机");
    ExitCode::SUCCESS
}

fn spawn_event_thread(mut receiver: NoticeReceiver, tx: Sender<Message>) -> JoinHandle<()> {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("无法创建事件运行时：{error}");
                let _ = tx.send(Message::Closed);
                return;
            }
        };

        runtime.block_on(async move {
            while let Some(event) = receiver.recv().await {
                let fatal = matches!(&event, NoticeEvent::Fatal { .. });
                if tx.send(Message::Event(event)).is_err() {
                    return;
                }
                if fatal {
                    break;
                }
            }
            let _ = tx.send(Message::Closed);
        });
    })
}

fn spawn_stdin_thread(tx: Sender<Message>) {
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) => {
                    if tx.send(Message::Line(line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn handle_line(
    handle: &bnote::SessionHandle,
    notices: &mut BTreeMap<u32, Notice>,
    info: &ServerInfo,
    caps: &[String],
    line: &str,
) -> Result<bool, Error> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    match parts.as_slice() {
        ["list"] => {
            if notices.is_empty() {
                println!("（无在册通知）");
            }
            for (id, notice) in notices.iter() {
                let actions: Vec<&str> = notice
                    .actions
                    .iter()
                    .map(|action| action.key.as_str())
                    .collect();
                println!(
                    "#{id} [{:?}] {}：{}  动作：{}  超时：{}",
                    notice.urgency,
                    notice.summary,
                    notice.body,
                    if actions.is_empty() {
                        "-".to_string()
                    } else {
                        format!("{actions:?}")
                    },
                    timeout_label(notice.expire_timeout_ms),
                );
            }
        }
        ["close", id] => {
            close(handle, notices, id, CloseReason::Dismissed)?;
        }
        ["close", id, reason] => {
            close(handle, notices, id, parse_reason(reason)?)?;
        }
        ["action", id, key] => {
            let id = parse_id(id)?;
            handle.invoke_action(id, key)?;
            println!("→ ActionInvoked #{id} {key}");
        }
        ["token", id, token] => {
            let id = parse_id(id)?;
            handle.activation_token(id, token)?;
            println!("→ ActivationToken #{id} {token}");
        }
        ["info"] => {
            println!(
                "服务器：{} {}（规范 {}）\n能力：{}",
                info.name,
                info.version,
                info.spec_version,
                caps.join(", ")
            );
        }
        ["quit"] => return Ok(true),
        [] => {}
        _ => println!("{HELP}"),
    }

    Ok(false)
}

fn close(
    handle: &bnote::SessionHandle,
    notices: &mut BTreeMap<u32, Notice>,
    id: &str,
    reason: CloseReason,
) -> Result<(), Error> {
    let id = parse_id(id)?;
    handle.close(id, reason)?;
    notices.remove(&id);
    println!("→ NotificationClosed #{id} ({})", reason_label(reason));
    Ok(())
}

fn print_notice(notice: &Notice) {
    let actions: Vec<String> = notice
        .actions
        .iter()
        .map(|action| format!("{}={}", action.key, action.label))
        .collect();

    println!(
        "← Notify #{} [{:?}]\n    应用：{}  图标：{:?}\n    标题：{}\n    正文：{}",
        notice.id, notice.urgency, notice.app_name, notice.icon, notice.summary, notice.body
    );

    if !actions.is_empty() {
        println!("    动作：{actions:?}（回发：action {} <key>）", notice.id);
    }

    println!("    超时：{}", timeout_label(notice.expire_timeout_ms));
}

fn timeout_label(timeout: Option<u32>) -> String {
    match timeout {
        None => "宿主默认".to_string(),
        Some(0) => "常驻".to_string(),
        Some(ms) => format!("{ms}ms"),
    }
}

fn parse_id(value: &str) -> Result<u32, Error> {
    value
        .parse()
        .map_err(|_| Error::InvalidArgs(format!("id 不是数字：{value}")))
}

fn parse_reason(value: &str) -> Result<CloseReason, Error> {
    match value {
        "expired" => Ok(CloseReason::Expired),
        "dismissed" => Ok(CloseReason::Dismissed),
        "request" => Ok(CloseReason::CloseRequest),
        other => Err(Error::InvalidArgs(format!("未知原因：{other}"))),
    }
}

fn reason_label(reason: CloseReason) -> &'static str {
    match reason {
        CloseReason::Expired => "expired",
        CloseReason::Dismissed => "dismissed",
        CloseReason::CloseRequest => "request",
    }
}
