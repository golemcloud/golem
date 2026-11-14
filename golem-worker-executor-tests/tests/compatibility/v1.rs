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

//! Serialization tests for worker information.
//!
//! The tests are assuming composability of the serializer implementation, so if a given type A has a field of type B,
//! the test for A only contains an example value of B but there exists a separate test that tests the serialization of B.

use desert_rust::BinaryCodec;
use goldenfile::Mint;
use goldenfile::differs::Differ;
use golem_common::serialization::{deserialize, serialize};
use std::fmt::Debug;
use std::io::Write;
use std::path::Path;
use test_r::test;

#[allow(unused)]
fn is_deserializable<T: BinaryCodec + PartialEq + Debug>(old: &Path, new: &Path) {
    let old = std::fs::read(old).unwrap();
    let new = std::fs::read(new).unwrap();

    // Both the old and the latest binary can be deserialized
    let old_decoded: T = deserialize(&old).unwrap();
    let new_decoded: T = deserialize(&new).unwrap();

    // And they represent the same value
    assert_eq!(old_decoded, new_decoded);
}

#[allow(unused)]
pub(crate) fn backward_compatible_custom<T: BinaryCodec + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
    differ: Differ,
) {
    let mut file = mint
        .new_goldenfile_with_differ(format!("{}.bin", name.as_ref()), differ)
        .unwrap();
    let encoded = serialize(&value).unwrap();
    file.write_all(&encoded).unwrap();
    file.flush().unwrap();
}

#[allow(unused)]
pub(crate) fn backward_compatible<T: BinaryCodec + PartialEq + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
) {
    backward_compatible_custom(name, mint, value, Box::new(is_deserializable::<T>))
}

#[test]
fn stream_terminal_oplog_roundtrips() {
    use golem_common::model::Timestamp;
    use golem_common::model::durable_stream::{
        DURABLE_STREAM_FORMAT_VERSION, LocalStreamId, StreamError, StreamOffset, StreamTerminal,
        StreamTerminalAuthor, StreamTerminalOrigin, StreamTerminalRecord,
    };
    use golem_common::model::oplog::payload::OplogPayload;
    use golem_common::model::oplog::{OplogEntry, OplogIndex};
    use golem_common::schema::IntoTypedSchemaValue;

    let declared = (73u32, "producer rejected input".to_string())
        .into_typed_schema_value()
        .unwrap();
    let cases = [
        StreamTerminal::Finished,
        StreamTerminal::Failed(StreamError::Failed(Box::new(declared))),
        StreamTerminal::Failed(StreamError::Trapped("producer trapped".into())),
        StreamTerminal::Failed(StreamError::Cancelled),
        StreamTerminal::Failed(StreamError::Abandoned),
        StreamTerminal::Failed(StreamError::Interrupted("source unavailable".into())),
        StreamTerminal::Failed(StreamError::ResourceExhausted("retained bytes".into())),
        StreamTerminal::ConsumerCancelled {
            details: Some("reader dropped".into()),
        },
        StreamTerminal::ConsumerCancelled { details: None },
    ];
    for authored_by in [
        StreamTerminalAuthor::Guest,
        StreamTerminalAuthor::Host,
        StreamTerminalAuthor::Protocol,
    ] {
        for terminal in &cases {
            for origin in [StreamTerminalOrigin::Source, StreamTerminalOrigin::Control] {
                let record = StreamTerminalRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id: LocalStreamId(OplogIndex::from_u64(19)),
                    sequence: 7,
                    offset: StreamOffset::new(OplogIndex::from_u64(31), 0),
                    authored_by,
                    origin,
                    terminal: terminal.clone(),
                };
                let record_bytes = serialize(&record).unwrap();
                assert_eq!(
                    deserialize::<StreamTerminalRecord>(&record_bytes).unwrap(),
                    record
                );
                let entry = OplogEntry::StreamTerminal {
                    timestamp: Timestamp::from(1_700_000_000_000u64),
                    entity_parent_start_index: Some(OplogIndex::from_u64(11)),
                    summary: None,
                    record: OplogPayload::SerializedInline {
                        bytes: record_bytes,
                        cached: None,
                    },
                };
                let encoded = serialize(&entry).unwrap();
                assert_eq!(deserialize::<OplogEntry>(&encoded).unwrap(), entry);
            }
        }
    }
}
