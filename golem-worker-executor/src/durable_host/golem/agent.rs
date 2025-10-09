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

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::bindings::golem::agent::common::AgentError;
use golem_common::model::agent::bindings::golem::agent::host;
use golem_common::model::agent::bindings::golem::agent::host::{DataValue, Host};
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{AgentId, RegisteredAgentType};
use golem_common::model::oplog::DurableFunctionType;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_agent_types(&mut self) -> anyhow::Result<Vec<host::RegisteredAgentType>> {
        let durability = Durability::<Vec<RegisteredAgentType>, SerializableError>::new(
            self,
            "golem_agent",
            "get_all_agent_types",
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let project_id = &self.owned_worker_id.project_id;
            let result = self.agent_types_service().get_all(project_id).await;
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    (),
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(result) => Ok(result.into_iter().map(|r| r.into()).collect()),
            Err(err) => Err(err.into()),
        }
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<Option<host::RegisteredAgentType>> {
        let durability = Durability::<Option<RegisteredAgentType>, SerializableError>::new(
            self,
            "golem_agent",
            "get_agent_type",
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let project_id = &self.owned_worker_id.project_id;
            let result = self.agent_types_service()
                .get(project_id, &agent_type_name)
                .await;
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    agent_type_name.clone(),
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(result) => Ok(result.map(|r| r.into())),
            Err(err) => Err(err.into()),
        }
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: DataValue,
    ) -> anyhow::Result<Result<String, AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "make_agent_id");

        if let Some(agent_type) = self.get_agent_type(agent_type_name.clone()).await? {
            match golem_common::model::agent::DataValue::try_from_bindings(
                input,
                agent_type.agent_type.constructor.input_schema,
            ) {
                Ok(input) => {
                    let agent_id = AgentId::new(agent_type_name.to_wit_naming(), input);
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
    ) -> anyhow::Result<Result<(String, DataValue), AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "parse_agent_id");

        let component_metadata = &self.component_metadata().metadata;
        match AgentId::parse(agent_id, component_metadata) {
            Ok(agent_id) => Ok(Ok((agent_id.agent_type, agent_id.parameters.into()))),
            Err(error) => Ok(Err(AgentError::InvalidAgentId(error))),
        }
    }
}
