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

use super::report::{Outcome, StepStatus};
use super::{PlanEntry, Selection, is_key_segment, plan, repository_key, result_path, run_phase};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use test_r::test;

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
    };

    assert_eq!(
        (
            plan(&names(&["base", "smoke"])).map(Vec::from),
            plan(&names(&["base", "unknown"])).map(Vec::from),
            serde_json::to_string(&entry("base", "files-1g")).unwrap(),
        ),
        (
            Ok(vec![
                entry("base", "files-128m"),
                entry("base", "files-1g"),
                entry("base", "sqlite-1g"),
                entry("smoke", "files-tiny"),
                entry("smoke", "sqlite-tiny"),
            ]),
            Err("unknown".to_string()),
            r#"{"scenario":"base","tree":"files-1g","phases":["save","restore"]}"#.to_string(),
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
            found("base", "files-tiny", "save"),
            found("base", "files-1g", "prune"),
            found("other", "files-1g", "save"),
        ]
        .map(|found| found.is_ok()),
        [true, true, false, false, false]
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
        result_path("base", "limit-3", "files-1g", "save"),
        std::path::PathBuf::from("results/base/limit-3/files-1g/save.json")
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
            )
        }
    }))
    .await;

    assert_eq!(
        outcomes,
        vec![
            (
                Outcome::Ok,
                vec![
                    ("generate_tree", true),
                    ("cold_save", true),
                    ("small_change", true),
                    ("hash_tree", true),
                    ("warm_save", true),
                ],
                true,
                Outcome::Ok,
                vec![("cold_restore", true), ("hash_tree", true)],
                true,
                Some(json!(true)),
                Some(true),
            );
            2
        ]
    );
}
