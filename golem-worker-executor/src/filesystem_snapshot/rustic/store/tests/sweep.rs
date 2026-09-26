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
//! calls of the two deletes, and nothing else changes it.

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
    "read_claim",
    "write_claim",
    "refresh_claim",
    "write_ledger",
    "list_ledgers",
    "delete_ledger",
    "delete_freed",
    "delete_claim",
    "list_claim_directories",
    "list_claim_blobs",
    "delete_claims",
];

/// The most steps that one delete takes before the test gives up on it.
const MOST_STEPS: usize = 64;

fn is_step(op_label: &str, path: &Path) -> bool {
    STEP_LABELS.contains(&op_label)
        || (op_label == "list" && path == Path::new("data"))
        || is_forget(op_label, path)
}

/// Tells whether the call writes or deletes, so that it can reach the storage late.
fn can_be_late(op_label: &str) -> bool {
    op_label.starts_with("write_") || op_label.starts_with("delete") || op_label == "refresh_claim"
}

fn is_forget(op_label: &str, path: &Path) -> bool {
    op_label == "delete" && path.starts_with("snapshots")
}

fn is_prune_start(op_label: &str, path: &str) -> bool {
    op_label == "list" && path == "data"
}

/// The order of the steps of one case: the delete that goes first, and the numbers of steps that
/// the deletes take in turn. After the listed turns, the delete whose turn is next runs to its
/// end, and then the other one does. `fail` refuses one step of one delete. `late` makes one
/// write or delete of one delete give no answer at its step, and reach the storage after the
/// given number of further steps of the case.
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub(super) first: usize,
    pub(super) turns: Vec<usize>,
    pub(super) fail: Option<(usize, usize)>,
    pub(super) late: Option<(usize, usize, usize)>,
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
}

/// One delete of the case: its scripted storage and its task.
struct Delete {
    storage: Arc<ScriptedBlobStorage>,
    task: JoinHandle<Result<(), SnapshotStoreError>>,
    taken: usize,
}

impl Delete {
    fn finished(&self) -> bool {
        self.task.is_finished()
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

/// Waits until the delete waits for a step or ended.
async fn settle(delete: &Delete) -> bool {
    until(|| delete.storage.waiting_steps() > 0 || delete.finished()).await
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
        let calls = self.deletes[who].storage.calls();
        let (op_label, path) = calls
            .iter()
            .rev()
            .find(|(op_label, path)| is_step(op_label, Path::new(path)))
            .cloned()
            .ok_or_else(|| format!("delete {who} waits for a step with no step call"))?;
        let number = self.deletes[who].taken;
        let refused = self.schedule.fail == Some((who, number));
        let late = self.schedule.late.filter(|(late_who, late_step, _)| {
            (*late_who, *late_step) == (who, number) && can_be_late(op_label)
        });
        let storage = self.deletes[who].storage.clone();
        let before = storage.stepped();
        storage.step();
        if !until(|| storage.stepped() > before).await {
            return Err(format!(
                "the step {op_label} {path} of delete {who} did not end"
            ));
        }
        let step = Step {
            delete: who,
            op_label,
            path,
            failed: refused || late.is_some(),
            effect: !refused && late.is_none(),
        };
        self.log.push(step.clone());
        if let Some((_, _, delay)) = late {
            self.pending = Some((who, self.log.len() + delay, step));
        }
        self.deletes[who].taken += 1;
        if self.deletes[who].taken > MOST_STEPS {
            return Err(format!("delete {who} took more than {MOST_STEPS} steps"));
        }
        self.land(false).await?;
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
            if is_step(op_label, path) {
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
        let deleting = store(
            storage.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        );
        let scope = scope.clone();
        let task =
            tokio::spawn(async move { deleting.delete(&scope, &name(["p-1", "p-2"][who])).await });
        Delete {
            storage,
            task,
            taken: 0,
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
    case.take_steps(next, MOST_STEPS + 1).await?;
    case.take_steps((next + 1) % 2, MOST_STEPS + 1).await?;
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
        return fail("no prune ran, although a prune was due and no call failed");
    }
    if !failed && results.iter().any(|result| !matches!(result, Ok(Ok(())))) {
        return fail(&format!("a delete failed with no failed call: {results:?}"));
    }
    let claims = blobs(&**shared, &scope.0, "golem/prune-claims/").await;
    if !failed && !claims.is_empty() {
        return fail(&format!("claims stay: {claims:?}"));
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
