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
    CancellationTokenEntry, RpcTargetAdmission, WasmRpcEntry, WasmRpcEntryPayload,
    ensure_rpc_target_activated, find_agent_method, invocation_metadata,
    invocation_target_agent_id, persist_schedule_denial, persist_schedule_denial_from_begun,
    reject_non_await_scope_card, remote_method_uses_streams,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::preview2::golem::agent::host::CancellationToken;
use crate::preview2::golem::agent::host::HostCancellationToken;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use futures::future::Either;
use golem_common::base_model::agent::Principal;
use golem_common::model::AgentInvocation;
use golem_common::model::OwnedAgentId;
use golem_common::model::ScheduleId;
use golem_common::model::ScheduledAction;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemRpcScheduledInvocation;
use golem_common::model::oplog::HostRequestGolemRpcScheduledInvocationCancellation;
use golem_common::model::oplog::HostResponseGolemRpcScheduledInvocationCompat;
use golem_common::model::oplog::HostResponseGolemRpcUnit;
use golem_common::model::oplog::host_functions::GolemRpcCancellationTokenCancel;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcScheduleInvocation;
use golem_common::model::oplog::types::SerializableScheduleId;
use golem_common::serialization::deserialize;
use golem_common::serialization::serialize;
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::decode_value_with;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::Resource;

fn scheduled_invocation_principal(owner: &OwnedAgentId) -> Principal {
    Principal::Agent(golem_common::model::agent::AgentPrincipal {
        agent_id: owner.agent_id(),
    })
}

pub(super) fn validate_cancellation_token_owner(
    token: &CancellationTokenEntry,
    current_agent: &OwnedAgentId,
) -> Result<(), WorkerExecutorError> {
    if token.owner != *current_agent {
        return Err(WorkerExecutorError::permission_denied(format!(
            "scheduled invocation owned by {} cannot be cancelled by {current_agent}",
            token.owner
        )));
    }
    if token.metadata.agent_id != token.recipient.agent_id.agent_id {
        return Err(WorkerExecutorError::runtime(
            "scheduled invocation cancellation token recipient does not match its metadata",
        ));
    }
    Ok(())
}

impl<Ctx: WorkerCtx> HostCancellationToken for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::cancellation-token::cancel")
            .map_err(wasmtime::Error::from)?;

        let current_agent = self.owned_agent_id().clone();
        let entry = self.table().get(&this)?;
        validate_cancellation_token_owner(entry, &current_agent)?;
        let serialized_schedule_id: SerializableScheduleId = deserialize(&entry.schedule_id)
            .map_err(|err| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Failed to deserialize cancellation token: {err}"
                )))
            })?;

        let mut handle =
            DurableCallSession::<GolemRpcCancellationTokenCancel, NotCancellable>::start(
                self,
                HostRequestGolemRpcScheduledInvocationCancellation {
                    schedule_id: serialized_schedule_id.clone(),
                },
                DurableFunctionType::WriteRemote,
            )
            .await?;

        'cancel: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(_) => break 'cancel,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            self.scheduler_service()
                .cancel(serialized_schedule_id.into_domain())
                .await;

            handle.complete(self, HostResponseGolemRpcUnit {}).await?;
        }

        Ok(())
    }

    async fn drop(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::cancellation-token", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(super) async fn schedule_cancelable_invocation_impl(
        &mut self,
        this: Resource<WasmRpcEntry>,
        datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
        scope_card: Option<Resource<PermissionCardHandleRep>>,
        function_name: &'static str,
    ) -> anyhow::Result<Result<Resource<CancellationToken>, RpcError>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::schedule-cancelable-invocation")
            .map_err(wasmtime::Error::from)?;

        // Deterministic local validation must happen before opening
        // durability so a guest bug (unknown method, input incompatible
        // with the declared schema, or invalid datetime) does not leave
        // an open durable function. `schedule_cancelable_invocation`
        // has no `RpcError` return channel, so these are surfaced as
        // wasmtime traps.
        let (
            logical_remote_agent_id,
            ephemeral_logical_agent_id,
            remote_agent_type,
            remote_method_streams,
            remote_component_revision,
            env,
            config,
        ) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let (env, config) = payload.target_activation.target_creation_data();
            (
                payload.remote_agent_id.clone(),
                payload.ephemeral_logical_agent_id.clone(),
                payload.remote_agent_type.clone(),
                payload.remote_method_streams.clone(),
                payload.remote_component_revision,
                env,
                config,
            )
        };

        let input = match reject_non_await_scope_card(&scope_card, input, function_name, self) {
            Ok(input) => input,
            Err(details) => {
                return persist_schedule_denial(
                    self,
                    logical_remote_agent_id,
                    datetime,
                    method_name,
                    RpcError::Denied(details),
                )
                .await;
            }
        };

        // Lift the input first, then validate the method exists. Lifting
        // consumes any owned `quota-token` handle the guest passed (releasing it
        // from the resource table into a trusted snapshot via the
        // `QuotaTokenResolver`), so it cannot leak if the method check fails. The
        // input then travels as a schema-free `SchemaValue`; the callee
        // validates it against its own schema when it lowers the scheduled
        // invocation.
        let input_value = decode_value_with(input, self)
            .map_err(|err| anyhow::anyhow!("Invalid RPC input: {err}"))?;
        let method = find_agent_method(&remote_agent_type, &method_name)?;
        method
            .validate_input(&remote_agent_type.schema, &input_value)
            .map_err(|error| anyhow::anyhow!("Invalid RPC input: {error}"))?;
        if remote_method_uses_streams(&remote_method_streams, &method.name)? {
            return Err(anyhow::anyhow!(
                "live streams cannot be used in scheduled invocations"
            ));
        }
        let scheduled_at = chrono::DateTime::from_timestamp(datetime.seconds, datetime.nanoseconds)
            .ok_or_else(|| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Received invalid datetime from wasi: seconds={}, nanoseconds={}",
                    datetime.seconds, datetime.nanoseconds
                )))
            })?;

        let admission = match self
            .authorize_and_record_rpc_target_activation(
                &this,
                &logical_remote_agent_id,
                &method_name,
            )
            .await?
        {
            Ok(admission) => admission,
            Err(denial) => {
                if denial.activation_decision.is_some() {
                    return Ok(Err(denial.error));
                }
                return persist_schedule_denial(
                    self,
                    logical_remote_agent_id,
                    datetime,
                    method_name,
                    denial.error,
                )
                .await;
            }
        };

        let begun = DurableCallSession::<GolemRpcWasmRpcScheduleInvocation, NotCancellable>::
            begin_with_agent_authority(self, DurableFunctionType::WriteRemote)
            .await?;
        let key_position = self.idempotency_key_position();
        let call = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let RpcTargetAdmission::LiveOnly(target) = &admission
                    && let Err(error) = self.authorize_live_permission(target).await?
                {
                    return persist_schedule_denial_from_begun(
                        self,
                        begun,
                        key_position,
                        logical_remote_agent_id,
                        datetime,
                        method_name,
                        RpcError::Denied(error.to_string()),
                    )
                    .await;
                }
                Either::Left(begun)
            }
            ResolvedCall::Replay(handle) => {
                let start_index = handle.start_index();
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(result) => {
                        let schedule_id = match result.result {
                            Ok(schedule_id) => schedule_id,
                            Err(error) => return Ok(Err(InternalRpcError::from(error).into())),
                        };
                        let idempotency_key = key_position.derive(start_index);
                        let remote_agent_id = invocation_target_agent_id(
                            &logical_remote_agent_id,
                            ephemeral_logical_agent_id.as_ref(),
                            &idempotency_key,
                        )?;
                        let schedule_id = serialize(&schedule_id).map_err(|err| {
                            anyhow::Error::from(WorkerExecutorError::runtime(format!(
                                "Failed to serialize schedule id: {err}"
                            )))
                        })?;
                        let owner = self.owned_agent_id().clone();
                        return self
                            .table()
                            .push(CancellationTokenEntry {
                                schedule_id,
                                metadata: invocation_metadata(&remote_agent_id, &idempotency_key),
                                owner,
                                recipient: remote_agent_id,
                            })
                            .map(Ok)
                            .map_err(Into::into);
                    }
                    CallReplayOutcome::Incomplete(live) => Either::Right(live),
                }
            }
        };

        // The persisted request embeds an idempotency key derived from the call's own `Start`
        // index. A live call builds its request when that index is assigned; an incomplete
        // `Start` from a previous run keeps its index, so a repair reproduces the same key and
        // `ScheduleId`.
        let mut handle = match call {
            Either::Left(begun) => {
                let request_key_position = key_position.clone();
                let request_logical_remote_agent_id = logical_remote_agent_id.clone();
                let request_ephemeral_logical_agent_id = ephemeral_logical_agent_id.clone();
                let request_method_name = method_name.clone();
                let request_input = input_value.clone();
                let request_datetime: golem_common::model::oplog::types::SerializableDateTime =
                    datetime.into();
                begun
                    .start_live_at_index(self, move |start_index| {
                        let idempotency_key = request_key_position.derive(start_index);
                        let remote_agent_id = invocation_target_agent_id(
                            &request_logical_remote_agent_id,
                            request_ephemeral_logical_agent_id.as_ref(),
                            &idempotency_key,
                        )
                        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                        Ok(HostRequestGolemRpcScheduledInvocation {
                            remote_agent_id: remote_agent_id.agent_id(),
                            idempotency_key,
                            method_name: request_method_name,
                            input: request_input,
                            datetime: request_datetime,
                            remote_agent_type: None,
                            remote_agent_parameters: None,
                        })
                    })
                    .await?
            }
            Either::Right(live) => live,
        };
        let idempotency_key = key_position.derive(handle.start_index());
        let remote_agent_id = invocation_target_agent_id(
            &logical_remote_agent_id,
            ephemeral_logical_agent_id.as_ref(),
            &idempotency_key,
        )?;
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);

        let schedule_id = ScheduleId::from_idempotency_key(&idempotency_key);

        let stack = InvocationContextStack::new(
            self.state.invocation_context.trace_id.clone(),
            InvocationContextSpan::external_parent(self.state.current_span_id.clone()),
            self.state.invocation_context.trace_states.clone(),
        );

        let invocation = Box::new(AgentInvocation::AgentMethod {
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value,
            invocation_context: stack,
            principal: scheduled_invocation_principal(self.owned_agent_id()),
            scope_card: None,
        });
        let action = if ephemeral_logical_agent_id.is_some() {
            ScheduledAction::InvokeEphemeral {
                account_id: self.created_by(),
                owned_agent_id: remote_agent_id.clone(),
                invocation,
                component_revision: remote_component_revision,
                env,
                config,
                parent: Some(self.agent_id().clone()),
                creation_principal: Box::new(Principal::Agent(
                    golem_common::model::agent::AgentPrincipal {
                        agent_id: self.agent_id().clone(),
                    },
                )),
            }
        } else {
            let target_worker_fingerprint = match ensure_rpc_target_activated(
                self,
                &this,
                handle.agent_auth_ctx(),
                &method_name,
            )
            .await
            {
                Ok(fingerprint) => fingerprint,
                Err(err) => {
                    return Err(handle.trap(err));
                }
            };
            ScheduledAction::Invoke {
                account_id: self.created_by(),
                owned_agent_id: remote_agent_id.clone(),
                invocation,
                target_worker_fingerprint,
            }
        };

        let result = self
            .state
            .scheduler_service
            .schedule_with_id(schedule_id, scheduled_at, action)
            .await;

        let schedule_id = SerializableScheduleId::from_domain(result);

        let result = handle
            .complete(
                self,
                HostResponseGolemRpcScheduledInvocationCompat {
                    result: Ok(schedule_id),
                },
            )
            .await?;

        let schedule_id = result.result.map_err(|error| {
            anyhow::anyhow!("live scheduled invocation returned an unexpected error: {error:?}")
        })?;
        let serialized_result = serialize(&schedule_id).map_err(|err| {
            anyhow::Error::from(WorkerExecutorError::runtime(format!(
                "Failed to serialize schedule id: {err}"
            )))
        })?;
        let cancellation_token = CancellationTokenEntry {
            schedule_id: serialized_result,
            metadata,
            owner: self.owned_agent_id().clone(),
            recipient: remote_agent_id,
        };

        let resource = self.table().push(cancellation_token)?;
        Ok(Ok(resource))
    }
}
