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

use golem_common::metrics::catalog;
use lazy_static::lazy_static;
use prometheus::*;
use std::time::Duration;

lazy_static! {
    static ref CAPTURE_SECONDS: HistogramVec =
        catalog::FILESYSTEM_SNAPSHOT_CAPTURE_SECONDS.register();
    static ref UPLOAD_SECONDS: HistogramVec =
        catalog::FILESYSTEM_SNAPSHOT_UPLOAD_SECONDS.register();
    static ref UPLOADED_BYTES: CounterVec =
        catalog::FILESYSTEM_SNAPSHOT_UPLOADED_BYTES_TOTAL.register();
    static ref UPLOADS_IN_PROGRESS: Gauge =
        catalog::FILESYSTEM_SNAPSHOT_UPLOADS_IN_PROGRESS.register();
    static ref RESTORE_SECONDS: HistogramVec =
        catalog::FILESYSTEM_SNAPSHOT_RESTORE_SECONDS.register();
    static ref DROPPED_CONFIRMATIONS: CounterVec =
        catalog::FILESYSTEM_SNAPSHOT_DROPPED_CONFIRMATIONS_TOTAL.register();
    static ref LEAKED_CLEANUPS: CounterVec =
        catalog::FILESYSTEM_SNAPSHOT_LEAKED_CLEANUPS_TOTAL.register();
    static ref CLEANUPS_PENDING: Gauge = catalog::FILESYSTEM_SNAPSHOT_CLEANUPS_PENDING.register();
    static ref COPY_SECONDS: HistogramVec = catalog::FILESYSTEM_SNAPSHOT_COPY_SECONDS.register();
    static ref STORED_BYTES: IntCounterVec =
        catalog::FILESYSTEM_SNAPSHOT_STORED_BYTES_TOTAL.register();
    static ref REMOVED_BYTES: IntCounterVec =
        catalog::FILESYSTEM_SNAPSHOT_REMOVED_BYTES_TOTAL.register();
    static ref RELEASED_BYTES: IntCounter =
        catalog::FILESYSTEM_SNAPSHOT_RELEASED_BYTES_TOTAL.register();
    static ref RESTORED_AGE_SECONDS: Histogram =
        catalog::FILESYSTEM_SNAPSHOT_RESTORED_AGE_SECONDS.register();
    static ref SKIPPED: IntCounterVec = catalog::FILESYSTEM_SNAPSHOT_SKIPPED_TOTAL.register();
    static ref DELETE_SECONDS: HistogramVec =
        catalog::FILESYSTEM_SNAPSHOT_DELETE_SECONDS.register();
    static ref PRUNE_SECONDS: HistogramVec = catalog::FILESYSTEM_SNAPSHOT_PRUNE_SECONDS.register();
    static ref PRUNE_SKIPPED: IntCounterVec =
        catalog::FILESYSTEM_SNAPSHOT_PRUNE_SKIPPED_TOTAL.register();
    static ref RETENTION_RUNS: IntCounterVec =
        catalog::FILESYSTEM_SNAPSHOT_RETENTION_RUNS_TOTAL.register();
}

label_values! {
    /// The work that wrote repository files, as the `source` label.
    StoredSource {
        /// A save of a snapshot: its packs, its index files and the config of a new repository.
        Save => "save",
        /// A prune: the packs that it repacks and the index files that it writes again.
        Prune => "prune",
        /// A copy of the repository of a fork source, on the side of the storage.
        Copy => "copy",
    }
}

label_values! {
    /// The work that deleted bytes, as the `source` label.
    RemovedSource {
        /// A prune: the packs whose time to stay after their mark is over.
        Prune => "prune",
        /// A delete of all snapshots of an agent: the whole repository.
        DeleteAll => "delete_all",
    }
}

label_values! {
    /// Why an admission skipped a snapshot, as the `reason` label.
    SkipReason {
        /// An upload of the agent runs now.
        UploadInFlight => "upload_in_flight",
        /// The disk of the agent filesystems has less free space than the pressure target.
        DiskUnderPressure => "disk_under_pressure",
        /// A delete of all snapshots of the agent is queued or runs.
        DeletingAllSnapshots => "deleting_all_snapshots",
    }
}

label_values! {
    /// The kind of a delete, as the `operation` label.
    DeleteOperation {
        /// A delete of named snapshots: a retention, a superseded snapshot or a queued clean-up.
        Delete => "delete",
        /// A delete of all snapshots of an agent.
        DeleteAll => "delete_all",
    }
}

label_values! {
    /// How a delete ended, as the `outcome` label.
    DeleteOutcome {
        /// The store deleted the snapshots.
        Deleted => "deleted",
        /// A stop or the shutdown ended the wait for the delete.
        Stopped => "stopped",
        /// The delete failed, and the snapshots stay in blob storage.
        Leaked => "leaked",
    }
}

label_values! {
    /// How a prune that a delete started ended, as the `outcome` label.
    PruneOutcome {
        /// The prune ran and wrote its ledger.
        Pruned => "pruned",
        /// The check or the prune failed. The next delete prunes again.
        Failed => "failed",
    }
}

label_values! {
    /// Why a delete ran no prune, as the `reason` label.
    PruneSkip {
        /// The repository holds too few freed bytes, or the last prune is too recent.
        NotDue => "not_due",
        /// Another delete holds the claim of the prune, or pruned since the check.
        ClaimedElsewhere => "claimed_elsewhere",
    }
}

label_values! {
    /// How a retention run ended, as the `outcome` label.
    RetentionOutcome {
        /// The listing of the snapshots failed.
        ListFailed => "list_failed",
        /// No snapshot was old enough to delete.
        NothingToDelete => "nothing_to_delete",
        /// The store deleted the old snapshots.
        Deleted => "deleted",
        /// A stop ended the retention.
        Stopped => "stopped",
        /// The delete failed, and the old snapshots stay until the next retention.
        Leaked => "leaked",
    }
}

/// Exports each label value of the metrics of the store at zero, so that a dashboard or an
/// alert reads 0 instead of no data before the first event. `kinds` are the labels of the
/// snapshot kinds. Called when the executor starts a service that keeps filesystem
/// snapshots, so an executor without them exports none of these metrics.
pub fn record_filesystem_snapshots_started(kinds: &[&'static str]) {
    for source in StoredSource::ALL {
        STORED_BYTES.with_label_values(&[source.label()]);
    }
    for source in RemovedSource::ALL {
        REMOVED_BYTES.with_label_values(&[source.label()]);
    }
    lazy_static::initialize(&RELEASED_BYTES);
    lazy_static::initialize(&RESTORED_AGE_SECONDS);
    for kind in kinds {
        for reason in SkipReason::ALL {
            SKIPPED.with_label_values(&[kind, reason.label()]);
        }
    }
    for operation in DeleteOperation::ALL {
        for outcome in DeleteOutcome::ALL {
            DELETE_SECONDS.with_label_values(&[operation.label(), outcome.label()]);
        }
    }
    for outcome in PruneOutcome::ALL {
        PRUNE_SECONDS.with_label_values(&[outcome.label()]);
    }
    for reason in PruneSkip::ALL {
        PRUNE_SKIPPED.with_label_values(&[reason.label()]);
    }
    for outcome in RetentionOutcome::ALL {
        RETENTION_RUNS.with_label_values(&[outcome.label()]);
    }
}

pub fn record_stored_bytes(source: StoredSource, bytes: u64) {
    STORED_BYTES
        .with_label_values(&[source.label()])
        .inc_by(bytes);
}

pub fn record_removed_bytes(source: RemovedSource, bytes: u64) {
    REMOVED_BYTES
        .with_label_values(&[source.label()])
        .inc_by(bytes);
}

pub fn record_released_bytes(bytes: u64) {
    RELEASED_BYTES.inc_by(bytes);
}

pub fn record_restored_age(age: Duration) {
    RESTORED_AGE_SECONDS.observe(age.as_secs_f64());
}

pub fn record_skipped(kind: &'static str, reason: SkipReason) {
    SKIPPED.with_label_values(&[kind, reason.label()]).inc();
}

pub fn record_delete(operation: DeleteOperation, outcome: DeleteOutcome, elapsed: Duration) {
    DELETE_SECONDS
        .with_label_values(&[operation.label(), outcome.label()])
        .observe(elapsed.as_secs_f64());
}

pub fn record_prune(outcome: PruneOutcome, elapsed: Duration) {
    PRUNE_SECONDS
        .with_label_values(&[outcome.label()])
        .observe(elapsed.as_secs_f64());
}

pub fn record_prune_skipped(reason: PruneSkip) {
    PRUNE_SKIPPED.with_label_values(&[reason.label()]).inc();
}

pub fn record_retention_run(outcome: RetentionOutcome) {
    RETENTION_RUNS.with_label_values(&[outcome.label()]).inc();
}

pub fn set_cleanups_pending(agents: usize) {
    CLEANUPS_PENDING.set(agents as f64);
}

pub fn record_copy(outcome: &'static str, elapsed: Duration) {
    COPY_SECONDS
        .with_label_values(&[outcome])
        .observe(elapsed.as_secs_f64());
}

pub fn record_capture(outcome: &'static str, elapsed: Duration) {
    CAPTURE_SECONDS
        .with_label_values(&[outcome])
        .observe(elapsed.as_secs_f64());
}

pub fn record_upload(kind: &'static str, outcome: &'static str, elapsed: Duration) {
    UPLOAD_SECONDS
        .with_label_values(&[kind, outcome])
        .observe(elapsed.as_secs_f64());
}

pub fn record_uploaded_bytes(kind: &'static str, bytes: u64) {
    UPLOADED_BYTES
        .with_label_values(&[kind])
        .inc_by(bytes as f64);
}

pub fn inc_uploads_in_progress() {
    UPLOADS_IN_PROGRESS.inc();
}

pub fn dec_uploads_in_progress() {
    UPLOADS_IN_PROGRESS.dec();
}

pub fn record_restore(outcome: &'static str, elapsed: Duration) {
    RESTORE_SECONDS
        .with_label_values(&[outcome])
        .observe(elapsed.as_secs_f64());
}

pub fn record_dropped_confirmation(reason: &'static str) {
    DROPPED_CONFIRMATIONS.with_label_values(&[reason]).inc();
}

pub fn record_leaked_cleanup(operation: &'static str) {
    LEAKED_CLEANUPS.with_label_values(&[operation]).inc();
}
