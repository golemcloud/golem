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

//! The save phase of the SQLite changes scenario.

use super::measure::measure;
use super::report::{Outcome, TreeFacts};
use super::{
    COLD_SAVE, PhaseContext, PhaseOutcome, WARM_SAVE, failed, save_record, snapshot_name, trees,
};
use serde_json::{Value, json};

/// The name of the snapshot after the clustered change.
const CLUSTERED_SAVE: &str = "warm-save-clustered";

/// The save phase of a SQLite tree into the repository of the variant of the phase: a cold save,
/// an update of 100 consecutive rows and a warm save, then an update of 100 rows spread over the
/// database and a second warm save, and the hash of the tree.
///
/// The chunker of the repository decides how many bytes each warm save adds. The second warm save
/// has the name of the warm save of the base scenario, so the restore phase of the base scenario
/// restores it.
pub(super) async fn sqlite_changes(context: &PhaseContext) -> PhaseOutcome {
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
    let later = [
        "cold_save",
        "clustered_change",
        "warm_save_clustered",
        "scattered_change",
        "warm_save_scattered",
        "hash_tree",
    ];

    let (record, generated) = measure("generate_tree", storage, trees::generate(spec, &tree)).await;
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

    let (record, cold) = measure("cold_save", storage, async {
        repository
            .save_with(&snapshot_name(COLD_SAVE)?, &tree, settings)
            .await
    })
    .await;
    steps.push(save_record(record, &cold));
    if cold.is_err() {
        return failed(facts, steps, "cold_save", &later[1..]);
    }

    let (record, clustered) = measure(
        "clustered_change",
        storage,
        trees::change_clustered(spec, &tree),
    )
    .await;
    steps.push(record.with_details(
        clustered.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    ));
    if clustered.is_err() {
        return failed(facts, steps, "clustered_change", &later[2..]);
    }

    let (record, warm) = measure("warm_save_clustered", storage, async {
        repository
            .save_with(&snapshot_name(CLUSTERED_SAVE)?, &tree, settings)
            .await
    })
    .await;
    steps.push(save_record(record, &warm).with_parameters(json!({ "change": "clustered" })));
    if warm.is_err() {
        return failed(facts, steps, "warm_save_clustered", &later[3..]);
    }

    let (record, scattered) =
        measure("scattered_change", storage, trees::change(spec, &tree)).await;
    steps.push(record.with_details(
        scattered.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    ));
    let Ok(change) = scattered else {
        return failed(facts, steps, "scattered_change", &later[4..]);
    };
    let facts = TreeFacts {
        change: Some(change),
        ..facts
    };

    let (record, warm) = measure("warm_save_scattered", storage, async {
        repository
            .save_with(&snapshot_name(WARM_SAVE)?, &tree, settings)
            .await
    })
    .await;
    steps.push(save_record(record, &warm).with_parameters(json!({ "change": "scattered" })));
    if warm.is_err() {
        return failed(facts, steps, "warm_save_scattered", &later[5..]);
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
