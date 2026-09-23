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

//! The phases that run the operations of several agents at the same time, in one pod.
//!
//! Each agent has its own repository (see [`super::agents`]). A measured step starts the
//! operations of all agents at the same time on the async runtime. Each operation runs on a
//! blocking thread of the runtime, as the executor runs it. The requests of all agents go into the
//! record of the step.

use super::agents::copy_first_agent;
use super::measure::measure;
use super::report::{Outcome, StepRecord, StepStatus, TreeFacts};
use super::requests::times;
use super::trees::{self, CopyCounts};
use super::{
    COLD_SAVE, PhaseContext, PhaseOutcome, WARM_SAVE, failed, saved_hash, snapshot_name,
    without_save,
};
use crate::filesystem_snapshot::rustic::SaveReport;
use futures::{StreamExt, TryStreamExt};
use serde_json::{Map, Value, json};
use std::future::Future;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The result of the operation of one agent in a batch.
struct AgentRun<T> {
    /// The time from the start of the operation to its end.
    time: Duration,
    /// The time from the start of the batch to the end of the operation.
    finished: Duration,
    result: anyhow::Result<T>,
}

/// Starts the operations at the same time, and gives the result of each, in the order of the
/// operations.
async fn batch<T>(
    operations: impl IntoIterator<Item = impl Future<Output = anyhow::Result<T>>>,
) -> Box<[AgentRun<T>]> {
    let started = Instant::now();
    futures::future::join_all(operations.into_iter().map(|operation| async move {
        let begin = Instant::now();
        let result = operation.await;
        AgentRun {
            time: begin.elapsed(),
            finished: started.elapsed(),
            result,
        }
    }))
    .await
    .into_boxed_slice()
}

/// Gives the details of a batch: the number of agents, the times of their operations, the number
/// of operations that failed, and the error of the operation that failed first.
fn batch_details<T>(runs: &[AgentRun<T>]) -> Map<String, Value> {
    let mut sorted = runs.iter().map(|run| run.time).collect::<Box<[_]>>();
    sorted.sort();
    let first_error = runs
        .iter()
        .filter_map(|run| run.result.as_ref().err().map(|error| (run.finished, error)))
        .min_by_key(|(finished, _)| *finished)
        .map(|(_, error)| format!("{error:#}"));
    let details = json!({
        "agents": runs.len(),
        "time_ms": times(&sorted),
        "failed": runs.iter().filter(|run| run.result.is_err()).count(),
        "first_error": first_error,
    });
    match details {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

/// Gives the record of the step of a batch with the parameters and the details. A batch with a
/// failed operation is a failed step, with the error of the operation that failed first.
fn batch_record(record: StepRecord, parameters: Value, details: Map<String, Value>) -> StepRecord {
    let status = match details.get("first_error").and_then(Value::as_str) {
        Some(error) => StepStatus::Error(error.into()),
        None => record.status.clone(),
    };
    StepRecord {
        status,
        ..record
            .with_parameters(parameters)
            .with_details(Value::Object(details), Box::default())
    }
}

/// Tells whether each operation of the batch succeeded.
fn all_ok<T>(runs: &anyhow::Result<Box<[AgentRun<T>]>>) -> bool {
    runs.as_ref()
        .is_ok_and(|runs| runs.iter().all(|run| run.result.is_ok()))
}

/// A restore phase of several agents: the repository of the first agent goes to each agent that
/// has none, the agents restore the warm save at the same time, and the hash of each restored
/// tree is compared with the hash that the save phase recorded.
///
/// The phase deletes the restored trees at its end, so the next phase has the space of the
/// volume.
pub(super) async fn concurrent_restore(
    context: &PhaseContext,
    agents: usize,
    reader_threads: Option<NonZeroUsize>,
) -> PhaseOutcome {
    let into = context.work_dir.join("restore");
    let outcome = restore_agents(context, agents, reader_threads, &into).await;
    let _ = tokio::fs::remove_dir_all(&into).await;
    outcome
}

async fn restore_agents(
    context: &PhaseContext,
    agents: usize,
    reader_threads: Option<NonZeroUsize>,
    into: &Path,
) -> PhaseOutcome {
    let storage = &context.storage;
    let facts = TreeFacts {
        name: context.selection.tree.name,
        ..TreeFacts::default()
    };
    let parameters = json!({ "agents": agents, "reader_threads": reader_threads });
    let expected = match saved_hash(context).await {
        Ok(expected) => expected,
        Err(error) => {
            return without_save(
                facts,
                &error,
                &["copy_scopes", "concurrent_restore", "hash_trees"],
            );
        }
    };

    let (record, copied) = measure(
        "copy_scopes",
        storage,
        copy_first_agent(storage.as_ref(), &context.scope().0, agents),
    )
    .await;
    let mut steps = vec![record.with_details(
        copied.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    )];
    if copied.is_err() {
        return failed(
            facts,
            steps,
            "copy_scopes",
            &["concurrent_restore", "hash_trees"],
        );
    }

    let targets = (0..agents)
        .map(|agent| into.join(agent.to_string()))
        .collect::<Box<[_]>>();
    let (record, restored) = measure("concurrent_restore", storage, async {
        targets.iter().try_for_each(std::fs::create_dir_all)?;
        let name = snapshot_name(WARM_SAVE)?;
        let name = &name;
        Ok(batch(
            targets
                .iter()
                .enumerate()
                .map(|(agent, target)| async move {
                    context
                        .agent_repository(&agent.to_string())
                        .restore(name, target, reader_threads)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no snapshot has the name {WARM_SAVE}"))
                }),
        )
        .await)
    })
    .await;
    steps.push(match &restored {
        Ok(runs) => batch_record(record, parameters, batch_details(runs)),
        Err(_) => record.with_parameters(parameters),
    });
    if !all_ok(&restored) {
        return failed(facts, steps, "concurrent_restore", &["hash_trees"]);
    }

    let (record, hashed) = measure("hash_trees", storage, hash_trees(&targets)).await;
    let Ok(hashes) = hashed else {
        steps.push(record);
        return failed(facts, steps, "hash_trees", &[]);
    };
    let matches = hashes
        .iter()
        .filter(|(hash, _)| **hash == *expected)
        .count();
    steps.push(record.with_details(
        json!({ "trees": hashes.len(), "matches": matches, "expected": expected }),
        Box::default(),
    ));
    let facts = match hashes.first() {
        Some((hash, counts)) => TreeFacts {
            files: Some(counts.files),
            directories: Some(counts.directories),
            bytes: Some(counts.bytes),
            hash: Some(hash.clone()),
            ..facts
        },
        None => facts,
    };
    PhaseOutcome {
        tree_facts: facts,
        steps,
        outcome: if matches == hashes.len() {
            Outcome::Ok
        } else {
            Outcome::Failed {
                reason: format!(
                    "{} of {} restored trees differ from the saved tree",
                    hashes.len() - matches,
                    hashes.len()
                )
                .into(),
            }
        },
    }
}

/// Gives the hash of each tree, in the order of the trees. The number of trees that are read at
/// the same time is the number of CPUs that the process can use.
async fn hash_trees(roots: &[PathBuf]) -> anyhow::Result<Box<[(Box<str>, trees::TreeCounts)]>> {
    let parallel = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    Ok(futures::stream::iter(roots)
        .map(|root| trees::hash(root))
        .buffered(parallel)
        .try_collect::<Vec<_>>()
        .await?
        .into_boxed_slice())
}

/// A save phase of several agents: a new tree and a copy of it for each other agent, the cold
/// saves of all agents at the same time, the small change of each tree, and the warm saves of all
/// agents at the same time.
///
/// Each agent of the phase has a repository of its own, which the other phases of the scenario do
/// not use, so each cold save makes a repository. The copies of the tree are reflinks where the
/// volume has them. After the copies, no page of a tree is in the page cache, so each save reads
/// its tree from the volume.
pub(super) async fn concurrent_save(context: &PhaseContext, agents: usize) -> PhaseOutcome {
    let spec = context.selection.tree;
    let storage = &context.storage;
    let roots = (0..agents)
        .map(|agent| context.work_dir.join("trees").join(agent.to_string()))
        .collect::<Box<[_]>>();
    let names = (0..agents)
        .map(|agent| format!("x{agents}-{agent}"))
        .collect::<Box<[_]>>();
    let parameters = json!({ "agents": agents });
    let facts = TreeFacts {
        name: spec.name,
        content: Some("incompressible"),
        page_cache: Some("dropped"),
        ..TreeFacts::default()
    };
    let later = [
        "copy_trees",
        "concurrent_cold_save",
        "small_change",
        "concurrent_warm_save",
    ];

    let (record, generated) = measure("generate_tree", storage, async {
        let first = roots
            .first()
            .ok_or_else(|| anyhow::anyhow!("a phase of agents needs one agent or more"))?;
        std::fs::create_dir_all(context.work_dir.join("trees"))?;
        trees::generate(spec, first).await
    })
    .await;
    let mut steps = vec![record];
    let Ok(counts) = generated else {
        return failed(facts, steps, "generate_tree", &later);
    };
    let facts = TreeFacts {
        files: Some(counts.files),
        directories: Some(counts.directories),
        bytes: Some(counts.bytes),
        ..facts
    };

    let (record, copied) = measure("copy_trees", storage, copy_trees(&roots)).await;
    steps.push(
        record.with_details(
            copied
                .as_ref()
                .map(|counts| {
                    json!({
                        "copies": agents.saturating_sub(1),
                        "files_reflinked": counts.reflinked,
                        "files_copied": counts.copied,
                    })
                })
                .unwrap_or(Value::Null),
            Box::default(),
        ),
    );
    if copied.is_err() {
        return failed(facts, steps, "copy_trees", &later[1..]);
    }

    let (record, cold) = measure(
        "concurrent_cold_save",
        storage,
        save_agents(context, &names, &roots, COLD_SAVE),
    )
    .await;
    steps.push(save_batch_record(record, parameters.clone(), &cold));
    if !all_ok(&cold) {
        return failed(facts, steps, "concurrent_cold_save", &later[2..]);
    }

    let (record, changed) = measure("small_change", storage, async {
        futures::stream::iter(roots.iter())
            .then(|root| trees::change(spec, root))
            .try_collect::<Vec<_>>()
            .await
    })
    .await;
    let change = changed
        .as_ref()
        .ok()
        .and_then(|changes| changes.first().cloned());
    steps.push(record.with_details(json!({ "trees": agents, "change": change }), Box::default()));
    if changed.is_err() {
        return failed(facts, steps, "small_change", &later[3..]);
    }
    let facts = TreeFacts { change, ..facts };

    let (record, warm) = measure(
        "concurrent_warm_save",
        storage,
        save_agents(context, &names, &roots, WARM_SAVE),
    )
    .await;
    steps.push(save_batch_record(record, parameters, &warm));
    if !all_ok(&warm) {
        return failed(facts, steps, "concurrent_warm_save", &[]);
    }
    PhaseOutcome {
        tree_facts: facts,
        steps,
        outcome: Outcome::Ok,
    }
}

/// Copies the first tree to each other root, and then removes the pages of each tree from the
/// page cache.
async fn copy_trees(roots: &[PathBuf]) -> anyhow::Result<CopyCounts> {
    let roots = roots.to_vec();
    tokio::task::spawn_blocking(move || {
        let counts = match roots.split_first() {
            Some((first, others)) => others
                .iter()
                .try_fold(CopyCounts::default(), |counts, root| {
                    trees::copy_tree(first, root, trees::Times::Drop).map(|copied| counts.with(copied))
                })?,
            None => CopyCounts::default(),
        };
        roots.iter().try_for_each(|root| trees::settle(root))?;
        Ok(counts)
    })
    .await?
}

/// Saves the tree of each agent into the repository of the agent with the snapshot name, all at
/// the same time.
async fn save_agents(
    context: &PhaseContext,
    names: &[String],
    roots: &[PathBuf],
    snapshot: &str,
) -> anyhow::Result<Box<[AgentRun<SaveReport>]>> {
    let name = snapshot_name(snapshot)?;
    let name = &name;
    Ok(
        batch(names.iter().zip(roots).map(|(agent, root)| async move {
            context.agent_repository(agent).save(name, root).await
        }))
        .await,
    )
}

/// Gives the record of a step of saves: the details of the batch, and the sums of the data that
/// the saves added.
fn save_batch_record(
    record: StepRecord,
    parameters: Value,
    runs: &anyhow::Result<Box<[AgentRun<SaveReport>]>>,
) -> StepRecord {
    match runs {
        Ok(runs) => {
            let reports = runs
                .iter()
                .filter_map(|run| run.result.as_ref().ok())
                .collect::<Box<[_]>>();
            let mut details = batch_details(runs);
            details.insert(
                "data_added".to_string(),
                json!(reports.iter().map(|report| report.data_added).sum::<u64>()),
            );
            details.insert(
                "data_added_packed".to_string(),
                json!(
                    reports
                        .iter()
                        .map(|report| report.data_added_packed)
                        .sum::<u64>()
                ),
            );
            batch_record(record, parameters, details)
        }
        Err(_) => record.with_parameters(parameters),
    }
}
