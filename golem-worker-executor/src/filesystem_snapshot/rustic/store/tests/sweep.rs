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

//! Two deletes of one scope whose prunes are both due, in each order of their blob calls.
//!
//! Each delete has its own store over its own scripted storage, and the two storages share one
//! in-memory storage, as two executors share a bucket. Each blob call of the prune protocol is a
//! step: it waits until the test gives its delete one step. So the test sets the order of the
//! calls of the two deletes. The grace period is short, so a prune writes new markers of its claim
//! while it runs. A timer starts each such write, so its place in the order can change from run to
//! run, and the log of a case gives the call that took each step.

use super::*;
use futures::TryStreamExt;
use tokio::task::JoinHandle;

/// The operation labels of the blob calls of the prune protocol. The listing of the packs by a
/// prune is a step too, and it is the start of the prune. The delete of a snapshot file is the
/// forget of a delete.
const STEP_LABELS: &[&str] = &[
    "write_freed",
    "read_freed",
    "list_snapshots",
    "read_ledger",
    "list_freed",
    "list_data",
    "list_claims",
    "write_marker",
    "write_claim",
    "delete_marker",
    "refresh_claim",
    "final_marker",
    "write_ledger",
    "list_ledgers",
    "delete_ledger",
    "delete_freed",
    "delete_claim",
    "list_claim_directories",
    "list_claim_blobs",
    "delete_claims",
];

/// The most steps that one delete takes before the test gives up on it, without the writes of
/// new markers. Those writes end with the prune.
const MOST_STEPS: usize = 64;

fn is_step(op_label: &str, path: &Path) -> bool {
    STEP_LABELS.contains(&op_label)
        || (op_label == "list" && path == Path::new("data"))
        || is_forget(op_label, path)
}

/// Tells whether the call writes or deletes, so that it can reach the storage late.
fn can_be_late(op_label: &str) -> bool {
    op_label.starts_with("write_") || op_label.starts_with("delete") || op_label == "final_marker"
}

/// Tells whether the call writes a new marker of a claim while its prune runs. A timer starts
/// such a call, so the schedule neither counts it nor makes it fail.
fn is_refresh(op_label: &str) -> bool {
    op_label == "refresh_claim"
}

/// The grace period of the deletes of a case. The claim of a prune gets a new marker at each
/// quarter of it, so the markers come while the prune runs.
const SWEEP_GRACE: Duration = Duration::from_millis(16);

fn is_forget(op_label: &str, path: &Path) -> bool {
    op_label == "delete" && path.starts_with("snapshots")
}

fn is_prune_start(op_label: &str, path: &str) -> bool {
    op_label == "list" && path == "data"
}

/// The order of the steps of one case: the delete that goes first, and the numbers of steps that
/// the deletes take in turn. A turn counts each step, also the write of a new marker. After the
/// listed turns, the delete whose turn is next runs to its end, and then the other one does.
/// `fail` refuses one step of one delete. `late` makes one write or delete of one delete give no
/// answer at its step, and reach the storage after the given number of further steps of the case.
/// `drop` drops one delete right after one of its steps, as a caller that stops waiting does. The
/// steps of `fail`, `late` and `drop` are numbered without the writes of new markers.
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub(super) first: usize,
    pub(super) turns: Vec<usize>,
    pub(super) fail: Option<(usize, usize)>,
    pub(super) late: Option<(usize, usize, usize)>,
    pub(super) drop: Option<(usize, usize)>,
}

/// One step that a delete took, or a late call that reached the storage, in the order of all
/// steps of the case.
#[derive(Clone, Debug)]
struct Step {
    delete: usize,
    op_label: &'static str,
    path: String,
    /// The call gave an error to its caller: it was refused, or it was late.
    failed: bool,
    /// The call reached the storage: a step that passed, or the landing of a late call.
    effect: bool,
    /// The entry is the landing of a late call, not a call.
    landed: bool,
}

/// One delete of the case: its store, its scripted storage and its task.
struct Delete {
    store: Arc<RusticSnapshotStore>,
    storage: Arc<ScriptedBlobStorage>,
    task: JoinHandle<Result<(), SnapshotStoreError>>,
    /// The steps that the delete took, other than the writes of new markers.
    taken: usize,
    /// Whether the case dropped the delete. A dropped delete makes a call only for the release of
    /// its claim.
    dropped: bool,
}

impl Delete {
    /// Tells whether the delete ended and its store has no work left, such as the release of the
    /// claim of a dropped delete.
    fn finished(&self) -> bool {
        self.task.is_finished() && self.store.work_in_flight() == 0
    }
}

/// The state of a case while it runs.
struct Case {
    deletes: [Delete; 2],
    log: Vec<Step>,
    schedule: Schedule,
    /// A late call that has not reached the storage: its delete, the log length at which it
    /// lands, and its step.
    pending: Option<(usize, usize, Step)>,
}

/// Waits until the condition holds, and gives false when it does not hold within [`LIMIT`]. The
/// wait yields to the runtime between two checks, so a step ends as soon as it can.
async fn until(condition: impl Fn() -> bool) -> bool {
    tokio::time::timeout(LIMIT, async {
        futures::stream::repeat(())
            .then(|()| tokio::task::yield_now())
            .take_while(|()| std::future::ready(!condition()))
            .for_each(|()| std::future::ready(()))
            .await
    })
    .await
    .is_ok()
}

/// Waits until the delete waits for a step or ended. While the prune waits for the listing of
/// the packs, it also waits until the first new marker of the claim waits for a step, so each
/// prune writes a new marker right after that listing.
async fn settle(delete: &Delete) -> bool {
    until(|| {
        let waiting = delete.storage.waiting_steps();
        delete.finished() || waiting > usize::from(prune_waits(&delete.storage))
    })
    .await
}

/// Tells whether the prune of the delete waits for its step: the storage has a call for the
/// listing of the packs that took no step yet.
fn prune_waits(storage: &ScriptedBlobStorage) -> bool {
    let starts = |calls: Vec<(&'static str, String)>| {
        calls
            .iter()
            .filter(|(op_label, path)| is_prune_start(op_label, path))
            .count()
    };
    starts(storage.calls()) > starts(storage.took())
}

impl Case {
    /// Lands the pending late call when its time came, or when `now` is true.
    async fn land(&mut self, now: bool) -> Result<(), String> {
        let due = self
            .pending
            .as_ref()
            .is_some_and(|(_, at, _)| now || self.log.len() >= *at);
        if !due {
            return Ok(());
        }
        let Some((who, _, step)) = self.pending.take() else {
            return Ok(());
        };
        let storage = self.deletes[who].storage.clone();
        let before = storage.landed();
        storage.land_late();
        if !until(|| storage.landed() > before).await {
            return Err(format!(
                "the late call {} {} did not land",
                step.op_label, step.path
            ));
        }
        self.log.push(Step {
            failed: false,
            effect: true,
            landed: true,
            ..step
        });
        Ok(())
    }

    /// Gives the delete one step and waits until the step ended and the delete waits again or
    /// ended.
    async fn take_step(&mut self, who: usize) -> Result<(), String> {
        if self.deletes[who].finished() {
            return Ok(());
        }
        if self.deletes[who].dropped {
            let delete = &self.deletes[who];
            if !until(|| delete.storage.waiting_steps() > 0 || delete.finished()).await {
                return Err(format!("the dropped delete {who} did not end"));
            }
            if delete.finished() {
                return Ok(());
            }
        }
        let storage = self.deletes[who].storage.clone();
        let (before, taker) = (storage.stepped(), storage.took().len());
        storage.step();
        // The drop cancels the operation of the delete, and a call that already waited for a step
        // then ends without the step. So for a dropped delete, the step can stay with no call to
        // take it, and the test takes it back.
        let dropped = self.deletes[who].dropped;
        let untaken = || {
            dropped
                && storage.stepped() == before
                && storage.waiting_steps() == 0
                && storage.took().len() == taker
        };
        if !until(|| storage.stepped() > before || untaken()).await {
            return Err(format!(
                "a step of the dropped delete {who} was neither taken nor left; its last calls {:?}",
                storage.calls().iter().rev().take(6).collect::<Vec<_>>()
            ));
        }
        if untaken() && storage.take_back_step() {
            return Ok(());
        }
        if !until(|| storage.stepped() > before).await {
            return Err(format!(
                "a step of delete {who} did not end; its last calls {:?}",
                storage.calls().iter().rev().take(6).collect::<Vec<_>>()
            ));
        }
        let took = storage.took();
        if took.len() != taker + 1 {
            return Err(format!(
                "one step of delete {who} was taken by {:?}",
                took.get(taker..)
            ));
        }
        let (op_label, path) = took
            .get(taker)
            .cloned()
            .ok_or_else(|| format!("a step of delete {who} ended, and no call took it"))?;
        let number = self.deletes[who].taken;
        let counted = !is_refresh(op_label);
        let refused = counted && self.schedule.fail == Some((who, number));
        let late = self.schedule.late.filter(|(late_who, late_step, _)| {
            counted && (*late_who, *late_step) == (who, number) && can_be_late(op_label)
        });
        let step = Step {
            delete: who,
            op_label,
            path,
            failed: refused || late.is_some(),
            effect: !refused && late.is_none(),
            landed: false,
        };
        self.log.push(step.clone());
        if let Some((_, _, delay)) = late {
            self.pending = Some((who, self.log.len() + delay, step));
        }
        self.deletes[who].taken += usize::from(counted);
        if self.deletes[who].taken > MOST_STEPS {
            return Err(format!("delete {who} took more than {MOST_STEPS} steps"));
        }
        self.land(false).await?;
        if counted && self.schedule.drop == Some((who, number)) {
            self.deletes[who].task.abort();
            self.deletes[who].dropped = true;
            // An abort only asks the task to end. Its waiting call counts as waiting until the
            // task ends, so the case gives no step before that.
            let task = &self.deletes[who].task;
            if !until(|| task.is_finished()).await {
                return Err(format!("the dropped delete {who} did not end"));
            }
            // The drop counts as a failed step of the delete, so the rules on a delete that
            // stopped after its claim apply to it.
            self.log.push(Step {
                delete: who,
                op_label: "drop",
                path: String::new(),
                failed: true,
                effect: false,
                landed: false,
            });
        }
        if settle(&self.deletes[who]).await {
            Ok(())
        } else {
            let calls = self.deletes[who].storage.calls();
            let last = calls.iter().rev().take(6).collect::<Vec<_>>();
            Err(format!(
                "delete {who} did not reach its next step; its last calls {last:?}; the other delete waits {}",
                self.deletes[(who + 1) % 2].storage.waiting_steps()
            ))
        }
    }

    /// Gives the delete the steps.
    async fn take_steps(&mut self, who: usize, steps: usize) -> Result<(), String> {
        futures::stream::iter(0..steps)
            .map(Ok)
            .try_fold(self, |case, _| async move {
                case.take_step(who).await?;
                Ok::<_, String>(case)
            })
            .await
            .map(|_| ())
    }

    /// Gives the delete steps until it ends.
    async fn run_to_end(&mut self, who: usize) -> Result<(), String> {
        futures::stream::unfold(self, |case| async move {
            if case.deletes[who].finished() {
                None
            } else {
                let stepped = case.take_step(who).await;
                Some((stepped, case))
            }
        })
        .try_for_each(|()| std::future::ready(Ok(())))
        .await
    }
}

/// What a case found.
#[derive(Debug)]
pub(super) struct Found {
    pub(super) prunes: usize,
    pub(super) failed: bool,
    pub(super) steps: [usize; 2],
}

/// Runs the two deletes of a copy of the prepared scope in the order of the schedule, and checks
/// the rules of the prune protocol.
pub(super) async fn run_case(
    shared: &Arc<InMemoryBlobStorage>,
    prepared: &SnapshotScope,
    schedule: &Schedule,
) -> Result<Found, String> {
    let scope = new_scope();
    store(shared.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO))
        .copy_scope(prepared, &scope)
        .await
        .map_err(|error| format!("the copy of the prepared scope failed: {error}"))?;
    let deletes = [0, 1].map(|who| {
        let counter = Arc::new(AtomicUsize::new(0));
        let (fail, late) = (schedule.fail, schedule.late);
        let storage = ScriptedBlobStorage::new(shared.clone(), move |op_label, path| {
            if is_refresh(op_label) {
                Script::Step {
                    refuse: false,
                    late: false,
                }
            } else if is_step(op_label, path) {
                let number = counter.fetch_add(1, Ordering::SeqCst);
                Script::Step {
                    refuse: fail == Some((who, number)),
                    late: can_be_late(op_label)
                        && late.is_some_and(|(late_who, late_step, _)| {
                            (late_who, late_step) == (who, number)
                        }),
                }
            } else {
                Script::Pass
            }
        });
        let deleting = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, SWEEP_GRACE));
        let scope = scope.clone();
        let task = tokio::spawn({
            let deleting = deleting.clone();
            async move { deleting.delete(&scope, &name(["p-1", "p-2"][who])).await }
        });
        Delete {
            store: deleting,
            storage,
            task,
            taken: 0,
            dropped: false,
        }
    });
    let mut case = Case {
        deletes,
        log: Vec::new(),
        schedule: schedule.clone(),
        pending: None,
    };
    if !(settle(&case.deletes[0]).await && settle(&case.deletes[1]).await) {
        return Err("a delete did not reach its first step".to_string());
    }
    let turns = schedule.turns.clone();
    let ran = run_turns(&mut case, schedule, &turns).await;
    if let Err(error) = ran {
        return Err(format!(
            "{error}; schedule {schedule:?}; steps {}",
            order(&case.log)
        ));
    }
    let results =
        futures::future::join_all(case.deletes.iter_mut().map(|delete| &mut delete.task)).await;
    check(shared, &scope, schedule, &case.log, results).await
}

/// Runs the turns of the schedule, then each delete to its end, and then lands a late call that
/// did not land yet.
async fn run_turns(case: &mut Case, schedule: &Schedule, turns: &[usize]) -> Result<(), String> {
    let next = futures::stream::iter(turns.iter().enumerate())
        .map(Ok)
        .try_fold(&mut *case, |case, (turn, steps)| async move {
            case.take_steps((schedule.first + turn) % 2, *steps).await?;
            Ok::<_, String>(case)
        })
        .await
        .map(|_| (schedule.first + turns.len()) % 2)?;
    case.run_to_end(next).await?;
    case.run_to_end((next + 1) % 2).await?;
    case.land(true).await
}

/// Gives the steps of the log as text: the delete and the operation label, with `!` for a call
/// that gave an error.
fn order(log: &[Step]) -> String {
    log.iter()
        .map(|step| {
            let mark = match (step.failed, step.effect) {
                (true, _) => "!",
                (false, true) => "",
                (false, false) => "?",
            };
            format!("{}:{}{}", step.delete, step.op_label, mark)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Gives the index in the log at which the delete forgot its snapshot, when its forget reached
/// the storage.
fn forgotten_at(log: &[Step], who: usize) -> Option<usize> {
    log.iter().position(|step| {
        step.delete == who && step.effect && is_forget(step.op_label, Path::new(&step.path))
    })
}

/// Gives each claim, or marker of a claim, that a delete wrote and that stays, when the first
/// failed call of that delete, or its drop, came after it took its claim and before it called for
/// the listing of the packs. A claim that the other delete wrote later at the same path is not the claim of the
/// delete. A blob whose own delete failed is left out, because no call can remove it then, and a
/// claim or a marker that stays only delays a prune.
fn kept_claims(log: &[Step], claims: &[String]) -> Vec<String> {
    [0, 1]
        .into_iter()
        .filter_map(|who| {
            let own = |step: &Step| step.delete == who;
            let claimed_at = log.iter().position(|step| {
                own(step) && step.op_label == "write_claim" && step.effect && !step.failed
            })?;
            let failed_at = log.iter().position(|step| own(step) && step.failed)?;
            let pruning = log[..=failed_at]
                .iter()
                .any(|step| own(step) && is_prune_start(step.op_label, &step.path));
            (claimed_at < failed_at && !pruning).then_some((who, claimed_at))
        })
        .flat_map(|(who, claimed_at)| {
            let claim = &log[claimed_at].path;
            let taken_again = log[claimed_at..].iter().any(|step| {
                step.delete != who
                    && step.op_label == "write_claim"
                    && step.effect
                    && step.path == *claim
            });
            let written = log
                .iter()
                .filter(|step| {
                    step.delete == who
                        && step.effect
                        && matches!(
                            step.op_label,
                            "write_marker" | "refresh_claim" | "final_marker"
                        )
                })
                .map(|step| step.path.clone())
                .chain((!taken_again).then(|| claim.clone()))
                .collect::<Vec<_>>();
            let refused = log
                .iter()
                .filter(|step| {
                    step.delete == who
                        && step.failed
                        && matches!(step.op_label, "delete_claim" | "delete_marker")
                })
                .map(|step| step.path.clone())
                .collect::<Vec<_>>();
            claims
                .iter()
                .filter(move |path| written.contains(path) && !refused.contains(path))
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Gives the claim of each delete whose prune started and that wrote no ledger entry, when that
/// claim is gone. A prune that started keeps its claim, so the next prune waits for the hold.
fn started_claims_gone(log: &[Step], claims: &[String]) -> Vec<String> {
    [0, 1]
        .into_iter()
        .filter_map(|who| {
            let own = |step: &&Step| step.delete == who;
            let claim = log
                .iter()
                .filter(own)
                .find(|step| step.op_label == "write_claim" && step.effect && !step.failed)?;
            let started = log
                .iter()
                .filter(own)
                .any(|step| is_prune_start(step.op_label, &step.path));
            let ledger_written = log
                .iter()
                .filter(own)
                .any(|step| step.op_label == "write_ledger" && step.effect);
            (started && !ledger_written && !claims.contains(&claim.path))
                .then(|| claim.path.clone())
        })
        .collect()
}

/// Gives each delete whose prune started and that made no call for its final marker. The prune
/// started when the delete called for the listing of the packs. A delete that called to delete its
/// claim released it, because each attempt of its prune found a snapshot file gone, so it writes
/// no final marker.
fn final_markers_missing(log: &[Step]) -> Vec<usize> {
    [0, 1]
        .into_iter()
        .filter(|who| {
            let own = log
                .iter()
                .filter(|step| step.delete == *who && !step.landed);
            let (mut started, mut released, mut marked) = (false, false, false);
            own.for_each(|step| {
                started |= is_prune_start(step.op_label, &step.path);
                released |= step.op_label == "delete_claim";
                marked |= step.op_label == "final_marker";
            });
            started && !released && !marked
        })
        .collect()
}

/// Gives each delete that called for a final marker after a final marker call of it that gave no
/// error.
fn final_markers_repeated(log: &[Step]) -> Vec<usize> {
    [0, 1]
        .into_iter()
        .filter(|who| {
            log.iter()
                .filter(|step| {
                    step.delete == *who && !step.landed && step.op_label == "final_marker"
                })
                .skip_while(|step| step.failed)
                .nth(1)
                .is_some()
        })
        .collect()
}

/// Checks the rules on the end state of a case.
async fn check(
    shared: &Arc<InMemoryBlobStorage>,
    scope: &SnapshotScope,
    schedule: &Schedule,
    log: &[Step],
    results: Vec<Result<Result<(), SnapshotStoreError>, tokio::task::JoinError>>,
) -> Result<Found, String> {
    let failed = log.iter().any(|step| step.failed);
    let prune_starts = log
        .iter()
        .enumerate()
        .filter(|(_, step)| step.effect && is_prune_start(step.op_label, &step.path))
        .collect::<Vec<_>>();
    let prunes = prune_starts.len();
    let fail = |rule: &str| {
        Err(format!(
            "{rule}; schedule {schedule:?}; steps {}",
            order(log)
        ))
    };
    if prunes > 1 {
        return fail("more than one prune ran");
    }
    if !failed && prunes != 1 {
        return fail(&format!(
            "no prune ran, although a prune was due and no call failed: {results:?}"
        ));
    }
    if !failed && results.iter().any(|result| !matches!(result, Ok(Ok(())))) {
        return fail(&format!("a delete failed with no failed call: {results:?}"));
    }
    let claims = blobs(&**shared, &scope.0, "golem/prune-claims/").await;
    if !failed && !claims.is_empty() {
        return fail(&format!("claims stay: {claims:?}"));
    }
    let released = kept_claims(log, &claims);
    if !released.is_empty() {
        return fail(&format!(
            "a delete that failed after its claim and before its prune kept its claim: {released:?}"
        ));
    }
    let dropped = started_claims_gone(log, &claims);
    if !dropped.is_empty() {
        return fail(&format!(
            "a delete whose prune started and wrote no ledger lost its claim: {dropped:?}"
        ));
    }
    let unmarked = final_markers_missing(log);
    if !unmarked.is_empty() {
        return fail(&format!(
            "a delete whose prune started and kept its claim made no final marker call: {unmarked:?}"
        ));
    }
    let repeated = final_markers_repeated(log);
    if !repeated.is_empty() {
        return fail(&format!(
            "a delete made a final marker call after one that succeeded: {repeated:?}"
        ));
    }
    let records = blobs(&**shared, &scope.0, "golem/prune-freed/").await;
    let entries = blobs(&**shared, &scope.0, "golem/prune-ledgers/").await;
    let ledger = ledger(shared, scope).await;
    let written = |path: &str| {
        log.iter()
            .find(|step| step.op_label == "write_freed" && step.effect && step.path == path)
            .map(|step| step.delete)
    };
    let lost = log
        .iter()
        .enumerate()
        .filter(|(_, step)| step.op_label == "delete_freed" && step.effect)
        .filter_map(|(deleted_at, step)| {
            let pruner = step.delete;
            let decided_at = log[..deleted_at].iter().rposition(|earlier| {
                earlier.delete == pruner
                    && matches!(
                        earlier.op_label,
                        "list_freed" | "read_freed" | "list_snapshots"
                    )
            })?;
            let writer = written(&step.path)?;
            let settled = forgotten_at(log, writer).is_some_and(|forgot| forgot < decided_at);
            (!settled).then(|| step.path.clone())
        })
        .collect::<Vec<_>>();
    if !lost.is_empty() {
        return fail(&format!(
            "a prune deleted a record whose snapshot was not gone at its count: {lost:?}"
        ));
    }
    if let Some((start, pruner)) = prune_starts
        .first()
        .map(|(index, step)| (*index, step.delete))
    {
        let wrote_ledger = log[start..]
            .iter()
            .find(|step| step.delete == pruner && step.op_label == "write_ledger" && step.effect);
        if let Some(written) = wrote_ledger {
            let written_ms = Path::new(&written.path)
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.split('-').next())
                .and_then(|ms| ms.parse::<u64>().ok());
            if ledger.last_prune.map(|last| last.to_millis()) != written_ms {
                return fail(&format!(
                    "the ledger {ledger:?} is not the entry of the prune {written_ms:?}; entries {entries:?}"
                ));
            }
        }
        let counted_at = log[..start].iter().rposition(|step| {
            step.delete == pruner
                && matches!(
                    step.op_label,
                    "list_freed" | "read_freed" | "list_snapshots"
                )
        });
        let gone = log
            .iter()
            .enumerate()
            .filter(|(_, step)| step.op_label == "write_freed" && step.effect)
            .filter(|(index, _)| counted_at.is_none_or(|counted| *index > counted))
            .filter(|(_, step)| !records.contains(&step.path))
            .map(|(_, step)| step.path.clone())
            .collect::<Vec<_>>();
        if !gone.is_empty() {
            return fail(&format!(
                "records that the prune did not count are gone: {gone:?}"
            ));
        }
    }
    let steps = [0, 1].map(|who| log.iter().filter(|step| step.delete == who).count());
    Ok(Found {
        prunes,
        failed,
        steps,
    })
}
