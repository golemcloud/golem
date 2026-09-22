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
use super::trees::{FILES_TINY, tree_hash};
use super::{
    PlanEntry, RESTORE_THREADS_PHASES, Repository, SAVE, Scenario, Selection, WARM_SAVE,
    concurrent_restore, concurrent_save, is_key_segment, plan, repository_key, repository_scope,
    result_path, run_phase, snapshot_name,
};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
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
    let work = tempfile::tempdir().unwrap();
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
        storage.clone(),
        json!({}),
    )
    .await;
    written.unwrap();
    (result, work)
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
            true, true, true, true, true, true, false, false, false, false, false
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
