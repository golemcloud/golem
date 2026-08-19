// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_broadcast::{Receiver, RecvError, Sender, TrySendError, broadcast};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LiveStreamEvent<T> {
    pub(crate) offset: u64,
    pub(crate) payload: LiveStreamEventPayload<T>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveStreamEventPayload<T> {
    Item(T),
    End,
    Error(String),
    Cancel(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveStreamBusCreateError {
    ZeroCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveStreamPublishError {
    Closed,
    Terminated,
    OffsetOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveStreamReceiveError {
    Closed,
    Lagged(u64),
}

struct PublishState {
    next_offset: u64,
    terminated: bool,
}

pub(crate) struct LiveStreamPublisher<T> {
    sender: Sender<LiveStreamEvent<T>>,
    state: Arc<Mutex<PublishState>>,
    primary_activated: Arc<AtomicBool>,
    primary_changed: Arc<Notify>,
}

impl<T> Clone for LiveStreamPublisher<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            state: self.state.clone(),
            primary_activated: self.primary_activated.clone(),
            primary_changed: self.primary_changed.clone(),
        }
    }
}

impl<T: Clone> LiveStreamPublisher<T> {
    pub(crate) async fn publish_item(&self, value: T) -> Result<u64, LiveStreamPublishError> {
        let mut state = self.state.lock().await;
        if state.terminated {
            return Err(LiveStreamPublishError::Terminated);
        }
        let following_offset = state
            .next_offset
            .checked_add(1)
            .ok_or(LiveStreamPublishError::OffsetOverflow)?;
        let offset = state.next_offset;
        self.send_event(LiveStreamEvent {
            offset,
            payload: LiveStreamEventPayload::Item(value),
        })
        .await?;
        state.next_offset = following_offset;
        Ok(offset)
    }

    pub(crate) async fn publish_end(&self) -> Result<u64, LiveStreamPublishError> {
        self.publish_terminal(LiveStreamEventPayload::End).await
    }

    pub(crate) async fn publish_error(&self, error: String) -> Result<u64, LiveStreamPublishError> {
        self.publish_terminal(LiveStreamEventPayload::Error(error))
            .await
    }

    pub(crate) async fn publish_cancel(
        &self,
        reason: String,
    ) -> Result<u64, LiveStreamPublishError> {
        self.publish_terminal(LiveStreamEventPayload::Cancel(reason))
            .await
    }

    async fn publish_terminal(
        &self,
        payload: LiveStreamEventPayload<T>,
    ) -> Result<u64, LiveStreamPublishError> {
        let mut state = self.state.lock().await;
        if state.terminated {
            return Err(LiveStreamPublishError::Terminated);
        }
        let offset = state.next_offset;
        self.send_event(LiveStreamEvent { offset, payload }).await?;
        state.terminated = true;
        Ok(offset)
    }

    async fn send_event(&self, event: LiveStreamEvent<T>) -> Result<(), LiveStreamPublishError> {
        if self.primary_activated.load(Ordering::Acquire) {
            self.sender
                .broadcast(event)
                .await
                .map_err(|_| LiveStreamPublishError::Closed)?;
            return Ok(());
        }

        match self.sender.try_broadcast(event) {
            Ok(_) => Ok(()),
            Err(TrySendError::Closed(_)) | Err(TrySendError::Inactive(_)) => {
                Err(LiveStreamPublishError::Closed)
            }
            Err(TrySendError::Full(event)) => loop {
                let changed = self.primary_changed.notified();
                if self.primary_activated.load(Ordering::Acquire) {
                    self.sender
                        .broadcast(event)
                        .await
                        .map_err(|_| LiveStreamPublishError::Closed)?;
                    return Ok(());
                }
                if self.sender.is_closed() {
                    return Err(LiveStreamPublishError::Closed);
                }
                changed.await;
            },
        }
    }

    pub(crate) fn subscribe_tail(&self) -> AuxiliaryLiveStreamSubscriber<T> {
        AuxiliaryLiveStreamSubscriber {
            receiver: self.sender.new_receiver(),
        }
    }

    pub(crate) fn close(&self) {
        self.sender.close();
    }
}

struct PrimaryDropGuard<T> {
    sender: Sender<LiveStreamEvent<T>>,
    on_drop: Arc<dyn Fn() + Send + Sync>,
    primary_changed: Arc<Notify>,
    armed: bool,
}

impl<T> Drop for PrimaryDropGuard<T> {
    fn drop(&mut self) {
        self.sender.close();
        self.primary_changed.notify_waiters();
        if self.armed {
            (self.on_drop)();
        }
    }
}

pub(crate) struct ReservedPrimaryLiveStreamSubscriber<T> {
    receiver: Option<Receiver<LiveStreamEvent<T>>>,
    drop_guard: Option<PrimaryDropGuard<T>>,
    primary_activated: Arc<AtomicBool>,
    primary_changed: Arc<Notify>,
}

impl<T> ReservedPrimaryLiveStreamSubscriber<T> {
    pub(crate) fn activate(mut self) -> PrimaryLiveStreamSubscriber<T> {
        self.primary_activated.store(true, Ordering::Release);
        self.primary_changed.notify_waiters();
        PrimaryLiveStreamSubscriber {
            receiver: self
                .receiver
                .take()
                .expect("reserved primary stream subscriber already activated"),
            drop_guard: self
                .drop_guard
                .take()
                .expect("reserved primary stream subscriber already activated"),
        }
    }
}

pub(crate) struct PrimaryLiveStreamSubscriber<T> {
    receiver: Receiver<LiveStreamEvent<T>>,
    drop_guard: PrimaryDropGuard<T>,
}

impl<T: Clone> PrimaryLiveStreamSubscriber<T> {
    pub(crate) async fn recv(&mut self) -> Result<LiveStreamEvent<T>, LiveStreamReceiveError> {
        let event = receive(&mut self.receiver).await?;
        if !matches!(&event.payload, LiveStreamEventPayload::Item(_)) {
            self.drop_guard.armed = false;
        }
        Ok(event)
    }
}

pub(crate) struct AuxiliaryLiveStreamSubscriber<T> {
    receiver: Receiver<LiveStreamEvent<T>>,
}

impl<T: Clone> AuxiliaryLiveStreamSubscriber<T> {
    pub(crate) async fn recv(&mut self) -> Result<LiveStreamEvent<T>, LiveStreamReceiveError> {
        receive(&mut self.receiver).await
    }
}

async fn receive<T: Clone>(
    receiver: &mut Receiver<LiveStreamEvent<T>>,
) -> Result<LiveStreamEvent<T>, LiveStreamReceiveError> {
    receiver.recv().await.map_err(|error| match error {
        RecvError::Closed => LiveStreamReceiveError::Closed,
        RecvError::Overflowed(missed) => LiveStreamReceiveError::Lagged(missed),
    })
}

pub(crate) fn live_stream_bus<T>(
    capacity: usize,
    on_primary_drop: impl Fn() + Send + Sync + 'static,
) -> Result<
    (
        LiveStreamPublisher<T>,
        ReservedPrimaryLiveStreamSubscriber<T>,
    ),
    LiveStreamBusCreateError,
> {
    if capacity == 0 {
        return Err(LiveStreamBusCreateError::ZeroCapacity);
    }
    let (mut sender, receiver) = broadcast(capacity);
    sender.set_overflow(false);
    let primary_activated = Arc::new(AtomicBool::new(false));
    let primary_changed = Arc::new(Notify::new());
    Ok((
        LiveStreamPublisher {
            sender: sender.clone(),
            state: Arc::new(Mutex::new(PublishState {
                next_offset: 0,
                terminated: false,
            })),
            primary_activated: primary_activated.clone(),
            primary_changed: primary_changed.clone(),
        },
        ReservedPrimaryLiveStreamSubscriber {
            receiver: Some(receiver),
            drop_guard: Some(PrimaryDropGuard {
                sender,
                on_drop: Arc::new(on_primary_drop),
                primary_changed: primary_changed.clone(),
                armed: true,
            }),
            primary_activated,
            primary_changed,
        },
    ))
}

pub(crate) fn live_output_stream_bus<T>(
    capacity: usize,
    invocation_cancellation: CancellationToken,
) -> Result<
    (
        LiveStreamPublisher<T>,
        ReservedPrimaryLiveStreamSubscriber<T>,
    ),
    LiveStreamBusCreateError,
> {
    live_stream_bus(capacity, move || invocation_cancellation.cancel())
}

pub(crate) fn live_input_stream_bus<T>(
    capacity: usize,
    stream_cancellation: CancellationToken,
    producer_notification: Arc<Notify>,
) -> Result<
    (
        LiveStreamPublisher<T>,
        ReservedPrimaryLiveStreamSubscriber<T>,
    ),
    LiveStreamBusCreateError,
> {
    live_stream_bus(capacity, move || {
        stream_cancellation.cancel();
        producer_notification.notify_one();
    })
}

#[cfg(test)]
mod tests {
    use super::{
        LiveStreamBusCreateError, LiveStreamEvent, LiveStreamEventPayload, LiveStreamPublishError,
        live_input_stream_bus, live_output_stream_bus, live_stream_bus,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use test_r::test;
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn rejects_zero_capacity() {
        let result = live_stream_bus::<u64>(0, || {});

        assert!(matches!(
            result,
            Err(LiveStreamBusCreateError::ZeroCapacity)
        ));
    }

    #[test]
    async fn fans_out_ordered_events_with_identical_offsets() {
        let (publisher, primary) = live_stream_bus(4, || {}).unwrap();
        let mut primary = primary.activate();
        let mut auxiliary = publisher.subscribe_tail();

        assert_eq!(publisher.publish_item("first").await, Ok(0));
        assert_eq!(publisher.publish_item("second").await, Ok(1));
        assert_eq!(publisher.publish_end().await, Ok(2));

        let expected = vec![
            LiveStreamEvent {
                offset: 0,
                payload: LiveStreamEventPayload::Item("first"),
            },
            LiveStreamEvent {
                offset: 1,
                payload: LiveStreamEventPayload::Item("second"),
            },
            LiveStreamEvent {
                offset: 2,
                payload: LiveStreamEventPayload::End,
            },
        ];
        let mut primary_events = Vec::new();
        let mut auxiliary_events = Vec::new();
        for _ in 0..3 {
            primary_events.push(primary.recv().await.unwrap());
            auxiliary_events.push(auxiliary.recv().await.unwrap());
        }
        assert_eq!(primary_events, expected);
        assert_eq!(auxiliary_events, expected);
    }

    #[test]
    async fn slowest_subscriber_applies_bounded_backpressure() {
        let (publisher, primary) = live_stream_bus(1, || {}).unwrap();
        let mut primary = primary.activate();
        let mut auxiliary = publisher.subscribe_tail();

        publisher.publish_item(1).await.unwrap();
        let blocked = tokio::spawn({
            let publisher = publisher.clone();
            async move { publisher.publish_item(2).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!blocked.is_finished());

        assert_eq!(primary.recv().await.unwrap().offset, 0);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!blocked.is_finished());
        assert_eq!(auxiliary.recv().await.unwrap().offset, 0);
        assert_eq!(blocked.await.unwrap(), Ok(1));
    }

    #[test]
    async fn late_subscriber_starts_at_the_current_tail() {
        let (publisher, primary) = live_stream_bus(4, || {}).unwrap();
        let mut primary = primary.activate();
        publisher.publish_item(1).await.unwrap();
        let mut late = publisher.subscribe_tail();
        publisher.publish_item(2).await.unwrap();

        assert_eq!(primary.recv().await.unwrap().offset, 0);
        assert_eq!(primary.recv().await.unwrap().offset, 1);
        assert_eq!(late.recv().await.unwrap().offset, 1);
    }

    #[test]
    async fn reserved_primary_buffers_one_item_and_backpressures_until_activation() {
        let (publisher, primary) = live_stream_bus(1, || {}).unwrap();

        assert_eq!(publisher.publish_item(1).await, Ok(0));
        let blocked = tokio::spawn({
            let publisher = publisher.clone();
            async move { publisher.publish_item(2).await }
        });
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());

        let mut primary = primary.activate();
        assert_eq!(primary.recv().await.unwrap().offset, 0);
        assert_eq!(blocked.await.unwrap(), Ok(1));
        assert_eq!(primary.recv().await.unwrap().offset, 1);
    }

    #[test]
    async fn reserved_primary_buffers_to_configured_capacity() {
        let (publisher, primary) = live_stream_bus(3, || {}).unwrap();

        for value in 1..=3 {
            assert_eq!(publisher.publish_item(value).await, Ok(value - 1));
        }
        let blocked = tokio::spawn({
            let publisher = publisher.clone();
            async move { publisher.publish_item(4).await }
        });
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());

        let mut primary = primary.activate();
        for offset in 0..3 {
            assert_eq!(primary.recv().await.unwrap().offset, offset);
        }
        assert_eq!(blocked.await.unwrap(), Ok(3));
        assert_eq!(primary.recv().await.unwrap().offset, 3);
    }

    #[test]
    async fn dropping_auxiliary_removes_only_its_backpressure() {
        let (publisher, primary) = live_stream_bus(1, || {}).unwrap();
        let mut primary = primary.activate();
        let auxiliary = publisher.subscribe_tail();
        publisher.publish_item(1).await.unwrap();
        drop(auxiliary);
        assert_eq!(primary.recv().await.unwrap().offset, 0);

        assert_eq!(publisher.publish_item(2).await, Ok(1));
        assert_eq!(primary.recv().await.unwrap().offset, 1);
    }

    #[test]
    async fn primary_loss_closes_the_bus_and_runs_its_scope_guard() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (publisher, primary) = live_stream_bus(1, {
            let cancelled = cancelled.clone();
            move || cancelled.store(true, Ordering::Release)
        })
        .unwrap();
        drop(primary);

        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(
            publisher.publish_item(1).await,
            Err(LiveStreamPublishError::Closed)
        );
    }

    #[test]
    fn output_primary_loss_cancels_the_invocation() {
        let invocation_cancellation = CancellationToken::new();
        let (_publisher, primary) =
            live_output_stream_bus::<u64>(1, invocation_cancellation.clone()).unwrap();

        drop(primary);

        assert!(invocation_cancellation.is_cancelled());
    }

    #[test]
    async fn input_primary_loss_is_stream_scoped_and_notifies_the_producer() {
        let invocation_cancellation = CancellationToken::new();
        let first_stream_cancellation = invocation_cancellation.child_token();
        let second_stream_cancellation = invocation_cancellation.child_token();
        let producer_notification = Arc::new(Notify::new());
        let (_first_publisher, first_primary) = live_input_stream_bus::<u64>(
            1,
            first_stream_cancellation.clone(),
            producer_notification.clone(),
        )
        .unwrap();
        let (second_publisher, second_primary) = live_input_stream_bus(
            1,
            second_stream_cancellation.clone(),
            Arc::new(Notify::new()),
        )
        .unwrap();
        let mut second_primary = second_primary.activate();
        let notification = producer_notification.notified();

        drop(first_primary);

        notification.await;
        assert!(first_stream_cancellation.is_cancelled());
        assert!(!second_stream_cancellation.is_cancelled());
        assert!(!invocation_cancellation.is_cancelled());
        assert_eq!(second_publisher.publish_item(1).await, Ok(0));
        assert_eq!(second_primary.recv().await.unwrap().offset, 0);
    }

    #[test]
    async fn stream_scoped_primary_loss_does_not_cancel_a_sibling_bus() {
        let first_cancelled = Arc::new(AtomicBool::new(false));
        let second_cancelled = Arc::new(AtomicBool::new(false));
        let (_first_publisher, first_primary) = live_stream_bus::<u64>(1, {
            let first_cancelled = first_cancelled.clone();
            move || first_cancelled.store(true, Ordering::Release)
        })
        .unwrap();
        let (second_publisher, second_primary) = live_stream_bus(1, {
            let second_cancelled = second_cancelled.clone();
            move || second_cancelled.store(true, Ordering::Release)
        })
        .unwrap();
        let mut second_primary = second_primary.activate();

        drop(first_primary);
        assert!(first_cancelled.load(Ordering::Acquire));
        assert!(!second_cancelled.load(Ordering::Acquire));
        assert_eq!(second_publisher.publish_item(1).await, Ok(0));
        assert_eq!(second_primary.recv().await.unwrap().offset, 0);
    }

    #[test]
    async fn terminal_is_unique_and_rejects_every_later_publish() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (publisher, primary) = live_stream_bus(4, {
            let cancelled = cancelled.clone();
            move || cancelled.store(true, Ordering::Release)
        })
        .unwrap();
        let mut primary = primary.activate();
        publisher.publish_item(1).await.unwrap();
        assert_eq!(publisher.publish_error("failed".to_string()).await, Ok(1));
        assert_eq!(
            publisher.publish_end().await,
            Err(LiveStreamPublishError::Terminated)
        );
        assert_eq!(
            publisher.publish_cancel("cancelled".to_string()).await,
            Err(LiveStreamPublishError::Terminated)
        );
        assert_eq!(
            publisher.publish_item(2).await,
            Err(LiveStreamPublishError::Terminated)
        );

        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(1)
        ));
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Error(error) if error == "failed"
        ));
        drop(primary);
        assert!(!cancelled.load(Ordering::Acquire));
    }

    #[test]
    async fn checked_offset_overflow_does_not_publish() {
        let (publisher, primary) = live_stream_bus(1, || {}).unwrap();
        let mut primary = primary.activate();
        publisher.state.lock().await.next_offset = u64::MAX;

        assert_eq!(
            publisher.publish_item(1).await,
            Err(LiveStreamPublishError::OffsetOverflow)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), primary.recv())
                .await
                .is_err()
        );
    }
}
