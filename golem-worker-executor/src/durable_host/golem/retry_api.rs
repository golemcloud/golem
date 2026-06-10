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

use crate::durable_host::concurrent::{CallHandle, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::get_oplog_entry;
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
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRetryPolicyByName, HostRequestGolemRetryResolvePolicy,
    HostRequestNoInput, HostResponseGolemRetryNamedPolicy, HostResponseGolemRetryPolicies,
    HostResponseGolemRetryResolvedPolicy, OplogEntry,
};
use golem_common::model::retry_policy::{NamedRetryPolicy, PredicateValue, RetryContext};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_retry_policies(&mut self) -> anyhow::Result<Vec<WitNamedRetryPolicy>> {
        self.observe_function_call("golem::api::retry", "get_retry_policies");

        let handle = CallHandle::<GolemApiRetryGetRetryPolicies, NotCancellable>::start(
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

        let handle = CallHandle::<GolemApiRetryGetRetryPolicyByName, NotCancellable>::start(
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
    ) -> anyhow::Result<Option<WitRetryPolicy>> {
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

        let handle = CallHandle::<GolemApiRetryResolveRetryPolicy, NotCancellable>::start(
            self,
            HostRequestGolemRetryResolvePolicy {
                verb,
                noun_uri,
                properties: properties_for_persist,
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let persisted = handle
            .run(self, async |ctx| -> anyhow::Result<_> {
                let policies = ctx.state.named_retry_policies().await;
                let resolved = match NamedRetryPolicy::resolve(&policies, &props) {
                    Ok(Some(matched)) => Some(matched.policy.clone()),
                    Ok(None) => None,
                    Err(err) => {
                        return Err(anyhow::anyhow!("Retry policy resolution error: {err}"));
                    }
                };
                Ok(HostResponseGolemRetryResolvedPolicy { policy: resolved })
            })
            .await?;

        Ok(persisted.policy.map(|p| p.into()))
    }

    async fn set_retry_policy(&mut self, policy: WitNamedRetryPolicy) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::retry", "set_retry_policy");

        let named_policy: NamedRetryPolicy = policy.into();

        if self.state.is_live() {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::set_retry_policy(named_policy.clone()))
                .await;
        } else {
            let (_, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::SetRetryPolicy)?;
        }

        self.state.apply_set_retry_policy(named_policy);
        Ok(())
    }

    async fn remove_retry_policy(&mut self, name: String) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::retry", "remove_retry_policy");

        if self.state.is_live() {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::remove_retry_policy(name.clone()))
                .await;
        } else {
            let (_, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::RemoveRetryPolicy)?;
        }

        self.state.apply_remove_retry_policy(&name);
        Ok(())
    }
}
