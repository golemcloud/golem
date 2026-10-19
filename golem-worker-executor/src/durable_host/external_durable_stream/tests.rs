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
use golem_common::schema::schema_value::SecretValuePayload;
use test_r::test;
use wasmtime::component::ResourceTable;

fn auth() -> SecretValuePayload {
    SecretValuePayload {
        secret_id: uuid::Uuid::from_u128(37),
        config_key: Some(vec!["streams".into(), "credential".into()]),
        version: 9,
        resolved_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        category: Some("external".into()),
    }
}

#[test]
fn reader_restores_recorded_auth_and_expands_request_independently_of_table_index() {
    let recorded = HostRequestDurableStreamReaderNew {
        options: DurableStreamReaderOptions {
            url: "https://streams.example/reader".into(),
            mode: DurableStreamMode::Bytes,
            timeout_ms: 7123,
        },
        auth: Some(auth()),
    };
    let mut reconstructed = recorded.clone();
    reconstructed.auth.as_mut().unwrap().resolved_at += chrono::Duration::seconds(123);
    let guest = DurableStreamReaderEntry::from_request(reconstructed.clone()).unwrap();
    let restored = DurableStreamReaderEntry::from_request(recorded).unwrap();
    validate_resource_id(&guest.resource_id, &restored.resource_id).unwrap();
    assert_eq!(restored.auth.as_ref().unwrap().to_snapshot(), auth());

    let mut before = ResourceTable::new();
    let first = before.push(guest).unwrap();
    let mut after = ResourceTable::new();
    after.push("unrelated resource").unwrap();
    let resource = after.push(restored.clone()).unwrap();
    assert_ne!(first.rep(), resource.rep());
    assert_eq!(
        before.get(&first).unwrap().resource_id,
        after.get(&resource).unwrap().resource_id
    );
    let request = HostRequestDurableStreamRead {
        resource_id: restored.resource_id.clone(),
        checkpoint: DurableStreamCheckpoint {
            offset: "offset-17".into(),
            cursor: Some("cursor-23".into()),
        },
        transport: DurableStreamTransport::Sse,
        content_type: Some("application/octet-stream".into()),
    };
    assert_eq!(
        after.get(&resource).unwrap().read_request(&request),
        DurableStreamReadRequest {
            url: "https://streams.example/reader".into(),
            checkpoint: DurableStreamCheckpoint {
                offset: "offset-17".into(),
                cursor: Some("cursor-23".into()),
            },
            mode: DurableStreamMode::Bytes,
            transport: DurableStreamTransport::Sse,
            content_type: Some("application/octet-stream".into()),
            timeout_ms: 7123,
        }
    );
    reconstructed.auth.as_mut().unwrap().version += 1;
    let rotated = DurableStreamReaderEntry::from_request(reconstructed).unwrap();
    assert!(validate_resource_id(&rotated.resource_id, &restored.resource_id).is_err());
    assert!(validate_resource_id(&restored.resource_id, "different recorded response").is_err());
}

#[test]
fn writer_keeps_immutable_descriptor_and_caller_owned_sequence_after_resource_drop() {
    let request = HostRequestDurableStreamWriterNew {
        options: DurableStreamWriterOptions {
            url: "https://streams.example/writer".into(),
            content_type: "application/json".into(),
            producer_id: "writer-1".into(),
            producer_epoch: 13,
            timeout_ms: 8111,
        },
        auth: Some(auth()),
    };
    let entry = DurableStreamWriterEntry::from_request(request).unwrap();
    let mut table = ResourceTable::new();
    let secret = table
        .push(SecretHandleRep::from(entry.auth.clone().unwrap()))
        .unwrap();
    let resource = table.push(entry).unwrap();
    table.delete(secret).unwrap();
    let pending = table.get(&resource).unwrap().clone();
    table.delete(resource).unwrap();
    assert_eq!(pending.auth.as_ref().unwrap().to_snapshot(), auth());
    let mut append = HostRequestDurableStreamAppend {
        resource_id: pending.resource_id.clone(),
        payload: DurableStreamAppendPayload::Json(vec!["[7,3]".into(), "9007199254740993".into()]),
        sequence: 17,
        close: false,
    };
    let expected = DurableStreamAppendRequest {
        url: "https://streams.example/writer".into(),
        content_type: "application/json".into(),
        payload: DurableStreamAppendPayload::Json(vec!["[7,3]".into(), "9007199254740993".into()]),
        producer: DurableStreamProducer {
            id: "writer-1".into(),
            epoch: 13,
            sequence: 17,
        },
        close: false,
        timeout_ms: 8111,
    };
    assert_eq!(pending.append_request(&append), expected);
    assert_eq!(pending.append_request(&append), expected);
    append.payload = DurableStreamAppendPayload::Bytes(vec![]);
    append.sequence = 18;
    append.close = true;
    let close = pending.append_request(&append);
    assert_eq!(
        close.producer,
        DurableStreamProducer {
            id: "writer-1".into(),
            epoch: 13,
            sequence: 18
        }
    );
    assert_eq!(close.payload, DurableStreamAppendPayload::Bytes(vec![]));
    assert!(close.close);
    assert_eq!(pending.options.producer_epoch, 13);
}
