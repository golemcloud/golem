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
/// instance, and its resource table, stay alive. The only live
/// `schema-value-tree` carried by a *raw* oplog entry is each
/// `create.local-agent-config[].value`; every other raw payload is an opaque
/// `oplog-payload` byte blob. Quota tokens are never permitted in agent config,
/// so any owned handle found there is deleted from the resource table (rather
/// than leaked) and the whole batch is rejected. All entries are drained before
/// the first error is surfaced, so a handle in a later entry cannot leak when an
/// earlier entry is rejected.
pub(crate) fn reject_quota_handles_in_oplog_entries<
    D: QuotaTokenHandleDropper + SecretHandleDropper + PermissionCardHandleDropper,
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
            other => other,
        };
        sanitized.push((index, entry));
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(sanitized),
    }
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
