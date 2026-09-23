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

//! The phase of the scopes scenario: the copy of the repository of an agent for a fork, and the
//! deletion of the repository of an agent.

use super::agents::{FIRST_AGENT, agent_blobs, copy_agent, delete_agent};
use super::measure::measure;
use super::report::{Outcome, TreeFacts};
use super::{PhaseContext, PhaseOutcome, failed};
use serde_json::{Value, json};

/// The agent that gets the copy of the repository of the first agent.
const FORK: &str = "fork";

/// The scopes phase: the repository of the save phase goes on the server to the fork agent, as
/// `copy_scope` copies a scope, and then the repository of the fork agent is deleted, as
/// `delete_scope` deletes a scope.
///
/// A listing after the copy checks that the fork has each blob of the source with its size, and a
/// listing after the delete checks that the fork has no blob. Each listing is outside the measured
/// steps.
pub(super) async fn scopes(context: &PhaseContext) -> PhaseOutcome {
    let storage = &context.storage;
    let namespace = context.scope().0;
    let facts = TreeFacts {
        name: context.selection.tree.name,
        ..TreeFacts::default()
    };

    let (record, copied) = measure(
        "copy_scope",
        storage,
        copy_agent(storage.as_ref(), &namespace, FIRST_AGENT, FORK),
    )
    .await;
    let (source, fork) = (
        agent_blobs(storage.as_ref(), &namespace, FIRST_AGENT).await,
        agent_blobs(storage.as_ref(), &namespace, FORK).await,
    );
    let same =
        matches!((&source, &fork), (Ok(source), Ok(fork)) if !source.is_empty() && source == fork);
    let details = match copied.as_ref() {
        Ok(Value::Object(details)) => {
            let mut details = details.clone();
            details.insert("same_as_source".to_string(), json!(same));
            Value::Object(details)
        }
        _ => Value::Null,
    };
    let mut steps = vec![record.with_details(details, Box::default())];
    if copied.is_err() {
        return failed(facts, steps, "copy_scope", &["delete_scope"]);
    }

    let (record, deleted) = measure(
        "delete_scope",
        storage,
        delete_agent(storage.as_ref(), &namespace, FORK),
    )
    .await;
    let left = agent_blobs(storage.as_ref(), &namespace, FORK)
        .await
        .map(|blobs| blobs.len());
    steps.push(record.with_details(
        json!({ "deleted": deleted.as_ref().ok(), "blobs_left": left.as_ref().ok() }),
        Box::default(),
    ));
    if deleted.is_err() {
        return failed(facts, steps, "delete_scope", &[]);
    }
    let outcome = match (same, left) {
        (true, Ok(0)) => Outcome::Ok,
        (false, _) => Outcome::Failed {
            reason: "the copy of the repository differs from the repository".into(),
        },
        (true, _) => Outcome::Failed {
            reason: "the deleted repository still has blobs".into(),
        },
    };
    PhaseOutcome {
        tree_facts: facts,
        steps,
        outcome,
    }
}
