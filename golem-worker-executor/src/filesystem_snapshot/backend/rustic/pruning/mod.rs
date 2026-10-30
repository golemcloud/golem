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

//! Best-effort prune orchestration after successful named-snapshot deletion.

pub(super) mod claim;
pub(super) mod eligibility;
pub(super) mod records;
#[cfg(test)]
pub(super) mod test_support;
pub(super) mod timing;

use super::blob_backend::BlobBackend;
use super::blob_io::SnapshotFiles;
use super::failure_classification::is_snapshot_missing;
use super::operation::Operation;
use super::repository::{RepositoryKey, open_existing};

use crate::metrics::filesystem_snapshots::{self, PruneSkip, RemovedSource, StoredSource};
use claim::{Claim, belongs_to_ledger, remove_old_claims};
use eligibility::due_prune;
use eligibility::{DueCheck, PrunePolicy};
use futures::future::{self, Either};
use records::{read_ledger, remove_freed, remove_older_ledgers, write_ledger};
use rustic_core::jiff::Span;
use rustic_core::{LimitOption, PruneOptions, PruneStats};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The settings of one prune. Each prune also has fixed limits on unused data and repacking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PruneSettings {
    /// Whether a repack copies the blobs as they are. Without it, a repack decrypts,
    /// decompresses, compresses and encrypts each blob that it keeps.
    pub(super) fast_repack: bool,
    /// How long a pack stays after a prune marks it for deletion. A later prune deletes a marked
    /// pack when this time is over.
    pub(super) keep_delete: Duration,
}

impl Default for PruneSettings {
    fn default() -> Self {
        Self {
            fast_repack: true,
            keep_delete: PRUNE_GRACE,
        }
    }
}

/// A delete prunes after freed bytes reach this share of the repository, rounded down.
pub(super) const PRUNE_THRESHOLD: eligibility::Percent = eligibility::Percent(10);

/// How long a marked pack stays before a later prune deletes it.
///
/// A save lands its snapshot and index writes before the earliest prune index read that could
/// delete its packs. That bound is this grace period, less two storage call deadlines and one
/// claim refresh period, after the save's first slot. A prune stops its data reads and deletes
/// within one refresh period and two deadlines of its newest successful marker write, including
/// when refresh writes fail or its executor stops. Later claim and ledger cleanup touches no data.
/// This assumes clocks differ by less than the skew margin, writes and deletes land within one
/// deadline of their call ending or never, and the host does not suspend. Restores need no bound:
/// they reread snapshots after a pack disappears and can read packs that are only marked.
pub(super) const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

/// The longest storage call deadline: one eighth of the prune grace period, so a save has time
/// for its backup before the earliest index read of a prune.
pub(super) const MAX_STORAGE_CALL_DEADLINE: Duration =
    Duration::from_millis(PRUNE_GRACE.as_millis() as u64 / 8);

/// The number of times that a prune plans again when a snapshot file that it listed is gone at
/// its read.
const PRUNE_ATTEMPTS: usize = 3;

/// Tells whether a later prune removes packs that this prune leaves marked: unused packs, repacked
/// packs, packs that no index lists, and packs of an earlier prune whose grace period is not over.
/// A marked pack is in an index after the prune, so a later prune does not count it as unindexed.
fn leaves_marked_packs(report: &PruneReport) -> bool {
    report.packs_unused > 0
        || report.packs_repacked > 0
        || report.packs_unindexed > 0
        || report.marked_packs_kept > 0
}

/// What a delete did about a prune.
pub(super) enum PruneRan {
    /// The prune ran and wrote its ledger.
    Pruned,
    /// No prune ran, for this reason.
    Skipped(PruneSkip),
}

/// The settings and due threshold of a prune.
pub(super) struct Pruning<'a> {
    pub(super) settings: &'a PruneSettings,
    pub(super) threshold: eligibility::Percent,
}

/// Prunes the repository when a prune is due, and tells whether the prune ran or why it did
/// not. It gives the failure of a prune that did not end. The records of freed bytes stay
/// until a prune succeeds, so a later prune counts them again. It lists the packs only when
/// their size can make a prune due. The claim protocol makes one try of each blob call,
/// because a try that lost its answer can still land, and the protocol counts each such
/// landing.
///
/// A due prune runs only after the delete takes a claim of its ledger, and only when a second
/// read of the ledger after the claim finds the time of the last prune of the first read. When
/// it finds another time, the claim is released and the prune gives no error. So a delete
/// prunes only with a claim in the claim directory of the ledger that the second read gives.
/// After an error before the prune ran, the claim is released, so the next delete prunes
/// again. A prune that found a snapshot file gone at each attempt changed nothing, so it counts
/// as an error before the prune ran. After a prune that started, the claim stays on each
/// outcome, also when the prune or its ledger write fails, or when the lease skips its ledger
/// write. A prune that succeeds deletes each claim of its ledger.
pub(super) async fn when_due(
    operation: &Operation<'_>,
    pruning: &Pruning<'_>,
    token: &CancellationToken,
) -> anyhow::Result<PruneRan> {
    let files = operation.files(token).once();
    let policy = PrunePolicy {
        grace: pruning.settings.keep_delete,
        deadline: operation.deadline(),
        threshold: pruning.threshold,
    };
    let due = match due_prune(&files, &**operation.clock, &policy).await? {
        DueCheck::Due(due) => due,
        DueCheck::NotDue => return Ok(PruneRan::Skipped(PruneSkip::NotDue)),
        DueCheck::Held => return Ok(PruneRan::Skipped(PruneSkip::ClaimedElsewhere)),
    };
    // The token of the tracker comes before the check of the cancel, so either the delete sees
    // a shut down and makes no more storage calls, or `shut_down` waits for the claim and for
    // each call that the claim makes.
    let tracked = operation.registration();
    if operation.root().is_cancelled() {
        anyhow::bail!("the filesystem snapshot store is shut down");
    }
    let spawner = operation.spawner()?;
    let Some(claim) = Claim::take(
        &files,
        due.claim,
        policy.grace,
        policy.deadline,
        Arc::clone(operation.clock),
        spawner,
        tracked,
    )
    .await?
    else {
        return Ok(PruneRan::Skipped(PruneSkip::ClaimedElsewhere));
    };
    // Before the prune starts, an error releases the claim, so the next delete prunes again.
    // When the second read of the ledger finds another time of the last prune than the first
    // read, the claim is released too, and the prune gives no error. A prune that started can
    // have marked packs, so its claim stays.
    let backend = match prepare_prune(operation, &files, &claim).await {
        Ok(Some(backend)) => backend,
        other => {
            claim.release().await;
            return other.map(|_| PruneRan::Skipped(PruneSkip::ClaimedElsewhere));
        }
    };
    // No attempt of a prune that found a snapshot file gone changed the repository, so no
    // prune ran, and the claim goes.
    let pruned = match run_prune(operation, pruning, backend, &files, &claim).await {
        Ok(None) => {
            claim.release().await;
            anyhow::bail!(
                "a concurrent delete removed a snapshot file at each attempt of the prune"
            );
        }
        other => other.map(|report| report.flatten().unwrap_or_default()),
    };
    // The final marker holds the claim for the hold from the end of this prune, also when the
    // prune or its ledger write fails. It goes through the files of the claim, which no cancel
    // ends, so a prune that a shut down stopped also gets it.
    claim.finish().await;
    let report = pruned?;
    filesystem_snapshots::record_removed_bytes(RemovedSource::Prune, report.bytes_removed);
    let ended = operation.clock.now();
    // The lease fences the ledger write as it fences the calls of rustic. A write that would
    // start after the lease ran out is not sent, and a write that starts before is bounded by
    // the time left, so the entry never lands after another delete can take the claim over.
    // A prune whose ledger write the lease skipped keeps its claim and runs no cleanup.
    write_ledger(&claim.leased(&files), ended, leaves_marked_packs(&report)).await?;
    remove_older_ledgers(&files, ended).await;
    remove_freed(&files, &due.records).await;
    remove_old_claims(&files, ended).await;
    Ok(PruneRan::Pruned)
}

/// Checks the ledger again and builds the backend of the prune. It gives `None` when the second
/// read finds another time of the last prune than the first read. So a prune runs only with a
/// claim in the claim directory of the ledger that the second read gives.
async fn prepare_prune(
    operation: &Operation<'_>,
    files: &SnapshotFiles,
    claim: &Claim,
) -> anyhow::Result<Option<Arc<BlobBackend>>> {
    // A prune writes its ledger before it deletes the claims, so a delete that claims in a
    // directory that such a prune removed sees the new ledger here.
    let again = read_ledger(files, &**operation.clock).await?;
    if !belongs_to_ledger(claim.directory(), &again) {
        return Ok(None);
    }
    Ok(Some(Arc::new(
        operation
            .backend(claim.leased(files))?
            .storing(StoredSource::Prune),
    )))
}

/// Runs the prune with a new marker of the claim at each refresh period, and gives its report.
/// A scope without a repository gives `Some(None)`. It gives `None` when each attempt found a
/// snapshot file gone.
async fn run_prune(
    operation: &Operation<'_>,
    pruning: &Pruning<'_>,
    backend: Arc<BlobBackend>,
    files: &SnapshotFiles,
    claim: &Claim,
) -> anyhow::Result<Option<Option<PruneReport>>> {
    let key = operation.key.clone();
    let settings = *pruning.settings;
    let low_priority = operation.low_priority;
    let start = claim.start();
    // The plan of a prune reads each snapshot file before the prune changes the repository.
    // A forget of another delete can remove a listed file before its read, so the prune
    // plans again from a new listing.
    let pruning = operation.blocking(move || {
        low_priority.run("fs-snap-prune", move || {
            // A delete that dropped before this point released the claim, so no prune runs.
            if !start.start() {
                return Ok(None);
            }
            Ok(
                std::iter::repeat_with(|| prune(backend.clone(), &key, &settings))
                    .take(PRUNE_ATTEMPTS)
                    .find(|attempt| {
                        !attempt
                            .as_ref()
                            .is_err_and(|error| is_snapshot_missing(&**error))
                    }),
            )
        })
    });
    // The claim gets a new marker while the prune runs, so a prune slower than one refresh
    // period keeps its claim. The markers stop when the prune ends or the operation is
    // cancelled. The tracker counts the whole step, so no timer of it runs after a shut down.
    let refreshing = claim.keep_fresh(files);
    let attempts = operation
        .track(async {
            match future::select(pin!(pruning), pin!(refreshing)).await {
                Either::Left((pruned, _)) => pruned,
                Either::Right(((), pruning)) => pruning.await,
            }
        })
        .await?;
    attempts.transpose()
}
/// The numbers of the plan of a prune that tell whether it leaves marked packs, and the bytes of
/// the packs that it deletes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PruneReport {
    /// The packs that no snapshot uses.
    pub(super) packs_unused: u64,
    pub(super) packs_repacked: u64,
    /// The packs that an earlier prune marked and that stay marked.
    pub(super) marked_packs_kept: u64,
    /// The packs that no index lists. The prune marks each of them.
    pub(super) packs_unindexed: u64,
    /// The bytes of the marked packs whose time to stay is over. The prune deletes them.
    pub(super) bytes_removed: u64,
}

/// Prunes the repository. It deletes the packs that an earlier prune marked and whose time to stay
/// is over. It marks the packs that no snapshot uses. It repacks the packs that hold both used and
/// unused blobs, within [`MAX_UNUSED`] and [`MAX_REPACK`]. The result is `None` when the scope has
/// no repository.
pub(super) fn prune(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    settings: &PruneSettings,
) -> anyhow::Result<Option<PruneReport>> {
    let Some(repository) = open_existing(backend, key)? else {
        return Ok(None);
    };
    let options = prune_options(settings)?;
    let plan = repository.prune_plan(&options)?;
    let report = prune_report(&plan.stats);
    repository.prune(&options, plan)?;
    Ok(Some(report))
}

/// The target of unused data after a prune. The prune repacks the data packs that hold both used
/// and unused blobs, within [`MAX_REPACK`]. It does this while the unused data that would stay is
/// 5% or more of the size of the repository after the prune.
const MAX_UNUSED: LimitOption = LimitOption::Percentage(5);

/// The limit of the repack of one prune. The prune repacks a pack only while the used data that it
/// copies stays below 10% of the size of the repository before the prune.
const MAX_REPACK: LimitOption = LimitOption::Percentage(10);

/// The options of a prune.
fn prune_options(settings: &PruneSettings) -> anyhow::Result<PruneOptions> {
    Ok(PruneOptions::default()
        .max_unused(MAX_UNUSED)
        .max_repack(MAX_REPACK)
        .fast_repack(settings.fast_repack)
        .keep_delete(Span::try_from(settings.keep_delete)?))
}

/// Gives the numbers of the plan of a prune that tell whether it leaves marked packs, and the bytes
/// of the packs that it deletes.
fn prune_report(stats: &PruneStats) -> PruneReport {
    PruneReport {
        packs_unused: stats.packs.unused,
        packs_repacked: stats.packs.repack,
        marked_packs_kept: stats.packs_to_delete.keep,
        packs_unindexed: stats.packs_unref,
        bytes_removed: stats.size_to_delete.remove,
    }
}

#[cfg(test)]
mod tests;
