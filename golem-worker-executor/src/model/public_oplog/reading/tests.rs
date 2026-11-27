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
use std::num::NonZeroU64;
use test_r::test;

#[test]
fn a_sub_millisecond_configured_scan_budget_is_not_spent_at_zero_elapsed_time() {
    let mut scan = RangeScan::within(ScanBudget {
        entries: NonZeroU64::new(2).unwrap(),
        time: Duration::from_micros(750),
    });
    // `Instant::elapsed` gives zero until this instant. The test does not race a timer.
    scan.started += Duration::from_secs(60);
    scan.charge(1);
    assert!(!scan.is_spent());
    scan.charge(1);
    assert!(scan.is_spent());
}
