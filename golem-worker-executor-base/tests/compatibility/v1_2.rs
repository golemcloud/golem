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
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::OplogRegion;
use golem_common::model::{IdempotencyKey, OplogIndex, Timestamp};
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
