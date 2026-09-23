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

use super::agents::AgentStorage;
use super::report::{Outcome, PhaseResult, StepRecord, StepStatus};
use super::requests::MeasuredBlobStorage;
use super::trees::{Content, FILES_TINY, SQLITE_TINY, TreeShape, TreeSpec, tree_hash};
use super::{
    BASE, CPU_DEFAULT, CPU_OPTIONS_PHASES, CPU_ZSTD_OFF, Compression, DEFAULTS, HISTORY, PRUNE,
    PRUNE_FAST_REPACK, Phase, PhaseContext, PhaseKind, PlanEntry, RESTORE_THREADS_PHASES,
    Repository, RepositorySettings, SAVE, SAVE_THREADS_2, SQLITE_FIXED_64K, SQLITE_RABIN,
    STORAGE_CALL_DEADLINE, SaveSettings, Scenario, Selection, WARM_SAVE, concurrent_restore,
    concurrent_save, is_key_segment, mixed_phase, plan, prune_phase, repository_key,
    repository_scope, result_path, run_phase, save_phase, save_threads_phase, snapshot_name,
    with_defaults, with_settings,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::num::{NonZeroI32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;

/// The phases of the scenario `restore-threads` on a tiny tree.
static RESTORE_THREADS_TINY: Scenario = Scenario {
    name: "restore-threads-tiny",
    trees: &[FILES_TINY],
    phases: RESTORE_THREADS_PHASES,
    memory_limited_phases: &[],
};

/// The phases of the concurrent scenarios with 3 agents, on a tiny tree.
static CONCURRENT_TINY: Scenario = Scenario {
    name: "concurrent-tiny",
    trees: &[FILES_TINY],
    phases: &[
        SAVE,
        concurrent_restore("restore-x3", 3, 2),
        concurrent_save("save-x3", 3),
    ],
    memory_limited_phases: &[],
};

/// Runs the phase of the scenario on its first tree, in a new work directory, and gives the
/// result and the work directory.
async fn run_tiny(
    scenario: &'static Scenario,
    phase: &str,
    storage: &Arc<InMemoryBlobStorage>,
) -> (PhaseResult, tempfile::TempDir) {
    run_tiny_with(scenario, phase, storage.clone(), |_| {}).await
}

/// Runs the phase of the scenario on its first tree over the storage, in a new work directory
/// that `prepare` gets before the phase runs, and gives the result and the work directory.
async fn run_tiny_with(
    scenario: &'static Scenario,
    phase: &str,
    storage: Arc<dyn BlobStorage>,
    prepare: impl FnOnce(&Path),
) -> (PhaseResult, tempfile::TempDir) {
    let work = tempfile::tempdir().unwrap();
    prepare(work.path());
    let selection = Selection {
        scenario,
        tree: &scenario.trees[0],
        phase: scenario
            .phases
            .iter()
            .find(|candidate| candidate.name == phase)
            .unwrap(),
    };
    let (result, written) = run_phase(
        "run-1",
        "no-limit",
        selection,
        work.path(),
        storage,
        json!({}),
    )
    .await;
    written.unwrap();
    (result, work)
}

/// Gives the files, the directories and the bytes of the tree facts of the result.
fn tree_counts(result: &PhaseResult) -> (Option<u64>, Option<u64>, Option<u64>) {
    (
        result.tree_facts.files,
        result.tree_facts.directories,
        result.tree_facts.bytes,
    )
}

/// The files, the directories and the bytes of a new `FILES_TINY` tree.
const FILES_TINY_COUNTS: (Option<u64>, Option<u64>, Option<u64>) =
    (Some(100), Some(10), Some(1024 * 1024));

/// Gives the name of each step of the result that did not run.
fn skipped_steps(result: &PhaseResult) -> Vec<&'static str> {
    result
        .steps
        .iter()
        .filter(|step| step.status == StepStatus::Skipped)
        .map(|step| step.name)
        .collect()
}

/// Gives the name of each step of the result and whether it succeeded.
fn step_states(result: &PhaseResult) -> Vec<(&'static str, bool)> {
    result
        .steps
        .iter()
        .map(|step| (step.name, step.status == StepStatus::Ok))
        .collect()
}

fn step<'a>(result: &'a PhaseResult, name: &str) -> &'a StepRecord {
    result.steps.iter().find(|step| step.name == name).unwrap()
}

/// Gives the namespace of the repositories of the agents of a phase of `CONCURRENT_TINY`.
fn concurrent_tiny_namespace() -> BlobStorageNamespace {
    repository_scope(CONCURRENT_TINY.name, "no-limit", FILES_TINY.name).0
}

#[test]
fn the_plan_gives_the_phases_of_each_tree_of_each_scenario() {
    let names = |names: &[&str]| {
        names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };
    let entry = |scenario, tree| PlanEntry {
        scenario,
        tree,
        phases: Box::new(["save", "restore"]),
        memory_limited_phases: Box::new([]),
    };
    let restore_threads = PlanEntry {
        phases: Box::new([
            "save",
            "restore-1",
            "restore-2",
            "restore-4",
            "restore-8",
            "restore-20",
        ]),
        ..entry("restore-threads", "files-1g")
    };
    let memory_pressure = PlanEntry {
        memory_limited_phases: Box::new(["restore"]),
        ..entry("memory-pressure", "files-1g")
    };

    assert_eq!(
        (
            plan(&names(&["base", "smoke", "restore-threads", "memory-pressure"])).map(Vec::from),
            plan(&names(&["base", "unknown"])).map(Vec::from),
            serde_json::to_string(&entry("base", "files-1g")).unwrap(),
            serde_json::to_string(&memory_pressure).unwrap(),
        ),
        (
            Ok(vec![
                entry("base", "files-128m"),
                entry("base", "files-1g"),
                entry("base", "sqlite-1g"),
                entry("base", "objects-128m"),
                entry("base", "objects-1g"),
                entry("smoke", "files-tiny"),
                entry("smoke", "sqlite-tiny"),
                restore_threads.clone(),
                memory_pressure.clone(),
            ]),
            Err("unknown".to_string()),
            r#"{"scenario":"base","tree":"files-1g","phases":["save","restore"]}"#.to_string(),
            r#"{"scenario":"memory-pressure","tree":"files-1g","phases":["save","restore"],"memory_limited_phases":["restore"]}"#.to_string(),
        )
    );
}

#[test]
fn the_plan_gives_the_counts_of_the_concurrent_scenarios() {
    let restores = |scenario| {
        ["files-128m", "files-1g"].map(|tree| PlanEntry {
            scenario,
            tree,
            phases: Box::new([
                "save",
                "restore-x1",
                "restore-x5",
                "restore-x10",
                "restore-x25",
                "restore-x50",
                "restore-x100",
                "restore-x200",
            ]),
            memory_limited_phases: Box::new([]),
        })
    };
    let saves = ["files-128m", "files-1g", "sqlite-1g"].map(|tree| PlanEntry {
        scenario: "concurrent-save",
        tree,
        phases: Box::new(["save-x1", "save-x2", "save-x4", "save-x8"]),
        memory_limited_phases: Box::new([]),
    });
    let kinds = |scenario: &str| {
        super::scenario(scenario)
            .unwrap()
            .phases
            .iter()
            .map(|phase| phase.kind)
            .collect::<Vec<_>>()
    };
    let threads = |count| super::NonZeroUsize::new(count);

    assert_eq!(
        (
            plan(&[
                "concurrent-restore-1".to_string(),
                "concurrent-restore-4".to_string(),
                "concurrent-restore-20".to_string(),
                "concurrent-save".to_string(),
            ])
            .map(Vec::from),
            kinds("concurrent-restore-4")[1..3].to_vec(),
            kinds("concurrent-restore-20").last().copied(),
            kinds("concurrent-restore-1")[1],
            kinds("concurrent-save"),
        ),
        (
            Ok([
                restores("concurrent-restore-1").to_vec(),
                restores("concurrent-restore-4").to_vec(),
                restores("concurrent-restore-20").to_vec(),
                saves.to_vec(),
            ]
            .concat()),
            vec![
                super::PhaseKind::ConcurrentRestore {
                    agents: 1,
                    reader_threads: threads(4)
                },
                super::PhaseKind::ConcurrentRestore {
                    agents: 5,
                    reader_threads: threads(4)
                },
            ],
            Some(super::PhaseKind::ConcurrentRestore {
                agents: 200,
                reader_threads: threads(20)
            }),
            super::PhaseKind::ConcurrentRestore {
                agents: 1,
                reader_threads: threads(1)
            },
            [1, 2, 4, 8]
                .map(|agents| super::PhaseKind::ConcurrentSave { agents })
                .to_vec(),
        )
    );
}

#[test]
fn a_selection_names_a_tree_and_a_phase_of_its_scenario() {
    let found = |scenario, tree, phase| Selection::find(scenario, tree, phase).map(|_| ());

    assert_eq!(
        [
            found("base", "files-1g", "save"),
            found("smoke", "sqlite-tiny", "restore"),
            found("base", "objects-1g", "restore"),
            found("restore-threads", "files-1g", "restore-8"),
            found("memory-pressure", "files-1g", "restore"),
            found("concurrent-restore-4", "files-128m", "restore-x200"),
            found("concurrent-save", "sqlite-1g", "save-x8"),
            found("base", "files-tiny", "save"),
            found("base", "files-1g", "prune"),
            found("restore-threads", "files-128m", "save"),
            found("concurrent-save", "files-1g", "save"),
            found("other", "files-1g", "save"),
        ]
        .map(|found| found.is_ok()),
        [
            true, true, true, true, true, true, true, false, false, false, false, false
        ]
    );
}

#[test]
fn a_key_segment_has_1_to_64_ascii_letters_digits_dashes_or_underscores() {
    assert_eq!(
        [
            "12345-1",
            "limit-3",
            "no_limit",
            "",
            "a/b",
            "a.b",
            &"x".repeat(64),
            &"x".repeat(65),
        ]
        .map(is_key_segment),
        [true, true, true, false, false, false, true, false]
    );
}

#[test]
fn each_run_id_gives_its_own_repository_key() {
    assert_eq!(
        (
            repository_key("1-1") == repository_key("1-1"),
            repository_key("1-1") == repository_key("1-2"),
        ),
        (true, false)
    );
}

#[test]
fn a_result_is_at_the_path_of_its_scenario_cpu_setting_tree_and_phase() {
    assert_eq!(
        &*result_path("base", "limit-3", "files-1g", "save"),
        std::path::Path::new("results/base/limit-3/files-1g/save.json")
    );
}

#[test]
async fn the_smoke_scenario_saves_and_restores_each_tree_with_the_same_hash() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let outcomes = futures::future::join_all(["files-tiny", "sqlite-tiny"].map(|tree| {
        let storage = storage.clone();
        async move {
            let save_pod = tempfile::tempdir().unwrap();
            let restore_pod = tempfile::tempdir().unwrap();
            let (save, save_written) = run_phase(
                "run-1",
                "no-limit",
                Selection::find("smoke", tree, "save").unwrap(),
                save_pod.path(),
                storage.clone(),
                json!({}),
            )
            .await;
            let (restore, restore_written) = run_phase(
                "run-1",
                "no-limit",
                Selection::find("smoke", tree, "restore").unwrap(),
                restore_pod.path(),
                storage.clone(),
                json!({}),
            )
            .await;
            let steps = |result: &super::report::PhaseResult| {
                result
                    .steps
                    .iter()
                    .map(|step| (step.name, step.status == StepStatus::Ok))
                    .collect::<Vec<_>>()
            };
            let hash_step = restore.steps.iter().find(|step| step.name == "hash_tree");
            let restore_step = restore
                .steps
                .iter()
                .find(|step| step.name == "cold_restore");
            let cgroup_keys = |result: &PhaseResult| {
                result.environment["cgroup"]
                    .as_object()
                    .map(|cgroup| cgroup.keys().cloned().collect::<Vec<_>>())
            };
            (
                save.outcome.clone(),
                steps(&save),
                save_written.is_ok(),
                restore.outcome.clone(),
                steps(&restore),
                restore_written.is_ok(),
                hash_step.map(|step| step.details["matches"].clone()),
                save.steps
                    .iter()
                    .find(|step| step.name == "cold_save")
                    .map(|step| {
                        step.requests
                            .iter()
                            .any(|request| request.call == "put_raw" && request.file_type == "pack")
                    }),
                restore_step.map(|step| step.parameters.clone()),
                cgroup_keys(&save),
            )
        }
    }))
    .await;
    let cgroup_keys = Some(vec![
        "memory_events_end".to_string(),
        "memory_events_start".to_string(),
    ]);

    assert_eq!(
        outcomes,
        vec![
            (
                Outcome::Ok,
                vec![
                    ("generate_tree", true),
                    ("cold_save", true),
                    ("small_change", true),
                    ("warm_save", true),
                    ("hash_tree", true),
                ],
                true,
                Outcome::Ok,
                vec![("cold_restore", true), ("hash_tree", true)],
                true,
                Some(json!(true)),
                Some(true),
                Some(json!({ "reader_threads": null })),
                cgroup_keys,
            );
            2
        ]
    );
}

#[test]
async fn each_restore_of_the_restore_threads_phases_records_its_reader_threads() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (save, _save_pod) = run_tiny(&RESTORE_THREADS_TINY, "save", &storage).await;

    let restores = futures::future::join_all(["restore-1", "restore-20"].map(|phase| {
        let storage = storage.clone();
        async move {
            let (restore, _restore_pod) = run_tiny(&RESTORE_THREADS_TINY, phase, &storage).await;
            (
                restore.outcome.clone(),
                step(&restore, "cold_restore").parameters.clone(),
                step(&restore, "hash_tree").details["matches"].clone(),
            )
        }
    }))
    .await;

    assert_eq!(
        (save.outcome, restores),
        (
            Outcome::Ok,
            vec![
                (Outcome::Ok, json!({ "reader_threads": 1 }), json!(true)),
                (Outcome::Ok, json!({ "reader_threads": 20 }), json!(true)),
            ]
        )
    );
}

#[test]
async fn the_agents_of_a_concurrent_restore_get_the_first_repository_and_restore_it() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (save, _save_pod) = run_tiny(&CONCURRENT_TINY, "save", &storage).await;

    let (first, first_pod) = run_tiny(&CONCURRENT_TINY, "restore-x3", &storage).await;
    let (again, _again_pod) = run_tiny(&CONCURRENT_TINY, "restore-x3", &storage).await;
    let configs = futures::future::join_all((0..4).map(|agent| {
        let storage = storage.clone();
        async move {
            storage
                .get_metadata(
                    "test",
                    "test",
                    concurrent_tiny_namespace(),
                    &Path::new("agents").join(agent.to_string()).join("config"),
                )
                .await
                .unwrap()
                .is_some()
        }
    }))
    .await;
    let restore = step(&first, "concurrent_restore");

    assert_eq!(
        (
            save.outcome,
            first.outcome.clone(),
            step_states(&first),
            step(&first, "copy_scopes").details.clone(),
            step(&again, "copy_scopes").details.clone(),
            restore.parameters.clone(),
            [
                &restore.details["agents"],
                &restore.details["failed"],
                &restore.details["first_error"]
            ]
            .map(Value::clone),
            [
                &step(&first, "hash_trees").details["trees"],
                &step(&first, "hash_trees").details["matches"]
            ]
            .map(Value::clone),
            configs,
            first_pod.path().join("restore").exists(),
        ),
        (
            Outcome::Ok,
            Outcome::Ok,
            vec![
                ("copy_scopes", true),
                ("concurrent_restore", true),
                ("hash_trees", true)
            ],
            json!({ "agents_copied": 2, "blobs_copied": restore_blobs(&storage).await * 2 }),
            json!({ "agents_copied": 0, "blobs_copied": 0 }),
            json!({ "agents": 3, "reader_threads": 2 }),
            [json!(3), json!(0), Value::Null],
            [json!(3), json!(3)],
            vec![true, true, true, false],
            false,
        )
    );
}

/// Gives the number of blobs of the repository of the first agent of `CONCURRENT_TINY`.
async fn restore_blobs(storage: &InMemoryBlobStorage) -> usize {
    storage
        .list_blobs_below(
            "test",
            "test",
            concurrent_tiny_namespace(),
            Path::new("agents/0"),
        )
        .await
        .unwrap()
        .len()
}

#[test]
async fn the_agents_of_a_concurrent_save_each_save_their_tree_into_their_own_repository() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let (save, pod) = run_tiny(&CONCURRENT_TINY, "save-x3", &storage).await;
    let restores = futures::future::join_all((0..3).map(|agent| {
        let storage = storage.clone();
        let tree = pod.path().join("trees").join(agent.to_string());
        async move {
            let repository = Repository::new(
                Arc::new(AgentStorage::new(storage.clone(), &format!("x3-{agent}"))),
                repository_scope(CONCURRENT_TINY.name, "no-limit", FILES_TINY.name),
                repository_key("run-1"),
                STORAGE_CALL_DEADLINE,
            );
            let into = tempfile::tempdir().unwrap();
            repository
                .restore(&snapshot_name(WARM_SAVE).unwrap(), into.path(), None)
                .await
                .unwrap();
            let snapshots = storage
                .list_blobs_below(
                    "test",
                    "test",
                    concurrent_tiny_namespace(),
                    &Path::new("agents")
                        .join(format!("x3-{agent}"))
                        .join("snapshots"),
                )
                .await
                .unwrap()
                .len();
            (
                snapshots,
                tree_hash(into.path()).unwrap() == tree_hash(&tree).unwrap(),
            )
        }
    }))
    .await;
    let batch = |name| {
        let step = step(&save, name);
        (
            step.parameters.clone(),
            [
                &step.details["agents"],
                &step.details["failed"],
                &step.details["first_error"],
            ]
            .map(Value::clone),
            step.details["data_added"]
                .as_u64()
                .is_some_and(|added| added > 0),
        )
    };
    let copied = &step(&save, "copy_trees").details;

    assert_eq!(
        (
            save.outcome.clone(),
            step_states(&save),
            batch("concurrent_cold_save"),
            batch("concurrent_warm_save"),
            (
                copied["copies"].clone(),
                copied["files_reflinked"].as_u64().unwrap_or_default()
                    + copied["files_copied"].as_u64().unwrap_or_default()
            ),
            step(&save, "small_change").details["trees"].clone(),
            restores,
        ),
        (
            Outcome::Ok,
            vec![
                ("generate_tree", true),
                ("copy_trees", true),
                ("concurrent_cold_save", true),
                ("small_change", true),
                ("concurrent_warm_save", true),
            ],
            (
                json!({ "agents": 3 }),
                [json!(3), json!(0), Value::Null],
                true
            ),
            (
                json!({ "agents": 3 }),
                [json!(3), json!(0), Value::Null],
                true
            ),
            (json!(2), 200),
            json!(3),
            vec![(2, true); 3],
        )
    );
}

/// A tiny tree whose content compresses.
const COMPRESSIBLE_TINY: TreeSpec = TreeSpec {
    name: "compressible-tiny",
    shape: TreeShape::Files {
        files: 100,
        directories: 10,
        bytes: 1024 * 1024,
    },
    content: Content::Compressible,
};

static CAPTURE_TINY: Scenario = Scenario {
    name: "capture-tiny",
    trees: &[FILES_TINY],
    phases: &[with_defaults("capture", PhaseKind::Capture)],
    memory_limited_phases: &[],
};

static PRUNE_TINY: Scenario = Scenario {
    name: "prune-tiny",
    trees: &[FILES_TINY],
    phases: &[
        HISTORY,
        prune_phase("prune", &PRUNE, false),
        prune_phase("prune-fast-repack", &PRUNE_FAST_REPACK, true),
    ],
    memory_limited_phases: &[],
};

static SQLITE_CHANGES_TINY: Scenario = Scenario {
    name: "sqlite-changes-tiny",
    trees: &[SQLITE_TINY],
    phases: &[
        Phase {
            name: "save-rabin",
            kind: PhaseKind::SqliteChanges,
            variant: &SQLITE_RABIN,
        },
        Phase {
            name: "save-fixed-64k",
            kind: PhaseKind::SqliteChanges,
            variant: &SQLITE_FIXED_64K,
        },
        Phase {
            name: "restore-fixed-64k",
            kind: PhaseKind::Restore {
                reader_threads: None,
            },
            variant: &SQLITE_FIXED_64K,
        },
    ],
    memory_limited_phases: &[],
};

static MIXED_TINY: Scenario = Scenario {
    name: "mixed-tiny",
    trees: &[FILES_TINY],
    phases: &[
        with_defaults("save", PhaseKind::Save),
        mixed_phase("mixed-s2-r2", &SAVE_THREADS_2, 2),
        save_threads_phase("save-x4-t2", &SAVE_THREADS_2),
    ],
    memory_limited_phases: &[],
};

static SCOPES_TINY: Scenario = Scenario {
    name: "scopes-tiny",
    trees: &[FILES_TINY],
    phases: &[
        with_defaults("save", PhaseKind::Save),
        with_defaults("scopes", PhaseKind::Scopes),
    ],
    memory_limited_phases: &[],
};

static CPU_OPTIONS_TINY: Scenario = Scenario {
    name: "cpu-options-tiny",
    trees: &[COMPRESSIBLE_TINY],
    phases: &[
        save_phase("save-default", &CPU_DEFAULT),
        save_phase("save-zstd-off", &CPU_ZSTD_OFF),
    ],
    memory_limited_phases: &[],
};

/// Gives the value at the JSON pointer of the details of the step.
fn detail(result: &PhaseResult, step_name: &str, pointer: &str) -> Value {
    step(result, step_name)
        .details
        .pointer(pointer)
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn the_plan_gives_the_phases_of_each_later_scenario() {
    let entry = |scenario, tree, phases: &[&'static str]| PlanEntry {
        scenario,
        tree,
        phases: phases.into(),
        memory_limited_phases: Box::new([]),
    };
    let base_trees = [
        "files-128m",
        "files-1g",
        "sqlite-1g",
        "objects-128m",
        "objects-1g",
    ];
    let history_trees = ["files-1g", "sqlite-1g", "objects-1g"];
    let each = |scenario, trees: &[&'static str], phases: &[&'static str]| {
        trees
            .iter()
            .map(|tree| entry(scenario, *tree, phases))
            .collect::<Vec<_>>()
    };
    let names = [
        "capture",
        "prune",
        "save-threads",
        "mixed",
        "repository-open",
        "sqlite-changes",
        "tree-shape",
        "cpu-options",
        "scopes",
    ]
    .map(str::to_string);

    assert_eq!(
        plan(&names).map(Vec::from),
        Ok([
            each("capture", &base_trees, &["capture"]),
            each(
                "prune",
                &history_trees,
                &["history", "prune", "prune-fast-repack"]
            ),
            each(
                "save-threads",
                &["files-1g", "sqlite-1g"],
                &["save-x4-t1", "save-x4-t2", "save-x4-t4", "save-x4-tdefault"]
            ),
            each(
                "mixed",
                &["files-1g"],
                &[
                    "save",
                    "mixed-s2-r2",
                    "mixed-s2-r4",
                    "mixed-s4-r2",
                    "mixed-s4-r4"
                ]
            ),
            each("repository-open", &history_trees, &["history", "prune"]),
            each(
                "sqlite-changes",
                &["sqlite-1g"],
                &[
                    "save-rabin",
                    "restore-rabin",
                    "save-fixed-64k",
                    "restore-fixed-64k"
                ]
            ),
            each(
                "tree-shape",
                &["files-128m", "modules-128m"],
                &["save", "restore"]
            ),
            each(
                "cpu-options",
                &["compressible-1g"],
                &[
                    "save-default",
                    "save-verify-off",
                    "save-zstd-off",
                    "save-zstd-1",
                    "save-zstd-9"
                ]
            ),
            each("scopes", &base_trees, &["save", "scopes"]),
        ]
        .concat())
    );
}

#[test]
fn a_later_phase_records_its_settings_and_an_earlier_phase_records_none() {
    let record = || StepRecord::skipped("save").with_parameters(json!({ "agents": 4 }));
    let own =
        StepRecord::skipped("save").with_parameters(json!({ "change_detection": "size-mtime" }));

    assert_eq!(
        (
            with_settings(record(), &BASE).parameters,
            with_settings(record(), &SAVE_THREADS_2).parameters,
            with_settings(own, &DEFAULTS).parameters["change_detection"].clone(),
            with_settings(record(), &SQLITE_FIXED_64K).parameters["chunker"].clone(),
            with_settings(record(), &CPU_ZSTD_OFF).parameters["compression"].clone(),
        ),
        (
            json!({ "agents": 4 }),
            json!({
                "agents": 4,
                "save_threads": 2,
                "change_detection": "ctime",
                "chunker": "rabin",
                "compression": null,
                "extra_verify": true,
            }),
            json!("size-mtime"),
            json!("fixed-65536"),
            json!("off"),
        )
    );
}

#[test]
async fn a_capture_phase_reads_every_file_with_ctime_and_the_changed_files_with_size_and_mtime() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let (result, _pod) = run_tiny(&CAPTURE_TINY, "capture", &storage).await;
    let counts = |name: &str| {
        (
            detail(&result, name, "/files_new"),
            detail(&result, name, "/files_changed"),
            detail(&result, name, "/files_unmodified"),
        )
    };
    let captured = |name: &str| {
        detail(&result, name, "/files_reflinked")
            .as_u64()
            .unwrap_or_default()
            + detail(&result, name, "/files_copied")
                .as_u64()
                .unwrap_or_default()
    };

    assert_eq!(
        (
            result.outcome.clone(),
            step_states(&result),
            [
                captured("capture"),
                captured("capture_full_read"),
                captured("capture_size_mtime")
            ],
            counts("warm_save_full_read"),
            counts("warm_save_size_mtime"),
            step(&result, "warm_save_size_mtime").parameters["change_detection"].clone(),
            tree_counts(&result),
            result.tree_facts.change.as_ref() == Some(&step(&result, "small_change").details),
        ),
        (
            Outcome::Ok,
            vec![
                ("generate_tree", true),
                ("capture", true),
                ("cold_save", true),
                ("copy_scopes", true),
                ("small_change", true),
                ("capture_full_read", true),
                ("warm_save_full_read", true),
                ("capture_size_mtime", true),
                ("warm_save_size_mtime", true),
            ],
            [100, 101, 101],
            (json!(1), json!(100), json!(0)),
            (json!(1), json!(10), json!(90)),
            json!("size-mtime"),
            FILES_TINY_COUNTS,
            true,
        )
    );
}

#[test]
async fn the_prune_phases_give_back_the_data_of_the_forgotten_snapshots_of_the_history() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (history, _history_pod) = run_tiny(&PRUNE_TINY, "history", &storage).await;

    let prunes = futures::future::join_all(["prune", "prune-fast-repack"].map(|phase| {
        let storage = storage.clone();
        async move {
            let (result, _pod) = run_tiny(&PRUNE_TINY, phase, &storage).await;
            (
                result.outcome.clone(),
                step_states(&result),
                step(&result, "prune_mark").parameters["fast_repack"].clone(),
                detail(&result, "prune_mark", "/packs_repacked")
                    .as_u64()
                    .is_some_and(|packs| packs > 0),
                detail(&result, "prune_delete", "/marked_packs_deleted")
                    .as_u64()
                    .is_some_and(|packs| packs > 0),
                detail(&result, "prune_delete", "/repository/bytes_given_back")
                    .as_u64()
                    .is_some_and(|bytes| bytes > 0),
                detail(&result, "open", "/snapshots"),
                detail(&result, "hash_tree", "/matches"),
                tree_counts(&result),
                result.tree_facts.hash.as_deref().map(|hash| json!(hash))
                    == Some(detail(&result, "hash_tree", "/hash")),
            )
        }
    }))
    .await;
    let opens = history
        .steps
        .iter()
        .filter(|step| step.name == "open")
        .map(|step| {
            (
                step.parameters["saves"].clone(),
                step.details["snapshots"].clone(),
            )
        })
        .collect::<Vec<_>>();
    let saves = history
        .steps
        .iter()
        .filter(|step| step.name == "save")
        .count();
    let prune = |fast_repack| {
        (
            Outcome::Ok,
            vec![
                ("copy_scopes", true),
                ("prune_mark", true),
                ("prune_delete", true),
                ("open", true),
                ("cold_restore", true),
                ("hash_tree", true),
            ],
            json!(fast_repack),
            true,
            true,
            true,
            json!(2),
            json!(true),
            // The history adds one file of 10,486 bytes in each of its 11 rounds.
            (Some(111), Some(10), Some(1024 * 1024 + 11 * 10_486)),
            true,
        )
    };

    assert_eq!(
        (
            history.outcome.clone(),
            saves,
            opens,
            detail(&history, "forget", "/snapshots_forgotten"),
            tree_counts(&history),
            prunes,
        ),
        (
            Outcome::Ok,
            11,
            vec![
                (json!(1), json!(1)),
                (json!(11), json!(11)),
                (json!(12), json!(12)),
            ],
            json!(10),
            FILES_TINY_COUNTS,
            vec![prune(false), prune(true)],
        )
    );
}

#[test]
async fn fixed_chunks_save_less_after_a_clustered_change_and_restore_the_database() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let (rabin, _rabin_pod) = run_tiny(&SQLITE_CHANGES_TINY, "save-rabin", &storage).await;
    let (fixed, _fixed_pod) = run_tiny(&SQLITE_CHANGES_TINY, "save-fixed-64k", &storage).await;
    let (restore, _restore_pod) =
        run_tiny(&SQLITE_CHANGES_TINY, "restore-fixed-64k", &storage).await;
    let added = |result: &PhaseResult, name: &str| {
        detail(result, name, "/data_added")
            .as_u64()
            .unwrap_or_default()
    };

    assert_eq!(
        (
            rabin.outcome.clone(),
            fixed.outcome.clone(),
            step_states(&fixed),
            added(&fixed, "warm_save_clustered") < added(&rabin, "warm_save_clustered"),
            step(&fixed, "cold_save").parameters["chunker"].clone(),
            detail(&fixed, "clustered_change", "/rows_updated"),
            restore.outcome.clone(),
            detail(&restore, "hash_tree", "/matches"),
            (fixed.tree_facts.files, fixed.tree_facts.directories),
            fixed
                .tree_facts
                .bytes
                .is_some_and(|bytes| bytes >= 4 * 1024 * 1024),
            fixed.tree_facts.change.as_ref() == Some(&step(&fixed, "scattered_change").details),
            [
                detail(&rabin, "open", "/settings/chunker"),
                detail(&fixed, "open", "/settings"),
            ],
        ),
        (
            Outcome::Ok,
            Outcome::Ok,
            vec![
                ("generate_tree", true),
                ("cold_save", true),
                ("clustered_change", true),
                ("warm_save_clustered", true),
                ("scattered_change", true),
                ("warm_save_scattered", true),
                ("hash_tree", true),
                ("open", true),
            ],
            true,
            json!("fixed-65536"),
            json!(100),
            Outcome::Ok,
            json!(true),
            (Some(1), Some(0)),
            true,
            true,
            [
                json!("rabin"),
                json!({ "chunker": "fixed-65536", "compression": null, "extra_verify": true }),
            ],
        )
    );
}

#[test]
async fn a_mixed_phase_saves_and_restores_at_the_same_time_with_its_thread_counts() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (save, _save_pod) = run_tiny(&MIXED_TINY, "save", &storage).await;

    let (mixed, _mixed_pod) = run_tiny(&MIXED_TINY, "mixed-s2-r2", &storage).await;
    let (threads, _threads_pod) = run_tiny(&MIXED_TINY, "save-x4-t2", &storage).await;
    let configs = futures::future::join_all(
        [
            "agents/1",
            "agents/4",
            "agents/5",
            "agents/mixed-s2-r2-3",
            "agents/save-x4-t2-3",
        ]
        .map(|agent| {
            let storage = storage.clone();
            async move {
                storage
                    .get_metadata(
                        "test",
                        "test",
                        repository_scope(MIXED_TINY.name, "no-limit", FILES_TINY.name).0,
                        &Path::new(agent).join("config"),
                    )
                    .await
                    .unwrap()
                    .is_some()
            }
        }),
    )
    .await;
    let step_mixed = step(&mixed, "mixed");

    assert_eq!(
        (
            save.outcome,
            mixed.outcome.clone(),
            step_states(&mixed),
            [
                &step_mixed.parameters["saves"],
                &step_mixed.parameters["restores"],
                &step_mixed.parameters["save_threads"],
                &step_mixed.parameters["reader_threads"],
            ]
            .map(Value::clone),
            [
                &step_mixed.details["saves"]["agents"],
                &step_mixed.details["saves"]["failed"],
                &step_mixed.details["restores"]["agents"],
                &step_mixed.details["restores"]["failed"],
            ]
            .map(Value::clone),
            detail(&mixed, "hash_trees", "/matches"),
            threads.outcome.clone(),
            step(&threads, "concurrent_cold_save").parameters["save_threads"].clone(),
            configs,
        ),
        (
            Outcome::Ok,
            Outcome::Ok,
            vec![
                ("copy_scopes", true),
                ("generate_tree", true),
                ("copy_trees", true),
                ("mixed", true),
                ("hash_trees", true),
            ],
            [json!(4), json!(4), json!(2), json!(2)],
            [json!(4), json!(0), json!(4), json!(0)],
            json!(4),
            Outcome::Ok,
            json!(2),
            vec![true, true, false, true, true],
        )
    );
}

#[test]
async fn a_scopes_phase_copies_the_repository_whole_and_deletes_the_copy_whole() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (save, _save_pod) = run_tiny(&SCOPES_TINY, "save", &storage).await;

    let (scopes, _pod) = run_tiny(&SCOPES_TINY, "scopes", &storage).await;

    assert_eq!(
        (
            save.outcome,
            scopes.outcome.clone(),
            step_states(&scopes),
            detail(&scopes, "copy_scope", "/same_as_source"),
            detail(&scopes, "copy_scope", "/blobs_copied")
                .as_u64()
                .is_some_and(|blobs| blobs > 0),
            detail(&scopes, "delete_scope", "/blobs_left"),
        ),
        (
            Outcome::Ok,
            Outcome::Ok,
            vec![("copy_scope", true), ("delete_scope", true)],
            json!(true),
            true,
            json!(0),
        )
    );
}

#[test]
async fn a_cpu_options_phase_makes_its_repository_with_its_compression() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let (default, _default_pod) = run_tiny(&CPU_OPTIONS_TINY, "save-default", &storage).await;
    let (off, _off_pod) = run_tiny(&CPU_OPTIONS_TINY, "save-zstd-off", &storage).await;
    let packed = |result: &PhaseResult| {
        (
            detail(result, "cold_save", "/data_added").as_u64(),
            detail(result, "cold_save", "/data_added_packed").as_u64(),
        )
    };
    let (default_added, default_packed) = packed(&default);
    let (off_added, off_packed) = packed(&off);

    assert_eq!(
        (
            default.outcome.clone(),
            off.outcome.clone(),
            default.tree_facts.content,
            default_packed < default_added.map(|added| added * 3 / 4),
            off_packed >= off_added,
            step(&off, "cold_save").parameters["compression"].clone(),
            step(&default, "open").details.clone(),
            detail(&off, "open", "/settings"),
        ),
        (
            Outcome::Ok,
            Outcome::Ok,
            Some("compressible"),
            true,
            true,
            json!("off"),
            json!({
                "snapshots": 2,
                "found": true,
                "settings": { "chunker": "rabin", "compression": null, "extra_verify": true },
            }),
            json!({ "chunker": "rabin", "compression": "off", "extra_verify": true }),
        )
    );
}

/// A blob storage that passes each call to an in-memory storage, and fails each write of a
/// snapshot file below `prefix` after the first `allowed` of them. After `config_hidden_after`
/// such writes, it gives no metadata for the config of the agent, so the repository looks absent.
#[derive(Debug)]
struct FailingSnapshotWrites {
    inner: Arc<InMemoryBlobStorage>,
    prefix: PathBuf,
    config: PathBuf,
    allowed: usize,
    config_hidden_after: usize,
    seen: AtomicUsize,
}

impl FailingSnapshotWrites {
    /// Gives a storage over `inner` that fails the writes of snapshot files of the agent after the
    /// first `allowed` of them.
    fn new(inner: Arc<InMemoryBlobStorage>, agent: &str, allowed: usize) -> Arc<Self> {
        Self::hiding_config(inner, agent, allowed, usize::MAX)
    }

    /// Gives a storage over `inner` that fails the writes of snapshot files of the agent after the
    /// first `allowed` of them, and hides the config of the agent after `config_hidden_after`
    /// writes of snapshot files.
    fn hiding_config(
        inner: Arc<InMemoryBlobStorage>,
        agent: &str,
        allowed: usize,
        config_hidden_after: usize,
    ) -> Arc<Self> {
        let root = Path::new("agents").join(agent);
        Arc::new(Self {
            inner,
            prefix: root.join("snapshots"),
            config: root.join("config"),
            allowed,
            config_hidden_after,
            seen: AtomicUsize::new(0),
        })
    }

    /// Tells whether the write of the path fails.
    fn fails(&self, path: &Path) -> bool {
        path.starts_with(&self.prefix) && self.seen.fetch_add(1, Ordering::SeqCst) >= self.allowed
    }
}

#[async_trait]
impl BlobStorage for FailingSnapshotWrites {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw(target_label, op_label, namespace, path)
            .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.inner
            .get_stream(target_label, op_label, namespace, path)
            .await
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw_slice(target_label, op_label, namespace, path, start, end)
            .await
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        if path == self.config && self.seen.load(Ordering::SeqCst) >= self.config_hidden_after {
            return Ok(None);
        }
        self.inner
            .get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!self.fails(path), "the write of {} fails", path.display());
        self.inner
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.inner
            .put_stream(target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.inner
            .delete(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.inner
            .delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists(target_label, op_label, namespace, path)
            .await
    }
}

/// The steps of the two forms of a capture phase, in the order in which they run.
const FORM_STEPS: [&str; 4] = [
    "capture_full_read",
    "warm_save_full_read",
    "capture_size_mtime",
    "warm_save_size_mtime",
];

#[test]
async fn a_failed_capture_or_warm_save_of_a_form_skips_the_steps_after_it() {
    // Form 0 saves into the repository of agent 0, after its cold save. Form 1 saves into the
    // repository of agent 1, after the copy of the cold save, which the default `copy` of the
    // storage writes. So in both repositories the warm save writes the second snapshot file. A
    // capture fails when its directory exists before it.
    let capture_fails = async |capture: &'static str| {
        let (result, _pod) = run_tiny_with(
            &CAPTURE_TINY,
            "capture",
            Arc::new(InMemoryBlobStorage::new()),
            |work| std::fs::create_dir(work.join(capture)).unwrap(),
        )
        .await;
        (result.outcome.clone(), skipped_steps(&result))
    };
    let save_fails = async |agent: &str, allowed| {
        let storage =
            FailingSnapshotWrites::new(Arc::new(InMemoryBlobStorage::new()), agent, allowed);
        let (result, _pod) = run_tiny_with(&CAPTURE_TINY, "capture", storage, |_| {}).await;
        (result.outcome.clone(), skipped_steps(&result))
    };
    let failed = |step: &str| Outcome::Failed {
        reason: format!("the step {step} failed").into(),
    };

    let outcomes = [
        capture_fails(FORM_STEPS[0]).await,
        save_fails("0", 1).await,
        capture_fails(FORM_STEPS[2]).await,
        save_fails("1", 1).await,
    ];

    assert_eq!(
        outcomes,
        [
            (failed(FORM_STEPS[0]), FORM_STEPS[1..].to_vec()),
            (failed(FORM_STEPS[1]), FORM_STEPS[2..].to_vec()),
            (failed(FORM_STEPS[2]), FORM_STEPS[3..].to_vec()),
            (failed(FORM_STEPS[3]), Vec::new()),
        ]
    );
}

/// Gives the namespace of the repositories of the agents of a phase of `MIXED_TINY`.
fn mixed_tiny_namespace() -> BlobStorageNamespace {
    repository_scope(MIXED_TINY.name, "no-limit", FILES_TINY.name).0
}

#[test]
async fn a_mixed_phase_whose_restores_fail_fails_at_the_mixed_step_with_the_error_of_a_restore() {
    // The repository of agent 1 has a config that no key opens, so the copy of the first
    // repository leaves it out, and its restore fails. The saves go to other agents and succeed.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (save, _save_pod) = run_tiny(&MIXED_TINY, "save", &storage).await;
    storage
        .put_raw(
            "test",
            "test",
            mixed_tiny_namespace(),
            Path::new("agents/1/config"),
            b"not a config",
        )
        .await
        .unwrap();

    let (mixed, _pod) = run_tiny(&MIXED_TINY, "mixed-s2-r2", &storage).await;
    let details = &step(&mixed, "mixed").details;

    assert_eq!(
        (
            save.outcome,
            mixed.outcome.clone(),
            step(&mixed, "mixed").status != StepStatus::Ok,
            details["first_error"].is_string(),
            [&details["saves"]["failed"], &details["restores"]["failed"]].map(Value::clone),
            skipped_steps(&mixed),
            tree_counts(&mixed),
        ),
        (
            Outcome::Ok,
            Outcome::Failed {
                reason: "the step mixed failed".into()
            },
            true,
            true,
            [json!(0), json!(1)],
            vec!["hash_trees"],
            FILES_TINY_COUNTS,
        )
    );
}

#[test]
async fn the_restores_of_a_mixed_phase_read_the_copies_and_not_the_first_repository() {
    // The copies exist before the phase, so the phase copies nothing, and the first repository
    // loses its snapshot files. A restore that reads the first repository finds no snapshot.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (save, _save_pod) = run_tiny(&MIXED_TINY, "save", &storage).await;
    super::agents::copy_first_agent(storage.as_ref(), &mixed_tiny_namespace(), 5)
        .await
        .unwrap();
    storage
        .delete_dir(
            "test",
            "test",
            mixed_tiny_namespace(),
            Path::new("agents/0/snapshots"),
        )
        .await
        .unwrap();

    let (mixed, _pod) = run_tiny(&MIXED_TINY, "mixed-s2-r2", &storage).await;

    assert_eq!(
        (
            save.outcome,
            mixed.outcome.clone(),
            detail(&mixed, "copy_scopes", "/agents_copied"),
            detail(&mixed, "hash_trees", "/matches"),
        ),
        (Outcome::Ok, Outcome::Ok, json!(0), json!(4))
    );
}

#[test]
fn each_cpu_options_variant_changes_only_its_own_setting_of_the_defaults() {
    let defaults = RepositorySettings::DEFAULT;
    let variants = CPU_OPTIONS_PHASES
        .iter()
        .map(|phase| {
            (
                phase.name,
                phase.variant.settings.map(|settings| settings.repository),
                phase.variant.settings.map(|settings| settings.save),
            )
        })
        .collect::<Vec<_>>();
    let level = |level| Compression::Level(NonZeroI32::new(level).unwrap());

    assert_eq!(
        variants,
        [
            ("save-default", defaults),
            (
                "save-verify-off",
                RepositorySettings {
                    extra_verify: false,
                    ..defaults
                }
            ),
            (
                "save-zstd-off",
                RepositorySettings {
                    compression: Compression::Off,
                    ..defaults
                }
            ),
            (
                "save-zstd-1",
                RepositorySettings {
                    compression: level(1),
                    ..defaults
                }
            ),
            (
                "save-zstd-9",
                RepositorySettings {
                    compression: level(9),
                    ..defaults
                }
            ),
        ]
        .map(|(name, repository)| (name, Some(repository), Some(SaveSettings::DEFAULT)))
        .to_vec()
    );
}

#[test]
fn a_save_threads_phase_saves_with_its_threads() {
    let scenario = super::scenario("save-threads").unwrap();
    let context = |phase: &str| PhaseContext {
        run_id: "run-1".into(),
        cpu_setting: "no-limit".into(),
        selection: Selection::find("save-threads", "files-1g", phase).unwrap(),
        work_dir: Path::new("/nowhere").into(),
        storage: Arc::new(MeasuredBlobStorage::new(Arc::new(
            InMemoryBlobStorage::new(),
        ))),
    };

    assert_eq!(
        (
            scenario.phases.len(),
            ["save-x4-t1", "save-x4-t2", "save-x4-t4", "save-x4-tdefault"]
                .map(|phase| context(phase).save_settings()),
        ),
        (
            4,
            [
                NonZeroUsize::new(1),
                NonZeroUsize::new(2),
                NonZeroUsize::new(4),
                None
            ]
            .map(|threads| SaveSettings {
                threads,
                ..SaveSettings::DEFAULT
            }),
        )
    );
}

#[test]
async fn a_save_and_open_phase_whose_save_fails_skips_the_open() {
    // The cold save writes the first snapshot file of the repository, and the last warm save
    // writes the last one: the second for the cpu-options phase, and the third for the
    // sqlite-changes phase, which also saves after its clustered change.
    let fail_last_save = async |scenario: &'static Scenario, phase, agent, snapshots: usize| {
        let storage =
            FailingSnapshotWrites::new(Arc::new(InMemoryBlobStorage::new()), agent, snapshots - 1);
        let (result, _pod) = run_tiny_with(scenario, phase, storage, |_| {}).await;
        (result.outcome.clone(), skipped_steps(&result))
    };

    let outcomes = [
        fail_last_save(&CPU_OPTIONS_TINY, "save-default", "default", 2).await,
        fail_last_save(&SQLITE_CHANGES_TINY, "save-rabin", "rabin", 3).await,
    ];

    assert_eq!(
        outcomes,
        [
            (
                Outcome::Failed {
                    reason: "the step warm_save failed".into()
                },
                vec!["hash_tree", "open"]
            ),
            (
                Outcome::Failed {
                    reason: "the step warm_save_scattered failed".into()
                },
                vec!["hash_tree", "open"]
            ),
        ]
    );
}

#[test]
async fn a_save_and_open_phase_fails_at_the_open_when_the_open_finds_no_repository() {
    // The config of the repository disappears after the second snapshot file, which the warm
    // save writes last, so the open after the saves finds no repository.
    let storage = FailingSnapshotWrites::hiding_config(
        Arc::new(InMemoryBlobStorage::new()),
        "default",
        usize::MAX,
        2,
    );

    let (result, _pod) = run_tiny_with(&CPU_OPTIONS_TINY, "save-default", storage, |_| {}).await;

    assert_eq!(
        (
            result.outcome.clone(),
            step_states(&result).last().copied(),
            step(&result, "open").details.clone(),
            skipped_steps(&result),
        ),
        (
            Outcome::Failed {
                reason: "the step open failed".into()
            },
            Some(("open", true)),
            json!({ "repository": null }),
            Vec::<&str>::new(),
        )
    );
}
