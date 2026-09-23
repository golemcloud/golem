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

//! The phases of the prune and repository open scenarios.
//!
//! The history phase makes a repository with a history of saves in the repository of the first
//! agent. Each prune phase copies that repository on the server into the repository of its own
//! agent, and prunes the copy. So each prune starts from the same repository.

use super::agents::{FIRST_AGENT, agent_blobs, copy_agent, total_bytes};
use super::measure::measure;
use super::report::{Outcome, StepRecord, TreeFacts};
use super::{
    COLD_SAVE, PhaseContext, PhaseOutcome, failed, inspect_record, phase_walls, save_record,
    saved_hash, snapshot_name, trees, without_save,
};
use crate::filesystem_snapshot::rustic::{PruneReport, PruneSettings, RepackLimits, Repository};
use futures::{StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::time::Duration;

/// The number of saves after the cold save: the warm save and 10 more.
const ROUNDS: u8 = 11;

/// The saves after which the history phase opens the repository.
const OPEN_AFTER: [u8; 3] = [1, 11, 12];

/// The number of newest snapshots that the forget keeps.
const KEEP: u8 = 2;

/// The name of the snapshot of the save of the round, from 1 to [`ROUNDS`].
fn snapshot_of_round(round: u8) -> Box<str> {
    format!("round-{round:02}").into()
}

/// The name of the newest snapshot of the history.
fn newest() -> Box<str> {
    snapshot_of_round(ROUNDS)
}

/// The settings of both prunes of a prune phase: no grace period, and no limit that leaves unused
/// data or stops a repack.
const fn prune_settings(fast_repack: bool) -> PruneSettings {
    PruneSettings {
        fast_repack,
        keep_delete: Duration::ZERO,
        repack: RepackLimits::Unlimited,
    }
}

/// The history phase: a cold save of a new tree, then a small change and a save in each of
/// [`ROUNDS`] rounds, each round with other content, a forget of every snapshot but the
/// [`KEEP`] newest, and the hash of the tree.
///
/// The phase opens the repository after the saves in [`OPEN_AFTER`]. Each open finds the newest
/// snapshot by its name and loads the index, as a restore does.
pub(super) async fn history(context: &PhaseContext) -> PhaseOutcome {
    let spec = context.selection.tree;
    let tree = context.work_dir.join("tree");
    let repository = context.repository();
    let storage = &context.storage;
    let settings = context.save_settings();
    let facts = TreeFacts {
        name: spec.name,
        content: Some(spec.content.label()),
        page_cache: Some("dropped"),
        ..TreeFacts::default()
    };

    let (record, generated) = measure("generate_tree", storage, trees::generate(spec, &tree)).await;
    let mut steps = vec![record];
    let Ok(counts) = generated else {
        return failed(facts, steps, "generate_tree", &["cold_save"]);
    };
    let facts = TreeFacts {
        files: Some(counts.files),
        directories: Some(counts.directories),
        bytes: Some(counts.bytes),
        ..facts
    };

    let (record, cold) = measure("cold_save", storage, async {
        repository
            .save_with(&snapshot_name(COLD_SAVE)?, &tree, settings)
            .await
    })
    .await;
    steps.push(save_record(record, &cold));
    if cold.is_err() {
        return failed(facts, steps, "cold_save", &[]);
    }
    let steps = open_step(context, &repository, COLD_SAVE, 1, steps).await;

    let (repository_ref, tree_ref) = (&repository, tree.as_path());
    let saved = futures::stream::iter(1..=ROUNDS)
        .map(Ok::<_, Vec<StepRecord>>)
        .try_fold(steps, |steps, round| async move {
            save_round(context, repository_ref, tree_ref, round, steps).await
        })
        .await;
    let steps = match saved {
        Ok(steps) => steps,
        Err(steps) => {
            let step = steps.last().map_or("round", |step| step.name);
            return failed(facts, steps, step, &[]);
        }
    };

    let forgotten = (0..=ROUNDS - KEEP)
        .map(|round| match round {
            0 => Box::from(COLD_SAVE),
            round => snapshot_of_round(round),
        })
        .collect::<Box<[_]>>();
    let (record, forgot) = measure("forget", storage, async {
        futures::stream::iter(forgotten.iter())
            .then(|name| async { repository.forget(&snapshot_name(name)?).await })
            .try_fold(0_u64, |total, count| async move { Ok(total + count) })
            .await
    })
    .await;
    let mut steps = steps;
    steps.push(record.with_details(
        json!({ "snapshots_forgotten": forgot.as_ref().ok(), "snapshots_kept": KEEP }),
        Box::default(),
    ));
    if forgot.is_err() {
        return failed(facts, steps, "forget", &["hash_tree"]);
    }

    let (record, hashed) = measure("hash_tree", storage, trees::hash(&tree)).await;
    steps.push(record);
    let Ok((hash, _)) = hashed else {
        return failed(facts, steps, "hash_tree", &[]);
    };
    PhaseOutcome {
        tree_facts: TreeFacts {
            hash_after_change: Some(hash),
            ..facts
        },
        steps,
        outcome: Outcome::Ok,
    }
}

/// Changes the tree for the round and saves it, and opens the repository after the save when
/// [`OPEN_AFTER`] names it. A failed step gives the records so far as the error.
async fn save_round(
    context: &PhaseContext,
    repository: &Repository,
    tree: &std::path::Path,
    round: u8,
    mut steps: Vec<StepRecord>,
) -> Result<Vec<StepRecord>, Vec<StepRecord>> {
    let storage = &context.storage;
    let spec = context.selection.tree;
    let parameters = json!({ "round": round });
    let (record, changed) = measure(
        "small_change",
        storage,
        trees::change_round(spec, tree, round),
    )
    .await;
    steps.push(record.with_parameters(parameters.clone()).with_details(
        changed.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    ));
    if changed.is_err() {
        return Err(steps);
    }
    let name = snapshot_of_round(round);
    let (record, saved) = measure("save", storage, async {
        repository
            .save_with(&snapshot_name(&name)?, tree, context.save_settings())
            .await
    })
    .await;
    steps.push(save_record(record, &saved).with_parameters(parameters));
    if saved.is_err() {
        return Err(steps);
    }
    Ok(open_step(context, repository, &name, round + 1, steps).await)
}

/// Opens the repository and finds the snapshot with the name, when the number of saves is in
/// [`OPEN_AFTER`], and gives the steps with the record of the open. A failed open is recorded and
/// does not stop the phase.
async fn open_step(
    context: &PhaseContext,
    repository: &Repository,
    name: &str,
    saves: u8,
    mut steps: Vec<StepRecord>,
) -> Vec<StepRecord> {
    if OPEN_AFTER.contains(&saves) {
        let (record, inspected) = measure("open", &context.storage, async {
            repository.inspect(&snapshot_name(name)?).await
        })
        .await;
        steps.push(inspect_record(record, &inspected).with_parameters(json!({ "saves": saves })));
    }
    steps
}

/// A prune phase: the repository of the history phase goes on the server to the agent of the
/// variant of the phase, two prunes with no grace period run on it, the first to mark the packs
/// that no snapshot uses and the second to delete them, then an open, and a restore of the newest
/// snapshot whose hash is compared with the hash of the history phase.
///
/// The details of the second prune give the listed bytes of the repository before the first prune
/// and after the second, and the difference, which is what the prunes gave back. The restore time
/// is the time until the files are readable, not until they are durable.
pub(super) async fn prune(context: &PhaseContext, fast_repack: bool) -> PhaseOutcome {
    let into = context.work_dir.join("restore");
    let storage = &context.storage;
    let namespace = context.scope().0;
    let agent = context.variant().agent;
    let repository = context.repository();
    let settings = prune_settings(fast_repack);
    let parameters = json!({
        "fast_repack": fast_repack,
        "keep_delete_s": settings.keep_delete.as_secs(),
        "max_unused": "0%",
        "max_repack": "unlimited",
    });
    let facts = TreeFacts {
        name: context.selection.tree.name,
        ..TreeFacts::default()
    };
    let later = [
        "copy_scopes",
        "prune_mark",
        "prune_delete",
        "open",
        "cold_restore",
        "hash_tree",
    ];
    let expected = match saved_hash(context).await {
        Ok(expected) => expected,
        Err(error) => return without_save(facts, &error, &later),
    };

    let (record, copied) = measure(
        "copy_scopes",
        storage,
        copy_agent(storage.as_ref(), &namespace, FIRST_AGENT, agent),
    )
    .await;
    let mut steps = vec![record.with_details(
        copied.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    )];
    if copied.is_err() {
        return failed(facts, steps, "copy_scopes", &later[1..]);
    }
    let before = agent_blobs(storage.as_ref(), &namespace, agent).await;

    let (record, marked) = measure("prune_mark", storage, repository.prune(settings)).await;
    steps.push(prune_record(record, &marked, parameters.clone()));
    if marked.is_err() {
        return failed(facts, steps, "prune_mark", &later[2..]);
    }
    let (record, deleted) = measure("prune_delete", storage, repository.prune(settings)).await;
    let after = agent_blobs(storage.as_ref(), &namespace, agent).await;
    let record = prune_record(record, &deleted, parameters);
    let given_back = match (&before, &after) {
        (Ok(before), Ok(after)) => json!({
            "bytes_before": total_bytes(before),
            "bytes_after": total_bytes(after),
            "bytes_given_back": total_bytes(before).saturating_sub(total_bytes(after)),
            "blobs_before": before.len(),
            "blobs_after": after.len(),
        }),
        _ => Value::Null,
    };
    steps.push(with_detail(record, "repository", given_back));
    if deleted.is_err() {
        return failed(facts, steps, "prune_delete", &later[3..]);
    }

    let newest = newest();
    let (record, inspected) = measure("open", storage, async {
        repository.inspect(&snapshot_name(&newest)?).await
    })
    .await;
    steps.push(inspect_record(record, &inspected).with_parameters(json!({ "saves": "pruned" })));

    let (record, restored) = measure("cold_restore", storage, async {
        std::fs::create_dir(&into)?;
        repository
            .restore(&snapshot_name(&newest)?, &into, None)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no snapshot has the name {newest}"))
    })
    .await;
    steps.push(match &restored {
        Ok(report) => record.with_details(
            json!({ "files": report.files, "dirs": report.dirs, "bytes": report.bytes }),
            phase_walls(&report.phases),
        ),
        Err(_) => record,
    });
    if restored.is_err() {
        return failed(facts, steps, "cold_restore", &later[5..]);
    }
    let (record, hashed) = measure("hash_tree", storage, trees::hash(&into)).await;
    let _ = tokio::fs::remove_dir_all(&into).await;
    let Ok((hash, counts)) = hashed else {
        steps.push(record);
        return failed(facts, steps, "hash_tree", &[]);
    };
    let matches = *hash == *expected;
    steps.push(record.with_details(
        json!({ "hash": hash, "expected": expected, "matches": matches }),
        Box::default(),
    ));
    PhaseOutcome {
        tree_facts: TreeFacts {
            files: Some(counts.files),
            directories: Some(counts.directories),
            bytes: Some(counts.bytes),
            hash: Some(hash),
            ..facts
        },
        steps,
        outcome: if matches {
            Outcome::Ok
        } else {
            Outcome::Failed {
                reason: "the restored tree differs from the saved tree".into(),
            }
        },
    }
}

/// Gives the record of a prune with the plan of the prune.
fn prune_record(
    record: StepRecord,
    pruned: &anyhow::Result<Option<PruneReport>>,
    parameters: Value,
) -> StepRecord {
    let record = record.with_parameters(parameters);
    match pruned {
        Ok(Some(report)) => record.with_details(prune_details(report), phase_walls(&report.phases)),
        Ok(None) => record.with_details(json!({ "repository": null }), Box::default()),
        Err(_) => record,
    }
}

fn prune_details(report: &PruneReport) -> Value {
    json!({
        "packs_used": report.packs_used,
        "packs_partly_used": report.packs_partly_used,
        "packs_unused": report.packs_unused,
        "packs_repacked": report.packs_repacked,
        "packs_kept": report.packs_kept,
        "marked_packs_deleted": report.marked_packs_deleted,
        "marked_bytes_deleted": report.marked_bytes_deleted,
        "marked_packs_kept": report.marked_packs_kept,
        "bytes_used": report.bytes_used,
        "bytes_unused": report.bytes_unused,
        "bytes_removed": report.bytes_removed,
        "bytes_repacked": report.bytes_repacked,
        "bytes_repack_removed": report.bytes_repack_removed,
        "index_files": report.index_files,
        "index_files_rebuilt": report.index_files_rebuilt,
    })
}

/// Gives the record with the value at the key in its details.
fn with_detail(record: StepRecord, key: &str, value: Value) -> StepRecord {
    let details = match record.details.clone() {
        Value::Object(mut details) => {
            details.insert(key.to_string(), value);
            Value::Object(details)
        }
        other => other,
    };
    let phases = record.phases.clone();
    record.with_details(details, phases)
}
