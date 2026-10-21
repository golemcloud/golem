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

pub(super) fn idle_refresh_threshold_secs(
    batch_update_interval: Duration,
    limit_refresh_interval: Duration,
) -> i64 {
    limit_refresh_interval
        .saturating_sub(batch_update_interval)
        .as_secs()
        .min(i64::MAX as u64) as i64
}

impl ResourceLimitsGrpc {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        batch_update_interval: Duration,
        limit_refresh_interval: Duration,
        metering: ResourceUsageMeteringConfig,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let svc = Self {
            client: registry_service,
            entries: scc::HashMap::new(),
            metering,
            refresh_generation: AtomicU64::new(1),
        };
        let svc = Arc::new(svc);
        let svc_weak = Arc::downgrade(&svc);

        // Background task for batch updates
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(batch_update_interval);
            let refresh_threshold_secs =
                idle_refresh_threshold_secs(batch_update_interval, limit_refresh_interval);
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        break;
                    }
                    _ = tick.tick() => {}
                }

                let svc_arc = match svc_weak.upgrade() {
                    Some(s) => s,
                    None => {
                        // service itself was dropped, we can exit
                        break;
                    }
                };

                svc_arc.send_batch(refresh_threshold_secs).await;
            }
        });

        svc
    }

    pub(super) async fn fetch_resource_limits(
        &self,
        account_id: AccountId,
    ) -> Result<golem_service_base::model::ResourceLimits, WorkerExecutorError> {
        debug!("Fetching resource limits for account {account_id}");

        let last_known_limits = self
            .client
            .get_resource_limits(
                account_id,
                golem_service_base::clients::registry::ResourceUsageMetering {
                    compute: self.metering.compute,
                    memory: self.metering.memory,
                    filesystem: self.metering.filesystem,
                },
            )
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed fetching resource limits: {}",
                    e.to_safe_string()
                ))
            })?;

        Ok(last_known_limits)
    }

    pub(super) fn next_refresh_generation(&self) -> u64 {
        self.refresh_generation.fetch_add(1, Ordering::AcqRel)
    }

    pub(super) async fn begin_monthly_refresh(
        &self,
        account_id: AccountId,
        generation: u64,
        update: &ResourceUsageUpdate,
        memory_by_mode: (i64, i64),
    ) {
        if let Some(cell) = self
            .entries
            .read_async(&account_id, |_, entry| entry.clone())
            .await
            && let Some(entry) = cell.get()
        {
            entry.begin_monthly_refresh(generation, update, memory_by_mode.0, memory_by_mode.1);
        }
    }

    /// Builds and sends a single batch to the registry covering:
    /// - active accounts with non-zero fuel, memory, storage, HTTP, or RPC deltas
    /// - otherwise-idle accounts past the refresh threshold
    ///
    /// On success, updates all entries via `update_last_known_limits`. On
    /// failure, drops the captured batch under the accepted bounded-loss semantics and
    /// resets in-flight quota tracking; stale idle accounts are retried next tick.
    pub(super) async fn send_batch(&self, refresh_threshold_secs: i64) {
        async {
            let mut entries = Vec::new();
            self.entries
                .iter_async(|account_id, cell| {
                    if let Some(entry) = cell.get() {
                        entries.push((*account_id, entry.clone()));
                    }
                    true
                })
                .await;
            let mut updates = HashMap::new();
            let mut memory_mode_updates = HashMap::new();
            for (account_id, entry) in entries {
                if let Some(captured) = entry.capture_usage_update(refresh_threshold_secs) {
                    updates.insert(account_id, captured.update);
                    memory_mode_updates.insert(
                        account_id,
                        (
                            captured.durable_memory_gb_seconds_delta,
                            captured.ephemeral_memory_gb_seconds_delta,
                        ),
                    );
                }
            }

            if updates.is_empty() {
                return;
            }

            let mut pending_updates = updates.into_iter();
            loop {
                let updates: HashMap<_, _> = pending_updates.by_ref().take(256).collect();
                if updates.is_empty() {
                    break;
                }
                let refresh_generation = self.next_refresh_generation();
                for (account_id, update) in &updates {
                    self.begin_monthly_refresh(
                        *account_id,
                        refresh_generation,
                        update,
                        memory_mode_updates
                            .get(account_id)
                            .copied()
                            .unwrap_or_default(),
                    )
                    .await;
                }

                tracing::debug!(
                    fuel_accounts = updates.values().filter(|u| u.fuel_delta != 0).count(),
                    memory_accounts = updates
                        .values()
                        .filter(|u| u.memory_gb_seconds_delta != 0)
                        .count(),
                    durable_storage_accounts = updates
                        .values()
                        .filter(|u| u.durable_storage_byte_seconds_delta != 0)
                        .count(),
                    ephemeral_storage_accounts = updates
                        .values()
                        .filter(|u| u.ephemeral_storage_byte_seconds_delta != 0)
                        .count(),
                    http_accounts = updates
                        .values()
                        .filter(|u| u.http_call_count_delta > 0)
                        .count(),
                    rpc_accounts = updates
                        .values()
                        .filter(|u| u.rpc_call_count_delta > 0)
                        .count(),
                    blob_storage_accounts = updates
                        .values()
                        .filter(|u| u.blob_storage_bytes_delta != 0)
                        .count(),
                    stale_idle_accounts = updates
                        .values()
                        .filter(|u| {
                            u.fuel_delta == 0
                                && u.memory_gb_seconds_delta == 0
                                && u.durable_storage_byte_seconds_delta == 0
                                && u.ephemeral_storage_byte_seconds_delta == 0
                                && u.http_call_count_delta == 0
                                && u.rpc_call_count_delta == 0
                                && u.blob_storage_bytes_delta == 0
                        })
                        .count(),
                    "Sending resource usage batch"
                );

                // Send resource usage batch. The response refreshes all account limits
                // (fuel, memory, disk, per-invocation caps, and monthly call budgets)
                // for every account in `updates`.
                match self
                    .client
                    .batch_update_resource_usage(updates.clone())
                    .await
                {
                    Ok(updated_limits) => {
                        for (account_id, update) in &updates {
                            let Some(resource_limits) = updated_limits.0.get(account_id) else {
                                record_resource_usage_batch_update_failure();
                                error!(
                                    %account_id,
                                    "Registry did not apply resource usage update; retaining retryable byte-time usage"
                                );
                                self.finish_in_flight_update(
                                    *account_id,
                                    refresh_generation,
                                    AtomicResourceEntry::finish_confirmed_non_application,
                                )
                                .await;
                                self.settle_blob_storage_delta(*account_id, update.blob_storage_bytes_delta).await;
                                continue;
                            };
                            if !resource_limits.usage_update_applied {
                                continue;
                            }
                            let durable = update.durable_storage_byte_seconds_delta;
                            let ephemeral = update.ephemeral_storage_byte_seconds_delta;
                            let (durable_memory, ephemeral_memory) = memory_mode_updates
                                .get(account_id)
                                .copied()
                                .unwrap_or_default();
                            if durable == 0
                                && ephemeral == 0
                                && durable_memory == 0
                                && ephemeral_memory == 0
                            {
                                continue;
                            }

                            let account_id = account_id.to_string();
                            if durable > 0 {
                                record_storage_byte_seconds(
                                    &account_id,
                                    AgentMode::Durable,
                                    durable,
                                );
                            }
                            if ephemeral > 0 {
                                record_storage_byte_seconds(
                                    &account_id,
                                    AgentMode::Ephemeral,
                                    ephemeral,
                                );
                            }
                            if durable_memory > 0 {
                                record_memory_gb_seconds(
                                    &account_id,
                                    AgentMode::Durable,
                                    durable_memory,
                                );
                            }
                            if ephemeral_memory > 0 {
                                record_memory_gb_seconds(
                                    &account_id,
                                    AgentMode::Ephemeral,
                                    ephemeral_memory,
                                );
                            }
                        }
                        for (account_id, resource_limits) in updated_limits.0 {
                            // Blob stock deltas are not safe to resend after an ambiguous batch
                            // outcome: the registry may already have committed them. Settle every
                            // submitted delta locally and rely on physical reconciliation to repair
                            // either direction of drift.
                            let settled_blob_storage_delta = updates
                                .get(&account_id)
                                .map_or(0, |update| update.blob_storage_bytes_delta);
                            self.update_last_known_limits(
                                account_id,
                                resource_limits,
                                refresh_generation,
                                settled_blob_storage_delta,
                            )
                            .await;
                        }
                    }
                    Err(err) => {
                        record_resource_usage_batch_update_failure();
                        error!(error = %err, "Failed to send batched resource usage updates");
                        for (account_id, update) in &updates {
                            if update.fuel_delta != 0
                                || update.memory_gb_seconds_delta != 0
                                || update.memory_byte_nanoseconds_remainder != 0
                                || update.durable_storage_byte_seconds_delta != 0
                                || update.durable_storage_byte_nanoseconds_remainder != 0
                                || update.ephemeral_storage_byte_seconds_delta != 0
                                || update.ephemeral_storage_byte_nanoseconds_remainder != 0
                                || update.http_call_count_delta > 0
                                || update.rpc_call_count_delta > 0
                                || update.blob_storage_bytes_delta != 0
                            {
                                error!(
                                    %account_id,
                                    fuel_delta = update.fuel_delta,
                                    memory_gb_seconds_delta = update.memory_gb_seconds_delta,
                                    durable_storage_byte_seconds_delta = update.durable_storage_byte_seconds_delta,
                                    ephemeral_storage_byte_seconds_delta = update.ephemeral_storage_byte_seconds_delta,
                                    http_call_count_delta = update.http_call_count_delta,
                                    rpc_call_count_delta = update.rpc_call_count_delta,
                                    blob_storage_bytes_delta = update.blob_storage_bytes_delta,
                                    "Dropping ambiguous resource usage update"
                                );
                                self.settle_blob_storage_delta(
                                    *account_id,
                                    update.blob_storage_bytes_delta,
                                )
                                .await;
                                self.finish_in_flight_update(
                                    *account_id,
                                    refresh_generation,
                                    AtomicResourceEntry::finish_ambiguous_transport_failure,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
        .instrument(info_span!("resource_limits_batch_update"))
        .await
    }

    pub(super) async fn update_last_known_limits(
        &self,
        account_id: AccountId,
        updated_limits: golem_service_base::model::ResourceLimits,
        refresh_generation: u64,
        settled_blob_storage_delta: i64,
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if !entry.apply_monthly_snapshot_with_policy_revision(
                refresh_generation,
                updated_limits.monthly_policy.clone(),
                updated_limits.monthly_usage_mode_revision,
                updated_limits.monthly_policy_revision,
                updated_limits.monthly_policy_status,
                updated_limits.usage_update_applied,
            ) {
                self.settle_blob_storage_delta(account_id, settled_blob_storage_delta)
                    .await;
                return;
            }
            entry.update_memory_limit(updated_limits.max_memory_per_worker);
            entry.max_table_elements.store(
                updated_limits.max_table_elements_per_worker as usize,
                Ordering::Release,
            );
            entry
                .queue_agent_filesystem_limit(account_id, updated_limits.max_disk_space_per_worker);
            entry.per_invocation_http_call_limit.store(
                updated_limits.per_invocation_http_call_limit,
                Ordering::Release,
            );
            entry.per_invocation_rpc_call_limit.store(
                updated_limits.per_invocation_rpc_call_limit,
                Ordering::Release,
            );
            entry.syncing_http_calls.store(0, Ordering::Release);
            entry
                .available_http_calls_from_server
                .store(updated_limits.available_http_calls, Ordering::Release);
            entry.syncing_rpc_calls.store(0, Ordering::Release);
            entry
                .available_rpc_calls_from_server
                .store(updated_limits.available_rpc_calls, Ordering::Release);
            {
                let mut quota = entry.blob_storage_quota.lock().unwrap();
                quota.syncing_delta = quota
                    .syncing_delta
                    .saturating_sub(settled_blob_storage_delta);
                quota.available_from_server = updated_limits.available_blob_storage_bytes;
            }
            entry.max_concurrent_agents_per_executor.store(
                updated_limits.max_concurrent_agents_per_executor,
                Ordering::Release,
            );
            entry
                .oplog_writes_per_second
                .store(updated_limits.oplog_writes_per_second, Ordering::Release);
            entry
                .last_refresh_secs
                .store(Utc::now().timestamp(), Ordering::Release);
        }
    }

    pub(super) async fn settle_blob_storage_delta(&self, account_id: AccountId, delta: i64) {
        if delta == 0 {
            return;
        }
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            let mut quota = entry.blob_storage_quota.lock().unwrap();
            quota.syncing_delta = quota.syncing_delta.saturating_sub(delta);
            if delta > 0 {
                quota.available_from_server =
                    quota.available_from_server.saturating_sub(delta as u64);
            } else {
                quota.available_from_server = quota
                    .available_from_server
                    .saturating_add(delta.unsigned_abs());
            }
        }
    }

    pub(super) async fn finish_in_flight_update(
        &self,
        account_id: AccountId,
        refresh_generation: u64,
        finish: fn(&AtomicResourceEntry, u64) -> bool,
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if !finish(entry, refresh_generation) {
                return;
            }
            entry.syncing_http_calls.store(0, Ordering::Release);
            entry.syncing_rpc_calls.store(0, Ordering::Release);
        }
    }
}

#[async_trait]
impl ResourceLimits for ResourceLimitsGrpc {
    async fn initialize_account(
        &self,
        account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        let cell = self
            .entries
            .entry_async(account_id)
            .await
            .or_insert_with(|| Arc::new(OnceCell::new()));

        let entry = cell
            .get_or_try_init(|| async {
                let refresh_generation = self.next_refresh_generation();
                let fetched = self.fetch_resource_limits(account_id).await?;
                if (self.metering.compute || self.metering.any_byte_time_enabled())
                    && fetched.monthly_policy_status
                        == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
                {
                    return Err(MonthlyResourceCapacityError::unavailable_error());
                }
                let entry = Arc::new(
                    AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                        fetched.monthly_policy,
                        fetched.max_memory_per_worker as usize,
                        fetched.max_table_elements_per_worker as usize,
                        fetched.max_disk_space_per_worker,
                        fetched.per_invocation_http_call_limit,
                        fetched.per_invocation_rpc_call_limit,
                        fetched.available_http_calls,
                        fetched.available_rpc_calls,
                        fetched.max_concurrent_agents_per_executor,
                        fetched.oplog_writes_per_second,
                        self.metering,
                        fetched.monthly_usage_mode_revision,
                        fetched.monthly_policy_revision,
                        refresh_generation,
                    ),
                );
                entry
                    .blob_storage_quota
                    .lock()
                    .unwrap()
                    .available_from_server = fetched.available_blob_storage_bytes;
                Ok::<Arc<AtomicResourceEntry>, WorkerExecutorError>(entry)
            })
            .await?;

        Ok(entry.clone())
    }
}
