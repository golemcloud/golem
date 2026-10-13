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
    FilesystemSnapshotRepositoryKey, FilesystemSnapshotsConfig, FilesystemSnapshotsKind,
    FilesystemStorageConfig, FilesystemStorageModeKind,
};
use rand::TryRngCore as _;
use std::io::Write as _;
use std::path::Path;

const KEY_FILE: &str = "filesystem-snapshots.repository-key";
const VALIDATION_ONLY_REPOSITORY_KEY: &str = concat!(
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
);

#[derive(Clone, Copy)]
enum LocalPlatform {
    MacOs,
    Other,
}

impl LocalPlatform {
    fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

struct LocalDefaults {
    storage_mode: FilesystemStorageModeKind,
    snapshots_mode: FilesystemSnapshotsKind,
    root: Option<Box<Path>>,
}

fn local_defaults(
    platform: LocalPlatform,
    agent_root: Option<&Path>,
    data_dir: &Path,
) -> LocalDefaults {
    let (storage_mode, snapshots_mode) = match platform {
        LocalPlatform::MacOs => (
            FilesystemStorageModeKind::Apfs,
            FilesystemSnapshotsKind::Enabled,
        ),
        LocalPlatform::Other if agent_root.is_some() => (
            FilesystemStorageModeKind::Directory,
            FilesystemSnapshotsKind::Disabled,
        ),
        LocalPlatform::Other => (
            FilesystemStorageModeKind::Temporary,
            FilesystemSnapshotsKind::Disabled,
        ),
    };
    let root = agent_root
        .map(Box::<Path>::from)
        .or_else(|| match platform {
            LocalPlatform::MacOs => Some(data_dir.join("agents").into_boxed_path()),
            LocalPlatform::Other => None,
        });
    LocalDefaults {
        storage_mode,
        snapshots_mode,
        root,
    }
}

/// Reads local filesystem settings and supplies the retained repository key only when none is configured.
pub(crate) fn prepare(
    data_dir: &Path,
    config_file: &Path,
    agent_root: Option<&Path>,
    environment: impl figment::Provider,
) -> anyhow::Result<(FilesystemStorageConfig, FilesystemSnapshotsConfig)> {
    let platform = LocalPlatform::native();
    prepare_with_defaults(
        data_dir,
        config_file,
        local_defaults(platform, agent_root, data_dir),
        environment,
    )
}

fn root_default(mode: FilesystemStorageModeKind, root: Option<Box<Path>>) -> Option<Box<Path>> {
    match mode {
        FilesystemStorageModeKind::Temporary => None,
        FilesystemStorageModeKind::Directory
        | FilesystemStorageModeKind::ManagedXfs
        | FilesystemStorageModeKind::ReflinkXfs
        | FilesystemStorageModeKind::Apfs => root,
    }
}

fn repository_key_needed(mode: FilesystemSnapshotsKind, configured: bool) -> bool {
    match mode {
        FilesystemSnapshotsKind::Enabled => !configured,
        FilesystemSnapshotsKind::Disabled => false,
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ExactPathAction {
    Read,
    AllowMissing,
}

fn exact_path_action(inspection: std::io::Result<()>) -> std::io::Result<ExactPathAction> {
    match inspection {
        Ok(()) => Ok(ExactPathAction::Read),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ExactPathAction::AllowMissing)
        }
        Err(error) => Err(error),
    }
}

fn prepare_with_defaults(
    data_dir: &Path,
    config_file: &Path,
    defaults: LocalDefaults,
    environment: impl figment::Provider,
) -> anyhow::Result<(FilesystemStorageConfig, FilesystemSnapshotsConfig)> {
    let mut figment = Figment::from(Serialized::defaults(serde_json::json!({
        "filesystem_storage": FilesystemStorageConfig::default(),
        "filesystem_snapshots": FilesystemSnapshotsConfig::default(),
    })))
    .merge(Serialized::defaults(serde_json::json!({
        "filesystem_storage": { "mode": { "type": defaults.storage_mode } },
        "filesystem_snapshots": { "type": defaults.snapshots_mode },
    })));
    let config_action = exact_path_action(std::fs::symlink_metadata(config_file).map(|_| ()))
        .with_context(|| {
            format!(
                "Failed to inspect the configuration at {}",
                config_file.display()
            )
        })?;
    match config_action {
        ExactPathAction::Read => figment = figment.merge(Toml::file_exact(config_file)),
        ExactPathAction::AllowMissing => {}
    }
    figment = figment.merge(environment);
    let storage_mode =
        figment.extract_inner::<FilesystemStorageModeKind>("filesystem_storage.mode.type")?;
    if let Some(root) = root_default(storage_mode, defaults.root) {
        // Root defaults must not introduce a config table for Temporary storage.
        figment = figment.join(Serialized::defaults(serde_json::json!({
            "filesystem_storage": { "mode": { "config": { "root": root } } }
        })));
    }
    let storage = figment.extract_inner("filesystem_storage")?;
    let snapshots_mode =
        figment.extract_inner::<FilesystemSnapshotsKind>("filesystem_snapshots.type")?;
    let configured_key = figment
        .find_value("filesystem_snapshots.config.repository_key")
        .is_ok();
    if repository_key_needed(snapshots_mode, configured_key) {
        if figment
            .find_value("filesystem_snapshots.config")?
            .as_dict()
            .is_none()
        {
            anyhow::bail!("filesystem_snapshots.config must be a table");
        }
        // Validate Enabled settings on a disposable clone before retained repository key I/O.
        with_key(figment.clone(), VALIDATION_ONLY_REPOSITORY_KEY)
            .extract_inner::<FilesystemSnapshotsConfig>("filesystem_snapshots")?;
        figment = with_key(figment, &retained_repository_key(data_dir)?);
    }
    let snapshots = figment.extract_inner("filesystem_snapshots")?;
    Ok((storage, snapshots))
}

fn with_key(figment: Figment, key: &str) -> Figment {
    figment.merge(Serialized::defaults(serde_json::json!({
        "filesystem_snapshots": { "config": { "repository_key": key } }
    })))
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

fn retained_repository_key(data_dir: &Path) -> anyhow::Result<Box<str>> {
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
    use super::LocalPlatform;
    use golem_worker_executor::services::golem_config::{
        FilesystemSnapshotsConfig, FilesystemSnapshotsKind, FilesystemStorageMode,
        FilesystemStorageModeKind,
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
    fn root_default_follows_the_selected_mode_without_configuration_io() {
        let root: Box<std::path::Path> = std::path::Path::new("requested-agents").into();
        [
            FilesystemStorageModeKind::Directory,
            FilesystemStorageModeKind::ManagedXfs,
            FilesystemStorageModeKind::ReflinkXfs,
            FilesystemStorageModeKind::Apfs,
        ]
        .into_iter()
        .for_each(|mode| {
            assert_eq!(
                super::root_default(mode, Some(root.clone())),
                Some(root.clone()),
            );
            assert!(super::root_default(mode, None).is_none());
        });
        assert!(super::root_default(FilesystemStorageModeKind::Temporary, Some(root)).is_none());
        assert!(super::root_default(FilesystemStorageModeKind::Temporary, None).is_none());
    }

    #[test]
    fn repository_key_need_depends_only_on_mode_and_key_presence() {
        [
            (FilesystemSnapshotsKind::Enabled, false, true),
            (FilesystemSnapshotsKind::Enabled, true, false),
            (FilesystemSnapshotsKind::Disabled, false, false),
            (FilesystemSnapshotsKind::Disabled, true, false),
        ]
        .into_iter()
        .for_each(|(mode, configured, expected)| {
            assert_eq!(super::repository_key_needed(mode, configured), expected);
        });
    }

    #[test]
    fn exact_path_inspection_permits_absence_only_for_not_found() {
        assert_eq!(
            super::exact_path_action(Ok(())).unwrap(),
            super::ExactPathAction::Read,
        );
        assert_eq!(
            super::exact_path_action(Err(std::io::ErrorKind::NotFound.into())).unwrap(),
            super::ExactPathAction::AllowMissing,
        );
        [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotADirectory,
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::Other,
        ]
        .into_iter()
        .for_each(|kind| {
            let error = super::exact_path_action(Err(std::io::Error::new(
                kind,
                "exact-path inspection failed",
            )))
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), "exact-path inspection failed");
        });
    }

    #[cfg(unix)]
    #[test]
    fn exact_path_inspection_preserves_the_native_error() {
        let errno = rustix::io::Errno::NOTDIR.raw_os_error();
        let error =
            super::exact_path_action(Err(std::io::Error::from_raw_os_error(errno))).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotADirectory);
        assert_eq!(error.raw_os_error(), Some(errno));
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
    fn local_server_default_policy_matches_the_platform_and_requested_root() {
        let data_dir = std::path::Path::new("local-data");
        let requested_root = std::path::Path::new("cli-agents");
        [
            (
                LocalPlatform::MacOs,
                None,
                FilesystemStorageModeKind::Apfs,
                FilesystemSnapshotsKind::Enabled,
                Some(Box::<std::path::Path>::from(std::path::Path::new(
                    "local-data/agents",
                ))),
            ),
            (
                LocalPlatform::MacOs,
                Some(requested_root),
                FilesystemStorageModeKind::Apfs,
                FilesystemSnapshotsKind::Enabled,
                Some(Box::<std::path::Path>::from(requested_root)),
            ),
            (
                LocalPlatform::Other,
                None,
                FilesystemStorageModeKind::Temporary,
                FilesystemSnapshotsKind::Disabled,
                None,
            ),
            (
                LocalPlatform::Other,
                Some(requested_root),
                FilesystemStorageModeKind::Directory,
                FilesystemSnapshotsKind::Disabled,
                Some(Box::<std::path::Path>::from(requested_root)),
            ),
        ]
        .into_iter()
        .for_each(
            |(platform, requested, storage_mode, snapshots_mode, root)| {
                let defaults = super::local_defaults(platform, requested, data_dir);
                assert_eq!(
                    (
                        defaults.storage_mode,
                        defaults.snapshots_mode,
                        defaults.root.as_deref(),
                    ),
                    (storage_mode, snapshots_mode, root.as_deref()),
                );
            },
        );
    }

    #[test]
    fn local_server_environment_root_overrides_toml_and_the_requested_root() {
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Directory\"\n\
             [filesystem_storage.mode.config]\nroot = \"toml-agents\"\n\
             [filesystem_snapshots]\ntype = \"Disabled\"",
        );
        [LocalPlatform::MacOs, LocalPlatform::Other]
            .into_iter()
            .for_each(|platform| {
                let data = tempfile::tempdir().unwrap();
                let (storage, snapshots) = super::prepare_with_defaults(
                    data.path(),
                    config.path(),
                    super::local_defaults(
                        platform,
                        Some(std::path::Path::new("cli-agents")),
                        data.path(),
                    ),
                    no_environment(),
                )
                .unwrap();
                assert_eq!(
                    storage.mode,
                    FilesystemStorageMode::Directory {
                        root: std::path::Path::new("toml-agents").into(),
                    }
                );
                assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
                let environment = figment::providers::Serialized::defaults(serde_json::json!({
                    "filesystem_storage": { "mode": { "config": { "root": "env-agents" } } }
                }));
                let (storage, snapshots) = super::prepare_with_defaults(
                    data.path(),
                    config.path(),
                    super::local_defaults(
                        platform,
                        Some(std::path::Path::new("cli-agents")),
                        data.path(),
                    ),
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
            });
    }

    #[test]
    fn local_server_environment_can_disable_storage_and_snapshots_without_a_root_payload() {
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\n\
             [filesystem_snapshots]\ntype = \"Enabled\"",
        );
        [LocalPlatform::MacOs, LocalPlatform::Other]
            .into_iter()
            .for_each(|platform| {
                let data = tempfile::tempdir().unwrap();
                let environment = figment::providers::Serialized::defaults(serde_json::json!({
                    "filesystem_storage": { "mode": { "type": "Temporary" } },
                    "filesystem_snapshots": { "type": "Disabled" }
                }));
                let (storage, snapshots) = super::prepare_with_defaults(
                    data.path(),
                    config.path(),
                    super::local_defaults(
                        platform,
                        Some(std::path::Path::new("cli-agents")),
                        data.path(),
                    ),
                    environment,
                )
                .unwrap();
                assert_eq!(storage.mode, FilesystemStorageMode::Temporary);
                assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
                assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
            });
    }

    #[test]
    fn local_server_uses_defaults_when_config_file_is_absent() {
        let data = tempfile::tempdir().unwrap();
        let config = data
            .path()
            .join("absent-worker-executor.toml")
            .into_boxed_path();
        let (storage, snapshots) = super::prepare_with_defaults(
            data.path(),
            &config,
            super::local_defaults(LocalPlatform::Other, None, data.path()),
            no_environment(),
        )
        .unwrap();
        assert_eq!(storage.mode, FilesystemStorageMode::Temporary);
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_mac_defaults_enable_apfs_and_reuse_the_retained_repository_key() {
        let data = tempfile::tempdir().unwrap();
        let config = data
            .path()
            .join("absent-worker-executor.toml")
            .into_boxed_path();
        let (storage, first) = super::prepare_with_defaults(
            data.path(),
            &config,
            super::local_defaults(LocalPlatform::MacOs, None, data.path()),
            no_environment(),
        )
        .unwrap();
        assert_eq!(
            storage.mode,
            FilesystemStorageMode::Apfs {
                root: data.path().join("agents").into_boxed_path(),
            }
        );
        let FilesystemSnapshotsConfig::Enabled(first) = first else {
            panic!("macOS local defaults must enable filesystem snapshots")
        };
        let (_, second) = super::prepare_with_defaults(
            data.path(),
            &config,
            super::local_defaults(LocalPlatform::MacOs, None, data.path()),
            no_environment(),
        )
        .unwrap();
        let FilesystemSnapshotsConfig::Enabled(second) = second else {
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
        [LocalPlatform::MacOs, LocalPlatform::Other]
            .into_iter()
            .for_each(|platform| {
                let data = tempfile::tempdir().unwrap();
                let config = data
                    .path()
                    .join("absent-worker-executor.toml")
                    .into_boxed_path();
                let root = data.path().join("custom-agents").into_boxed_path();
                let (storage, snapshots) = super::prepare_with_defaults(
                    data.path(),
                    &config,
                    super::local_defaults(platform, Some(&root), data.path()),
                    no_environment(),
                )
                .unwrap();
                match platform {
                    LocalPlatform::MacOs => {
                        assert_eq!(storage.mode, FilesystemStorageMode::Apfs { root });
                        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Enabled(_)));
                    }
                    LocalPlatform::Other => {
                        assert_eq!(storage.mode, FilesystemStorageMode::Directory { root });
                        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
                    }
                }
            });
    }

    #[test]
    fn local_server_temporary_mode_rejects_an_explicit_root_payload() {
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\n\
             [filesystem_storage.mode.config]\nroot = \"toml-agents\"\n\
             [filesystem_snapshots]\ntype = \"Enabled\"",
        );
        [LocalPlatform::MacOs, LocalPlatform::Other]
            .into_iter()
            .for_each(|platform| {
                let data = tempfile::tempdir().unwrap();
                let environment = figment::providers::Serialized::defaults(serde_json::json!({
                    "filesystem_storage": { "mode": { "type": "Temporary" } },
                    "filesystem_snapshots": { "type": "Disabled" }
                }));
                let error = super::prepare_with_defaults(
                    data.path(),
                    config.path(),
                    super::local_defaults(
                        platform,
                        Some(std::path::Path::new("cli-agents")),
                        data.path(),
                    ),
                    environment,
                )
                .unwrap_err();
                assert!(error.to_string().contains("filesystem_storage"));
                assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
            });
    }

    #[test]
    fn local_server_mac_root_default_applies_to_an_explicit_rooted_mode() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Directory\"\n\
             [filesystem_snapshots]\ntype = \"Disabled\"",
        );
        let (storage, snapshots) = super::prepare_with_defaults(
            data.path(),
            config.path(),
            super::local_defaults(LocalPlatform::MacOs, None, data.path()),
            no_environment(),
        )
        .unwrap();
        assert_eq!(
            storage.mode,
            FilesystemStorageMode::Directory {
                root: data.path().join("agents").into_boxed_path(),
            }
        );
        assert!(matches!(snapshots, FilesystemSnapshotsConfig::Disabled(_)));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_other_rooted_mode_requires_a_root_before_snapshot_validation() {
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Directory\"\n\
             [filesystem_storage.mode.config]\n\
             [filesystem_snapshots]\ntype = \"Enabled\"\n\
             [filesystem_snapshots.config]\nsave_threads = 0",
        );
        [false, true]
            .into_iter()
            .for_each(|has_retained_repository_key| {
                let root = tempfile::tempdir().unwrap();
                let data = root.path().join("data").into_boxed_path();
                let key = data.join(super::KEY_FILE).into_boxed_path();
                if has_retained_repository_key {
                    std::fs::create_dir(&data).unwrap();
                    std::fs::write(&key, b"corrupt retained key").unwrap();
                }
                let error = super::prepare_with_defaults(
                    &data,
                    config.path(),
                    super::local_defaults(LocalPlatform::Other, None, &data),
                    no_environment(),
                )
                .unwrap_err();
                let message = error.to_string().into_boxed_str();
                assert!(message.contains("filesystem_storage"));
                assert!(message.contains("root"));
                assert!(!message.contains("save_threads"));
                if has_retained_repository_key {
                    assert!(std::fs::read(&key).unwrap() == b"corrupt retained key");
                } else {
                    assert!(!data.exists());
                }
            });
    }

    #[test]
    fn local_server_rejects_a_non_table_storage_config_without_making_a_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\nconfig = 42\n\
             [filesystem_snapshots]\ntype = \"Enabled\"",
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
    fn local_server_environment_key_overrides_toml_without_reading_or_creating_a_retained_repository_key()
     {
        let config = config_file(&format!(
            "[filesystem_snapshots]\ntype = \"Enabled\"\n\
             [filesystem_snapshots.config]\nrepository_key = \"{}\"\nsave_threads = 2",
            "2a".repeat(64)
        ));
        [false, true]
            .into_iter()
            .for_each(|has_retained_repository_key| {
                let data = tempfile::tempdir().unwrap();
                let key_path = data.path().join(super::KEY_FILE).into_boxed_path();
                if has_retained_repository_key {
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
                let FilesystemSnapshotsConfig::Enabled(store) = snapshots else {
                    panic!("configured snapshots must be enabled")
                };
                assert!(store.repository_key().bytes() == &[0x3b; 64]);
                assert_eq!(store.save_threads().get(), 3);
                if has_retained_repository_key {
                    assert!(std::fs::read(&key_path).unwrap() == b"corrupt");
                } else {
                    assert!(!key_path.exists());
                }
            });
    }

    #[test]
    fn local_server_uses_a_configured_key_instead_of_the_retained_repository_key() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(super::KEY_FILE), b"corrupt").unwrap();
        let config = config_file(&format!(
            "[filesystem_snapshots]\ntype = \"Enabled\"\n\
             [filesystem_snapshots.config]\nrepository_key = \"{}\"",
            "2a".repeat(64)
        ));
        let (_, snapshots) =
            super::prepare(data.path(), config.path(), None, no_environment()).unwrap();
        let FilesystemSnapshotsConfig::Enabled(store) = snapshots else {
            panic!("configured snapshots must be enabled")
        };
        assert!(store.repository_key().bytes() == &[0x2a; 64]);
        assert!(std::fs::read(data.path().join(super::KEY_FILE)).unwrap() == b"corrupt");
    }

    #[test]
    fn local_server_retained_repository_key_read_failures_do_not_replace_the_entry() {
        let config = config_file("[filesystem_snapshots]\ntype = \"Enabled\"");
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

    #[cfg(unix)]
    #[test]
    fn local_server_retained_repository_key_read_not_found_does_not_generate_a_replacement() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file("[filesystem_snapshots]\ntype = \"Enabled\"");
        let key = data.path().join(super::KEY_FILE).into_boxed_path();
        let target = data.path().join("missing-key").into_boxed_path();
        std::os::unix::fs::symlink(&target, &key).unwrap();
        let error = super::prepare(data.path(), config.path(), None, no_environment()).unwrap_err();
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

    #[test]
    fn local_server_retained_repository_key_is_the_same_for_concurrent_starts() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file("[filesystem_snapshots]\ntype = \"Enabled\"");
        let barrier = std::sync::Barrier::new(4);
        let keys = std::thread::scope(|scope| {
            let threads = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let (_, snapshots) =
                            super::prepare(data.path(), config.path(), None, no_environment())
                                .unwrap();
                        let FilesystemSnapshotsConfig::Enabled(store) = snapshots else {
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
        let retained_repository_key =
            golem_worker_executor::services::golem_config::FilesystemSnapshotRepositoryKey::parse(
                &std::fs::read_to_string(&key_path).unwrap(),
            )
            .unwrap();
        assert!(
            keys.iter()
                .all(|key| key == retained_repository_key.bytes())
        );
        let (_, later) =
            super::prepare(data.path(), config.path(), None, no_environment()).unwrap();
        let FilesystemSnapshotsConfig::Enabled(later) = later else {
            panic!("later starts must keep snapshots enabled")
        };
        assert!(later.repository_key().bytes() == retained_repository_key.bytes());
    }

    #[test]
    fn local_server_refuses_a_corrupt_retained_repository_key_and_does_not_replace_it() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(super::KEY_FILE), b"corrupt").unwrap();
        let config = config_file("[filesystem_snapshots]\ntype = \"Enabled\"");
        assert!(super::prepare(data.path(), config.path(), None, no_environment()).is_err());
        assert!(std::fs::read(data.path().join(super::KEY_FILE)).unwrap() == b"corrupt");
    }

    #[test]
    fn local_server_rejects_invalid_environment_settings_before_key_effects() {
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Directory\"\n\
             [filesystem_storage.mode.config]\nroot = \"toml-agents\"\n\
             [filesystem_snapshots]\ntype = \"Enabled\"\n\
             [filesystem_snapshots.config]\nsave_threads = 2",
        );
        let cases = &[
            (
                serde_json::json!({"filesystem_storage": {"mode": {"type": "Unknown"}}}),
                "filesystem_storage",
            ),
            (
                serde_json::json!({"filesystem_storage": {"mode": {"config": {"root": 42}}}}),
                "filesystem_storage",
            ),
            (
                serde_json::json!({"filesystem_storage": {"mode": {"config": 42}}}),
                "filesystem_storage",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"type": "Unknown"}}),
                "filesystem_snapshots",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": 42}}),
                "filesystem_snapshots.config",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"save_threads": 0}}}),
                "save_threads",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"restore_reader_threads": 0}}}),
                "restore_reader_threads",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"storage_call_deadline": "0s"}}}),
                "storage_call_deadline",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"max_concurrent_uploads": 0}}}),
                "max_concurrent_uploads",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"max_concurrent_restores": 0}}}),
                "max_concurrent_restores",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"confirmation_wait": "0s"}}}),
                "confirmation_wait",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"store_check_limit": "0s"}}}),
                "store_check_limit",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"capture_wait": "0s"}}}),
                "capture_wait",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"retained_periodic_snapshots": 0}}}),
                "retained_periodic_snapshots",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"retained_update_snapshots": 0}}}),
                "retained_update_snapshots",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"max_pending_deletes_per_agent": 0}}}),
                "max_pending_deletes_per_agent",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": ""}}}),
                "repository_key",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": "2a"}}}),
                "repository_key",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": "zz".repeat(64)}}}),
                "repository_key",
            ),
            (
                serde_json::json!({"filesystem_snapshots": {"config": {"repository_key": 42}}}),
                "invalid type: found unsigned int `42`, expected a string",
            ),
        ];
        cases.iter().for_each(|(settings, expected_field)| {
            [false, true]
                .into_iter()
                .for_each(|has_retained_repository_key| {
                    let root = tempfile::tempdir().unwrap();
                    let data = root.path().join("data").into_boxed_path();
                    let key = data.join(super::KEY_FILE).into_boxed_path();
                    if has_retained_repository_key {
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
                        error.to_string().contains(*expected_field),
                        "the override error must identify {expected_field}: {error}"
                    );
                    if has_retained_repository_key {
                        assert!(std::fs::read(&key).unwrap() == b"corrupt retained key");
                    } else {
                        assert!(
                            !data.exists(),
                            "invalid overrides must not create the data directory"
                        );
                    }
                });
        });
    }

    #[test]
    fn local_server_rejects_invalid_settings_before_making_a_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_snapshots]\ntype = \"Enabled\"\n\
             [filesystem_snapshots.config]\nsave_threads = 0",
        );
        assert!(super::prepare(data.path(), config.path(), None, no_environment()).is_err());
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_server_uses_normal_filesystem_settings_and_reuses_the_retained_repository_key() {
        let data = tempfile::tempdir().unwrap();
        let config = config_file(
            "[filesystem_storage.mode]\ntype = \"Apfs\"\n\
             [filesystem_storage.mode.config]\nroot = \"local-agents\"\n\
             [filesystem_snapshots]\ntype = \"Enabled\"",
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
        let FilesystemSnapshotsConfig::Enabled(first) = first else {
            panic!("normal configuration must enable snapshots")
        };
        let FilesystemSnapshotsConfig::Enabled(second) = second else {
            panic!("normal configuration must enable snapshots")
        };
        assert!(
            first.repository_key().bytes() == second.repository_key().bytes(),
            "later starts must use the retained repository key"
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
