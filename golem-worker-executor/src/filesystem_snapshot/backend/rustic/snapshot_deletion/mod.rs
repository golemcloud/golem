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

use super::failure_classification::{ended_by, storage_end};
use super::operation::Operation;
use super::pruning::records::record_freed;
use super::pruning::{PruneRan, PruneSettings};
use super::run_outcome::{Ended, Ran, RunResult};
use super::snapshot_catalog::repository_snapshots;
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

/// Records freed bytes before forgetting the snapshots, then prunes when due. A successful
/// forget answers successfully even if the prune fails. The next delete can retry that prune.
pub(super) async fn delete_run(
    operation: &Operation<'_>,
    settings: PruneSettings,
    names: &HashSet<Box<str>>,
) -> RunResult<Result<(), CallError>> {
    let mut run = operation.begin_run()?;
    let backend = run.take_backend();
    let names = names.clone();
    let found = operation
        .read_existing(backend, move |repository, backend| {
            let Some(repository) = repository else {
                return Ok(None);
            };
            // One listing finds every snapshot of the batch.
            let named = repository_snapshots(&repository, backend)?
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
        Ok(None) => return Ok(Ran::Answered(Ok(()))),
        Err(error) => return Err(ended_by(error)),
    };
    // The record comes before the forget, so a stop between the two cannot lose the bytes. A
    // forget that then fails ends the run, and the record stays, which only brings a prune
    // earlier.
    let files = operation.files(run.token());
    if freed > 0 {
        let snapshots = ids
            .iter()
            .map(|id| id.to_hex().to_string().into_boxed_str())
            .collect::<Box<[_]>>();
        if let Err(error) = record_freed(&files, freed, &snapshots).await {
            return Err(Ended::new(storage_end(&error), error));
        }
    }
    // The forget deletes the snapshot files with rayon, so it runs in a pool of its own. The
    // blocking thread starts the threads of that pool, so they have its normal priority.
    let pool = operation.low_priority();
    if let Err(error) = operation
        .blocking(move || {
            pool.in_own_pool("fs-snap-delete", move || {
                repository.delete_snapshots(&ids)?;
                Ok(())
            })
        })
        .await
    {
        return Err(ended_by(error));
    }
    filesystem_snapshots::record_released_bytes(freed);
    let started = Instant::now();
    match super::pruning::when_due(operation, &settings, run.token()).await {
        Ok(PruneRan::Pruned) => {
            filesystem_snapshots::record_prune(PruneOutcome::Pruned, started.elapsed());
        }
        Ok(PruneRan::Skipped(reason)) => filesystem_snapshots::record_prune_skipped(reason),
        Err(error) => {
            warn!(
                snapshots = ?operation.agent(),
                error = %format!("{error:#}"),
                "A prune of the filesystem snapshots of an agent failed; the next delete prunes again"
            );
            filesystem_snapshots::record_prune(PruneOutcome::Failed, started.elapsed());
            #[cfg(test)]
            operation.record_prune_failure(error);
        }
    }
    Ok(Ran::Answered(Ok(())))
}
#[cfg(test)]
mod tests;
