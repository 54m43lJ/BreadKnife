//! Unix socket 服务端：控制接口（§4.2 行协议，处理结果不回写）
//! 与 daemon ↔ 窗口 IPC 服务端（帧协议，请求应答 + 状态推送）。

use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::Sender;

use crate::daemon::DaemonEvent;
use crate::shared::protocol::{parse_control_line, read_frame, write_frame, Frame};

/// 控制接口：一行一条指令；客户端断开即收线。
pub fn serve_control(path: &str, tx: Sender<DaemonEvent>) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::thread::Builder::new()
        .name("control-accept".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => handle_control_conn(stream, tx.clone()),
                    Err(e) => eprintln!("[ipc] control accept error: {e}"),
                }
            }
        })
        .expect("spawn control-accept");
    eprintln!("[ipc] control socket listening on {path}");
    Ok(())
}

fn handle_control_conn(stream: UnixStream, tx: Sender<DaemonEvent>) {
    let _ = stream.set_nonblocking(false);
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match parse_control_line(trimmed) {
                    Some(cmd) => {
                        let _ = tx.send(DaemonEvent::Control(cmd));
                    }
                    None => eprintln!("[ipc] unknown control command: {trimmed}"),
                }
            }
            Err(e) => {
                eprintln!("[ipc] control read error: {e}");
                break;
            }
        }
    }
}

/// 窗口 IPC：每连接一个读线程（请求上行）+ 一个写线程（应答与推送下行）；
/// 全部下行经同一写线程串行化，避免帧交错。
pub fn serve_windows(path: &str, tx: Sender<DaemonEvent>) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::thread::Builder::new()
        .name("ipc-accept".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => handle_window_conn(stream, tx.clone()),
                    Err(e) => eprintln!("[ipc] window accept error: {e}"),
                }
            }
        })
        .expect("spawn ipc-accept");
    eprintln!("[ipc] window socket listening on {path}");
    Ok(())
}

fn handle_window_conn(stream: UnixStream, tx: Sender<DaemonEvent>) {
    let _ = stream.set_nonblocking(false);
    let Ok(read_half) = stream.try_clone() else { return };
    let Ok(write_half) = stream.try_clone() else { return };

    let (out_tx, out_rx) = std::sync::mpsc::channel::<Frame>();

    // 写线程：唯一持有写半套接字
    std::thread::Builder::new()
        .name("ipc-writer".into())
        .spawn(move || {
            let mut stream = write_half;
            for frame in out_rx {
                if write_frame(&mut stream, &frame).is_err() {
                    break; // 窗口已断开
                }
            }
        })
        .expect("spawn ipc-writer");

    // 读线程：请求上行（含首条 Hello）
    std::thread::Builder::new()
        .name("ipc-reader".into())
        .spawn(move || {
            let mut stream = read_half;
            while let Ok(frame) = read_frame(&mut stream) {
                match frame {
                    Frame::Request { id, request } => {
                        if tx
                            .send(DaemonEvent::WindowMessage { id, request, out: out_tx.clone() })
                            .is_err()
                        {
                            return;
                        }
                    }
                    other => eprintln!("[ipc] unexpected frame from window: {other:?}"),
                }
            }
            // EOF：进程退出由 ChildExited 路径处理，读线程到此为止
        })
        .expect("spawn ipc-reader");
}

/// 供 daemon 侧各线程回写应答。
pub fn reply(out: &Sender<Frame>, id: u64, response: crate::shared::protocol::Response) {
    let _ = out.send(Frame::Response { id, response });
}

/// 供 daemon 主循环推送（定向 / 广播共用）。
pub fn push(out: &Sender<Frame>, push: crate::shared::protocol::Push) {
    let _ = out.send(Frame::Push { push });
}

/// Request::Hello 的下行序：先全量快照（首连推送），后应答。
pub fn hello_reply(out: &Sender<Frame>, id: u64, snapshot: crate::shared::model::WindowSnapshot) {
    push(out, crate::shared::protocol::Push::Snapshot(snapshot));
    reply(out, id, crate::shared::protocol::Response::Ok);
}
