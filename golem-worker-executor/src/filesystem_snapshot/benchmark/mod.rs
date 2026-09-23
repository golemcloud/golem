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

//! A benchmark of the save, restore and forget operations of the rustic repository over the blob
//! storage of the executor.
//!
//! A scenario has trees and phases. A phase is the work of one pod, and it runs on one tree. A
//! phase measures each of its steps, and writes its result as JSON into the blob storage and on
//! the standard output. A later phase of the same scenario and tree reads the result of an
//! earlier phase from the blob storage.
//!
//! Each repository and each result is in the namespace `InitialAgentFiles` of an environment
//! that only the benchmark uses, below the object prefix of the run. The repositories of a
//! scenario, a CPU setting and a tree are the repositories of its agents (see [`agents`]).

mod agents;
pub mod cli;
mod concurrent;
mod measure;
mod report;
mod requests;
mod trees;
mod volume;

use super::rustic::{PhaseTime, Repository, RepositoryKey, STORAGE_CALL_DEADLINE};
use super::{SnapshotName, SnapshotScope};
use agents::{AgentStorage, FIRST_AGENT};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use measure::measure;
use report::{FORMAT, Outcome, PhaseResult, PhaseWall, StepRecord, TreeFacts, millis};
use requests::MeasuredBlobStorage;
use serde::Serialize;
use serde_json::{Value, json};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use trees::{
    FILES_1G, FILES_128M, FILES_TINY, OBJECTS_1G, OBJECTS_128M, SQLITE_1G, SQLITE_TINY, TreeSpec,
};
use uuid::Uuid;

/// The labels of the blob storage calls of the benchmark itself.
const TARGET_LABEL: &str = "filesystem_snapshot_benchmark";

/// The namespace of the UUIDs of the environments of the repositories.
const REPOSITORY_ENVIRONMENTS: Uuid = Uuid::from_u128(0x6f1c_5d2e_9a4b_4c3d_8e7f_0a1b_2c3d_4e5f);

/// The names of the snapshots of the base scenario.
const COLD_SAVE: &str = "cold-save";
const WARM_SAVE: &str = "warm-save";

/// A scenario: its trees and the phases that run on each tree, in order.
struct Scenario {
    name: &'static str,
    trees: &'static [TreeSpec],
    phases: &'static [Phase],
    /// The phases that run under the lower memory limit of the workflow.
    memory_limited_phases: &'static [&'static str],
}

/// A phase of a scenario: the work of one pod.
struct Phase {
    name: &'static str,
    kind: PhaseKind,
}

/// What a phase does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhaseKind {
    /// A cold save of a new tree, a small change and a warm save, into the repository of the
    /// first agent.
    Save,
    /// A cold restore of the warm save of the first agent with the number of reader threads.
    /// `None` is the default of rustic.
    Restore {
        reader_threads: Option<NonZeroUsize>,
    },
    /// The restores of the warm save into the repositories of `agents` agents at the same time.
    ConcurrentRestore {
        agents: usize,
        reader_threads: Option<NonZeroUsize>,
    },
    /// The cold saves of `agents` agents at the same time, a small change of the tree of each
    /// agent, and then the warm saves of the agents at the same time.
    ConcurrentSave { agents: usize },
}

const SAVE: Phase = Phase {
    name: "save",
    kind: PhaseKind::Save,
};

const fn restore(name: &'static str, reader_threads: usize) -> Phase {
    Phase {
        name,
        kind: PhaseKind::Restore {
            reader_threads: NonZeroUsize::new(reader_threads),
        },
    }
}

const fn concurrent_restore(name: &'static str, agents: usize, reader_threads: usize) -> Phase {
    Phase {
        name,
        kind: PhaseKind::ConcurrentRestore {
            agents,
            reader_threads: NonZeroUsize::new(reader_threads),
        },
    }
}

const fn concurrent_save(name: &'static str, agents: usize) -> Phase {
    Phase {
        name,
        kind: PhaseKind::ConcurrentSave { agents },
    }
}

const BASE_PHASES: &[Phase] = &[
    SAVE,
    Phase {
        name: "restore",
        kind: PhaseKind::Restore {
            reader_threads: None,
        },
    },
];

const RESTORE_THREADS_PHASES: &[Phase] = &[
    SAVE,
    restore("restore-1", 1),
    restore("restore-2", 2),
    restore("restore-4", 4),
    restore("restore-8", 8),
    restore("restore-20", 20),
];

const CONCURRENT_RESTORE_1_PHASES: &[Phase] = &[
    SAVE,
    concurrent_restore("restore-x1", 1, 1),
    concurrent_restore("restore-x5", 5, 1),
    concurrent_restore("restore-x10", 10, 1),
    concurrent_restore("restore-x25", 25, 1),
    concurrent_restore("restore-x50", 50, 1),
    concurrent_restore("restore-x100", 100, 1),
    concurrent_restore("restore-x200", 200, 1),
];

const CONCURRENT_RESTORE_4_PHASES: &[Phase] = &[
    SAVE,
    concurrent_restore("restore-x1", 1, 4),
    concurrent_restore("restore-x5", 5, 4),
    concurrent_restore("restore-x10", 10, 4),
    concurrent_restore("restore-x25", 25, 4),
    concurrent_restore("restore-x50", 50, 4),
    concurrent_restore("restore-x100", 100, 4),
    concurrent_restore("restore-x200", 200, 4),
];

const CONCURRENT_RESTORE_20_PHASES: &[Phase] = &[
    SAVE,
    concurrent_restore("restore-x1", 1, 20),
    concurrent_restore("restore-x5", 5, 20),
    concurrent_restore("restore-x10", 10, 20),
    concurrent_restore("restore-x25", 25, 20),
    concurrent_restore("restore-x50", 50, 20),
    concurrent_restore("restore-x100", 100, 20),
    concurrent_restore("restore-x200", 200, 20),
];

const CONCURRENT_SAVE_PHASES: &[Phase] = &[
    concurrent_save("save-x1", 1),
    concurrent_save("save-x2", 2),
    concurrent_save("save-x4", 4),
    concurrent_save("save-x8", 8),
];

/// The scenarios of the benchmark.
const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "base",
        trees: &[FILES_128M, FILES_1G, SQLITE_1G, OBJECTS_128M, OBJECTS_1G],
        phases: BASE_PHASES,
        memory_limited_phases: &[],
    },
    Scenario {
        name: "smoke",
        trees: &[FILES_TINY, SQLITE_TINY],
        phases: BASE_PHASES,
        memory_limited_phases: &[],
    },
    Scenario {
        name: "restore-threads",
        trees: &[FILES_1G],
        phases: RESTORE_THREADS_PHASES,
        memory_limited_phases: &[],
    },
    Scenario {
        name: "memory-pressure",
        trees: &[FILES_1G],
        phases: BASE_PHASES,
        memory_limited_phases: &["restore"],
    },
    Scenario {
        name: "concurrent-restore-1",
        trees: &[FILES_128M, FILES_1G],
        phases: CONCURRENT_RESTORE_1_PHASES,
        memory_limited_phases: &[],
    },
    Scenario {
        name: "concurrent-restore-4",
        trees: &[FILES_128M, FILES_1G],
        phases: CONCURRENT_RESTORE_4_PHASES,
        memory_limited_phases: &[],
    },
    Scenario {
        name: "concurrent-restore-20",
        trees: &[FILES_128M, FILES_1G],
        phases: CONCURRENT_RESTORE_20_PHASES,
        memory_limited_phases: &[],
    },
    Scenario {
        name: "concurrent-save",
        trees: &[FILES_128M, FILES_1G, SQLITE_1G],
        phases: CONCURRENT_SAVE_PHASES,
        memory_limited_phases: &[],
    },
];

/// One line of a plan: the phases of one tree of a scenario, in order.
///
/// `memory_limited_phases` names the phases that run under the lower memory limit of the
/// workflow. A line without such phases does not have the field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PlanEntry {
    scenario: &'static str,
    tree: &'static str,
    phases: Box<[&'static str]>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    memory_limited_phases: Box<[&'static str]>,
}

/// Gives the plan of the scenarios with the names, or the first name that no scenario has.
fn plan(names: &[String]) -> Result<Box<[PlanEntry]>, String> {
    names
        .iter()
        .try_fold(Vec::new(), |mut entries, name| {
            let scenario = scenario(name).ok_or_else(|| name.clone())?;
            entries.extend(scenario.trees.iter().map(|tree| PlanEntry {
                scenario: scenario.name,
                tree: tree.name,
                phases: scenario.phases.iter().map(|phase| phase.name).collect(),
                memory_limited_phases: scenario.memory_limited_phases.into(),
            }));
            Ok(entries)
        })
        .map(Vec::into_boxed_slice)
}

fn scenario(name: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|scenario| scenario.name == name)
}

/// The phase of one pod, found by its names.
#[derive(Clone, Copy)]
struct Selection {
    scenario: &'static Scenario,
    tree: &'static TreeSpec,
    phase: &'static Phase,
}

impl Selection {
    /// Gives the phase with the names, or an error that says which name is unknown.
    fn find(scenario: &str, tree: &str, phase: &str) -> Result<Self, String> {
        let found = self::scenario(scenario)
            .ok_or_else(|| format!("no scenario has the name {scenario:?}"))?;
        Ok(Self {
            scenario: found,
            tree: found
                .trees
                .iter()
                .find(|spec| spec.name == tree)
                .ok_or_else(|| format!("the scenario {scenario:?} has no tree {tree:?}"))?,
            phase: found
                .phases
                .iter()
                .find(|spec| spec.name == phase)
                .ok_or_else(|| format!("the scenario {scenario:?} has no phase {phase:?}"))?,
        })
    }
}

/// Tells whether the text can be a run id or a CPU setting: 1 to 64 ASCII letters, digits, `-`
/// or `_`. Each is a segment of an object key.
fn is_key_segment(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Gives the repository key of the run. The data of a run is synthetic and the workflow deletes
/// it after the run, so each pod of the run derives the same key from the run id.
fn repository_key(run_id: &str) -> RepositoryKey {
    let mut bytes = [0; 64];
    blake3::Hasher::new_derive_key("golem fs-snapshot benchmark repository key")
        .update(run_id.as_bytes())
        .finalize_xof()
        .fill(&mut bytes);
    RepositoryKey::new(bytes)
}

/// Gives the namespace of the results of a run.
fn results_namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::InitialAgentFiles {
        environment_id: EnvironmentId(Uuid::nil()),
    }
}

/// Gives the path of the result of a phase in the namespace of the results.
fn result_path(scenario: &str, cpu_setting: &str, tree: &str, phase: &str) -> Box<Path> {
    PathBuf::from(format!(
        "results/{scenario}/{cpu_setting}/{tree}/{phase}.json"
    ))
    .into_boxed_path()
}

/// Gives the scope of the repository of a scenario, a CPU setting and a tree.
fn repository_scope(scenario: &str, cpu_setting: &str, tree: &str) -> SnapshotScope {
    SnapshotScope(BlobStorageNamespace::InitialAgentFiles {
        environment_id: EnvironmentId(Uuid::new_v5(
            &REPOSITORY_ENVIRONMENTS,
            format!("{scenario}/{cpu_setting}/{tree}").as_bytes(),
        )),
    })
}

/// What a phase gets.
struct PhaseContext {
    run_id: Box<str>,
    cpu_setting: Box<str>,
    selection: Selection,
    work_dir: Box<Path>,
    storage: Arc<MeasuredBlobStorage>,
}

impl PhaseContext {
    /// Gives the scope of the repositories of the agents of the phase.
    fn scope(&self) -> SnapshotScope {
        repository_scope(
            self.selection.scenario.name,
            &self.cpu_setting,
            self.selection.tree.name,
        )
    }

    /// Gives the repository of the agent.
    fn agent_repository(&self, agent: &str) -> Repository {
        Repository::new(
            Arc::new(AgentStorage::new(self.storage.clone(), agent)),
            self.scope(),
            repository_key(&self.run_id),
            STORAGE_CALL_DEADLINE,
        )
    }

    /// Gives the repository of the first agent.
    fn repository(&self) -> Repository {
        self.agent_repository(FIRST_AGENT)
    }

    fn result_path(&self, phase: &str) -> Box<Path> {
        result_path(
            self.selection.scenario.name,
            &self.cpu_setting,
            self.selection.tree.name,
            phase,
        )
    }
}

/// What a phase gives.
struct PhaseOutcome {
    tree_facts: TreeFacts,
    steps: Vec<StepRecord>,
    outcome: Outcome,
}

/// Runs one phase on the storage, writes its result into the storage, and gives the result and
/// whether the write succeeded.
///
/// `environment` is recorded as it is, with the time of a first request of the storage, which
/// gets the credentials and a connection as a running executor already has them. The counters of
/// the cgroup `memory.events` at the start and at the end of the phase go into its `cgroup`
/// object.
async fn run_phase(
    run_id: &str,
    cpu_setting: &str,
    selection: Selection,
    work_dir: &Path,
    storage: Arc<dyn BlobStorage>,
    environment: Value,
) -> (PhaseResult, anyhow::Result<()>) {
    let environment =
        with_cgroup_value(environment, "memory_events_start", measure::memory_events());
    let context = PhaseContext {
        run_id: run_id.into(),
        cpu_setting: cpu_setting.into(),
        selection,
        work_dir: work_dir.into(),
        storage: Arc::new(MeasuredBlobStorage::new(storage)),
    };
    let volume = volume::check(work_dir);
    let started = Instant::now();
    let warm_up = context
        .storage
        .get_metadata(
            TARGET_LABEL,
            "warm_up",
            results_namespace(),
            Path::new("warm-up"),
        )
        .await;
    let environment = with_warm_up(environment, millis(started.elapsed()), warm_up.err());
    let outcome = run_kind(&context, selection.phase.kind).await;
    let environment = with_cgroup_value(environment, "memory_events_end", measure::memory_events());
    let result = PhaseResult {
        format: FORMAT,
        run_id: run_id.into(),
        scenario: selection.scenario.name,
        phase: selection.phase.name,
        tree: selection.tree.name,
        cpu_setting: cpu_setting.into(),
        environment,
        volume,
        tree_facts: outcome.tree_facts,
        steps: outcome.steps.into_boxed_slice(),
        outcome: outcome.outcome,
    };
    let written = write_result(&context, &result).await;
    (result, written)
}

/// Runs the work of the kind of phase.
async fn run_kind(context: &PhaseContext, kind: PhaseKind) -> PhaseOutcome {
    match kind {
        PhaseKind::Save => base_save(context).await,
        PhaseKind::Restore { reader_threads } => base_restore(context, reader_threads).await,
        PhaseKind::ConcurrentRestore {
            agents,
            reader_threads,
        } => concurrent::concurrent_restore(context, agents, reader_threads).await,
        PhaseKind::ConcurrentSave { agents } => concurrent::concurrent_save(context, agents).await,
    }
}

/// Gives the environment with the value at the key in its `cgroup` object. An environment that
/// is an object without a `cgroup` object gets one.
fn with_cgroup_value(environment: Value, key: &str, value: Value) -> Value {
    match environment {
        Value::Object(mut fields) => {
            let cgroup = fields
                .entry("cgroup")
                .or_insert_with(|| Value::Object(Default::default()));
            if let Value::Object(cgroup) = cgroup {
                cgroup.insert(key.to_string(), value);
            }
            Value::Object(fields)
        }
        other => other,
    }
}

fn with_warm_up(environment: Value, warm_up_ms: f64, error: Option<anyhow::Error>) -> Value {
    match environment {
        Value::Object(mut fields) => {
            fields.insert("warm_up_ms".to_string(), json!(warm_up_ms));
            fields.insert(
                "warm_up_error".to_string(),
                json!(error.map(|error| format!("{error:#}"))),
            );
            Value::Object(fields)
        }
        other => other,
    }
}

async fn write_result(context: &PhaseContext, result: &PhaseResult) -> anyhow::Result<()> {
    let json = serde_json::to_vec(result)?;
    context
        .storage
        .put_raw(
            TARGET_LABEL,
            "result",
            results_namespace(),
            &context.result_path(context.selection.phase.name),
            &json,
        )
        .await
}

/// Gives the times of the parts of an operation as the phases of a step.
fn phase_walls(phases: &[PhaseTime]) -> Box<[PhaseWall]> {
    phases
        .iter()
        .map(|time| PhaseWall {
            name: phase_name(time.phase),
            wall_ms: millis(time.wall),
        })
        .collect()
}

fn phase_name(phase: super::rustic::OperationPhase) -> &'static str {
    use super::rustic::OperationPhase;
    match phase {
        OperationPhase::Create => "create",
        OperationPhase::Open => "open",
        OperationPhase::Lookup => "lookup",
        OperationPhase::IndexLoad => "index_load",
        OperationPhase::Backup => "backup",
        OperationPhase::RestorePlan => "restore_plan",
        OperationPhase::Restore => "restore",
        OperationPhase::PrunePlan => "prune_plan",
        OperationPhase::Prune => "prune",
    }
}

fn snapshot_name(text: &str) -> anyhow::Result<SnapshotName> {
    Ok(SnapshotName::new(text)?)
}

/// Gives the outcome of a phase whose step failed: the records so far, and a skipped record for
/// each step that did not run.
fn failed(
    tree_facts: TreeFacts,
    steps: Vec<StepRecord>,
    failed_step: &'static str,
    skipped: &[&'static str],
) -> PhaseOutcome {
    PhaseOutcome {
        tree_facts,
        steps: steps
            .into_iter()
            .chain(skipped.iter().copied().map(StepRecord::skipped))
            .collect(),
        outcome: Outcome::Failed {
            reason: format!("the step {failed_step} failed").into(),
        },
    }
}

/// The save phase of the base scenario: a cold save of a new tree, a small change of the tree, a
/// warm save, and the hash of the changed tree.
///
/// `generate_tree` and `small_change` remove the pages of the tree from the page cache. No step
/// reads the content of a file between one of them and the save after it, so each save reads the
/// tree from the volume. The hash of the new tree is read after the cold save, and the hash of the
/// changed tree after the warm save. A save does not change the tree.
async fn base_save(context: &PhaseContext) -> PhaseOutcome {
    let spec = context.selection.tree;
    let tree = context.work_dir.join("tree");
    let repository = context.repository();
    let facts = TreeFacts {
        name: spec.name,
        content: Some("incompressible"),
        page_cache: Some("dropped"),
        ..TreeFacts::default()
    };
    let storage = &context.storage;

    let (record, generated) = measure("generate_tree", storage, trees::generate(spec, &tree)).await;
    let mut steps = vec![record];
    let Ok(counts) = generated else {
        return failed(
            facts,
            steps,
            "generate_tree",
            &["cold_save", "small_change", "warm_save", "hash_tree"],
        );
    };
    let facts = TreeFacts {
        files: Some(counts.files),
        directories: Some(counts.directories),
        bytes: Some(counts.bytes),
        ..facts
    };

    let (record, cold) = measure("cold_save", storage, async {
        repository.save(&snapshot_name(COLD_SAVE)?, &tree).await
    })
    .await;
    steps.push(save_record(record, &cold));
    if cold.is_err() {
        return failed(
            facts,
            steps,
            "cold_save",
            &["small_change", "warm_save", "hash_tree"],
        );
    }
    let facts = TreeFacts {
        hash: trees::hash(&tree).await.ok().map(|(hash, _)| hash),
        ..facts
    };

    let (record, changed) = measure("small_change", storage, trees::change(spec, &tree)).await;
    steps.push(record.with_details(
        changed.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    ));
    let Ok(change) = changed else {
        return failed(facts, steps, "small_change", &["warm_save", "hash_tree"]);
    };
    let facts = TreeFacts {
        change: Some(change),
        ..facts
    };

    let (record, warm) = measure("warm_save", storage, async {
        repository.save(&snapshot_name(WARM_SAVE)?, &tree).await
    })
    .await;
    steps.push(save_record(record, &warm));
    if warm.is_err() {
        return failed(facts, steps, "warm_save", &["hash_tree"]);
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

fn save_record(record: StepRecord, save: &anyhow::Result<super::rustic::SaveReport>) -> StepRecord {
    match save {
        Ok(report) => record.with_details(
            json!({
                "snapshot": report.snapshot,
                "parent": report.parent,
                "files_new": report.files_new,
                "files_changed": report.files_changed,
                "files_unmodified": report.files_unmodified,
                "dirs_new": report.dirs_new,
                "dirs_changed": report.dirs_changed,
                "dirs_unmodified": report.dirs_unmodified,
                "bytes_processed": report.bytes_processed,
                "data_added": report.data_added,
                "data_added_packed": report.data_added_packed,
                "data_blobs": report.data_blobs,
                "tree_blobs": report.tree_blobs,
            }),
            phase_walls(&report.phases),
        ),
        Err(_) => record,
    }
}

/// The restore phase of the base scenario: a cold restore of the warm save into an empty
/// directory, and a comparison of the hash of the restored tree with the hash that the save
/// phase recorded.
///
/// `reader_threads` is the number of threads of the restore that read data, and the
/// `parameters` of the restore step record it. `None` is the default of rustic.
async fn base_restore(
    context: &PhaseContext,
    reader_threads: Option<NonZeroUsize>,
) -> PhaseOutcome {
    let spec = context.selection.tree;
    let into = context.work_dir.join("restore");
    let repository = context.repository();
    let storage = &context.storage;
    let facts = TreeFacts {
        name: spec.name,
        ..TreeFacts::default()
    };
    let expected = match saved_hash(context).await {
        Ok(expected) => expected,
        Err(error) => return without_save(facts, &error, &["cold_restore", "hash_tree"]),
    };

    let (record, restored) = measure("cold_restore", storage, async {
        std::fs::create_dir(&into)?;
        repository
            .restore(&snapshot_name(WARM_SAVE)?, &into, reader_threads)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no snapshot has the name {WARM_SAVE}"))
    })
    .await;
    let record = record.with_parameters(json!({ "reader_threads": reader_threads }));
    let record = match &restored {
        Ok(report) => record.with_details(
            json!({ "files": report.files, "dirs": report.dirs, "bytes": report.bytes }),
            phase_walls(&report.phases),
        ),
        Err(_) => record,
    };
    let mut steps = vec![record];
    if restored.is_err() {
        return failed(facts, steps, "cold_restore", &["hash_tree"]);
    }

    let (record, hashed) = measure("hash_tree", storage, trees::hash(&into)).await;
    let Ok((hash, counts)) = hashed else {
        steps.push(record);
        return failed(facts, steps, "hash_tree", &[]);
    };
    let matches = *hash == *expected;
    steps.push(record.with_details(
        json!({ "hash": hash, "expected": expected, "matches": matches }),
        Box::default(),
    ));
    let facts = TreeFacts {
        files: Some(counts.files),
        directories: Some(counts.directories),
        bytes: Some(counts.bytes),
        hash: Some(hash),
        ..facts
    };
    PhaseOutcome {
        tree_facts: facts,
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

/// Gives the outcome of a restore phase without a result of the save phase: a skipped record for
/// each step.
fn without_save(
    tree_facts: TreeFacts,
    error: &anyhow::Error,
    skipped: &[&'static str],
) -> PhaseOutcome {
    PhaseOutcome {
        tree_facts,
        steps: skipped.iter().copied().map(StepRecord::skipped).collect(),
        outcome: Outcome::Failed {
            reason: format!("the save phase gave no result to compare with: {error:#}").into(),
        },
    }
}

/// Reads the hash after the change from the result of the save phase.
async fn saved_hash(context: &PhaseContext) -> anyhow::Result<Box<str>> {
    let saved = context
        .storage
        .get_raw(
            TARGET_LABEL,
            "result",
            results_namespace(),
            &context.result_path("save"),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("the save phase wrote no result"))?;
    let saved: Value = serde_json::from_slice(&saved)?;
    if saved.pointer("/outcome/status") != Some(&json!("ok")) {
        anyhow::bail!("the save phase failed");
    }
    saved
        .pointer("/tree_facts/hash_after_change")
        .and_then(Value::as_str)
        .map(Box::from)
        .ok_or_else(|| anyhow::anyhow!("the result of the save phase has no hash"))
}

#[cfg(test)]
mod tests;
