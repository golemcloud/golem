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

use super::{
    AgentSnapshots, Arc, BlobBackend, CallError, CancellationToken, Claim, DueCheck, Either, Ended,
    HashSet, Instant, OperationWork, PruneOutcome, PrunePolicy, PruneReport, PruneSkip, Ran,
    RemovedSource, RunEnd, RusticSnapshotStore, SnapshotFile, SnapshotFiles, SnapshotId,
    StoredSource, belongs_to_ledger, due_prune, ended_by, filesystem_snapshots, future,
    is_snapshot_missing, open_existing, pin, prune, read_ledger, record_freed, remove_freed,
    remove_old_claims, remove_older_ledgers, scope_snapshots, storage_end, warn, write_ledger,
};

/// The number of times that a prune plans again when a snapshot file that it listed is gone at
/// its read.
const PRUNE_ATTEMPTS: usize = 3;

/// Tells whether a later prune removes packs that this prune leaves marked: unused packs, repacked
/// packs, packs that no index lists, and packs of an earlier prune whose grace period is not over.
/// A marked pack is in an index after the prune, so a later prune does not count it as unindexed.
pub(super) fn leaves_marked_packs(report: &PruneReport) -> bool {
    report.packs_unused > 0
        || report.packs_repacked > 0
        || report.packs_unindexed > 0
        || report.marked_packs_kept > 0
}

/// What a delete did about a prune.
enum PruneRan {
    /// The prune ran and wrote its ledger.
    Pruned,
    /// No prune ran, for this reason.
    Skipped(PruneSkip),
}

/// Gives the packed bytes that the save of the snapshot added to the repository.
fn added_packed_bytes(snapshot: &SnapshotFile) -> u64 {
    snapshot
        .summary
        .as_ref()
        .map_or(0, |summary| summary.data_added_packed)
}

impl RusticSnapshotStore {
    /// Gives the values of the prune decision from the policy of the store.
    fn prune_policy(&self) -> PrunePolicy {
        PrunePolicy {
            grace: self.policy.prune.keep_delete,
            deadline: self.policy.deadline,
            threshold: self.policy.prune_threshold,
        }
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
    async fn prune_when_due(
        &self,
        work: &OperationWork,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> anyhow::Result<PruneRan> {
        let files = self.files(work, scope, token).once();
        let policy = self.prune_policy();
        let due = match due_prune(&files, &*self.clock, &policy).await? {
            DueCheck::Due(due) => due,
            DueCheck::NotDue => return Ok(PruneRan::Skipped(PruneSkip::NotDue)),
            DueCheck::Held => return Ok(PruneRan::Skipped(PruneSkip::ClaimedElsewhere)),
        };
        // The token of the tracker comes before the check of the cancel, so either the delete sees
        // a shut down and makes no more storage calls, or `shut_down` waits for the claim and for
        // each call that the claim makes.
        let tracked = self.tracker.token();
        if self.root.is_cancelled() {
            anyhow::bail!("the filesystem snapshot store is shut down");
        }
        let spawner = self.spawner(work)?;
        let Some(claim) = Claim::take(
            &files,
            due.claim,
            &policy,
            self.clock.clone(),
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
        let backend = match self.prepare_prune(&files, &claim).await {
            Ok(Some(backend)) => backend,
            other => {
                claim.release().await;
                return other.map(|_| PruneRan::Skipped(PruneSkip::ClaimedElsewhere));
            }
        };
        // No attempt of a prune that found a snapshot file gone changed the repository, so no
        // prune ran, and the claim goes.
        let pruned = match self.run_prune(work, backend, &files, &claim).await {
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
        let ended = self.clock.now();
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
        &self,
        files: &SnapshotFiles,
        claim: &Claim,
    ) -> anyhow::Result<Option<Arc<BlobBackend>>> {
        // A prune writes its ledger before it deletes the claims, so a delete that claims in a
        // directory that such a prune removed sees the new ledger here.
        let again = read_ledger(files, &*self.clock).await?;
        if !belongs_to_ledger(claim.directory(), &again) {
            return Ok(None);
        }
        Ok(Some(Arc::new(
            self.backend(claim.leased(files))?
                .storing(StoredSource::Prune),
        )))
    }

    /// Runs the prune with a new marker of the claim at each refresh period, and gives its report.
    /// A scope without a repository gives `Some(None)`. It gives `None` when each attempt found a
    /// snapshot file gone.
    async fn run_prune(
        &self,
        work: &OperationWork,
        backend: Arc<BlobBackend>,
        files: &SnapshotFiles,
        claim: &Claim,
    ) -> anyhow::Result<Option<Option<PruneReport>>> {
        let key = self.key.clone();
        let settings = self.policy.prune;
        let low_priority = self.low_priority;
        let start = claim.start();
        // The plan of a prune reads each snapshot file before the prune changes the repository.
        // A forget of another delete can remove a listed file before its read, so the prune
        // plans again from a new listing.
        let pruning = self.blocking(work, move || {
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
        let attempts = self
            .tracker
            .track_future(async {
                match future::select(pin!(pruning), pin!(refreshing)).await {
                    Either::Left((pruned, _)) => pruned,
                    Either::Right(((), pruning)) => pruning.await,
                }
            })
            .await?;
        attempts.transpose()
    }

    /// One run of a delete: the record of the freed bytes, the forget of the snapshot files, and
    /// then the prune when one is due. The run answers when the forget succeeded; a prune that
    /// fails gives a warning and a count, and the next delete prunes.
    pub(super) async fn delete_run(
        &self,
        work: &OperationWork,
        scope: &AgentSnapshots,
        names: &HashSet<Box<str>>,
    ) -> Ran<Result<(), CallError>> {
        let (token, _guard) = self.run_token();
        let backend = match self.scope_backend(work, scope, &token) {
            Ok(backend) => Arc::new(backend),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let key = self.key.clone();
        let names = names.clone();
        let found = self
            .blocking(work, move || {
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
        let files = self.files(work, scope, &token);
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
        let pool = self.low_priority;
        if let Err(error) = self
            .blocking(work, move || {
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
        match self.prune_when_due(work, scope, &token).await {
            Ok(PruneRan::Pruned) => {
                filesystem_snapshots::record_prune(PruneOutcome::Pruned, started.elapsed());
            }
            Ok(PruneRan::Skipped(reason)) => filesystem_snapshots::record_prune_skipped(reason),
            Err(error) => {
                warn!(
                    snapshots = ?scope,
                    error = %format!("{error:#}"),
                    "A prune of the filesystem snapshots of an agent failed; the next delete prunes again"
                );
                filesystem_snapshots::record_prune(PruneOutcome::Failed, started.elapsed());
                #[cfg(test)]
                self.failed_prunes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(error);
            }
        }
        Ran::Answered(Ok(()))
    }
}
