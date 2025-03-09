// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
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
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    IdempotencyKey, OplogIndex, Timestamp, TimestampedWorkerInvocation, WorkerInvocation,
};
use golem_wasm_rpc::Value;
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

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_revert", &mut mint, oe31);
    backward_compatible("oplog_entry_cancel_pending_invocation", &mut mint, oe32);
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
