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

use super::{agent_operation_denial, agent_operation_error, resolve_agent_owner};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::agent_worker_target;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::AgentAnyFilter;
use crate::preview2::golem_api_1_x::host::GetAgents;
use crate::preview2::golem_api_1_x::host::HostGetAgents;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::ScanCursor;
use golem_common::model::card::AgentResourcePattern;
use golem_common::model::card::AgentVerb;
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemApiGetAgents;
use golem_common::model::oplog::HostResponseGolemApiAgents;
use golem_common::model::oplog::host_functions::GolemApiGetAgents;
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

async fn component_agents_view_denied<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    component_id: &ComponentId,
) -> Result<bool, WorkerExecutorError> {
    let (owner, _) = resolve_agent_owner(ctx, component_id, None).await?;
    let target = agent_worker_target(owner, AgentVerb::View, AgentResourcePattern::Empty);
    Ok(ctx.authorize_live_permission(&target).await?.is_err())
}

impl<Ctx: WorkerCtx> HostGetAgents for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: golem_api_1_x::host::ComponentId,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetAgents>> {
        self.observe_function_call("golem::api::get-workers", "new");
        let entry = GetAgentsEntry::new(
            component_id.into(),
            filter
                .map(|f| f.try_into())
                .transpose()
                .map_err(|e: String| anyhow!(e))?,
            precise,
        );
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetAgents>,
    ) -> anyhow::Result<
        Result<
            Option<Vec<golem_api_1_x::host::AgentMetadata>>,
            golem_api_1_x::host::AgentOperationError,
        >,
    > {
        self.observe_function_call("golem::api::get-workers", "get-next");
        let (component_id, filter, count, precise, cursor) = self
            .as_wasi_view()
            .table()
            .get::<GetAgentsEntry>(&self_)
            .map(|e| {
                (
                    e.component_id,
                    e.filter.clone(),
                    e.count,
                    e.precise,
                    e.next_cursor.clone(),
                )
            })?;

        if let Some(cursor) = cursor {
            let begun = DurableCallSession::<GolemApiGetAgents, NotCancellable>::begin(
                self,
                DurableFunctionType::ReadRemote,
            )
            .await?;
            let result = 'result: {
                let handle = match begun.resolve(self).await? {
                    ResolvedCall::Live(begun) => {
                        let denied = component_agents_view_denied(self, &component_id).await?;
                        let request = HostRequestGolemApiGetAgents { component_id };
                        if let Some(denial) = agent_operation_denial(denied) {
                            break 'result begun.start_live_denied(self, request, denial).await?;
                        }
                        begun.start_live(self, request).await?
                    }
                    ResolvedCall::Replay(handle) => handle,
                };
                handle
                    .run(self, async |ctx| {
                        let result = ctx
                            .state
                            .get_workers(&component_id, filter, cursor, count, precise)
                            .await
                            .map(|(new_cursor, workers)| {
                                (
                                    new_cursor,
                                    workers
                                        .into_iter()
                                        .map(AgentMetadataForGuests::from)
                                        .collect(),
                                )
                            })
                            .map_err(|error| error.to_string());
                        Ok::<_, anyhow::Error>(HostResponseGolemApiAgents { result })
                    })
                    .await?
            };
            let (new_cursor, workers) = match result.result {
                Ok((cursor, workers)) => (cursor, workers),
                Err(error) => return Ok(Err(agent_operation_error(error))),
            };

            self.as_wasi_view()
                .table()
                .get_mut::<GetAgentsEntry>(&self_)
                .map(|e| e.set_next_cursor(new_cursor))?;

            Ok(Ok(Some(workers.into_iter().map(Into::into).collect())))
        } else {
            Ok(Ok(None))
        }
    }

    async fn drop(&mut self, rep: Resource<GetAgents>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::get-workers", "drop");
        self.as_wasi_view().table().delete::<GetAgentsEntry>(rep)?;
        Ok(())
    }
}

pub struct GetAgentsEntry {
    component_id: ComponentId,
    filter: Option<golem_common::model::AgentFilter>,
    precise: bool,
    count: u64,
    next_cursor: Option<ScanCursor>,
}

impl GetAgentsEntry {
    pub fn new(
        component_id: ComponentId,
        filter: Option<golem_common::model::AgentFilter>,
        precise: bool,
    ) -> Self {
        Self {
            component_id,
            filter,
            precise,
            count: 50,
            next_cursor: Some(ScanCursor::default()),
        }
    }

    fn set_next_cursor(&mut self, cursor: Option<ScanCursor>) {
        self.next_cursor = cursor;
    }
}
