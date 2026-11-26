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

use crate::durable_host::durable_session::strip_streams;
use golem_common::model::oplog::{FinishedResult, OplogEntry, OplogPayload, ReturnedValue};
use golem_common::model::{AgentInvocationResult, InvocationId, OplogIndex};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::streaming::decode_recursive_stream_value;
use prost::Message;
use std::future::Future;

/// Resolves the one authoritative value of a completed invocation. A reference must name an
/// earlier return of the same invocation, never another value or another kind of oplog entry.
pub async fn resolve_finished_result<
    Read,
    ReadFuture,
    Payload,
    PayloadFuture,
    Returned,
    ReturnedFuture,
    Error,
>(
    terminal: OplogIndex,
    invocation_id: InvocationId,
    result: FinishedResult,
    read: Read,
    payload: Payload,
    returned: Returned,
    corrupt: impl Fn(String) -> Error,
) -> Result<AgentInvocationResult, Error>
where
    Read: FnOnce(OplogIndex) -> ReadFuture,
    ReadFuture: Future<Output = Result<OplogEntry, Error>>,
    Payload: FnOnce(OplogPayload<AgentInvocationResult>) -> PayloadFuture,
    PayloadFuture: Future<Output = Result<AgentInvocationResult, Error>>,
    Returned: FnOnce(OplogPayload<ReturnedValue>) -> ReturnedFuture,
    ReturnedFuture: Future<Output = Result<ReturnedValue, Error>>,
{
    match result {
        FinishedResult::Payload(value) => payload(value).await,
        FinishedResult::Returned(index) => {
            if index >= terminal {
                return Err(corrupt(format!(
                    "invocation result at {terminal} refers to non-earlier entry {index}"
                )));
            }
            let OplogEntry::AgentInvocationReturned {
                invocation_id: recorded_id,
                result,
                ..
            } = read(index).await?
            else {
                return Err(corrupt(format!(
                    "invocation result reference {index} is not AgentInvocationReturned"
                )));
            };
            if recorded_id != invocation_id {
                return Err(corrupt(format!(
                    "invocation result reference {index} names another invocation"
                )));
            }
            let result = returned(result).await?;
            let value =
                golem_schema::proto::golem::schema::SchemaValue::decode(result.value.as_slice())
                    .map_err(|error| {
                        corrupt(format!("invalid returned value at {index}: {error}"))
                    })?;
            let value = decode_recursive_stream_value(value, |handle, _| {
                if handle as u128 >= result.stream_bindings.len() as u128 {
                    return Err(format!(
                        "unknown returned stream binding {handle} at {index}"
                    ));
                }
                Ok(SchemaValueStream::from_host_endpoint(()))
            })
            .map_err(corrupt)?;
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

    async fn resolve(entry: OplogEntry, reference: u64) -> Result<AgentInvocationResult, String> {
        resolve_finished_result(
            OplogIndex::from_u64(20),
            Uuid::from_u128(42).into(),
            FinishedResult::Returned(OplogIndex::from_u64(reference)),
            |_| async { Ok(entry) },
            |_| async { panic!("a reference must not download a terminal payload") },
            |payload| async {
                match payload {
                    OplogPayload::Inline(value) => Ok(*value),
                    _ => panic!("test uses inline payloads"),
                }
            },
            std::convert::identity,
        )
        .await
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
