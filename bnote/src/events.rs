//! Event queue between the protocol server and the host receiver.
//!
//! The queue is bounded: a slow host applies backpressure to D-Bus methods
//! instead of letting memory grow without limit.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, mpsc};

use crate::NoticeReceiver;
use crate::types::NoticeEvent;

/// Hard upper bound for queued events.  When the host is slow, normal sends
/// wait for capacity; the terminal `Fatal` event waits as well and is delivered
/// once a slot is free.
pub(crate) const EVENT_QUEUE_CAPACITY: usize = 64;

pub(crate) struct EventQueue {
    tx: Mutex<Option<mpsc::Sender<NoticeEvent>>>,
    closed: AtomicBool,
    closed_notify: Notify,
}

impl EventQueue {
    pub(crate) fn new() -> (Arc<Self>, NoticeReceiver) {
        let (tx, rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let queue = Arc::new(Self {
            tx: Mutex::new(Some(tx)),
            closed: AtomicBool::new(false),
            closed_notify: Notify::new(),
        });
        let receiver = NoticeReceiver::new(rx, Arc::clone(&queue));
        (queue, receiver)
    }

    /// Deliver a normal event, waiting for capacity when the host is slow.
    /// Returns `false` after the receiver was dropped or the queue was closed.
    pub(crate) async fn send(&self, event: NoticeEvent) -> bool {
        if self.is_closed() {
            return false;
        }

        let tx = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match tx {
            Some(tx) => tx.send(event).await.is_ok(),
            None => false,
        }
    }

    /// Deliver the terminal event.  It waits for capacity like normal events,
    /// so a full queue never drops the termination notice.
    pub(crate) async fn send_fatal(&self, message: String) -> bool {
        self.send(NoticeEvent::Fatal { message }).await
    }

    /// Resolve when the receiver side is gone or the queue was closed.
    pub(crate) async fn closed(&self) {
        if self.is_closed() {
            return;
        }

        let notified = self.closed_notify.notified();
        if self.is_closed() {
            return;
        }

        let tx = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(tx) = tx {
            tokio::select! {
                _ = tx.closed() => {}
                _ = notified => {}
            }
        }
    }

    /// Close the receiver side once all queued events have been delivered.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.take();
        drop(guard);
        self.closed_notify.notify_waiters();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Called by [`NoticeReceiver`] when the host drops the single consumer.
    pub(crate) fn receiver_dropped(&self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn close_delivers_queued_events_then_ends_the_stream() {
        let (queue, mut receiver) = EventQueue::new();

        assert!(queue.send(NoticeEvent::Close { id: 7 }).await);
        queue.close();
        assert!(!queue.send(NoticeEvent::Close { id: 8 }).await);

        assert_eq!(receiver.recv().await, Some(NoticeEvent::Close { id: 7 }));
        assert_eq!(receiver.recv().await, None);
    }

    #[tokio::test]
    async fn dropping_the_receiver_makes_send_fail() {
        let (queue, receiver) = EventQueue::new();
        drop(receiver);
        assert!(queue.is_closed());
        assert!(!queue.send(NoticeEvent::Close { id: 1 }).await);
    }

    #[tokio::test]
    async fn closed_wakes_when_the_queue_is_closed() {
        let (queue, _receiver) = EventQueue::new();
        let closed = queue.clone();
        let waiter = tokio::spawn(async move { closed.closed().await });

        tokio::task::yield_now().await;
        queue.close();

        tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn queue_has_a_hard_capacity() {
        let (queue, _receiver) = EventQueue::new();

        for id in 0..EVENT_QUEUE_CAPACITY as u32 {
            assert!(queue.send(NoticeEvent::Close { id }).await);
        }

        // The next event has no slot until the host consumes one.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                queue.send(NoticeEvent::Close { id: 999 }),
            )
            .await
            .is_err()
        );
    }
}
