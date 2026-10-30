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

//! Configuration mapping and the store's existing safety bounds and rustic options.

use super::pruning::eligibility::Percent;
use super::pruning::timing::refresh_period;
use super::publication::{PublishBound, backup_end, index_read_bound};
use super::repository::backup_options;
use crate::filesystem_snapshot::ChangeDetection;
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use golem_common::model::RetryConfig;
use rustic_core::repofile::SnapshotId;
use rustic_core::{BackupOptions, DevIdOption, LocalSourceSaveOptions, RestoreOptions};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

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

/// The share of the size of the repository that deleted snapshots must free before a delete prunes
/// the scope. The threshold is 10% of the size, rounded down to a whole byte, so about 10%.
const PRUNE_THRESHOLD: Percent = Percent(10);

/// How long a pack that a prune marks stays before a later prune deletes it.
///
/// A save publishes its snapshot, and writes its index files, only before the earliest time at
/// which a prune can read the index that could delete a pack of the save. That time is this grace
/// period, less two storage call deadlines, less one refresh period of a claim, after the first
/// slot of the save. A prune stops its reads and deletes of snapshot data at most one refresh
/// period plus two deadlines after the start of its newest marker write that succeeded, so this
/// holds also when its refresh writes fail and when its executor stops. The clean-ups of its ledger
/// and its claim can run later, and they touch no snapshot data. So no snapshot resolves to a pack
/// that a prune deleted, and the bound needs only that clocks differ by less than the skew margin,
/// that a write or a delete lands within one deadline of the end of its call or never, and that the
/// host does not suspend. A restore needs no bound: it answers a pack that a prune removed during
/// it by reading the snapshots again, and it reads a pack that a prune only marked, because its
/// index holds the marked packs.
const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

/// The longest storage call deadline that the store accepts: one eighth of [`PRUNE_GRACE`], so a
/// save has time for its backup before [`index_read_bound`].
pub(super) const MAX_STORAGE_CALL_DEADLINE: Duration =
    Duration::from_millis(PRUNE_GRACE.as_millis() as u64 / 8);

/// The settings of the store: the rustic settings of each operation, the prune threshold, and the
/// runs of a call after a failed storage call.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StorePolicy {
    /// The longest time that one blob storage call waits for an answer, over all its tries.
    pub(super) deadline: Duration,
    /// The number of threads of each parallel stage of a save. `None` is the number of CPUs that
    /// the process can use.
    pub(super) save_threads: Option<NonZeroUsize>,
    /// The number of threads that read packs in a restore.
    pub(super) restore_reader_threads: NonZeroUsize,
    /// The settings of a prune. Two prunes never run at once. The next prune waits a full hold from
    /// the newest claim marker that was written.
    pub(super) prune: PruneSettings,
    /// The share of the size of the repository that deleted snapshots must free before a delete
    /// prunes.
    pub(super) prune_threshold: Percent,
    /// The runs of one call after a run whose storage call failed.
    pub(super) retry: RetryConfig,
    /// Whether a save lands its writes before the earliest index read of a prune.
    pub(super) publish_bound: PublishBound,
    /// The tries of each blob call that the store makes outside the claim protocol of a prune.
    #[cfg(test)]
    pub(super) in_call_tries: u32,
}

impl StorePolicy {
    /// Gives the policy with the values of the configuration.
    pub(super) fn from_config(config: &FilesystemSnapshotStoreConfig) -> Self {
        Self {
            deadline: config.storage_call_deadline(),
            save_threads: Some(config.save_threads()),
            restore_reader_threads: config.restore_reader_threads(),
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: PRUNE_GRACE,
            },
            prune_threshold: PRUNE_THRESHOLD,
            retry: config.storage_retry().clone(),
            publish_bound: PublishBound::On,
            #[cfg(test)]
            in_call_tries: super::blob_io::IN_CALL_TRIES,
        }
    }

    /// The tries of each blob call outside the claim protocol of a prune.
    pub(super) fn in_call_tries(&self) -> u32 {
        #[cfg(test)]
        {
            self.in_call_tries
        }
        #[cfg(not(test))]
        {
            super::blob_io::IN_CALL_TRIES
        }
    }

    /// The earliest index read of a prune that can delete a pack of a save whose first slot was at
    /// `t0`, or `None` when the bound is off.
    pub(super) fn index_read_bound(&self, t0: Instant) -> Option<Instant> {
        match self.publish_bound {
            PublishBound::On => Some(index_read_bound(
                t0,
                self.prune.keep_delete,
                self.deadline,
                refresh_period(self.prune.keep_delete, self.deadline),
            )),
            #[cfg(test)]
            PublishBound::Off => None,
        }
    }

    /// The time after which no run of a save whose first slot was at `t0` starts its backup, or
    /// `None` when the bound is off.
    pub(super) fn backup_end(&self, t0: Instant) -> Option<Instant> {
        self.index_read_bound(t0)
            .map(|bound| backup_end(bound, self.deadline))
    }
}

/// The options of a save of the store: a failed read of an entry fails the save, and no device id
/// is kept. `SizeMtime` compares each file with the parent that the id names. It compares the type,
/// the size and the modification time, and not the change time or the inode. `Full`, and a save
/// without a parent, use no parent, so they read every file.
pub(super) fn store_backup_options(
    policy: &StorePolicy,
    parent: Option<(SnapshotId, ChangeDetection)>,
) -> BackupOptions {
    let base = backup_options(policy.save_threads)
        .fail_on_read_error(true)
        .ignore_save_opts(LocalSourceSaveOptions::default().set_devid(DevIdOption::No));
    let parent_opts = match parent {
        // rustic compares the inodes only when `ignore_inode` is true.
        Some((id, ChangeDetection::SizeMtime)) => base
            .parent_opts
            .clone()
            .parents(vec![id.to_hex().to_string()])
            .ignore_ctime(true)
            .ignore_inode(false),
        None | Some((_, ChangeDetection::Full)) => base.parent_opts.clone().force(true),
    };
    base.parent_opts(parent_opts)
}

/// The options of a restore of the store. A metadata error fails the restore. The restore does not
/// set the owner, because a snapshot does not keep it.
pub(super) fn store_restore_options(policy: &StorePolicy) -> RestoreOptions {
    RestoreOptions::default()
        .reader_threads(Some(policy.restore_reader_threads))
        .fail_on_metadata_error(true)
        .no_ownership(true)
}

#[cfg(test)]
mod tests;
