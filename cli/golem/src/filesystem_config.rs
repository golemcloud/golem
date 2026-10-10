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
use figment::providers::{Format, Serialized, Toml};
use golem_worker_executor::services::golem_config::{
    FilesystemSnapshotRepositoryKey, FilesystemSnapshotsConfig, FilesystemStorageConfig,
};
use rand::TryRngCore as _;
use std::io::Write as _;
use std::path::Path;

const KEY_FILE: &str = "filesystem-snapshots.repository-key";

/// Reads local filesystem settings and supplies the retained key only when none is configured.
pub(crate) fn prepare(
    data_dir: &Path,
    config_file: &Path,
    agent_root: Option<&Path>,
    environment: impl figment::Provider,
) -> anyhow::Result<(FilesystemStorageConfig, FilesystemSnapshotsConfig)> {
    let storage_mode = if cfg!(target_os = "macos") {
        "Apfs"
    } else if agent_root.is_some() {
        "Directory"
    } else {
        "Temporary"
    };
    let snapshots_mode = if cfg!(target_os = "macos") {
        "Managed"
    } else {
        "Disabled"
    };
    let mut figment = Figment::from(Serialized::defaults(serde_json::json!({
        "filesystem_storage": FilesystemStorageConfig::default(),
        "filesystem_snapshots": FilesystemSnapshotsConfig::default(),
    })))
    .merge(Serialized::defaults(serde_json::json!({
        "filesystem_storage": { "mode": { "type": storage_mode } },
        "filesystem_snapshots": { "type": snapshots_mode },
    })));
    match std::fs::symlink_metadata(config_file) {
        Ok(_) => figment = figment.merge(Toml::file_exact(config_file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to inspect the configuration at {}",
                    config_file.display()
                )
            });
        }
    }
    figment = figment.merge(environment);
    if figment.extract_inner::<String>("filesystem_storage.mode.type")? != "Temporary" {
        let root = agent_root.map(Box::<Path>::from).or_else(|| {
            cfg!(target_os = "macos").then(|| data_dir.join("agents").into_boxed_path())
        });
        if let Some(root) = root {
            // Root defaults must not introduce a config table for Temporary storage.
            figment = figment.join(Serialized::defaults(serde_json::json!({
                "filesystem_storage": { "mode": { "config": { "root": root } } }
            })));
        }
    }
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
    let key = std::fs::read_to_string(path)
        .with_context(|| {
            format!(
                "Failed to read the filesystem snapshot key at {}",
                path.display()
            )
        })?
        .into_boxed_str();
    FilesystemSnapshotRepositoryKey::parse(&key).map_err(anyhow::Error::msg)?;
    Ok(key)
}

fn local_key(data_dir: &Path) -> anyhow::Result<Box<str>> {
    let path = data_dir.join(KEY_FILE).into_boxed_path();
    match read_key(&path) {
        Ok(key) => return Ok(key),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(data_dir)?;
    let mut bytes = [0; 64];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .context("Failed to generate the local filesystem snapshot key")?;
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
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => read_key(&path),
        Err(error) => Err(error.error).context("Failed to keep the local filesystem snapshot key"),
    }
}

#[cfg(test)]
mod tests {
    use golem_worker_executor::services::golem_config::{
        FilesystemSnapshotsConfig, FilesystemStorageMode,
    };
    use test_r::test;

    fn no_environment() -> figment::providers::Serialized<serde_json::Value> {
        figment::providers::Serialized::defaults(serde_json::json!({}))
    }

    fn config_file(contents: &str) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), contents).unwrap();
        file
    }

    #[test]
    fn local_server_environment_root_overrides_toml_and_the_requested_root() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Directory\"\n\
             [filesystem_storage.mode.config]\nroot = \"toml-agents\"\n\
             [filesystem_snapshots]\ntype = \"Disabled\"",
        );
        let environment = figment::providers::Serialized::defaults(serde_json::json!({
            "filesystem_storage": { "mode": { "config": { "root": "env-agents" } } }
        }));
        let (storage, snapshots) = super::prepare(
            data.path(),
            config.path(),
            Some(std::path::Path::new("cli-agents")),
            environment,
        )
        .unwrap();
        assert_eq!(
            storage.mode,
            FilesystemStorageMode::Directory {
                root: std::path::Path::new("env-agents").into(),
            }
        );
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_environment_can_disable_storage_and_snapshots_without_a_root_payload() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\n\
             [filesystem_snapshots]\ntype = \"Managed\"",
        );
        let environment = figment::providers::Serialized::defaults(serde_json::json!({
            "filesystem_storage": { "mode": { "type": "Temporary" } },
            "filesystem_snapshots": { "type": "Disabled" }
        }));
        let (storage, snapshots) = super::prepare(
            data.path(),
            config.path(),
            Some(std::path::Path::new("cli-agents")),
            environment,
        )
        .unwrap();
        assert_eq!(storage.mode, FilesystemStorageMode::Temporary);
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn local_server_uses_defaults_when_config_file_is_absent() {
        let data = tempfile::tempdir().unwrap();
        let config = data
            .path()
            .join("absent-worker-executor.toml")
            .into_boxed_path();
        let (storage, snapshots) =
            super::prepare(data.path(), &config, None, no_environment()).unwrap();
        assert_eq!(storage.mode, FilesystemStorageMode::Temporary);
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_server_mac_defaults_enable_apfs_and_keep_the_key() {
        let data = tempfile::tempdir().unwrap();
        let config = data
            .path()
            .join("absent-worker-executor.toml")
            .into_boxed_path();
        let (storage, first) =
            super::prepare(data.path(), &config, None, no_environment()).unwrap();
        assert_eq!(
            storage.mode,
            FilesystemStorageMode::Apfs {
                root: data.path().join("agents").into_boxed_path(),
            }
        );
        let FilesystemSnapshotsConfig::Managed(first) = first else {
            panic!("macOS local defaults must enable filesystem snapshots")
        };
        let (_, second) = super::prepare(data.path(), &config, None, no_environment()).unwrap();
        let FilesystemSnapshotsConfig::Managed(second) = second else {
            panic!("later starts must keep filesystem snapshots enabled")
        };
        assert!(first.repository_key().bytes() == second.repository_key().bytes());
        assert_eq!(
            std::fs::metadata(data.path().join(super::KEY_FILE))
                .unwrap()
                .len(),
            128
        );
    }

    #[test]
    fn local_server_uses_the_requested_agent_root() {
        let data = tempfile::tempdir().unwrap();
        let config = data
            .path()
            .join("absent-worker-executor.toml")
            .into_boxed_path();
        let root = data.path().join("custom-agents").into_boxed_path();
        let (storage, snapshots) =
            super::prepare(data.path(), &config, Some(&root), no_environment()).unwrap();
        #[cfg(target_os = "macos")]
        {
            assert_eq!(storage.mode, FilesystemStorageMode::Apfs { root });
            assert!(matches!(snapshots, FilesystemSnapshotsConfig::Managed(_)));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(storage.mode, FilesystemStorageMode::Directory { root });
            assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        }
    }

    #[test]
    fn local_server_rejects_a_non_table_storage_config_without_making_a_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\nconfig = 42\n\
             [filesystem_snapshots]\ntype = \"Managed\"",
        );
        assert!(super::prepare(data.path(), config.path(), None, no_environment()).is_err());
        assert!(!data.path().join(super::KEY_FILE).exists());
    }

    #[test]
    fn local_server_rejects_malformed_config_without_making_a_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file("[filesystem_snapshots");
        assert!(super::prepare(data.path(), config.path(), None, no_environment()).is_err());
        assert!(!data.path().join(super::KEY_FILE).exists());
    }

    #[test]
    fn local_server_rejects_a_config_directory_despite_valid_environment_settings() {
        let config = tempfile::tempdir().unwrap();
        let data = config.path().join("data").into_boxed_path();
        let error = super::prepare(
            &data,
            config.path(),
            None,
            figment::providers::Serialized::defaults(serde_json::json!({
                "filesystem_storage": { "mode": { "type": "Temporary" } },
                "filesystem_snapshots": { "type": "Disabled" },
            })),
        )
        .unwrap_err();
        assert!(error.to_string().contains(config.path().to_str().unwrap()));
        assert!(!data.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_server_rejects_a_config_path_with_a_file_parent_before_key_effects() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("file").into_boxed_path();
        std::fs::write(&parent, b"not a directory").unwrap();
        let data = root.path().join("data").into_boxed_path();
        let config = parent.join("worker-executor.toml").into_boxed_path();
        let error = super::prepare(&data, &config, None, no_environment()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to inspect the configuration")
        );
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotADirectory,
        );
        assert!(!data.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_server_rejects_a_dangling_config_symlink() {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("worker-executor.toml").into_boxed_path();
        std::os::unix::fs::symlink(data.path().join("absent.toml"), &config).unwrap();
        assert!(super::prepare(data.path(), &config, None, no_environment()).is_err());
        assert!(!data.path().join(super::KEY_FILE).exists());
    }

    #[test]
    fn local_server_keeps_snapshots_disabled_without_writing_a_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Temporary\"\n\
             [filesystem_snapshots]\ntype = \"Disabled\"",
        );
        let (storage, snapshots) =
            super::prepare(data.path(), config.path(), None, no_environment()).unwrap();
        assert_eq!(storage.mode, FilesystemStorageMode::Temporary);
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_environment_key_overrides_toml_without_reading_or_creating_a_kept_key() {
        let config = config_file(&format!(
            "[filesystem_snapshots]\ntype = \"Managed\"\n\
             [filesystem_snapshots.config]\nrepository_key = \"{}\"\nsave_threads = 2",
            "2a".repeat(64)
        ));
        [false, true].into_iter().for_each(|has_kept_key| {
            let data = tempfile::tempdir().unwrap();
            let key_path = data.path().join(super::KEY_FILE).into_boxed_path();
            if has_kept_key {
                std::fs::write(&key_path, b"corrupt").unwrap();
            }
            let environment = figment::providers::Serialized::defaults(serde_json::json!({
                "filesystem_snapshots": { "config": {
                    "repository_key": "3b".repeat(64),
                    "save_threads": 3,
                } }
            }));
            let (_, snapshots) =
                super::prepare(data.path(), config.path(), None, environment).unwrap();
            let FilesystemSnapshotsConfig::Managed(store) = snapshots else {
                panic!("configured snapshots must be enabled")
            };
            assert!(store.repository_key().bytes() == &[0x3b; 64]);
            assert_eq!(store.save_threads().get(), 3);
            if has_kept_key {
                assert!(std::fs::read(&key_path).unwrap() == b"corrupt");
            } else {
                assert!(!key_path.exists());
            }
        });
    }

    #[test]
    fn local_server_uses_a_configured_key_instead_of_the_kept_key() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(super::KEY_FILE), b"corrupt").unwrap();
        let config = config_file(&format!(
            "[filesystem_snapshots]\ntype = \"Managed\"\n\
             [filesystem_snapshots.config]\nrepository_key = \"{}\"",
            "2a".repeat(64)
        ));
        let (_, snapshots) =
            super::prepare(data.path(), config.path(), None, no_environment()).unwrap();
        let FilesystemSnapshotsConfig::Managed(store) = snapshots else {
            panic!("configured snapshots must be enabled")
        };
        assert!(store.repository_key().bytes() == &[0x2a; 64]);
        assert!(std::fs::read(data.path().join(super::KEY_FILE)).unwrap() == b"corrupt");
    }

    #[test]
    fn local_server_kept_key_read_failures_do_not_replace_the_entry() {
        let config = config_file("[filesystem_snapshots]\ntype = \"Managed\"");
        [false, true].into_iter().for_each(|is_directory| {
            let data = tempfile::tempdir().unwrap();
            let key = data.path().join(super::KEY_FILE).into_boxed_path();
            if is_directory {
                std::fs::create_dir(&key).unwrap();
            } else {
                std::fs::write(&key, [0xff]).unwrap();
            }
            let error =
                super::prepare(data.path(), config.path(), None, no_environment()).unwrap_err();
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

    #[test]
    fn local_server_kept_key_is_the_same_for_concurrent_starts() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file("[filesystem_snapshots]\ntype = \"Managed\"");
        let barrier = std::sync::Barrier::new(4);
        let keys = std::thread::scope(|scope| {
            let threads = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let (_, snapshots) =
                            super::prepare(data.path(), config.path(), None, no_environment())
                                .unwrap();
                        let FilesystemSnapshotsConfig::Managed(store) = snapshots else {
                            panic!("configured snapshots must be enabled")
                        };
                        *store.repository_key().bytes()
                    })
                })
                .collect::<Box<[_]>>();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Box<[_]>>()
        });
        let key_path = data.path().join(super::KEY_FILE).into_boxed_path();
        let retained =
            golem_worker_executor::services::golem_config::FilesystemSnapshotRepositoryKey::parse(
                &std::fs::read_to_string(&key_path).unwrap(),
            )
            .unwrap();
        assert!(keys.iter().all(|key| key == retained.bytes()));
        let (_, later) =
            super::prepare(data.path(), config.path(), None, no_environment()).unwrap();
        let FilesystemSnapshotsConfig::Managed(later) = later else {
            panic!("later starts must keep snapshots enabled")
        };
        assert!(later.repository_key().bytes() == retained.bytes());
    }

    #[test]
    fn local_server_refuses_a_corrupt_kept_key_and_does_not_replace_it() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(super::KEY_FILE), b"corrupt").unwrap();
        let config = config_file("[filesystem_snapshots]\ntype = \"Managed\"");
        assert!(super::prepare(data.path(), config.path(), None, no_environment()).is_err());
        assert!(std::fs::read(data.path().join(super::KEY_FILE)).unwrap() == b"corrupt");
    }

    #[test]
    fn local_server_rejects_invalid_environment_settings_before_key_effects() {
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Directory\"\n\
             [filesystem_storage.mode.config]\nroot = \"toml-agents\"\n\
             [filesystem_snapshots]\ntype = \"Managed\"\n\
             [filesystem_snapshots.config]\nsave_threads = 2",
        );
        [
            (serde_json::json!({"filesystem_storage": {"mode": {"type": "Unknown"}}}), "filesystem_storage"),
            (serde_json::json!({"filesystem_storage": {"mode": {"config": {"root": 42}}}}), "filesystem_storage"),
            (serde_json::json!({"filesystem_storage": {"mode": {"config": 42}}}), "filesystem_storage"),
            (serde_json::json!({"filesystem_snapshots": {"type": "Unknown"}}), "filesystem_snapshots"),
            (serde_json::json!({"filesystem_snapshots": {"config": 42}}), "filesystem_snapshots.config"),
            (serde_json::json!({"filesystem_snapshots": {"config": {"save_threads": 0}}}), "save_threads"),
            (serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": ""}}}), "repository_key"),
            (serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": "2a"}}}), "repository_key"),
            (serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": "zz".repeat(64)}}}), "repository_key"),
            (serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": 42}}}), "invalid type: found unsigned int `42`, expected a string"),
        ]
        .into_iter()
        .for_each(|(settings, expected_field)| {
            [false, true].into_iter().for_each(|has_kept_key| {
                let root = tempfile::tempdir().unwrap();
                let data = root.path().join("data").into_boxed_path();
                let key = data.join(super::KEY_FILE).into_boxed_path();
                if has_kept_key {
                    std::fs::create_dir(&data).unwrap();
                    std::fs::write(&key, b"corrupt retained key").unwrap();
                }
                let error = super::prepare(
                    &data,
                    config.path(),
                    Some(std::path::Path::new("cli-agents")),
                    figment::providers::Serialized::defaults(settings.clone()),
                )
                .unwrap_err();
                assert!(
                    error.to_string().contains(expected_field),
                    "the override error must identify {expected_field}: {error}"
                );
                if has_kept_key {
                    assert!(std::fs::read(&key).unwrap() == b"corrupt retained key");
                } else {
                    assert!(!data.exists(), "invalid overrides must not create the data directory");
                }
            });
        });
    }

    #[test]
    fn local_server_rejects_invalid_settings_before_making_a_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_snapshots]\ntype = \"Managed\"\n\
             [filesystem_snapshots.config]\nsave_threads = 0",
        );
        assert!(super::prepare(data.path(), config.path(), None, no_environment()).is_err());
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_uses_normal_filesystem_settings_and_keeps_the_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\n\
             [filesystem_storage.mode.config]\nroot = \"local-agents\"\n\
             [filesystem_snapshots]\ntype = \"Managed\"",
        );
        let cli_root = std::path::Path::new("cli-agents");
        let (storage, first) =
            super::prepare(data.path(), config.path(), Some(cli_root), no_environment()).unwrap();
        let (_, second) =
            super::prepare(data.path(), config.path(), Some(cli_root), no_environment()).unwrap();
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
        let key = data
            .path()
            .join("filesystem-snapshots.repository-key")
            .into_boxed_path();
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
