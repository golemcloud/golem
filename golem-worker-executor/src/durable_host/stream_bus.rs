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
use golem_common::base_model::durable_stream::{
    MAX_LIVE_JOIN_BUFFER_SIZE, MAX_LIVE_READERS_PER_STREAM, MIN_LIVE_JOIN_BUFFER_SIZE,
    StreamOffsetV1,
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DurableLiveStreamEvent<T> {
    pub(crate) offset: StreamOffsetV1,
    pub(crate) payload: T,
}

type TerminalLoad<T> = Pin<Box<dyn Future<Output = Result<T, DurableLiveStreamBusError>> + Send>>;

#[derive(Clone)]
pub(crate) enum QueuedDurableEvent<T> {
    Ready(DurableLiveStreamEvent<T>),
    Terminal {
        offset: StreamOffsetV1,
        load: Arc<dyn Fn(StreamOffsetV1) -> TerminalLoad<T> + Send + Sync>,
    },
}

impl<T> QueuedDurableEvent<T> {
    fn offset(&self) -> StreamOffsetV1 {
        match self {
            Self::Ready(event) => event.offset,
            Self::Terminal { offset, .. } => *offset,
        }
    }
}

impl<T> From<DurableLiveStreamEvent<T>> for QueuedDurableEvent<T> {
    fn from(event: DurableLiveStreamEvent<T>) -> Self {
        Self::Ready(event)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DurableLiveStreamBusError {
    InvalidCapacity,
    ReaderLimit,
    ReaderIdOverflow,
    NonIncreasingOffset,
    PublicationAborted,
    Retired,
}

type PublicationStatus = watch::Receiver<Option<Result<(), DurableLiveStreamBusError>>>;
pub(crate) type PublicationReceipt = Pin<
    Box<
        dyn Future<Output = Result<Result<(), DurableLiveStreamBusError>, watch::error::RecvError>>
            + Send,
    >,
>;

fn publication_receipt(
    mut status: PublicationStatus,
    retired: CancellationToken,
) -> PublicationReceipt {
    Box::pin(async move {
        loop {
            if retired.is_cancelled() {
                return Ok(Err(DurableLiveStreamBusError::Retired));
            }
            if let Some(result) = *status.borrow_and_update() {
                return Ok(result);
            }
            tokio::select! {
                biased;
                _ = retired.cancelled() => return Ok(Err(DurableLiveStreamBusError::Retired)),
                changed = status.changed() => changed?,
            }
        }
    })
}

#[derive(Default)]
struct PublicationOrder {
    tail: Option<PublicationStatus>,
    terminal: Option<DeferredTerminal>,
}

struct DeferredTerminal {
    offset: StreamOffsetV1,
    replayed: bool,
    previous: Option<PublicationStatus>,
    completed: watch::Sender<Option<Result<(), DurableLiveStreamBusError>>>,
}

impl DeferredTerminal {
    fn ready(&self) -> Result<bool, DurableLiveStreamBusError> {
        if self.completed.borrow().is_some() {
            return Ok(false);
        }
        match &self.previous {
            None => Ok(true),
            Some(previous) => match *previous.borrow() {
                Some(result) => result.map(|_| true),
                None if previous.has_changed().is_err() => {
                    Err(DurableLiveStreamBusError::PublicationAborted)
                }
                None => Ok(false),
            },
        }
    }
}

struct DurableLiveStreamBusState<T> {
    high_water: Option<StreamOffsetV1>,
    readers: HashMap<u64, mpsc::Sender<QueuedDurableEvent<T>>>,
}

/// Bounded live-tail optimization for events that have already committed to the producer oplog.
/// With no readers publication is a non-blocking high-water update. With readers, each reader has
/// its own bounded queue and the slowest attached reader backpressures publication without loss.
pub(crate) struct DurableLiveStreamBus<T> {
    capacity: usize,
    max_readers: usize,
    next_reader_id: AtomicU64,
    state: Mutex<DurableLiveStreamBusState<T>>,
    publication_order: std::sync::Mutex<PublicationOrder>,
    publication_progress: Arc<OnceLock<Arc<Notify>>>,
    retired: CancellationToken,
}

impl<T: Clone> DurableLiveStreamBus<T> {
    pub(crate) fn retire(&self) {
        self.retired.cancel();
    }

    pub(crate) fn ensure_available(&self) -> Result<(), DurableLiveStreamBusError> {
        if self.retired.is_cancelled() {
            Err(DurableLiveStreamBusError::Retired)
        } else {
            Ok(())
        }
    }

    async fn while_available<F: Future>(
        &self,
        future: F,
    ) -> Result<F::Output, DurableLiveStreamBusError> {
        tokio::select! {
            biased;
            _ = self.retired.cancelled() => Err(DurableLiveStreamBusError::Retired),
            result = future => {
                self.ensure_available()?;
                Ok(result)
            }
        }
    }

    pub(crate) fn new(capacity: usize) -> Result<Self, DurableLiveStreamBusError> {
        Self::with_reader_limit(capacity, MAX_LIVE_READERS_PER_STREAM)
    }

    pub(crate) fn from_committed_high_water(
        capacity: usize,
        high_water: Option<StreamOffsetV1>,
    ) -> Result<Self, DurableLiveStreamBusError> {
        let mut bus = Self::new(capacity)?;
        bus.state.get_mut().high_water = high_water;
        Ok(bus)
    }

    fn with_reader_limit(
        capacity: usize,
        max_readers: usize,
    ) -> Result<Self, DurableLiveStreamBusError> {
        if !(MIN_LIVE_JOIN_BUFFER_SIZE..=MAX_LIVE_JOIN_BUFFER_SIZE).contains(&capacity)
            || max_readers == 0
            || max_readers > MAX_LIVE_READERS_PER_STREAM
        {
            crate::metrics::durable_stream::record_limit_violation("live_join_capacity");
            return Err(DurableLiveStreamBusError::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            max_readers,
            next_reader_id: AtomicU64::new(0),
            state: Mutex::new(DurableLiveStreamBusState {
                high_water: None,
                readers: HashMap::new(),
            }),
            publication_order: std::sync::Mutex::new(PublicationOrder::default()),
            publication_progress: Arc::new(OnceLock::new()),
            retired: CancellationToken::new(),
        })
    }

    /// Installs a tail subscription and samples the committed high-water under the same lock used
    /// by publication. The returned receiver therefore either contains a concurrent event or the
    /// sampled high-water includes it for historical catch-up.
    pub(crate) async fn subscribe(
        &self,
    ) -> Result<DurableLiveStreamSubscription<T>, DurableLiveStreamBusError> {
        let mut state = self.while_available(self.state.lock()).await?;
        state.readers.retain(|_, sender| !sender.is_closed());
        if state.readers.len() >= self.max_readers {
            crate::metrics::durable_stream::record_limit_violation("live_readers");
            crate::metrics::durable_stream::record_live_join_rejected();
            return Err(DurableLiveStreamBusError::ReaderLimit);
        }
        let reader_id = self
            .next_reader_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| {
                crate::metrics::durable_stream::record_limit_violation("reader_id");
                crate::metrics::durable_stream::record_live_join_rejected();
                DurableLiveStreamBusError::ReaderIdOverflow
            })?;
        let (sender, receiver) = mpsc::channel(self.capacity);
        state.readers.insert(reader_id, sender);
        crate::metrics::durable_stream::reader_attached();
        Ok(DurableLiveStreamSubscription {
            reader_id,
            high_water: state.high_water,
            receiver,
            pending: None,
            publication_progress: self.publication_progress.clone(),
            retired: self.retired.clone(),
        })
    }

    /// Publishes an event only after its producer-oplog commit has completed.
    pub(crate) async fn publish_committed(
        &self,
        event: DurableLiveStreamEvent<T>,
    ) -> Result<(), DurableLiveStreamBusError> {
        let mut state = self.while_available(self.state.lock()).await?;
        if state
            .high_water
            .is_some_and(|high_water| event.offset <= high_water)
        {
            return Err(DurableLiveStreamBusError::NonIncreasingOffset);
        }
        state.high_water = Some(event.offset);

        let readers = state
            .readers
            .iter()
            .map(|(reader_id, sender)| (*reader_id, sender.clone()))
            .collect::<Vec<_>>();
        for (reader_id, sender) in readers {
            if sender.capacity() == 0 {
                crate::metrics::durable_stream::record_backpressure();
            }
            if self
                .while_available(sender.send(event.clone().into()))
                .await?
                .is_err()
            {
                state.readers.remove(&reader_id);
            }
        }
        Ok(())
    }

    /// Republishes an already committed event after replay. The durable offset may be at or below
    /// the current high-water; attached readers discard the overlap by offset.
    pub(crate) async fn republish_committed(
        &self,
        event: DurableLiveStreamEvent<T>,
    ) -> Result<(), DurableLiveStreamBusError> {
        let mut state = self.while_available(self.state.lock()).await?;
        if state
            .high_water
            .is_none_or(|high_water| event.offset > high_water)
        {
            state.high_water = Some(event.offset);
        }

        let readers = state
            .readers
            .iter()
            .map(|(reader_id, sender)| (*reader_id, sender.clone()))
            .collect::<Vec<_>>();
        for (reader_id, sender) in readers {
            if sender.capacity() == 0 {
                crate::metrics::durable_stream::record_backpressure();
            }
            if self
                .while_available(sender.send(event.clone().into()))
                .await?
                .is_err()
            {
                state.readers.remove(&reader_id);
            }
        }
        Ok(())
    }

    pub(crate) async fn unsubscribe(&self, reader_id: u64) {
        self.state.lock().await.readers.remove(&reader_id);
    }

    #[cfg(test)]
    async fn reader_count(&self) -> usize {
        self.state.lock().await.readers.len()
    }

    #[cfg(test)]
    pub(crate) async fn hold_state_lock_until(
        &self,
        acquired: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        let _state = self.state.lock().await;
        let _ = acquired.send(());
        let _ = release.await;
    }
}

impl<T: Clone + Send + 'static> DurableLiveStreamBus<T> {
    pub(crate) fn has_pending_deferred_terminal(&self) -> bool {
        self.publication_order
            .lock()
            .expect("publication order lock poisoned")
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal.completed.borrow().is_none())
    }

    /// Registers one durable terminal behind all earlier publications without retaining its
    /// payload or spawning a task. The producer dispatcher supplies the payload when ready.
    pub(crate) fn defer_terminal(
        &self,
        offset: StreamOffsetV1,
        replayed: bool,
        progress: Arc<Notify>,
    ) -> Result<PublicationReceipt, DurableLiveStreamBusError> {
        self.ensure_available()?;
        self.publication_progress.get_or_init(|| progress);
        let mut order = self
            .publication_order
            .lock()
            .expect("publication order lock poisoned");
        if let Some(terminal) = &order.terminal {
            if terminal.offset != offset {
                return Err(DurableLiveStreamBusError::NonIncreasingOffset);
            }
            return Ok(publication_receipt(
                terminal.completed.subscribe(),
                self.retired.clone(),
            ));
        }
        let (completed, status) = watch::channel(None);
        let previous = order.tail.replace(status.clone());
        order.terminal = Some(DeferredTerminal {
            offset,
            replayed,
            previous,
            completed,
        });
        drop(order);
        self.publication_progress.get().unwrap().notify_one();
        Ok(publication_receipt(status, self.retired.clone()))
    }

    /// Avoids loading a terminal payload while its predecessors or any reader are blocked.
    pub(crate) fn ready_deferred_terminal(
        &self,
    ) -> Result<Option<StreamOffsetV1>, DurableLiveStreamBusError> {
        self.ensure_available()?;
        let order = self
            .publication_order
            .lock()
            .expect("publication order lock poisoned");
        let Some(terminal) = &order.terminal else {
            return Ok(None);
        };
        if !terminal.ready()? {
            return Ok(None);
        }
        let Ok(state) = self.state.try_lock() else {
            return Ok(None);
        };
        if state
            .readers
            .values()
            .any(|sender| !sender.is_closed() && sender.capacity() == 0)
        {
            return Ok(None);
        }
        Ok(Some(terminal.offset))
    }

    /// Reserves every live reader's capacity before changing any queue or the high-water.
    /// A slow reader leaves the terminal pending for a later dispatcher pass.
    pub(crate) fn try_publish_deferred_terminal(
        &self,
        event: impl Into<QueuedDurableEvent<T>>,
    ) -> Result<bool, DurableLiveStreamBusError> {
        self.ensure_available()?;
        let event = event.into();
        let offset = event.offset();
        let order = self
            .publication_order
            .lock()
            .expect("publication order lock poisoned");
        let Some(terminal) = &order.terminal else {
            return Ok(false);
        };
        if offset != terminal.offset {
            return Err(DurableLiveStreamBusError::NonIncreasingOffset);
        }
        if !terminal.ready()? {
            return Ok(false);
        }
        let Ok(mut state) = self.state.try_lock() else {
            return Ok(false);
        };
        if !terminal.replayed && state.high_water.is_some_and(|head| offset <= head) {
            let error = DurableLiveStreamBusError::NonIncreasingOffset;
            terminal.completed.send_replace(Some(Err(error)));
            return Err(error);
        }
        state.readers.retain(|_, sender| !sender.is_closed());
        let mut permits = Vec::with_capacity(state.readers.len());
        for sender in state.readers.values() {
            match sender.clone().try_reserve_owned() {
                Ok(permit) => permits.push(permit),
                Err(mpsc::error::TrySendError::Closed(_)) => {}
                Err(mpsc::error::TrySendError::Full(_)) => return Ok(false),
            }
        }
        if state.high_water.is_none_or(|head| offset > head) {
            state.high_water = Some(offset);
        }
        for permit in permits {
            permit.send(event.clone());
        }
        terminal.completed.send_replace(Some(Ok(())));
        Ok(true)
    }

    pub(crate) fn fail_deferred_terminal(&self, error: DurableLiveStreamBusError) {
        let order = self
            .publication_order
            .lock()
            .expect("publication order lock poisoned");
        if let Some(terminal) = &order.terminal {
            let pending = terminal.completed.borrow().is_none();
            if pending {
                terminal.completed.send_replace(Some(Err(error)));
            }
        }
    }

    /// The producer calls this while holding its index lock to establish commit order.
    /// Reader backpressure is handled by the owned publisher, never under that lock.
    pub(crate) fn enqueue_batch(
        self: &Arc<Self>,
        events: Vec<DurableLiveStreamEvent<T>>,
        replayed: bool,
        keepalive: impl Send + 'static,
    ) -> PublicationReceipt {
        let (completed, next) = watch::channel(None);
        let previous = self
            .publication_order
            .lock()
            .expect("publication order lock poisoned")
            .tail
            .replace(next.clone());
        let bus = self.clone();
        tokio::spawn(async move {
            let _keepalive = keepalive;
            let result = async {
                bus.ensure_available()?;
                if let Some(previous) = previous {
                    publication_receipt(previous, bus.retired.clone())
                        .await
                        .map_err(|_| DurableLiveStreamBusError::PublicationAborted)??;
                }
                for event in events {
                    if replayed {
                        bus.republish_committed(event).await?;
                    } else {
                        bus.publish_committed(event).await?;
                    }
                }
                Ok(())
            }
            .await;
            completed.send_replace(Some(result));
            if let Some(progress) = bus.publication_progress.get() {
                progress.notify_one();
            }
        });
        publication_receipt(next, self.retired.clone())
    }
}

pub(crate) struct DurableLiveStreamSubscription<T> {
    reader_id: u64,
    pub(crate) high_water: Option<StreamOffsetV1>,
    receiver: mpsc::Receiver<QueuedDurableEvent<T>>,
    pending: Option<QueuedDurableEvent<T>>,
    publication_progress: Arc<OnceLock<Arc<Notify>>>,
    retired: CancellationToken,
}

impl<T: Clone> DurableLiveStreamSubscription<T> {
    pub(crate) fn reader_id(&self) -> u64 {
        self.reader_id
    }

    pub(crate) async fn recv(
        &mut self,
    ) -> Result<DurableLiveStreamEvent<T>, DurableLiveStreamBusError> {
        if self.pending.is_none() {
            self.pending = Some(tokio::select! {
                biased;
                _ = self.retired.cancelled() => return Err(DurableLiveStreamBusError::Retired),
                event = self.receiver.recv() => event.ok_or(DurableLiveStreamBusError::PublicationAborted)?,
            });
            if let Some(progress) = self.publication_progress.get() {
                progress.notify_one();
            }
        }
        if self.retired.is_cancelled() {
            return Err(DurableLiveStreamBusError::Retired);
        }
        let event = match self.pending.as_ref().unwrap() {
            QueuedDurableEvent::Ready(_) => {
                let Some(QueuedDurableEvent::Ready(event)) = self.pending.take() else {
                    unreachable!()
                };
                return Ok(event);
            }
            QueuedDurableEvent::Terminal { offset, load } => {
                let payload = tokio::select! {
                    biased;
                    _ = self.retired.cancelled() => return Err(DurableLiveStreamBusError::Retired),
                    payload = load(*offset) => payload?,
                };
                DurableLiveStreamEvent {
                    offset: *offset,
                    payload,
                }
            }
        };
        if self.retired.is_cancelled() {
            return Err(DurableLiveStreamBusError::Retired);
        }
        self.pending = None;
        Ok(event)
    }
}

impl<T> Drop for DurableLiveStreamSubscription<T> {
    fn drop(&mut self) {
        self.receiver.close();
        if let Some(progress) = self.publication_progress.get() {
            progress.notify_one();
        }
        crate::metrics::durable_stream::reader_detached();
    }
}

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

    #[cfg(test)]
    pub(crate) fn subscribe_tail(&self) -> AuxiliaryLiveStreamSubscriber<T> {
        AuxiliaryLiveStreamSubscriber {
            receiver: self.sender.new_receiver(),
        }
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

#[allow(dead_code)]
pub(crate) struct AuxiliaryLiveStreamSubscriber<T> {
    receiver: Receiver<LiveStreamEvent<T>>,
}

#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::{
        DurableLiveStreamBus, DurableLiveStreamBusError, DurableLiveStreamEvent,
        LiveStreamBusCreateError, LiveStreamEvent, LiveStreamEventPayload, LiveStreamPublishError,
        live_output_stream_bus, live_stream_bus,
    };
    use golem_common::base_model::OplogIndex;
    use golem_common::base_model::durable_stream::StreamOffsetV1;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use test_r::{test, timeout};
    use tokio::sync::{Notify, mpsc};
    use tokio_util::sync::CancellationToken;

    #[test]
    #[timeout("30s")]
    async fn queued_terminal_loads_only_on_receive_and_survives_cancelled_receive() {
        let bus = DurableLiveStreamBus::new(1).unwrap();
        let mut left = bus.subscribe().await.unwrap();
        let mut right = bus.subscribe().await.unwrap();
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let offset = StreamOffsetV1::new(OplogIndex::from_u64(17), 0);
        let receipt = bus
            .defer_terminal(offset, false, Arc::new(Notify::new()))
            .unwrap();
        assert!(
            bus.try_publish_deferred_terminal(super::QueuedDurableEvent::Terminal {
                offset,
                load: Arc::new({
                    let loads = loads.clone();
                    let release = release.clone();
                    move |requested| {
                        let loads = loads.clone();
                        let release = release.clone();
                        Box::pin(async move {
                            assert_eq!(requested, offset);
                            loads.fetch_add(1, Ordering::AcqRel);
                            release.notified().await;
                            Ok(vec![19; 128 * 1024])
                        })
                    }
                }),
            })
            .unwrap()
        );
        receipt.await.unwrap().unwrap();
        assert_eq!(loads.load(Ordering::Acquire), 0);
        {
            let read = left.recv();
            tokio::pin!(read);
            assert!(futures::poll!(&mut read).is_pending());
        }
        assert_eq!(loads.load(Ordering::Acquire), 1);
        release.notify_one();
        let event = left.recv().await.unwrap();
        assert_eq!(event.offset, offset);
        assert_eq!(event.payload, vec![19; 128 * 1024]);
        assert_eq!(loads.load(Ordering::Acquire), 2);
        {
            let read = right.recv();
            tokio::pin!(read);
            assert!(futures::poll!(&mut read).is_pending());
            bus.retire();
            assert_eq!(read.await, Err(DurableLiveStreamBusError::Retired));
        }
        assert_eq!(loads.load(Ordering::Acquire), 3);
        assert_eq!(right.recv().await, Err(DurableLiveStreamBusError::Retired));
    }

    #[test]
    #[timeout("30s")]
    async fn retirement_interrupts_blocked_publication_and_discards_queued_events() {
        let bus = Arc::new(DurableLiveStreamBus::new(1).unwrap());
        let mut reader = bus.subscribe().await.unwrap();
        let event = |index| DurableLiveStreamEvent {
            offset: StreamOffsetV1::new(OplogIndex::from_u64(index), 0),
            payload: index,
        };
        bus.publish_committed(event(7)).await.unwrap();
        let blocked = bus.publish_committed(event(11));
        tokio::pin!(blocked);
        assert!(futures::poll!(&mut blocked).is_pending());
        assert!(bus.state.try_lock().is_err());
        let subscriber = bus.subscribe();
        tokio::pin!(subscriber);
        assert!(futures::poll!(&mut subscriber).is_pending());

        bus.retire();
        assert!(matches!(
            subscriber.await,
            Err(DurableLiveStreamBusError::Retired)
        ));
        assert_eq!(reader.recv().await, Err(DurableLiveStreamBusError::Retired));
        assert_eq!(blocked.await, Err(DurableLiveStreamBusError::Retired));
        assert_eq!(reader.recv().await, Err(DurableLiveStreamBusError::Retired));
        assert_eq!(
            bus.republish_committed(event(7)).await,
            Err(DurableLiveStreamBusError::Retired)
        );
    }

    #[test]
    #[timeout("30s")]
    async fn retirement_releases_predecessor_waiters_and_deferred_terminals() {
        let bus = Arc::new(DurableLiveStreamBus::<u8>::new(1).unwrap());
        let mut reader = bus.subscribe().await.unwrap();
        let read = reader.recv();
        tokio::pin!(read);
        assert!(futures::poll!(&mut read).is_pending());
        let (_pending, previous) = tokio::sync::watch::channel(None);
        bus.publication_order.lock().unwrap().tail = Some(previous);
        let (keepalive, released) = tokio::sync::oneshot::channel::<()>();
        let publication = bus.enqueue_batch(vec![], false, keepalive);
        let terminal = bus
            .defer_terminal(
                StreamOffsetV1::new(OplogIndex::from_u64(19), 0),
                false,
                Arc::new(Notify::new()),
            )
            .unwrap();
        bus.retire();
        assert_eq!(read.await, Err(DurableLiveStreamBusError::Retired));
        assert_eq!(
            publication.await.unwrap(),
            Err(DurableLiveStreamBusError::Retired)
        );
        assert_eq!(
            terminal.await.unwrap(),
            Err(DurableLiveStreamBusError::Retired)
        );
        assert!(
            released.await.is_err(),
            "publisher must release its admission ownership"
        );
        assert_eq!(
            bus.ready_deferred_terminal(),
            Err(DurableLiveStreamBusError::Retired)
        );
    }

    #[test]
    #[timeout("30s")]
    async fn queued_batches_preserve_order_when_receipts_are_dropped() {
        let bus = Arc::new(DurableLiveStreamBus::new(1).unwrap());
        let mut left = bus.subscribe().await.unwrap();
        let mut right = bus.subscribe().await.unwrap();
        let event = |index, value| DurableLiveStreamEvent {
            offset: StreamOffsetV1::new(OplogIndex::from_u64(index), 0),
            payload: value,
        };
        // Both batches exceed each reader's capacity; the first receipt is abandoned.
        drop(bus.enqueue_batch(vec![event(10, 3), event(11, 7), event(12, 19)], false, ()));
        let terminal = bus.enqueue_batch(vec![event(13, 31)], false, ());
        let read_left = async {
            let mut values = Vec::new();
            for _ in 0..4 {
                values.push(left.recv().await.unwrap().payload);
            }
            values
        };
        let read_right = async {
            let mut values = Vec::new();
            for _ in 0..4 {
                values.push(right.recv().await.unwrap().payload);
            }
            values
        };
        let (left, right, result) = tokio::join!(read_left, read_right, terminal);
        assert_eq!(left, vec![3, 7, 19, 31]);
        assert_eq!(right, left);
        result.unwrap().unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn deferred_terminal_preserves_order_and_waits_for_every_reader() {
        let bus = Arc::new(DurableLiveStreamBus::new(1).unwrap());
        let progress = Arc::new(Notify::new());
        let mut left = bus.subscribe().await.unwrap();
        let mut right = bus.subscribe().await.unwrap();
        let event = |index, value| DurableLiveStreamEvent {
            offset: StreamOffsetV1::new(OplogIndex::from_u64(index), 0),
            payload: value,
        };
        let data = bus.enqueue_batch(vec![event(10, 3), event(11, 7)], false, ());
        drop(
            bus.defer_terminal(event(12, 19).offset, false, progress.clone())
                .unwrap(),
        );
        let terminal = bus
            .defer_terminal(event(12, 19).offset, false, progress)
            .unwrap();
        assert_eq!(bus.ready_deferred_terminal().unwrap(), None);
        assert_eq!(left.recv().await.unwrap().payload, 3);
        assert_eq!(right.recv().await.unwrap().payload, 3);
        data.await.unwrap().unwrap();
        assert_eq!(bus.ready_deferred_terminal().unwrap(), None);
        assert_eq!(left.recv().await.unwrap().payload, 7);
        assert!(!bus.try_publish_deferred_terminal(event(12, 19)).unwrap());
        assert!(matches!(
            left.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(right.recv().await.unwrap().payload, 7);
        assert_eq!(
            bus.ready_deferred_terminal().unwrap(),
            Some(event(12, 19).offset)
        );
        assert!(bus.try_publish_deferred_terminal(event(12, 19)).unwrap());
        terminal.await.unwrap().unwrap();
        assert_eq!(left.recv().await.unwrap().payload, 19);
        assert_eq!(right.recv().await.unwrap().payload, 19);
        assert_eq!(bus.ready_deferred_terminal().unwrap(), None);
        assert!(!bus.try_publish_deferred_terminal(event(12, 19)).unwrap());
        assert!(matches!(
            left.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    #[timeout("30s")]
    async fn deferred_terminal_failure_fences_successors_and_retry_receipts() {
        let bus = Arc::new(DurableLiveStreamBus::<u8>::new(1).unwrap());
        let offset = StreamOffsetV1::new(OplogIndex::from_u64(12), 0);
        let progress = Arc::new(Notify::new());
        let terminal = bus.defer_terminal(offset, false, progress.clone()).unwrap();
        bus.fail_deferred_terminal(DurableLiveStreamBusError::PublicationAborted);
        assert_eq!(
            terminal.await.unwrap(),
            Err(DurableLiveStreamBusError::PublicationAborted)
        );
        assert_eq!(
            bus.defer_terminal(offset, false, progress)
                .unwrap()
                .await
                .unwrap(),
            Err(DurableLiveStreamBusError::PublicationAborted)
        );
        assert_eq!(
            bus.enqueue_batch(
                vec![DurableLiveStreamEvent {
                    offset: StreamOffsetV1::new(OplogIndex::from_u64(13), 0),
                    payload: 7,
                }],
                false,
                ()
            )
            .await
            .unwrap(),
            Err(DurableLiveStreamBusError::PublicationAborted)
        );
        assert_eq!(bus.subscribe().await.unwrap().high_water, None);
    }

    #[test]
    #[timeout("30s")]
    async fn publication_failure_fences_later_batches() {
        let bus = Arc::new(DurableLiveStreamBus::new(1).unwrap());
        let event = |index| DurableLiveStreamEvent {
            offset: StreamOffsetV1::new(OplogIndex::from_u64(index), 0),
            payload: index,
        };
        bus.enqueue_batch(vec![event(10)], false, ())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            bus.enqueue_batch(vec![event(9)], false, ()).await.unwrap(),
            Err(DurableLiveStreamBusError::NonIncreasingOffset)
        );
        assert_eq!(
            bus.enqueue_batch(vec![event(11)], false, ()).await.unwrap(),
            Err(DurableLiveStreamBusError::NonIncreasingOffset)
        );
        assert_eq!(
            bus.subscribe().await.unwrap().high_water,
            Some(StreamOffsetV1::new(OplogIndex::from_u64(10), 0))
        );
    }

    #[test]
    async fn durable_bus_supports_zero_readers_and_subscribe_high_water() {
        let bus = DurableLiveStreamBus::new(2).unwrap();
        let first = StreamOffsetV1::new(OplogIndex::from_u64(10), 0);
        bus.publish_committed(DurableLiveStreamEvent {
            offset: first,
            payload: "historical",
        })
        .await
        .unwrap();

        let mut subscription = bus.subscribe().await.unwrap();
        assert_eq!(subscription.high_water, Some(first));
        let second = StreamOffsetV1::new(OplogIndex::from_u64(11), 0);
        bus.publish_committed(DurableLiveStreamEvent {
            offset: second,
            payload: "live",
        })
        .await
        .unwrap();
        assert_eq!(subscription.recv().await.unwrap().offset, second);
    }

    #[test]
    #[timeout("30s")]
    async fn durable_bus_enforces_reader_limit_and_per_reader_backpressure() {
        let bus = Arc::new(DurableLiveStreamBus::with_reader_limit(1, 2).unwrap());
        let mut fast = bus.subscribe().await.unwrap();
        let mut slow = bus.subscribe().await.unwrap();
        assert!(matches!(
            bus.subscribe().await,
            Err(DurableLiveStreamBusError::ReaderLimit)
        ));

        bus.publish_committed(DurableLiveStreamEvent {
            offset: StreamOffsetV1::new(OplogIndex::from_u64(10), 0),
            payload: 1,
        })
        .await
        .unwrap();
        let blocked = tokio::spawn({
            let bus = bus.clone();
            async move {
                bus.publish_committed(DurableLiveStreamEvent {
                    offset: StreamOffsetV1::new(OplogIndex::from_u64(11), 0),
                    payload: 2,
                })
                .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());
        fast.recv().await.unwrap();
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());
        slow.recv().await.unwrap();
        assert_eq!(blocked.await.unwrap(), Ok(()));
    }

    #[test]
    async fn durable_bus_rejects_non_increasing_committed_offsets() {
        let bus = DurableLiveStreamBus::new(1).unwrap();
        let offset = StreamOffsetV1::new(OplogIndex::from_u64(10), 0);
        bus.publish_committed(DurableLiveStreamEvent { offset, payload: 1 })
            .await
            .unwrap();
        assert_eq!(
            bus.publish_committed(DurableLiveStreamEvent { offset, payload: 2 })
                .await,
            Err(DurableLiveStreamBusError::NonIncreasingOffset)
        );
        assert_eq!(bus.reader_count().await, 0);
    }

    #[test]
    async fn durable_bus_allows_repair_publication_without_moving_high_water_backwards() {
        let bus = DurableLiveStreamBus::new(2).unwrap();
        let first = StreamOffsetV1::new(OplogIndex::from_u64(10), 0);
        let second = StreamOffsetV1::new(OplogIndex::from_u64(11), 0);
        bus.publish_committed(DurableLiveStreamEvent {
            offset: first,
            payload: 1,
        })
        .await
        .unwrap();
        bus.publish_committed(DurableLiveStreamEvent {
            offset: second,
            payload: 2,
        })
        .await
        .unwrap();
        let mut subscription = bus.subscribe().await.unwrap();

        bus.republish_committed(DurableLiveStreamEvent {
            offset: first,
            payload: 1,
        })
        .await
        .unwrap();
        assert_eq!(subscription.recv().await.unwrap().offset, first);
        let later = bus.subscribe().await.unwrap();
        assert_eq!(later.high_water, Some(second));
    }

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
