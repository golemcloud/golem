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

//! The phase of the capture scenario: the time of a capture, and the warm saves of a capture with
//! each change detection.
//!
//! A capture is a reflink copy of the tree of an agent that keeps the permissions and the
//! modification times, as the executor makes it before an upload. Each captured file is a new
//! inode with a new change time. So a save that compares change times reads every file of a
//! capture, and a save that compares only sizes and modification times reads the changed files.

use super::agents::{FIRST_AGENT, copy_agent};
use super::measure::measure;
use super::report::{Outcome, StepRecord, TreeFacts};
use super::trees::{self, CopyCounts, Times};
use super::{
    COLD_SAVE, PhaseContext, PhaseOutcome, WARM_SAVE, change_detection_name, failed, save_record,
    snapshot_name,
};
use crate::filesystem_snapshot::rustic::{ChangeDetection, SaveSettings};
use futures::{StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The agent whose repository gets the warm save with change detection by size and modification
/// time. The first agent gets the warm save that compares change times.
const SIZE_MTIME_AGENT: &str = "1";

/// The capture phase: a new tree, a capture of it, and a cold save of the capture into the
/// repository of the first agent, which then goes to a second agent on the server. Then a small
/// change of the tree, and for each change detection a new capture and a warm save of it into the
/// repository of one of the agents. So both warm saves have the same parent, and each reads a
/// capture that no step read before.
///
/// The time of a capture step is the time of the capture: the copy does not sync the volume.
pub(super) async fn capture(context: &PhaseContext) -> PhaseOutcome {
    let spec = context.selection.tree;
    let storage = &context.storage;
    let tree = context.work_dir.join("tree");
    let facts = TreeFacts {
        name: spec.name,
        content: Some(spec.content.label()),
        page_cache: Some("dropped"),
        ..TreeFacts::default()
    };
    let later = [
        "capture",
        "cold_save",
        "copy_scopes",
        "small_change",
        FORM_STEPS[0],
        FORM_STEPS[1],
        FORM_STEPS[2],
        FORM_STEPS[3],
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

    let first = context.work_dir.join("capture-cold");
    let (record, captured) = measure("capture", storage, capture_tree(&tree, &first)).await;
    steps.push(capture_record(record, &captured));
    if captured.is_err() {
        return failed(facts, steps, "capture", &later[1..]);
    }

    let (record, cold) = measure("cold_save", storage, async {
        context
            .repository()
            .save_with(&snapshot_name(COLD_SAVE)?, &first, SaveSettings::DEFAULT)
            .await
    })
    .await;
    steps.push(save_record(record, &cold));
    if cold.is_err() {
        return failed(facts, steps, "cold_save", &later[2..]);
    }

    let (record, copied) = measure(
        "copy_scopes",
        storage,
        copy_agent(
            storage.as_ref(),
            &context.scope().0,
            FIRST_AGENT,
            SIZE_MTIME_AGENT,
        ),
    )
    .await;
    steps.push(record.with_details(
        copied.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    ));
    if copied.is_err() {
        return failed(facts, steps, "copy_scopes", &later[3..]);
    }

    let (record, changed) = measure("small_change", storage, trees::change(spec, &tree)).await;
    steps.push(record.with_details(
        changed.as_ref().ok().cloned().unwrap_or(Value::Null),
        Box::default(),
    ));
    let Ok(change) = changed else {
        return failed(facts, steps, "small_change", &later[4..]);
    };
    let facts = TreeFacts {
        change: Some(change),
        ..facts
    };

    match warm_forms(context, &tree, steps).await {
        Ok(steps) => PhaseOutcome {
            tree_facts: facts,
            steps,
            outcome: Outcome::Ok,
        },
        Err((steps, step, skipped)) => failed(facts, steps, step, skipped),
    }
}

/// One form of a warm save: the name of its capture step, the name of its save step, its agent
/// and its change detection.
type Form = (&'static str, &'static str, &'static str, ChangeDetection);

/// The forms of the warm saves, in the order in which they run.
const FORMS: [Form; 2] = [
    (
        "capture_full_read",
        "warm_save_full_read",
        FIRST_AGENT,
        ChangeDetection::Ctime,
    ),
    (
        "capture_size_mtime",
        "warm_save_size_mtime",
        SIZE_MTIME_AGENT,
        ChangeDetection::SizeMtime,
    ),
];

/// The names of the steps of the forms, in the order in which they run.
static FORM_STEPS: [&str; 4] = [FORMS[0].0, FORMS[0].1, FORMS[1].0, FORMS[1].1];

/// Runs the capture and the warm save of each form, one form after the other. A failed step gives
/// the records so far, the name of the step and the steps that did not run.
async fn warm_forms(
    context: &PhaseContext,
    tree: &Path,
    steps: Vec<StepRecord>,
) -> Result<Vec<StepRecord>, (Vec<StepRecord>, &'static str, &'static [&'static str])> {
    let storage = &context.storage;
    futures::stream::iter(FORMS.iter().copied().enumerate())
        .map(Ok)
        .try_fold(
            steps,
            |mut steps, (index, (capture_step, save_step, agent, detection))| async move {
                let target: PathBuf = context.work_dir.join(capture_step);
                let (record, captured) =
                    measure(capture_step, storage, capture_tree(tree, &target)).await;
                steps.push(capture_record(record, &captured));
                if captured.is_err() {
                    return Err((steps, capture_step, &FORM_STEPS[2 * index + 1..]));
                }
                let settings = SaveSettings {
                    detection,
                    ..SaveSettings::DEFAULT
                };
                let (record, warm) = measure(save_step, storage, async {
                    context
                        .agent_repository(agent)
                        .save_with(&snapshot_name(WARM_SAVE)?, &target, settings)
                        .await
                })
                .await;
                steps.push(save_record(record, &warm).with_parameters(json!({
                    "change_detection": change_detection_name(detection),
                    "agent": agent,
                })));
                if warm.is_err() {
                    return Err((steps, save_step, &FORM_STEPS[2 * index + 2..]));
                }
                Ok(steps)
            },
        )
        .await
}

/// Captures the tree `from` into the new directory `to` on a blocking thread.
async fn capture_tree(from: &Path, to: &Path) -> anyhow::Result<CopyCounts> {
    let (from, to) = (from.to_path_buf(), to.to_path_buf());
    tokio::task::spawn_blocking(move || trees::copy_tree(&from, &to, Times::Keep)).await?
}

/// Gives the record of a capture with the number of files that are reflinks and copies.
fn capture_record(record: StepRecord, captured: &anyhow::Result<CopyCounts>) -> StepRecord {
    record.with_details(
        captured
            .as_ref()
            .map(|counts| {
                json!({
                    "files_reflinked": counts.reflinked,
                    "files_copied": counts.copied,
                })
            })
            .unwrap_or(Value::Null),
        Box::default(),
    )
}
