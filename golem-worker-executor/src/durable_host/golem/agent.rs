// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::agent::host::Host;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::agent::bindings::golem::agent::common::{
    AgentError, DataValue, RegisteredAgentType,
};
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{AgentId, AgentTypeName};
use golem_common::model::oplog::host_functions::{
    GolemAgentCreateWebhook, GolemAgentGetAgentType, GolemAgentGetAllAgentTypes,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemAgentGetAgentType, HostRequestGolemApiPromiseId,
    HostRequestNoInput, HostResponseGolemAgentAgentType, HostResponseGolemAgentAgentTypes,
    HostResponseGolemAgentWebhookUrl,
};
use golem_common::model::PromiseId;
use golem_service_base::custom_api::AgentWebhookId;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_agent_types(&mut self) -> anyhow::Result<Vec<RegisteredAgentType>> {
        let durability =
            Durability::<GolemAgentGetAllAgentTypes>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
            let result = self
                .agent_types_service()
                .get_all(
                    self.owned_worker_id.environment_id,
                    self.owned_worker_id.worker_id.component_id,
                    self.state.component_metadata.revision,
                )
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemAgentAgentTypes { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(result) => Ok(result.into_iter().map(|r| r.into()).collect()),
            Err(err) => Err(anyhow!(err)),
        }
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<Option<RegisteredAgentType>> {
        let agent_type_name = AgentTypeName(agent_type_name);
        let durability =
            Durability::<GolemAgentGetAgentType>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
            let component_revision = self.state.component_metadata.revision;
            let result = self
                .agent_types_service()
                .get(
                    self.owned_worker_id.environment_id,
                    self.owned_worker_id.worker_id.component_id,
                    component_revision,
                    &agent_type_name,
                )
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestGolemAgentGetAgentType { agent_type_name },
                    HostResponseGolemAgentAgentType { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(result) => Ok(result.map(|r| r.into())),
            Err(err) => Err(anyhow!(err)),
        }
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: DataValue,
        phantom_id: Option<crate::preview2::golem::rpc::types::Uuid>,
    ) -> anyhow::Result<Result<String, AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "make_agent_id");

        if let Some(agent_type) = self.get_agent_type(agent_type_name.clone()).await? {
            match golem_common::model::agent::DataValue::try_from_bindings(
                input,
                agent_type.agent_type.constructor.input_schema,
            ) {
                Ok(input) => {
                    let agent_id = AgentId::new(
                        AgentTypeName(agent_type_name).to_wit_naming(),
                        input,
                        phantom_id.map(|id| id.into()),
                    );
                    Ok(Ok(agent_id.to_string()))
                }
                Err(err) => Ok(Err(AgentError::InvalidInput(err))),
            }
        } else {
            Ok(Err(AgentError::InvalidType(agent_type_name)))
        }
    }

    async fn parse_agent_id(
        &mut self,
        agent_id: String,
    ) -> anyhow::Result<
        Result<
            (
                String,
                DataValue,
                Option<crate::preview2::golem::rpc::types::Uuid>,
            ),
            AgentError,
        >,
    > {
        DurabilityHost::observe_function_call(self, "golem_agent", "parse_agent_id");

        let component_metadata = &self.component_metadata().metadata;
        match AgentId::parse(agent_id, component_metadata) {
            Ok(agent_id) => Ok(Ok((
                agent_id.agent_type.to_string(),
                agent_id.parameters.into(),
                agent_id.phantom_id.map(|id| id.into()),
            ))),
            Err(error) => Ok(Err(AgentError::InvalidAgentId(error))),
        }
    }

    async fn create_webhook(
        &mut self,
        promise_id: crate::preview2::golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<String> {
        let durability =
            Durability::<GolemAgentCreateWebhook>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        if durability.is_live() {
            let promise_id: PromiseId = promise_id.clone().into();
            if promise_id.worker_id.component_id != self.state.component_metadata.id {
                let error = "Attempted to create a webhook for a promise not created by the current component".to_string();
                let persisted = durability
                    .persist(
                        self,
                        HostRequestGolemApiPromiseId { promise_id },
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
                return persisted.result.map_err(|e| anyhow!(e));
            }

            let Some(agent_id) = self.state.agent_id.as_ref() else {
                let error =
                    "Creating webhook urls is only supported for agentic components".to_string();
                let persisted = durability
                    .persist(
                        self,
                        HostRequestGolemApiPromiseId { promise_id },
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
                return persisted.result.map_err(|e| anyhow!(e));
            };

            let webhook_id = AgentWebhookId {
                worker_name: promise_id.worker_id.worker_name.clone(),
                oplog_idx: promise_id.oplog_idx,
            };

            let webhook_url = self
                .state
                .agent_deployments_service
                .get_agent_webhook_url(
                    self.state.component_metadata.environment_id,
                    &agent_id.agent_type,
                    &webhook_id,
                )
                .await?;

            let Some(webhook_url) = webhook_url else {
                return Err(anyhow!("Agent is not currently deployed as part of an http api. Only deployed agents can create webhook urls"));
            };

            let persisted = durability
                .persist(
                    self,
                    HostRequestGolemApiPromiseId { promise_id },
                    HostResponseGolemAgentWebhookUrl {
                        result: Ok(webhook_url),
                    },
                )
                .await?;

            Ok(persisted.result.map_err(|e| anyhow!(e))?)
        } else {
            Ok(durability
                .replay(self)
                .await?
                .result
                .map_err(|e| anyhow!(e))?)
        }
    }
}
