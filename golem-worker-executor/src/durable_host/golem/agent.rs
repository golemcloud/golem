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
use crate::preview2::golem::agent::host::{Agent, Host, ValueAndType};
use crate::workerctx::{IndexedResourceStore, WorkerCtx};
use golem_common::model::oplog::WorkerResourceId;
use wasmtime::component::Resource;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn register_agent(
        &mut self,
        agent: Resource<Agent>,
        id: String,
        _parameters: Vec<ValueAndType>,
    ) -> anyhow::Result<()> {
        // TODO: log

        // For now, we index agents by their ID and not their constructor parameters
        // let resource_params = parameters
        //     .into_iter()
        //     .map(|v| print_value_and_type(&v.into()))
        //     .collect::<Result<Vec<_>, _>>()?;

        let resource_params = vec![id];
        let resource_id = agent.rep() as u64; // TODO: this needs to be verified whether it's safe to store this ID and can be used to access

        // TODO: need to add to ResourceStore with `add`

        self.store_indexed_resource(
            "golem:agent/guest",
            "agent",
            &resource_params,
            WorkerResourceId(resource_id),
        )
        .await;

        Ok(())
    }

    async fn unregister_agent(
        &mut self,
        _agent: Resource<Agent>,
        id: String,
        _parameters: Vec<ValueAndType>,
    ) -> anyhow::Result<()> {
        // TODO: log

        // For now, we index agents by their ID and not their constructor parameters
        self.drop_indexed_resource("golem:agent/guest", "agent", &[id]);

        // TODO: need to drop from ResourceStore with `add`

        Ok(())
    }
}
