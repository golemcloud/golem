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

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::{AgentStore, WorkerCtx};
use anyhow::anyhow;
use golem_common::model::agent::bindings::golem::agent::host::{DataValue, Host};
use golem_common::model::agent::DataSchema;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn register_agent(
        &mut self,
        agent_type: String,
        agent_id: String,
        parameters: DataValue,
    ) -> anyhow::Result<()> {
        let agent = self
            .component_metadata()
            .metadata
            .find_agent_type(&agent_type)
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or_else(|| anyhow!("Unknown agent type: {}", agent_type))?;
        let parameters = get_data_value(parameters, agent.constructor.input_schema)?;
        self.store_agent_instance(agent_type, agent_id, parameters)
            .await;

        Ok(())
    }

    async fn unregister_agent(
        &mut self,
        agent_type: String,
        agent_id: String,
        parameters: DataValue,
    ) -> anyhow::Result<()> {
        let agent = self
            .component_metadata()
            .metadata
            .find_agent_type(&agent_type)
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or_else(|| anyhow!("Unknown agent type: {}", agent_type))?;
        let parameters = get_data_value(parameters, agent.constructor.input_schema)?;
        self.remove_agent_instance(agent_type, agent_id, parameters)
            .await;

        Ok(())
    }
}

fn get_data_value(
    value: DataValue,
    schema: DataSchema,
) -> anyhow::Result<golem_common::model::agent::DataValue> {
    let parameters = golem_common::model::agent::DataValue::try_from_bindings(value, schema.into())
        .map_err(|err| anyhow!(err))?;

    Ok(parameters)
}
