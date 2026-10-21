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
        let (_, reconciling) = self.linear_memory.grow(delta, Instant::now());
        if self.runtime == OwnerRuntime::Agent
            && self.state.is_live()
            && !self.state.snapshotting_mode
            && !reconciling
        {
            // This is called from the `memory.grow` async resource limiter, which
            // Wasmtime runs through a blocking libcall on the store's fiber. While
            // that libcall waits, the store cannot make progress, so nothing may be
            // awaited here (see https://github.com/bytecodealliance/wasmtime/issues/11869).
            // The oplog hint runs as a fire-and-forget job on the worker-state
            // actor. Host-capacity admission completed before Wasmtime committed
            // this growth.
            self.public_state.worker().request_memory_grow(delta);
        }
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
        let tracker = &self.linear_memory;
        let growth = tracker.prepare_unshared_growth(current, desired);
        let Some(growth) = validate_unshared_memory_growth(
            growth,
            self.max_linear_memory_size(),
            desired,
            maximum,
        ) else {
            tracker.memory_grow_failed();
            return Err(GolemSpecificWasmTrap::WorkerExceededMemoryLimit.into());
        };

        if growth.admission_delta > 0 {
            let Some(grant) = self.try_acquire_linear_memory(growth.admission_delta).await else {
                tracker.memory_grow_failed();
                crate::metrics::workers::record_worker_memory_grow_rejected();
                return Err(GolemSpecificWasmTrap::WorkerOutOfMemory.into());
            };
            tracker.retain_growth_grant(grant);
        }
        Ok(true)
    }

    pub fn unshared_memory_growth_failed(&self) {
        self.linear_memory.memory_grow_failed();
    }
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
