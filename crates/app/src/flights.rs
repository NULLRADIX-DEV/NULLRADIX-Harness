//! One piece of work per key at a time: the others wait and then find its result.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

/// The lock of one key and how many flights use it, waiting or not.
struct Slot {
    lock: Arc<tokio::sync::Mutex<()>>,
    users: usize,
}

/// A lock per key, held across the work.
pub(crate) struct Flights<K> {
    slots: parking_lot::Mutex<HashMap<K, Slot>>,
}

impl<K> Default for Flights<K> {
    fn default() -> Self {
        Self {
            slots: parking_lot::Mutex::default(),
        }
    }
}

/// The turn of one key, handed to the next waiter on drop.
pub(crate) struct Flight<'a, K: Hash + Eq + Clone> {
    flights: &'a Flights<K>,
    key: K,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl<K: Hash + Eq + Clone> Flights<K> {
    pub(crate) async fn enter(&self, key: K) -> Flight<'_, K> {
        let lock = {
            let mut slots = self.slots.lock();
            let slot = slots.entry(key.clone()).or_insert_with(|| Slot {
                lock: Arc::default(),
                users: 0,
            });
            slot.users += 1;
            slot.lock.clone()
        };
        // The flight exists before the wait to count out a cancelled wait.
        let mut flight = Flight {
            flights: self,
            key,
            guard: None,
        };
        flight.guard = Some(lock.lock_owned().await);
        flight
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.lock().len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K: Hash + Eq + Clone> Drop for Flight<'_, K> {
    fn drop(&mut self) {
        self.guard.take();
        let mut slots = self.flights.slots.lock();
        if let Some(slot) = slots.get_mut(&self.key) {
            slot.users = slot.users.saturating_sub(1);
            if slot.users == 0 {
                slots.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_wait_cancelled_after_the_lock_was_handed_over_leaves_no_entry() {
        let flights = Flights::default();
        let first = flights.enter("icon").await;
        let mut waiting = Box::pin(flights.enter("icon"));
        // the waiter polls once and queues, then the lock is handed to it unpolled
        tokio::select! {
            biased;
            _ = &mut waiting => unreachable!("the first flight still holds the lock"),
            () = std::future::ready(()) => {}
        }
        drop(first);
        drop(waiting);
        assert!(flights.is_empty());
    }

    #[tokio::test]
    async fn a_slot_stays_while_anyone_waits_and_goes_with_the_last() {
        let flights = Flights::default();
        let first = flights.enter("icon").await;
        let mut waiting = Box::pin(flights.enter("icon"));
        tokio::select! {
            biased;
            _ = &mut waiting => unreachable!("the first flight still holds the lock"),
            () = std::future::ready(()) => {}
        }
        drop(first);
        // the waiter keeps the slot until it is done too
        assert_eq!(flights.len(), 1);
        let second = waiting.await;
        assert_eq!(flights.len(), 1);
        drop(second);
        assert!(flights.is_empty());
    }

    #[tokio::test]
    async fn other_keys_never_wait() {
        let flights = Flights::default();
        let _icon = flights.enter("icon").await;
        let _font = tokio::time::timeout(Duration::from_secs(5), flights.enter("font"))
            .await
            .expect("another key waited for the first");
        assert_eq!(flights.len(), 2);
    }
}
