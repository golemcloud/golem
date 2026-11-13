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

//! Configuration mapping to the store's operation settings and retry policy.

use super::pruning::PruneSettings;
use super::publication::PublishBound;
use super::restore::RestoreSettings;
use super::save::SaveSettings;
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use golem_common::model::RetryConfig;
use std::num::NonZeroUsize;
use std::time::Duration;

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
            prune: PruneSettings::default(),
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

    pub(super) fn save_settings(&self) -> SaveSettings {
        SaveSettings {
            threads: self.save_threads,
            grace: self.prune.keep_delete,
            publish_bound: self.publish_bound,
        }
    }

    pub(super) fn restore_settings(&self) -> RestoreSettings {
        RestoreSettings {
            reader_threads: self.restore_reader_threads,
        }
    }

    pub(super) fn prune_settings(&self) -> PruneSettings {
        self.prune
    }
}

#[cfg(test)]
mod tests;
