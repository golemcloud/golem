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

use golem_common::SafeDisplay;
use golem_common::model::RetryConfig;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt::Write;
use std::num::NonZeroUsize;
use std::time::Duration;

/// The default of [`FilesystemSnapshotStoreConfig::storage_call_deadline`]. The slowest measured
/// call on S3 took 1.7 s, and the value stays at least 10 times the slowest measured call.
pub const DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE: Duration = Duration::from_secs(60);

/// The default of [`FilesystemSnapshotStoreConfig::restore_reader_threads`].
const DEFAULT_FILESYSTEM_SNAPSHOT_RESTORE_READER_THREADS: usize = 6;

/// The default of [`FilesystemSnapshotStoreConfig::save_threads`].
const DEFAULT_FILESYSTEM_SNAPSHOT_SAVE_THREADS: usize = 2;

/// Tells whether the executor keeps filesystem snapshots, and gives the settings of the store.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum FilesystemSnapshotsConfig {
    Disabled(FilesystemSnapshotsDisabledConfig),
    Enabled(Box<FilesystemSnapshotStoreConfig>),
}

/// The `type` tag of a [`FilesystemSnapshotsConfig`], without its settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FilesystemSnapshotsKind {
    Disabled,
    Enabled,
}

impl FilesystemSnapshotsConfig {
    pub fn kind(&self) -> FilesystemSnapshotsKind {
        match self {
            Self::Disabled(_) => FilesystemSnapshotsKind::Disabled,
            Self::Enabled(_) => FilesystemSnapshotsKind::Enabled,
        }
    }
}

impl Default for FilesystemSnapshotsConfig {
    fn default() -> Self {
        Self::Disabled(FilesystemSnapshotsDisabledConfig {})
    }
}

impl SafeDisplay for FilesystemSnapshotsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            Self::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
            Self::Enabled(store) => {
                let _ = writeln!(&mut result, "enabled:");
                let _ = writeln!(&mut result, "{}", store.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilesystemSnapshotsDisabledConfig {}

/// The settings of the store of filesystem snapshots.
#[derive(Clone, Debug, Serialize)]
pub struct FilesystemSnapshotStoreConfig {
    /// The key that encrypts each repository, as 128 hex characters. The key is a secret.
    repository_key: FilesystemSnapshotRepositoryKey,
    /// The longest time that one blob storage call of the store waits for an answer.
    #[serde(with = "humantime_serde")]
    storage_call_deadline: Duration,
    /// The number of threads that read packs in a restore.
    restore_reader_threads: NonZeroUsize,
    /// The number of threads of each parallel stage of a save.
    save_threads: NonZeroUsize,
    /// The runs of one store call after a run whose storage call failed.
    storage_retry: RetryConfig,
    /// The settings of the uploads, the restores and the retention of the snapshots.
    #[serde(flatten)]
    uploads: FilesystemSnapshotUploadConfig,
}

/// The settings of the uploads, the restores and the retention of filesystem snapshots.
#[derive(Clone, Debug, Serialize)]
pub struct FilesystemSnapshotUploadConfig {
    /// The number of runs of store operations that save, delete, copy or list for retention at the
    /// same time on one executor. It is also the number of clean-ups that run at the same time.
    max_concurrent_uploads: NonZeroUsize,
    /// The number of restores that run at the same time on one executor.
    max_concurrent_restores: NonZeroUsize,
    /// How long a start of an agent waits for an upload of the same agent on this executor. The
    /// wait and the check of the store after it take at most this time together. A loaded agent
    /// that restarts in place for an automatic update waits as long for the upload of its newest
    /// record. The waits of a manual update for a running upload of the agent and for a slot of
    /// the uploads also end this long after the update started.
    #[serde(with = "humantime_serde")]
    confirmation_wait: Duration,
    /// How long a start checks the store for the snapshot of its newest record when it did not
    /// wait for an upload. A fork from a snapshot checks the store for that snapshot for at most
    /// this time too.
    #[serde(with = "humantime_serde")]
    store_check_limit: Duration,
    /// How long a capture waits for open file calls before it gives up.
    #[serde(with = "humantime_serde")]
    capture_wait: Duration,
    /// The number of periodic snapshots that retention keeps for each agent, the newest first.
    /// A revert restores exactly only from a periodic snapshot that the store still holds, so this
    /// number sets how far back a revert can go without a full replay. Names that the agent still
    /// uses (the records that a start can select, the successful and pending updates and the
    /// authoritative snapshot-assisted baseline), and names at most 2 minutes older than the new
    /// snapshot, or newer, are kept as well and do not count toward this number.
    retained_periodic_snapshots: NonZeroUsize,
    /// The number of manual-update snapshots that retention keeps for each agent among those that
    /// no successful or pending update of the agent uses. The snapshot of each successful or
    /// pending update stays.
    retained_update_snapshots: NonZeroUsize,
    /// The largest number of snapshot names of one agent that wait in the clean-up queue for
    /// deletion. Only the names of a revert wait there; retention deletes directly, and a delete
    /// of all snapshots carries no names. When the bound is reached, the names of a new request
    /// that do not fit are refused and counted as a leaked clean-up: these are the oldest names of
    /// the reverted region, and count retention or the delete of the agent removes them later.
    max_pending_deletes_per_agent: NonZeroUsize,
}

/// The default of [`FilesystemSnapshotUploadConfig::max_concurrent_uploads`].
const DEFAULT_FILESYSTEM_SNAPSHOT_MAX_CONCURRENT_UPLOADS: usize = 4;

/// The default of [`FilesystemSnapshotUploadConfig::max_concurrent_restores`], the value for the
/// node type c8gd.4xlarge.
const DEFAULT_FILESYSTEM_SNAPSHOT_MAX_CONCURRENT_RESTORES: usize = 8;

/// The default of [`FilesystemSnapshotUploadConfig::confirmation_wait`].
const DEFAULT_FILESYSTEM_SNAPSHOT_CONFIRMATION_WAIT: Duration = Duration::from_secs(60);

/// The default of [`FilesystemSnapshotUploadConfig::store_check_limit`].
const DEFAULT_FILESYSTEM_SNAPSHOT_STORE_CHECK_LIMIT: Duration = Duration::from_secs(5);

/// The default of [`FilesystemSnapshotUploadConfig::capture_wait`]. An executor without filesystem
/// snapshots waits as long in the check of a tree of initial files.
pub(crate) const DEFAULT_FILESYSTEM_SNAPSHOT_CAPTURE_WAIT: Duration = Duration::from_secs(5);

/// The largest jitter factor of the runs of a store call: a jitter at most doubles a wait.
const MAX_FILESYSTEM_SNAPSHOT_JITTER_FACTOR: f64 = 1.0;

/// The default of [`FilesystemSnapshotUploadConfig::retained_periodic_snapshots`]: about 20 min
/// of exact reverts at one upload every 10 s.
const DEFAULT_RETAINED_PERIODIC_SNAPSHOTS: usize = 128;

/// The default of [`FilesystemSnapshotUploadConfig::retained_update_snapshots`].
const DEFAULT_RETAINED_UPDATE_SNAPSHOTS: usize = 2;

/// The default of [`FilesystemSnapshotUploadConfig::max_pending_deletes_per_agent`].
const DEFAULT_MAX_PENDING_DELETES_PER_AGENT: usize = 1024;

/// The default of [`FilesystemSnapshotStoreConfig::storage_retry`]: 5 runs of one store call, with
/// waits of 2 s, 8 s, 32 s and 120 s between them.
const DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_RETRY: RetryConfig = RetryConfig {
    max_attempts: 5,
    min_delay: Duration::from_secs(2),
    max_delay: Duration::from_secs(120),
    multiplier: 4.0,
    max_jitter_factor: None,
};

pub(crate) fn default_filesystem_snapshot_storage_retry() -> RetryConfig {
    DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_RETRY
}

/// The values of a [`FilesystemSnapshotUploadConfig`] before [`FilesystemSnapshotUploadConfig::new`]
/// checks them. The default holds the default of each setting.
#[derive(Clone, Debug)]
pub struct FilesystemSnapshotUploadValues {
    pub max_concurrent_uploads: usize,
    pub max_concurrent_restores: usize,
    pub confirmation_wait: Duration,
    pub store_check_limit: Duration,
    pub capture_wait: Duration,
    pub retained_periodic_snapshots: usize,
    pub retained_update_snapshots: usize,
    pub max_pending_deletes_per_agent: usize,
}

impl Default for FilesystemSnapshotUploadValues {
    fn default() -> Self {
        Self {
            max_concurrent_uploads: DEFAULT_FILESYSTEM_SNAPSHOT_MAX_CONCURRENT_UPLOADS,
            max_concurrent_restores: DEFAULT_FILESYSTEM_SNAPSHOT_MAX_CONCURRENT_RESTORES,
            confirmation_wait: DEFAULT_FILESYSTEM_SNAPSHOT_CONFIRMATION_WAIT,
            store_check_limit: DEFAULT_FILESYSTEM_SNAPSHOT_STORE_CHECK_LIMIT,
            capture_wait: DEFAULT_FILESYSTEM_SNAPSHOT_CAPTURE_WAIT,
            retained_periodic_snapshots: DEFAULT_RETAINED_PERIODIC_SNAPSHOTS,
            retained_update_snapshots: DEFAULT_RETAINED_UPDATE_SNAPSHOTS,
            max_pending_deletes_per_agent: DEFAULT_MAX_PENDING_DELETES_PER_AGENT,
        }
    }
}

impl FilesystemSnapshotUploadConfig {
    /// Checks `values`: each count and each wait must be greater than zero.
    pub fn new(values: FilesystemSnapshotUploadValues) -> Result<Self, String> {
        let FilesystemSnapshotUploadValues {
            max_concurrent_uploads,
            max_concurrent_restores,
            confirmation_wait,
            store_check_limit,
            capture_wait,
            retained_periodic_snapshots,
            retained_update_snapshots,
            max_pending_deletes_per_agent,
        } = values;
        let count = |value: usize, name: &str| {
            NonZeroUsize::new(value).ok_or_else(|| format!("{name} must be greater than zero"))
        };
        let wait = |value: Duration, name: &str| {
            if value.is_zero() {
                Err(format!("{name} must be greater than zero"))
            } else {
                Ok(value)
            }
        };
        Ok(Self {
            max_concurrent_uploads: count(max_concurrent_uploads, "max_concurrent_uploads")?,
            max_concurrent_restores: count(max_concurrent_restores, "max_concurrent_restores")?,
            confirmation_wait: wait(confirmation_wait, "confirmation_wait")?,
            store_check_limit: wait(store_check_limit, "store_check_limit")?,
            capture_wait: wait(capture_wait, "capture_wait")?,
            retained_periodic_snapshots: count(
                retained_periodic_snapshots,
                "retained_periodic_snapshots",
            )?,
            retained_update_snapshots: count(
                retained_update_snapshots,
                "retained_update_snapshots",
            )?,
            max_pending_deletes_per_agent: count(
                max_pending_deletes_per_agent,
                "max_pending_deletes_per_agent",
            )?,
        })
    }

    pub const fn max_concurrent_uploads(&self) -> NonZeroUsize {
        self.max_concurrent_uploads
    }

    pub const fn max_concurrent_restores(&self) -> NonZeroUsize {
        self.max_concurrent_restores
    }

    pub const fn confirmation_wait(&self) -> Duration {
        self.confirmation_wait
    }

    pub const fn store_check_limit(&self) -> Duration {
        self.store_check_limit
    }

    pub const fn capture_wait(&self) -> Duration {
        self.capture_wait
    }

    pub const fn retained_periodic_snapshots(&self) -> NonZeroUsize {
        self.retained_periodic_snapshots
    }

    pub const fn retained_update_snapshots(&self) -> NonZeroUsize {
        self.retained_update_snapshots
    }

    pub const fn max_pending_deletes_per_agent(&self) -> NonZeroUsize {
        self.max_pending_deletes_per_agent
    }
}

impl Default for FilesystemSnapshotUploadConfig {
    fn default() -> Self {
        Self::new(FilesystemSnapshotUploadValues::default())
            .expect("the default filesystem snapshot upload settings are valid")
    }
}

impl SafeDisplay for FilesystemSnapshotUploadConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "max concurrent uploads: {}",
            self.max_concurrent_uploads
        );
        let _ = writeln!(
            &mut result,
            "max concurrent restores: {}",
            self.max_concurrent_restores
        );
        let _ = writeln!(
            &mut result,
            "confirmation wait: {:?}",
            self.confirmation_wait
        );
        let _ = writeln!(
            &mut result,
            "store check limit: {:?}",
            self.store_check_limit
        );
        let _ = writeln!(&mut result, "capture wait: {:?}", self.capture_wait);
        let _ = writeln!(
            &mut result,
            "retained periodic snapshots: {}",
            self.retained_periodic_snapshots
        );
        let _ = writeln!(
            &mut result,
            "retained update snapshots: {}",
            self.retained_update_snapshots
        );
        let _ = writeln!(
            &mut result,
            "max pending deletes per agent: {}",
            self.max_pending_deletes_per_agent
        );
        result
    }
}

#[derive(Deserialize)]
struct RawFilesystemSnapshotStoreConfig {
    repository_key: String,
    #[serde(
        with = "humantime_serde",
        default = "default_filesystem_snapshot_storage_call_deadline"
    )]
    storage_call_deadline: Duration,
    #[serde(default = "default_filesystem_snapshot_restore_reader_threads")]
    restore_reader_threads: usize,
    #[serde(default = "default_filesystem_snapshot_save_threads")]
    save_threads: usize,
    #[serde(default = "default_filesystem_snapshot_storage_retry")]
    storage_retry: RetryConfig,
    #[serde(default = "default_filesystem_snapshot_max_concurrent_uploads")]
    max_concurrent_uploads: usize,
    #[serde(default = "default_filesystem_snapshot_max_concurrent_restores")]
    max_concurrent_restores: usize,
    #[serde(
        with = "humantime_serde",
        default = "default_filesystem_snapshot_confirmation_wait"
    )]
    confirmation_wait: Duration,
    #[serde(
        with = "humantime_serde",
        default = "default_filesystem_snapshot_store_check_limit"
    )]
    store_check_limit: Duration,
    #[serde(
        with = "humantime_serde",
        default = "default_filesystem_snapshot_capture_wait"
    )]
    capture_wait: Duration,
    #[serde(default = "default_retained_periodic_snapshots")]
    retained_periodic_snapshots: usize,
    #[serde(default = "default_retained_update_snapshots")]
    retained_update_snapshots: usize,
    #[serde(default = "default_max_pending_deletes_per_agent")]
    max_pending_deletes_per_agent: usize,
}

fn default_filesystem_snapshot_max_concurrent_uploads() -> usize {
    DEFAULT_FILESYSTEM_SNAPSHOT_MAX_CONCURRENT_UPLOADS
}

fn default_filesystem_snapshot_max_concurrent_restores() -> usize {
    DEFAULT_FILESYSTEM_SNAPSHOT_MAX_CONCURRENT_RESTORES
}

fn default_filesystem_snapshot_confirmation_wait() -> Duration {
    DEFAULT_FILESYSTEM_SNAPSHOT_CONFIRMATION_WAIT
}

fn default_filesystem_snapshot_store_check_limit() -> Duration {
    DEFAULT_FILESYSTEM_SNAPSHOT_STORE_CHECK_LIMIT
}

fn default_filesystem_snapshot_capture_wait() -> Duration {
    DEFAULT_FILESYSTEM_SNAPSHOT_CAPTURE_WAIT
}

fn default_retained_periodic_snapshots() -> usize {
    DEFAULT_RETAINED_PERIODIC_SNAPSHOTS
}

fn default_retained_update_snapshots() -> usize {
    DEFAULT_RETAINED_UPDATE_SNAPSHOTS
}

fn default_max_pending_deletes_per_agent() -> usize {
    DEFAULT_MAX_PENDING_DELETES_PER_AGENT
}

fn default_filesystem_snapshot_storage_call_deadline() -> Duration {
    DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE
}

fn default_filesystem_snapshot_restore_reader_threads() -> usize {
    DEFAULT_FILESYSTEM_SNAPSHOT_RESTORE_READER_THREADS
}

fn default_filesystem_snapshot_save_threads() -> usize {
    DEFAULT_FILESYSTEM_SNAPSHOT_SAVE_THREADS
}

impl FilesystemSnapshotStoreConfig {
    /// Checks the values: the deadline must be in the range that the store accepts, and each count
    /// must be greater than zero. The runs after a failed call are the default ones, and
    /// [`Self::with_storage_retry`] changes them.
    pub fn new(
        repository_key: &str,
        storage_call_deadline: Duration,
        restore_reader_threads: usize,
        save_threads: usize,
    ) -> Result<Self, String> {
        let repository_key = FilesystemSnapshotRepositoryKey::parse(repository_key)?;
        let deadlines = crate::filesystem_snapshot::storage_call_deadlines();
        if !deadlines.contains(&storage_call_deadline) {
            return Err(format!(
                "storage_call_deadline must be from {:?} to {:?}",
                deadlines.start(),
                deadlines.end()
            ));
        }
        let restore_reader_threads = NonZeroUsize::new(restore_reader_threads)
            .ok_or_else(|| "restore_reader_threads must be greater than zero".to_string())?;
        let save_threads = NonZeroUsize::new(save_threads)
            .ok_or_else(|| "save_threads must be greater than zero".to_string())?;
        Ok(Self {
            repository_key,
            storage_call_deadline,
            restore_reader_threads,
            save_threads,
            storage_retry: DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_RETRY,
            uploads: FilesystemSnapshotUploadConfig::default(),
        })
    }

    /// Gives this configuration with the runs after a failed call `storage_retry`, which must make
    /// a run with a growing wait.
    pub fn with_storage_retry(self, storage_retry: RetryConfig) -> Result<Self, String> {
        if storage_retry.max_attempts == 0 {
            return Err("storage_retry.max_attempts must be greater than zero".to_string());
        }
        if storage_retry.min_delay > storage_retry.max_delay {
            return Err("storage_retry.min_delay must not be greater than max_delay".to_string());
        }
        if !storage_retry.multiplier.is_finite() || storage_retry.multiplier < 1.0 {
            return Err("storage_retry.multiplier must be at least 1".to_string());
        }
        if storage_retry
            .max_jitter_factor
            .is_some_and(|factor| !(0.0..=MAX_FILESYSTEM_SNAPSHOT_JITTER_FACTOR).contains(&factor))
        {
            return Err(format!(
                "storage_retry.max_jitter_factor must be between 0 and \
                 {MAX_FILESYSTEM_SNAPSHOT_JITTER_FACTOR}"
            ));
        }
        Ok(Self {
            storage_retry,
            ..self
        })
    }

    /// The runs of one store call after a run whose storage call failed.
    pub fn storage_retry(&self) -> &RetryConfig {
        &self.storage_retry
    }

    /// Gives this configuration with the upload settings `uploads`.
    pub fn with_uploads(self, uploads: FilesystemSnapshotUploadConfig) -> Self {
        Self { uploads, ..self }
    }

    pub fn uploads(&self) -> &FilesystemSnapshotUploadConfig {
        &self.uploads
    }

    pub fn repository_key(&self) -> &FilesystemSnapshotRepositoryKey {
        &self.repository_key
    }

    pub const fn storage_call_deadline(&self) -> Duration {
        self.storage_call_deadline
    }

    pub const fn restore_reader_threads(&self) -> NonZeroUsize {
        self.restore_reader_threads
    }

    pub const fn save_threads(&self) -> NonZeroUsize {
        self.save_threads
    }
}

impl<'de> Deserialize<'de> for FilesystemSnapshotStoreConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawFilesystemSnapshotStoreConfig::deserialize(deserializer)?;
        let uploads = FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
            max_concurrent_uploads: raw.max_concurrent_uploads,
            max_concurrent_restores: raw.max_concurrent_restores,
            confirmation_wait: raw.confirmation_wait,
            store_check_limit: raw.store_check_limit,
            capture_wait: raw.capture_wait,
            retained_periodic_snapshots: raw.retained_periodic_snapshots,
            retained_update_snapshots: raw.retained_update_snapshots,
            max_pending_deletes_per_agent: raw.max_pending_deletes_per_agent,
        })
        .map_err(D::Error::custom)?;
        Self::new(
            &raw.repository_key,
            raw.storage_call_deadline,
            raw.restore_reader_threads,
            raw.save_threads,
        )
        .and_then(|config| config.with_storage_retry(raw.storage_retry))
        .map(|config| config.with_uploads(uploads))
        .map_err(D::Error::custom)
    }
}

impl SafeDisplay for FilesystemSnapshotStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "repository key: ****");
        let _ = writeln!(
            &mut result,
            "storage call deadline: {:?}",
            self.storage_call_deadline
        );
        let _ = writeln!(
            &mut result,
            "restore reader threads: {}",
            self.restore_reader_threads
        );
        let _ = writeln!(&mut result, "save threads: {}", self.save_threads);
        let _ = writeln!(&mut result, "storage retry:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.storage_retry.to_safe_string_indented()
        );
        let _ = write!(&mut result, "{}", self.uploads.to_safe_string());
        result
    }
}

/// The key that encrypts the repositories of filesystem snapshots. It has 64 bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct FilesystemSnapshotRepositoryKey([u8; 64]);

impl FilesystemSnapshotRepositoryKey {
    /// Gives the key from its 128 hex characters.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() {
            return Err("repository_key must not be empty".to_string());
        }
        hex::decode(text)
            .ok()
            .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
            .map(Self)
            .ok_or_else(|| "repository_key must be 128 hex characters".to_string())
    }

    pub const fn bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl std::fmt::Debug for FilesystemSnapshotRepositoryKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FilesystemSnapshotRepositoryKey(****)")
    }
}

impl Serialize for FilesystemSnapshotRepositoryKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}
