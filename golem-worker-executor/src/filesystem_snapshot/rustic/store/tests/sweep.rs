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
/// prune is a step too, and it is the start of the prune.
const STEP_LABELS: &[&str] = &[
    "write_freed",
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
    "delete_claims",
];

/// The most steps that one delete takes before the test gives up on it.
const MOST_STEPS: usize = 64;

fn is_step(op_label: &str, path: &Path) -> bool {
    STEP_LABELS.contains(&op_label) || (op_label == "list" && path == Path::new("data"))
}

fn is_prune_start(op_label: &str, path: &str) -> bool {
    op_label == "list" && path == "data"
}

/// The order of the steps of one case: the delete that goes first, and the numbers of steps that
/// the deletes take in turn. After the listed turns, the delete whose turn is next runs to its
/// end, and then the other one does. `fail` refuses one step of one delete.
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub(super) first: usize,
    pub(super) turns: Vec<usize>,
    pub(super) fail: Option<(usize, usize)>,
}

/// One step that a delete took, in the order of all steps of the case.
#[derive(Clone, Debug)]
struct Step {
    delete: usize,
    op_label: &'static str,
    path: String,
    refused: bool,
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

/// Gives the delete one step and waits until the step ended and the delete waits again or ended.
async fn take_step(
    deletes: &mut [Delete; 2],
    who: usize,
    fail: Option<(usize, usize)>,
    log: &mut Vec<Step>,
) -> Result<(), String> {
    let delete = &mut deletes[who];
    if delete.finished() {
        return Ok(());
    }
    let calls = delete.storage.calls();
    let (op_label, path) = calls
        .iter()
        .rev()
        .find(|(op_label, path)| is_step(op_label, Path::new(path)))
        .cloned()
        .ok_or_else(|| format!("delete {who} waits for a step with no step call"))?;
    let before = delete.storage.stepped();
    delete.storage.step();
    let storage = delete.storage.clone();
    if !until(|| storage.stepped() > before).await {
        return Err(format!(
            "the step {op_label} {path} of delete {who} did not end"
        ));
    }
    log.push(Step {
        delete: who,
        op_label,
        path,
        refused: fail == Some((who, delete.taken)),
    });
    delete.taken += 1;
    if delete.taken > MOST_STEPS {
        return Err(format!("delete {who} took more than {MOST_STEPS} steps"));
    }
    if settle(delete).await {
        Ok(())
    } else {
        Err(format!("delete {who} did not reach its next step"))
    }
}

/// Gives the delete steps until it ends.
async fn run_to_end(
    deletes: &mut [Delete; 2],
    who: usize,
    fail: Option<(usize, usize)>,
    log: &mut Vec<Step>,
) -> Result<(), String> {
    futures::stream::iter(0..=MOST_STEPS)
        .map(Ok)
        .try_fold((deletes, log), |(deletes, log), _| async move {
            take_step(deletes, who, fail, log).await?;
            Ok::<_, String>((deletes, log))
        })
        .await
        .map(|_| ())
}

/// What a case found.
#[derive(Debug)]
pub(super) struct Found {
    pub(super) prunes: usize,
    pub(super) refused: bool,
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
        let fail = schedule.fail;
        let storage = ScriptedBlobStorage::new(shared.clone(), move |op_label, path| {
            if is_step(op_label, path) {
                let number = counter.fetch_add(1, Ordering::SeqCst);
                Script::Step {
                    refuse: fail == Some((who, number)),
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
    let mut deletes = deletes;
    let mut log = Vec::new();
    if !(settle(&deletes[0]).await && settle(&deletes[1]).await) {
        return Err("a delete did not reach its first step".to_string());
    }
    let (next, deletes, log) = futures::stream::iter(schedule.turns.iter().enumerate())
        .map(Ok)
        .try_fold(
            (schedule.first, &mut deletes, &mut log),
            |(_, deletes, log), (turn, steps)| async move {
                let who = (schedule.first + turn) % 2;
                futures::stream::iter(0..*steps)
                    .map(Ok)
                    .try_fold((deletes, log), |(deletes, log), _| async move {
                        take_step(deletes, who, schedule.fail, log).await?;
                        Ok::<_, String>((deletes, log))
                    })
                    .await
                    .map(|(deletes, log)| ((who + 1) % 2, deletes, log))
            },
        )
        .await?;
    run_to_end(deletes, next, schedule.fail, log).await?;
    run_to_end(deletes, (next + 1) % 2, schedule.fail, log).await?;
    let results =
        futures::future::join_all(deletes.iter_mut().map(|delete| &mut delete.task)).await;
    check(shared, &scope, schedule, log, results).await
}

/// Checks the rules on the end state of a case.
async fn check(
    shared: &Arc<InMemoryBlobStorage>,
    scope: &SnapshotScope,
    schedule: &Schedule,
    log: &[Step],
    results: Vec<Result<Result<(), SnapshotStoreError>, tokio::task::JoinError>>,
) -> Result<Found, String> {
    let refused = log.iter().any(|step| step.refused);
    let prune_starts = log
        .iter()
        .enumerate()
        .filter(|(_, step)| !step.refused && is_prune_start(step.op_label, &step.path))
        .collect::<Vec<_>>();
    let prunes = prune_starts.len();
    let order = || {
        log.iter()
            .map(|step| {
                format!(
                    "{}:{}{}",
                    step.delete,
                    step.op_label,
                    if step.refused { "!" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let fail = |rule: &str| Err(format!("{rule}; schedule {schedule:?}; steps {}", order()));
    if prunes > 1 {
        return fail("more than one prune ran");
    }
    if !refused && prunes != 1 {
        return fail("no prune ran, although a prune was due and no call failed");
    }
    if !refused && results.iter().any(|result| !matches!(result, Ok(Ok(())))) {
        return fail(&format!(
            "a delete failed with no refused call: {results:?}"
        ));
    }
    let claims = blobs(&**shared, &scope.0, "golem/prune-claims/").await;
    if !refused && !claims.is_empty() {
        return fail(&format!("claims stay: {claims:?}"));
    }
    let records = blobs(&**shared, &scope.0, "golem/prune-freed/").await;
    let entries = blobs(&**shared, &scope.0, "golem/prune-ledgers/").await;
    let ledger = ledger(shared, scope).await;
    if let Some((start, pruner)) = prune_starts
        .first()
        .map(|(index, step)| (*index, step.delete))
    {
        let wrote_ledger = log[start..]
            .iter()
            .find(|step| step.delete == pruner && step.op_label == "write_ledger" && !step.refused);
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
        let counted_at = log[..start]
            .iter()
            .rposition(|step| step.delete == pruner && step.op_label == "list_freed");
        let written_records = log
            .iter()
            .enumerate()
            .filter(|(_, step)| step.op_label == "write_freed" && !step.refused)
            .collect::<Vec<_>>();
        let lost = written_records
            .iter()
            .filter(|(index, _)| counted_at.is_none_or(|counted| *index > counted))
            .filter(|(_, step)| !records.contains(&step.path))
            .map(|(_, step)| step.path.clone())
            .collect::<Vec<_>>();
        if !lost.is_empty() {
            return fail(&format!(
                "records that the prune did not count are gone: {lost:?}"
            ));
        }
    }
    let steps = [0, 1].map(|who| log.iter().filter(|step| step.delete == who).count());
    Ok(Found {
        prunes,
        refused,
        steps,
    })
}
