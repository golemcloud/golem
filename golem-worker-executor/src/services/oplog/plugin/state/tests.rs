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
use test_r::test;

use crate::services::oplog::retention::test_oplog::{at, idx};

/// The batch from `start` to `last`, both included.
fn batch(start: u64, last: u64) -> OplogRange {
    OplogRange {
        start: idx(start),
        count: last + 1 - start,
    }
}

/// A cut does not pass the confirmed index of a plugin, so a repeated batch starts at or after
/// the retained start.
#[test]
fn a_repeated_batch_keeps_the_range_that_was_sent() {
    assert_eq!(
        next_batch(idx(3), idx(5), idx(4), idx(10), at(4)),
        Some((batch(4, 5), true))
    );
    assert_eq!(
        next_batch(idx(3), idx(4), idx(4), idx(10), OplogRetention::FULL),
        Some((batch(4, 4), true))
    );
}

#[test]
fn a_new_batch_starts_after_the_confirmed_index_or_at_the_retained_start() {
    assert_eq!(
        next_batch(idx(3), idx(3), idx(2), idx(10), OplogRetention::FULL),
        Some((batch(4, 10), false))
    );
    assert_eq!(
        next_batch(idx(3), idx(3), idx(2), idx(10), at(4)),
        Some((batch(4, 10), false))
    );
    assert_eq!(
        next_batch(idx(3), idx(3), idx(2), idx(10), at(5)),
        Some((batch(5, 10), false))
    );
    assert_eq!(
        next_batch(idx(3), idx(3), idx(2), idx(10), at(10)),
        Some((batch(10, 10), false))
    );
    assert_eq!(next_batch(idx(3), idx(3), idx(2), idx(10), at(11)), None);
    assert_eq!(
        next_batch(idx(9), idx(9), idx(8), idx(10), OplogRetention::FULL),
        Some((batch(10, 10), false))
    );
    assert_eq!(
        next_batch(idx(10), idx(10), idx(8), idx(10), OplogRetention::FULL),
        None
    );
    assert_eq!(
        next_batch(idx(11), idx(11), idx(8), idx(10), OplogRetention::FULL),
        None
    );
}
