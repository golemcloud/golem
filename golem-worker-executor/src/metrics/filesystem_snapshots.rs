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

use super::{SNAPSHOT_AGE_BUCKETS, SNAPSHOT_STORE_BUCKETS};
use lazy_static::lazy_static;
use prometheus::*;
use std::time::Duration;

lazy_static! {
    static ref CAPTURE_SECONDS: HistogramVec = register_histogram_vec!(
        "filesystem_snapshot_capture_seconds",
        "Time that a capture of an agent filesystem, or a check of its initial files on an executor \
         without filesystem snapshots, stops the file calls, by outcome",
        &["outcome"],
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref UPLOAD_SECONDS: HistogramVec = register_histogram_vec!(
        "filesystem_snapshot_upload_seconds",
        "Time of an upload of a filesystem snapshot, with its retries, by kind and outcome",
        &["kind", "outcome"],
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref UPLOADED_BYTES: CounterVec = register_counter_vec!(
        "filesystem_snapshot_uploaded_bytes_total",
        "Bytes of the trees of the uploaded filesystem snapshots, by kind",
        &["kind"]
    )
    .unwrap();
    static ref UPLOADS_IN_PROGRESS: Gauge = register_gauge!(
        "filesystem_snapshot_uploads_in_progress",
        "Number of store attempts of uploads of filesystem snapshots that hold a slot now"
    )
    .unwrap();
    static ref RESTORE_SECONDS: HistogramVec = register_histogram_vec!(
        "filesystem_snapshot_restore_seconds",
        "Time of a restore of a filesystem snapshot, with the waits for a restore slot and the retries, by outcome",
        &["outcome"],
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref DROPPED_CONFIRMATIONS: CounterVec = register_counter_vec!(
        "filesystem_snapshot_dropped_confirmations_total",
        "Uploaded filesystem snapshots that no confirmation record names, by reason",
        &["reason"]
    )
    .unwrap();
    static ref LEAKED_CLEANUPS: CounterVec = register_counter_vec!(
        "filesystem_snapshot_leaked_cleanups_total",
        "Clean-ups of filesystem snapshots that left snapshots in blob storage, by operation",
        &["operation"]
    )
    .unwrap();
    static ref CLEANUPS_PENDING: Gauge = register_gauge!(
        "filesystem_snapshot_cleanups_pending",
        "Number of agents with pending or running clean-up work of filesystem snapshots"
    )
    .unwrap();
    static ref COPY_SECONDS: HistogramVec = register_histogram_vec!(
        "filesystem_snapshot_copy_seconds",
        "Time of a copy of the filesystem snapshots of a fork source, by outcome",
        &["outcome"],
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref STORED_BYTES: IntCounterVec = register_int_counter_vec!(
        "filesystem_snapshot_stored_bytes_total",
        "Bytes of the repository files that the filesystem snapshot store wrote to blob storage, by source",
        &["source"]
    )
    .unwrap();
    static ref REMOVED_BYTES: IntCounterVec = register_int_counter_vec!(
        "filesystem_snapshot_removed_bytes_total",
        "Bytes that the filesystem snapshot store deleted from blob storage, by source",
        &["source"]
    )
    .unwrap();
    static ref RELEASED_BYTES: IntCounter = register_int_counter!(
        "filesystem_snapshot_released_bytes_total",
        "Packed bytes of the deleted filesystem snapshots, which a later prune can remove when no other snapshot uses them"
    )
    .unwrap();
    static ref RESTORED_AGE_SECONDS: Histogram = register_histogram!(
        "filesystem_snapshot_restored_age_seconds",
        "Time from the save of a filesystem snapshot to its successful restore",
        SNAPSHOT_AGE_BUCKETS.to_vec()
    )
    .unwrap();
    static ref SKIPPED: IntCounterVec = register_int_counter_vec!(
        "filesystem_snapshot_skipped_total",
        "Filesystem snapshots that the admission skipped, by kind and reason",
        &["kind", "reason"]
    )
    .unwrap();
    static ref DELETE_SECONDS: HistogramVec = register_histogram_vec!(
        "filesystem_snapshot_delete_seconds",
        "Time of a delete of filesystem snapshots, with its prune, by operation and outcome",
        &["operation", "outcome"],
        SNAPSHOT_STORE_BUCKETS.to_vec()
    )
    .unwrap();
    static ref PRUNE_SECONDS: HistogramVec = register_histogram_vec!(
        "filesystem_snapshot_prune_seconds",
        "Time of a prune of a filesystem snapshot repository after a delete, with its check, by outcome",
        &["outcome"],
        SNAPSHOT_STORE_BUCKETS.to_vec()
    )
    .unwrap();
    static ref PRUNE_SKIPPED: IntCounterVec = register_int_counter_vec!(
        "filesystem_snapshot_prune_skipped_total",
        "Deletes of filesystem snapshots after which no prune ran, by reason",
        &["reason"]
    )
    .unwrap();
    static ref RETENTION_RUNS: IntCounterVec = register_int_counter_vec!(
        "filesystem_snapshot_retention_runs_total",
        "Retention runs after a confirmed or committed filesystem snapshot, by outcome",
        &["outcome"]
    )
    .unwrap();
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
        /// The volume of the agent filesystems has less free space than the pressure target.
        VolumeUnderPressure => "volume_under_pressure",
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
