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

use super::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::pruning::eligibility::Percent;
use crate::filesystem_snapshot::backend::rustic::test_support::config;
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use std::time::Duration;
use test_r::test;

#[test]
fn the_policy_takes_the_configured_values() {
    let policy = StorePolicy::from_config(&config());

    assert_eq!(
        (
            policy.deadline,
            policy.save_threads.map(NonZeroUsize::get),
            policy.restore_reader_threads.get(),
            policy.prune.keep_delete,
            policy.prune.fast_repack,
            policy.prune.threshold,
        ),
        (
            Duration::from_secs(30),
            Some(3),
            4,
            Duration::from_secs(15 * 60),
            true,
            Percent(10),
        )
    );
}
