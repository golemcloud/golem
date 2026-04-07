// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

pub mod types;

use crate::durable_host::quota::types::{LeaseInterestHandle, QuotaTokenEntry, ReservationEntry};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::quota::host::{Host, HostQuotaToken, HostReservation, FailedReservation};
use crate::services::quota::LeaseInterest;
use crate::workerctx::WorkerCtx;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::host_functions;
use golem_common::model::oplog::payload::{
    HostRequestQuotaCommitRequest, HostRequestQuotaReserveRequest, HostRequestQuotaTokenRequest,
    HostResponseQuotaCommitResult, HostResponseQuotaReserveResult, HostResponseQuotaTokenAcquired,
};
use golem_common::model::quota::ReserveResult;
use golem_common::model::quota::ResourceName;
use golem_common::model::{ScheduledAction, Timestamp};
use golem_service_base::error::worker_executor::GolemSpecificWasmTrap;
use wasmtime::component::Resource;

/// Ensure the token is live, acquiring a lease if it is still pending.
/// Returns a mutable reference to the `LeaseInterest`.
async fn get_live_lease_interest<'a, Ctx: WorkerCtx>(
    ctx: &'a mut DurableWorkerCtx<Ctx>,
    token_resource: &Resource<QuotaTokenEntry>,
) -> anyhow::Result<&'a mut LeaseInterest> {
    let svc = ctx.state.quota_service.clone();
    let entry = ctx.table().get_mut(token_resource)?;

    if let LeaseInterestHandle::Live(ref mut interest) = entry.lease {
        return Ok(interest);
    }

    let interest = match &entry.lease {
        LeaseInterestHandle::Pending(p) => svc
            .acquire(
                p.environment_id,
                p.resource_name.clone(),
                p.expected_use,
                p.last_credit,
                Some(p.last_credit_at),
            )
            .await
            .map_err(|e| anyhow::anyhow!("quota acquire failed: {e}"))?,
        LeaseInterestHandle::Live(_) => unreachable!(),
    };

    entry.lease = LeaseInterestHandle::Live(interest);

    match &mut entry.lease {
        LeaseInterestHandle::Live(i) => Ok(i),
        _ => unreachable!(),
    }
}

impl<Ctx: WorkerCtx> HostQuotaToken for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        resource_name: String,
        expected_use: u64,
    ) -> anyhow::Result<Resource<QuotaTokenEntry>> {
        DurabilityHost::observe_function_call(
            self,
            "golem::quota::quota-token",
            "[constructor]quota-token",
        );

        let env_id = self.owned_agent_id.environment_id;
        let rn = ResourceName(resource_name.clone());
        let is_live = self.state.is_live();

        let token_entry = if is_live {
            let svc = self.state.quota_service.clone();
            let interest = svc
                .acquire(env_id, rn, expected_use, 0, None)
                .await
                .map_err(|e| anyhow::anyhow!("quota acquire failed: {e}"))?;

            let credit_at_ms = interest.last_credit_value_at.timestamp_millis();

            let durability = Durability::<host_functions::GolemQuotaTokenNew>::new(
                self,
                DurableFunctionType::WriteLocal,
            )
            .await?;
            durability
                .persist(
                    self,
                    HostRequestQuotaTokenRequest {
                        resource_name: resource_name.clone(),
                        expected_use,
                    },
                    HostResponseQuotaTokenAcquired { credit_at_ms },
                )
                .await?;

            QuotaTokenEntry::live(interest)
        } else {
            let durability = Durability::<host_functions::GolemQuotaTokenNew>::new(
                self,
                DurableFunctionType::WriteLocal,
            )
            .await?;
            let replayed: HostResponseQuotaTokenAcquired = durability.replay(self).await?;

            let credit_at = Utc
                .timestamp_millis_opt(replayed.credit_at_ms)
                .single()
                .unwrap_or_else(Utc::now);

            // Credit is always 0 on a fresh acquire; timestamp is restored from oplog.
            QuotaTokenEntry::pending(env_id, rn, expected_use, 0, credit_at)
        };

        let resource = self.table().push(token_entry)?;
        Ok(resource)
    }

    async fn reserve(
        &mut self,
        self_: Resource<QuotaTokenEntry>,
        amount: u64,
    ) -> anyhow::Result<Result<Resource<ReservationEntry>, FailedReservation>> {
        DurabilityHost::observe_function_call(self, "golem::quota::quota-token", "reserve");

        let durability = Durability::<host_functions::GolemQuotaTokenReserve>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let (reserve_result, is_live) = if self.state.is_live() {
            let svc = self.state.quota_service.clone();
            let interest = get_live_lease_interest(self, &self_).await?;
            let result = svc.try_reserve(interest, amount).await;

            let credit_after = interest.last_credit_value;
            let credit_after_at_ms = interest.last_credit_value_at.timestamp_millis();

            durability
                .persist(
                    self,
                    HostRequestQuotaReserveRequest { amount },
                    HostResponseQuotaReserveResult {
                        result: result.clone(),
                        credit_after,
                        credit_after_at_ms,
                    },
                )
                .await?;

            (result, true)
        } else {
            let replayed: HostResponseQuotaReserveResult = durability.replay(self).await?;

            let credit_at = Utc
                .timestamp_millis_opt(replayed.credit_after_at_ms)
                .single()
                .unwrap_or_else(Utc::now);

            self.table()
                .get_mut(&self_)?
                .update_replayed_credit(replayed.credit_after, credit_at);

            (replayed.result, false)
        };

        match reserve_result {
            ReserveResult::Ok(reservation) => {
                let res_entry = if is_live {
                    ReservationEntry::Live {
                        reservation,
                        token: Resource::new_own(self_.rep()),
                    }
                } else {
                    ReservationEntry::Replayed {
                        token: Resource::new_own(self_.rep()),
                    }
                };
                Ok(Ok(self.table().push(res_entry)?))
            }
            ReserveResult::InsufficientAllocation {
                enforcement_action,
                estimated_wait_nanos,
            } => {
                use golem_common::model::quota::EnforcementAction;
                match enforcement_action {
                    EnforcementAction::Reject => Ok(Err(FailedReservation {
                        estimated_wait_nanos,
                    })),
                    EnforcementAction::Throttle => {
                        let agent_created_by = self.created_by();
                        let owned_agent_id = self.owned_agent_id().clone();
                        let scheduler_service = self.scheduler_service();

                        let quota_token = self.table().get(&self_)?;
                        if let Some(estimated_wait_nanos) = estimated_wait_nanos {
                            // schedule a continuation for when we expect the quota to be ready to serve us. If it still doesn't have capacity
                            // we will just end up suspending again.
                            let estimated_wait_nanos_i64 = if estimated_wait_nanos > i64::MAX as u64
                            {
                                i64::MAX
                            } else {
                                estimated_wait_nanos as i64
                            };
                            let next_attempt_time: DateTime<Utc> = Utc::now()
                                .checked_add_signed(TimeDelta::nanoseconds(
                                    estimated_wait_nanos_i64,
                                ))
                                .unwrap_or(DateTime::<Utc>::MAX_UTC);
                            scheduler_service
                                .schedule(
                                    next_attempt_time,
                                    ScheduledAction::Resume {
                                        agent_created_by,
                                        owned_agent_id,
                                    },
                                )
                                .await;
                        }
                        anyhow::bail!(GolemSpecificWasmTrap::AgentThrottledByQuota {
                            environment_id: quota_token.environment_id(),
                            resource_name: quota_token.resource_name().clone(),
                            timestamp: Timestamp::now_utc()
                        })
                    }
                    EnforcementAction::Terminate => {
                        let quota_token = self.table().get(&self_)?;
                        anyhow::bail!(GolemSpecificWasmTrap::AgentTerminatedByQuota {
                            environment_id: quota_token.environment_id(),
                            resource_name: quota_token.resource_name().clone()
                        })
                    }
                }
            }
        }
    }

    async fn split(
        &mut self,
        self_: Resource<QuotaTokenEntry>,
        child_expected_use: u64,
    ) -> anyhow::Result<Resource<QuotaTokenEntry>> {
        DurabilityHost::observe_function_call(self, "golem::quota::quota-token", "split");

        let child_entry = {
            let entry = self.table().get_mut(&self_)?;
            match &mut entry.lease {
                LeaseInterestHandle::Live(interest) => {
                    let child_interest = interest
                        .split(child_expected_use)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    QuotaTokenEntry::live(child_interest)
                }
                LeaseInterestHandle::Pending(p) => {
                    if child_expected_use > p.expected_use {
                        anyhow::bail!(
                            "cannot split {} units from a token with only {} expected-use",
                            child_expected_use,
                            p.expected_use
                        );
                    }
                    let parent_expected_use = p.expected_use - child_expected_use;
                    // Proportional credit split.
                    let child_credit = if p.expected_use > 0 {
                        (p.last_credit as i128 * child_expected_use as i128
                            / p.expected_use as i128) as i64
                    } else {
                        0
                    };
                    let parent_credit = p.last_credit - child_credit;
                    let credit_at = p.last_credit_at;

                    p.expected_use = parent_expected_use;
                    p.last_credit = parent_credit;

                    QuotaTokenEntry::pending(
                        p.environment_id,
                        p.resource_name.clone(),
                        child_expected_use,
                        child_credit,
                        credit_at,
                    )
                }
            }
        };

        let child_resource = self.table().push(child_entry)?;
        Ok(child_resource)
    }

    async fn merge(
        &mut self,
        self_: Resource<QuotaTokenEntry>,
        other: Resource<QuotaTokenEntry>,
    ) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::quota::quota-token", "merge");

        // Validate that both tokens refer to the same resource before consuming `other`.
        let (self_env, self_rn) = {
            let e = self.table().get(&self_)?;
            (e.environment_id(), e.resource_name().clone())
        };
        let (other_env, other_rn) = {
            let e = self.table().get(&other)?;
            (e.environment_id(), e.resource_name().clone())
        };
        if self_env != other_env || self_rn != other_rn {
            anyhow::bail!(
                "cannot merge tokens for different resources: `{}` vs `{}`",
                self_rn,
                other_rn
            );
        }

        // Consume `other` from the resource table.
        let other_entry = self.table().delete(other)?;

        let self_entry = self.table().get_mut(&self_)?;
        match (&mut self_entry.lease, other_entry.lease) {
            (LeaseInterestHandle::Live(self_interest), LeaseInterestHandle::Live(other_interest)) => {
                self_interest
                    .merge(other_interest)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            (LeaseInterestHandle::Pending(p), LeaseInterestHandle::Pending(other_p)) => {
                p.expected_use = p.expected_use.saturating_add(other_p.expected_use);
                p.last_credit = p.last_credit.saturating_add(other_p.last_credit);
            }
            (LeaseInterestHandle::Live(interest), LeaseInterestHandle::Pending(other_p)) => {
                interest.expected_use =
                    interest.expected_use.saturating_add(other_p.expected_use);
                interest.credit_rate =
                    interest.expected_use as f64 * crate::services::quota::CREDIT_RATE_FACTOR;
                interest.max_credit =
                    crate::services::quota::max_credit_for(interest.expected_use);
                interest.last_credit_value = interest
                    .last_credit_value
                    .saturating_add(other_p.last_credit)
                    .min(interest.max_credit);
            }
            (LeaseInterestHandle::Pending(p), LeaseInterestHandle::Live(other_interest)) => {
                p.expected_use = p.expected_use.saturating_add(other_interest.expected_use);
                p.last_credit = p
                    .last_credit
                    .saturating_add(other_interest.last_credit_value);
            }
        }

        Ok(())
    }

    async fn drop(&mut self, rep: Resource<QuotaTokenEntry>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::quota::quota-token", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostReservation for DurableWorkerCtx<Ctx> {
    async fn commit(&mut self, self_: Resource<ReservationEntry>, used: u64) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::quota::reservation", "commit");

        let entry = self.table().delete(self_)?;

        match entry {
            ReservationEntry::Live { reservation, token } => {
                let token_resource: Resource<QuotaTokenEntry> = Resource::new_own(token.rep());
                let svc = self.state.quota_service.clone();

                let interest = get_live_lease_interest(self, &token_resource).await?;
                svc.commit(interest, reservation, used).await;
                let credit_after = interest.last_credit_value;
                let credit_after_at_ms = interest.last_credit_value_at.timestamp_millis();

                let durability = Durability::<host_functions::GolemQuotaReservationCommit>::new(
                    self,
                    DurableFunctionType::WriteLocal,
                )
                .await?;
                durability
                    .persist(
                        self,
                        HostRequestQuotaCommitRequest { used },
                        HostResponseQuotaCommitResult {
                            credit_after,
                            credit_after_at_ms,
                        },
                    )
                    .await?;
            }
            ReservationEntry::Replayed { token } => {
                let token_resource: Resource<QuotaTokenEntry> = Resource::new_own(token.rep());

                let durability = Durability::<host_functions::GolemQuotaReservationCommit>::new(
                    self,
                    DurableFunctionType::WriteLocal,
                )
                .await?;

                let replayed: HostResponseQuotaCommitResult = durability.replay(self).await?;

                let credit_at = Utc
                    .timestamp_millis_opt(replayed.credit_after_at_ms)
                    .single()
                    .unwrap_or_else(Utc::now);

                self.table()
                    .get_mut(&token_resource)?
                    .update_replayed_credit(replayed.credit_after, credit_at);
            }
        }

        Ok(())
    }

    async fn drop(&mut self, rep: Resource<ReservationEntry>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::quota::reservation", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
