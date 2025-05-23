// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::compatibility::v1::backward_compatible;
use goldenfile::Mint;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
};
use golem_common::model::oplog::{OplogEntry, OplogPayload, SpanData};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    IdempotencyKey, OplogIndex, Timestamp, TimestampedWorkerInvocation, WorkerInvocation,
};
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::num::{NonZeroU128, NonZeroU64};
use test_r::test;

#[test]
pub fn oplog_entry() {
    let oe31 = OplogEntry::Revert {
        timestamp: Timestamp::from(1724701938466),
        dropped_region: OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(10),
        },
    };

    let oe32 = OplogEntry::CancelPendingInvocation {
        timestamp: Timestamp::from(1724701938466),
        idempotency_key: IdempotencyKey {
            value: "idempotency_key".to_string(),
        },
    };

    let oe33 = OplogEntry::ExportedFunctionInvoked {
        timestamp: Timestamp::from(1724701938466),
        function_name: "test:pkg/iface.{fn}".to_string(),
        request: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        idempotency_key: IdempotencyKey {
            value: "id1".to_string(),
        },
        trace_id: TraceId::from_string("4bf92f3577b34da6a3ce929d0e0e4736").unwrap(),
        trace_states: vec!["a=1".to_string(), "b=2".to_string()],
        invocation_context: vec![
            SpanData::LocalSpan {
                span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
                start: Timestamp::from(1724701938466),
                parent_id: Some(SpanId::from_string("00f067aa0ba902b7").unwrap()),
                linked_context: Some(vec![SpanData::LocalSpan {
                    span_id: SpanId::from_string("d0fa4a9110f2dcab").unwrap(),
                    start: Timestamp::from(1724701938466),
                    parent_id: None,
                    linked_context: None,
                    attributes: HashMap::new(),
                    inherited: true,
                }]),
                attributes: HashMap::from_iter(vec![(
                    "key".to_string(),
                    AttributeValue::String("value".to_string()),
                )]),
                inherited: false,
            },
            SpanData::ExternalSpan {
                span_id: SpanId::from_string("00f067aa0ba902b7").unwrap(),
            },
        ],
    };

    let oe34 = OplogEntry::StartSpan {
        timestamp: Timestamp::from(1724701938466),
        span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
        parent_id: Some(SpanId::from_string("00f067aa0ba902b7").unwrap()),
        linked_context_id: Some(SpanId::from_string("d0fa4a9110f2dcab").unwrap()),
        attributes: HashMap::from_iter(vec![(
            "key".to_string(),
            AttributeValue::String("value".to_string()),
        )]),
    };

    let oe35 = OplogEntry::FinishSpan {
        timestamp: Timestamp::from(1724701938466),
        span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
    };

    let oe36 = OplogEntry::SetSpanAttribute {
        timestamp: Timestamp::from(1724701938466),
        span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
        key: "key".to_string(),
        value: AttributeValue::String("value".to_string()),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_revert", &mut mint, oe31);
    backward_compatible("oplog_entry_cancel_pending_invocation", &mut mint, oe32);
    backward_compatible("oplog_entry_exported_function_invoked_v12", &mut mint, oe33);
    backward_compatible("oplog_entry_start_span", &mut mint, oe34);
    backward_compatible("oplog_entry_finish_span", &mut mint, oe35);
    backward_compatible("oplog_entry_set_span_attribute", &mut mint, oe36);
}

#[test]
#[test_r::non_flaky(100)]
pub async fn timestamped_worker_invocation() {
    let timestamp = Timestamp::from(1724701938466);
    let root_span = InvocationContextSpan::local()
        .with_start(timestamp)
        .with_span_id(SpanId(NonZeroU64::new(4567).unwrap()))
        .build();
    root_span.set_attribute(
        "key".to_string(),
        AttributeValue::String("value".to_string()),
    );
    let invocation_context = InvocationContextStack::new(
        TraceId(NonZeroU128::new(1234).unwrap()),
        root_span,
        vec!["x".to_string(), "y".to_string()],
    );

    let twi3 = TimestampedWorkerInvocation {
        timestamp: Timestamp::from(1724701938466),
        invocation: WorkerInvocation::ExportedFunction {
            idempotency_key: IdempotencyKey {
                value: "idempotency_key".to_string(),
            },
            full_function_name: "function-name".to_string(),
            function_input: vec![Value::Bool(true)],
            invocation_context,
        },
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "timestamped_worker_invocation_exported_function_v1_2",
        &mut mint,
        twi3,
    );
}
