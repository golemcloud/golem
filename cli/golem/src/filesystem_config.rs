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

//! The local filesystem defaults of the single executable and the retained repository key of its
//! filesystem snapshots.

use anyhow::Context;
use golem_worker_executor::services::golem_config::{
    FilesystemSnapshotRepositoryKey, FilesystemSnapshotsKind, FilesystemStorageMode,
};
use rand::TryRngCore as _;
use std::io::Write as _;
use std::path::Path;

pub(crate) const KEY_FILE: &str = "filesystem-snapshots.repository-key";

/// A repository key that only lets a configuration with enabled snapshots be validated. A
/// configuration that holds it never starts an executor.
pub(crate) const VALIDATION_ONLY_REPOSITORY_KEY: &str = concat!(
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
);

#[derive(Clone, Copy, Debug)]
pub(crate) enum LocalPlatform {
    MacOs,
    Other,
}

impl LocalPlatform {
    pub(crate) fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }

    /// The storage mode of agent filesystems under `root`: APFS clones on macOS, and plain
    /// directories elsewhere.
    pub(crate) fn rooted_mode(self, root: &Path) -> FilesystemStorageMode {
        let root = Box::<Path>::from(root);
        match self {
            Self::MacOs => FilesystemStorageMode::Apfs { root },
            Self::Other => FilesystemStorageMode::Directory { root },
        }
    }

    /// The settings of the worker executor that this platform changes in the local development
    /// configuration, as a config layer: on macOS, APFS storage in `<data_dir>/agents` with
    /// filesystem snapshots. Elsewhere, agent filesystems are temporary and snapshots are disabled,
    /// which are the defaults.
    pub(crate) fn defaults(self, data_dir: &Path) -> serde_json::Value {
        match self {
            Self::MacOs => serde_json::json!({
                "worker_executor": {
                    "filesystem_storage": {
                        "mode": self.rooted_mode(&data_dir.join("agents")),
                    },
                    "filesystem_snapshots": { "type": FilesystemSnapshotsKind::Enabled },
                },
            }),
            Self::Other => serde_json::json!({}),
        }
    }
}

fn validate_retained_repository_key(key: Box<str>) -> anyhow::Result<Box<str>> {
    FilesystemSnapshotRepositoryKey::parse(&key).map_err(anyhow::Error::msg)?;
    Ok(key)
}

fn read_retained_repository_key(path: &Path) -> anyhow::Result<Box<str>> {
    let key = std::fs::read_to_string(path)
        .with_context(|| {
            format!(
                "Failed to read the retained repository key at {}",
                path.display()
            )
        })?
        .into_boxed_str();
    validate_retained_repository_key(key)
}

fn repository_key_creation_allowed(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// The repository key kept in `data_dir`. A missing key file is created with a new random key;
/// an unreadable or invalid one is an error and stays as it is.
pub(crate) fn retained_repository_key(data_dir: &Path) -> anyhow::Result<Box<str>> {
    let path = data_dir.join(KEY_FILE).into_boxed_path();
    match read_retained_repository_key(&path) {
        Ok(key) => return Ok(key),
        Err(error) if repository_key_creation_allowed(&error) => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(data_dir)?;
    let mut bytes = [0; 64];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .context("Failed to generate the retained repository key")?;
    let key = hex::encode(bytes).into_boxed_str();
    let mut temporary = tempfile::NamedTempFile::new_in(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(key.as_bytes())?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&path) {
        Ok(_) => {
            #[cfg(unix)]
            rustix::fs::fsync(std::fs::File::open(data_dir)?)?;
            Ok(key)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_retained_repository_key(&path)
        }
        Err(error) => Err(error.error).context("Failed to persist the retained repository key"),
    }
}

#[cfg(test)]
mod tests {
    use super::{KEY_FILE, LocalPlatform};
    use golem_worker_executor::services::golem_config::{
        FilesystemSnapshotRepositoryKey, FilesystemStorageMode,
    };
    use std::path::Path;
    use test_r::test;

    #[test]
    fn rooted_mode_is_apfs_on_macos_and_a_directory_elsewhere() {
        let root = Path::new("agents");
        assert_eq!(
            LocalPlatform::MacOs.rooted_mode(root),
            FilesystemStorageMode::Apfs { root: root.into() }
        );
        assert_eq!(
            LocalPlatform::Other.rooted_mode(root),
            FilesystemStorageMode::Directory { root: root.into() }
        );
    }

    #[test]
    fn repository_key_creation_requires_an_underlying_io_not_found() {
        [
            (std::io::ErrorKind::NotFound, true),
            (std::io::ErrorKind::PermissionDenied, false),
            (std::io::ErrorKind::InvalidData, false),
            (std::io::ErrorKind::Other, false),
        ]
        .into_iter()
        .for_each(|(kind, expected)| {
            let error = anyhow::Error::new(std::io::Error::from(kind))
                .context("Failed to read the retained repository key");
            assert_eq!(super::repository_key_creation_allowed(&error), expected);
        });
        assert!(!super::repository_key_creation_allowed(&anyhow::anyhow!(
            "No such file or directory"
        )));
    }

    #[test]
    fn retained_repository_key_validation_preserves_valid_text_and_rejects_corruption() {
        let key = "2a".repeat(64).into_boxed_str();
        assert_eq!(
            super::validate_retained_repository_key(key.clone()).unwrap(),
            key,
        );
        ["corrupt", "2a", ""].into_iter().for_each(|key| {
            assert!(super::validate_retained_repository_key(key.into()).is_err());
        });
    }

    #[test]
    fn retained_repository_key_read_failures_do_not_replace_the_entry() {
        [false, true].into_iter().for_each(|is_directory| {
            let data = tempfile::tempdir().unwrap();
            let key = data.path().join(KEY_FILE).into_boxed_path();
            if is_directory {
                std::fs::create_dir(&key).unwrap();
            } else {
                std::fs::write(&key, [0xff]).unwrap();
            }
            let error = super::retained_repository_key(data.path()).unwrap_err();
            assert!(error.downcast_ref::<std::io::Error>().is_some());
            if is_directory {
                assert!(key.is_dir());
                assert_eq!(std::fs::read_dir(&key).unwrap().count(), 0);
            } else {
                assert!(std::fs::read(&key).unwrap() == [0xff]);
            }
            assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 1);
        });
    }

    #[cfg(unix)]
    #[test]
    fn retained_repository_key_read_not_found_does_not_generate_a_replacement() {
        let data = tempfile::tempdir().unwrap();
        let key = data.path().join(KEY_FILE).into_boxed_path();
        let target = data.path().join("missing-key").into_boxed_path();
        std::os::unix::fs::symlink(&target, &key).unwrap();
        let error = super::retained_repository_key(data.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to read the retained repository key")
        );
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound,
        );
        assert_eq!(std::fs::read_link(&key).unwrap().as_path(), target.as_ref());
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn retained_repository_key_read_failure_makes_no_replacement_attempt() {
        use std::os::unix::fs::PermissionsExt as _;
        let data = tempfile::tempdir().unwrap();
        let key = data.path().join(KEY_FILE).into_boxed_path();
        std::fs::write(&key, [0xff]).unwrap();
        // A replacement attempt needs a temporary file in the data directory, which a read-only
        // directory refuses with an error other than the read failure.
        std::fs::set_permissions(data.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = super::retained_repository_key(data.path());
        std::fs::set_permissions(data.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to read the retained repository key"),
            "{error:#}"
        );
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::InvalidData,
        );
        assert_eq!(std::fs::read(&key).unwrap(), [0xff]);
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 1);
    }

    #[test]
    fn retained_repository_key_is_the_same_for_concurrent_starts() {
        let data = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(4);
        let keys = std::thread::scope(|scope| {
            let threads = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        super::retained_repository_key(data.path()).unwrap()
                    })
                })
                .collect::<Box<[_]>>();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Box<[_]>>()
        });
        let retained = std::fs::read_to_string(data.path().join(KEY_FILE)).unwrap();
        FilesystemSnapshotRepositoryKey::parse(&retained).unwrap();
        assert!(keys.iter().all(|key| **key == *retained));
        assert_eq!(
            *super::retained_repository_key(data.path()).unwrap(),
            *retained
        );
    }

    #[test]
    fn corrupt_retained_repository_key_is_refused_and_not_replaced() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(KEY_FILE), b"corrupt").unwrap();
        assert!(super::retained_repository_key(data.path()).is_err());
        assert!(std::fs::read(data.path().join(KEY_FILE)).unwrap() == b"corrupt");
    }
}
