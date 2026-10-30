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

use super::{OperationCohort, OperationDrain, ends_entry};
use crate::filesystem_snapshot::test_support::new_scope;
use futures::FutureExt;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use test_r::test;
use tokio_util::task::TaskTracker;

/// The longest time that a test waits for a drain that must end.
const LIMIT: Duration = Duration::from_secs(10);

fn works() -> OperationDrain {
    OperationDrain::default()
}

/// A work outside of each map, for the pure rule.
fn lone_work() -> Arc<OperationCohort> {
    Arc::new(OperationCohort {
        tracker: TaskTracker::new(),
        registry: std::sync::Weak::new(),
        agent: new_scope(),
        successor: std::sync::OnceLock::new(),
    })
}

/// Whether the entry of the agent in the map is the work.
fn entry_is(
    works: &OperationDrain,
    agent: &crate::filesystem_snapshot::AgentSnapshots,
    work: &Arc<OperationCohort>,
) -> bool {
    works
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(agent)
        .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), Arc::as_ptr(work)))
}

fn entries(works: &OperationDrain) -> usize {
    works
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

#[test]
fn the_end_of_a_work_removes_only_an_entry_that_is_still_this_work() {
    let (this, other) = (lone_work(), lone_work());
    let (own, newer) = (Arc::downgrade(&this), Arc::downgrade(&other));

    assert_eq!(
        (
            ends_entry(Some(&own), Arc::as_ptr(&this)),
            ends_entry(Some(&newer), Arc::as_ptr(&this)),
            ends_entry(None, Arc::as_ptr(&this)),
        ),
        (true, false, false)
    );
}

#[test]
fn the_operations_of_an_incarnation_share_one_work_and_others_do_not() {
    let works = works();
    let (agent, other) = (new_scope(), new_scope());

    let first = works.begin(&agent);
    let second = works.begin(&agent);
    let third = works.begin(&other);

    assert_eq!(
        (
            Arc::ptr_eq(&first.cohort, &second.cohort),
            Arc::ptr_eq(&first.cohort, &third.cohort),
            entries(&works),
        ),
        (true, false, 2)
    );
}

#[test]
fn the_end_of_the_last_operation_removes_the_entry_of_its_incarnation() {
    let works = works();
    let agent = new_scope();
    let operation = works.begin(&agent);
    let task = operation.clone();

    drop(operation);
    let while_a_task_runs = entries(&works);
    drop(task);

    assert_eq!((while_a_task_runs, entries(&works)), (1, 0));
}

#[test]
fn an_end_of_an_old_cohort_never_removes_a_newer_entry() {
    let works = works();
    let agent = new_scope();
    let old = works.begin(&agent);
    let mut drained = std::pin::pin!(works.drain_before(&agent));
    let while_old_runs = (&mut drained).now_or_never().is_some();
    let new = works.begin(&agent);

    drop(old);
    let own = (&mut drained).now_or_never();

    assert_eq!(
        (
            while_old_runs,
            own.is_some_and(|own| std::sync::Arc::ptr_eq(&own.cohort, &new.cohort)),
            entry_is(&works, &agent, &new.cohort),
        ),
        (false, true, true)
    );
}

#[test]
fn a_second_drain_waits_for_the_first_drain_and_for_the_work_before_it() {
    let works = works();
    let agent = new_scope();
    let before = works.begin(&agent);
    let mut first = std::pin::pin!(works.drain_before(&agent));
    let first_waits = (&mut first).now_or_never().is_none();
    let mut second = std::pin::pin!(works.drain_before(&agent));
    let second_waits = (&mut second).now_or_never().is_none();

    drop(before);
    let first_work = (&mut first).now_or_never();
    let second_waits_for_the_first = (&mut second).now_or_never().is_none();
    let first_ended = first_work.is_some();
    drop(first_work);
    let second_ended = (&mut second).now_or_never().is_some();

    assert_eq!(
        (
            first_waits,
            second_waits,
            first_ended,
            second_waits_for_the_first,
            second_ended
        ),
        (true, true, true, true, true)
    );
}

#[test]
async fn a_drain_waits_for_each_operation_that_began_before_it_and_for_none_after() {
    let works = works();
    let agent = new_scope();
    let before = works.begin(&agent);
    let task = before.clone();
    drop(before);
    let mut drained = std::pin::pin!(works.drain_before(&agent));

    let while_a_task_runs = (&mut drained).now_or_never().is_some();
    let after = works.begin(&agent);
    drop(task);
    let ended = tokio::time::timeout(LIMIT, drained).await.is_ok();

    assert_eq!(
        (
            while_a_task_runs,
            ended,
            entry_is(&works, &agent, &after.cohort)
        ),
        (false, true, true)
    );
}

#[test]
async fn a_drain_of_an_incarnation_without_work_ends_at_once() {
    let works = works();

    let ended = works.drain_before(&new_scope()).now_or_never().is_some();

    assert!(ended);
}

/// The drain holds the last strong reference of the work when the operation ends during its wait,
/// so the `Drop` of the work runs in the drain. It locks the map, so a drop under the lock of the
/// map never ends. The drain runs on a thread of its own, so such a drop fails the test by the
/// limit and does not hang it.
#[test]
fn no_cohort_is_dropped_under_the_map_mutex() {
    let works = works();
    let agent = new_scope();
    let operation = works.begin(&agent);
    let (ended, ends) = std::sync::mpsc::channel();
    let drained = {
        let (works, agent) = (works.clone(), agent.clone());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            drop(runtime.block_on(works.drain_before(&agent)));
            let _ = ended.send(());
        })
    };

    let waited_while_alive = ends.recv_timeout(Duration::from_millis(100)).is_err();
    drop(operation);
    let drained_after = ends.recv_timeout(LIMIT).is_ok();
    // A drain that never ended can hold the lock, so the test only tries it.
    let entries_after = works.entries.try_lock().ok().map(|entries| entries.len());

    assert_eq!(
        (waited_while_alive, drained_after, entries_after),
        (true, true, Some(0))
    );
    drained.join().unwrap();
}

/// A drain that its caller drops while the work before it runs does not let a later drain end
/// before that work: when the drop comes before the later drain registers, and when it comes
/// after.
#[test]
fn a_dropped_drain_keeps_the_work_before_it_for_a_later_drain() {
    let outcomes = [false, true].map(|second_first| {
        let works = works();
        let agent = new_scope();
        let before = works.begin(&agent);
        let mut first = Box::pin(works.drain_before(&agent));
        let first_waits = (&mut first).now_or_never().is_none();
        let mut second = if second_first {
            let second = Box::pin(works.drain_before(&agent));
            drop(first);
            second
        } else {
            drop(first);
            Box::pin(works.drain_before(&agent))
        };

        let second_waits = (&mut second).now_or_never().is_none();
        drop(before);
        let second_ended = second_waits && (&mut second).now_or_never().is_some();
        (first_waits, second_waits, second_ended)
    });

    assert_eq!(outcomes, [(true, true, true), (true, true, true)]);
}
