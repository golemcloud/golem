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

use super::{
    ActiveAgents, Arc, AtomicBool, AtomicU64, AttachmentCharge, AttachmentMemory,
    AttachmentReservation, AttachmentReservationOutcome, AttachmentTrackingRollback,
    HistoricalAttachmentChargeState, HistoricalAttachmentCharges, HistoricalAttachmentReservation,
    MemoryGrant, Mutex, Ordering, WorkerCtx,
};

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

    pub(super) async fn reserve(&self, bytes: usize) -> AttachmentReservationOutcome {
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

    pub(super) async fn reserve_tracked(&self, bytes: u64) -> Option<MemoryGrant> {
        (self.reserve_tracked)(bytes).await
    }

    pub(super) fn tracking_enabled(&self) -> bool {
        self.tracking_enabled.load(Ordering::Acquire)
    }

    pub(super) fn tracking_pending(&self) -> bool {
        self.tracking_pending.load(Ordering::Acquire)
    }

    pub(super) fn begin_tracking(&self) {
        self.tracking_pending.store(true, Ordering::Release);
        self.historical_charges.close();
    }

    pub(super) fn prepare_grant(&self, grant: MemoryGrant) {
        *self.prepared_grant.lock().unwrap() = Some(grant);
    }

    pub(super) fn commit_tracking(&self) {
        if let Some(grant) = self.prepared_grant.lock().unwrap().take() {
            self.historical_charges.install_upgrade(grant);
        }
        self.tracking_enabled.store(true, Ordering::Release);
        self.tracking_pending.store(false, Ordering::Release);
    }

    pub(super) fn abort_tracking(&self) {
        self.prepared_grant.lock().unwrap().take();
        if !self.tracking_enabled() {
            self.historical_charges.reopen();
        }
        self.tracking_pending.store(false, Ordering::Release);
    }
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
    pub(super) fn reserve(self: &Arc<Self>, bytes: u64) -> Option<HistoricalAttachmentReservation> {
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

    pub(super) fn release(&self, bytes: u64) {
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

    pub(super) fn bytes(&self) -> u64 {
        self.state.lock().unwrap().bytes
    }

    pub(super) fn install_upgrade(&self, grant: MemoryGrant) {
        let mut state = self.state.lock().unwrap();
        if state.bytes != 0 {
            state.upgrade_grant = Some(grant);
        }
    }

    pub(super) fn close(&self) {
        self.state.lock().unwrap().accepting = false;
    }

    pub(super) fn reopen(&self) {
        self.state.lock().unwrap().accepting = true;
    }
}

impl HistoricalAttachmentReservation {
    pub(super) fn into_charges(mut self) -> Arc<HistoricalAttachmentCharges> {
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

impl AttachmentTrackingRollback<'_> {
    pub(super) fn disarm(&mut self) {
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

impl AttachmentCharge {
    pub(in crate::durable_host::tool) fn new(
        reservation: AttachmentReservation,
        charged_bytes: Arc<AtomicU64>,
    ) -> Self {
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
            reconstruction_bytes: None,
        }
    }
}

impl Drop for AttachmentCharge {
    fn drop(&mut self) {
        if let Some(historical_charges) = &self.historical_charges {
            historical_charges.release(self.grant.bytes());
        }
        if let Some(reconstruction_bytes) = &self.reconstruction_bytes {
            reconstruction_bytes.fetch_sub(self.grant.bytes(), Ordering::AcqRel);
        }
        self.charged_bytes
            .fetch_sub(self.grant.bytes(), Ordering::AcqRel);
    }
}
