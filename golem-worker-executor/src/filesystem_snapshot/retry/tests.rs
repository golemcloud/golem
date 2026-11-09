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

use super::run_delay;
use crate::filesystem_snapshot::test_support::retry_script as retry;
use golem_common::model::RetryConfig;
use pretty_assertions::assert_eq;
use std::time::Duration;
use test_r::test;

#[test]
fn only_a_failed_call_within_the_retries_gets_a_wait() {
    let retry = RetryConfig {
        max_attempts: 3,
        ..retry()
    };
    assert_eq!(
        [
            run_delay(&retry, 1, 0.0),
            run_delay(&retry, 2, 0.0),
            run_delay(&retry, 3, 0.0)
        ],
        [
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(8)),
            None
        ]
    );
}

#[test]
fn a_jitter_that_is_negative_not_a_number_or_huge_never_panics() {
    let retry = RetryConfig {
        max_delay: Duration::from_secs(20),
        max_jitter_factor: Some(1.0),
        ..retry()
    };
    assert_eq!(
        [-0.5, f64::NAN, 1e300, f64::INFINITY].map(|jitter| run_delay(&retry, 1, jitter)),
        [
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(20)),
            Some(Duration::from_secs(20))
        ]
    );
}

#[test]
fn the_drawn_jitter_grows_the_wait_up_to_the_largest_wait() {
    let retry = RetryConfig {
        max_delay: Duration::from_secs(20),
        max_jitter_factor: Some(0.5),
        ..retry()
    };
    assert_eq!(
        [
            run_delay(&retry, 1, 0.25),
            run_delay(&retry, 2, 0.5),
            run_delay(&retry, 3, 0.1)
        ],
        [
            Some(Duration::from_millis(2500)),
            Some(Duration::from_secs(12)),
            Some(Duration::from_secs(20))
        ]
    );
}
