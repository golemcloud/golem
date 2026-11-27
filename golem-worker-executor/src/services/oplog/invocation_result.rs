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

use super::{Oplog, OplogOps, OplogService, OplogServiceOps};
use crate::durable_host::durable_session::strip_streams;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{FinishedResult, OplogEntry, OplogPayload, ReturnedValue};
use golem_common::model::{AgentInvocationResult, InvocationId, OplogIndex, OwnedAgentId};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::streaming::decode_recursive_stream_value;
use prost::Message;
use std::fmt::Debug;
use std::sync::Arc;

#[async_trait]
pub trait InvocationResultReader: Sync {
    async fn read(&self, index: OplogIndex) -> anyhow::Result<OplogEntry>;

    async fn download<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static>(
        &self,
        payload: OplogPayload<T>,
    ) -> anyhow::Result<T>;
}

#[async_trait]
impl InvocationResultReader for Arc<dyn Oplog> {
    async fn read(&self, index: OplogIndex) -> anyhow::Result<OplogEntry> {
        Oplog::read(self.as_ref(), index).await.map_err(Into::into)
    }

    async fn download<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static>(
        &self,
        payload: OplogPayload<T>,
    ) -> anyhow::Result<T> {
        self.download_payload(payload)
            .await
            .map_err(anyhow::Error::msg)
    }
}

#[async_trait]
impl InvocationResultReader for (&dyn OplogService, &OwnedAgentId, AgentMode) {
    async fn read(&self, index: OplogIndex) -> anyhow::Result<OplogEntry> {
        self.0
            .read_exact(self.1, self.2, index, 1)
            .await?
            .remove(&index)
            .ok_or_else(|| anyhow::anyhow!("missing invocation return {index}"))
    }

    async fn download<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static>(
        &self,
        payload: OplogPayload<T>,
    ) -> anyhow::Result<T> {
        self.0
            .download_payload(self.1, self.2, payload)
            .await
            .map_err(Into::into)
    }
}

/// Validates a reference without downloading the value it names.
pub async fn read_invocation_returned(
    reader: &impl InvocationResultReader,
    terminal: OplogIndex,
    invocation_id: InvocationId,
    index: OplogIndex,
) -> anyhow::Result<OplogPayload<ReturnedValue>> {
    anyhow::ensure!(
        index < terminal,
        "invocation result at {terminal} refers to non-earlier entry {index}"
    );
    let OplogEntry::AgentInvocationReturned {
        invocation_id: recorded_id,
        result,
        ..
    } = reader.read(index).await?
    else {
        anyhow::bail!("invocation result reference {index} is not AgentInvocationReturned");
    };
    anyhow::ensure!(
        recorded_id == invocation_id,
        "invocation result reference {index} names another invocation"
    );
    Ok(result)
}

/// Resolves the one authoritative value of a completed invocation. A reference must name an
/// earlier return of the same invocation, never another value or another kind of oplog entry.
pub async fn resolve_finished_result(
    reader: &impl InvocationResultReader,
    terminal: OplogIndex,
    invocation_id: InvocationId,
    result: FinishedResult,
) -> anyhow::Result<AgentInvocationResult> {
    match result {
        FinishedResult::Payload(value) => reader.download(value).await,
        FinishedResult::Returned(index) => {
            let result = read_invocation_returned(reader, terminal, invocation_id, index).await?;
            let result = reader.download(result).await?;
            let value =
                golem_schema::proto::golem::schema::SchemaValue::decode(result.value.as_slice())
                    .map_err(|error| {
                        anyhow::anyhow!("invalid returned value at {index}: {error}")
                    })?;
            let value = decode_recursive_stream_value(value, |handle, _| {
                if handle as u128 >= result.stream_bindings.len() as u128 {
                    return Err(format!(
                        "unknown returned stream binding {handle} at {index}"
                    ));
                }
                Ok(SchemaValueStream::from_host_endpoint(()))
            })
            .map_err(anyhow::Error::msg)?;
            Ok(AgentInvocationResult::AgentMethod {
                output: strip_streams(value),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::durable_stream::{
        LocalStreamId, SessionStreamRole, StreamBindingRecord, StreamRecordReference,
    };
    use golem_common::schema::streaming::encode_recursive_stream_value;
    use golem_common::schema::{SchemaValue, VariantValuePayload};
    use test_r::test;
    use uuid::Uuid;

    struct Reader(OplogEntry);

    #[async_trait]
    impl InvocationResultReader for Reader {
        async fn read(&self, _: OplogIndex) -> anyhow::Result<OplogEntry> {
            Ok(self.0.clone())
        }

        async fn download<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static>(
            &self,
            payload: OplogPayload<T>,
        ) -> anyhow::Result<T> {
            match payload {
                OplogPayload::Inline(value) => Ok(*value),
                _ => panic!("test uses inline payloads"),
            }
        }
    }

    async fn resolve(entry: OplogEntry, reference: u64) -> Result<AgentInvocationResult, String> {
        resolve_finished_result(
            &Reader(entry),
            OplogIndex::from_u64(20),
            Uuid::from_u128(42).into(),
            FinishedResult::Returned(OplogIndex::from_u64(reference)),
        )
        .await
        .map_err(|error| error.to_string())
    }

    fn returned(id: u128, value: SchemaValue, bindings: usize) -> OplogEntry {
        let value = encode_recursive_stream_value(&value, |_, _| Ok(0))
            .unwrap()
            .encode_to_vec();
        OplogEntry::AgentInvocationReturned {
            timestamp: Timestamp::from(1_000),
            invocation_id: Uuid::from_u128(id).into(),
            result: OplogPayload::Inline(Box::new(ReturnedValue {
                value,
                stream_bindings: (0..bindings)
                    .map(|_| StreamBindingRecord {
                        transport_stream_id: 9,
                        source: StreamRecordReference::Local(LocalStreamId(OplogIndex::from_u64(
                            3,
                        ))),
                        role: SessionStreamRole::Output,
                    })
                    .collect(),
            })),
            method_name: Some("nested".into()),
            component_revision: ComponentRevision::INITIAL,
        }
    }

    #[test]
    async fn returned_result_projects_nested_streams_without_removing_real_units() {
        let stream = || SchemaValue::Stream(SchemaValueStream::from_host_endpoint(()));
        let unit = || SchemaValue::Tuple { elements: vec![] };
        let input = SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(17),
                stream(),
                SchemaValue::List {
                    elements: vec![stream(), unit()],
                },
                SchemaValue::Variant(VariantValuePayload {
                    case: 2,
                    payload: Some(Box::new(stream())),
                }),
                unit(),
            ],
        };
        let expected = SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(17),
                unit(),
                SchemaValue::List {
                    elements: vec![unit(), unit()],
                },
                SchemaValue::Variant(VariantValuePayload {
                    case: 2,
                    payload: Some(Box::new(unit())),
                }),
                unit(),
            ],
        };
        assert_eq!(
            resolve(returned(42, input, 1), 10).await.unwrap(),
            AgentInvocationResult::AgentMethod { output: expected }
        );
    }

    #[test]
    async fn returned_result_rejects_wrong_invocation_kind_position_and_binding() {
        let stream = || SchemaValue::Stream(SchemaValueStream::from_host_endpoint(()));
        assert!(
            resolve(returned(43, stream(), 1), 10)
                .await
                .unwrap_err()
                .contains("another invocation")
        );
        assert!(
            resolve(
                OplogEntry::Suspend {
                    timestamp: Timestamp::from(1_000)
                },
                10
            )
            .await
            .unwrap_err()
            .contains("not AgentInvocationReturned")
        );
        for reference in [20, 21] {
            assert!(
                resolve(returned(42, stream(), 1), reference)
                    .await
                    .unwrap_err()
                    .contains("non-earlier")
            );
        }
        assert!(
            resolve(returned(42, stream(), 0), 10)
                .await
                .unwrap_err()
                .contains("unknown returned stream binding")
        );
    }
}
