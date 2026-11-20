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

/// Encode a public-oplog [`TypedSchemaValue`] into the `golem:core@2.0.0` WIT
/// wire form used by the oplog-processor plugin interface. Public-oplog
/// values are redacted before encoding so host-managed secrets and quota
/// capabilities cannot cross this untrusted boundary. Encoding remains
/// fallible so malformed schema/value pairs surface as conversion errors.
pub(super) fn encode_public_typed_schema_value(
    value: TypedSchemaValue,
) -> Result<wire::TypedSchemaValue, String> {
    let value = redact_host_managed_typed_value(value);
    encode_typed(&value).map_err(|e| {
        format!("public oplog TypedSchemaValue is not encodable as golem:core@2.0.0 WIT: {e}")
    })
}

pub(super) fn encode_untyped_schema_value(
    value: SchemaValue,
) -> Result<wire::SchemaValueTree, String> {
    encode_value(&redact_host_managed_value(value)).map_err(|e| e.to_string())
}

pub(super) fn decode_untyped_schema_value(
    value: wire::SchemaValueTree,
) -> Result<SchemaValue, String> {
    decode_value(&value).map_err(|e| e.to_string())
}

/// Drain owned `quota-token` handles out of guest-supplied raw oplog entries
/// before they are converted with the pure (resolver-less)
/// [`TryFrom<oplog::OplogEntry>`] path.
///
/// `enrich-oplog-entries` is the only guest-reachable caller of that conversion,
/// and it returns the conversion error as a non-trapping `result::err` — so the
/// instance, and its resource table, stay alive. Each
/// `create.local-agent-config[].value` and each request of the divergence of a
/// `failed-update` is a live `schema-value-tree`. Capability handles are never
/// permitted there, so any owned handle found is deleted from the resource
/// table (rather than leaked) and the whole batch is rejected. A divergence
/// request does not permit an owned stream either, and its streams are disposed
/// of as a guest drop of the resource does. All entries are drained before the first error is
/// surfaced, so a handle in a later entry cannot leak when an earlier entry is
/// rejected.
pub(crate) fn reject_quota_handles_in_oplog_entries<
    D: QuotaTokenHandleDropper
        + SecretHandleDropper
        + PermissionCardHandleDropper
        + SchemaValueStreamResolver,
>(
    entries: Vec<(u64, oplog::OplogEntry)>,
    dropper: &mut D,
) -> Result<Vec<(u64, oplog::OplogEntry)>, String> {
    let mut first_error: Option<String> = None;
    let mut sanitized = Vec::with_capacity(entries.len());
    for (index, entry) in entries {
        let entry = match entry {
            oplog::OplogEntry::Create(mut params) => {
                let configs = std::mem::take(&mut params.local_agent_config);
                let mut new_configs = Vec::with_capacity(configs.len());
                for mut cfg in configs {
                    match reject_quota_handles_in_value_tree(cfg.value, dropper) {
                        Ok(tree) => {
                            cfg.value = tree;
                            new_configs.push(cfg);
                        }
                        Err(e) => {
                            first_error.get_or_insert_with(|| {
                                format!("agent config value must not contain a quota token: {e}")
                            });
                            // The owned handle(s) were already dropped; this
                            // entry is discarded once the error is returned.
                        }
                    }
                }
                params.local_agent_config = new_configs;
                oplog::OplogEntry::Create(params)
            }
            oplog::OplogEntry::FailedUpdate(mut params) => {
                if let Some(divergence) = params.divergence.as_mut() {
                    for step in [&mut divergence.expected, &mut divergence.actual] {
                        if let oplog::ReplayStep::HostCall(call) = step
                            && let Some(mut request) = call.request.take()
                        {
                            let stream =
                                drop_stream_handles_in_value_tree(&mut request.value, dropper);
                            match (
                                stream,
                                reject_quota_handles_in_value_tree(request.value, dropper),
                            ) {
                                (None, Ok(tree)) => {
                                    request.value = tree;
                                    call.request = Some(request);
                                }
                                (_, Err(e)) => {
                                    first_error.get_or_insert_with(|| {
                                        format!("a divergence request must not contain an owned handle: {e}")
                                    });
                                }
                                (Some(node), Ok(_)) => {
                                    first_error.get_or_insert_with(|| {
                                        format!("a divergence request must not contain an owned handle: stream at value node {node}")
                                    });
                                }
                            }
                        }
                    }
                }
                oplog::OplogEntry::FailedUpdate(params)
            }
            other => other,
        };
        sanitized.push((index, entry));
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(sanitized),
    }
}

/// Disposes of every owned stream of `tree` through the resolver, as a guest drop of the resource
/// does, and returns the index of the first one. Unreachable nodes are disposed of too.
fn drop_stream_handles_in_value_tree<D: SchemaValueStreamResolver>(
    tree: &mut wire::SchemaValueTree,
    dropper: &mut D,
) -> Option<usize> {
    let mut first = None;
    for (index, node) in tree.value_nodes.iter_mut().enumerate() {
        if matches!(node, wire::SchemaValueNode::StreamValue(_))
            && let wire::SchemaValueNode::StreamValue(handle) =
                std::mem::replace(node, wire::SchemaValueNode::BoolValue(false))
        {
            dropper.drop_stream_handle(handle);
            first.get_or_insert(index);
        }
    }
    first
}

pub(super) fn oplog_payload_from_wit<
    T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq,
>(
    payload: oplog::OplogPayload,
) -> golem_common::model::oplog::payload::OplogPayload<T> {
    match payload {
        oplog::OplogPayload::Inline(bytes) => {
            golem_common::model::oplog::payload::OplogPayload::SerializedInline {
                bytes,
                cached: None,
            }
        }
        oplog::OplogPayload::External(ext) => {
            golem_common::model::oplog::payload::OplogPayload::External {
                payload_id: golem_common::model::oplog::PayloadId(uuid::Uuid::from_u64_pair(
                    ext.payload_id.high_bits,
                    ext.payload_id.low_bits,
                )),
                md5_hash: ext.md5_hash,
                cached: None,
            }
        }
    }
}

pub(super) fn oplog_payload_to_wit<
    T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq,
>(
    payload: golem_common::model::oplog::payload::OplogPayload<T>,
) -> Result<oplog::OplogPayload, String> {
    match payload.try_into_raw()? {
        golem_common::model::oplog::payload::RawOplogPayload::SerializedInline(bytes) => {
            Ok(oplog::OplogPayload::Inline(bytes))
        }
        golem_common::model::oplog::payload::RawOplogPayload::External {
            payload_id,
            md5_hash,
        } => Ok(oplog::OplogPayload::External(oplog::OplogExternalPayload {
            payload_id: payload_id.0.into(),
            md5_hash,
        })),
    }
}

pub(super) fn replay_divergence_to_wit(
    divergence: ReplayDivergence,
) -> Result<oplog::ReplayDivergence, String> {
    Ok(oplog::ReplayDivergence {
        oplog_index: divergence.oplog_index.into(),
        expected: replay_step_to_wit(divergence.expected)?,
        actual: replay_step_to_wit(divergence.actual)?,
        mismatched: divergence
            .mismatched
            .into_iter()
            .map(start_identity_field_to_wit)
            .collect(),
    })
}

pub(super) fn replay_divergence_from_wit(
    divergence: oplog::ReplayDivergence,
) -> Result<ReplayDivergence, String> {
    Ok(ReplayDivergence {
        oplog_index: golem_common::model::OplogIndex::from_u64(divergence.oplog_index),
        expected: replay_step_from_wit(divergence.expected)?,
        actual: replay_step_from_wit(divergence.actual)?,
        mismatched: divergence
            .mismatched
            .into_iter()
            .map(start_identity_field_from_wit)
            .collect(),
    })
}

fn replay_step_to_wit(step: ReplayStep) -> Result<oplog::ReplayStep, String> {
    Ok(match step {
        ReplayStep::HostCall(call) => oplog::ReplayStep::HostCall(oplog::ReplayHostCall {
            function_name: call.function_name,
            durable_function_type: call.durable_function_type.into(),
            parent_start_index: call.parent_start_index.map(Into::into),
            invocation_id: call.invocation_id.map(durable_invocation_id_to_wit),
            observational_owner: call.observational_owner.map(Into::into),
            driver: call.driver.map(Into::into),
            carries_request: call.carries_request,
            request: call
                .request
                .map(|request| encode_public_typed_schema_value(*request))
                .transpose()?,
        }),
        ReplayStep::InvocationEnd(_) => oplog::ReplayStep::InvocationEnd,
        ReplayStep::EndOfHistory(_) => oplog::ReplayStep::EndOfHistory,
        ReplayStep::Other(description) => oplog::ReplayStep::Other(description.description),
    })
}

fn replay_step_from_wit(step: oplog::ReplayStep) -> Result<ReplayStep, String> {
    Ok(match step {
        oplog::ReplayStep::HostCall(call) => ReplayStep::HostCall(ReplayHostCall {
            function_name: call.function_name,
            durable_function_type: golem_common::model::oplog::DurableFunctionType::from(
                call.durable_function_type,
            )
            .into(),
            parent_start_index: call
                .parent_start_index
                .map(golem_common::model::OplogIndex::from_u64),
            invocation_id: call.invocation_id.map(durable_invocation_id_from_wit),
            observational_owner: call
                .observational_owner
                .map(golem_common::model::OplogIndex::from_u64),
            driver: call.driver.map(golem_common::model::OplogIndex::from_u64),
            carries_request: call.carries_request,
            request: call
                .request
                .map(|request| {
                    golem_schema::schema::wit::decode_typed(&request)
                        .map(Box::new)
                        .map_err(|error| error.to_string())
                })
                .transpose()?,
        }),
        oplog::ReplayStep::InvocationEnd => ReplayStep::invocation_end(),
        oplog::ReplayStep::EndOfHistory => ReplayStep::end_of_history(),
        oplog::ReplayStep::Other(description) => ReplayStep::described(description),
    })
}

fn start_identity_field_to_wit(field: StartIdentityField) -> oplog::StartIdentityField {
    match field {
        StartIdentityField::FunctionName => oplog::StartIdentityField::FunctionName,
        StartIdentityField::FunctionType => oplog::StartIdentityField::FunctionType,
        StartIdentityField::InvocationId => oplog::StartIdentityField::InvocationId,
        StartIdentityField::ObservationalOwner => oplog::StartIdentityField::ObservationalOwner,
        StartIdentityField::RequestPresence => oplog::StartIdentityField::RequestPresence,
        StartIdentityField::Parent => oplog::StartIdentityField::Parent,
        StartIdentityField::Driver => oplog::StartIdentityField::Driver,
    }
}

fn start_identity_field_from_wit(field: oplog::StartIdentityField) -> StartIdentityField {
    match field {
        oplog::StartIdentityField::FunctionName => StartIdentityField::FunctionName,
        oplog::StartIdentityField::FunctionType => StartIdentityField::FunctionType,
        oplog::StartIdentityField::InvocationId => StartIdentityField::InvocationId,
        oplog::StartIdentityField::ObservationalOwner => StartIdentityField::ObservationalOwner,
        oplog::StartIdentityField::RequestPresence => StartIdentityField::RequestPresence,
        oplog::StartIdentityField::Parent => StartIdentityField::Parent,
        oplog::StartIdentityField::Driver => StartIdentityField::Driver,
    }
}

pub(super) fn update_failure_class_to_wit(class: UpdateFailureClass) -> oplog::UpdateFailureClass {
    match class {
        UpdateFailureClass::Divergence => oplog::UpdateFailureClass::Divergence,
        UpdateFailureClass::TargetTrap => oplog::UpdateFailureClass::TargetTrap,
        UpdateFailureClass::NotReplayable => oplog::UpdateFailureClass::NotReplayable,
        UpdateFailureClass::TargetRejected => oplog::UpdateFailureClass::TargetRejected,
        UpdateFailureClass::Interrupted => oplog::UpdateFailureClass::Interrupted,
        UpdateFailureClass::Operational => oplog::UpdateFailureClass::Operational,
        UpdateFailureClass::Other => oplog::UpdateFailureClass::Other,
    }
}

pub(super) fn update_failure_class_from_wit(
    class: oplog::UpdateFailureClass,
) -> UpdateFailureClass {
    match class {
        oplog::UpdateFailureClass::Divergence => UpdateFailureClass::Divergence,
        oplog::UpdateFailureClass::TargetTrap => UpdateFailureClass::TargetTrap,
        oplog::UpdateFailureClass::NotReplayable => UpdateFailureClass::NotReplayable,
        oplog::UpdateFailureClass::TargetRejected => UpdateFailureClass::TargetRejected,
        oplog::UpdateFailureClass::Interrupted => UpdateFailureClass::Interrupted,
        oplog::UpdateFailureClass::Operational => UpdateFailureClass::Operational,
        oplog::UpdateFailureClass::Other => UpdateFailureClass::Other,
    }
}
