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

use crate::durable_host::concurrent::{DurableCallSession, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem_api_1_x::retry::{
    Host, NamedRetryPolicy as WitNamedRetryPolicy, PredicateValue as WitPredicateValue,
    RetryPolicy as WitRetryPolicy,
};
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    GolemApiRetryGetRetryPolicies, GolemApiRetryGetRetryPolicyByName,
    GolemApiRetryResolveRetryPolicy,
};
use golem_common::model::oplog::payload::types::SerializableToolPositionalRequest;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRetryPolicyByName, HostRequestGolemRetryResolvePolicy,
    HostRequestNoInput, HostResponseGolemRetryNamedPolicy, HostResponseGolemRetryPolicies,
    HostResponseGolemRetryResolvedPolicy, OplogEntry,
};
use golem_common::model::retry_policy::{NamedRetryPolicy, PredicateValue, RetryContext};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn next_retry_delay(
        &mut self,
        policy: Option<WitRetryPolicy>,
        state: Vec<u8>,
        elapsed: u64,
        verb: String,
        noun_uri: String,
        properties: Vec<(String, WitPredicateValue)>,
    ) -> anyhow::Result<Result<crate::preview2::golem_api_1_x::retry::RetryDecision, String>> {
        use golem_common::model::oplog::host_functions::GolemApiRetryNextRetryDelay;
        use golem_common::model::oplog::{
            HostRequestGolemRetryNextDelay, HostResponseGolemRetryNextDelay,
        };
        use golem_common::model::retry_policy::{RetryPolicy, ThreadRng};
        use std::time::Duration;

        self.observe_function_call("golem::api::retry", "next_retry_delay");
        let policy_identity = format!("{policy:?}");
        let policy = policy.map(RetryPolicy::try_from).transpose();
        let properties: Vec<(String, PredicateValue)> = properties
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect();
        let handle = DurableCallSession::<GolemApiRetryNextRetryDelay, NotCancellable>::start(
            self,
            HostRequestGolemRetryNextDelay {
                policy: policy_identity,
                state: state.clone(),
                elapsed,
                verb: verb.clone(),
                noun_uri: noun_uri.clone(),
                properties: properties.clone(),
            },
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let persisted = handle
            .run(self, async |ctx| -> anyhow::Result<_> {
                let result = (|| {
                    let policy = policy?.unwrap_or_else(|| {
                        NamedRetryPolicy::default_from_config(&ctx.state.config.retry).policy
                    });
                    if verb == "saga" && !policy.is_bounded() {
                        return Err(
                            "Saga retry policy must have a finite count or time bound".into()
                        );
                    }
                    let mut props = RetryContext::custom(&verb, &noun_uri);
                    for (key, value) in properties {
                        props.set(key, value);
                    }
                    policy
                        .next_retry_delay(
                            &state,
                            Duration::from_nanos(elapsed),
                            &props,
                            &mut ThreadRng,
                        )
                        .map(|(state, delay)| {
                            (
                                state,
                                delay.map(|d| d.as_nanos().min(u64::MAX as u128) as u64),
                            )
                        })
                })();
                Ok(HostResponseGolemRetryNextDelay { result })
            })
            .await?;
        Ok(persisted.result.map(|(state, delay)| {
            crate::preview2::golem_api_1_x::retry::RetryDecision { state, delay }
        }))
    }

    async fn get_retry_policies(&mut self) -> anyhow::Result<Vec<WitNamedRetryPolicy>> {
        self.observe_function_call("golem::api::retry", "get_retry_policies");

        let handle = DurableCallSession::<GolemApiRetryGetRetryPolicies, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let persisted = handle
            .run(self, async |ctx| -> anyhow::Result<_> {
                let policies: Vec<NamedRetryPolicy> = ctx.state.named_retry_policies().await;
                Ok(HostResponseGolemRetryPolicies { policies })
            })
            .await?;

        Ok(persisted.policies.into_iter().map(|p| p.into()).collect())
    }

    async fn get_retry_policy_by_name(
        &mut self,
        name: String,
    ) -> anyhow::Result<Option<WitNamedRetryPolicy>> {
        self.observe_function_call("golem::api::retry", "get_retry_policy_by_name");

        let handle =
            DurableCallSession::<GolemApiRetryGetRetryPolicyByName, NotCancellable>::start(
                self,
                HostRequestGolemRetryPolicyByName { name: name.clone() },
                DurableFunctionType::ReadRemote,
            )
            .await?;

        let persisted = handle
            .run(self, async |ctx| -> anyhow::Result<_> {
                let policies = ctx.state.named_retry_policies().await;
                let found = policies.iter().find(|p| p.name == name).cloned();
                Ok(HostResponseGolemRetryNamedPolicy { policy: found })
            })
            .await?;

        Ok(persisted.policy.map(|p| p.into()))
    }

    async fn resolve_retry_policy(
        &mut self,
        verb: String,
        noun_uri: String,
        properties: Vec<(String, WitPredicateValue)>,
    ) -> anyhow::Result<Result<Option<WitRetryPolicy>, String>> {
        self.observe_function_call("golem::api::retry", "resolve_retry_policy");

        let mut props = RetryContext::custom(&verb, &noun_uri);
        let properties_for_persist: Vec<(String, PredicateValue)> = properties
            .into_iter()
            .map(|(k, v)| {
                let pv = PredicateValue::from(v);
                props.set(k.clone(), pv.clone());
                (k, pv)
            })
            .collect();

        let handle = DurableCallSession::<GolemApiRetryResolveRetryPolicy, NotCancellable>::start(
            self,
            HostRequestGolemRetryResolvePolicy {
                verb,
                noun_uri,
                properties: properties_for_persist,
            },
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let persisted = handle
            .run(self, async |ctx| -> anyhow::Result<_> {
                let policies = ctx.state.named_retry_policies().await;
                let result =
                    NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
                        &policies, &props,
                    )
                    .map(|matched| matched.map(|p| p.policy.clone()))
                    .map_err(|err| format!("Retry policy resolution error: {err}"));
                Ok(HostResponseGolemRetryResolvedPolicy { result })
            })
            .await?;

        Ok(persisted.result.map(|policy| policy.map(Into::into)))
    }

    async fn set_retry_policy(&mut self, policy: WitNamedRetryPolicy) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::retry", "set_retry_policy");

        let named_policy: NamedRetryPolicy = policy.try_into().map_err(anyhow::Error::msg)?;

        if self.state.durability_is_suppressed() {
            // Apply the in-memory change without creating durable history.
        } else if let Some((_, entry)) = self
            .get_oplog_entry_or_continue_live(SerializableToolPositionalRequest::SetRetryPolicy)
            .await?
        {
            if !matches!(entry, OplogEntry::SetRetryPolicy { .. }) {
                return Err(golem_service_base::error::worker_executor::WorkerExecutorError::unexpected_oplog_entry(
                    "SetRetryPolicy",
                    format!("{entry:?}"),
                ).into());
            }
        } else {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::set_retry_policy(
                    self.entity_parent_start_index(),
                    Box::new(named_policy.clone()),
                ))
                .await?;
        }

        self.state.apply_set_retry_policy(named_policy);
        Ok(())
    }

    async fn remove_retry_policy(&mut self, name: String) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::retry", "remove_retry_policy");

        if self.state.durability_is_suppressed() {
            // Apply the in-memory change without creating durable history.
        } else if let Some((_, entry)) = self
            .get_oplog_entry_or_continue_live(SerializableToolPositionalRequest::RemoveRetryPolicy)
            .await?
        {
            if !matches!(entry, OplogEntry::RemoveRetryPolicy { .. }) {
                return Err(golem_service_base::error::worker_executor::WorkerExecutorError::unexpected_oplog_entry(
                    "RemoveRetryPolicy",
                    format!("{entry:?}"),
                ).into());
            }
        } else {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::remove_retry_policy(
                    self.entity_parent_start_index(),
                    name.clone(),
                ))
                .await?;
        }

        self.state.apply_remove_retry_policy(&name);
        Ok(())
    }
}
