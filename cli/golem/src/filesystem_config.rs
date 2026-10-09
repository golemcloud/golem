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

use anyhow::Context;
use figment::Figment;
use figment::providers::Serialized;
use golem_worker_executor::services::golem_config::{
    FilesystemSnapshotRepositoryKey, FilesystemSnapshotsConfig, FilesystemStorageConfig,
};
use rand::TryRngCore as _;
use std::io::Write as _;
use std::path::Path;

const KEY_FILE: &str = "filesystem-snapshots.repository-key";

/// Reads the normal filesystem configuration and supplies the local key only when none is set.
pub(crate) fn prepare(
    data_dir: &Path,
    mut figment: Figment,
) -> anyhow::Result<(FilesystemStorageConfig, FilesystemSnapshotsConfig)> {
    let storage = figment.extract_inner("filesystem_storage")?;
    if figment.extract_inner::<String>("filesystem_snapshots.type")? == "Managed"
        && figment
            .find_value("filesystem_snapshots.config.repository_key")
            .is_err()
    {
        if figment
            .find_value("filesystem_snapshots.config")?
            .as_dict()
            .is_none()
        {
            anyhow::bail!("filesystem_snapshots.config must be a table");
        }
        with_key(figment.clone(), &"00".repeat(64))
            .extract_inner::<FilesystemSnapshotsConfig>("filesystem_snapshots")?;
        figment = with_key(figment, &local_key(data_dir)?);
    }
    let snapshots = figment.extract_inner("filesystem_snapshots")?;
    Ok((storage, snapshots))
}

fn with_key(figment: Figment, key: &str) -> Figment {
    figment.merge(Serialized::defaults(serde_json::json!({
        "filesystem_snapshots": { "config": { "repository_key": key } }
    })))
}

fn read_key(path: &Path) -> anyhow::Result<Box<str>> {
    let key = std::fs::read_to_string(path).with_context(|| {
        format!(
            "Failed to read the filesystem snapshot key at {}",
            path.display()
        )
    })?;
    FilesystemSnapshotRepositoryKey::parse(&key).map_err(anyhow::Error::msg)?;
    Ok(key.into_boxed_str())
}

fn local_key(data_dir: &Path) -> anyhow::Result<Box<str>> {
    let path = data_dir.join(KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(key) => {
            FilesystemSnapshotRepositoryKey::parse(&key).map_err(anyhow::Error::msg)?;
            return Ok(key.into_boxed_str());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).context("Failed to read the local filesystem snapshot key");
        }
    }
    std::fs::create_dir_all(data_dir)?;
    let mut bytes = [0; 64];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .context("Failed to generate the local filesystem snapshot key")?;
    let key = hex::encode(bytes);
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
            Ok(key.into_boxed_str())
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => read_key(&path),
        Err(error) => Err(error.error).context("Failed to keep the local filesystem snapshot key"),
    }
}

#[cfg(test)]
mod tests {
    use figment::Figment;
    use figment::providers::{Format, Serialized, Toml};
    use golem_worker_executor::services::golem_config::{
        FilesystemSnapshotsConfig, FilesystemStorageMode, GolemConfig,
    };
    use test_r::test;

    #[test]
    fn local_server_keeps_snapshots_disabled_without_writing_a_key() {
        let data = tempfile::tempdir().unwrap();
        let (storage, snapshots) = super::prepare(
            data.path(),
            Figment::from(Serialized::defaults(GolemConfig::default())),
        )
        .unwrap();
        assert_eq!(storage.mode, FilesystemStorageMode::Temporary);
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_uses_a_configured_key_instead_of_the_kept_key() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(super::KEY_FILE), b"corrupt").unwrap();
        let figment = Figment::from(Serialized::defaults(GolemConfig::default()))
            .merge(Serialized::defaults(serde_json::json!({
                "filesystem_snapshots": { "type": "Managed", "config": { "repository_key": "2a".repeat(64) } }
            })));
        let (_, snapshots) = super::prepare(data.path(), figment).unwrap();
        let FilesystemSnapshotsConfig::Managed(store) = snapshots else {
            panic!("configured snapshots must be enabled")
        };
        assert!(store.repository_key().bytes() == &[0x2a; 64]);
        assert!(std::fs::read(data.path().join(super::KEY_FILE)).unwrap() == b"corrupt");
    }

    #[test]
    fn local_server_refuses_a_corrupt_kept_key_and_does_not_replace_it() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(super::KEY_FILE), b"corrupt").unwrap();
        let figment = Figment::from(Serialized::defaults(GolemConfig::default()))
            .merge(Toml::string("[filesystem_snapshots]\ntype = \"Managed\""));
        assert!(super::prepare(data.path(), figment).is_err());
        assert!(std::fs::read(data.path().join(super::KEY_FILE)).unwrap() == b"corrupt");
    }

    #[test]
    fn local_server_rejects_invalid_settings_before_making_a_key() {
        let data = tempfile::tempdir().unwrap();
        let figment = Figment::from(Serialized::defaults(GolemConfig::default()))
            .merge(Toml::string("[filesystem_snapshots]\ntype = \"Managed\"\n[filesystem_snapshots.config]\nsave_threads = 0"));
        assert!(super::prepare(data.path(), figment).is_err());
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_uses_normal_filesystem_settings_and_keeps_the_key() {
        let data = tempfile::tempdir().unwrap();
        let settings = || {
            Figment::from(Serialized::defaults(GolemConfig::default())).merge(Toml::string(
                "[filesystem_storage.mode]\ntype = \"Apfs\"\n\
                 [filesystem_storage.mode.config]\nroot = \"local-agents\"\n\
                 [filesystem_snapshots]\ntype = \"Managed\"",
            ))
        };
        let (storage, first) = super::prepare(data.path(), settings()).unwrap();
        let (_, second) = super::prepare(data.path(), settings()).unwrap();
        assert_eq!(
            storage.mode,
            FilesystemStorageMode::Apfs {
                root: std::path::Path::new("local-agents").into()
            }
        );
        let FilesystemSnapshotsConfig::Managed(first) = first else {
            panic!("normal configuration must enable snapshots")
        };
        let FilesystemSnapshotsConfig::Managed(second) = second else {
            panic!("normal configuration must enable snapshots")
        };
        assert!(
            first.repository_key().bytes() == second.repository_key().bytes(),
            "later starts must keep the key"
        );
        let key = data.path().join("filesystem-snapshots.repository-key");
        assert_eq!(std::fs::metadata(&key).unwrap().len(), 128);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
