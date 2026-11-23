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

use super::LowPriority;
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use test_r::test;

fn seen() -> anyhow::Result<(Option<String>, bool, usize)> {
    Ok((
        std::thread::current().name().map(str::to_string),
        rayon::current_thread_index().is_some(),
        rayon::current_num_threads(),
    ))
}

#[test]
fn work_whose_priority_step_does_nothing_runs_on_its_own_thread_in_a_pool_of_its_own() {
    let low_priority = LowPriority {
        lower: || Ok(()),
        ..LowPriority::new(NonZeroUsize::new(2))
    };

    let inside = low_priority.run("fs-snap-test", seen);

    assert_eq!(
        inside.ok().map(|(name, in_pool, threads)| (
            name.is_some_and(|name| name.starts_with("fs-snap-test-")),
            in_pool,
            threads
        )),
        Some((true, true, 2))
    );
}

#[test]
fn work_whose_priority_cannot_be_lowered_still_runs_in_its_own_pool() {
    let low_priority = LowPriority {
        lower: || Err(std::io::Error::other("the priority cannot change here")),
        ..LowPriority::new(NonZeroUsize::new(2))
    };

    let inside = low_priority.run("fs-snap-test", seen);

    assert_eq!(
        inside.ok().map(|(_, in_pool, threads)| (in_pool, threads)),
        Some((true, 2))
    );
}

#[test]
fn a_panic_of_the_work_gives_an_error() {
    let done = LowPriority {
        lower: || Ok(()),
        ..LowPriority::new(NonZeroUsize::new(1))
    }
    .run::<()>("fs-snap-test", || panic!("the work panics"));

    assert!(done.is_err());
}
