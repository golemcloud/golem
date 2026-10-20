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

use super::{PreparedRpcInvocation, WasmRpcEntry, WasmRpcEntryPayload};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::durable_session::strip_streams;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::IdempotencyKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::card::ScopeCard;
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::schema::agent::AgentMethodSchema;
use golem_common::schema::agent::AgentTypeSchema;
use golem_common::schema::agent::FieldSource;
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::NamedFieldType;
use golem_schema::schema::SchemaType;
use golem_schema::schema::wit::EncodeError;
use golem_schema::schema::wit::PermissionCardHandleDropper;
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::QuotaTokenHandleDropper;
use golem_schema::schema::wit::SecretHandleDropper;
use golem_schema::schema::wit::decode_typed_rejecting_quota_with;
use golem_schema::schema::wit::decode_value_with;
use golem_schema::schema::wit::encode_value_with_streams;
use golem_schema::schema::wit::reject_quota_handles_in_value_tree;
use golem_schema::schema::wit::wire as core_wire;
use std::collections::BTreeMap;
use wasmtime::component::Resource;

pub(super) fn discard_owned_rpc_input<D>(input: core_wire::SchemaValueTree, dropper: &mut D)
where
    D: QuotaTokenHandleDropper + SecretHandleDropper + PermissionCardHandleDropper,
{
    let _ = reject_quota_handles_in_value_tree(input, dropper);
}

pub(super) fn discard_owned_rpc_config<D>(
    config: Vec<golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue>,
    dropper: &mut D,
) where
    D: QuotaTokenHandleDropper + SecretHandleDropper + PermissionCardHandleDropper,
{
    for entry in config {
        let _ = decode_typed_rejecting_quota_with(entry.value, dropper);
    }
}

pub(super) fn reject_non_await_scope_card<D>(
    scope_card: &Option<Resource<PermissionCardHandleRep>>,
    input: core_wire::SchemaValueTree,
    operation: &'static str,
    dropper: &mut D,
) -> Result<core_wire::SchemaValueTree, String>
where
    D: QuotaTokenHandleDropper + SecretHandleDropper + PermissionCardHandleDropper,
{
    if scope_card.is_some() {
        discard_owned_rpc_input(input, dropper);
        Err(format!("{operation} does not accept scope cards"))
    } else {
        Ok(input)
    }
}

impl PreparedRpcInvocation {
    pub(super) fn invoke_request(
        &self,
        remote_agent_id: &OwnedAgentId,
        idempotency_key: &IdempotencyKey,
        scope_card: Option<&ScopeCard>,
        logical_streaming_origin: Option<StreamInvocationId>,
    ) -> HostRequestGolemRpcInvoke {
        HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: self.method_name.clone(),
            input: strip_streams(self.input_value.clone()),
            logical_streaming_origin,
            remote_agent_type: None,
            remote_agent_parameters: None,
            scope_card: scope_card.cloned(),
        }
    }

    pub(super) fn is_ephemeral(&self) -> bool {
        self.ephemeral_logical_agent_id.is_some()
    }

    pub(super) fn is_streaming(&self) -> bool {
        self.streaming
    }
}

pub(super) fn rpc_input_root(prepared: &PreparedRpcInvocation) -> SchemaType {
    let method = prepared
        .remote_agent_type
        .methods
        .iter()
        .find(|method| method.name == prepared.method_name)
        .expect("prepared RPC method disappeared from its persisted agent type");
    SchemaType::record(
        method
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    )
}

pub(super) fn rpc_output_root(prepared: &PreparedRpcInvocation) -> SchemaType {
    prepared
        .remote_agent_type
        .methods
        .iter()
        .find(|method| method.name == prepared.method_name)
        .expect("prepared RPC method disappeared from its persisted agent type")
        .output_schema
        .schema()
        .cloned()
        .unwrap_or_else(|| SchemaType::tuple(Vec::new()))
}

pub(super) fn prepare_rpc_invocation<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    host_function_name: &str,
    synchronous: bool,
    method_name: String,
    input: core_wire::SchemaValueTree,
    streaming_allowed: bool,
) -> anyhow::Result<Result<PreparedRpcInvocation, RpcError>> {
    ctx.check_read_only_allows(host_function_name)
        .map_err(wasmtime::Error::from)?;

    let own_agent_id = ctx.owned_agent_id().clone();
    let (
        logical_remote_agent_id,
        ephemeral_logical_agent_id,
        connection_span_id,
        remote_agent_type,
        remote_method_streams,
        env,
        config,
    ) = {
        let entry = ctx.table().get(resource)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let (env, config) = payload.target_activation.target_creation_data();
        (
            payload.remote_agent_id.clone(),
            payload.ephemeral_logical_agent_id.clone(),
            payload.span_id.clone(),
            payload.remote_agent_type.clone(),
            payload.remote_method_streams.clone(),
            env,
            config,
        )
    };

    let input_value =
        match resolve_method_and_lift_input(&remote_agent_type, &method_name, input, ctx) {
            Ok(input_value) => input_value,
            Err(err) => return Ok(Err(err.into())),
        };
    let method = remote_agent_type
        .methods
        .iter()
        .find(|method| method.name == method_name)
        .expect("method existence was checked while decoding RPC input");
    let streaming = remote_method_uses_streams(&remote_method_streams, &method.name)?;

    if streaming && !streaming_allowed {
        return Ok(Err(RpcError::ProtocolError(
            "live streams require invoke-and-await".to_string(),
        )));
    }

    if ephemeral_logical_agent_id.is_none() && logical_remote_agent_id == own_agent_id {
        match ctx.runtime() {
            golem_common::model::entity::OwnerRuntime::Agent => {
                return Err(anyhow::anyhow!(
                    "RPC calls to the same agent are not supported"
                ));
            }
            golem_common::model::entity::OwnerRuntime::Entity(_) if synchronous => {
                let caller = ctx.owner_invocation_id()?;
                ctx.owner_execution
                    .lane()
                    .ensure_synchronous_owner_call(&caller)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            }
            golem_common::model::entity::OwnerRuntime::Entity(_) => {}
        }
    }

    Ok(Ok(PreparedRpcInvocation {
        logical_remote_agent_id,
        ephemeral_logical_agent_id,
        connection_span_id,
        env,
        config,
        method_name,
        input_value,
        streaming,
        remote_agent_type,
    }))
}

pub(super) fn remote_method_uses_streams(
    methods: &BTreeMap<String, bool>,
    method_name: &str,
) -> anyhow::Result<bool> {
    methods.get(method_name).copied().ok_or_else(|| {
        anyhow::anyhow!(
            "Streaming classification for remote agent method '{method_name}' is missing"
        )
    })
}

/// Look up an [`AgentMethodSchema`] by name from the cached remote agent type.
/// Used on the schedule path where the result is surfaced as a
/// `wasmtime::Error` trap, since `schedule_cancelable_invocation` has no
/// way to return `Err(RpcError)` to the guest.
pub(super) fn find_agent_method<'a>(
    agent_type: &'a AgentTypeSchema,
    method_name: &str,
) -> anyhow::Result<&'a AgentMethodSchema> {
    let method = agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Method '{method_name}' not found on agent type '{}'",
                agent_type.type_name
            )
        })?;
    Ok(method)
}

/// Resolve and lift the guest-side input value tree into the schema-native
/// [`SchemaValue`] carrier used across the executor↔executor RPC hop.
///
/// The wire value tree is transported as a schema-free [`SchemaValue`]; each
/// end validates it against its own declared schema (the callee when it lowers
/// the invocation, see [`lower_invocation`](crate::worker::invocation)). The
/// method is resolved only to fast-fail an unknown method before durability is
/// opened — a deterministic check that replay reproduces, surfaced as
/// [`InternalRpcError`] so the caller can return `Err(RpcError)` to the guest.
///
/// Any owned `quota-token` handle the guest passed in the input is consumed
/// from the resource table here and converted into its trusted
/// [`SchemaValue::QuotaToken`] snapshot via the `QuotaTokenResolver`, so the
/// capability travels across the RPC hop as an unforgeable host snapshot rather
/// than a guest-visible handle.
pub(super) fn resolve_method_and_lift_input<Ctx: WorkerCtx>(
    agent_type: &AgentTypeSchema,
    method_name: &str,
    input: core_wire::SchemaValueTree,
    resolver: &mut DurableWorkerCtx<Ctx>,
) -> Result<SchemaValue, InternalRpcError> {
    // Lift (and thereby consume) the guest input *before* the method-existence
    // check. The owned `quota-token` handles the input may carry were already
    // transferred into the host resource table at the WIT boundary, and the
    // unknown-method branch returns a non-trapping `RpcError` (or, for
    // `async-invoke-and-await`, a baked future) that leaves the instance — and
    // its resource table — alive. Decoding first guarantees those handles are
    // consumed/dropped even when the method is unknown.
    let input_value =
        decode_value_with(input, resolver).map_err(|err| InternalRpcError::ProtocolError {
            details: format!("Invalid RPC input for method '{method_name}': {err}"),
        })?;
    let method = agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| InternalRpcError::RemoteAgentError {
            error: Box::new(golem_common::model::agent::AgentError::InvalidMethod(
                format!(
                    "Method '{method_name}' not found on agent type '{}'",
                    agent_type.type_name
                ),
            )),
        })?;
    method
        .validate_input(&agent_type.schema, &input_value)
        .map_err(|error| InternalRpcError::ProtocolError {
            details: format!("Invalid RPC input for method '{method_name}': {error}"),
        })?;
    Ok(input_value)
}

/// Project an RPC output [`SchemaValue`] into the WIT
/// `option<schema-value-tree>` result shape used by `invoke-and-await` and
/// `future-invoke-result.get`.
///
/// Per the `golem:agent@2.0.0` contract a declared `unit` output (the
/// canonical empty tuple) maps to `none`, while a `single` output maps to
/// `some(value)`. A method that declares a single `()`/empty-tuple output is
/// structurally indistinguishable from `unit` here and is likewise reported as
/// `none`; both live and replay paths funnel through this helper, so the choice
/// is applied consistently.
///
/// Lowering the reply to the guest-facing wire form mints a fresh owned
/// `quota-token` handle for every [`SchemaValue::QuotaToken`] snapshot via the
/// `QuotaTokenResolver`, so a capability returned from an RPC call reaches the
/// caller's guest as an opaque, unforgeable resource handle.
pub(super) fn schema_value_to_wire_output<Ctx: WorkerCtx>(
    value: &SchemaValue,
    resolver: &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<core_wire::SchemaValueTree>, EncodeError> {
    match value {
        SchemaValue::Tuple { elements } if elements.is_empty() => Ok(None),
        value => Ok(Some(encode_value_with_streams(value, resolver)?)),
    }
}

pub(super) fn invoke_and_await_response_to_wire<Ctx: WorkerCtx>(
    result: Result<SchemaValue, SerializableRpcError>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        Ok(value) => Ok(Ok(schema_value_to_wire_output(&value, ctx)?)),
        Err(error) => {
            let rpc_error: InternalRpcError = error.into();
            Ok(Err(rpc_error.into()))
        }
    }
}
