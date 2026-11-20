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

use super::{
    AGENT_OPERATION_PERMISSION_DENIED, agent_operation_denial, agent_operation_error,
    component_owner,
};
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
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::model::card::AgentResourcePattern;
use golem_common::model::card::AgentVerb;
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemApiGetAgents;
use golem_common::model::oplog::HostResponseGolemApiAgents;
use golem_common::model::oplog::host_functions::GolemApiGetAgents;
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

/// Whether the caller may not view the agents of the agent type.
async fn agent_type_view_denied<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    registered: &RegisteredAgentType,
) -> Result<bool, WorkerExecutorError> {
    let component = component_owner(ctx, registered.implemented_by.component_id).await?;
    let owner = AgentOwnerPattern::Agent {
        account: component.account_email,
        application: component.application_name,
        environment: component.environment_name,
        component: component.component_name,
        agent: AgentOwnerLeafPattern::AgentTypeWildcard(registered.agent_type.type_name.clone()),
    };
    let target = agent_worker_target(owner, AgentVerb::View, AgentResourcePattern::Empty);
    Ok(ctx.authorize_live_permission(&target).await?.is_err())
}

/// The deployed agent type of the name in the caller's environment.
async fn deployed_agent_type<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    agent_type_name: &AgentTypeName,
) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
    let owner = ctx.owner_component_metadata();
    ctx.state
        .agent_types_service
        .get(
            ctx.owned_agent_id.environment_id,
            owner.id,
            owner.revision,
            agent_type_name,
        )
        .await
}

impl<Ctx: WorkerCtx> HostGetAgents for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_type_name: String,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetAgents>> {
        self.observe_function_call("golem::api::get-workers", "new");
        let entry = GetAgentsEntry::new(
            AgentTypeName(agent_type_name),
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
        let (agent_type_name, filter, count, precise, cursor) = self
            .as_wasi_view()
            .table()
            .get::<GetAgentsEntry>(&self_)
            .map(|e| {
                (
                    e.agent_type_name.clone(),
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
                let request = HostRequestGolemApiGetAgents {
                    agent_type_name: agent_type_name.clone(),
                };
                let (handle, resolved) = match begun.resolve(self).await? {
                    ResolvedCall::Live(begun) => {
                        let resolved = deployed_agent_type(self, &agent_type_name).await?;
                        if let Some(registered) = &resolved {
                            let denied = agent_type_view_denied(self, registered).await?;
                            if let Some(denial) = agent_operation_denial(denied) {
                                break 'result begun
                                    .start_live_denied(self, request, denial)
                                    .await?;
                            }
                        }
                        (begun.start_live(self, request).await?, Some(resolved))
                    }
                    ResolvedCall::Replay(handle) => (handle, None),
                };
                handle
                    .run(self, async |ctx| {
                        let authorized = resolved.is_some();
                        let resolved = match resolved {
                            Some(resolved) => resolved,
                            None => deployed_agent_type(ctx, &agent_type_name).await?,
                        };
                        let registered = match resolved {
                            Some(registered) if authorized => registered,
                            Some(registered) => {
                                // A repaired call resolves the type only now, so the check
                                // that a live call runs before its Start runs here.
                                if agent_type_view_denied(ctx, &registered).await? {
                                    return Ok(HostResponseGolemApiAgents {
                                        result: Err(AGENT_OPERATION_PERMISSION_DENIED.to_string()),
                                    });
                                }
                                registered
                            }
                            None => {
                                return Ok::<_, anyhow::Error>(HostResponseGolemApiAgents {
                                    result: Err(format!(
                                        "Agent type {agent_type_name} is not deployed"
                                    )),
                                });
                            }
                        };
                        let result = ctx
                            .state
                            .get_workers(registered.type_id, filter, cursor, count, precise)
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
    agent_type_name: AgentTypeName,
    filter: Option<golem_common::model::AgentFilter>,
    precise: bool,
    count: u64,
    next_cursor: Option<ScanCursor>,
}

impl GetAgentsEntry {
    pub fn new(
        agent_type_name: AgentTypeName,
        filter: Option<golem_common::model::AgentFilter>,
        precise: bool,
    ) -> Self {
        Self {
            agent_type_name,
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
