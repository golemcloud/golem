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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::agent::invocation_context::{
    CurrentInvocationId, Host, InvocationCaller,
};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::wit::wire;

/// Both functions read values that the invocation start recorded, so neither is a durable call.
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn invocation_id(&mut self) -> anyhow::Result<Option<CurrentInvocationId>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "invocation_id");
        Ok(self
            .current_invocation_id()
            .map(|invocation_id| CurrentInvocationId {
                owner: wire::InvocationOwner::Agent(invocation_id.agent_id().clone().into()),
                id: invocation_id.uuid().into(),
            }))
    }

    async fn caller(&mut self) -> anyhow::Result<InvocationCaller> {
        DurabilityHost::observe_function_call(self, "golem_agent", "caller");
        let principal = self.invocation_principal();
        Ok(InvocationCaller {
            original: principal.clone().into(),
            immediate: principal.into(),
        })
    }
}
