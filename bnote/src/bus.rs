//! Internal bus domain: connection, runtime, command bridge and shutdown.
//!
//! The host never touches this module.  A dedicated thread owns a
//! current-thread Tokio runtime; the public synchronous commands are bridged
//! into it through a command channel.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::runtime::{Builder, Handle};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::time::{MissedTickBehavior, timeout};
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};

use crate::events::EventQueue;
use crate::server::{self, Server};
use crate::types::{NoticeEvent, SessionConfig};
use crate::{BUS_NAME, Error, PATH};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

type BoxResult = Result<Box<dyn Any + Send>, Error>;
type BoxFuture = Pin<Box<dyn Future<Output = BoxResult> + Send>>;
type StartupReceiver = std_mpsc::Receiver<Result<Bus, Error>>;

struct Command {
    run: Box<dyn FnOnce(Bus) -> BoxFuture + Send>,
    reply: std_mpsc::SyncSender<BoxResult>,
}

/// Cheap, cloneable handle to the runtime and its resources.
#[derive(Clone)]
pub(crate) struct Bus {
    pub(crate) conn: zbus::Connection,
    pub(crate) rt: Handle,
    pub(crate) events: Arc<EventQueue>,
    pub(crate) state: Arc<Mutex<server::State>>,
    cmds: mpsc::UnboundedSender<Command>,
    stop: watch::Sender<bool>,
    stopped: Arc<AtomicBool>,
    fatal: Arc<AtomicBool>,
    command_timeout_ms: u64,
}

impl Bus {
    /// Start the internal thread and runtime.  The returned receiver yields
    /// exactly one startup result.
    pub(crate) fn spawn_runtime(
        config: SessionConfig,
        events: Arc<EventQueue>,
    ) -> Result<(StartupReceiver, JoinHandle<()>), Error> {
        let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);

        let join = thread::Builder::new()
            .name("bnote-runtime".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(Error::Bus(format!(
                            "failed to build internal runtime: {error}"
                        ))));
                        return;
                    }
                };

                runtime.block_on(async move {
                    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
                    let (stop_tx, stop_rx) = watch::channel(false);
                    let stopped = Arc::new(AtomicBool::new(false));
                    let fatal = Arc::new(AtomicBool::new(false));
                    let state = Arc::new(Mutex::new(server::State::default()));

                    let bus = match connect(config, events, cmd_tx, stop_tx, stopped, fatal, state)
                        .await
                    {
                        Ok(bus) => bus,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };

                    bus.spawn(monitor(bus.clone()));
                    let _ = ready_tx.send(Ok(bus.clone()));
                    run_loop(bus, cmd_rx, stop_rx).await;
                });
            })
            .map_err(|error| Error::Bus(format!("failed to spawn runtime thread: {error}")))?;

        Ok((ready_rx, join))
    }

    /// Dispatch a future to the internal runtime and block until it finishes
    /// or the configured command timeout expires.
    pub(crate) fn cmd<T, F, Fut>(&self, f: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(Bus) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, Error>> + Send + 'static,
    {
        if self.is_stopped() {
            return Err(Error::Stopped);
        }

        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
        let run = Box::new(move |bus: Bus| -> BoxFuture {
            Box::pin(async move {
                let value = f(bus).await?;
                Ok(Box::new(value) as Box<dyn Any + Send>)
            })
        });

        if self
            .cmds
            .send(Command {
                run,
                reply: reply_tx,
            })
            .is_err()
        {
            return Err(Error::Stopped);
        }

        match reply_rx.recv_timeout(Duration::from_millis(self.command_timeout_ms)) {
            Ok(Ok(value)) => value.downcast::<T>().map(|value| *value).map_err(|_| {
                Error::Protocol("internal command returned an unexpected type".to_owned())
            }),
            Ok(Err(error)) => Err(error),
            Err(std_mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout {
                ms: self.command_timeout_ms,
            }),
            Err(std_mpsc::RecvTimeoutError::Disconnected) => Err(Error::Stopped),
        }
    }

    /// Spawn a background task on the internal runtime.
    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if !self.is_stopped() {
            self.rt.spawn(future);
        }
    }

    pub(crate) fn shutdown(&self) {
        if self.claim_stop() {
            self.events.close();
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn claim_stop(&self) -> bool {
        let first = self
            .stopped
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if first {
            let _ = self.stop.send(true);
        }
        first
    }

    fn report_fatal(&self, message: String) {
        if self.fatal.swap(true, Ordering::SeqCst) || !self.claim_stop() {
            return;
        }

        self.events.send(NoticeEvent::Fatal { message });
        self.events.close();
    }
}

async fn connect(
    config: SessionConfig,
    events: Arc<EventQueue>,
    cmds: mpsc::UnboundedSender<Command>,
    stop: watch::Sender<bool>,
    stopped: Arc<AtomicBool>,
    fatal: Arc<AtomicBool>,
    state: Arc<Mutex<server::State>>,
) -> Result<Bus, Error> {
    let SessionConfig {
        capabilities,
        server_info,
        command_timeout_ms,
    } = config;

    let conn = zbus::Connection::session().await.map_err(Error::from)?;

    let bus = Bus {
        conn,
        rt: Handle::current(),
        events: Arc::clone(&events),
        state: Arc::clone(&state),
        cmds,
        stop,
        stopped: Arc::clone(&stopped),
        fatal,
        command_timeout_ms,
    };

    let server = Server::new(capabilities, server_info, events, state, stopped);

    if !bus
        .conn
        .object_server()
        .at(PATH, server)
        .await
        .map_err(Error::from)?
    {
        return Err(Error::Bus(format!(
            "interface is already registered at {PATH}"
        )));
    }

    match bus
        .conn
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await
        .map_err(Error::from)?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(bus),
        RequestNameReply::InQueue | RequestNameReply::Exists => {
            Err(Error::NameTaken(BUS_NAME.to_owned()))
        }
    }
}

async fn run_loop(
    bus: Bus,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
            command = commands.recv() => {
                match command {
                    Some(command) => {
                        if bus.is_stopped() {
                            let _ = command.reply.send(Err(Error::Stopped));
                            break;
                        }

                        let bus = bus.clone();
                        tokio::spawn(async move {
                            let result = (command.run)(bus).await;
                            let _ = command.reply.send(result);
                        });
                    }
                    None => break,
                }
            }
        }
    }
}

async fn monitor(bus: Bus) {
    let proxy = match DBusProxy::new(&bus.conn).await {
        Ok(proxy) => proxy,
        Err(error) => {
            bus.report_fatal(format!("cannot create a bus proxy: {error}"));
            return;
        }
    };

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            _ = bus.conn.closed() => {
                bus.report_fatal("session bus connection closed".to_owned());
                break;
            }
            _ = heartbeat.tick() => {
                if bus.is_stopped() {
                    break;
                }

                match timeout(HEARTBEAT_TIMEOUT, proxy.get_id()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        bus.report_fatal(format!("session bus heartbeat failed: {error}"));
                        break;
                    }
                    Err(_) => {
                        bus.report_fatal("session bus heartbeat timed out".to_owned());
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_timer_is_reasonable() {
        assert!(HEARTBEAT_TIMEOUT < HEARTBEAT_INTERVAL);
    }
}
