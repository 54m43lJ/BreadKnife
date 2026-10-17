//! 交互式通知服务器测试器（随设计分支起草，API 契约见 ARCHITECTURE §4；
//! 实现分支建 crate 后即可编译运行）。
//!
//! 以常驻 notification server 形式占用会话总线的 org.freedesktop.Notifications，
//! stdout 打印全部库事件；stdin 命令驱动宿主侧行为，覆盖 bnote 的全部交互面：
//!
//!   list                    列出在册通知
//!   close <id> [reason]     宿主侧关闭并回发 NotificationClosed（expired|dismissed|request，默认 request）
//!   action <id> <key>       回发 ActionInvoked
//!   token <id> <token>      回发 ActivationToken
//!   info                    打印服务器信息与能力
//!   quit                    退出
//!
//! 外部客户端驱动示例：
//!   notify-send [-u critical] [-r <replaces_id>] "标题" "正文"
//!   dunstify -A "标签:键" "标题"      # 客户端阻塞等 ActionInvoked，本进程用 action 命令回发
//!   gdbus call --session -d org.freedesktop.Notifications \
//!     -o /org/freedesktop/Notifications \
//!     -m org.freedesktop.Notifications.CloseNotification <id>

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use bnote::{CloseReason, Notice, NoticeEvent, ServerInfo, Session};

const HELP: &str = "\
命令：
  list                    列出在册通知
  close <id> [reason]     关闭并回发 NotificationClosed（expired|dismissed|request，默认 request）
  action <id> <key>       回发 ActionInvoked
  token <id> <token>      回发 ActivationToken
  info                    打印服务器信息与能力
  quit                    退出";

#[tokio::main]
async fn main() -> Result<(), bnote::Error> {
    let mut args = std::env::args().skip(1);
    let mut name = "bnote-server".to_string();
    let mut vendor = "breadknife".to_string();
    let mut caps: Vec<String> = vec![
        "actions".into(), "body".into(), "body-markup".into(), "icon-static".into(),
    ];
    while let Some(a) = args.next() {
        match a.as_str() {
            "--name" => name = args.next().unwrap_or(name),
            "--vendor" => vendor = args.next().unwrap_or(vendor),
            "--caps" => {
                caps = args.next().unwrap_or_default().split(',').map(str::to_string).collect()
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

    let conn = zbus::Connection::session().await.map_err(bnote::Error::Bus)?;
    let (etx, mut erx) = tokio::sync::mpsc::channel(16);
    let _session = Session::start(
        &conn,
        etx,
        bnote::Capabilities { entries: caps.clone() },
        info.clone(),
    )?;

    let (ctx, crx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if ctx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });

    println!("通知服务器已就绪（org.freedesktop.Notifications）\n{HELP}");
    let mut notices: BTreeMap<u32, Notice> = BTreeMap::new();

    loop {
        tokio::select! {
            ev = erx.recv() => match ev {
                Some(NoticeEvent::Notify { notice }) => {
                    print_notice(&notice);
                    notices.insert(notice.id, notice);
                }
                Some(NoticeEvent::Close { id }) => {
                    println!("← 客户端请求关闭 #{id}");
                    notices.remove(&id);
                }
                None => break,
            },
            cmd = crx.recv() => {
                let Some(line) = cmd else { break };
                let ps: Vec<&str> = line.split_whitespace().collect();
                match ps.as_slice() {
                    ["list"] => {
                        if notices.is_empty() {
                            println!("（无在册通知）");
                        }
                        for (id, n) in &notices {
                            let acts: Vec<&str> = n.actions.iter().map(|a| a.key.as_str()).collect();
                            println!("#{} [{}] {}：{} {} {}",
                                id, n.urgency, n.summary, n.body,
                                if acts.is_empty() { "".into() } else { format!("动作{acts:?}") },
                                match n.expire_timeout_ms {
                                    None => "常驻".to_string(),
                                    Some(ms) => format!("超时 {ms}ms"),
                                });
                        }
                    }
                    ["close", id] => {
                        do_close(&conn, &mut notices, id, CloseReason::Dismissed).await?;
                    }
                    ["close", id, reason] => {
                        do_close(&conn, &mut notices, id, parse_reason(reason)?).await?;
                    }
                    ["action", id, key] => {
                        bnote::invoke_action(&conn, *id, key).await?;
                        println!("→ ActionInvoked #{id} {key}");
                    }
                    ["token", id, token] => {
                        bnote::activation_token(&conn, *id, token).await?;
                        println!("→ ActivationToken #{id} {token}");
                    }
                    ["info"] => {
                        println!("服务器：{} {}（规范 {}）\n能力：{}",
                            info.name, info.version, info.spec_version, caps.join(", "));
                    }
                    ["quit"] => break,
                    [] => {}
                    _ => println!("{HELP}"),
                }
                std::io::stdout().flush().ok();
            }
        }
    }
    Ok(())
}

fn print_notice(n: &Notice) {
    let acts: Vec<String> =
        n.actions.iter().map(|a| format!("{}={}", a.key, a.label)).collect();
    println!("← Notify #{} [{}]\n    应用：{}  图标：{:?}\n    标题：{}\n    正文：{}",
        n.id, n.urgency, n.app_name, n.icon, n.summary, n.body);
    if !acts.is_empty() {
        println!("    动作：{acts:?}（回发：action {} <key>）", n.id);
    }
    println!("    超时：{}",
        match n.expire_timeout_ms {
            None => "常驻".to_string(),
            Some(ms) => format!("{ms}ms"),
        });
}

async fn do_close(
    conn: &zbus::Connection,
    notices: &mut BTreeMap<u32, Notice>,
    id: &str,
    reason: CloseReason,
) -> Result<(), bnote::Error> {
    let id: u32 = id.parse().map_err(|_| bnote::Error::InvalidArgs("id 非数字".into()))?;
    bnote::close(conn, id, reason).await?;
    notices.remove(&id);
    println!("→ NotificationClosed #{id} ({reason})");
    Ok(())
}

fn parse_reason(s: &str) -> Result<CloseReason, bnote::Error> {
    Ok(match s {
        "expired" => CloseReason::Expired,
        "dismissed" => CloseReason::Dismissed,
        "request" => CloseReason::CloseRequest,
        other => return Err(bnote::Error::InvalidArgs(format!("未知原因 {other}"))),
    })
}
