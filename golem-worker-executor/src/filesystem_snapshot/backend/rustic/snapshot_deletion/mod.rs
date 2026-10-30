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

use super::call_execution::{Ended, Ran};
use super::execution::{BackendRun, Execution};
use super::failure_classification::{ended_by, storage_end};
use super::pruning::PruneRan;
use super::pruning::records::record_freed;
use super::repository::open_existing;
use super::snapshot_catalog::scope_snapshots;
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use crate::filesystem_snapshot::errors::CallError;
use crate::metrics::filesystem_snapshots::{self, PruneOutcome};
use rustic_core::repofile::{SnapshotFile, SnapshotId};
use std::collections::HashSet;
use std::time::Instant;
use tracing::warn;

/// Gives the packed bytes that the save of the snapshot added to the repository.
fn added_packed_bytes(snapshot: &SnapshotFile) -> u64 {
    snapshot
        .summary
        .as_ref()
        .map_or(0, |summary| summary.data_added_packed)
}

/// One run of a delete: the record of the freed bytes, the forget of the snapshot files, and
/// then the prune when one is due. The run answers when the forget succeeded; a prune that
/// fails gives a warning and a count, and the next delete prunes.
pub(super) async fn delete_run(
    execution: &Execution<'_>,
    repository_key: &super::repository::RepositoryKey,
    agent: &AgentSnapshots,
    names: &HashSet<Box<str>>,
    pool: super::cpu_priority::LowPriority,
    pruning: super::pruning::Pruning<'_>,
    #[cfg(test)] failed_prunes: &std::sync::Mutex<Vec<anyhow::Error>>,
) -> Ran<Result<(), CallError>> {
    let BackendRun {
        token,
        guard: _guard,
        backend,
    } = match execution.begin_run(agent) {
        Ok(run) => run,
        Err(ended) => return Ran::Ended(ended),
    };
    let key = repository_key.clone();
    let names = names.clone();
    let found = execution
        .blocking(move || {
            let Some(repository) = open_existing(backend.clone(), &key)? else {
                return Ok(None);
            };
            // One listing finds every snapshot of the batch.
            let named = scope_snapshots(&repository, &backend)?
                .readable
                .into_iter()
                .filter(|snapshot| names.contains(snapshot.label.as_str()))
                .collect::<Box<[_]>>();
            let ids = named
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Box<[SnapshotId]>>();
            let freed = named.iter().map(added_packed_bytes).sum::<u64>();
            Ok(Some((repository, ids, freed)))
        })
        .await;
    let (repository, ids, freed) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return Ran::Answered(Ok(())),
        Err(error) => return Ran::Ended(ended_by(error)),
    };
    // The record comes before the forget, so a stop between the two cannot lose the bytes. A
    // forget that then fails ends the run, and the record stays, which only brings a prune
    // earlier.
    let files = execution.files(agent, &token);
    if freed > 0 {
        let snapshots = ids
            .iter()
            .map(|id| id.to_hex().to_string().into_boxed_str())
            .collect::<Box<[_]>>();
        if let Err(error) = record_freed(&files, freed, &snapshots).await {
            return Ran::Ended(Ended::new(storage_end(&error), error));
        }
    }
    // The forget deletes the snapshot files with rayon, so it runs in a pool of its own. The
    // blocking thread starts the threads of that pool, so they have its normal priority.
    if let Err(error) = execution
        .blocking(move || {
            pool.in_own_pool("fs-snap-delete", move || {
                repository.delete_snapshots(&ids)?;
                Ok(())
            })
        })
        .await
    {
        return Ran::Ended(ended_by(error));
    }
    filesystem_snapshots::record_released_bytes(freed);
    let started = Instant::now();
    match super::pruning::when_due(execution, &pruning, agent, &token).await {
        Ok(PruneRan::Pruned) => {
            filesystem_snapshots::record_prune(PruneOutcome::Pruned, started.elapsed());
        }
        Ok(PruneRan::Skipped(reason)) => filesystem_snapshots::record_prune_skipped(reason),
        Err(error) => {
            warn!(
                snapshots = ?agent,
                error = %format!("{error:#}"),
                "A prune of the filesystem snapshots of an agent failed; the next delete prunes again"
            );
            filesystem_snapshots::record_prune(PruneOutcome::Failed, started.elapsed());
            #[cfg(test)]
            failed_prunes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(error);
        }
    }
    Ran::Answered(Ok(()))
}
#[cfg(test)]
mod tests;
