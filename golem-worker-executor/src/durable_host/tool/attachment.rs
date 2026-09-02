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

use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError,
};
use crate::services::active_agents::{ActiveAgents, MemoryGrant};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio::sync::Notify;
use wasmtime::AsContextMut;
use wasmtime::component::{Destination, StreamProducer, StreamResult, VecBuffer};

type MemoryReservation = Pin<Box<dyn Future<Output = Option<MemoryGrant>> + Send + 'static>>;

#[derive(Clone)]
pub(crate) struct AttachmentMemory {
    reserve_tracked: Arc<dyn Fn(u64) -> MemoryReservation + Send + Sync>,
    tracking_enabled: Arc<AtomicBool>,
    tracking_pending: Arc<AtomicBool>,
    prepared_grant: Arc<Mutex<Option<MemoryGrant>>>,
    historical_charges: Arc<HistoricalAttachmentCharges>,
    live_activation: Arc<tokio::sync::Mutex<()>>,
}

impl AttachmentMemory {
    pub(crate) fn tracked<Ctx: WorkerCtx>(active_agents: Arc<ActiveAgents<Ctx>>) -> Self {
        Self {
            reserve_tracked: Arc::new(move |bytes| {
                let active_agents = active_agents.clone();
                Box::pin(async move { active_agents.try_acquire(bytes).await })
            }),
            tracking_enabled: Arc::new(AtomicBool::new(true)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn for_store<Ctx: WorkerCtx>(
        active_agents: Arc<ActiveAgents<Ctx>>,
        is_live: bool,
    ) -> Self {
        let memory = Self::tracked(active_agents);
        memory.tracking_enabled.store(is_live, Ordering::Release);
        memory
    }

    #[cfg(test)]
    pub(crate) fn inert() -> Self {
        Self {
            reserve_tracked: Arc::new(|bytes| {
                Box::pin(async move { Some(MemoryGrant::inert(bytes)) })
            }),
            tracking_enabled: Arc::new(AtomicBool::new(true)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_reservation<F, Fut>(is_live: bool, reserve: F) -> Self
    where
        F: Fn(u64) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<MemoryGrant>> + Send + 'static,
    {
        Self {
            reserve_tracked: Arc::new(move |bytes| Box::pin(reserve(bytes))),
            tracking_enabled: Arc::new(AtomicBool::new(is_live)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn reserve(&self, bytes: usize) -> AttachmentReservationOutcome {
        if self.tracking_pending.load(Ordering::Acquire) {
            return AttachmentReservationOutcome::Pending;
        }
        let historical = !self.tracking_enabled.load(Ordering::Acquire);
        if historical {
            match self.historical_charges.reserve(bytes as u64) {
                Some(historical_reservation) => {
                    AttachmentReservationOutcome::Reserved(AttachmentReservation {
                        grant: MemoryGrant::inert(bytes as u64),
                        historical_reservation: Some(historical_reservation),
                    })
                }
                None => AttachmentReservationOutcome::Pending,
            }
        } else {
            match self.reserve_tracked(bytes as u64).await {
                Some(grant) => AttachmentReservationOutcome::Reserved(AttachmentReservation {
                    grant,
                    historical_reservation: None,
                }),
                None => AttachmentReservationOutcome::Rejected,
            }
        }
    }

    async fn reserve_tracked(&self, bytes: u64) -> Option<MemoryGrant> {
        (self.reserve_tracked)(bytes).await
    }

    fn tracking_enabled(&self) -> bool {
        self.tracking_enabled.load(Ordering::Acquire)
    }

    fn tracking_pending(&self) -> bool {
        self.tracking_pending.load(Ordering::Acquire)
    }

    fn begin_tracking(&self) {
        self.tracking_pending.store(true, Ordering::Release);
        self.historical_charges.close();
    }

    fn prepare_grant(&self, grant: MemoryGrant) {
        *self.prepared_grant.lock().unwrap() = Some(grant);
    }

    fn commit_tracking(&self) {
        if let Some(grant) = self.prepared_grant.lock().unwrap().take() {
            self.historical_charges.install_upgrade(grant);
        }
        self.tracking_enabled.store(true, Ordering::Release);
        self.tracking_pending.store(false, Ordering::Release);
    }

    fn abort_tracking(&self) {
        self.prepared_grant.lock().unwrap().take();
        if !self.tracking_enabled() {
            self.historical_charges.reopen();
        }
        self.tracking_pending.store(false, Ordering::Release);
    }
}

struct HistoricalAttachmentCharges {
    state: Mutex<HistoricalAttachmentChargeState>,
}

struct HistoricalAttachmentChargeState {
    bytes: u64,
    accepting: bool,
    upgrade_grant: Option<MemoryGrant>,
}

impl Default for HistoricalAttachmentCharges {
    fn default() -> Self {
        Self {
            state: Mutex::new(HistoricalAttachmentChargeState {
                bytes: 0,
                accepting: true,
                upgrade_grant: None,
            }),
        }
    }
}

impl HistoricalAttachmentCharges {
    fn reserve(self: &Arc<Self>, bytes: u64) -> Option<HistoricalAttachmentReservation> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return None;
        }
        state.bytes = state
            .bytes
            .checked_add(bytes)
            .expect("historical attachment charge overflow");
        Some(HistoricalAttachmentReservation {
            charges: Some(self.clone()),
            bytes,
        })
    }

    fn release(&self, bytes: u64) {
        let mut state = self.state.lock().unwrap();
        assert!(
            state.bytes >= bytes,
            "historical attachment charge underflow"
        );
        state.bytes -= bytes;
        if state.bytes == 0 {
            state.upgrade_grant.take();
        }
    }

    fn bytes(&self) -> u64 {
        self.state.lock().unwrap().bytes
    }

    fn install_upgrade(&self, grant: MemoryGrant) {
        let mut state = self.state.lock().unwrap();
        if state.bytes != 0 {
            state.upgrade_grant = Some(grant);
        }
    }

    fn close(&self) {
        self.state.lock().unwrap().accepting = false;
    }

    fn reopen(&self) {
        self.state.lock().unwrap().accepting = true;
    }
}

struct HistoricalAttachmentReservation {
    charges: Option<Arc<HistoricalAttachmentCharges>>,
    bytes: u64,
}

impl HistoricalAttachmentReservation {
    fn into_charges(mut self) -> Arc<HistoricalAttachmentCharges> {
        self.charges
            .take()
            .expect("historical attachment reservation must own its charges")
    }
}

impl Drop for HistoricalAttachmentReservation {
    fn drop(&mut self) {
        if let Some(charges) = self.charges.take() {
            charges.release(self.bytes);
        }
    }
}

struct AttachmentTrackingRollback<'a> {
    memory: &'a AttachmentMemory,
    changed: &'a Notify,
    armed: bool,
}

impl AttachmentTrackingRollback<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AttachmentTrackingRollback<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.memory.abort_tracking();
            self.changed.notify_waiters();
        }
    }
}

enum AttachmentReservationOutcome {
    Reserved(AttachmentReservation),
    Pending,
    Rejected,
}

struct AttachmentReservation {
    grant: MemoryGrant,
    historical_reservation: Option<HistoricalAttachmentReservation>,
}

struct BufferedChunk {
    bytes: Vec<u8>,
    charge: AttachmentCharge,
}

struct AttachmentCharge {
    grant: MemoryGrant,
    charged_bytes: Arc<AtomicU64>,
    historical_charges: Option<Arc<HistoricalAttachmentCharges>>,
}

impl AttachmentCharge {
    fn new(reservation: AttachmentReservation, charged_bytes: Arc<AtomicU64>) -> Self {
        let AttachmentReservation {
            grant,
            historical_reservation,
        } = reservation;
        charged_bytes.fetch_add(grant.bytes(), Ordering::AcqRel);
        let historical_charges =
            historical_reservation.map(HistoricalAttachmentReservation::into_charges);
        Self {
            grant,
            charged_bytes,
            historical_charges,
        }
    }
}

impl Drop for AttachmentCharge {
    fn drop(&mut self) {
        if let Some(historical_charges) = &self.historical_charges {
            historical_charges.release(self.grant.bytes());
        }
        self.charged_bytes
            .fetch_sub(self.grant.bytes(), Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttachmentMode {
    Pending,
    Live,
    Completion { published: bool },
    TerminalOnly,
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolAttachmentModeMetadata {
    Pending,
    Live,
    CompletionStaged,
    CompletionPublished,
    TerminalOnly,
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolAttachmentTerminalMetadata {
    Finished,
    Cancelled,
    Abandoned,
    ResourceExhausted,
    Failed,
    ConsumerCancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolAttachmentMetadata {
    pub mode: ToolAttachmentModeMetadata,
    pub capacity_bytes: usize,
    pub accepted_bytes: u64,
    pub delivered_bytes: u64,
    pub buffered_bytes: usize,
    pub charged_bytes: u64,
    pub backpressured: bool,
    pub terminal_selected: bool,
    pub terminal: Option<ToolAttachmentTerminalMetadata>,
    pub owner_fenced: bool,
    pub host_resource_exhausted: bool,
    pub producer_operation_active: bool,
    pub producer_active: bool,
    pub consumer_active: bool,
}

pub(super) fn terminal_metadata(cause: &ByteStreamCloseCause) -> ToolAttachmentTerminalMetadata {
    match cause {
        ByteStreamCloseCause::Finished => ToolAttachmentTerminalMetadata::Finished,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled) => {
            ToolAttachmentTerminalMetadata::Cancelled
        }
        ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned) => {
            ToolAttachmentTerminalMetadata::Abandoned
        }
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted) => {
            ToolAttachmentTerminalMetadata::ResourceExhausted
        }
        ByteStreamCloseCause::Failed(ByteStreamFailure::Failed(_)) => {
            ToolAttachmentTerminalMetadata::Failed
        }
        ByteStreamCloseCause::ConsumerCancelled => {
            ToolAttachmentTerminalMetadata::ConsumerCancelled
        }
    }
}

struct AttachmentState {
    mode: AttachmentMode,
    chunks: VecDeque<BufferedChunk>,
    buffered_bytes: usize,
    accepted_completion_bytes: usize,
    accepted_bytes: u64,
    delivered_bytes: u64,
    terminal: Option<ByteStreamCloseCause>,
    owner_fenced: bool,
    host_resource_exhausted: bool,
    terminal_item_delivered: bool,
    producer_active: bool,
    consumer_active: bool,
    reader_waker: Option<Waker>,
}

struct ByteAttachment {
    limit: usize,
    memory: AttachmentMemory,
    charged_bytes: Arc<AtomicU64>,
    producer_operation: AtomicBool,
    state: Mutex<AttachmentState>,
    changed: Notify,
    terminal_changed: Notify,
}

impl ByteAttachment {
    fn new(limit: usize, memory: AttachmentMemory) -> Arc<Self> {
        Arc::new(Self {
            limit,
            memory,
            charged_bytes: Arc::new(AtomicU64::new(0)),
            producer_operation: AtomicBool::new(false),
            state: Mutex::new(AttachmentState {
                mode: AttachmentMode::Pending,
                chunks: VecDeque::new(),
                buffered_bytes: 0,
                accepted_completion_bytes: 0,
                accepted_bytes: 0,
                delivered_bytes: 0,
                terminal: None,
                owner_fenced: false,
                host_resource_exhausted: false,
                terminal_item_delivered: false,
                producer_active: true,
                consumer_active: true,
                reader_waker: None,
            }),
            changed: Notify::new(),
            terminal_changed: Notify::new(),
        })
    }

    fn discard(memory: AttachmentMemory) -> Arc<Self> {
        Arc::new(Self {
            limit: 0,
            memory,
            charged_bytes: Arc::new(AtomicU64::new(0)),
            producer_operation: AtomicBool::new(false),
            state: Mutex::new(AttachmentState {
                mode: AttachmentMode::Discard,
                chunks: VecDeque::new(),
                buffered_bytes: 0,
                accepted_completion_bytes: 0,
                accepted_bytes: 0,
                delivered_bytes: 0,
                terminal: None,
                owner_fenced: false,
                host_resource_exhausted: false,
                terminal_item_delivered: false,
                producer_active: true,
                consumer_active: false,
                reader_waker: None,
            }),
            changed: Notify::new(),
            terminal_changed: Notify::new(),
        })
    }

    fn begin_producer_operation(self: &Arc<Self>) -> Result<ProducerOperation, StreamWriteError> {
        self.producer_operation
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ProducerOperation(self.clone()))
            .map_err(|_| StreamWriteError::ConcurrentOperation)
    }

    fn configure(&self, mode: AttachmentMode) -> bool {
        let reader = {
            let mut state = self.state.lock().unwrap();
            if state.mode != AttachmentMode::Pending {
                return false;
            }
            if mode == AttachmentMode::Live {
                state.accepted_completion_bytes = 0;
            }
            state.mode = mode;
            state.reader_waker.take()
        };
        tracing::debug!(attachment = ?(self as *const Self), ?mode, "configured tool attachment");
        self.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        true
    }

    async fn prepare_live_memory_accounting(&self) -> bool {
        let _activation = self.memory.live_activation.lock().await;
        if self.memory.tracking_enabled() {
            return true;
        }
        if self.memory.tracking_pending() {
            return self.memory.prepared_grant.lock().unwrap().is_some();
        }

        self.memory.begin_tracking();
        self.changed.notify_waiters();
        let mut rollback = AttachmentTrackingRollback {
            memory: &self.memory,
            changed: &self.changed,
            armed: true,
        };
        let mut grant = MemoryGrant::inert(0);
        loop {
            let historical_bytes = self.memory.historical_charges.bytes();
            if historical_bytes <= grant.bytes() {
                self.memory.prepare_grant(grant);
                rollback.disarm();
                return true;
            }
            let Some(additional) = self
                .memory
                .reserve_tracked(historical_bytes - grant.bytes())
                .await
            else {
                rollback.disarm();
                return false;
            };
            grant.merge(additional);
        }
    }

    fn commit_live_memory_accounting(&self) {
        if self.memory.tracking_enabled() {
            return;
        }
        self.memory.commit_tracking();
        self.changed.notify_waiters();
    }

    fn complete_rejected_live_memory_accounting(&self) {
        {
            let mut state = self.state.lock().unwrap();
            if matches!(
                state.mode,
                AttachmentMode::Pending
                    | AttachmentMode::Live
                    | AttachmentMode::Completion { published: false }
            ) {
                state.chunks.clear();
                state.buffered_bytes = 0;
                state.accepted_completion_bytes = 0;
            }
        }
        let _ = self.select_terminal_with_origin(
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
            true,
        );
        self.publish_no_body_terminal();
    }

    fn publish_completion(&self) -> bool {
        let reader = {
            let mut state = self.state.lock().unwrap();
            let AttachmentMode::Completion { published: false } = state.mode else {
                return false;
            };
            if state.terminal.is_none() {
                return false;
            }
            state.mode = AttachmentMode::Completion { published: true };
            state.reader_waker.take()
        };
        self.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        true
    }

    fn publish_no_body_terminal(&self) -> bool {
        self.memory.abort_tracking();
        let reader = {
            let mut state = self.state.lock().unwrap();
            if state.owner_fenced || state.terminal.is_none() {
                return false;
            }
            match state.mode {
                AttachmentMode::Pending => {
                    state.chunks.clear();
                    state.buffered_bytes = 0;
                    state.mode = AttachmentMode::TerminalOnly;
                }
                AttachmentMode::Completion { published: false } => {
                    state.mode = AttachmentMode::Completion { published: true };
                }
                AttachmentMode::Live
                | AttachmentMode::Completion { published: true }
                | AttachmentMode::TerminalOnly
                | AttachmentMode::Discard => {}
            }
            state.reader_waker.take()
        };
        self.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        true
    }

    fn select_terminal(&self, cause: ByteStreamCloseCause) -> Result<(), StreamWriteError> {
        self.select_terminal_with_origin(cause, false)
    }

    fn select_terminal_with_origin(
        &self,
        cause: ByteStreamCloseCause,
        host_resource_exhausted: bool,
    ) -> Result<(), StreamWriteError> {
        let terminal = terminal_metadata(&cause);
        let reader = {
            let mut state = self.state.lock().unwrap();
            if let Some(selected) = &state.terminal {
                return if close_cause_eq(selected, &cause) {
                    Ok(())
                } else {
                    Err(StreamWriteError::Closed(selected.clone()))
                };
            }
            if matches!(cause, ByteStreamCloseCause::ConsumerCancelled) {
                state.chunks.clear();
                state.buffered_bytes = 0;
            }
            state.terminal = Some(cause);
            state.host_resource_exhausted = host_resource_exhausted;
            state.reader_waker.take()
        };
        tracing::debug!(
            attachment = ?(self as *const Self),
            ?terminal,
            host_resource_exhausted,
            "Selected tool attachment terminal"
        );
        self.changed.notify_waiters();
        self.terminal_changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        Ok(())
    }

    fn fence_owner(&self) {
        self.memory.abort_tracking();
        let (reader, selected_terminal) = {
            let mut state = self.state.lock().unwrap();
            state.owner_fenced = true;
            state.chunks.clear();
            state.buffered_bytes = 0;
            let selected_terminal = state.terminal.is_none();
            if selected_terminal {
                state.terminal = Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled));
            }
            (state.reader_waker.take(), selected_terminal)
        };
        self.changed.notify_waiters();
        if selected_terminal {
            self.terminal_changed.notify_waiters();
        }
        tracing::debug!(
            attachment = ?(self as *const Self),
            selected_terminal,
            "Fenced tool attachment with owner generation"
        );
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn terminate_unconfigured(
        &self,
        cause: ByteStreamCloseCause,
    ) -> Result<bool, StreamWriteError> {
        let (reader, selected_terminal, result) = {
            let mut state = self.state.lock().unwrap();
            if state.mode != AttachmentMode::Pending || !state.chunks.is_empty() {
                return Ok(false);
            }
            let (selected_terminal, result) = if let Some(selected) = &state.terminal {
                (
                    false,
                    if close_cause_eq(selected, &cause) {
                        Ok(())
                    } else {
                        Err(StreamWriteError::Closed(selected.clone()))
                    },
                )
            } else {
                state.terminal = Some(cause);
                (true, Ok(()))
            };
            state.mode = AttachmentMode::TerminalOnly;
            (state.reader_waker.take(), selected_terminal, result)
        };
        self.changed.notify_waiters();
        if selected_terminal {
            self.terminal_changed.notify_waiters();
        }
        if let Some(reader) = reader {
            reader.wake();
        }
        result.map(|()| true)
    }

    fn select_resource_exhausted(&self) -> StreamWriteError {
        let cause = ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted);
        match self.select_terminal_with_origin(cause.clone(), true) {
            Ok(()) => StreamWriteError::Closed(cause),
            Err(error) => error,
        }
    }

    async fn write(self: &Arc<Self>, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        let _operation = self.begin_producer_operation()?;
        if bytes.is_empty() {
            return Ok(());
        }

        let len = bytes.len();
        if len > self.limit {
            {
                let state = self.state.lock().unwrap();
                if let Some(cause) = &state.terminal {
                    return Err(StreamWriteError::Closed(cause.clone()));
                }
                if state.mode == AttachmentMode::Discard {
                    return Ok(());
                }
            }
            return Err(self.select_resource_exhausted());
        }
        let bytes = bytes.into_boxed_slice().into_vec();
        let allocation_bytes = bytes.capacity();
        let charge = loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let plan = {
                let state = self.state.lock().unwrap();
                if let Some(cause) = &state.terminal {
                    return Err(StreamWriteError::Closed(cause.clone()));
                }
                if self.memory.tracking_pending() {
                    WritePlan::Wait
                } else {
                    match state.mode {
                        AttachmentMode::Discard => WritePlan::Discard,
                        AttachmentMode::Pending | AttachmentMode::Live => WritePlan::Accept,
                        AttachmentMode::Completion { .. } => {
                            if len > self.limit.saturating_sub(state.accepted_completion_bytes) {
                                WritePlan::ResourceExhausted
                            } else {
                                WritePlan::Accept
                            }
                        }
                        AttachmentMode::TerminalOnly => {
                            unreachable!("terminal-only attachments always have a terminal")
                        }
                    }
                }
            };

            match plan {
                WritePlan::Discard => return Ok(()),
                WritePlan::ResourceExhausted => {
                    return Err(self.select_resource_exhausted());
                }
                WritePlan::Accept => {}
                WritePlan::Wait => {
                    notified.as_mut().await;
                    continue;
                }
            }

            let reservation = self.memory.reserve(allocation_bytes);
            tokio::pin!(reservation);
            let charge = tokio::select! {
                charge = &mut reservation => charge,
                () = &mut notified => continue,
            };
            let reservation = match charge {
                AttachmentReservationOutcome::Reserved(reservation) => reservation,
                AttachmentReservationOutcome::Pending => continue,
                AttachmentReservationOutcome::Rejected => {
                    return Err(self.select_resource_exhausted());
                }
            };
            break AttachmentCharge::new(reservation, self.charged_bytes.clone());
        };

        let mut bytes = Some(bytes);
        let mut charge = Some(charge);
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (plan, reader) = {
                let mut state = self.state.lock().unwrap();
                if let Some(cause) = &state.terminal {
                    return Err(StreamWriteError::Closed(cause.clone()));
                }
                let plan = if self.memory.tracking_pending() {
                    WritePlan::Wait
                } else {
                    match state.mode {
                        AttachmentMode::Discard => return Ok(()),
                        AttachmentMode::Pending => {
                            if len <= self.limit.saturating_sub(state.buffered_bytes) {
                                WritePlan::Accept
                            } else {
                                WritePlan::Wait
                            }
                        }
                        AttachmentMode::Live => {
                            if len <= self.limit.saturating_sub(state.buffered_bytes) {
                                WritePlan::Accept
                            } else {
                                WritePlan::Wait
                            }
                        }
                        AttachmentMode::Completion { .. } => {
                            if len <= self.limit.saturating_sub(state.accepted_completion_bytes) {
                                WritePlan::Accept
                            } else {
                                WritePlan::ResourceExhausted
                            }
                        }
                        AttachmentMode::TerminalOnly => {
                            unreachable!("terminal-only attachments always have a terminal")
                        }
                    }
                };
                let reader = if matches!(plan, WritePlan::Accept) {
                    state.chunks.push_back(BufferedChunk {
                        bytes: bytes.take().expect("write bytes already accepted"),
                        charge: charge.take().expect("write charge already accepted"),
                    });
                    state.buffered_bytes += len;
                    state.accepted_bytes = state.accepted_bytes.saturating_add(len as u64);
                    if matches!(
                        state.mode,
                        AttachmentMode::Pending | AttachmentMode::Completion { .. }
                    ) {
                        state.accepted_completion_bytes += len;
                    }
                    state.reader_waker.take()
                } else {
                    None
                };
                tracing::debug!(
                    attachment = ?(Arc::as_ptr(self)),
                    ?state.mode,
                    ?plan,
                    chunk_len = len,
                    buffered_bytes = state.buffered_bytes,
                    capacity_bytes = self.limit,
                    has_reader_waker = reader.is_some(),
                    "planned tool attachment write"
                );
                (plan, reader)
            };
            if let Some(reader) = reader {
                reader.wake();
            }
            match plan {
                WritePlan::Accept | WritePlan::Discard => return Ok(()),
                WritePlan::Wait => notified.as_mut().await,
                WritePlan::ResourceExhausted => {
                    return Err(self.select_resource_exhausted());
                }
            }
        }
    }

    async fn wait_terminal(&self) -> ByteStreamCloseCause {
        loop {
            let notified = self.terminal_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(cause) = self.state.lock().unwrap().terminal.clone() {
                return cause;
            }
            notified.as_mut().await;
        }
    }

    fn consumer_cancel(&self) {
        let (reader, selected_terminal) = {
            let mut state = self.state.lock().unwrap();
            state.consumer_active = false;
            state.chunks.clear();
            state.buffered_bytes = 0;
            let selected_terminal = if state.terminal.is_none() {
                state.terminal = Some(ByteStreamCloseCause::ConsumerCancelled);
                true
            } else {
                false
            };
            (state.reader_waker.take(), selected_terminal)
        };
        self.changed.notify_waiters();
        if selected_terminal {
            self.terminal_changed.notify_waiters();
        }
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn producer_abandon(&self) {
        let _ = self.select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned));
        self.state.lock().unwrap().producer_active = false;
        self.changed.notify_waiters();
    }

    fn sanitize_failure(failure: ByteStreamFailure) -> ByteStreamFailure {
        match failure {
            ByteStreamFailure::Failed(_) => {
                ByteStreamFailure::Failed("stream producer failed".to_string())
            }
            other => other,
        }
    }

    #[cfg(test)]
    fn activity(&self) -> AttachmentActivity {
        let state = self.state.lock().unwrap();
        AttachmentActivity {
            producer_active: state.producer_active,
            consumer_active: state.consumer_active,
            buffered_bytes: state.buffered_bytes,
            charged_bytes: self.charged_bytes.load(Ordering::Acquire),
        }
    }

    fn metadata(&self) -> ToolAttachmentMetadata {
        let state = self.state.lock().unwrap();
        let terminal = state.terminal.as_ref().map(terminal_metadata);
        ToolAttachmentMetadata {
            mode: match state.mode {
                AttachmentMode::Pending => ToolAttachmentModeMetadata::Pending,
                AttachmentMode::Live => ToolAttachmentModeMetadata::Live,
                AttachmentMode::Completion { published: false } => {
                    ToolAttachmentModeMetadata::CompletionStaged
                }
                AttachmentMode::Completion { published: true } => {
                    ToolAttachmentModeMetadata::CompletionPublished
                }
                AttachmentMode::TerminalOnly => ToolAttachmentModeMetadata::TerminalOnly,
                AttachmentMode::Discard => ToolAttachmentModeMetadata::Discard,
            },
            capacity_bytes: self.limit,
            accepted_bytes: state.accepted_bytes,
            delivered_bytes: state.delivered_bytes,
            buffered_bytes: state.buffered_bytes,
            charged_bytes: self.charged_bytes.load(Ordering::Acquire),
            backpressured: matches!(state.mode, AttachmentMode::Live)
                && state.buffered_bytes >= self.limit
                && state.terminal.is_none()
                && state.producer_active
                && state.consumer_active,
            terminal_selected: state.terminal.is_some(),
            terminal,
            owner_fenced: state.owner_fenced,
            host_resource_exhausted: state.host_resource_exhausted,
            producer_operation_active: self.producer_operation.load(Ordering::Acquire),
            producer_active: state.producer_active,
            consumer_active: state.consumer_active,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum WritePlan {
    Wait,
    Discard,
    ResourceExhausted,
    Accept,
}

struct ProducerOperation(Arc<ByteAttachment>);

impl Drop for ProducerOperation {
    fn drop(&mut self) {
        self.0.producer_operation.store(false, Ordering::Release);
    }
}

fn failure_eq(left: &ByteStreamFailure, right: &ByteStreamFailure) -> bool {
    match (left, right) {
        (ByteStreamFailure::Cancelled, ByteStreamFailure::Cancelled)
        | (ByteStreamFailure::Abandoned, ByteStreamFailure::Abandoned)
        | (ByteStreamFailure::ResourceExhausted, ByteStreamFailure::ResourceExhausted) => true,
        (ByteStreamFailure::Failed(left), ByteStreamFailure::Failed(right)) => left == right,
        _ => false,
    }
}

fn close_cause_eq(left: &ByteStreamCloseCause, right: &ByteStreamCloseCause) -> bool {
    match (left, right) {
        (ByteStreamCloseCause::Finished, ByteStreamCloseCause::Finished)
        | (ByteStreamCloseCause::ConsumerCancelled, ByteStreamCloseCause::ConsumerCancelled) => {
            true
        }
        (ByteStreamCloseCause::Failed(left), ByteStreamCloseCause::Failed(right)) => {
            failure_eq(left, right)
        }
        _ => false,
    }
}

pub(crate) struct AttachmentProducer {
    attachment: Arc<ByteAttachment>,
}

impl AttachmentProducer {
    pub(crate) fn controller(&self) -> AttachmentController {
        AttachmentController {
            attachment: self.attachment.clone(),
        }
    }

    pub(crate) fn writer(&self) -> AttachmentWriter {
        AttachmentWriter {
            attachment: self.attachment.clone(),
        }
    }

    pub(crate) async fn write(&self, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        self.attachment.write(bytes).await
    }

    pub(crate) fn finish(&self) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Finished)
    }

    pub(crate) fn fail(&self, failure: ByteStreamFailure) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(
                ByteAttachment::sanitize_failure(failure),
            ))
    }

    #[cfg(test)]
    pub(crate) fn cancel(&self) -> Result<(), StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled))
    }

    pub(crate) fn abandon_unconfigured(&self) -> Result<bool, StreamWriteError> {
        self.attachment
            .terminate_unconfigured(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    }

    pub(crate) fn reject_unconfigured(&self) -> Result<bool, StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Failed(
                "tool invocation rejected".to_string(),
            )))?;
        Ok(self.attachment.publish_no_body_terminal())
    }

    #[cfg(test)]
    pub(crate) fn configure_live(&self) -> bool {
        self.attachment.configure(AttachmentMode::Live)
    }

    #[cfg(test)]
    pub(crate) fn configure_completion(&self) -> bool {
        self.attachment
            .configure(AttachmentMode::Completion { published: false })
    }

    #[cfg(test)]
    pub(crate) fn publish_completion(&self) -> bool {
        self.attachment.publish_completion()
    }

    #[cfg(test)]
    pub(crate) fn observer(&self) -> AttachmentObserver {
        AttachmentObserver {
            attachment: self.attachment.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AttachmentWriter {
    attachment: Arc<ByteAttachment>,
}

impl AttachmentWriter {
    pub(crate) async fn write(&self, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        self.attachment.write(bytes).await
    }

    pub(crate) fn finish(&self) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Finished)
    }

    pub(crate) fn fail(&self, failure: ByteStreamFailure) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(
                ByteAttachment::sanitize_failure(failure),
            ))
    }
}

impl Drop for AttachmentProducer {
    fn drop(&mut self) {
        self.attachment.producer_abandon();
    }
}

pub(crate) struct AttachmentConsumer {
    attachment: Arc<ByteAttachment>,
}

impl AttachmentConsumer {
    pub(crate) fn controller(&self) -> AttachmentController {
        AttachmentController {
            attachment: self.attachment.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn configure_live(&self) -> bool {
        self.attachment.configure(AttachmentMode::Live)
    }

    #[cfg(test)]
    pub(crate) fn configure_completion(&self) -> bool {
        self.attachment
            .configure(AttachmentMode::Completion { published: false })
    }

    pub(crate) fn into_stream_producer(self) -> AttachmentStreamProducer {
        AttachmentStreamProducer {
            consumer: Some(self),
            in_flight_charge: None,
            finished: false,
        }
    }

    pub(crate) fn into_raw_stream_producer(self) -> RawAttachmentStreamProducer {
        RawAttachmentStreamProducer {
            consumer: Some(self),
            in_flight_charge: None,
            finished: false,
        }
    }

    #[cfg(test)]
    async fn read_next(&self) -> AttachmentRead {
        loop {
            let notified = self.attachment.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let read = {
                let mut state = self.attachment.state.lock().unwrap();
                let chunks_visible = matches!(state.mode, AttachmentMode::Live)
                    || matches!(state.mode, AttachmentMode::Completion { published: true });
                let terminal_visible =
                    chunks_visible || matches!(state.mode, AttachmentMode::TerminalOnly);
                if chunks_visible && !state.chunks.is_empty() {
                    let chunk = state.chunks.pop_front().unwrap();
                    state.buffered_bytes -= chunk.bytes.len();
                    state.delivered_bytes = state
                        .delivered_bytes
                        .saturating_add(chunk.bytes.len() as u64);
                    Some(AttachmentRead::Item(Ok(chunk.bytes)))
                } else if !terminal_visible {
                    None
                } else {
                    match state.terminal.clone() {
                        Some(ByteStreamCloseCause::Finished) => Some(AttachmentRead::End),
                        Some(ByteStreamCloseCause::Failed(failure))
                            if !state.terminal_item_delivered =>
                        {
                            state.terminal_item_delivered = true;
                            Some(AttachmentRead::Item(Err(failure)))
                        }
                        Some(ByteStreamCloseCause::Failed(_))
                        | Some(ByteStreamCloseCause::ConsumerCancelled) => {
                            Some(AttachmentRead::End)
                        }
                        None => None,
                    }
                }
            };
            if let Some(read) = read {
                self.attachment.changed.notify_waiters();
                return read;
            }
            notified.as_mut().await;
        }
    }
}

impl Drop for AttachmentConsumer {
    fn drop(&mut self) {
        self.attachment.consumer_cancel();
    }
}

#[derive(Clone)]
pub(crate) struct AttachmentObserver {
    attachment: Arc<ByteAttachment>,
}

impl AttachmentObserver {
    pub(crate) async fn wait_terminal(&self) -> ByteStreamCloseCause {
        self.attachment.wait_terminal().await
    }

    #[cfg(test)]
    pub(crate) fn terminal(&self) -> Option<ByteStreamCloseCause> {
        self.attachment.state.lock().unwrap().terminal.clone()
    }

    pub(crate) fn terminal_snapshot(&self) -> Option<AttachmentTerminalSnapshot> {
        let state = self.attachment.state.lock().unwrap();
        state
            .terminal
            .clone()
            .map(|_cause| AttachmentTerminalSnapshot {
                #[cfg(test)]
                cause: _cause,
                host_resource_exhausted: state.host_resource_exhausted,
            })
    }
}

/// Operation-owned control plane for an attachment. It carries no producer or consumer role, so
/// retaining it for owner fencing and delayed completion publication cannot keep either endpoint
/// artificially open.
#[derive(Clone)]
pub(crate) struct AttachmentController {
    attachment: Arc<ByteAttachment>,
}

impl AttachmentController {
    pub(crate) async fn prepare_live_memory_accounting(&self) -> bool {
        self.attachment.prepare_live_memory_accounting().await
    }

    pub(crate) fn commit_live_memory_accounting(&self) {
        self.attachment.commit_live_memory_accounting();
    }

    pub(crate) fn complete_rejected_live_memory_accounting(&self) {
        self.attachment.complete_rejected_live_memory_accounting();
    }

    pub(crate) fn abort_prepared_live_memory_accounting(&self) {
        self.attachment.memory.abort_tracking();
        self.attachment.changed.notify_waiters();
    }

    pub(crate) fn configure_live(&self) -> bool {
        self.attachment.configure(AttachmentMode::Live)
    }

    pub(crate) fn configure_completion(&self) -> bool {
        self.attachment
            .configure(AttachmentMode::Completion { published: false })
    }

    pub(crate) fn publish_completion(&self) -> bool {
        self.attachment.publish_completion()
    }

    pub(crate) fn publish_no_body_terminal(&self) -> bool {
        self.attachment.publish_no_body_terminal()
    }

    pub(crate) fn cancel(&self) -> Result<(), StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled))
    }

    pub(crate) fn fence_owner(&self) {
        self.attachment.fence_owner();
    }

    pub(crate) fn host_fail(&self, failure: ByteStreamFailure) -> Result<(), StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(
                ByteAttachment::sanitize_failure(failure),
            ))
    }

    pub(crate) fn observer(&self) -> AttachmentObserver {
        AttachmentObserver {
            attachment: self.attachment.clone(),
        }
    }

    pub(crate) fn metadata(&self) -> ToolAttachmentMetadata {
        self.attachment.metadata()
    }

    #[cfg(test)]
    pub(crate) fn live_memory_accounting_state(&self) -> (bool, bool) {
        (
            self.attachment.memory.tracking_enabled(),
            self.attachment.memory.tracking_pending(),
        )
    }
}

pub(crate) struct AttachmentStreamProducer {
    consumer: Option<AttachmentConsumer>,
    in_flight_charge: Option<AttachmentCharge>,
    finished: bool,
}

impl<D> StreamProducer<D> for AttachmentStreamProducer {
    type Item = Result<Vec<u8>, ByteStreamFailure>;
    type Buffer = VecBuffer<Self::Item>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        drop(self.in_flight_charge.take());
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let attachment = self
            .consumer
            .as_ref()
            .expect("attachment stream producer lost its consumer")
            .attachment
            .clone();
        let remaining = dst.remaining(store.as_context_mut());
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if remaining == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        enum Produced {
            Item(Result<Vec<u8>, ByteStreamFailure>, Option<AttachmentCharge>),
            End,
            Cancelled,
            OwnerFenced,
            Pending,
        }

        let produced = {
            let mut state = attachment.state.lock().unwrap();
            let chunks_visible = matches!(state.mode, AttachmentMode::Live)
                || matches!(state.mode, AttachmentMode::Completion { published: true });
            let terminal_visible =
                chunks_visible || matches!(state.mode, AttachmentMode::TerminalOnly);
            if state.owner_fenced {
                Produced::OwnerFenced
            } else if chunks_visible && !state.chunks.is_empty() {
                let chunk = state.chunks.pop_front().unwrap();
                state.buffered_bytes -= chunk.bytes.len();
                state.delivered_bytes = state
                    .delivered_bytes
                    .saturating_add(chunk.bytes.len() as u64);
                Produced::Item(Ok(chunk.bytes), Some(chunk.charge))
            } else if !terminal_visible {
                state.reader_waker = Some(cx.waker().clone());
                Produced::Pending
            } else {
                match state.terminal.clone() {
                    Some(ByteStreamCloseCause::Finished) => Produced::End,
                    Some(ByteStreamCloseCause::Failed(failure))
                        if !state.terminal_item_delivered =>
                    {
                        state.terminal_item_delivered = true;
                        Produced::Item(Err(failure), None)
                    }
                    Some(ByteStreamCloseCause::Failed(_)) => Produced::End,
                    Some(ByteStreamCloseCause::ConsumerCancelled) => Produced::Cancelled,
                    None => {
                        state.reader_waker = Some(cx.waker().clone());
                        Produced::Pending
                    }
                }
            }
        };

        match produced {
            Produced::Item(item, charge) => {
                self.in_flight_charge = charge;
                dst.set_buffer(VecBuffer::from(vec![item]));
                attachment.changed.notify_waiters();
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Produced::End => {
                self.finished = true;
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Produced::Cancelled => {
                self.finished = true;
                Poll::Ready(Ok(StreamResult::Cancelled))
            }
            Produced::OwnerFenced => {
                self.finished = true;
                Poll::Ready(Err(wasmtime::Error::msg(
                    "owner generation fenced tool attachment",
                )))
            }
            Produced::Pending => Poll::Pending,
        }
    }
}

pub(crate) struct RawAttachmentStreamProducer {
    consumer: Option<AttachmentConsumer>,
    in_flight_charge: Option<AttachmentCharge>,
    finished: bool,
}

impl<D> StreamProducer<D> for RawAttachmentStreamProducer {
    type Item = u8;
    type Buffer = Bytes;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        drop(self.in_flight_charge.take());
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let attachment = self
            .consumer
            .as_ref()
            .expect("raw attachment stream producer lost its consumer")
            .attachment
            .clone();
        let remaining = dst.remaining(store.as_context_mut());
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if remaining == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        enum Produced {
            Item(Vec<u8>, AttachmentCharge),
            End,
            Cancelled,
            Failed(ByteStreamFailure),
            OwnerFenced,
            Pending,
        }

        let produced = {
            let mut state = attachment.state.lock().unwrap();
            let chunks_visible = matches!(state.mode, AttachmentMode::Live)
                || matches!(state.mode, AttachmentMode::Completion { published: true });
            let terminal_visible =
                chunks_visible || matches!(state.mode, AttachmentMode::TerminalOnly);
            if state.owner_fenced {
                Produced::OwnerFenced
            } else if chunks_visible && !state.chunks.is_empty() {
                let chunk = state.chunks.pop_front().unwrap();
                state.buffered_bytes -= chunk.bytes.len();
                state.delivered_bytes = state
                    .delivered_bytes
                    .saturating_add(chunk.bytes.len() as u64);
                Produced::Item(chunk.bytes, chunk.charge)
            } else if !terminal_visible {
                state.reader_waker = Some(cx.waker().clone());
                Produced::Pending
            } else {
                match state.terminal.clone() {
                    Some(ByteStreamCloseCause::Finished) => Produced::End,
                    Some(ByteStreamCloseCause::Failed(failure)) => Produced::Failed(failure),
                    Some(ByteStreamCloseCause::ConsumerCancelled) => Produced::Cancelled,
                    None => {
                        state.reader_waker = Some(cx.waker().clone());
                        Produced::Pending
                    }
                }
            }
        };

        match produced {
            Produced::Item(bytes, charge) => {
                self.in_flight_charge = Some(charge);
                dst.set_buffer(Bytes::from(bytes));
                attachment.changed.notify_waiters();
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Produced::End => {
                self.finished = true;
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Produced::Cancelled => {
                self.finished = true;
                Poll::Ready(Ok(StreamResult::Cancelled))
            }
            Produced::Failed(failure) => {
                self.finished = true;
                Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "underlying tool stdout failed: {failure:?}"
                ))))
            }
            Produced::OwnerFenced => {
                self.finished = true;
                Poll::Ready(Err(wasmtime::Error::msg(
                    "owner generation fenced tool attachment",
                )))
            }
            Produced::Pending => Poll::Pending,
        }
    }
}

pub(crate) fn attachment_pair(
    limit: usize,
    memory: AttachmentMemory,
) -> (AttachmentProducer, AttachmentConsumer, AttachmentObserver) {
    let attachment = ByteAttachment::new(limit, memory);
    (
        AttachmentProducer {
            attachment: attachment.clone(),
        },
        AttachmentConsumer {
            attachment: attachment.clone(),
        },
        AttachmentObserver { attachment },
    )
}

pub(crate) fn discard_producer(memory: AttachmentMemory) -> AttachmentProducer {
    AttachmentProducer {
        attachment: ByteAttachment::discard(memory),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AttachmentTerminalSnapshot {
    #[cfg(test)]
    pub(crate) cause: ByteStreamCloseCause,
    pub(crate) host_resource_exhausted: bool,
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct AttachmentActivity {
    pub(crate) producer_active: bool,
    pub(crate) consumer_active: bool,
    pub(crate) buffered_bytes: usize,
    pub(crate) charged_bytes: u64,
}

#[cfg(test)]
#[derive(Debug)]
enum AttachmentRead {
    Item(Result<Vec<u8>, ByteStreamFailure>),
    End,
}

#[cfg(test)]
mod tests {
    use super::super::{
        ToolStdinEntry, ToolStdinWriterEntry, ToolStdoutEntry, ToolStdoutWriterEntry,
    };
    use super::*;
    use test_r::test;
    use tokio::sync::mpsc;
    use wasmtime::component::{Component, Linker, Source, StreamConsumer, StreamReader};
    use wasmtime::{Config, Engine, Store, StoreContextMut};

    fn pair(limit: usize) -> (AttachmentProducer, AttachmentConsumer, AttachmentObserver) {
        attachment_pair(limit, AttachmentMemory::inert())
    }

    fn rejecting_memory() -> AttachmentMemory {
        AttachmentMemory {
            reserve_tracked: Arc::new(|_| Box::pin(async { None })),
            tracking_enabled: Arc::new(AtomicBool::new(true)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn blocked_memory() -> AttachmentMemory {
        AttachmentMemory {
            reserve_tracked: Arc::new(|_| Box::pin(std::future::pending())),
            tracking_enabled: Arc::new(AtomicBool::new(true)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn wait_for_producer_operation(producer: &AttachmentProducer) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !producer
                .attachment
                .producer_operation
                .load(Ordering::Acquire)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_for_charged_bytes(producer: &AttachmentProducer, expected: u64) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while producer.attachment.activity().charged_bytes != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    struct CollectConsumer {
        items: mpsc::UnboundedSender<Result<Vec<u8>, ByteStreamFailure>>,
    }

    impl<D> StreamConsumer<D> for CollectConsumer {
        type Item = Result<Vec<u8>, ByteStreamFailure>;

        fn poll_consume(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            mut store: StoreContextMut<'_, D>,
            mut source: Source<'_, Self::Item>,
            finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            let count = source.remaining(store.as_context_mut());
            let mut received = Vec::with_capacity(count);
            source.read(store.as_context_mut(), &mut received)?;
            for item in received {
                let _ = self.items.send(item);
            }
            Poll::Ready(Ok(if finish {
                StreamResult::Dropped
            } else {
                StreamResult::Completed
            }))
        }
    }

    struct CollectRawConsumer {
        chunks: mpsc::UnboundedSender<Vec<u8>>,
    }

    impl<D> StreamConsumer<D> for CollectRawConsumer {
        type Item = u8;

        fn poll_consume(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            mut store: StoreContextMut<'_, D>,
            source: Source<'_, Self::Item>,
            finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            let mut source = source.as_direct(store.as_context_mut());
            let remaining = source.remaining();
            let count = remaining.len();
            if !remaining.is_empty() {
                let _ = self.chunks.send(remaining.to_vec());
            }
            source.mark_read(count);
            Poll::Ready(Ok(if finish {
                StreamResult::Dropped
            } else {
                StreamResult::Completed
            }))
        }
    }

    struct UpgradeInFlightRawConsumer {
        controller: AttachmentController,
        upgraded: bool,
        completed: mpsc::UnboundedSender<()>,
    }

    impl<D> StreamConsumer<D> for UpgradeInFlightRawConsumer {
        type Item = u8;

        fn poll_consume(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            mut store: StoreContextMut<'_, D>,
            source: Source<'_, Self::Item>,
            finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            let mut source = source.as_direct(store.as_context_mut());
            let count = source.remaining().len();
            if count != 0 && !self.upgraded {
                assert!(futures::executor::block_on(
                    self.controller.prepare_live_memory_accounting()
                ));
                self.controller.commit_live_memory_accounting();
                self.upgraded = true;
            }
            source.mark_read(count);
            if finish {
                let _ = self.completed.send(());
            }
            Poll::Ready(Ok(if finish {
                StreamResult::Dropped
            } else {
                StreamResult::Completed
            }))
        }
    }

    async fn drain_with_store(
        store: &mut Store<()>,
        consumer: AttachmentConsumer,
    ) -> Vec<Result<Vec<u8>, ByteStreamFailure>> {
        let (items, mut received) = mpsc::unbounded_channel();
        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<_>> {
                accessor.with(|mut store| {
                    StreamReader::new(&mut store, consumer.into_stream_producer())?
                        .pipe(&mut store, CollectConsumer { items })
                })?;
                let mut result = Vec::new();
                while let Some(item) = received.recv().await {
                    result.push(item);
                }
                Ok(result)
            })
            .await
            .unwrap()
            .unwrap()
    }

    #[test]
    async fn live_stream_preserves_order_and_terminal() {
        let (producer, consumer, observer) = pair(8);
        assert!(producer.configure_live());
        producer.write(vec![0, 1, 2]).await.unwrap();
        producer.write(vec![3, 4]).await.unwrap();
        producer.finish().unwrap();
        producer.finish().unwrap();
        assert!(matches!(
            producer.fail(ByteStreamFailure::Cancelled),
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Finished))
        ));

        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![0, 1, 2]
        ));
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![3, 4]
        ));
        assert!(matches!(consumer.read_next().await, AttachmentRead::End));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Finished
        ));
    }

    #[test]
    async fn blocked_live_write_wakes_after_consumer_progress() {
        let (producer, consumer, _) = pair(4);
        assert!(producer.configure_live());
        producer.write(vec![1, 2, 3, 4]).await.unwrap();

        let writer = producer.writer();
        let blocked = tokio::spawn(async move { writer.write(vec![5, 6]).await });
        wait_for_producer_operation(&producer).await;
        wait_for_charged_bytes(&producer, 6).await;
        assert!(!blocked.is_finished());
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]
        ));
        blocked.await.unwrap().unwrap();
        producer.finish().unwrap();
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![5, 6]
        ));
    }

    #[test]
    async fn completion_attachment_accepts_exact_limit_and_rejects_crossing_chunk_atomically() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_completion());
        producer.write(vec![1, 2, 3, 4]).await.unwrap();
        let error = producer.write(vec![5]).await.unwrap_err();
        assert!(matches!(
            error,
            StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::ResourceExhausted
            ))
        ));
        assert_eq!(producer.attachment.activity().buffered_bytes, 4);
        assert!(producer.publish_completion());
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]
        ));
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
        ));
        let snapshot = observer.terminal_snapshot().unwrap();
        assert!(snapshot.host_resource_exhausted);
        assert!(matches!(
            snapshot.cause,
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
        ));
    }

    #[test]
    async fn completion_reader_remains_silent_until_terminal_publication() {
        let (producer, consumer, _) = pair(16);
        assert!(producer.configure_completion());
        producer.write(vec![1, 2, 3]).await.unwrap();

        let mut read = tokio::spawn(async move { consumer.read_next().await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
                .await
                .is_err()
        );
        producer.finish().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
                .await
                .is_err()
        );
        assert!(producer.publish_completion());
        assert!(matches!(
            read.await.unwrap(),
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
        ));
    }

    #[test]
    async fn no_body_terminal_publication_handles_every_attachment_mode() {
        let (pending, pending_consumer, _) = pair(16);
        let pending_controller = pending.controller();
        pending.write(vec![1, 2, 3]).await.unwrap();
        let mut pending_read = tokio::spawn(async move {
            (
                pending_consumer.read_next().await,
                pending_consumer.read_next().await,
                pending_consumer.read_next().await,
            )
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending_read)
                .await
                .is_err()
        );
        pending_controller.cancel().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending_read)
                .await
                .is_err()
        );
        assert!(pending_controller.publish_no_body_terminal());
        assert_eq!(
            pending_controller.metadata().mode,
            ToolAttachmentModeMetadata::TerminalOnly
        );
        assert_eq!(pending_controller.metadata().buffered_bytes, 0);
        assert_eq!(pending_controller.metadata().charged_bytes, 0);
        assert!(pending_controller.publish_no_body_terminal());
        let (failure, end, repeated_end) = pending_read.await.unwrap();
        assert!(matches!(
            failure,
            AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
        ));
        assert!(matches!(end, AttachmentRead::End));
        assert!(matches!(repeated_end, AttachmentRead::End));

        let (completion, completion_consumer, _) = pair(16);
        let completion_controller = completion.controller();
        assert!(completion_controller.configure_completion());
        completion.write(vec![4, 5]).await.unwrap();
        completion_controller.cancel().unwrap();
        assert!(completion_controller.publish_no_body_terminal());
        assert_eq!(
            completion_controller.metadata().mode,
            ToolAttachmentModeMetadata::CompletionPublished
        );
        assert!(completion_controller.publish_no_body_terminal());
        assert!(matches!(
            completion_consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![4, 5]
        ));
        assert!(matches!(
            completion_consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
        ));
        assert!(matches!(
            completion_consumer.read_next().await,
            AttachmentRead::End
        ));

        let (live, live_consumer, _) = pair(16);
        let live_controller = live.controller();
        assert!(live_controller.configure_live());
        live.write(vec![6, 7]).await.unwrap();
        live_controller.cancel().unwrap();
        assert!(live_controller.publish_no_body_terminal());
        assert_eq!(
            live_controller.metadata().mode,
            ToolAttachmentModeMetadata::Live
        );
        assert!(matches!(
            live_consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![6, 7]
        ));
        assert!(matches!(
            live_consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
        ));
        assert!(matches!(
            live_consumer.read_next().await,
            AttachmentRead::End
        ));

        let discard = discard_producer(AttachmentMemory::inert());
        let discard_controller = discard.controller();
        discard_controller.cancel().unwrap();
        assert!(discard_controller.publish_no_body_terminal());
        assert_eq!(
            discard_controller.metadata().mode,
            ToolAttachmentModeMetadata::Discard
        );

        let (unterminated, _consumer, _) = pair(16);
        assert!(!unterminated.controller().publish_no_body_terminal());
    }

    #[test]
    async fn no_body_terminal_publication_wakes_a_wasmtime_reader() {
        let (producer, consumer, _) = pair(16);
        let controller = producer.controller();
        assert!(controller.configure_completion());
        controller.cancel().unwrap();

        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let drain = drain_with_store(&mut store, consumer);
        tokio::pin!(drain);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut drain)
                .await
                .is_err()
        );

        assert!(controller.publish_no_body_terminal());
        let items = tokio::time::timeout(std::time::Duration::from_secs(1), &mut drain)
            .await
            .unwrap();
        assert!(matches!(
            items.as_slice(),
            [Err(ByteStreamFailure::Cancelled)]
        ));
    }

    #[test]
    async fn unconfigured_stdout_target_drop_publishes_only_abandonment() {
        let (producer, consumer, _) = pair(4);
        let target = ToolStdoutEntry {
            producer: Some(producer),
            completion_only: false,
        };
        let mut read =
            tokio::spawn(async move { (consumer.read_next().await, consumer.read_next().await) });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
                .await
                .is_err()
        );

        drop(target);

        let (failure, end) = read.await.unwrap();
        assert!(matches!(
            failure,
            AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
        ));
        assert!(matches!(end, AttachmentRead::End));
    }

    #[test]
    async fn unconfigured_stdout_rejection_publishes_bounded_failure_context() {
        let (producer, consumer, _) = pair(4);
        let target = ToolStdoutEntry {
            producer: Some(producer),
            completion_only: false,
        };
        target.reject_unconfigured();
        drop(target);

        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason)))
                if reason == "tool invocation rejected"
        ));
        assert!(matches!(consumer.read_next().await, AttachmentRead::End));
    }

    #[test]
    async fn endpoint_drop_causes_are_directional_and_wake_waiters() {
        let (producer, consumer, observer) = pair(4);
        assert!(consumer.configure_live());
        drop(producer);
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)
        ));

        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        drop(consumer);
        assert!(matches!(
            producer.write(vec![1]).await,
            Err(StreamWriteError::Closed(
                ByteStreamCloseCause::ConsumerCancelled
            ))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::ConsumerCancelled
        ));
    }

    #[test]
    async fn endpoint_role_drop_order_freezes_the_first_terminal() {
        let (producer, consumer, observer) = pair(4);
        drop(producer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));
        drop(consumer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));

        let (producer, consumer, observer) = pair(4);
        drop(consumer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));
        drop(producer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));

        let (producer, reader, observer) = pair(4);
        let target = ToolStdoutEntry {
            producer: Some(producer),
            completion_only: false,
        };
        drop(target);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));
        drop(reader);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));

        let (producer, reader, observer) = pair(4);
        let target = ToolStdoutEntry {
            producer: Some(producer),
            completion_only: false,
        };
        drop(reader);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));
        drop(target);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));

        let (producer, reader, observer) = pair(4);
        let writer: ToolStdoutWriterEntry = ToolStdoutEntry {
            producer: Some(producer),
            completion_only: false,
        }
        .into_writer();
        drop(writer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));
        drop(reader);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));

        let (producer, reader, observer) = pair(4);
        let writer: ToolStdoutWriterEntry = ToolStdoutEntry {
            producer: Some(producer),
            completion_only: false,
        }
        .into_writer();
        drop(reader);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));
        drop(writer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));

        let (producer, consumer, observer) = pair(4);
        let writer = ToolStdinWriterEntry { producer };
        let source = ToolStdinEntry { consumer };
        drop(writer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));
        drop(source);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
        ));

        let (producer, consumer, observer) = pair(4);
        let writer = ToolStdinWriterEntry { producer };
        let source = ToolStdinEntry { consumer };
        drop(source);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));
        drop(writer);
        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::ConsumerCancelled)
        ));
    }

    #[test]
    async fn blocked_live_read_wakes_on_source_abandonment() {
        let (producer, consumer, observer) = pair(4);
        assert!(consumer.configure_live());
        let mut read = tokio::spawn(async move { consumer.read_next().await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
                .await
                .is_err()
        );

        drop(producer);

        assert!(matches!(
            read.await.unwrap(),
            AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)
        ));
    }

    #[test]
    async fn discard_output_never_backpressures_and_retains_terminal() {
        let producer = discard_producer(AttachmentMemory::inert());
        let observer = producer.observer();
        producer.write(vec![0; 1024]).await.unwrap();
        producer.finish().unwrap();
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Finished
        ));
    }

    #[test]
    async fn owner_fenced_wasmtime_reader_traps_without_delivering_cancellation() {
        let (producer, consumer, _) = pair(4);
        let controller = consumer.controller();
        producer.write(vec![1, 2, 3, 4]).await.unwrap();
        controller.fence_owner();
        assert!(controller.metadata().owner_fenced);
        assert_eq!(
            controller.metadata().mode,
            ToolAttachmentModeMetadata::Pending
        );
        assert_eq!(controller.metadata().buffered_bytes, 0);
        assert_eq!(controller.metadata().charged_bytes, 0);
        assert!(!controller.publish_no_body_terminal());

        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let (items, mut received) = mpsc::unbounded_channel();
        let result = store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<_>> {
                accessor.with(|mut store| {
                    StreamReader::new(&mut store, consumer.into_stream_producer())?
                        .pipe(&mut store, CollectConsumer { items })
                })?;
                let mut result = Vec::new();
                while let Some(item) = received.recv().await {
                    result.push(item);
                }
                Ok(result)
            })
            .await;
        let error = result.expect_err("owner-fenced stream must trap");
        assert!(error.to_string().contains("owner generation fenced"));
    }

    #[test]
    async fn closing_wasmtime_reader_cancels_the_consumer_and_cleans_transmit_state() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let mut reader = StreamReader::new(&mut store, consumer.into_stream_producer()).unwrap();
        reader.close(store.as_context_mut()).unwrap();

        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::ConsumerCancelled
        ));
        assert!(matches!(
            producer.write(vec![1]).await,
            Err(StreamWriteError::Closed(
                ByteStreamCloseCause::ConsumerCancelled
            ))
        ));
    }

    #[test]
    async fn cancelling_raw_wasmtime_read_operation_preserves_and_resumes_the_attachment() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        let mut config = Config::new();
        config.concurrency_support(true);
        config.wasm_component_model_more_async_builtins(true);
        let engine = Engine::new(&config).unwrap();
        let component = Component::new(
            &engine,
            r#"
(component
  (import "cancelled" (func $cancelled))
  (core func $cancelled (canon lower (func $cancelled)))
  (core module $memory (memory (export "mem") 1))
  (core instance $memory (instantiate $memory))
  (core module $core
    (import "" "mem" (memory 1))
    (import "" "stream.read-async" (func $stream.read-async (param i32 i32 i32) (result i32)))
    (import "" "stream.read-sync" (func $stream.read-sync (param i32 i32 i32) (result i32)))
    (import "" "stream.cancel-read" (func $stream.cancel-read (param i32) (result i32)))
    (import "" "stream.drop-readable" (func $stream.drop-readable (param i32)))
    (import "" "cancelled" (func $cancelled))
    (func (export "run") (param $stream i32)
      (local $result i32)
      (local.set $result (call $stream.read-async (local.get $stream) (i32.const 8) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (local.set $result (call $stream.cancel-read (local.get $stream)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
      (call $cancelled)
      (local.set $result (call $stream.read-sync (local.get $stream) (i32.const 8) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const 48)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 8)) (i32.const 4)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 9)) (i32.const 5)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 10)) (i32.const 6)) (then unreachable))
      (call $stream.drop-readable (local.get $stream))
    )
  )
  (type $stream (stream u8))
  (core func $stream.read-async (canon stream.read $stream async (memory $memory "mem")))
  (core func $stream.read-sync (canon stream.read $stream (memory $memory "mem")))
  (core func $stream.cancel-read (canon stream.cancel-read $stream))
  (core func $stream.drop-readable (canon stream.drop-readable $stream))
  (core instance $core (instantiate $core (with "" (instance
    (export "mem" (memory $memory "mem"))
    (export "stream.read-async" (func $stream.read-async))
    (export "stream.read-sync" (func $stream.read-sync))
    (export "stream.cancel-read" (func $stream.cancel-read))
    (export "stream.drop-readable" (func $stream.drop-readable))
    (export "cancelled" (func $cancelled))
  ))))
  (func (export "run") async (param "stream" (stream u8))
    (canon lift (core func $core "run") (memory $memory "mem")))
)
            "#,
        )
        .unwrap();
        let (cancelled, mut cancellation) = mpsc::unbounded_channel();
        let mut linker = Linker::new(&engine);
        linker
            .root()
            .func_wrap("cancelled", move |_, (): ()| {
                cancelled.send(()).unwrap();
                Ok(())
            })
            .unwrap();
        let mut store = Store::new(&engine, ());
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let run = instance
            .get_typed_func::<(StreamReader<u8>,), ()>(&mut store, "run")
            .unwrap();
        let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
        let observer_during_cancellation = observer.clone();

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                tokio::try_join!(
                    async {
                        run.call_concurrent(accessor, (reader,)).await?;
                        wasmtime::error::Ok(())
                    },
                    async {
                        cancellation.recv().await.unwrap();
                        assert!(observer_during_cancellation.terminal().is_none());
                        producer.write(vec![4, 5, 6]).await.unwrap();
                        producer.finish().unwrap();
                        wasmtime::error::Ok(())
                    }
                )?;
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            observer.terminal(),
            Some(ByteStreamCloseCause::Finished)
        ));
    }

    #[test]
    async fn closing_raw_wasmtime_reader_still_cancels_the_consumer() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let mut reader =
            StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
        reader.close(store.as_context_mut()).unwrap();

        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::ConsumerCancelled
        ));
        assert!(matches!(
            producer.write(vec![1]).await,
            Err(StreamWriteError::Closed(
                ByteStreamCloseCause::ConsumerCancelled
            ))
        ));
    }

    #[test]
    async fn unread_raw_completion_stream_retains_grants_until_wasmtime_consumes_bytes() {
        let (producer, consumer, _) = pair(8);
        assert!(producer.configure_completion());
        producer.write(vec![1, 2, 3, 4]).await.unwrap();
        producer.finish().unwrap();
        assert!(producer.publish_completion());

        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
        assert_eq!(producer.attachment.activity().charged_bytes, 4);

        let (chunks, mut received) = mpsc::unbounded_channel();
        let collected = store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<u8>> {
                accessor
                    .with(|mut store| reader.pipe(&mut store, CollectRawConsumer { chunks }))?;
                let mut collected = Vec::new();
                while let Some(chunk) = received.recv().await {
                    collected.extend(chunk);
                }
                Ok(collected)
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(collected, vec![1, 2, 3, 4]);
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
    }

    #[test]
    async fn wasmtime_in_flight_historical_chunk_is_included_in_live_upgrade() {
        let reservations = Arc::new(Mutex::new(Vec::new()));
        let reservations_for_memory = reservations.clone();
        let memory = AttachmentMemory::with_test_reservation(false, move |bytes| {
            reservations_for_memory.lock().unwrap().push(bytes);
            async move { Some(MemoryGrant::inert(bytes)) }
        });
        let (producer, consumer, _) = attachment_pair(8, memory);
        let controller = producer.controller();
        producer.write(vec![1, 2, 3, 4]).await.unwrap();
        assert!(producer.configure_live());
        producer.finish().unwrap();

        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
        let (completed, mut completion) = mpsc::unbounded_channel();

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                accessor.with(|mut store| {
                    reader.pipe(
                        &mut store,
                        UpgradeInFlightRawConsumer {
                            controller,
                            upgraded: false,
                            completed,
                        },
                    )
                })?;
                completion.recv().await;
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(*reservations.lock().unwrap(), vec![4]);
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
    }

    #[test]
    async fn concurrent_producer_operations_are_rejected() {
        let (producer, consumer, _) = pair(2);
        assert!(producer.configure_live());
        producer.write(vec![1, 2]).await.unwrap();

        let writer = producer.writer();
        let blocked = tokio::spawn(async move { writer.write(vec![3]).await });
        wait_for_producer_operation(&producer).await;
        assert!(!blocked.is_finished());
        assert!(matches!(
            producer.finish(),
            Err(StreamWriteError::ConcurrentOperation)
        ));

        drop(consumer);
        assert!(matches!(
            blocked.await.unwrap(),
            Err(StreamWriteError::Closed(
                ByteStreamCloseCause::ConsumerCancelled
            ))
        ));
    }

    #[test]
    async fn blocked_write_wakes_when_reader_cancels() {
        let (producer, consumer, observer) = pair(2);
        assert!(producer.configure_live());
        producer.write(vec![1, 2]).await.unwrap();

        let writer = producer.writer();
        let blocked = tokio::spawn(async move { writer.write(vec![3]).await });
        wait_for_producer_operation(&producer).await;
        assert!(!blocked.is_finished());
        drop(consumer);

        assert!(matches!(
            blocked.await.unwrap(),
            Err(StreamWriteError::Closed(
                ByteStreamCloseCause::ConsumerCancelled
            ))
        ));
        assert_eq!(
            producer.attachment.activity(),
            AttachmentActivity {
                producer_active: true,
                consumer_active: false,
                buffered_bytes: 0,
                charged_bytes: 0,
            }
        );
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::ConsumerCancelled
        ));
    }

    #[test]
    async fn host_cancellation_interrupts_an_outstanding_write() {
        let (producer, consumer, observer) = pair(2);
        assert!(producer.configure_live());
        producer.write(vec![1, 2]).await.unwrap();

        let writer = producer.writer();
        let blocked = tokio::spawn(async move { writer.write(vec![3]).await });
        wait_for_producer_operation(&producer).await;
        assert!(!blocked.is_finished());
        producer.cancel().unwrap();

        assert!(matches!(
            blocked.await.unwrap(),
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::Cancelled
            )))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
        ));
        drop(consumer);
    }

    #[test]
    async fn host_cancellation_interrupts_a_pending_memory_reservation() {
        let (producer, consumer, observer) = attachment_pair(2, blocked_memory());
        assert!(producer.configure_live());
        let writer = producer.writer();
        let blocked = tokio::spawn(async move { writer.write(vec![1]).await });
        wait_for_producer_operation(&producer).await;
        assert!(!blocked.is_finished());

        producer.cancel().unwrap();

        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), blocked)
                .await
                .unwrap()
                .unwrap(),
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::Cancelled
            )))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
        ));
        drop(consumer);
    }

    #[test]
    async fn rejected_memory_reservation_selects_resource_exhaustion_without_buffering() {
        let (producer, consumer, observer) = attachment_pair(4, rejecting_memory());
        assert!(consumer.configure_completion());

        assert!(matches!(
            producer.write(vec![1]).await,
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::ResourceExhausted
            )))
        ));
        assert_eq!(producer.attachment.activity().buffered_bytes, 0);
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
        ));
        assert!(
            observer
                .terminal_snapshot()
                .unwrap()
                .host_resource_exhausted
        );
    }

    #[test]
    async fn historical_writes_bypass_measured_admission_until_live_upgrade() {
        let reservation_count = Arc::new(AtomicU64::new(0));
        let reservation_count_for_memory = reservation_count.clone();
        let memory = AttachmentMemory {
            reserve_tracked: Arc::new(move |bytes| {
                reservation_count_for_memory.fetch_add(1, Ordering::AcqRel);
                Box::pin(async move { Some(MemoryGrant::inert(bytes)) })
            }),
            tracking_enabled: Arc::new(AtomicBool::new(false)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        };
        let (producer, consumer, _) = attachment_pair(8, memory);
        let controller = producer.controller();

        producer.write(vec![1, 2, 3]).await.unwrap();
        assert_eq!(reservation_count.load(Ordering::Acquire), 0);
        assert!(controller.prepare_live_memory_accounting().await);
        assert_eq!(reservation_count.load(Ordering::Acquire), 1);
        controller.commit_live_memory_accounting();
        assert!(controller.prepare_live_memory_accounting().await);
        assert_eq!(reservation_count.load(Ordering::Acquire), 1);

        assert!(producer.configure_live());
        producer.write(vec![4]).await.unwrap();
        assert_eq!(reservation_count.load(Ordering::Acquire), 2);
        producer.finish().unwrap();
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
        ));
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![4]
        ));
    }

    #[test]
    async fn rejected_historical_upgrade_selects_resource_exhaustion() {
        let reservation_count = Arc::new(AtomicU64::new(0));
        let reservation_count_for_memory = reservation_count.clone();
        let memory = AttachmentMemory {
            reserve_tracked: Arc::new(move |_| {
                reservation_count_for_memory.fetch_add(1, Ordering::AcqRel);
                Box::pin(async { None })
            }),
            tracking_enabled: Arc::new(AtomicBool::new(false)),
            tracking_pending: Arc::new(AtomicBool::new(false)),
            prepared_grant: Arc::new(Mutex::new(None)),
            historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
            live_activation: Arc::new(tokio::sync::Mutex::new(())),
        };
        let (producer, consumer, observer) = attachment_pair(8, memory);
        let controller = producer.controller();

        producer.write(vec![1, 2, 3]).await.unwrap();
        assert_eq!(reservation_count.load(Ordering::Acquire), 0);
        assert!(!controller.prepare_live_memory_accounting().await);
        assert_eq!(reservation_count.load(Ordering::Acquire), 1);
        assert!(observer.terminal_snapshot().is_none());
        controller.complete_rejected_live_memory_accounting();
        assert_eq!(controller.metadata().buffered_bytes, 0);
        assert_eq!(controller.metadata().charged_bytes, 0);
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
        ));
        assert!(
            observer
                .terminal_snapshot()
                .unwrap()
                .host_resource_exhausted
        );
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
        ));
        assert!(matches!(consumer.read_next().await, AttachmentRead::End));
    }

    #[test]
    async fn reader_cancellation_after_terminal_releases_queued_memory_without_rewriting_terminal()
    {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        producer.write(vec![1, 2, 3, 4]).await.unwrap();
        producer.finish().unwrap();
        assert_eq!(producer.attachment.activity().charged_bytes, 4);

        drop(consumer);

        assert_eq!(producer.attachment.activity().buffered_bytes, 0);
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Finished
        ));
    }

    #[test]
    async fn buffered_chunks_normalize_and_charge_their_exact_allocation() {
        let (producer, consumer, _) = pair(4);
        assert!(producer.configure_live());
        let mut bytes = Vec::with_capacity(1024);
        bytes.extend([1, 2, 3, 4]);

        producer.write(bytes).await.unwrap();

        assert_eq!(producer.attachment.activity().buffered_bytes, 4);
        assert_eq!(producer.attachment.activity().charged_bytes, 4);
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]
        ));
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
    }

    #[test]
    async fn oversized_write_in_streaming_mode_exhausts_resource() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        assert!(matches!(
            producer.write(vec![1, 2, 3, 4, 5]).await,
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::ResourceExhausted
            )))
        ));
        assert_eq!(producer.attachment.activity().buffered_bytes, 0);
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
        assert!(
            observer
                .terminal_snapshot()
                .unwrap()
                .host_resource_exhausted
        );
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
        ));
    }

    #[test]
    async fn pending_crossing_write_is_rejected_atomically_after_completion_selection() {
        let (producer, consumer, observer) = pair(4);
        producer.write(vec![1, 2, 3]).await.unwrap();
        let writer = producer.writer();
        let pending = tokio::spawn(async move { writer.write(vec![4, 5]).await });
        wait_for_producer_operation(&producer).await;
        wait_for_charged_bytes(&producer, 5).await;
        assert!(!pending.is_finished());

        assert!(consumer.configure_completion());
        assert!(matches!(
            pending.await.unwrap(),
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::ResourceExhausted
            )))
        ));
        assert_eq!(producer.attachment.activity().buffered_bytes, 3);
        assert_eq!(producer.attachment.activity().charged_bytes, 3);
        assert!(
            observer
                .terminal_snapshot()
                .unwrap()
                .host_resource_exhausted
        );
        assert!(producer.publish_completion());
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
        ));
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
        ));
    }

    #[test]
    async fn caller_declared_resource_exhaustion_is_not_host_enforced() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        producer.fail(ByteStreamFailure::ResourceExhausted).unwrap();

        let snapshot = observer.terminal_snapshot().unwrap();
        assert!(!snapshot.host_resource_exhausted);
        assert!(matches!(
            snapshot.cause,
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
        ));
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
        ));
    }

    #[test]
    async fn explicit_failure_cancellation_and_writer_clone_drop_remain_distinct() {
        let (producer, consumer, observer) = pair(4);
        assert!(producer.configure_live());
        drop(producer.writer());
        assert!(observer.terminal().is_none());
        producer
            .fail(ByteStreamFailure::Failed(
                "request contents must not escape".to_string(),
            ))
            .unwrap();
        producer
            .fail(ByteStreamFailure::Failed(
                "a different request must not escape either".to_string(),
            ))
            .unwrap();
        assert!(
            !observer
                .terminal_snapshot()
                .unwrap()
                .host_resource_exhausted
        );
        assert!(matches!(
            producer.cancel(),
            Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                ByteStreamFailure::Failed(reason)
            ))) if reason == "stream producer failed"
        ));
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason)))
                if reason == "stream producer failed"
        ));
        assert!(matches!(consumer.read_next().await, AttachmentRead::End));

        let (producer, consumer, observer) = pair(4);
        assert!(consumer.configure_live());
        producer.cancel().unwrap();
        producer.cancel().unwrap();
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
        ));
    }

    #[test]
    async fn completion_terminal_precedes_publication_and_charges_follow_buffer_drain() {
        let (producer, consumer, observer) = pair(5);
        assert!(producer.configure_completion());
        producer.write(Vec::new()).await.unwrap();
        producer.write(vec![0, 255]).await.unwrap();
        producer.write(vec![1, 2, 3]).await.unwrap();
        assert_eq!(producer.attachment.activity().buffered_bytes, 5);
        assert_eq!(producer.attachment.activity().charged_bytes, 5);
        producer.finish().unwrap();

        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Finished
        ));
        assert_eq!(producer.attachment.activity().buffered_bytes, 5);
        assert!(producer.publish_completion());
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![0, 255]
        ));
        assert_eq!(producer.attachment.activity().charged_bytes, 3);
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
        ));
        assert_eq!(producer.attachment.activity().buffered_bytes, 0);
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
        assert!(matches!(consumer.read_next().await, AttachmentRead::End));
    }

    #[test]
    async fn separate_wasmtime_stores_drive_live_and_completion_attachments() {
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut sidecar_store = Store::new(&engine, ());
        let mut owner_store = Store::new(&engine, ());

        let input_bytes = (0..=u16::MAX)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        let expected_input = input_bytes.clone();
        let (input, sidecar_input, _) = pair(1024);
        assert!(input.configure_live());
        let input_writer = tokio::spawn(async move {
            for chunk in input_bytes.chunks(1024) {
                input.write(chunk.to_vec()).await.unwrap();
            }
            input.finish().unwrap();
        });
        let received_input = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            drain_with_store(&mut sidecar_store, sidecar_input),
        )
        .await
        .unwrap();
        input_writer.await.unwrap();
        let received_input = received_input
            .into_iter()
            .flat_map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(received_input, expected_input);

        let (output, owner_output, _) = pair(6);
        assert!(output.configure_completion());
        output.write(vec![10, 11]).await.unwrap();
        output.write(vec![12, 13, 14, 15]).await.unwrap();
        output.finish().unwrap();
        let drain = drain_with_store(&mut owner_store, owner_output);
        tokio::pin!(drain);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut drain)
                .await
                .is_err()
        );
        assert!(output.publish_completion());
        let received_output = tokio::time::timeout(std::time::Duration::from_secs(1), &mut drain)
            .await
            .unwrap();
        let received_output = received_output
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(received_output, vec![vec![10, 11], vec![12, 13, 14, 15]]);
    }
}
