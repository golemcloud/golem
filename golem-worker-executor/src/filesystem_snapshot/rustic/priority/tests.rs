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

use super::{LOW_PRIORITY, at_low_priority, on_own_thread, own_nice};
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
fn work_at_low_priority_runs_at_nice_19_on_its_own_thread_and_the_caller_keeps_its_priority() {
    let before = own_nice();

    let inside = at_low_priority("fs-snap-test", || {
        Ok((
            own_nice(),
            std::thread::current().name().map(str::to_string),
        ))
    });

    assert_eq!(
        (inside.ok(), own_nice()),
        (
            Some((LOW_PRIORITY, Some("fs-snap-test".to_string()))),
            before
        )
    );
}

#[test]
fn work_whose_priority_cannot_be_lowered_still_runs_at_the_normal_priority() {
    let before = own_nice();

    let done = on_own_thread(
        "fs-snap-test",
        || Ok(own_nice()),
        || Err(std::io::Error::other("the priority cannot change here")),
    );

    assert_eq!(done.ok(), Some(before));
}

#[test]
fn a_panic_of_the_work_gives_an_error() {
    let done = on_own_thread::<()>("fs-snap-test", || panic!("the work panics"), || Ok(()));

    assert!(done.is_err());
}
