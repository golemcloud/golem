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

use super::super::ConfirmOutcome;
use super::*;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use std::collections::HashMap;
use test_r::test;

fn agent_snapshots(name: &str) -> AgentSnapshots {
    AgentSnapshots::agent(
        &OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: name.to_string(),
            },
        ),
        golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
    )
}

/// Runs the rule `rule` on the state in `state`, puts its next state back, and gives its
/// answer, as the registry does.
fn step<T>(state: &mut State, rule: impl FnOnce(State) -> Next<T>) -> T {
    let (next, answer) = rule(std::mem::take(state)).into_parts();
    *state = next;
    answer
}

fn saving(state: &mut State, agent: &AgentSnapshots, id: JobId) {
    step(state, |state| super::run_granted(state, agent, id));
}

fn granted(state: &mut State, agent: &AgentSnapshots, id: JobId) -> bool {
    step(state, |state| super::run_granted(state, agent, id))
}

/// The save call of the job `id` holds the save mark, as the call that runs the job's save
/// does, and its run failed: the call waits for its next run.
fn failed_run(state: &mut State, agent: &AgentSnapshots, id: JobId) {
    if !state.save_running.contains_key(agent) {
        begin(state, agent, &CallKind::Save { job: id });
    }
    step(state, |state| super::run_failed(state, agent, id))
}

fn decide(state: &mut State, agent: &AgentSnapshots, id: JobId, decision: JobDecision) {
    step(state, |state| super::decide(state, agent, id, decision))
}

fn delete_all_snapshots(state: &mut State, agent: &AgentSnapshots) -> Option<CancellationToken> {
    step(state, |state| super::request_all(state, agent)).stop
}

fn request_all(state: &mut State, agent: &AgentSnapshots) -> Requested {
    step(state, |state| super::request_all(state, agent))
}

fn request_names(state: &mut State, agent: &AgentSnapshots, names: &[SnapshotName]) -> Requested {
    step(state, |state| super::request_names(state, agent, names))
}

fn take(state: &mut State) -> Option<(AgentSnapshots, Work)> {
    step(state, take_ready)
}

fn ended_cleanup(state: &mut State, agent: &AgentSnapshots) {
    step(state, |state| cleanup_ended(state, agent))
}

fn begin(state: &mut State, agent: &AgentSnapshots, kind: &CallKind) -> bool {
    step(state, |state| begin_call(state, agent, kind))
}

fn ended_call(state: &mut State, agent: &AgentSnapshots, kind: &CallKind) {
    step(state, |state| store_call_ended(state, agent, kind))
}

/// Takes each ready agent and ends its work at once, until none is ready.
fn drain(state: &mut State) {
    if let Some((agent, _)) = take(state) {
        ended_cleanup(state, &agent);
        drain(state);
    }
}

fn snapshot_names(texts: &[&str]) -> Vec<SnapshotName> {
    texts
        .iter()
        .map(|text| SnapshotName::new(text).unwrap())
        .collect()
}

fn work_names(work: &Work) -> std::collections::BTreeSet<String> {
    match work {
        Work::Names(names) => names.iter().map(|name| name.as_str().to_string()).collect(),
        Work::All => std::collections::BTreeSet::from(["*".to_string()]),
    }
}

fn unwatch(state: &mut State, agent: &AgentSnapshots, id: JobId) {
    step(state, |state| super::unwatch(state, agent, id))
}

fn admit(
    state: &mut State,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
    stop: CancellationToken,
    retention_stop: CancellationToken,
    room: bool,
) -> Result<JobId, Refusal> {
    admit_kind(
        state,
        agent,
        name,
        SnapshotKind::Periodic,
        stop,
        retention_stop,
        room,
    )
    .map(|admitted| admitted.id)
}

fn admit_kind(
    state: &mut State,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
    kind: SnapshotKind,
    stop: CancellationToken,
    retention_stop: CancellationToken,
    room: bool,
) -> Result<Admitted, Refusal> {
    step(state, |state| {
        super::admit(state, agent, name, kind, stop, retention_stop, room)
    })
}

fn try_admit(
    state: &mut State,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
    room: bool,
) -> Result<JobId, Refusal> {
    admit(
        state,
        agent,
        name,
        CancellationToken::new(),
        CancellationToken::new(),
        room,
    )
}

fn admitted(state: &mut State, agent: &AgentSnapshots, name: &FilesystemSnapshotName) -> JobId {
    try_admit(state, agent, name, true).expect("admitted")
}

fn refusal(result: Result<JobId, Refusal>) -> Option<(SnapshotSkip, Option<JobId>)> {
    result
        .err()
        .map(|refusal| (refusal.skip, refusal.running.map(|running| running.id)))
}

fn end_job(state: &mut State, agent: &AgentSnapshots, id: JobId) {
    step(state, |state| end(state, agent, id))
}

fn wait(
    state: &mut State,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
) -> Result<JobId, Option<JobDecision>> {
    step(state, |state| start_wait(state, agent, name))
}

#[test]
fn a_clean_up_entry_has_its_measured_size() {
    // The map entry of a clean-up: the key, which shares the namespace of the agent, and the
    // entry. The pending names of the entry live on the heap, counted by `pending_names`.
    assert_eq!(
        (
            std::mem::size_of::<Cleanup>(),
            std::mem::size_of::<(AgentSnapshots, Cleanup)>(),
            std::mem::size_of::<SnapshotName>(),
        ),
        (80, 88, 16)
    );
}

#[test]
fn a_shutdown_loses_the_pending_and_the_running_clean_ups() {
    let mut state = State::default();
    let (pending, running, both) = (
        agent_snapshots("pending"),
        agent_snapshots("running"),
        agent_snapshots("both"),
    );
    let name = || SnapshotName::new(FilesystemSnapshotName::periodic().as_str()).unwrap();
    request_names(&mut state, &running, &[name()]);
    request_names(&mut state, &both, &[name()]);
    let taken = [take(&mut state), take(&mut state)].map(|taken| taken.is_some());
    request_names(&mut state, &both, &[name()]);
    request_names(&mut state, &pending, &[name()]);

    assert_eq!(
        (taken, cleanups_lost_at_shutdown(&state)),
        ([true, true], 4)
    );
}

#[test]
fn a_shutdown_loses_one_pending_and_two_running_clean_ups() {
    let mut state = State::default();
    let (pending, first, second) = (
        agent_snapshots("pending"),
        agent_snapshots("first-running"),
        agent_snapshots("second-running"),
    );
    let name = || SnapshotName::new(FilesystemSnapshotName::periodic().as_str()).unwrap();
    request_names(&mut state, &first, &[name()]);
    request_names(&mut state, &second, &[name()]);
    let taken = [take(&mut state), take(&mut state)].map(|taken| taken.is_some());
    request_names(&mut state, &pending, &[name()]);

    assert_eq!(
        (taken, cleanups_lost_at_shutdown(&state)),
        ([true, true], 3)
    );
}

#[test]
fn a_replacing_admission_queues_the_agent_that_the_replaced_job_held() {
    let mut state = State::default();
    let agent = agent_snapshots("agent");
    let (first, second) = (
        FilesystemSnapshotName::periodic(),
        FilesystemSnapshotName::periodic(),
    );
    let old = admitted(&mut state, &agent, &first);
    granted(&mut state, &agent, old);
    failed_run(&mut state, &agent, old);
    request_names(
        &mut state,
        &agent,
        &[SnapshotName::new(first.as_str()).unwrap()],
    );
    let held = take(&mut state).is_none();

    admitted(&mut state, &agent, &second);
    // The save call of the replaced job runs on as a tail until it returns.
    let while_the_tail_runs = take(&mut state).is_none();
    ended_call(&mut state, &agent, &CallKind::Save { job: old });
    let taken = take(&mut state).map(|(taken, _)| taken);

    assert_eq!(
        (held, while_the_tail_runs, taken),
        (true, true, Some(agent))
    );
}

#[test]
fn an_admission_checks_room_then_a_delete_of_all_snapshots_then_a_running_job() {
    let mut state = State::default();
    let agent = agent_snapshots("order");
    let other = agent_snapshots("other");
    let name = FilesystemSnapshotName::periodic();

    delete_all_snapshots(&mut state, &agent);
    let deleting_without_room = refusal(try_admit(&mut state, &agent, &name, false));
    let deleting = refusal(try_admit(&mut state, &agent, &name, true));
    let first = admitted(&mut state, &other, &name);
    let running = refusal(try_admit(&mut state, &other, &name, true));
    let running_without_room = refusal(try_admit(&mut state, &other, &name, false));

    assert_eq!(
        deleting_without_room,
        Some((SnapshotSkip::VolumeUnderPressure, None))
    );
    assert_eq!(deleting, Some((SnapshotSkip::DeletingAllSnapshots, None)));
    assert_eq!(running, Some((SnapshotSkip::UploadInFlight, Some(first))));
    assert_eq!(
        running_without_room,
        Some((SnapshotSkip::VolumeUnderPressure, Some(first)))
    );
}

#[test]
fn a_job_number_is_never_given_twice_and_an_end_frees_only_its_own_job() {
    let mut state = State::default();
    let agent = agent_snapshots("numbers");
    let name = FilesystemSnapshotName::periodic();

    let first = admitted(&mut state, &agent, &name);
    end_job(&mut state, &agent, first);
    let second = admitted(&mut state, &agent, &name);
    end_job(&mut state, &agent, first);
    let still_running = !is_free(&state, &agent);
    end_job(&mut state, &agent, second);

    assert!(second > first);
    assert!(still_running);
    assert!(is_free(&state, &agent));
    assert!(has_ended(&state, &agent, second));
}

#[test]
fn the_phase_only_moves_forward_and_the_first_decision_stays() {
    let mut state = State::default();
    let agent = agent_snapshots("phase");
    let name = FilesystemSnapshotName::periodic();
    let id = admitted(&mut state, &agent, &name);

    let admitted_decision = decision_of(&state, &agent, id);
    saving(&mut state, &agent, id);
    let saving_decision = decision_of(&state, &agent, id);
    decide(&mut state, &agent, id, JobDecision::SaveFailed);
    decide(
        &mut state,
        &agent,
        id,
        JobDecision::Confirmed(ConfirmOutcome::Confirmed),
    );
    saving(&mut state, &agent, id);
    let decided = decision_of(&state, &agent, id);
    let phase = state.jobs.get(&agent).map(|job| job.phase);

    assert_eq!(
        (admitted_decision, saving_decision, decided, phase),
        (
            None,
            None,
            Some(JobDecision::SaveFailed),
            Some(JobPhase::Decided(JobDecision::SaveFailed))
        )
    );
}

#[test]
fn a_start_waits_only_for_a_saving_undecided_job_with_its_name() {
    let mut state = State::default();
    let agent = agent_snapshots("start-wait");
    let name = FilesystemSnapshotName::periodic();
    let other = FilesystemSnapshotName::periodic();

    let no_job = wait(&mut state, &agent, &name);
    let id = admitted(&mut state, &agent, &name);
    let while_admitted = wait(&mut state, &agent, &name);
    saving(&mut state, &agent, id);
    let other_name = wait(&mut state, &agent, &other);
    let while_saving = wait(&mut state, &agent, &name);
    let waiters = state.jobs.get(&agent).map(|job| job.waiters);
    decide(
        &mut state,
        &agent,
        id,
        JobDecision::Confirmed(ConfirmOutcome::Deferred),
    );
    let decided = wait(&mut state, &agent, &name);

    assert_eq!(no_job, Err(None));
    assert_eq!(while_admitted, Err(None));
    assert_eq!(other_name, Err(None));
    assert_eq!(while_saving, Ok(id));
    assert_eq!(waiters, Some(1));
    assert_eq!(
        decided,
        Err(Some(JobDecision::Confirmed(ConfirmOutcome::Deferred)))
    );
}

#[test]
fn an_ended_job_keeps_its_decision_only_while_a_start_waits_for_it() {
    let mut state = State::default();
    let agent = agent_snapshots("ended");
    let name = FilesystemSnapshotName::periodic();

    let unwatched = admitted(&mut state, &agent, &name);
    saving(&mut state, &agent, unwatched);
    decide(
        &mut state,
        &agent,
        unwatched,
        JobDecision::Confirmed(ConfirmOutcome::Superseded),
    );
    end_job(&mut state, &agent, unwatched);
    let without_waiter = (state.ended.len(), decision_of(&state, &agent, unwatched));

    let watched = admitted(&mut state, &agent, &name);
    saving(&mut state, &agent, watched);
    assert!(wait(&mut state, &agent, &name).is_ok());
    assert!(wait(&mut state, &agent, &name).is_ok());
    decide(
        &mut state,
        &agent,
        watched,
        JobDecision::Confirmed(ConfirmOutcome::Superseded),
    );
    end_job(&mut state, &agent, watched);
    let kept = decision_of(&state, &agent, watched);
    unwatch(&mut state, &agent, unwatched);
    unwatch(&mut state, &agent, watched);
    let after_one = state.ended.len();
    unwatch(&mut state, &agent, watched);

    assert_eq!(without_waiter, (0, Some(JobDecision::Stopped)));
    assert_eq!(
        kept,
        Some(JobDecision::Confirmed(ConfirmOutcome::Superseded))
    );
    assert_eq!(after_one, 1);
    assert!(state.ended.is_empty());
}

#[test]
fn an_undecided_job_that_ends_with_a_waiter_keeps_stopped() {
    let mut state = State::default();
    let agent = agent_snapshots("stopped");
    let name = FilesystemSnapshotName::periodic();
    let id = admitted(&mut state, &agent, &name);
    saving(&mut state, &agent, id);
    assert!(wait(&mut state, &agent, &name).is_ok());

    end_job(&mut state, &agent, id);

    assert_eq!(
        state.ended.get(&agent).map(|ended| ended.decision),
        Some(JobDecision::Stopped)
    );
}

#[test]
fn an_unwatch_of_a_live_job_takes_one_waiter_and_a_stale_one_does_nothing() {
    let mut state = State::default();
    let agent = agent_snapshots("unwatch");
    let name = FilesystemSnapshotName::periodic();
    let id = admitted(&mut state, &agent, &name);
    saving(&mut state, &agent, id);
    assert!(wait(&mut state, &agent, &name).is_ok());
    assert!(wait(&mut state, &agent, &name).is_ok());

    unwatch(&mut state, &agent, id + 1);
    let after_stale = state.jobs.get(&agent).map(|job| job.waiters);
    unwatch(&mut state, &agent, id);

    assert_eq!(after_stale, Some(2));
    assert_eq!(state.jobs.get(&agent).map(|job| job.waiters), Some(1));
}

#[test]
fn a_delete_of_all_snapshots_refuses_admissions_until_its_work_ends_and_drops_the_ended_decision() {
    let mut state = State::default();
    let agent = agent_snapshots("deletes");
    let name = FilesystemSnapshotName::periodic();
    let id = admitted(&mut state, &agent, &name);
    saving(&mut state, &agent, id);
    assert!(wait(&mut state, &agent, &name).is_ok());
    let stop = state.jobs.get(&agent).map(|job| job.stop.clone());

    let first = delete_all_snapshots(&mut state, &agent);
    let while_the_job_runs = take(&mut state);
    end_job(&mut state, &agent, id);
    let second = delete_all_snapshots(&mut state, &agent);
    let taken = take(&mut state);
    let while_running = (
        refusal(try_admit(&mut state, &agent, &name, true)),
        state.ended.len(),
    );
    ended_cleanup(&mut state, &agent);
    let after = try_admit(&mut state, &agent, &name, true).is_ok();

    assert!(first.is_some_and(|token| Some(&token) == stop.as_ref()));
    assert!(second.is_none());
    assert!(while_the_job_runs.is_none());
    assert_eq!(taken.map(|(_, work)| work), Some(Work::All));
    assert_eq!(
        while_running,
        (Some((SnapshotSkip::DeletingAllSnapshots, None)), 1)
    );
    assert!(after);
    assert!(state.ended.is_empty());
    assert!(state.cleanups.is_empty());
}

#[test]
fn only_transitions_that_end_or_add_work_wake_the_waiters() {
    let agent = agent_snapshots("wakes");
    let name = FilesystemSnapshotName::periodic();
    let names = snapshot_names(&["p-1"]);
    let flight = (
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "target".to_string(),
        },
        [0; 32],
    );
    let fresh = State::default;

    assert_eq!(
        [
            super::admit(
                fresh(),
                &agent,
                &name,
                SnapshotKind::Periodic,
                CancellationToken::new(),
                CancellationToken::new(),
                true
            )
            .wakes(),
            super::run_granted(fresh(), &agent, 1).wakes(),
            super::run_failed(fresh(), &agent, 1).wakes(),
            super::run_waiting_for_late_writes(fresh(), &agent, 1).wakes(),
            replacing(&agent, &name).wakes(),
            super::decide(fresh(), &agent, 1, JobDecision::Stopped).wakes(),
            end(fresh(), &agent, 1).wakes(),
            start_wait(fresh(), &agent, &name).wakes(),
            super::unwatch(fresh(), &agent, 1).wakes(),
            super::request_names(fresh(), &agent, &names).wakes(),
            super::request_all(fresh(), &agent).wakes(),
            take_ready(fresh()).wakes(),
            cleanup_ended(fresh(), &agent).wakes(),
            begin_call(fresh(), &agent, &CallKind::Other).wakes(),
            store_call_ended(
                begin_call(fresh(), &agent, &CallKind::Other).into_parts().0,
                &agent,
                &CallKind::Other
            )
            .wakes(),
            fork_began(fresh(), &agent, &flight).wakes(),
            fork_publishing(fresh(), &flight).wakes(),
            fork_ended(
                fork_began(fresh(), &agent, &flight).into_parts().0,
                &agent,
                &flight,
                Some(&agent),
                Some(ForkOutcome::Published)
            )
            .wakes(),
            super::revert_began(fresh(), &agent).wakes(),
            super::revert_ended(super::revert_began(fresh(), &agent).into_parts().0, &agent)
                .wakes(),
        ],
        [
            false, false, true, true, true, true, true, false, false, true, true, false, true,
            false, true, false, false, true, false, false
        ]
    );
}

/// Admits a periodic job for `agent` and gives whether the deletes of the new job must stop.
fn admitted_under_revert(state: &mut State, agent: &AgentSnapshots) -> Option<bool> {
    admit_kind(
        state,
        agent,
        &FilesystemSnapshotName::periodic(),
        SnapshotKind::Periodic,
        CancellationToken::new(),
        CancellationToken::new(),
        true,
    )
    .ok()
    .map(|admitted| admitted.under_revert)
}

#[test]
fn a_revert_hold_stops_the_deletes_of_the_running_job_and_of_each_job_admitted_while_it_lives() {
    let mut state = State::default();
    let agent = agent_snapshots("revert-hold");
    let other = agent_snapshots("revert-hold-other");
    let retention_stop = CancellationToken::new();
    let running = admit(
        &mut state,
        &agent,
        &FilesystemSnapshotName::periodic(),
        CancellationToken::new(),
        retention_stop.clone(),
        true,
    )
    .unwrap();
    let before = step(&mut state, |state| revert_began(state, &other));

    let held = step(&mut state, |state| revert_began(state, &agent));
    end_job(&mut state, &agent, running);
    let during = admitted_under_revert(&mut state, &agent);
    let other_during = admitted_under_revert(&mut state, &other);
    step(&mut state, |state| revert_ended(state, &agent));
    let id = state.jobs.get(&agent).map(|job| job.id).unwrap();
    end_job(&mut state, &agent, id);
    let after = admitted_under_revert(&mut state, &agent);

    assert_eq!(
        (
            before.is_none(),
            held.is_some_and(|stop| stop == retention_stop),
            during,
            other_during,
            after
        ),
        (true, true, Some(true), Some(true), Some(false))
    );
}

#[test]
fn two_reverts_hold_until_both_end_and_an_end_without_a_hold_changes_nothing() {
    let mut state = State::default();
    let agent = agent_snapshots("two-reverts");
    step(&mut state, |state| revert_ended(state, &agent));
    step(&mut state, |state| revert_began(state, &agent));
    step(&mut state, |state| revert_began(state, &agent));

    step(&mut state, |state| revert_ended(state, &agent));
    let after_one = state.reverts.contains_key(&agent);
    step(&mut state, |state| revert_ended(state, &agent));
    let after_two = state.reverts.contains_key(&agent);
    step(&mut state, |state| revert_ended(state, &agent));

    assert_eq!(
        (after_one, after_two, state.reverts.is_empty()),
        (true, false, true)
    );
}

#[test]
fn a_burst_of_deletes_of_one_agent_merges_into_one_pending_clean_up() {
    let mut state = State::default();
    let agent = agent_snapshots("burst");

    request_names(&mut state, &agent, &snapshot_names(&["p-1", "p-2"]));
    request_names(&mut state, &agent, &snapshot_names(&["p-2", "p-3"]));
    request_names(&mut state, &agent, &[]);
    let entries = (state.pending_entries, state.pending_names_total);
    let taken = take(&mut state);

    assert_eq!(entries, (1, 3));
    assert_eq!(
        taken.map(|(_, work)| work_names(&work)),
        Some(
            ["p-1", "p-2", "p-3"]
                .map(String::from)
                .into_iter()
                .collect()
        )
    );
    assert!(take(&mut state).is_none());
    assert_eq!((state.pending_entries, state.pending_names_total), (0, 0));
}

#[test]
fn an_empty_delete_of_names_makes_no_entry() {
    let mut state = State::default();
    let agent = agent_snapshots("empty");

    let requested = request_names(&mut state, &agent, &[]);

    assert!(requested.stop.is_none() && requested.overflow.is_none());
    assert!(state.cleanups.is_empty() && state.ready.is_empty());
}

#[test]
fn a_delete_of_all_snapshots_replaces_the_pending_names_of_its_agent() {
    let mut state = State::default();
    let agent = agent_snapshots("replaced");

    request_names(&mut state, &agent, &snapshot_names(&["p-1", "p-2"]));
    request_all(&mut state, &agent);
    let ignored = request_names(&mut state, &agent, &snapshot_names(&["p-3"]));
    let counts = (
        state.pending_entries,
        state.pending_names_entries,
        state.pending_names_total,
    );

    assert_eq!(ignored.overflow, None);
    assert_eq!(counts, (1, 0, 0));
    assert_eq!(take(&mut state).map(|(_, work)| work), Some(Work::All));
}

#[test]
fn names_for_an_agent_whose_delete_of_all_runs_and_an_empty_request_at_the_limit_add_nothing() {
    let mut state = limited(1, 10, 10);
    let (running, full, empty) = (
        agent_snapshots("running-all"),
        agent_snapshots("fills-the-limit"),
        agent_snapshots("empty-request"),
    );
    request_all(&mut state, &running);
    let taken = take(&mut state).map(|(agent, work)| (agent == running, work));
    request_all(&mut state, &full);

    let names = request_names(&mut state, &running, &snapshot_names(&["p-1"]));
    let nothing = request_names(&mut state, &empty, &[]);

    assert_eq!(taken, Some((true, Work::All)));
    assert_eq!(
        (
            names.overflow,
            nothing.overflow,
            state
                .cleanups
                .get(&running)
                .map(|cleanup| cleanup.pending.clone()),
            state.cleanups.contains_key(&empty),
            (
                state.pending_entries,
                state.pending_names_entries,
                state.pending_names_total
            ),
        ),
        (None, None, Some(None), false, (1, 0, 0))
    );
}

#[test]
fn a_delete_of_all_for_an_agent_whose_clean_up_runs_counts_one_pending_entry() {
    let mut state = State::default();
    let agent = agent_snapshots("all-while-running");
    request_names(&mut state, &agent, &snapshot_names(&["p-1"]));
    let taken = take(&mut state).map(|(_, work)| work_names(&work));

    let requested = request_all(&mut state, &agent);

    assert_eq!(
        taken,
        Some(std::collections::BTreeSet::from(["p-1".to_string()]))
    );
    assert_eq!((requested.overflow, state.pending_entries), (None, 1));
}

#[test]
fn a_delete_of_names_stops_only_the_job_whose_name_it_holds() {
    let mut state = State::default();
    let agent = agent_snapshots("stops");
    let name = FilesystemSnapshotName::periodic();
    let own = SnapshotName::new(name.as_str()).unwrap();
    let id = admitted(&mut state, &agent, &name);

    let other = request_names(&mut state, &agent, &snapshot_names(&["p-other"]));
    let while_other_job = take(&mut state).map(|(_, work)| work_names(&work));
    ended_cleanup(&mut state, &agent);
    let own_name = request_names(&mut state, &agent, std::slice::from_ref(&own));
    let while_own_job = take(&mut state);
    end_job(&mut state, &agent, id);
    let after_the_job = take(&mut state).map(|(_, work)| work_names(&work));

    assert!(other.stop.is_none());
    assert_eq!(
        while_other_job,
        Some(std::collections::BTreeSet::from(["p-other".to_string()]))
    );
    assert!(own_name.stop.is_some());
    assert!(while_own_job.is_none());
    assert_eq!(
        after_the_job,
        Some(std::collections::BTreeSet::from([own.as_str().to_string()]))
    );
}

fn limited(pending_cleanups: usize, pending_names: usize, names_per_cleanup: usize) -> State {
    State::with_limits(Limits {
        pending_cleanups,
        pending_names,
        names_per_cleanup,
    })
}

#[test]
fn a_delete_of_all_snapshots_past_the_limit_evicts_pending_names() {
    let mut state = limited(2, 100, 10);
    let (first, second, third, fourth) = (
        agent_snapshots("first"),
        agent_snapshots("second"),
        agent_snapshots("third"),
        agent_snapshots("fourth"),
    );
    request_names(&mut state, &first, &snapshot_names(&["p-1", "p-2"]));
    request_names(&mut state, &second, &snapshot_names(&["p-3"]));

    let all = request_all(&mut state, &third);
    let names_past_the_limit = request_names(&mut state, &fourth, &snapshot_names(&["p-4"]));
    let all_again = request_all(&mut state, &fourth);
    let no_names_left = request_all(&mut state, &agent_snapshots("fifth"));
    let pending_all_again = request_all(&mut state, &third);

    assert_eq!(
        [
            all.overflow,
            names_past_the_limit.overflow,
            all_again.overflow,
            no_names_left.overflow,
            pending_all_again.overflow
        ],
        [
            Some(Overflow::Evicted(first.clone())),
            Some(Overflow::Refused),
            Some(Overflow::Evicted(second.clone())),
            Some(Overflow::Refused),
            None
        ]
    );
    assert_eq!(
        [&first, &second, &third, &fourth].map(|agent| state.cleanups.contains_key(agent)),
        [false, false, true, true]
    );
    assert_eq!(
        (
            state.pending_entries,
            state.pending_names_entries,
            state.pending_names_total
        ),
        (2, 0, 0)
    );
}

#[test]
fn names_past_the_limit_of_one_entry_are_counted_as_leaked() {
    let mut state = limited(1, 100, 2);
    let agent = agent_snapshots("one-entry");

    let first = request_names(&mut state, &agent, &snapshot_names(&["p-3", "p-2", "p-1"]));
    let again = request_names(&mut state, &agent, &snapshot_names(&["p-3"]));

    assert_eq!(
        (first.overflow, again.overflow),
        (Some(Overflow::Refused), None)
    );
    assert_eq!(
        take(&mut state).map(|(_, work)| work_names(&work)),
        Some(["p-3", "p-2"].map(String::from).into_iter().collect())
    );
}

#[test]
fn a_request_past_the_bound_of_one_agent_keeps_the_pending_names_and_refuses_the_new_ones() {
    let bound = crate::services::golem_config::FilesystemSnapshotUploadConfig::default()
        .max_pending_deletes_per_agent();
    let mut state = State::with_limits(Limits::new(bound));
    let agent = agent_snapshots("many-names");
    let texts = (0..bound.get())
        .map(|index| format!("p-{index}"))
        .collect::<Vec<_>>();
    let names = snapshot_names(&texts.iter().map(String::as_str).collect::<Vec<_>>());

    let fits = request_names(&mut state, &agent, &names);
    let past = request_names(&mut state, &agent, &snapshot_names(&["p-new"]));
    let pending = take(&mut state).map(|(_, work)| work_names(&work));

    assert_eq!(
        (bound.get(), fits.overflow, past.overflow),
        (1024, None, Some(Overflow::Refused))
    );
    assert_eq!(pending, Some(texts.into_iter().collect()));
}

#[test]
fn names_past_the_global_limit_are_counted_as_leaked() {
    let mut state = limited(10, 3, 10);
    let (first, second) = (agent_snapshots("first"), agent_snapshots("second"));

    let fits = request_names(&mut state, &first, &snapshot_names(&["p-1", "p-2"]));
    let past = request_names(&mut state, &second, &snapshot_names(&["p-3", "p-4"]));
    let none_left = request_names(
        &mut state,
        &agent_snapshots("third"),
        &snapshot_names(&["p-5"]),
    );

    assert_eq!(
        [fits.overflow, past.overflow, none_left.overflow],
        [None, Some(Overflow::Refused), Some(Overflow::Refused)]
    );
    assert_eq!(state.pending_names_total, 3);
    assert_eq!(state.pending_entries, 2);
}

#[test]
fn a_delete_of_all_at_the_bound_names_the_agent_whose_names_it_evicts() {
    let mut state = limited(2, 100, 10);
    let (first, second, source, stage) = (
        agent_snapshots("first"),
        agent_snapshots("second"),
        agent_snapshots("source"),
        agent_snapshots("stage"),
    );
    let flight = (
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "target".to_string(),
        },
        [9; 32],
    );
    request_names(&mut state, &first, &snapshot_names(&["p-1"]));
    request_names(&mut state, &second, &snapshot_names(&["p-2"]));
    step(&mut state, |state| fork_began(state, &source, &flight));

    let all = request_all(&mut state, &agent_snapshots("all"));
    let fork_end = step(&mut state, |state| {
        fork_ended(state, &source, &flight, Some(&stage), None)
    });

    assert_eq!(
        (all.overflow, fork_end),
        (
            Some(Overflow::Evicted(first)),
            ForkEnd::StageDeleted {
                overflow: Some(Overflow::Evicted(second))
            }
        )
    );
    assert!(all_requested(&state, &stage));
}

#[test]
fn a_delete_of_all_for_an_agent_whose_clean_up_runs_keeps_the_bound() {
    let (running, waiting) = (agent_snapshots("running"), agent_snapshots("waiting"));
    let running_names = |state: &mut State| {
        request_names(state, &running, &snapshot_names(&["p-1"]));
        take(state).map(|(agent, _)| agent)
    };
    let mut evicts = limited(1, 100, 10);
    let taken = running_names(&mut evicts);
    request_names(&mut evicts, &waiting, &snapshot_names(&["p-2"]));
    let mut refuses = limited(1, 100, 10);
    running_names(&mut refuses);
    request_all(&mut refuses, &waiting);

    let evicted = request_all(&mut evicts, &running);
    let refused = request_all(&mut refuses, &running);

    assert_eq!(taken, Some(running.clone()));
    assert_eq!(
        (
            evicted.overflow,
            evicts.pending_entries,
            all_requested(&evicts, &running)
        ),
        (Some(Overflow::Evicted(waiting)), 1, true)
    );
    assert_eq!(
        (
            refused.overflow,
            refuses.pending_entries,
            refuses
                .cleanups
                .get(&running)
                .map(|cleanup| (cleanup.pending.clone(), cleanup.running))
        ),
        (
            Some(Overflow::Refused),
            1,
            Some((None, Some(RunningWork::Names)))
        )
    );
}

#[test]
fn names_for_an_agent_whose_clean_up_runs_keep_the_bound() {
    let mut state = limited(1, 100, 10);
    let running = agent_snapshots("running");
    request_names(&mut state, &running, &snapshot_names(&["p-1"]));
    let taken = take(&mut state).map(|(agent, _)| agent);
    request_all(&mut state, &agent_snapshots("full"));

    let names = request_names(&mut state, &running, &snapshot_names(&["p-2"]));

    assert_eq!(taken, Some(running.clone()));
    assert_eq!(
        (
            names.overflow,
            state.pending_entries,
            state.pending_names_total,
            state
                .cleanups
                .get(&running)
                .map(|cleanup| cleanup.pending.clone())
        ),
        (Some(Overflow::Refused), 1, 0, Some(None))
    );
}

#[test]
fn names_order_stays_below_twice_the_pending_names_entries() {
    let mut state = limited(1000, 100_000, 10);
    let agents = (0..50)
        .map(|index| agent_snapshots(&format!("agent-{index}")))
        .collect::<Vec<_>>();
    let names = snapshot_names(&["p-1"]);

    let within = (0..20)
        .flat_map(|_| {
            let pushed = agents
                .iter()
                .map(|agent| {
                    request_names(&mut state, agent, &names);
                    state.names_order.len() <= 2 * state.pending_names_entries + 16
                })
                .collect::<Vec<_>>();
            agents.iter().skip(5).for_each(|agent| {
                request_all(&mut state, agent);
            });
            drain(&mut state);
            pushed
        })
        .collect::<Vec<_>>();

    assert!(within.iter().all(|within| *within));
}

#[test]
fn the_eviction_takes_the_oldest_pending_names_and_skips_an_older_entry_of_newer_names() {
    // A busy agent keeps its entry at the front. The names of `again` are taken, so its first
    // entry is stale behind the front; its new names come after the names of `newer`.
    let mut state = limited(3, 100, 10);
    let (front, again, newer) = (
        agent_snapshots("front"),
        agent_snapshots("again"),
        agent_snapshots("newer"),
    );
    let names = snapshot_names(&["p-1"]);
    request_names(&mut state, &front, &names);
    begin(&mut state, &front, &CallKind::Other);
    request_names(&mut state, &again, &names);
    let taken = take(&mut state).map(|(agent, _)| agent);
    request_names(&mut state, &newer, &names);
    request_names(&mut state, &again, &names);

    request_all(&mut state, &agent_snapshots("first-all"));
    request_all(&mut state, &agent_snapshots("second-all"));

    let holds_names = |agent: &AgentSnapshots| {
        state
            .cleanups
            .get(agent)
            .is_some_and(|cleanup| matches!(cleanup.pending, Some(Work::Names(_))))
    };
    assert_eq!(taken, Some(again.clone()));
    assert_eq!(
        [&front, &newer, &again].map(holds_names),
        [false, false, true]
    );
}

#[test]
fn names_order_stays_within_its_bound_when_the_live_entries_drop_before_a_push() {
    // A busy agent keeps its entry at the front, so the entries of the names that are taken
    // stay behind it while the live entries drop to one.
    let mut state = limited(1000, 100_000, 10);
    let front = agent_snapshots("front");
    let names = snapshot_names(&["p-1"]);
    request_names(&mut state, &front, &names);
    begin(&mut state, &front, &CallKind::Other);
    (0..30).for_each(|index| {
        request_names(
            &mut state,
            &agent_snapshots(&format!("agent-{index}")),
            &names,
        );
    });
    drain(&mut state);
    let before = (state.names_order.len(), state.pending_names_entries);

    request_names(&mut state, &agent_snapshots("pushed"), &names);

    assert_eq!(before, (31, 1));
    assert!(
        state.names_order.len() <= 2 * state.pending_names_entries + 16,
        "{} entries for {} live ones",
        state.names_order.len(),
        state.pending_names_entries
    );
}

#[test]
fn the_compaction_of_names_order_is_amortized() {
    let mut state = limited(100_000, 1_000_000, 10);
    let names = snapshot_names(&["p-1"]);
    // A live entry at the front keeps the stale entries behind it until a compaction.
    let front = agent_snapshots("front");
    let job = FilesystemSnapshotName::periodic();
    let blocked = SnapshotName::new(job.as_str()).unwrap();
    admitted(&mut state, &front, &job);
    request_names(&mut state, &front, std::slice::from_ref(&blocked));
    // Many live entries make each compaction cost much, so a threshold that does not grow
    // with twice the live entries compacts too often.
    (0..1000).for_each(|index| {
        request_names(
            &mut state,
            &agent_snapshots(&format!("live-{index}")),
            &names,
        );
    });
    let pushes = 10_000usize;

    let compacted = (0..pushes)
        .map(|index| {
            let agent = agent_snapshots(&format!("agent-{index}"));
            let before = state.names_order.len();
            request_names(&mut state, &agent, &names);
            let after = state.names_order.len();
            request_all(&mut state, &agent);
            // A push that left the order shorter than one more entry compacted it, at the
            // cost of the order before the compaction.
            if after <= before { before + 1 } else { 0 }
        })
        .sum::<usize>();

    assert!(
        compacted <= 3 * pushes,
        "the compactions read {compacted} entries for {pushes} pushes"
    );
    assert!(compacted > 0);
    assert!(state.names_order.len() <= 2 * state.pending_names_entries + 16);
}

#[test]
fn a_delete_of_all_snapshots_runs_when_the_tail_of_a_stopped_job_ends() {
    let mut state = State::default();
    let agent = agent_snapshots("tail");
    let name = FilesystemSnapshotName::periodic();
    let id = admitted(&mut state, &agent, &name);
    assert!(begin(&mut state, &agent, &CallKind::Save { job: 0 }));

    let stop = delete_all_snapshots(&mut state, &agent);
    end_job(&mut state, &agent, id);
    let while_the_save_runs = take(&mut state);
    let second_save = begin(&mut state, &agent, &CallKind::Save { job: 0 });
    ended_call(&mut state, &agent, &CallKind::Save { job: 0 });
    let after = take(&mut state).map(|(_, work)| work);

    assert!(stop.is_some());
    assert!(while_the_save_runs.is_none());
    assert!(!second_save);
    assert_eq!(after, Some(Work::All));
}

#[test]
fn a_delete_of_all_snapshots_starts_when_the_last_detached_call_of_its_agent_returns() {
    let mut state = State::default();
    let (agent, target) = (agent_snapshots("source"), agent_snapshots("target"));
    let copy = CallKind::Copy { to: target.clone() };
    assert!(begin(&mut state, &agent, &CallKind::Other));
    assert!(begin(&mut state, &agent, &copy));

    delete_all_snapshots(&mut state, &agent);
    delete_all_snapshots(&mut state, &target);
    ended_call(&mut state, &agent, &CallKind::Other);
    let after_one = take(&mut state);
    ended_call(&mut state, &agent, &copy);
    let after_both = std::iter::from_fn(|| take(&mut state))
        .map(|(agent, _)| agent)
        .collect::<std::collections::HashSet<_>>();

    assert!(after_one.is_none());
    assert_eq!(
        after_both,
        std::collections::HashSet::from([agent.clone(), target.clone()])
    );
    assert!(state.busy.is_empty());
}

/// The phase of the store call of a job in the table of [`replaceable`].
#[derive(Clone, Copy, Debug)]
enum Call {
    NotWaiting,
    AfterFailure,
    ForLateWrites,
}

/// Who holds the save mark of the agent in the table of [`replaceable`].
#[derive(Clone, Copy, Debug)]
enum Mark {
    Own,
    Other,
    Free,
}

/// Whether an admission replaces a job of `kind` whose run phase is `call`, which decided when
/// `decided`, while `mark` holds the save mark.
fn replaceable_case(kind: SnapshotKind, call: Call, decided: bool, mark: Mark) -> bool {
    let mut state = State::default();
    let agent = agent_snapshots("replaceable");
    let name = FilesystemSnapshotName::periodic();
    let id = admit_kind(
        &mut state,
        &agent,
        &name,
        kind,
        CancellationToken::new(),
        CancellationToken::new(),
        true,
    )
    .unwrap()
    .id;
    saving(&mut state, &agent, id);
    match call {
        Call::NotWaiting => {}
        Call::AfterFailure => step(&mut state, |state| super::run_failed(state, &agent, id)),
        Call::ForLateWrites => step(&mut state, |state| {
            run_waiting_for_late_writes(state, &agent, id)
        }),
    }
    if decided {
        decide(&mut state, &agent, id, JobDecision::SaveFailed);
    }
    match mark {
        Mark::Own => assert!(begin(&mut state, &agent, &CallKind::Save { job: id })),
        Mark::Other => assert!(begin(&mut state, &agent, &CallKind::Save { job: id + 1 })),
        Mark::Free => {}
    }
    is_replaceable(&state, &agent, id)
}

#[test]
fn only_a_periodic_job_whose_live_call_waits_and_that_has_not_decided_is_replaceable() {
    let cases = [SnapshotKind::Periodic, SnapshotKind::Update]
        .into_iter()
        .flat_map(|kind| {
            [Call::NotWaiting, Call::AfterFailure, Call::ForLateWrites]
                .into_iter()
                .flat_map(move |call| {
                    [false, true].into_iter().flat_map(move |decided| {
                        [Mark::Own, Mark::Other, Mark::Free]
                            .into_iter()
                            .map(move |mark| (kind, call, decided, mark))
                    })
                })
        })
        .map(|(kind, call, decided, mark)| {
            let replaceable = replaceable_case(kind, call, decided, mark);
            let expected = kind == SnapshotKind::Periodic
                && matches!(call, Call::AfterFailure | Call::ForLateWrites)
                && !decided
                && matches!(mark, Mark::Own);
            (
                format!("{kind:?} {call:?} decided={decided} {mark:?}"),
                replaceable == expected,
            )
        })
        .filter(|(_, right)| !right)
        .map(|(case, _)| case)
        .collect::<Vec<_>>();

    assert_eq!(cases, Vec::<String>::new());
}

#[test]
fn a_replacement_frees_the_save_mark_of_the_old_call_and_its_end_leaves_the_new_mark() {
    let mut state = State::default();
    let agent = agent_snapshots("replaced-tail");
    let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
    saving(&mut state, &agent, old);
    assert!(begin(&mut state, &agent, &CallKind::Save { job: old }));
    step(&mut state, |state| {
        run_waiting_for_late_writes(state, &agent, old)
    });

    let new = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
    let new_save = begin(&mut state, &agent, &CallKind::Save { job: new });
    let busy_with_both = busy(&state, &agent);
    let old_run_granted = granted(&mut state, &agent, old);
    ended_call(&mut state, &agent, &CallKind::Save { job: old });
    let after_the_tail = (
        state.save_running.get(&agent).copied(),
        busy(&state, &agent),
    );
    step(&mut state, |state| {
        run_waiting_for_late_writes(state, &agent, old)
    });
    saving(&mut state, &agent, new);
    step(&mut state, |state| {
        run_waiting_for_late_writes(state, &agent, new)
    });
    let new_replaceable_while_its_call_lives = is_replaceable(&state, &agent, new);
    ended_call(&mut state, &agent, &CallKind::Save { job: new });
    let new_replaceable_after_its_call = is_replaceable(&state, &agent, new);

    assert_eq!(
        (
            new_save,
            busy_with_both,
            old_run_granted,
            after_the_tail,
            new_replaceable_while_its_call_lives,
            new_replaceable_after_its_call,
        ),
        (true, 2, false, (Some(new), 1), true, false)
    );
}

#[test]
fn a_second_save_of_an_agent_is_refused_while_the_first_runs() {
    let mut state = State::default();
    let agent = agent_snapshots("one-save");

    let first = begin(&mut state, &agent, &CallKind::Save { job: 0 });
    let second = begin(&mut state, &agent, &CallKind::Save { job: 0 });
    let other = begin(&mut state, &agent, &CallKind::Other);
    let running = save_running(&state, &agent);
    ended_call(&mut state, &agent, &CallKind::Save { job: 0 });
    let after = (save_running(&state, &agent), busy(&state, &agent));

    assert_eq!((first, second, other, running), (true, false, true, true));
    assert_eq!(after, (false, 1));
}

#[test]
fn fork_ended_gives_the_stage_scope_only_for_an_attempt_that_never_published() {
    let source = agent_snapshots("source");
    let flight = (
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "target".to_string(),
        },
        [7; 32],
    );
    let end_of = |publishing: bool, published: bool| {
        let mut state = State::default();
        let stage = agent_snapshots("stage");
        let began = step(&mut state, |state| fork_began(state, &source, &flight));
        let again = step(&mut state, |state| fork_began(state, &source, &flight));
        if publishing {
            step(&mut state, |state| fork_publishing(state, &flight));
        }
        let end = step(&mut state, |state| {
            fork_ended(
                state,
                &source,
                &flight,
                Some(&stage),
                published.then_some(ForkOutcome::Published),
            )
        });
        (
            began,
            again,
            end,
            all_requested(&state, &stage),
            busy(&state, &source),
        )
    };

    assert_eq!(
        [
            end_of(false, false),
            end_of(true, false),
            end_of(true, true)
        ],
        [
            (
                true,
                false,
                ForkEnd::StageDeleted { overflow: None },
                true,
                0
            ),
            (true, false, ForkEnd::StageLeaked, false, 0),
            (true, false, ForkEnd::Done, false, 0),
        ]
    );
}

#[test]
fn the_end_of_a_fork_attempt_deletes_its_stage_only_when_nothing_can_publish_it() {
    let source = agent_snapshots("source");
    let flight = (
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "target".to_string(),
        },
        [8; 32],
    );
    let end_of = |publishing: bool, stage: bool, outcome: Option<ForkOutcome>| {
        let mut state = State::default();
        let staged = agent_snapshots("stage");
        step(&mut state, |state| fork_began(state, &source, &flight));
        if publishing {
            step(&mut state, |state| fork_publishing(state, &flight));
        }
        let end = step(&mut state, |state| {
            fork_ended(state, &source, &flight, stage.then_some(&staged), outcome)
        });
        (end, all_requested(&state, &staged))
    };
    let deleted = (ForkEnd::StageDeleted { overflow: None }, true);

    assert_eq!(
        [
            end_of(false, false, None),
            end_of(false, true, None),
            end_of(false, true, Some(ForkOutcome::Lost)),
            end_of(false, true, Some(ForkOutcome::Published)),
            end_of(true, true, Some(ForkOutcome::Lost)),
            end_of(true, true, Some(ForkOutcome::Unknown)),
            end_of(true, true, None),
            end_of(true, true, Some(ForkOutcome::Published)),
        ],
        [
            (ForkEnd::Done, false),
            deleted.clone(),
            deleted.clone(),
            (ForkEnd::Done, false),
            deleted,
            (ForkEnd::StageLeaked, false),
            (ForkEnd::StageLeaked, false),
            (ForkEnd::Done, false),
        ]
    );
}

#[test]
fn a_fork_of_a_source_whose_snapshots_are_being_deleted_is_refused() {
    let mut state = State::default();
    let source = agent_snapshots("source");
    let flight = (
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "target".to_string(),
        },
        [1; 32],
    );

    delete_all_snapshots(&mut state, &source);
    let began = step(&mut state, |state| fork_began(state, &source, &flight));

    assert!(!began);
}

/// One transition of the rules, over agents and names chosen by index.
#[derive(Clone, Debug)]
enum Step {
    Admit(usize, usize),
    End(usize),
    RequestNames(usize, Vec<usize>),
    RequestAll(usize),
    Take,
    CleanupEnded(usize),
    BeginCall(usize, Option<usize>, bool),
    StoreCallEnded,
    ForkBegan(usize, usize),
    ForkPublishing,
    ForkEnded(usize, bool),
    RunGranted(usize),
    RunFailed(usize),
    EndReplaced(usize),
}

fn step_strategy() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    let agent = 0usize..3;
    let name = 0usize..3;
    prop_oneof![
        (agent.clone(), name.clone()).prop_map(|(agent, name)| Step::Admit(agent, name)),
        agent.clone().prop_map(Step::End),
        (agent.clone(), proptest::collection::vec(name, 0..3))
            .prop_map(|(agent, names)| Step::RequestNames(agent, names)),
        agent.clone().prop_map(Step::RequestAll),
        Just(Step::Take),
        agent.clone().prop_map(Step::CleanupEnded),
        (
            agent.clone(),
            proptest::option::of(agent.clone()),
            any::<bool>()
        )
            .prop_map(|(agent, to, save)| Step::BeginCall(agent, to, save)),
        Just(Step::StoreCallEnded),
        (agent.clone(), agent.clone()).prop_map(|(from, stage)| Step::ForkBegan(from, stage)),
        Just(Step::ForkPublishing),
        (0usize..3, any::<bool>()).prop_map(|(index, published)| Step::ForkEnded(index, published)),
        agent.clone().prop_map(Step::RunGranted),
        agent.clone().prop_map(Step::RunFailed),
        (0usize..3).prop_map(Step::EndReplaced),
    ]
}

/// The agents that are ready, and the agents that are in the ready queue with `queued`.
fn ready_and_queued(
    state: &State,
    agents: &[AgentSnapshots],
) -> (Vec<AgentSnapshots>, Vec<AgentSnapshots>) {
    let ready = agents
        .iter()
        .filter(|agent| is_ready(state, agent))
        .cloned()
        .collect();
    let queued = agents
        .iter()
        .filter(|agent| {
            state.ready.contains(agent)
                && state
                    .cleanups
                    .get(agent)
                    .is_some_and(|cleanup| cleanup.queued)
        })
        .cloned()
        .collect();
    (ready, queued)
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    #[test]
    fn every_ready_agent_is_queued_after_any_sequence_of_transitions(
        steps in proptest::collection::vec(step_strategy(), 1..40),
        pending_cleanups in 1usize..3,
        names_per_cleanup in 1usize..4,
    ) {
        let agents = (0..3)
            .map(|index| agent_snapshots(&format!("agent-{index}")))
            .collect::<Vec<_>>();
        let stages = (0..3)
            .map(|index| agent_snapshots(&format!("stage-{index}")))
            .collect::<Vec<_>>();
        let job_names = (0..3)
            .map(|_| FilesystemSnapshotName::periodic())
            .collect::<Vec<_>>();
        let names = job_names
            .iter()
            .map(|name| SnapshotName::new(name.as_str()).unwrap())
            .collect::<Vec<_>>();
        let mut state = limited(pending_cleanups, 4, names_per_cleanup);
        let mut jobs: HashMap<usize, JobId> = HashMap::new();
        let mut replaced: Vec<(usize, JobId)> = Vec::new();
        let mut calls: Vec<(AgentSnapshots, CallKind)> = Vec::new();
        let mut forks: Vec<(AgentSnapshots, Flight, AgentSnapshots, bool)> = Vec::new();
        let mut fork_counter = 0u8;
        let all = agents.iter().chain(stages.iter()).cloned().collect::<Vec<_>>();
        steps.into_iter().for_each(|next| {
            match next {
                Step::Admit(agent, name) => {
                    let name = job_names[name].clone();
                    if let Ok(id) = try_admit(&mut state, &agents[agent], &name, true)
                        && let Some(old) = jobs.insert(agent, id)
                    {
                        replaced.push((agent, old));
                    }
                }
                Step::End(agent) => {
                    if let Some(id) = jobs.remove(&agent) {
                        end_job(&mut state, &agents[agent], id);
                    }
                }
                Step::RequestNames(agent, chosen) => {
                    let chosen = chosen.into_iter().map(|index| names[index].clone()).collect::<Vec<_>>();
                    request_names(&mut state, &agents[agent], &chosen);
                }
                Step::RequestAll(agent) => {
                    request_all(&mut state, &agents[agent]);
                }
                Step::Take => {
                    take(&mut state);
                }
                Step::CleanupEnded(agent) => {
                    ended_cleanup(&mut state, &agents[agent]);
                }
                Step::BeginCall(agent, to, save) => {
                    let kind = match (to, save) {
                        (Some(to), _) => CallKind::Copy { to: agents[to].clone() },
                        (None, true) => CallKind::Save { job: 0 },
                        (None, false) => CallKind::Other,
                    };
                    if begin(&mut state, &agents[agent], &kind) {
                        calls.push((agents[agent].clone(), kind));
                    }
                }
                Step::StoreCallEnded => {
                    if let Some((agent, kind)) = calls.pop() {
                        ended_call(&mut state, &agent, &kind);
                    }
                }
                Step::ForkBegan(from, stage) => {
                    fork_counter = fork_counter.wrapping_add(1);
                    let flight = (AgentId { component_id: ComponentId::new(), agent_id: "target".to_string() }, [fork_counter; 32]);
                    if step(&mut state, |state| fork_began(state, &agents[from], &flight)) {
                        forks.push((agents[from].clone(), flight, stages[stage].clone(), false));
                    }
                }
                Step::ForkPublishing => {
                    if let Some((_, flight, _, publishing)) = forks.last_mut() {
                        step(&mut state, |state| fork_publishing(state, flight));
                        *publishing = true;
                    }
                }
                Step::ForkEnded(index, published) => {
                    if !forks.is_empty() {
                        let (from, flight, stage, _) = forks.remove(index % forks.len());
                        step(&mut state, |state| fork_ended(state, &from, &flight, Some(&stage), published.then_some(ForkOutcome::Published)));
                    }
                }
                Step::RunGranted(agent) => {
                    if let Some(id) = jobs.get(&agent) {
                        granted(&mut state, &agents[agent], *id);
                    }
                }
                Step::RunFailed(agent) => {
                    if let Some(id) = jobs.get(&agent) {
                        failed_run(&mut state, &agents[agent], *id);
                    }
                }
                Step::EndReplaced(index) => {
                    if !replaced.is_empty() {
                        let (agent, id) = replaced.remove(index % replaced.len());
                        end_job(&mut state, &agents[agent], id);
                    }
                }
            }
            let (ready, queued) = ready_and_queued(&state, &all);
            assert!(
                ready.iter().all(|agent| queued.contains(agent)),
                "ready {ready:?} queued {queued:?}"
            );
            assert!(state.pending_names_total <= 4);
            assert!(
                state.pending_entries <= state.limits.pending_cleanups,
                "{} agents with pending work past the bound {}",
                state.pending_entries,
                state.limits.pending_cleanups
            );
            assert_eq!(
                state.pending_entries,
                state
                    .cleanups
                    .values()
                    .filter(|cleanup| cleanup.pending.is_some())
                    .count()
            );
        });
        // The queue gives `None` only when no agent is ready.
        let (ready_before, _) = ready_and_queued(&state, &all);
        let taken = take(&mut state);
        proptest::prop_assert!(taken.is_some() || ready_before.is_empty());
    }
}

#[test]
fn a_refused_admission_carries_the_retention_stop_of_the_running_job() {
    let mut state = State::default();
    let agent = agent_snapshots("retention-stop");
    let name = FilesystemSnapshotName::periodic();
    let stop = CancellationToken::new();
    let retention_stop = stop.child_token();
    let admitted = admit(
        &mut state,
        &agent,
        &name,
        stop.clone(),
        retention_stop.clone(),
        true,
    );

    let refusal = try_admit(&mut state, &agent, &name, true).err();
    if let Some(running) = refusal
        .as_ref()
        .and_then(|refusal| refusal.running.as_ref())
    {
        running.retention_stop.cancel();
    }

    assert!(admitted.is_ok());
    assert_eq!(
        refusal.map(|refusal| refusal.skip),
        Some(SnapshotSkip::UploadInFlight)
    );
    assert!(retention_stop.is_cancelled());
    assert!(!stop.is_cancelled());
}

#[test]
fn a_manual_update_asks_again_when_its_job_ended_or_waits_after_a_failed_run() {
    let mut state = State::default();
    let agent = agent_snapshots("agent");
    let first = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
    granted(&mut state, &agent, first);
    let while_saving = update_may_ask_again(&state, &agent, first);
    failed_run(&mut state, &agent, first);
    let after_a_failed_run = update_may_ask_again(&state, &agent, first);
    end_job(&mut state, &agent, first);
    let after_the_end = update_may_ask_again(&state, &agent, first);

    assert_eq!(
        (while_saving, after_a_failed_run, after_the_end),
        (false, true, true)
    );
}

/// The state after an admission of a periodic job of `agent` whose run failed, and the next
/// state that a replacing admission gives.
fn replacing(
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
) -> Next<Result<Admitted, Refusal>> {
    let mut state = State::default();
    let old = admitted(&mut state, agent, name);
    failed_run(&mut state, agent, old);
    super::admit(
        state,
        agent,
        name,
        SnapshotKind::Periodic,
        CancellationToken::new(),
        CancellationToken::new(),
        true,
    )
}

/// Admits a job of `kind` with new stops, and gives its id and the stop of the job that it
/// replaced.
fn admit_new(
    state: &mut State,
    agent: &AgentSnapshots,
    kind: SnapshotKind,
) -> Result<(JobId, Option<CancellationToken>), Option<JobId>> {
    admit_kind(
        state,
        agent,
        &FilesystemSnapshotName::periodic(),
        kind,
        CancellationToken::new(),
        CancellationToken::new(),
        true,
    )
    .map(|admitted| (admitted.id, admitted.replaced))
    .map_err(|refusal| refusal.running.map(|running| running.id))
}

#[test]
fn a_periodic_admission_replaces_a_periodic_job_that_waits_after_a_failure() {
    let mut state = State::default();
    let agent = agent_snapshots("replaced");
    let stop = CancellationToken::new();
    let old = admit(
        &mut state,
        &agent,
        &FilesystemSnapshotName::periodic(),
        stop.clone(),
        stop.child_token(),
        true,
    )
    .unwrap();
    saving(&mut state, &agent, old);
    failed_run(&mut state, &agent, old);

    let new = admit_new(&mut state, &agent, SnapshotKind::Periodic);
    let update = admit_new(&mut state, &agent, SnapshotKind::Update);

    let (new_id, replaced) = new.unwrap();
    assert!(new_id > old);
    assert!(replaced.is_some_and(|replaced| !replaced.is_cancelled()));
    assert!(!stop.is_cancelled());
    assert_eq!(update.err(), Some(Some(new_id)));
    assert!(has_ended(&state, &agent, old));
}

#[test]
fn a_replacement_records_replaced_and_gives_the_old_stop_without_cancelling_it() {
    let mut state = State::default();
    let agent = agent_snapshots("replaced-decision");
    let name = FilesystemSnapshotName::periodic();
    let stop = CancellationToken::new();
    let old = admit(
        &mut state,
        &agent,
        &name,
        stop.clone(),
        stop.child_token(),
        true,
    )
    .unwrap();
    saving(&mut state, &agent, old);
    let waiting = wait(&mut state, &agent, &name);
    failed_run(&mut state, &agent, old);

    let replaced = admit_new(&mut state, &agent, SnapshotKind::Periodic)
        .unwrap()
        .1;

    assert_eq!(waiting, Ok(old));
    assert_eq!(
        decision_of(&state, &agent, old),
        Some(JobDecision::Replaced)
    );
    assert!(replaced.is_some_and(|replaced| {
        let same = !replaced.is_cancelled();
        replaced.cancel();
        same && stop.is_cancelled()
    }));
}

#[test]
fn an_admission_refuses_a_job_that_is_not_waiting_after_a_failure_or_is_decided() {
    let agent = agent_snapshots("not-replaced");
    let not_waiting = {
        let mut state = State::default();
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        saving(&mut state, &agent, old);
        admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
    };
    let granted_again = {
        let mut state = State::default();
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        failed_run(&mut state, &agent, old);
        saving(&mut state, &agent, old);
        admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
    };
    let decided = {
        let mut state = State::default();
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        failed_run(&mut state, &agent, old);
        decide(&mut state, &agent, old, JobDecision::SaveFailed);
        admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
    };

    assert_eq!(
        [not_waiting, granted_again, decided],
        [Some(Some(1)), Some(Some(1)), Some(Some(1))]
    );
}

#[test]
fn an_update_job_is_never_replaced() {
    let mut state = State::default();
    let agent = agent_snapshots("update-kept");
    let (old, _) = admit_new(&mut state, &agent, SnapshotKind::Update).unwrap();
    failed_run(&mut state, &agent, old);

    assert_eq!(
        [
            admit_new(&mut state, &agent, SnapshotKind::Periodic).err(),
            admit_new(&mut state, &agent, SnapshotKind::Update).err(),
        ],
        [Some(Some(old)), Some(Some(old))]
    );
}

#[test]
fn the_grant_of_a_replaced_job_is_refused() {
    let mut state = State::default();
    let agent = agent_snapshots("stale-grant");
    let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
    failed_run(&mut state, &agent, old);
    let (new, _) = admit_new(&mut state, &agent, SnapshotKind::Periodic).unwrap();

    assert_eq!(
        [
            granted(&mut state, &agent, old),
            granted(&mut state, &agent, new)
        ],
        [false, true]
    );
}

#[test]
fn a_stale_run_report_of_a_replaced_job_changes_nothing() {
    let mut state = State::default();
    let agent = agent_snapshots("stale-report");
    let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
    failed_run(&mut state, &agent, old);
    let (new, _) = admit_new(&mut state, &agent, SnapshotKind::Periodic).unwrap();
    saving(&mut state, &agent, new);

    failed_run(&mut state, &agent, old);

    assert_eq!(
        (
            is_replaceable(&state, &agent, new),
            admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
        ),
        (false, Some(Some(new)))
    );
}

#[test]
fn no_replacement_while_a_delete_of_all_snapshots_is_pending() {
    let mut state = State::default();
    let agent = agent_snapshots("no-replacement-while-deleting");
    let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
    failed_run(&mut state, &agent, old);
    delete_all_snapshots(&mut state, &agent);

    let refusal = admit_kind(
        &mut state,
        &agent,
        &FilesystemSnapshotName::periodic(),
        SnapshotKind::Periodic,
        CancellationToken::new(),
        CancellationToken::new(),
        true,
    )
    .err()
    .map(|refusal| refusal.skip);

    assert_eq!(refusal, Some(SnapshotSkip::DeletingAllSnapshots));
    assert!(!has_ended(&state, &agent, old));
}

/// One step of the property of the replacement.
#[derive(Clone, Debug)]
enum ReplaceStep {
    Admit(usize, bool),
    Granted(usize, u64),
    Failed(usize, u64),
    Decide(usize, u64),
    End(usize, u64),
}

fn replace_step_strategy() -> impl proptest::strategy::Strategy<Value = ReplaceStep> {
    use proptest::prelude::*;
    prop_oneof![
        (0..2usize, any::<bool>())
            .prop_map(|(agent, periodic)| ReplaceStep::Admit(agent, periodic)),
        (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::Granted(agent, id)),
        (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::Failed(agent, id)),
        (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::Decide(agent, id)),
        (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::End(agent, id)),
    ]
}

proptest::proptest! {
    #[test]
    fn replacements_keep_one_live_job_and_refuse_stale_reports(
        steps in proptest::collection::vec(replace_step_strategy(), 1..40)
    ) {
        let agents = [agent_snapshots("one"), agent_snapshots("two")];
        let mut state = State::default();
        let mut replaced = HashSet::new();
        // A manual update waits for the job of an agent until the job is gone or an admission
        // can replace it. Each transition that makes that true for a job must wake the waiters.
        let ready_for_an_update = |state: &State, agent: &AgentSnapshots, id: JobId| {
            has_ended(state, agent, id) || is_replaceable(state, agent, id)
        };
        steps.into_iter().for_each(|step| {
            let waited_on = agents
                .iter()
                .filter_map(|agent| {
                    state
                        .jobs
                        .get(agent)
                        .map(|job| (agent.clone(), job.id))
                        .filter(|(agent, id)| !ready_for_an_update(&state, agent, *id))
                })
                .collect::<Vec<_>>();
            let wakes = match step {
                ReplaceStep::Admit(agent, periodic) => {
                    let kind = if periodic { SnapshotKind::Periodic } else { SnapshotKind::Update };
                    let before = state.jobs.get(&agents[agent]).map(|job| job.id);
                    let next = super::admit(
                        std::mem::take(&mut state),
                        &agents[agent],
                        &FilesystemSnapshotName::periodic(),
                        kind,
                        CancellationToken::new(),
                        CancellationToken::new(),
                        true,
                    );
                    let wakes = next.wakes();
                    let (next, answer) = next.into_parts();
                    state = next;
                    if let (Ok(admitted), Some(before)) = (&answer, before) {
                        assert!(admitted.replaced.is_some());
                        replaced.insert(before);
                    }
                    wakes
                }
                ReplaceStep::Granted(agent, id) => {
                    let live = state.jobs.get(&agents[agent]).is_some_and(|job| job.id == id);
                    let next = super::run_granted(std::mem::take(&mut state), &agents[agent], id);
                    let wakes = next.wakes();
                    let (next, granted) = next.into_parts();
                    state = next;
                    assert_eq!(granted, live);
                    assert!(!(granted && replaced.contains(&id)));
                    wakes
                }
                ReplaceStep::Failed(agent, id) => {
                    let before = state.jobs.get(&agents[agent]).map(|job| (job.id, job.run_phase));
                    let next = super::run_failed(std::mem::take(&mut state), &agents[agent], id);
                    let wakes = next.wakes();
                    state = next.into_parts().0;
                    let after = state.jobs.get(&agents[agent]).map(|job| (job.id, job.run_phase));
                    if before.is_none_or(|(live, _)| live != id) {
                        assert_eq!(before, after);
                    }
                    wakes
                }
                ReplaceStep::Decide(agent, id) => {
                    let next = super::decide(
                        std::mem::take(&mut state),
                        &agents[agent],
                        id,
                        JobDecision::SaveFailed,
                    );
                    let wakes = next.wakes();
                    state = next.into_parts().0;
                    wakes
                }
                ReplaceStep::End(agent, id) => {
                    let next = end(std::mem::take(&mut state), &agents[agent], id);
                    let wakes = next.wakes();
                    state = next.into_parts().0;
                    wakes
                }
            };
            // After each step: no replaced job is live, and a waiter whose job became ready
            // for an update was woken.
            agents.iter().for_each(|agent| {
                assert!(state.jobs.get(agent).is_none_or(|job| !replaced.contains(&job.id)));
            });
            waited_on.iter().for_each(|(agent, id)| {
                assert!(
                    !ready_for_an_update(&state, agent, *id) || wakes,
                    "the job {id} became ready for an update without a wake"
                );
            });
        });
    }
}
