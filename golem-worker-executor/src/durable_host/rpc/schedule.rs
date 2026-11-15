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

use super::execution::known_fresh_dispatch_allowed;
use super::{
    CancellationTokenEntry, RpcCallTarget, RpcTargetAdmission, WasmRpcEntry, WasmRpcEntryPayload,
    ensure_rpc_target_activated, find_agent_method, invocation_metadata, persist_schedule_denial,
    persist_schedule_denial_from_begun, reject_non_await_scope_card, remote_method_uses_streams,
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
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use futures::future::Either;
use golem_common::base_model::agent::Principal;
use golem_common::model::AgentInvocation;
use golem_common::model::InvocationRequestKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentTarget;
use golem_common::model::agent::InvocationFreshnessDisposition;
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
        let recipient = entry.recipient.clone();
        let invocation_key = golem_common::model::InvocationId::from(uuid::Uuid::from(
            entry.metadata.invocation_id.id,
        ));
        let serialized_schedule_id: SerializableScheduleId = deserialize(&entry.schedule_id)
            .map_err(|err| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Failed to deserialize cancellation token: {err}"
                )))
            })?;

        let mut handle =
            DurableCallSession::<GolemRpcCancellationTokenCancel, NotCancellable>::start_with_agent_authority(
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

            let schedule_id = serialized_schedule_id.into_domain();
            self.rpc()
                .cancel_scheduled_invocation(
                    &recipient,
                    invocation_key,
                    schedule_id,
                    &current_agent.agent_id(),
                    handle.agent_auth_ctx(),
                )
                .await
                .map_err(|error| handle.trap(anyhow::anyhow!(error)))?;
            self.scheduler_service().cancel(schedule_id).await;

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
        let call_target = RpcCallTarget::new(
            begun.idempotency_key_position(self),
            &logical_remote_agent_id,
            ephemeral_logical_agent_id.as_ref(),
            &method_name,
        );
        let call = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let RpcTargetAdmission::LiveOnly(target) = &admission
                    && let Err(error) = self.authorize_live_permission(target).await?
                {
                    return persist_schedule_denial_from_begun(
                        self,
                        begun,
                        logical_remote_agent_id,
                        datetime,
                        method_name,
                        RpcError::Denied(error.to_string()),
                    )
                    .await;
                }
                Either::Left(begun)
            }
            ResolvedCall::Replay(handle) => match handle.replay(self).await? {
                CallReplayOutcome::Replayed(result) => {
                    let (schedule_id, invocation_id) = match result.result {
                        Ok(receipt) => receipt,
                        Err(error) => return Ok(Err(InternalRpcError::from(error).into())),
                    };
                    let remote_agent_id = OwnedAgentId::new(
                        self.owned_agent_id().environment_id,
                        invocation_id.agent_id(),
                    );
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
                            metadata: invocation_metadata(&invocation_id),
                            owner,
                            recipient: remote_agent_id,
                        })
                        .map(Ok)
                        .map_err(Into::into);
                }
                CallReplayOutcome::Incomplete(live) => Either::Right(live),
            },
        };

        // The persisted request embeds an idempotency key derived from the call's own `Start`
        // index. A live call builds its request when that index is assigned; an incomplete
        // `Start` from a previous run keeps its index, so a repair reproduces the same key and
        // `ScheduleId`.
        // Only the first live dispatch may create a fresh ephemeral target; a repair may find it.
        let first_live_dispatch = matches!(call, Either::Left(_));
        let mut handle = match call {
            Either::Left(begun) => {
                let request_target = call_target.clone();
                let request_method_name = method_name.clone();
                let request_input = input_value.clone();
                let request_datetime: golem_common::model::oplog::types::SerializableDateTime =
                    datetime.into();
                begun
                    .start_live_at_index(self, move |start_index| {
                        let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
                        Ok(HostRequestGolemRpcScheduledInvocation {
                            remote_agent_id: remote_agent_id.agent_id(),
                            request_key: InvocationRequestKey::Internal(idempotency_key),
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
        let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
        let request_key = InvocationRequestKey::Internal(idempotency_key.clone());

        let stack = InvocationContextStack::new(
            self.state.invocation_context.trace_id.clone(),
            InvocationContextSpan::external_parent(self.state.current_span_id.clone()),
            self.state.invocation_context.trace_states.clone(),
        );

        let target_fingerprint = if ephemeral_logical_agent_id.is_some() {
            // The scheduled ephemeral target is created now, so the schedule commits in its oplog
            // and the wake pins this incarnation. The target runs nothing before the deadline.
            let freshness = if known_fresh_dispatch_allowed(first_live_dispatch, true) {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
                InvocationFreshnessDisposition::KnownFresh
            } else {
                InvocationFreshnessDisposition::MayExist
            };
            let span_id = self
                .table()
                .get(&this)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .unwrap()
                .span_id
                .clone();
            let rpc = self.rpc();
            let demand = rpc
                .create_demand(
                    AgentTarget::Current(remote_agent_id.agent_id()),
                    remote_agent_id.environment_id,
                    &method_name,
                    self.created_by(),
                    self.agent_id(),
                    &env,
                    self.clone_as_inherited_stack(&span_id),
                    config,
                    handle.agent_auth_ctx(),
                    Some(idempotency_key),
                    freshness,
                )
                .await
                .map_err(|error| handle.trap(anyhow::anyhow!(error)))?;
            demand.fingerprint()
        } else {
            match ensure_rpc_target_activated(self, &this, handle.agent_auth_ctx(), &method_name)
                .await
            {
                Ok(fingerprint) => fingerprint,
                Err(err) => {
                    return Err(handle.trap(err));
                }
            }
        };

        let invocation_id =
            request_key.finalize(remote_agent_id.agent_id(), target_fingerprint, &method_name);
        let invocation = AgentInvocation::AgentMethod {
            idempotency_key: invocation_id.id(),
            method_name,
            input: input_value,
            invocation_context: stack,
            principal: scheduled_invocation_principal(self.owned_agent_id()),
            scope_card: None,
        };
        let result = self
            .rpc()
            .schedule_invocation(
                &remote_agent_id,
                target_fingerprint,
                request_key,
                invocation,
                scheduled_at,
                handle.agent_auth_ctx(),
            )
            .await
            .map_err(|error| handle.trap(anyhow::anyhow!(error)))?;

        let schedule_id = SerializableScheduleId::from_domain(result);

        let result = handle
            .complete(
                self,
                HostResponseGolemRpcScheduledInvocationCompat {
                    result: Ok((schedule_id, invocation_id)),
                },
            )
            .await?;

        let (schedule_id, invocation_id) = result.result.map_err(|error| {
            anyhow::anyhow!("live scheduled invocation returned an unexpected error: {error:?}")
        })?;
        let serialized_result = serialize(&schedule_id).map_err(|err| {
            anyhow::Error::from(WorkerExecutorError::runtime(format!(
                "Failed to serialize schedule id: {err}"
            )))
        })?;
        let cancellation_token = CancellationTokenEntry {
            schedule_id: serialized_result,
            metadata: invocation_metadata(&invocation_id),
            owner: self.owned_agent_id().clone(),
            recipient: remote_agent_id,
        };

        let resource = self.table().push(cancellation_token)?;
        Ok(Ok(resource))
    }
}
