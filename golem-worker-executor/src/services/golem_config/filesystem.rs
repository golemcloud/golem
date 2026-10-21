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
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;
use std::time::Duration;

/// Configuration for agent filesystems, their storage and their cleanup.
///
/// An unknown key, such as a key of an earlier storage configuration, makes the configuration fail
/// to load.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemStorageConfig {
    /// Retry policy for deleting and verifying runtime filesystem directories.
    /// `max_attempts` includes the initial deletion attempt.
    pub cleanup_retry: RetryConfig,
    /// Where the agent filesystems live, and how the executor accounts for them.
    pub mode: FilesystemStorageMode,
    /// Private policy for deriving an agent's filesystem-object hard limit
    /// proportionally from its allocated-byte limit, with fixed bounds.
    pub filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig,
    /// Physical capacity watermarks for managed filesystem pressure recovery.
    #[serde(default)]
    pub pressure: FilesystemPressureConfig,
}

/// The storage of the agent filesystems. Exactly one mode applies.
///
/// With a root, an agent filesystem is the directory
/// `<root>/<environment_id>/<component_id>/<agent segment>/`. The agent segment is the agent name
/// with each character that is not an ASCII letter, a digit, `-` or `_` replaced by `_`, cut to 32
/// characters, then `-` and the BLAKE3 hash of the agent id. An empty agent name gives `agent`.
/// The executor also makes the host directories `.scratch` and `.initial-files` directly under the
/// root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "config", deny_unknown_fields)]
pub enum FilesystemStorageMode {
    /// Development storage in a new temporary directory for each agent filesystem.
    Temporary,
    /// Development storage under `root`, on any filesystem.
    Directory { root: Box<Path> },
    /// XFS with project quotas, at the root of a dedicated XFS filesystem. Each agent filesystem
    /// is a project, which enforces its disk limits and measures its usage. Copies are reflinks.
    ///
    /// The quota calls go to `xfs-quotad`, which listens on `xfs_quotad_socket`: a separate
    /// process that holds `CAP_SYS_ADMIN`, so that the executor needs no capability.
    ManagedXfs {
        root: Box<Path>,
        xfs_quotad_socket: Box<Path>,
    },
    /// XFS with reflink and without project quotas, at the root of a dedicated XFS filesystem.
    /// Copies are reflinks. The executor enforces no per-agent disk limit and measures no
    /// per-agent usage.
    ReflinkXfs { root: Box<Path> },
    /// APFS storage for local development and tests on macOS. Copies are clones. The executor
    /// enforces no per-agent disk limit and measures no per-agent usage.
    Apfs { root: Box<Path> },
}

/// The `type` tag of a [`FilesystemStorageMode`], without its settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FilesystemStorageModeKind {
    Temporary,
    Directory,
    ManagedXfs,
    ReflinkXfs,
    Apfs,
}

impl FilesystemStorageMode {
    pub fn kind(&self) -> FilesystemStorageModeKind {
        match self {
            Self::Temporary => FilesystemStorageModeKind::Temporary,
            Self::Directory { .. } => FilesystemStorageModeKind::Directory,
            Self::ManagedXfs { .. } => FilesystemStorageModeKind::ManagedXfs,
            Self::ReflinkXfs { .. } => FilesystemStorageModeKind::ReflinkXfs,
            Self::Apfs { .. } => FilesystemStorageModeKind::Apfs,
        }
    }
}

impl SafeDisplay for FilesystemStorageMode {
    fn to_safe_string(&self) -> String {
        match self {
            Self::Temporary => "temporary directories".to_string(),
            Self::Directory { root } => format!("directory at {}", root.display()),
            Self::ManagedXfs {
                root,
                xfs_quotad_socket: socket,
            } => format!(
                "managed XFS at {}, with xfs-quotad at {}",
                root.display(),
                socket.display()
            ),
            Self::ReflinkXfs { root } => format!("reflink XFS at {}", root.display()),
            Self::Apfs { root } => format!("APFS for local development at {}", root.display()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FilesystemPressureConfig {
    /// Available bytes at or below which physical pressure is active.
    minimum_available_bytes: u64,
    /// Available bytes required after reclamation before retrying a mutation.
    target_available_bytes: u64,
    /// Available filesystem objects at or below which object pressure is active.
    minimum_available_filesystem_objects: u64,
    /// Available filesystem objects required after reclamation before retrying a mutation.
    target_available_filesystem_objects: u64,
    /// Number of fresh observations allowed after each completed deletion.
    reclamation_observation_attempts: NonZeroU32,
    /// Delay between post-deletion observations while reclamation settles.
    #[serde(with = "humantime_serde")]
    reclamation_observation_delay: Duration,
}

#[derive(Deserialize)]
struct RawFilesystemPressureConfig {
    minimum_available_bytes: u64,
    target_available_bytes: u64,
    minimum_available_filesystem_objects: u64,
    target_available_filesystem_objects: u64,
    reclamation_observation_attempts: u32,
    #[serde(with = "humantime_serde")]
    reclamation_observation_delay: Duration,
}

impl FilesystemPressureConfig {
    pub fn new(
        minimum_available_bytes: u64,
        target_available_bytes: u64,
        minimum_available_filesystem_objects: u64,
        target_available_filesystem_objects: u64,
        reclamation_observation_attempts: u32,
        reclamation_observation_delay: Duration,
    ) -> Result<Self, String> {
        if minimum_available_bytes >= target_available_bytes {
            return Err(
                "minimum_available_bytes must be less than target_available_bytes".to_string(),
            );
        }
        if minimum_available_filesystem_objects >= target_available_filesystem_objects {
            return Err("minimum_available_filesystem_objects must be less than target_available_filesystem_objects".to_string());
        }
        let reclamation_observation_attempts = NonZeroU32::new(reclamation_observation_attempts)
            .ok_or_else(|| {
                "reclamation_observation_attempts must be greater than zero".to_string()
            })?;

        Ok(Self {
            minimum_available_bytes,
            target_available_bytes,
            minimum_available_filesystem_objects,
            target_available_filesystem_objects,
            reclamation_observation_attempts,
            reclamation_observation_delay,
        })
    }

    pub const fn minimum_available_bytes(&self) -> u64 {
        self.minimum_available_bytes
    }

    pub const fn target_available_bytes(&self) -> u64 {
        self.target_available_bytes
    }

    pub const fn minimum_available_filesystem_objects(&self) -> u64 {
        self.minimum_available_filesystem_objects
    }

    pub const fn target_available_filesystem_objects(&self) -> u64 {
        self.target_available_filesystem_objects
    }

    pub const fn reclamation_observation_attempts(&self) -> u32 {
        self.reclamation_observation_attempts.get()
    }

    pub const fn reclamation_observation_delay(&self) -> Duration {
        self.reclamation_observation_delay
    }
}

impl<'de> Deserialize<'de> for FilesystemPressureConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawFilesystemPressureConfig::deserialize(deserializer)?;
        Self::new(
            raw.minimum_available_bytes,
            raw.target_available_bytes,
            raw.minimum_available_filesystem_objects,
            raw.target_available_filesystem_objects,
            raw.reclamation_observation_attempts,
            raw.reclamation_observation_delay,
        )
        .map_err(D::Error::custom)
    }
}

impl Default for FilesystemPressureConfig {
    fn default() -> Self {
        Self::new(
            64 * 1024 * 1024,
            128 * 1024 * 1024,
            8_192,
            16_384,
            4,
            Duration::from_millis(25),
        )
        .expect("default filesystem pressure configuration must be valid")
    }
}

impl SafeDisplay for FilesystemPressureConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "minimum available bytes: {}",
            self.minimum_available_bytes
        );
        let _ = writeln!(
            &mut result,
            "target available bytes: {}",
            self.target_available_bytes
        );
        let _ = writeln!(
            &mut result,
            "minimum available filesystem objects: {}",
            self.minimum_available_filesystem_objects
        );
        let _ = writeln!(
            &mut result,
            "target available filesystem objects: {}",
            self.target_available_filesystem_objects
        );
        let _ = writeln!(
            &mut result,
            "reclamation observation attempts: {}",
            self.reclamation_observation_attempts
        );
        let _ = writeln!(
            &mut result,
            "reclamation observation delay: {:?}",
            self.reclamation_observation_delay
        );
        result
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FilesystemObjectLimitPolicyConfig {
    /// Number of filesystem objects granted per GiB of allocated storage.
    objects_per_gib: NonZeroU64,
    /// Object quota floor for small storage allocations.
    minimum_objects: NonZeroU64,
    /// Object quota ceiling for large storage allocations.
    maximum_objects: NonZeroU64,
}

#[derive(Deserialize)]
struct RawFilesystemObjectLimitPolicyConfig {
    objects_per_gib: u64,
    minimum_objects: u64,
    maximum_objects: u64,
}

impl FilesystemObjectLimitPolicyConfig {
    pub fn new(
        objects_per_gib: u64,
        minimum_objects: u64,
        maximum_objects: u64,
    ) -> Result<Self, String> {
        let objects_per_gib = NonZeroU64::new(objects_per_gib)
            .ok_or_else(|| "objects_per_gib must be greater than zero".to_string())?;
        let minimum_objects = NonZeroU64::new(minimum_objects)
            .ok_or_else(|| "minimum_objects must be greater than zero".to_string())?;
        let maximum_objects = NonZeroU64::new(maximum_objects)
            .ok_or_else(|| "maximum_objects must be greater than zero".to_string())?;
        if minimum_objects > maximum_objects {
            return Err("minimum_objects must not exceed maximum_objects".to_string());
        }

        Ok(Self {
            objects_per_gib,
            minimum_objects,
            maximum_objects,
        })
    }

    pub const fn objects_per_gib(&self) -> u64 {
        self.objects_per_gib.get()
    }

    pub const fn minimum_objects(&self) -> u64 {
        self.minimum_objects.get()
    }

    pub const fn maximum_objects(&self) -> u64 {
        self.maximum_objects.get()
    }
}

impl<'de> Deserialize<'de> for FilesystemObjectLimitPolicyConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawFilesystemObjectLimitPolicyConfig::deserialize(deserializer)?;
        Self::new(
            raw.objects_per_gib,
            raw.minimum_objects,
            raw.maximum_objects,
        )
        .map_err(D::Error::custom)
    }
}

impl Default for FilesystemObjectLimitPolicyConfig {
    fn default() -> Self {
        Self::new(32_768, 8192, 131_072)
            .expect("default filesystem object limit policy must be valid")
    }
}

impl SafeDisplay for FilesystemObjectLimitPolicyConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "objects per GiB: {}", self.objects_per_gib);
        let _ = writeln!(&mut result, "minimum objects: {}", self.minimum_objects);
        let _ = writeln!(&mut result, "maximum objects: {}", self.maximum_objects);
        result
    }
}

impl SafeDisplay for FilesystemStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "cleanup retry:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.cleanup_retry.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "mode: {}", self.mode.to_safe_string());
        let _ = writeln!(&mut result, "filesystem object limit policy:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.filesystem_object_limit_policy
                .to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "pressure:");
        let _ = writeln!(&mut result, "{}", self.pressure.to_safe_string_indented());
        result
    }
}

impl Default for FilesystemStorageConfig {
    fn default() -> Self {
        Self {
            cleanup_retry: RetryConfig {
                max_attempts: 4,
                min_delay: Duration::from_millis(25),
                max_delay: Duration::from_millis(250),
                multiplier: 4.0,
                max_jitter_factor: None,
            },
            mode: FilesystemStorageMode::Temporary,
            filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig::default(),
            pressure: FilesystemPressureConfig::default(),
        }
    }
}
