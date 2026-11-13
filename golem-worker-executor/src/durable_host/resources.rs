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

use super::*;

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Resets the per-invocation HTTP and RPC call counters to zero.
    ///
    /// Delegates to `PrivateDurableWorkerState::reset_invocation_call_counts`.
    pub fn reset_invocation_call_counts(&mut self) {
        self.state.reset_invocation_call_counts();
    }

    /// Records one outgoing HTTP call against the monthly account quota when it is fresh live
    /// work (see `WorkerState::durable_call_is_fresh`).
    ///
    /// Returns `Err(WorkerMonthlyHttpCallBudgetExhausted)` if the monthly budget
    /// is exhausted. This trap maps to `RetryDecision::TryStop`; the worker is
    /// suspended and resumed when the registry replenishes the budget.
    pub fn record_monthly_http_call(&mut self) -> anyhow::Result<()> {
        if self.state.durable_call_is_fresh(QuotaClass::Http)
            && !self.state.resource_limit_entry.record_http_call()
        {
            Err(anyhow!(
                GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted
            ))
        } else {
            Ok(())
        }
    }

    /// Records one outgoing RPC call against the monthly account quota when it is fresh live work
    /// (see `WorkerState::durable_call_is_fresh`).
    ///
    /// Returns `Err(WorkerMonthlyRpcCallBudgetExhausted)` if the monthly budget
    /// is exhausted.
    pub fn record_monthly_rpc_call(&mut self) -> anyhow::Result<()> {
        if self.state.durable_call_is_fresh(QuotaClass::Rpc)
            && !self.state.resource_limit_entry.record_rpc_call()
        {
            Err(anyhow!(
                GolemSpecificWasmTrap::WorkerMonthlyRpcCallBudgetExhausted
            ))
        } else {
            Ok(())
        }
    }

    pub fn increase_memory(&mut self, delta: u64) {
        self.linear_memory
            .grow_and_record(delta, Instant::now(), |delta| {
                if self.runtime == OwnerRuntime::Agent && !self.state.snapshotting_mode {
                    // Enqueue under the tracker's accounting boundary, without awaiting on the
                    // Wasmtime fiber. Growth after the absolute baseline must be recorded even
                    // while reconstruction settlement still prevents live effects.
                    self.public_state.worker().request_memory_grow(delta);
                }
            });
    }

    pub(crate) async fn try_acquire_linear_memory(&self, delta: u64) -> Option<MemoryGrant> {
        self.public_state
            .worker()
            .active_agents()
            .try_acquire(delta)
            .await
    }

    pub async fn admit_unshared_memory_growth(
        &self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        admit_memory_growth(
            &self.linear_memory,
            self.max_linear_memory_size(),
            current,
            desired,
            maximum,
            |delta| self.try_acquire_linear_memory(delta),
        )
        .await
    }

    pub fn unshared_memory_growth_failed(&self) {
        self.linear_memory.memory_grow_failed();
    }
}

pub(crate) async fn admit_memory_growth<F, Fut>(
    tracker: &LinearMemoryTracker,
    worker_limit: u64,
    current: usize,
    desired: usize,
    maximum: Option<usize>,
    acquire: F,
) -> wasmtime::Result<bool>
where
    F: FnOnce(u64) -> Fut,
    Fut: Future<Output = Option<MemoryGrant>>,
{
    let growth = tracker.prepare_unshared_growth(current, desired);
    let Some(growth) = validate_unshared_memory_growth(growth, worker_limit, desired, maximum)
    else {
        tracker.memory_grow_failed();
        return Err(GolemSpecificWasmTrap::WorkerExceededMemoryLimit.into());
    };
    if growth.admission_delta > 0 {
        let Some(grant) = acquire(growth.admission_delta).await else {
            tracker.memory_grow_failed();
            crate::metrics::workers::record_worker_memory_grow_rejected();
            return Err(GolemSpecificWasmTrap::WorkerOutOfMemory.into());
        };
        tracker.retain_growth_grant(grant);
    }
    Ok(true)
}

#[async_trait]
impl<Ctx: WorkerCtx> ResourceStore for DurableWorkerCtx<Ctx> {
    fn self_uri(&self) -> Uri {
        self.state.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        let id = self.state.add(resource, name.clone()).await;
        let resource_id = AgentResourceId(id);
        if self.state.is_live() {
            let entry = OplogEntry::create_resource(
                self.entity_parent_start_index(),
                resource_id,
                name.clone(),
            );
            self.public_state
                .worker()
                .add_to_oplog_or_retire(entry)
                .await;
        }
        id
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        let result = self.state.borrow(resource_id).await;
        if let Some((resource_type_id, _)) = &result {
            let id = AgentResourceId(resource_id);
            if self.state.is_live() {
                let entry = OplogEntry::drop_resource(
                    self.entity_parent_start_index(),
                    id,
                    resource_type_id.clone(),
                );
                self.public_state
                    .worker()
                    .add_to_oplog_or_retire(entry)
                    .await;
            }
        }
        result
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        self.state.borrow(resource_id).await
    }
}

impl PrivateDurableWorkerState {
    /// Increments the HTTP call counter for the current invocation if the call is fresh live
    /// work.
    ///
    /// Returns `Err` if the per-invocation HTTP call limit would be exceeded.
    /// The check and increment are performed only during live execution; replay
    /// mode is a no-op so that recovering workers are not penalised for calls
    /// already made in a prior execution. Call quotas are charged before the
    /// call's `Start` is claimed or written, so they use
    /// [`Self::durable_call_is_fresh`]: a call that may still adopt a retained
    /// recorded `Start` charged to the same quota is not charged again.
    pub fn check_and_increment_http_call_count(&mut self) -> Result<(), GolemSpecificWasmTrap> {
        if !self.durable_call_is_fresh(QuotaClass::Http) {
            return Ok(());
        }
        if self.per_invocation_http_call_limit != u64::MAX
            && self.http_call_count >= self.per_invocation_http_call_limit
        {
            return Err(GolemSpecificWasmTrap::WorkerExceededHttpCallLimit);
        }
        self.http_call_count = self.http_call_count.saturating_add(1);
        Ok(())
    }

    /// Increments the RPC call counter for the current invocation if the call is fresh live work.
    ///
    /// Returns `Err` if the per-invocation RPC call limit would be exceeded. Uses
    /// [`Self::durable_call_is_fresh`] for the same reason as
    /// [`Self::check_and_increment_http_call_count`].
    pub fn check_and_increment_rpc_call_count(&mut self) -> Result<(), GolemSpecificWasmTrap> {
        if !self.durable_call_is_fresh(QuotaClass::Rpc) {
            return Ok(());
        }
        if self.per_invocation_rpc_call_limit != u64::MAX
            && self.rpc_call_count >= self.per_invocation_rpc_call_limit
        {
            return Err(GolemSpecificWasmTrap::WorkerExceededRpcCallLimit);
        }
        self.rpc_call_count = self.rpc_call_count.saturating_add(1);
        Ok(())
    }

    pub fn reset_invocation_call_counts(&mut self) {
        self.http_call_count = 0;
        self.rpc_call_count = 0;
        self.scope_card_mint_ordinal = 0;
        self.tool_invocation_attempt_ordinals.clear();
    }
}

#[async_trait]
impl ResourceStore for PrivateDurableWorkerState {
    fn self_uri(&self) -> Uri {
        Uri {
            value: self.owned_agent_id.agent_id.to_agent_urn(),
        }
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        let id = self.last_resource_id;
        self.last_resource_id = self.last_resource_id.next();
        self.resources.insert(id, (name, resource));
        id.0
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        let resource_id = AgentResourceId(resource_id);
        self.resources.remove(&resource_id)
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        self.resources.get(&AgentResourceId(resource_id)).cloned()
    }
}

pub(in crate::durable_host) fn validate_unshared_memory_growth(
    growth: Option<UnsharedMemoryGrowth>,
    worker_limit: u64,
    desired: usize,
    memory_maximum: Option<usize>,
) -> Option<UnsharedMemoryGrowth> {
    growth.filter(|growth| {
        growth.protected_total <= worker_limit
            && memory_maximum.is_none_or(|maximum| desired <= maximum)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::active_agents::admission::{
        AdmissionController, AdmissionPolicy, NoEvictionSource,
    };
    use crate::services::active_agents::memory_probe::FixedProbe;
    use crate::services::resource_limits::AtomicResourceEntry;
    use golem_common::model::agent::AgentMode;
    use golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache;
    use std::sync::Mutex;
    use test_r::test;
    use wasmtime::{Engine, Instance, MemoryKind, Module, ResourceLimiterAsync, Store};

    struct LimitedMemory {
        tracker: LinearMemoryTracker,
        controller: Arc<AdmissionController>,
        limit: u64,
        gc_growths: usize,
    }

    #[async_trait]
    impl ResourceLimiterAsync for LimitedMemory {
        async fn memory_growing(
            &mut self,
            current: usize,
            desired: usize,
            maximum: Option<usize>,
            _kind: MemoryKind,
        ) -> wasmtime::Result<bool> {
            admit_memory_growth(
                &self.tracker,
                self.limit,
                current,
                desired,
                maximum,
                |bytes| self.controller.admit(bytes, &NoEvictionSource),
            )
            .await
        }

        fn memory_grown(&mut self, current: usize, desired: usize, kind: MemoryKind) {
            if matches!(kind, MemoryKind::GcHeap) {
                self.gc_growths += 1;
            }
            self.tracker
                .grow((desired - current) as u64, Instant::now());
        }

        fn memory_grow_failed(
            &mut self,
            _error: wasmtime::Error,
            _kind: MemoryKind,
        ) -> wasmtime::Result<()> {
            self.tracker.memory_grow_failed();
            Ok(())
        }

        async fn table_growing(
            &mut self,
            _current: usize,
            _desired: usize,
            _maximum: Option<usize>,
        ) -> wasmtime::Result<bool> {
            Ok(true)
        }
    }

    #[test]
    async fn gc_initializers_reuse_prepaid_capacity_across_multiple_growths() -> anyhow::Result<()>
    {
        const CAPACITY: u64 = 256 * 1024 * 1024;
        let engine = Engine::new(&create_wasmtime_config_without_fs_cache())?;
        let module = Module::new(
            &engine,
            r#"
            (module
              (memory (export "memory") 256)
              (type $bytes (array (mut i8)))
              (global $first (mut (ref null $bytes)) (ref.null $bytes))
              (global $second (mut (ref null $bytes)) (ref.null $bytes))
              (func $init
                (global.set $first (array.new_default $bytes (i32.const 8388608)))
                (global.set $second (array.new_default $bytes (i32.const 25165824))))
              (start $init))
        "#,
        )?;
        let mut actual = 0;
        for replaying in [false, true] {
            let controller = Arc::new(AdmissionController::new(
                Box::new(FixedProbe::new(CAPACITY, 0)),
                AdmissionPolicy { usable_ratio: 1.0 },
            ));
            let reservation = controller.admit(actual, &NoEvictionSource).await.unwrap();
            let tracker = LinearMemoryTracker::new(
                actual,
                actual,
                AgentMode::Durable,
                replaying,
                Arc::new(AtomicResourceEntry::new(0, CAPACITY as usize, 0, 0, 0)),
                Arc::new(Mutex::new(reservation)),
                Instant::now(),
            );
            tracker.begin_instantiation(false);
            let mut store = Store::new(
                &engine,
                LimitedMemory {
                    tracker: tracker.clone(),
                    controller: controller.clone(),
                    limit: if replaying { actual } else { CAPACITY },
                    gc_growths: 0,
                },
            );
            store.limiter_async(|state| state);
            store.set_fuel(u64::MAX)?;
            store.set_epoch_deadline(u64::MAX);
            let instance = Instance::new_async(&mut store, &module, &[]).await?;
            let linear = instance
                .get_memory(&mut store, "memory")
                .unwrap()
                .data_size(&store) as u64;
            let measured = linear + store.gc_heap_capacity() as u64;
            assert!(
                store.data().gc_growths >= 2,
                "fixture must grow the GC heap repeatedly during initialization"
            );
            tracker.reconcile(measured, Instant::now());
            tracker.switch_to_live();
            tracker.meter().resume(measured, Instant::now());
            tracker.activate_live_usage();
            assert_eq!(tracker.current_bytes(), measured);
            assert_eq!(tracker.billable_bytes(), measured);
            assert_eq!(controller.headroom_bytes(), CAPACITY - measured);
            if replaying {
                assert_eq!(measured, actual);
            }
            actual = measured;
        }
        Ok(())
    }

    #[test]
    async fn live_publication_preserves_pending_growth_and_releases_failed_capacity() {
        for (desired, succeeds) in [(100, true), (100, false), (60, false)] {
            let controller = Arc::new(AdmissionController::new(
                Box::new(FixedProbe::new(200, 0)),
                AdmissionPolicy { usable_ratio: 1.0 },
            ));
            let reservation = controller.admit(80, &NoEvictionSource).await.unwrap();
            let limits = Arc::new(AtomicResourceEntry::new(0, 200, 0, 0, 0));
            let tracker = LinearMemoryTracker::new(
                80,
                80,
                AgentMode::Durable,
                true,
                limits.clone(),
                Arc::new(Mutex::new(reservation)),
                Instant::now(),
            );
            tracker.begin_instantiation(false);
            tracker.reconcile(40, Instant::now());
            let (release, gate) = tokio::sync::oneshot::channel();
            let admission_controller = controller.clone();
            let admission =
                admit_memory_growth(&tracker, 200, 40, desired, None, |bytes| async move {
                    gate.await.unwrap();
                    admission_controller.admit(bytes, &NoEvictionSource).await
                });
            tokio::pin!(admission);
            if desired > 80 {
                assert!(futures::poll!(&mut admission).is_pending());
            } else {
                assert!(futures::poll!(&mut admission).is_ready());
            }
            tracker.switch_to_live();
            assert_eq!(tracker.reserved_bytes(), (desired as u64).min(80));
            if desired > 80 {
                release.send(()).unwrap();
                assert!(admission.await.unwrap());
            }
            if succeeds {
                tracker.grow((desired - 40) as u64, Instant::now());
                assert_eq!(tracker.current_bytes(), desired as u64);
                assert_eq!(tracker.reserved_bytes(), desired as u64);
                assert_eq!(controller.headroom_bytes(), 200 - desired as u64);
            } else {
                tracker.memory_grow_failed();
                assert_eq!(tracker.current_bytes(), 40);
                assert_eq!(tracker.reserved_bytes(), 40);
                assert_eq!(controller.headroom_bytes(), 160);
                limits.update_memory_limit(50);
                assert!(!tracker.exceeds_current_limit());
            }
        }
    }

    #[test]
    async fn agent_limit_rejection_preserves_memory_and_host_capacity() {
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe::new(200, 0)),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let reservation = controller.admit(40, &NoEvictionSource).await.unwrap();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 50, 0, 0, 0)),
            Arc::new(Mutex::new(reservation)),
            Instant::now(),
        );
        tracker.reconcile(40, Instant::now());
        tracker.meter().resume(40, Instant::now());
        let denied = admit_memory_growth(&tracker, 50, 40, 51, None, |bytes| {
            controller.admit(bytes, &NoEvictionSource)
        })
        .await;
        assert!(matches!(
            denied.unwrap_err().downcast_ref::<GolemSpecificWasmTrap>(),
            Some(GolemSpecificWasmTrap::WorkerExceededMemoryLimit)
        ));
        assert_eq!(tracker.current_bytes(), 40);
        assert_eq!(tracker.reserved_bytes(), 40);
        assert_eq!(tracker.billable_bytes(), 40);
        assert_eq!(controller.headroom_bytes(), 160);
    }

    #[test]
    async fn reconstructed_capacity_is_admitted_and_released_before_live_growth()
    -> anyhow::Result<()> {
        for reconstructed in [24, 57] {
            let controller = Arc::new(AdmissionController::new(
                Box::new(FixedProbe::new(100, 0)),
                AdmissionPolicy { usable_ratio: 1.0 },
            ));
            let reservation = controller.admit(40, &NoEvictionSource).await.unwrap();
            let tracker = LinearMemoryTracker::new(
                40,
                40,
                AgentMode::Durable,
                true,
                Arc::new(AtomicResourceEntry::new(0, 100, 0, 0, 0)),
                Arc::new(Mutex::new(reservation)),
                Instant::now(),
            );
            tracker.begin_instantiation(false);
            admit_memory_growth(&tracker, 100, 0, reconstructed, None, |bytes| {
                controller.admit(bytes, &NoEvictionSource)
            })
            .await?;
            tracker.grow(reconstructed as u64, Instant::now());
            assert_eq!(
                controller.headroom_bytes(),
                100 - (reconstructed as u64).max(40)
            );
            tracker.reconcile(reconstructed as u64, Instant::now());
            tracker.switch_to_live();
            assert_eq!(controller.headroom_bytes(), 100 - reconstructed as u64);
            admit_memory_growth(
                &tracker,
                100,
                reconstructed,
                reconstructed + 9,
                None,
                |bytes| controller.admit(bytes, &NoEvictionSource),
            )
            .await?;
            tracker.grow(9, Instant::now());
            assert_eq!(tracker.current_bytes(), reconstructed as u64 + 9);
            assert_eq!(controller.headroom_bytes(), 91 - reconstructed as u64);
            let before = controller.headroom_bytes();
            let denied =
                admit_memory_growth(&tracker, 200, reconstructed + 9, 101, None, |bytes| {
                    controller.admit(bytes, &NoEvictionSource)
                })
                .await;
            assert!(matches!(
                denied.unwrap_err().downcast_ref::<GolemSpecificWasmTrap>(),
                Some(GolemSpecificWasmTrap::WorkerOutOfMemory)
            ));
            assert_eq!(controller.headroom_bytes(), before);
            assert_eq!(tracker.current_bytes(), reconstructed as u64 + 9);
        }
        Ok(())
    }
}
