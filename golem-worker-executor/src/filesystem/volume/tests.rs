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

use super::*;
use std::path::PathBuf;
use test_r::test;

#[test]
fn volume_handles_share_one_bound_state() {
    let volume = FilesystemVolume::unmanaged_development();
    let shared = volume.clone();
    assert!(Arc::ptr_eq(&volume.state, &shared.state));
    assert_eq!(
        std::mem::size_of::<FilesystemVolume>(),
        std::mem::size_of::<usize>()
    );
}

#[test]
fn opening_directory_storage_does_not_provision_sandboxes_or_executor_directories() {
    let root = tempfile::tempdir().unwrap();
    let volume = FilesystemVolume::open(
        &FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        RetryConfig::default(),
    )
    .unwrap();
    assert_eq!(volume.root_location().0.as_deref(), Some(root.path()));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

/// A synthetic copy-on-write volume for the rules that read only its mode.
fn copy_on_write_volume() -> FilesystemVolume {
    let file = tempfile::NamedTempFile::new().unwrap();
    FilesystemVolume::copy_on_write(
        Arc::new(file.reopen().unwrap()),
        #[cfg(target_os = "linux")]
        FilesystemIdentity { device: 1 },
    )
}

#[test]
fn a_development_volume_mode_states_its_facts() {
    assert_eq!(
        volume_facts(&FilesystemVolumeMode::UnmanagedDevelopment),
        VolumeFacts {
            copies_on_write: false,
            storage_profile: NativeStorageProfile::Unknown,
            file_copy_mode: FileCopyMode::Buffered,
        }
    );
}

#[test]
fn a_copy_on_write_volume_mode_states_its_facts() {
    assert_eq!(
        volume_facts(&copy_on_write_volume().state.mode),
        VolumeFacts {
            copies_on_write: true,
            storage_profile: NativeStorageProfile::KnownLocal,
            file_copy_mode: FileCopyMode::Reflink,
        }
    );
}

#[test]
fn storage_profile_follows_the_volume() {
    assert_eq!(
        FilesystemVolume::unmanaged_development().storage_profile(),
        NativeStorageProfile::Unknown
    );
    assert_eq!(
        copy_on_write_volume().storage_profile(),
        NativeStorageProfile::KnownLocal
    );
}

#[test]
fn a_development_volume_makes_no_copy_on_write_copies() {
    assert!(!FilesystemVolume::unmanaged_development().copies_on_write());
}

#[test]
fn a_copy_on_write_volume_makes_copy_on_write_copies() {
    assert!(copy_on_write_volume().copies_on_write());
}

#[test]
fn space_uses_fragment_size_and_executor_available_counts() {
    let space = space_from_values(10, 4, 4096, 20, 7).unwrap();

    assert_eq!(
        space,
        FilesystemSpace::Observed {
            total_bytes: 40_960,
            available_bytes: 16_384,
            total_filesystem_objects: 20,
            available_filesystem_objects: 7,
        }
    );
}

#[test]
fn space_rejects_byte_overflow() {
    assert!(space_from_values(u64::MAX, 1, 2, 0, 0).is_err());
    assert!(space_from_values(1, u64::MAX, 2, 0, 0).is_err());
}

#[test]
fn file_copy_mode_follows_the_volume() {
    assert_eq!(
        FilesystemVolume::unmanaged_development().file_copy_mode(),
        FileCopyMode::Buffered
    );
    assert_eq!(
        copy_on_write_volume().file_copy_mode(),
        FileCopyMode::Reflink
    );
}

#[test]
async fn unmanaged_volume_is_unlimited_without_an_existing_root() {
    let provisioning = FilesystemVolume::open(
        &crate::services::golem_config::FilesystemStorageMode::Directory {
            root: PathBuf::from("/definitely/not/observed").into(),
        },
        golem_common::model::RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(
        observe_space(&provisioning).await.unwrap(),
        FilesystemSpace::Unlimited
    );
}

#[test]
fn each_storage_mode_states_its_accounting_and_its_host_directory_check() {
    let root: Box<Path> = Box::from(Path::new("/var/lib/golem/agents"));
    assert_eq!(
        [
            FilesystemStorageMode::Temporary,
            FilesystemStorageMode::Directory { root: root.clone() },
            FilesystemStorageMode::ManagedXfs {
                root: root.clone(),
                xfs_quotad_socket: root.clone(),
            },
            FilesystemStorageMode::ReflinkXfs { root: root.clone() },
            FilesystemStorageMode::Apfs { root },
        ]
        .iter()
        .map(storage_facts)
        .collect::<Vec<_>>(),
        [
            (AgentAccounting::Development, HostDirectoryCheck::None),
            (AgentAccounting::Development, HostDirectoryCheck::None),
            (
                AgentAccounting::ProjectQuotas,
                HostDirectoryCheck::NoXfsProject
            ),
            (
                AgentAccounting::Unaccounted,
                HostDirectoryCheck::NoXfsProject
            ),
            (AgentAccounting::Unaccounted, HostDirectoryCheck::None),
        ]
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn apfs_startup_refuses_other_platforms_without_creating_the_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("agents").into_boxed_path();
    let storage = FilesystemStorageMode::Apfs { root: root.clone() };
    let error = FilesystemVolume::open(&storage, RetryConfig::default())
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("initialize APFS storage on a non-macOS platform")
    );
    assert!(!root.exists());
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn apfs_startup_refuses_a_file_as_its_root_with_a_clear_error() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("agents").into_boxed_path();
    std::fs::write(&root, b"not a directory").unwrap();
    let storage = FilesystemStorageMode::Apfs { root: root.clone() };
    let error = FilesystemVolume::open(&storage, RetryConfig::default())
        .err()
        .unwrap();
    assert!(error.to_string().contains("create APFS development root"));
    assert!(error.to_string().contains(root.to_str().unwrap()));
    assert_eq!(std::fs::read(&root).unwrap(), b"not a directory");
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn apfs_startup_makes_and_probes_a_development_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("agents").into_boxed_path();
    let storage = FilesystemStorageMode::Apfs { root: root.clone() };
    let provisioning = FilesystemVolume::open(&storage, RetryConfig::default())
        .expect("APFS must start on the existing Mac volume");

    assert!(root.is_dir());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert_eq!(
        provisioning.agent_accounting(),
        AgentAccounting::Unaccounted
    );
    assert_eq!(
        provisioning.state.host_directory_check,
        HostDirectoryCheck::None
    );
    assert!(provisioning.copies_on_write());
    assert!(matches!(
        observe_space_blocking(&provisioning).unwrap(),
        FilesystemSpace::Observed { total_bytes, available_bytes, .. }
            if total_bytes > 0 && available_bytes > 0 && available_bytes <= total_bytes
    ));
    assert!(FilesystemVolume::open(&storage, RetryConfig::default()).is_ok());
}

#[test]
fn development_storage_has_development_accounting_and_no_project_check() {
    let root = tempfile::tempdir().unwrap();
    [
        FilesystemStorageMode::Temporary,
        FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
    ]
    .iter()
    .for_each(|storage| {
        let provisioning = FilesystemVolume::open(storage, RetryConfig::default()).unwrap();
        assert_eq!(
            provisioning.agent_accounting(),
            AgentAccounting::Development
        );
        assert_eq!(
            provisioning.state.host_directory_check,
            HostDirectoryCheck::None
        );
    });
}
