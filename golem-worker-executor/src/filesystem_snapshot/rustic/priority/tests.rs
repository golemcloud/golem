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

use super::{LOW_PRIORITY, LowPriority, own_nice};
use pretty_assertions::assert_eq;
use rayon::{ThreadPool, ThreadPoolBuildError, ThreadPoolBuilder};
use std::num::NonZeroUsize;
use test_r::test;

/// What the work sees: its nice value, its thread name, whether it runs in a rayon pool, and
/// the thread count of the current rayon pool.
fn seen() -> anyhow::Result<(i32, Option<String>, bool, usize)> {
    Ok((
        own_nice(),
        std::thread::current().name().map(str::to_string),
        rayon::current_thread_index().is_some(),
        rayon::current_num_threads(),
    ))
}

/// A pool builder that cannot start a thread.
fn no_pool(_: &str, _: Option<NonZeroUsize>) -> Result<ThreadPool, ThreadPoolBuildError> {
    ThreadPoolBuilder::new()
        .num_threads(1)
        .spawn_handler(|_| Err(std::io::Error::other("no thread can start here")))
        .build()
}

#[test]
fn work_at_low_priority_runs_at_nice_19_in_a_pool_of_its_own_and_the_caller_keeps_its_priority() {
    let before = own_nice();

    let inside = LowPriority::new(NonZeroUsize::new(3)).run("fs-snap-test", seen);

    assert_eq!(
        (
            inside.ok().map(|(nice, name, in_pool, threads)| (
                nice,
                name.is_some_and(|name| name.starts_with("fs-snap-test-")),
                in_pool,
                threads
            )),
            own_nice()
        ),
        (Some((LOW_PRIORITY, true, true, 3)), before)
    );
}

#[test]
fn work_whose_priority_cannot_be_lowered_still_runs_at_the_normal_priority() {
    let before = own_nice();
    let low_priority = LowPriority {
        lower: || Err(std::io::Error::other("the priority cannot change here")),
        ..LowPriority::new(NonZeroUsize::new(2))
    };

    let inside = low_priority.run("fs-snap-test", seen);

    assert_eq!(
        inside.ok().map(|(nice, _, in_pool, _)| (nice, in_pool)),
        Some((before, true))
    );
}

#[test]
fn work_without_its_pool_still_runs_at_nice_19_on_its_own_thread() {
    let low_priority = LowPriority {
        build_pool: no_pool,
        ..LowPriority::new(NonZeroUsize::new(2))
    };

    let inside = low_priority.run("fs-snap-test", seen);

    assert_eq!(
        inside
            .ok()
            .map(|(nice, name, in_pool, _)| (nice, name, in_pool)),
        Some((LOW_PRIORITY, Some("fs-snap-test".to_string()), false))
    );
}

#[test]
fn a_panic_of_the_work_gives_an_error() {
    let done = LowPriority::new(NonZeroUsize::new(1))
        .run::<()>("fs-snap-test", || panic!("the work panics"));

    assert!(done.is_err());
}
