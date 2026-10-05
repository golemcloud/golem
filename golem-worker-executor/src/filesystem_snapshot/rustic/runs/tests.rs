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

use super::super::scope::CopyError;
use super::{
    Answers, Checked, Ended, Kind, Late, LateWrite, MOST_RACE_RUNS, NextRun, OwnFile, Ran, RunEnd,
    RunOutcome, RunSeen, Settle, Settled, Shell, copy_end, jitter, next_run, run_delay,
};
use crate::filesystem_snapshot::{Failed, RunSlots, Slot, SnapshotInfo, Withdrawal};
use futures::future::BoxFuture;
use golem_common::model::{RetryConfig, Timestamp};
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use test_r::test;
use tokio_util::sync::CancellationToken;

fn retry() -> RetryConfig {
    RetryConfig {
        max_attempts: 5,
        min_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(120),
        multiplier: 4.0,
        max_jitter_factor: None,
    }
}

fn info() -> SnapshotInfo {
    SnapshotInfo {
        created_at: Timestamp::from(7),
        files: 1,
        bytes: 2,
    }
}

/// What a call of `kind` saw after a run that ended with `end`, with nothing else.
fn seen(kind: Kind, end: RunEnd, now: Instant) -> RunSeen {
    RunSeen {
        kind,
        failed_runs: u32::from(end == RunEnd::CallFailed),
        races: u32::from(end == RunEnd::RaceRunAgain),
        ran: end == RunEnd::CallFailed,
        end,
        late: None,
        withdrawn: None,
        shut_down: false,
        now,
        backup_end: None,
    }
}

fn late(until: Instant, checked: Option<Checked>) -> Option<Late> {
    Some(Late { until, checked })
}

#[test]
fn each_row_of_next_run_decides_in_its_fixed_order() {
    let now = Instant::now();
    let until = now + Duration::from_secs(60);
    let decide = |seen: RunSeen| next_run(&seen, &retry(), 0.0);

    assert_eq!(
        [
            // 0: an answer with a late write waits for it.
            decide(RunSeen {
                late: late(until, None),
                ..seen(Kind::Copy, RunEnd::Answered, now)
            }),
            // 1: an answer comes first, also with a late write and a shutdown.
            decide(RunSeen {
                late: late(until, None),
                shut_down: true,
                ..seen(Kind::Save, RunEnd::Answered, now)
            }),
            // 2: a shutdown does not wait for a write that can still land.
            decide(RunSeen {
                late: late(until, None),
                withdrawn: Some(Withdrawal::Deadline),
                shut_down: true,
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            // 3: a late write of a save is waited for and checked, before a withdrawal and a
            // cancel.
            decide(RunSeen {
                late: late(until, None),
                withdrawn: Some(Withdrawal::Stopped),
                ..seen(Kind::Save, RunEnd::Cancelled, now)
            }),
            // 3: a late write of a copy is waited for, before every terminal row.
            decide(RunSeen {
                late: late(until, None),
                ..seen(Kind::Copy, RunEnd::Permanent, now)
            }),
            // 4 to 6: the check decides, before a withdrawal.
            decide(RunSeen {
                late: late(until, Some(Checked::Found(info()))),
                withdrawn: Some(Withdrawal::Deadline),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                late: late(until, Some(Checked::Other)),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                late: late(until, Some(Checked::Unreadable)),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            // 7 to 9: a withdrawal.
            decide(RunSeen {
                withdrawn: Some(Withdrawal::Stopped),
                ..seen(Kind::Delete, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                withdrawn: Some(Withdrawal::Deadline),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                withdrawn: Some(Withdrawal::Deadline),
                ..seen(Kind::Save, RunEnd::NotRun, now)
            }),
            // 10: a cancel.
            decide(seen(Kind::Save, RunEnd::Cancelled, now)),
            // 11: a failed call runs again after the wait of the retries.
            decide(seen(Kind::Delete, RunEnd::CallFailed, now)),
            // 12: other failures.
            decide(seen(Kind::Save, RunEnd::Permanent, now)),
            decide(seen(Kind::Save, RunEnd::BoundPassed, now)),
            // 13 and 14: races.
            decide(seen(Kind::Restore, RunEnd::RaceRunAgain, now)),
            decide(RunSeen {
                races: MOST_RACE_RUNS,
                ..seen(Kind::Restore, RunEnd::RaceRunAgain, now)
            }),
        ],
        [
            NextRun::WaitThenRun {
                until,
                after_failure: false
            },
            NextRun::Answer(RunOutcome::AsTheRunSaid),
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped)),
            NextRun::WaitThenCheck(until),
            NextRun::WaitThenRun {
                until,
                after_failure: false
            },
            NextRun::Answer(RunOutcome::Saved(info())),
            NextRun::Answer(RunOutcome::NameInUse),
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped)),
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Deadline)),
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped)),
            NextRun::WaitThenRun {
                until: now + Duration::from_secs(2),
                after_failure: true
            },
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::RunNow,
            NextRun::Answer(RunOutcome::FailedWithLast),
        ]
    );
}

#[test]
fn a_granted_slot_starts_its_run_only_before_the_end_of_the_backup_and_after_a_shutdown_never() {
    let now = Instant::now();
    let decide = |seen: RunSeen| next_run(&seen, &retry(), 0.0);

    assert_eq!(
        [
            decide(seen(Kind::Save, RunEnd::SlotTaken, now)),
            decide(RunSeen {
                backup_end: Some(now + Duration::from_millis(1)),
                ..seen(Kind::Save, RunEnd::SlotTaken, now)
            }),
            decide(RunSeen {
                backup_end: Some(now),
                ..seen(Kind::Save, RunEnd::SlotTaken, now)
            }),
            decide(RunSeen {
                shut_down: true,
                ..seen(Kind::Save, RunEnd::SlotTaken, now)
            }),
            decide(RunSeen {
                late: late(now + Duration::from_secs(60), None),
                ..seen(Kind::Delete, RunEnd::SlotTaken, now)
            }),
        ],
        [
            NextRun::Start,
            NextRun::Start,
            NextRun::Answer(RunOutcome::TooLate),
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Stopped)),
            NextRun::Start,
        ]
    );
}

#[test]
fn a_failed_call_runs_again_only_within_the_retries_and_when_its_wait_ends_before_the_end_of_the_backup()
 {
    let now = Instant::now();
    let decide = |seen: RunSeen| next_run(&seen, &retry(), 0.0);

    assert_eq!(
        [
            decide(RunSeen {
                failed_runs: 4,
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                failed_runs: 5,
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                backup_end: Some(now + Duration::from_millis(2001)),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                backup_end: Some(now + Duration::from_secs(2)),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                backup_end: Some(now + Duration::from_millis(1)),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
            decide(RunSeen {
                backup_end: Some(now),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            }),
        ],
        [
            NextRun::WaitThenRun {
                until: now + Duration::from_secs(120),
                after_failure: true
            },
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::WaitThenRun {
                until: now + Duration::from_secs(2),
                after_failure: true
            },
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::Answer(RunOutcome::FailedWithLast),
        ]
    );
}

#[test]
fn only_the_wait_after_a_failed_run_tells_the_limiter() {
    let now = Instant::now();
    let until = now + Duration::from_secs(60);
    let after_failure = |decided: NextRun| match decided {
        NextRun::WaitThenRun { after_failure, .. } => Some(after_failure),
        NextRun::Start | NextRun::RunNow | NextRun::WaitThenCheck(_) | NextRun::Answer(_) => None,
    };

    assert_eq!(
        [
            seen(Kind::Save, RunEnd::CallFailed, now),
            RunSeen {
                late: late(until, None),
                ..seen(Kind::Save, RunEnd::CallFailed, now)
            },
            RunSeen {
                late: late(until, None),
                ..seen(Kind::Copy, RunEnd::CallFailed, now)
            },
            seen(Kind::Restore, RunEnd::RaceRunAgain, now),
        ]
        .map(|seen| after_failure(next_run(&seen, &retry(), 0.0))),
        [Some(true), None, Some(false), None]
    );
}

#[test]
fn a_deadline_after_a_race_run_and_no_failed_run_gives_no_slot_and_after_races_then_a_failed_call_gives_failed()
 {
    let now = Instant::now();
    let decide = |seen: RunSeen| next_run(&seen, &retry(), 0.0);

    assert_eq!(
        [
            decide(RunSeen {
                races: 2,
                withdrawn: Some(Withdrawal::Deadline),
                ..seen(Kind::Restore, RunEnd::NotRun, now)
            }),
            decide(RunSeen {
                races: 2,
                failed_runs: 1,
                ran: true,
                withdrawn: Some(Withdrawal::Deadline),
                ..seen(Kind::Restore, RunEnd::NotRun, now)
            }),
            decide(RunSeen {
                races: 3,
                ..seen(Kind::Restore, RunEnd::CallFailed, now)
            }),
        ],
        [
            NextRun::Answer(RunOutcome::Stopped(Withdrawal::Deadline)),
            NextRun::Answer(RunOutcome::FailedWithLast),
            NextRun::WaitThenRun {
                until: now + Duration::from_secs(2),
                after_failure: true
            },
        ]
    );
}

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
            run_delay(&retry, 3, 0.0),
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
            Some(Duration::from_secs(20)),
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
            run_delay(&retry, 3, 0.1),
        ],
        [
            Some(Duration::from_millis(2500)),
            Some(Duration::from_secs(12)),
            Some(Duration::from_secs(20)),
        ]
    );
}

#[test]
fn a_jitter_is_drawn_below_a_positive_factor_and_is_zero_otherwise() {
    let with = |max_jitter_factor| RetryConfig {
        max_jitter_factor,
        ..retry()
    };
    let drawn = (0..200)
        .map(|_| jitter(&with(Some(0.5))))
        .collect::<Vec<_>>();

    assert_eq!((jitter(&with(None)), jitter(&with(Some(0.0)))), (0.0, 0.0));
    assert!(drawn.iter().all(|factor| (0.0..0.5).contains(factor)));
    assert!(drawn.iter().any(|factor| *factor > 0.0));
}

#[test]
fn copy_end_runs_a_race_again_and_maps_a_failed_call_and_a_name_error() {
    let error = || anyhow::anyhow!("error");

    assert_eq!(
        [
            copy_end(&CopyError::CopySourceMissing {
                path: Path::new("data/ab/abab").into()
            }),
            copy_end(&CopyError::Race),
            copy_end(&CopyError::CallFailed(error())),
            copy_end(&CopyError::Permanent(error())),
            copy_end(&CopyError::Cancelled(error())),
        ],
        [
            RunEnd::RaceRunAgain,
            RunEnd::RaceRunAgain,
            RunEnd::CallFailed,
            RunEnd::Permanent,
            RunEnd::Cancelled,
        ]
    );
}

/// A limiter that grants each take, records what the shell tells it, and withdraws the call after
/// `withdraw_after` takes.
struct Recording {
    events: Mutex<Vec<&'static str>>,
    withdraw: CancellationToken,
}

impl Recording {
    fn new() -> Self {
        Self {
            events: Mutex::default(),
            withdraw: CancellationToken::new(),
        }
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }
}

impl RunSlots for Recording {
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        self.events
            .lock()
            .unwrap()
            .push(if immediate { "take now" } else { "take" });
        Box::pin(std::future::ready(Ok(Slot::new(()))))
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(async move {
            self.withdraw.cancelled().await;
            Withdrawal::Stopped
        })
    }

    fn waiting_after_failure(&self) {
        self.events.lock().unwrap().push("waiting after failure");
    }

    fn waiting_for_late_writes(&self) {
        self.events.lock().unwrap().push("waiting for late writes");
    }
}

fn answers() -> Answers<Result<SnapshotInfo, &'static str>> {
    Answers {
        failed: |_: Failed| Err("failed"),
        stopped: |cause| match cause {
            Withdrawal::Stopped => Err("stopped"),
            Withdrawal::Deadline => Err("deadline"),
        },
        saved: Ok,
        name_in_use: || Err("name in use"),
    }
}

/// A run that ended with `end`, and with a staged snapshot file whose writes land or never land
/// by `late`, when it has one.
fn ended(end: RunEnd, late: Option<Instant>) -> Ran<Result<SnapshotInfo, &'static str>> {
    Ran::Ended(Ended {
        end,
        failure: anyhow::anyhow!("run"),
        late: late.map(|until| LateWrite {
            until,
            own: Some(own_file()),
        }),
    })
}

/// The snapshot file that a lost publish staged.
fn own_file() -> OwnFile {
    OwnFile {
        path: Arc::from(Path::new("snapshots/0123")),
        info: info(),
    }
}

/// Runs `test` on a current-thread runtime whose time is paused, so each wait ends as soon as the
/// runtime has nothing else to do.
fn paused<T>(test: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(test)
}

#[test]
fn the_shell_tells_its_limiter_before_a_wait_for_late_writes_and_before_a_wait_after_a_failed_run()
{
    paused(async {
        let slots = Recording::new();
        let root = CancellationToken::new();
        let retry = retry();
        let shell = Shell {
            kind: Kind::Save,
            slots: &slots,
            root: &root,
            cancel: None,
            retry: &retry,
        };
        let runs = Mutex::new(0u32);
        let checks = Mutex::new(Vec::new());

        // Run 1 loses a publish; the check finds nothing. Run 2 fails a call. Run 3 answers.
        let answer = shell
            .call(
                answers(),
                |_| None,
                |_| {
                    let run = {
                        let mut runs = runs.lock().unwrap();
                        *runs += 1;
                        *runs
                    };
                    async move {
                        match run {
                            1 => ended(
                                RunEnd::CallFailed,
                                Some(super::now() + Duration::from_secs(60)),
                            ),
                            2 => ended(RunEnd::CallFailed, None),
                            _ => Ran::Answered(Ok(info())),
                        }
                    }
                },
                |own| {
                    assert_eq!(own, own_file());
                    checks.lock().unwrap().push("check");
                    async { Checked::Absent }
                },
            )
            .await;

        assert_eq!(
            (answer, slots.events(), checks.lock().unwrap().clone()),
            (
                Ok(info()),
                vec![
                    "take now",
                    "waiting for late writes",
                    "waiting after failure",
                    "take",
                    "waiting after failure",
                    "take",
                ],
                vec!["check"]
            )
        );
    })
}

#[test]
fn a_save_with_two_lost_publishes_waits_for_and_checks_each_and_answers_with_the_landed_one() {
    paused(async {
        let slots = Recording::new();
        let root = CancellationToken::new();
        let retry = retry();
        let shell = Shell {
            kind: Kind::Save,
            slots: &slots,
            root: &root,
            cancel: None,
            retry: &retry,
        };
        let runs = Mutex::new(0u32);
        let checks = Mutex::new(0u32);
        let started = super::now();

        let answer = shell
            .call(
                answers(),
                |_| None,
                |_| {
                    let run = {
                        let mut runs = runs.lock().unwrap();
                        *runs += 1;
                        *runs
                    };
                    async move {
                        match run {
                            1 | 2 => ended(
                                RunEnd::CallFailed,
                                Some(super::now() + Duration::from_secs(60)),
                            ),
                            _ => Ran::Answered(Err("a third run started")),
                        }
                    }
                },
                |_| {
                    let check = {
                        let mut checks = checks.lock().unwrap();
                        *checks += 1;
                        *checks
                    };
                    async move {
                        match check {
                            1 => Checked::Absent,
                            _ => Checked::Found(info()),
                        }
                    }
                },
            )
            .await;

        assert_eq!(
            (
                answer,
                *runs.lock().unwrap(),
                *checks.lock().unwrap(),
                super::now().duration_since(started) >= Duration::from_secs(122)
            ),
            (Ok(info()), 2, 2, true)
        );
    })
}

#[test]
fn a_withdrawal_during_the_wait_after_a_failed_run_ends_the_call_and_a_shutdown_ends_a_late_wait() {
    paused(async {
        let slots = Recording::new();
        let root = CancellationToken::new();
        let retry = retry();
        let shell = |kind| Shell {
            kind,
            slots: &slots,
            root: &root,
            cancel: None,
            retry: &retry,
        };
        slots.withdraw.cancel();
        let checks = Mutex::new(0u32);

        let withdrawn = shell(Kind::Delete)
            .call(
                answers(),
                |_| None,
                |_| async { ended(RunEnd::CallFailed, None) },
                |_| async { Checked::Absent },
            )
            .await;
        let stopping = root.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            stopping.cancel();
        });
        let started = tokio::time::Instant::now();
        let shut_down = shell(Kind::Save)
            .call(
                answers(),
                |_| None,
                |_| async {
                    ended(
                        RunEnd::CallFailed,
                        Some(super::now() + Duration::from_secs(3600)),
                    )
                },
                |_| {
                    *checks.lock().unwrap() += 1;
                    async { Checked::Found(info()) }
                },
            )
            .await;

        assert_eq!(
            (
                withdrawn,
                shut_down,
                *checks.lock().unwrap(),
                started.elapsed() < Duration::from_secs(60)
            ),
            (Err("stopped"), Err("stopped"), 0, true)
        );
    })
}

/// A limiter that grants each take with a slot whose drop it records, and that records each wait
/// for late writes.
#[derive(Default)]
struct SlotWatch {
    dropped: Arc<std::sync::atomic::AtomicBool>,
    late_waits: std::sync::atomic::AtomicUsize,
}

/// A slot that records its drop.
struct WatchedSlot(Arc<std::sync::atomic::AtomicBool>);

impl Drop for WatchedSlot {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl RunSlots for SlotWatch {
    fn take(&self, _immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        let slot = Slot::new(WatchedSlot(Arc::clone(&self.dropped)));
        Box::pin(std::future::ready(Ok(slot)))
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(std::future::pending())
    }

    fn waiting_for_late_writes(&self) {
        self.late_waits
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Runs one call whose run settles with `may_land` and a wait that ends when `gate` is
/// cancelled, while `root` is the root of the store. Gives the answer, and what the test saw
/// while the gate was closed: whether the slot was dropped, how many waits for late writes the
/// limiter heard of, and whether the finish ran.
async fn settled_call(
    may_land: bool,
    root: CancellationToken,
    shut_down: bool,
) -> (
    Result<SnapshotInfo, &'static str>,
    (bool, usize, bool),
    Option<Settled>,
) {
    let slots = SlotWatch::default();
    let retry = retry();
    let shell = Shell {
        kind: Kind::Save,
        slots: &slots,
        root: &root,
        cancel: None,
        retry: &retry,
    };
    let gate = CancellationToken::new();
    let finished = Arc::new(Mutex::new(None::<Settled>));
    let call = shell.call(
        answers(),
        |_| None,
        |_| {
            let (gate, finished) = (gate.clone(), Arc::clone(&finished));
            async move {
                Ran::Settling(Settle {
                    wait: Box::pin(async move { gate.cancelled().await }),
                    finish: Box::new(move |settled| {
                        *finished.lock().unwrap() = Some(settled);
                        Ran::Answered(Ok(info()))
                    }),
                    may_land,
                })
            }
        },
        |_| async { Checked::Absent },
    );
    let watching = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let seen = (
            slots.dropped.load(std::sync::atomic::Ordering::SeqCst),
            slots.late_waits.load(std::sync::atomic::Ordering::SeqCst),
            finished.lock().unwrap().is_some(),
        );
        if shut_down {
            root.cancel();
        } else {
            gate.cancel();
        }
        seen
    };
    let (answer, seen) = futures::join!(call, watching);
    let settled = *finished.lock().unwrap();
    (answer, seen, settled)
}

#[test]
fn the_shell_gives_the_slot_back_before_the_settle_and_finishes_the_run_only_after_it() {
    paused(async {
        assert_eq!(
            [
                settled_call(true, CancellationToken::new(), false).await,
                settled_call(false, CancellationToken::new(), false).await,
                settled_call(true, CancellationToken::new(), true).await,
            ],
            [
                (Ok(info()), (true, 1, false), Some(Settled::Waited)),
                (Ok(info()), (true, 0, false), Some(Settled::Waited)),
                (Ok(info()), (true, 1, false), Some(Settled::ShutDown)),
            ]
        );
    })
}
