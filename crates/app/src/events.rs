use std::num::NonZeroUsize;

use tokio::sync::broadcast;

/// In-process fan-out of domain events to subscribers such as SSE streams.
#[derive(Debug, Clone)]
pub struct EventBus<E> {
    sender: broadcast::Sender<E>,
}

impl<E: Clone> EventBus<E> {
    #[must_use]
    pub fn new(capacity: NonZeroUsize) -> Self {
        let (sender, _) = broadcast::channel(capacity.get());
        Self { sender }
    }

    pub fn publish(&self, event: E) {
        // no subscribers is fine, nobody is listening yet
        let _ = self.sender.send(event);
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<E> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capacity(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    #[tokio::test]
    async fn delivers_events_to_every_subscriber() {
        let bus = EventBus::new(capacity(8));
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        bus.publish("changed");
        assert_eq!(first.recv().await.unwrap(), "changed");
        assert_eq!(second.recv().await.unwrap(), "changed");
    }

    #[test]
    fn publishing_without_subscribers_is_a_no_op() {
        EventBus::new(capacity(1)).publish(42);
    }
}
