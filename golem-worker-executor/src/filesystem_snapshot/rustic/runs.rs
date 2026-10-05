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

//! The runs of one store call.
//!
//! A call runs one or more times. Each run takes a slot of the limiter of the call before it
//! starts, and frees the slot and all that it holds when it ends. [`next_run`] decides what the
//! call does after each run, and [`Shell`] does what it decides. The shell only records what each
//! take, run, wait and check gave, and at a take it looks at the shutdown first, then the cancel,
//! then the limiter.

use crate::filesystem_snapshot::{Failed, RunSlots, Slot, SnapshotInfo, Withdrawal};
use golem_common::model::RetryConfig;
use golem_common::retries::get_delay;
use rand::Rng as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Gives the instant now, from the clock of the runtime, which a test can pause and advance.
pub(super) fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

/// The most runs of one call that end because another call of the agent made their work invalid.
pub(super) const MOST_RACE_RUNS: u32 = 8;

/// The method of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Save,
    Restore,
    Copy,
    Delete,
    DeleteAll,
    List,
    Stat,
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunEnd {
    /// The run gives the answer of the call.
    Answered,
    /// A storage call of the run failed after its tries.
    CallFailed,
    /// A storage call gave a failure that no try can fix, or the work gave an error that no new
    /// run can change.
    Permanent,
    /// Another call of the agent made the work of the run invalid, and a new run can do it again.
    RaceRunAgain,
    /// The save did not finish its backup before the time that the store allows.
    BoundPassed,
    /// The cancel of the save fired, or the store shut down.
    Cancelled,
    /// No run started: the limiter withdrew the call at the take.
    NotRun,
    /// The slot of a new run was granted, and the run has not started.
    SlotTaken,
}

/// What the check of the own name of a save found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Checked {
    /// The snapshot file that the publish wrote is there.
    Found(SnapshotInfo),
    /// Another snapshot file has the name.
    Other,
    /// No snapshot file has the name.
    Absent,
    /// The check could not read the snapshots of the agent.
    Unreadable,
}

/// A write of a run that ended without an answer, and the check that belongs to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Late {
    /// After this instant, the write has landed or never lands.
    pub(super) until: Instant,
    /// What the check of the own name found after `until`.
    pub(super) checked: Option<Checked>,
}

/// What the call saw before it decides its next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RunSeen {
    pub(super) kind: Kind,
    /// The runs that ended `CallFailed`, the last one included.
    pub(super) failed_runs: u32,
    /// The runs that ended `RaceRunAgain`, the last one included.
    pub(super) races: u32,
    /// Whether a run of the call ended with a storage failure.
    pub(super) ran: bool,
    /// How the last run ended.
    pub(super) end: RunEnd,
    pub(super) late: Option<Late>,
    /// A withdrawal that came at a take or during a wait.
    pub(super) withdrawn: Option<Withdrawal>,
    pub(super) shut_down: bool,
    pub(super) now: Instant,
    /// The time after which a run of a save does not start.
    pub(super) backup_end: Option<Instant>,
}

/// What the call does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NextRun {
    /// Start the run whose slot was granted.
    Start,
    /// Take a slot at once, and run again.
    RunNow,
    /// Wait until the instant, then run again. Only a wait after a failed run tells the limiter.
    WaitThenRun {
        until: Instant,
        after_failure: bool,
    },
    /// Wait until the instant, then check the own name.
    WaitThenCheck(Instant),
    Answer(RunOutcome),
}

/// The answer of the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunOutcome {
    /// The answer that the last run gave.
    AsTheRunSaid,
    /// The publish of an earlier run landed.
    Saved(SnapshotInfo),
    /// Another writer published a snapshot with the name.
    NameInUse,
    /// `Failed`, with the failure of the last run.
    FailedWithLast,
    /// `Failed`: a run of a save would start after the end of the backup.
    TooLate,
    Stopped(Withdrawal),
}

/// Decides the next step of a call that saw `seen`, with the runs after a failed call of `retry`
/// and the jitter factor `jitter`. The rows are tried in a fixed order, and the first that matches
/// decides:
///
/// 0. A run that answered, and whose write ended without an answer, waits until that write has
///    landed or can no longer land, unless the store shuts down.
/// 1. A run that answered gives its answer.
/// 2. A shutdown gives `Stopped`; a write that can still land is not waited for.
///    Next, a granted slot starts its run, unless the time of the grant is at or after
///    `backup_end`: then the call gives `Failed`, and no run of a save starts after the end of its
///    backup.
/// 3. A write that ended without an answer is waited for: a save then checks its own name, and
///    each other call goes on by the rows below.
/// 4. to 6. The check of the own name decides: the own file gives the info, another file with the
///    name gives `NameInUse`, and a check that could not read gives `Failed`.
/// 7. to 9. A withdrawal gives `Stopped` with its cause, or `Failed` when the cause is the
///    deadline and a run met a storage failure.
/// 10. A cancel gives `Stopped`.
/// 11. A failed call runs again after a wait while the retries allow it and the wait ends before
///     `backup_end`. This is the only wait that tells the limiter.
/// 12. Any other failure gives `Failed`.
/// 13. and 14. A race runs again at once, at most [`MOST_RACE_RUNS`] times.
pub(super) fn next_run(seen: &RunSeen, retry: &RetryConfig, jitter: f64) -> NextRun {
    let late = seen.late.map(|late| (late.until, late.checked));
    match (seen.end, late, seen.withdrawn) {
        (RunEnd::Answered, Some((until, None)), _) if !seen.shut_down => NextRun::WaitThenRun {
            until,
            after_failure: false,
        },
        (RunEnd::Answered, _, _) => NextRun::Answer(RunOutcome::AsTheRunSaid),
        _ if seen.shut_down => NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped)),
        (RunEnd::SlotTaken, _, _) if seen.backup_end.is_some_and(|end| seen.now >= end) => {
            NextRun::Answer(RunOutcome::TooLate)
        }
        (RunEnd::SlotTaken, _, _) => NextRun::Start,
        (_, Some((until, None)), _) => match seen.kind {
            Kind::Save => NextRun::WaitThenCheck(until),
            Kind::Restore
            | Kind::Copy
            | Kind::Delete
            | Kind::DeleteAll
            | Kind::List
            | Kind::Stat => NextRun::WaitThenRun {
                until,
                after_failure: false,
            },
        },
        (_, Some((_, Some(Checked::Found(info)))), _) => NextRun::Answer(RunOutcome::Saved(info)),
        (_, Some((_, Some(Checked::Other))), _) => NextRun::Answer(RunOutcome::NameInUse),
        (_, Some((_, Some(Checked::Unreadable))), _) => NextRun::Answer(RunOutcome::FailedWithLast),
        (_, _, Some(Withdrawal::Stopped)) => {
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped))
        }
        (_, _, Some(Withdrawal::Deadline)) if seen.ran => {
            NextRun::Answer(RunOutcome::FailedWithLast)
        }
        (_, _, Some(Withdrawal::Deadline)) => {
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Deadline))
        }
        (RunEnd::Cancelled, _, None) => NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped)),
        (RunEnd::CallFailed, _, None) => {
            match run_delay(retry, seen.failed_runs, jitter)
                .filter(|delay| seen.backup_end.is_none_or(|end| seen.now + *delay < end))
            {
                Some(delay) => NextRun::WaitThenRun {
                    until: seen.now + delay,
                    after_failure: true,
                },
                None => NextRun::Answer(RunOutcome::FailedWithLast),
            }
        }
        (RunEnd::RaceRunAgain, _, None) if seen.races < MOST_RACE_RUNS => NextRun::RunNow,
        (
            RunEnd::RaceRunAgain | RunEnd::Permanent | RunEnd::BoundPassed | RunEnd::NotRun,
            _,
            None,
        ) => NextRun::Answer(RunOutcome::FailedWithLast),
    }
}

/// The wait before the next run after `failed_runs` runs failed, or `None` when no run follows.
/// `jitter` is the jitter factor that the caller drew below the `max_jitter_factor` of `retry`:
/// the wait grows by that part of itself, and stays at most the `max_delay` of `retry`. A jitter
/// that is negative or not a number counts as none, and a wait too large for a [`Duration`] is
/// the `max_delay`, so no jitter panics.
pub(super) fn run_delay(retry: &RetryConfig, failed_runs: u32, jitter: f64) -> Option<Duration> {
    let without_jitter = RetryConfig {
        max_jitter_factor: None,
        ..retry.clone()
    };
    let base = get_delay(&without_jitter, failed_runs)?;
    let grown = base.as_secs_f64() * (1.0 + jitter.max(0.0));
    Some(
        Duration::try_from_secs_f64(grown)
            .unwrap_or(retry.max_delay)
            .min(retry.max_delay),
    )
}

/// Draws the jitter factor of a wait below the `max_jitter_factor` of `retry`. A factor of 0 or
/// none gives no jitter.
pub(super) fn jitter(retry: &RetryConfig) -> f64 {
    retry
        .max_jitter_factor
        .filter(|factor| *factor > 0.0)
        .map_or(0.0, |factor| rand::rng().random_range(0.0..factor))
}

/// What one run of a call gave.
pub(super) enum Ran<T> {
    /// The run gives the answer of the call.
    Answered(T),
    /// The run gives the answer of the call after its writes that ended without an answer have
    /// landed or can no longer land.
    AnsweredAfter(T, LateWrite),
    /// The run ended without an answer.
    Ended(Ended),
}

/// A run that ended without an answer.
#[derive(Debug)]
pub(super) struct Ended {
    /// How the run ended. It is never `Answered`.
    pub(super) end: RunEnd,
    /// The failure of the run.
    pub(super) failure: anyhow::Error,
    /// The writes of the run that ended without an answer, when it has some.
    pub(super) late: Option<LateWrite>,
}

impl Ended {
    /// A run that ended with `end` and `failure`, and with no write that can still land.
    pub(super) fn new(end: RunEnd, failure: anyhow::Error) -> Self {
        Self {
            end,
            failure,
            late: None,
        }
    }
}

/// The writes of a run that ended without an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LateWrite {
    /// After this instant, each of the writes has landed or never lands.
    pub(super) until: Instant,
    /// The snapshot file that the publish of a save staged, which the check of the own name looks
    /// for.
    pub(super) own: Option<OwnFile>,
}

/// The snapshot file that a publish staged, with the info of its snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnFile {
    /// The path of the file, which is the hash of its content.
    pub(super) path: Arc<Path>,
    pub(super) info: SnapshotInfo,
}

/// What a run gets from the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RunStart {
    /// The instant of the first slot of the call.
    pub(super) t0: Instant,
    /// The number of the run in the call, from 1.
    pub(super) number: u32,
}

/// How the shell gives each outcome as the answer of one method.
pub(super) struct Answers<T> {
    pub(super) failed: fn(Failed) -> T,
    pub(super) stopped: fn(Withdrawal) -> T,
    pub(super) saved: fn(SnapshotInfo) -> T,
    pub(super) name_in_use: fn() -> T,
}

/// The runs of one call: its method, its limiter, the root of the store, the cancel of a save, and
/// the runs after a failed call.
pub(super) struct Shell<'a> {
    pub(super) kind: Kind,
    pub(super) slots: &'a dyn RunSlots,
    /// Cancelled when the store shuts down.
    pub(super) root: &'a CancellationToken,
    /// The cancel of a save.
    pub(super) cancel: Option<&'a CancellationToken>,
    pub(super) retry: &'a RetryConfig,
}

/// The state of a call between its steps.
struct Calling<T> {
    failed_runs: u32,
    races: u32,
    ran: bool,
    end: RunEnd,
    late: Option<Late>,
    /// The file that the own-name check of the late write looks for.
    own: Option<OwnFile>,
    last_failure: Option<anyhow::Error>,
    withdrawn: Option<Withdrawal>,
    shut_down: bool,
    /// The instant of the first slot of the call.
    t0: Option<Instant>,
    /// The runs that started.
    runs: u32,
    /// The slot of the run that starts next.
    slot: Option<Slot>,
    answer: Option<T>,
}

/// The next step of the shell.
enum Step {
    Take { immediate: bool },
    Run,
    Decide,
}

impl Shell<'_> {
    /// Runs the call to its final answer. `run` does one run under a slot; it gets the instant of
    /// the first slot of the call and the number of the run. `check` reads the own name of a save
    /// for the file that a late publish staged. `backup_end` gives the time after which a run of a
    /// save does not start, from the instant of the first slot.
    pub(super) async fn call<T, R, F, C, G>(
        &self,
        answers: Answers<T>,
        backup_end: impl Fn(Instant) -> Option<Instant>,
        mut run: R,
        check: C,
    ) -> T
    where
        R: FnMut(RunStart) -> F,
        F: Future<Output = Ran<T>>,
        C: Fn(OwnFile) -> G,
        G: Future<Output = Checked>,
    {
        let calling = Calling {
            failed_runs: 0,
            races: 0,
            ran: false,
            end: RunEnd::NotRun,
            late: None,
            own: None,
            last_failure: None,
            withdrawn: None,
            shut_down: false,
            t0: None,
            runs: 0,
            slot: None,
            answer: None,
        };
        let backup_end = &backup_end;
        let check = &check;
        let steps = futures::stream::unfold(
            Some((calling, Step::Take { immediate: true }, &mut run)),
            |state| async move {
                let (calling, step, run) = state?;
                match step {
                    Step::Take { immediate } => {
                        let calling = self.take(calling, immediate).await;
                        Some((None, Some((calling, Step::Decide, run))))
                    }
                    Step::Run => {
                        let calling = self.run(calling, run).await;
                        Some((None, Some((calling, Step::Decide, run))))
                    }
                    Step::Decide => match self.decide(calling, backup_end, check).await {
                        Ok((calling, step)) => Some((None, Some((calling, step, run)))),
                        Err(outcome) => Some((Some(outcome), None)),
                    },
                }
            },
        );
        let ended = futures::StreamExt::next(&mut std::pin::pin!(futures::StreamExt::filter_map(
            steps,
            |ended| async move { ended }
        )))
        .await;
        match ended {
            Some(Outcome(outcome, calling)) => answer_of(outcome, calling, &answers),
            None => (answers.failed)(Failed::new(anyhow::anyhow!(
                "the filesystem snapshot call ended without an answer"
            ))),
        }
    }

    /// Takes a slot. A withdrawal, a cancel or a shutdown at the take gives no slot.
    async fn take<T>(&self, mut calling: Calling<T>, immediate: bool) -> Calling<T> {
        let taken = tokio::select! {
            biased;
            () = self.root.cancelled() => Taken::ShutDown,
            () = cancelled(self.cancel) => Taken::Cancelled,
            taken = self.slots.take(immediate) => match taken {
                Ok(slot) => Taken::Slot(slot),
                Err(cause) => Taken::Withdrawn(cause),
            },
        };
        match taken {
            Taken::ShutDown => {
                calling.shut_down = true;
                calling.end = RunEnd::NotRun;
            }
            Taken::Cancelled => calling.end = RunEnd::Cancelled,
            Taken::Withdrawn(cause) => {
                calling.withdrawn = Some(cause);
                calling.end = RunEnd::NotRun;
            }
            Taken::Slot(slot) => {
                calling.t0.get_or_insert_with(now);
                calling.slot = Some(slot);
                calling.end = RunEnd::SlotTaken;
            }
        }
        calling
    }

    /// Runs once under the slot that the take gave, and frees the slot when the run ends.
    async fn run<T, R, F>(&self, mut calling: Calling<T>, run: &mut R) -> Calling<T>
    where
        R: FnMut(RunStart) -> F,
        F: Future<Output = Ran<T>>,
    {
        let slot = calling.slot.take();
        calling.runs += 1;
        let start = RunStart {
            t0: calling.t0.unwrap_or_else(now),
            number: calling.runs,
        };
        let ran = run(start).await;
        drop(slot);
        match ran {
            Ran::Answered(answer) => {
                calling.end = RunEnd::Answered;
                calling.answer = Some(answer);
            }
            Ran::AnsweredAfter(answer, late) => {
                calling.end = RunEnd::Answered;
                calling.answer = Some(answer);
                calling.late = Some(Late {
                    until: late.until,
                    checked: None,
                });
            }
            Ran::Ended(ended) => {
                calling.end = ended.end;
                calling.failed_runs += u32::from(ended.end == RunEnd::CallFailed);
                calling.races += u32::from(ended.end == RunEnd::RaceRunAgain);
                calling.ran |= matches!(ended.end, RunEnd::CallFailed | RunEnd::Permanent);
                calling.last_failure = Some(ended.failure);
                calling.late = ended.late.as_ref().map(|late| Late {
                    until: late.until,
                    checked: None,
                });
                calling.own = ended.late.and_then(|late| late.own);
            }
        }
        calling
    }

    /// Asks [`next_run`] and does what it says. Gives the next step, or the answer.
    async fn decide<T, C, G>(
        &self,
        mut calling: Calling<T>,
        backup_end: &impl Fn(Instant) -> Option<Instant>,
        check: &C,
    ) -> Result<(Calling<T>, Step), Outcome<T>>
    where
        C: Fn(OwnFile) -> G,
        G: Future<Output = Checked>,
    {
        let seen = RunSeen {
            kind: self.kind,
            failed_runs: calling.failed_runs,
            races: calling.races,
            ran: calling.ran,
            end: calling.end,
            late: calling.late,
            withdrawn: calling.withdrawn,
            shut_down: calling.shut_down || self.root.is_cancelled(),
            now: now(),
            backup_end: calling.t0.and_then(backup_end),
        };
        match next_run(&seen, self.retry, jitter(self.retry)) {
            NextRun::Answer(outcome) => Err(Outcome(outcome, calling)),
            NextRun::Start => Ok((calling, Step::Run)),
            NextRun::RunNow => Ok((calling, Step::Take { immediate: true })),
            NextRun::WaitThenRun {
                until,
                after_failure: true,
            } => {
                self.slots.waiting_after_failure();
                let until = tokio::time::Instant::from_std(until);
                tokio::select! {
                    biased;
                    () = self.root.cancelled() => {
                        calling.shut_down = true;
                        Ok((calling, Step::Decide))
                    }
                    () = cancelled(self.cancel) => {
                        calling.end = RunEnd::Cancelled;
                        Ok((calling, Step::Decide))
                    }
                    cause = self.slots.withdrawn() => {
                        calling.withdrawn = Some(cause);
                        Ok((calling, Step::Decide))
                    }
                    () = tokio::time::sleep_until(until) => {
                        Ok((calling, Step::Take { immediate: false }))
                    }
                }
            }
            NextRun::WaitThenRun {
                until,
                after_failure: false,
            } => {
                self.slots.waiting_for_late_writes();
                tokio::select! {
                    biased;
                    () = self.root.cancelled() => calling.shut_down = true,
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(until)) => {
                        calling.late = None;
                        calling.own = None;
                    }
                }
                Ok((calling, Step::Decide))
            }
            NextRun::WaitThenCheck(until) => {
                self.slots.waiting_for_late_writes();
                let own = calling.own.clone();
                let checked = tokio::select! {
                    biased;
                    () = self.root.cancelled() => None,
                    checked = async {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(until)).await;
                        match own {
                            Some(own) => check(own).await,
                            None => Checked::Absent,
                        }
                    } => Some(checked),
                };
                match checked {
                    None => calling.shut_down = true,
                    Some(Checked::Absent) => {
                        calling.late = None;
                        calling.own = None;
                    }
                    Some(checked) => {
                        calling.late = calling.late.map(|late| Late {
                            checked: Some(checked),
                            ..late
                        });
                    }
                }
                Ok((calling, Step::Decide))
            }
        }
    }
}

/// What a take gave.
enum Taken {
    Slot(Slot),
    Withdrawn(Withdrawal),
    Cancelled,
    ShutDown,
}

/// The outcome that ended a call, with the state that holds its answer and its last failure.
struct Outcome<T>(RunOutcome, Calling<T>);

/// Gives the answer of the method for the outcome.
fn answer_of<T>(outcome: RunOutcome, calling: Calling<T>, answers: &Answers<T>) -> T {
    let failed = |last_failure: Option<anyhow::Error>| {
        (answers.failed)(Failed::new(last_failure.unwrap_or_else(|| {
            anyhow::anyhow!("the filesystem snapshot call failed")
        })))
    };
    match outcome {
        RunOutcome::AsTheRunSaid => match calling.answer {
            Some(answer) => answer,
            None => failed(calling.last_failure),
        },
        RunOutcome::Saved(info) => (answers.saved)(info),
        RunOutcome::NameInUse => (answers.name_in_use)(),
        RunOutcome::FailedWithLast => failed(calling.last_failure),
        RunOutcome::TooLate => failed(Some(anyhow::anyhow!(
            "the save would take longer than the filesystem snapshot store allows"
        ))),
        RunOutcome::Stopped(cause) => (answers.stopped)(cause),
    }
}

/// Completes when the token is cancelled, and never without a token.
async fn cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Gives the end of a run of a copy whose error is `error`: a source blob that is gone, and a
/// catch-up that did not end, run again; a failed storage call fails the run; and a name error
/// cannot change.
pub(super) fn copy_end(error: &super::scope::CopyError) -> RunEnd {
    match error {
        super::scope::CopyError::CopySourceMissing { .. } | super::scope::CopyError::Race => {
            RunEnd::RaceRunAgain
        }
        super::scope::CopyError::CallFailed(_) => RunEnd::CallFailed,
        super::scope::CopyError::Permanent(_) => RunEnd::Permanent,
        super::scope::CopyError::Cancelled(_) => RunEnd::Cancelled,
    }
}

#[cfg(test)]
mod tests;
