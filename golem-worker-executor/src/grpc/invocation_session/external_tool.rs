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

use super::*;
use crate::durable_host::schema_value_stream::contains_stream;
use crate::model::LookupResult;
use crate::services::{HasActiveAgents, HasEnvironmentStateService};
use crate::worker::ResultOrSubscription;
use golem_common::model::agent::OwnerKind;
use golem_common::schema::TypedSchemaValue;

impl<Ctx, Svcs> WorkerExecutorImpl<Ctx, Svcs>
where
    Ctx: WorkerCtx,
    Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static,
{
    pub(super) async fn invoke_external_tool_internal(
        &self,
        request: &InvocationStart,
        acceptance_committed: tokio::sync::oneshot::Sender<()>,
        accepted: tokio::sync::oneshot::Sender<AcceptedInvocation>,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        use golem_api_grpc::proto::golem::worker::AgentInvocationMode as Mode;

        let tool = request.external_tool.as_ref().expect("native tool request");
        let key: IdempotencyKey = request
            .idempotency_key
            .clone()
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "external tool invocation requires an idempotency key",
                )
            })?
            .into();
        let owned = extract_owned_agent_id(request, |r| &r.agent_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned.agent_id)?;
        if tool.fresh_owner {
            if owned.agent_id.agent_id != OwnerKind::external_tool_instance_name(&key) {
                return Err(WorkerExecutorError::invalid_request(
                    "virtual owner identity does not match the invocation key",
                ));
            }
        } else if OwnerKind::is_reserved_instance_name(&owned.agent_id.agent_id) {
            return Err(WorkerExecutorError::invalid_request(
                "existing tool target must be a real agent",
            ));
        }
        let principal = request
            .principal
            .clone()
            .map(TryInto::try_into)
            .transpose()
            .map_err(WorkerExecutorError::invalid_request)?
            .unwrap_or_else(Principal::anonymous);
        let context = self
            .limit_invocation_context_stack_depth(from_proto_invocation_context(&request.context));
        let scope_card: Option<ScopeCard> = request
            .scope_card
            .clone()
            .map(TryInto::try_into)
            .transpose()
            .map_err(WorkerExecutorError::permission_denied)?;
        let mode = request.mode();
        if scope_card.is_some() && mode != Mode::Await {
            return Err(WorkerExecutorError::permission_denied(
                "scope cards are supported only for invoke-and-await",
            ));
        }
        let mut response = AgentInvocationOutput {
            result: AgentInvocationResult::AgentInitialization,
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: Some(owned.agent_id.clone()),
            idempotency_key: Some(key.clone()),
            oplog_index: None,
            agent_fingerprint: None,
        };
        if mode == Mode::Lookup {
            let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned).await?;
            let Some(metadata) = metadata else {
                response.invocation_status = Some(InvocationStatus::Unknown);
                publish_acceptance(acceptance_committed, accepted, None)?;
                return Ok(response);
            };
            if !tool.fresh_owner || request.expected_callee_fingerprint.is_some() {
                require_expected_callee_fingerprint(
                    request.expected_callee_fingerprint,
                    metadata.fingerprint,
                )?;
            }
            let worker =
                Worker::get_exact_existing_suspended(self, &owned, &context, principal).await?;
            response.component_revision = Some(metadata.last_known_status.component_revision);
            response.agent_fingerprint = Some(metadata.fingerprint);
            match worker.lookup_invocation_result(&key).await {
                LookupResult::Complete(Ok(mut output)) => {
                    output.agent_id = response.agent_id;
                    output.idempotency_key = response.idempotency_key;
                    output.invocation_status = Some(InvocationStatus::Complete);
                    response = output;
                }
                LookupResult::Complete(Err(error)) => return Err(error),
                LookupResult::Pending => {
                    response.invocation_status = Some(InvocationStatus::Pending)
                }
                LookupResult::New | LookupResult::Interrupted => {
                    response.invocation_status = Some(InvocationStatus::Unknown)
                }
            }
            publish_acceptance(acceptance_committed, accepted, response.component_revision)?;
            return Ok(response);
        }
        let raw = tool.input.clone().ok_or_else(|| {
            WorkerExecutorError::invalid_request("external tool input is required")
        })?;
        let encoded_len = raw.encoded_len();
        let graph: SchemaGraph = raw
            .graph
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request("external tool input schema is required")
            })?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let value = decode_invocation_input(raw.value.ok_or_else(|| {
            WorkerExecutorError::invalid_request("external tool input value is required")
        })?)
        .map_err(WorkerExecutorError::invalid_request)?;
        golem_common::schema::validation::validate_value(&graph, &graph.root, &value).map_err(
            |errors| {
                WorkerExecutorError::invalid_request(
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            },
        )?;
        if contains_stream(&value) {
            return Err(WorkerExecutorError::invalid_request(
                "typed tool input cannot contain streams; use the stdin attachment",
            ));
        }
        let streaming = tool.stdin || tool.stdout;
        if streaming && (mode != Mode::Await || request.schedule_at.is_some()) {
            return Err(WorkerExecutorError::invalid_request(
                "live streams require an attached Await invocation session",
            ));
        }
        if tool.fresh_owner
            && mode == Mode::Schedule
            && let Some(timestamp) = request.schedule_at.as_ref()
        {
            let component = self
                .component_service()
                .get_metadata(owned.component_id(), None)
                .await?;
            if component.environment_id != owned.environment_id {
                return Err(WorkerExecutorError::invalid_request(
                    "external tool owner environment does not match the component environment",
                ));
            }
            let tool_name = tool
                .tool_name
                .clone()
                .try_into()
                .map_err(WorkerExecutorError::invalid_request)?;
            let owner = golem_common::model::tool::ToolBindingOwner::ComponentBaseline {
                component_id: component.id,
            };
            let activation = match self
                .environment_state_service()
                .get_tool_activation(
                    owned.environment_id,
                    component.id,
                    component.revision,
                    &owner,
                    &tool_name,
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            {
                crate::services::environment_state::ToolActivationOutcome::Ready(activation) => {
                    activation
                }
                crate::services::environment_state::ToolActivationOutcome::NotBound => {
                    return Err(WorkerExecutorError::permission_denied(
                        "tool is not bound to component owner",
                    ));
                }
                crate::services::environment_state::ToolActivationOutcome::NotRegistered => {
                    return Err(WorkerExecutorError::invalid_request(
                        "tool is not registered",
                    ));
                }
            };
            crate::worker::require_expected_tool_deployment_revision(
                tool.expected_deployment_revision
                    .map(golem_common::model::deployment::DeploymentRevision::try_from)
                    .transpose()
                    .map_err(WorkerExecutorError::invalid_request)?,
                activation.registered_tool.deployment_revision,
            )?;
            let at = DateTime::from_timestamp(
                timestamp.seconds,
                timestamp.nanos.try_into().map_err(|_| {
                    WorkerExecutorError::invalid_request("invalid schedule timestamp")
                })?,
            )
            .ok_or_else(|| WorkerExecutorError::invalid_request("invalid schedule timestamp"))?;
            let account_id = request
                .component_owner_account_id
                .ok_or_else(|| WorkerExecutorError::invalid_request("account_id not found"))?
                .try_into()
                .map_err(WorkerExecutorError::invalid_request)?;
            let invocation = AgentInvocation::ExternalTool {
                idempotency_key: key,
                tool_name,
                command_path: tool.command_path.clone(),
                input: TypedSchemaValue::new(graph, value),
                stdin: false,
                stdout: false,
                activation,
                invocation_context: context,
                principal: principal.clone(),
                scope_card,
            };
            self.scheduler_service()
                .schedule(
                    at,
                    ScheduledAction::InvokeEphemeral {
                        account_id,
                        owned_agent_id: owned,
                        invocation: Box::new(invocation),
                        component_revision: component.revision,
                        env: Vec::new(),
                        config: Vec::new(),
                        parent: request.parent(),
                        creation_principal: Box::new(principal),
                    },
                )
                .await;
            response.component_revision = Some(component.revision);
            publish_acceptance(acceptance_committed, accepted, response.component_revision)?;
            return Ok(response);
        }
        let worker = if tool.fresh_owner {
            self.active_agents()
                .get_or_add_ephemeral_external_tool(
                    self,
                    owned.component_id(),
                    owned.environment_id,
                    &key,
                    &context,
                    principal.clone(),
                )
                .await?
        } else {
            let worker =
                Worker::get_exact_existing_suspended(self, &owned, &context, principal.clone())
                    .await?;
            require_expected_callee_fingerprint(
                request.expected_callee_fingerprint,
                worker.get_initial_worker_metadata().fingerprint,
            )?;
            worker
        };
        let metadata = worker.get_latest_worker_metadata().await;
        response.component_revision = Some(metadata.last_known_status.component_revision);
        response.agent_fingerprint = Some(metadata.fingerprint);
        if streaming && let Some((prepared, _)) = worker.native_tool_session(&key).await? {
            response.component_revision =
                Some(prepared.attempt.invocation.target_component_revision);
        }
        let expected_deployment_revision = tool
            .expected_deployment_revision
            .map(golem_common::model::deployment::DeploymentRevision::try_from)
            .transpose()
            .map_err(WorkerExecutorError::invalid_request)?;
        let prepared = if expected_deployment_revision.is_some() {
            Some(
                worker
                    .prepare_external_tool_invocation(
                        key.clone(),
                        tool.tool_name
                            .clone()
                            .try_into()
                            .map_err(WorkerExecutorError::invalid_request)?,
                        tool.command_path.clone(),
                        TypedSchemaValue::new(graph.clone(), value.clone()),
                        tool.stdin,
                        tool.stdout,
                        expected_deployment_revision,
                        context.clone(),
                        principal.clone(),
                        scope_card.clone(),
                    )
                    .await?,
            )
        } else {
            None
        };
        if !streaming && worker.lookup_invocation_result(&key).await != LookupResult::New {
            publish_acceptance(acceptance_committed, accepted, response.component_revision)?;
            if mode == Mode::Await {
                let mut output = worker.await_enqueued_invocation(key.clone()).await?;
                output.agent_id = response.agent_id;
                output.idempotency_key = Some(key);
                return Ok(output);
            }
            return Ok(response);
        }
        let invocation = if let Some(prepared) = prepared {
            prepared
        } else {
            worker
                .prepare_external_tool_invocation(
                    key.clone(),
                    tool.tool_name
                        .clone()
                        .try_into()
                        .map_err(WorkerExecutorError::invalid_request)?,
                    tool.command_path.clone(),
                    TypedSchemaValue::new(graph, value),
                    tool.stdin,
                    tool.stdout,
                    None,
                    context,
                    principal,
                    scope_card,
                )
                .await?
        };
        if streaming {
            let component = self
                .component_service()
                .get_metadata(owned.component_id(), response.component_revision)
                .await?;
            let mut pinned_request = request.clone();
            pinned_request.expected_callee_fingerprint = Some(metadata.fingerprint.0.into());
            let durable_request = build_durable_streaming_request(
                &pinned_request,
                &component.metadata,
                component.revision,
                metadata.fingerprint,
                invocation,
                encoded_len,
                acceptance_committed,
                self.services
                    .config()
                    .limits
                    .live_stream_event_broadcast_capacity
                    .get(),
            )?;
            let acceptance = worker
                .accept_durable_streaming_invocation(durable_request)
                .await?;
            accepted
                .send(AcceptedInvocation {
                    component_revision: response.component_revision,
                    durable_streams: Some(acceptance.streams),
                    prepared: Some(acceptance.prepared),
                    durable_replayed: acceptance.replayed,
                })
                .map_err(|_| {
                    WorkerExecutorError::runtime(
                        "invocation session ended before durable acceptance",
                    )
                })?;
            Worker::start_if_needed(worker.clone()).await?;
        } else if mode == Mode::Schedule && request.schedule_at.is_some() {
            let timestamp = request.schedule_at.as_ref().unwrap();
            let at = DateTime::from_timestamp(
                timestamp.seconds,
                timestamp.nanos.try_into().map_err(|_| {
                    WorkerExecutorError::invalid_request("invalid schedule timestamp")
                })?,
            )
            .ok_or_else(|| WorkerExecutorError::invalid_request("invalid schedule timestamp"))?;
            let account_id = request
                .component_owner_account_id
                .ok_or_else(|| WorkerExecutorError::invalid_request("account_id not found"))?
                .try_into()
                .map_err(WorkerExecutorError::invalid_request)?;
            self.scheduler_service()
                .schedule(
                    at,
                    ScheduledAction::Invoke {
                        account_id,
                        owned_agent_id: owned,
                        invocation: Box::new(invocation),
                        target_worker_fingerprint: metadata.fingerprint,
                    },
                )
                .await;
            publish_acceptance(acceptance_committed, accepted, response.component_revision)?;
            return Ok(response);
        } else {
            let result = worker.clone().invoke(invocation).await?;
            if let ResultOrSubscription::Finished(Err(error)) = result {
                return Err(error);
            }
            publish_acceptance(acceptance_committed, accepted, response.component_revision)?;
            Worker::start_if_needed(worker.clone()).await?;
            if mode == Mode::Schedule {
                return Ok(response);
            }
        }
        let mut output = worker.await_enqueued_invocation(key.clone()).await?;
        output.agent_id = response.agent_id;
        output.idempotency_key = Some(key);
        Ok(output)
    }
}
