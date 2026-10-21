//! Event queue between the protocol server and the host receiver.
//!
//! The queue is unbounded so a slow host never blocks a D-Bus method call.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::NoticeReceiver;
use crate::types::NoticeEvent;

pub(crate) struct EventQueue {
    tx: Mutex<Option<mpsc::UnboundedSender<NoticeEvent>>>,
}

impl EventQueue {
    pub(crate) fn new() -> (Arc<Self>, NoticeReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                tx: Mutex::new(Some(tx)),
            }),
            NoticeReceiver::new(rx),
        )
    }

    /// Deliver an event.  Returns `false` after the receiver was dropped or
    /// the queue was closed.
    pub(crate) fn send(&self, event: NoticeEvent) -> bool {
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_ref() {
            Some(tx) => tx.send(event).is_ok(),
            None => false,
        }
    }

    /// Close the receiver side once all queued events have been delivered.
    pub(crate) fn close(&self) {
        let mut guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_delivers_queued_events_then_ends_the_stream() {
        let (queue, mut receiver) = EventQueue::new();

        assert!(queue.send(NoticeEvent::Close { id: 7 }));
        queue.close();
        assert!(!queue.send(NoticeEvent::Close { id: 8 }));

        assert_eq!(receiver.recv().await, Some(NoticeEvent::Close { id: 7 }));
        assert_eq!(receiver.recv().await, None);
    }

    #[tokio::test]
    async fn dropping_the_receiver_makes_send_fail() {
        let (queue, receiver) = EventQueue::new();
        drop(receiver);
        assert!(!queue.send(NoticeEvent::Close { id: 1 }));
    }
}
