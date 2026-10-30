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
use crate::filesystem::host_directories::HostDirectory;
use crate::filesystem::sandbox::host_adapter::sandbox_provisioning::SandboxFilesystemProvisioning;
use crate::filesystem::sandbox::{
    SandboxFilesystemAdapter, SandboxPath, SeedAccess, SeedEntry, SeedPlacement,
};
use crate::filesystem::volume::{AgentAccounting, file_project_id};
use crate::filesystem::volume::{FilesystemAllocation, FilesystemLimits};
use crate::services::golem_config::FilesystemStorageMode;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::Path;
use std::time::Duration;
use test_r::{test, timeout};

/// Reads the allocation of the project of `filesystem` after XFS frees its unlinked inodes.
///
/// XFS keeps an unlinked inode, with its blocks, charged to its project until background
/// inactivation frees it. The reading is the first one whose object count equals the inodes
/// that are reachable from the root. The readings are 25 ms apart. After 5 s the function
/// panics with the last reading and the reachable count.
async fn settled_allocation(filesystem: &SandboxFilesystem) -> FilesystemAllocation {
    use futures::StreamExt as _;

    let reachable = reachable_inodes(filesystem.root());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let last = futures::stream::unfold(Some(Duration::ZERO), move |delay| async move {
        tokio::time::sleep(delay?).await;
        let reading = filesystem.observe_allocation().await.unwrap();
        let next = (reading.filesystem_objects != reachable
            && tokio::time::Instant::now() < deadline)
            .then_some(Duration::from_millis(25));
        Some((reading, next))
    })
    .fold(None, |_, reading| std::future::ready(Some(reading)))
    .await
    .expect("the first allocation reading always happens");
    assert_eq!(
        last.filesystem_objects, reachable,
        "project allocation did not settle within 5 s: last reading {last:?}, reachable inodes {reachable}"
    );
    last
}

/// Counts the root and every entry below it, without following symlinks.
fn reachable_inodes(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .fold(1, |count, entry| {
            match entry.file_type().unwrap().is_dir() {
                true => count + reachable_inodes(&entry.path()),
                false => count + 1,
            }
        })
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("60s")]
async fn managed_xfs_sandbox_filesystem_owns_allocation_limits_and_cleanup() {
    use std::collections::BTreeSet;

    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::ManagedXfs {
                xfs_quotad_socket: crate::filesystem::volume::in_process_xfs_quotad(&root),
                root: root.clone().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
    assert!(
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::ManagedXfs {
                xfs_quotad_socket: crate::filesystem::volume::in_process_xfs_quotad(&root),
                root: root.clone().into(),
            },
            RetryConfig::default()
        )
        .await
        .is_err()
    );
    assert_eq!(
        provisioning.volume().agent_accounting(),
        AgentAccounting::ProjectQuotas
    );
    assert!(provisioning.volume().copies_on_write());
    assert!(directories.scratch.path().as_path().is_dir());
    assert!(directories.initial_files.path().as_path().is_dir());

    let sources = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("sandbox-filesystem-source"),
    )
    .await
    .unwrap();
    let source = sources
        .path()
        .child(std::ffi::OsStr::new("source"))
        .unwrap();
    std::fs::write(source.as_path(), vec![0x5a; 8192]).unwrap();
    let source_descriptor = File::open(source.as_path()).unwrap();
    assert_eq!(file_project_id(&source_descriptor).unwrap(), None);
    drop(source_descriptor);

    let filesystem = provisioning
        .create_fresh(
            SandboxFilesystemName::new(
                "native-test-environment".to_string(),
                "native-test-component".to_string(),
                format!("native-test-filesystem-{}", std::process::id()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let name_mode_parent = filesystem.root().join("name-mode-parent");
    std::fs::create_dir(&name_mode_parent).unwrap();
    filesystem
        .resolve_path_target(SandboxPath::at_root("name-mode-parent/child"))
        .await
        .unwrap();
    assert_eq!(filesystem.name_mode_probe_count(), 0);
    std::fs::remove_dir(name_mode_parent).unwrap();

    let unmanaged_parent = tempfile::tempdir().unwrap();
    let unmanaged = SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory {
            root: unmanaged_parent.path().into(),
        },
        RetryConfig::default(),
    )
    .unwrap()
    .create_fresh(
        SandboxFilesystemName::new(
            "native-test-unmanaged-environment".to_string(),
            "native-test-unmanaged-component".to_string(),
            format!("native-test-unmanaged-filesystem-{}", std::process::id()),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    unmanaged
        .resolve_path_target(SandboxPath::at_root("name-mode-child"))
        .await
        .unwrap();
    assert_eq!(unmanaged.name_mode_probe_count(), 1);
    SandboxFilesystem::delete_and_verify(unmanaged)
        .await
        .unwrap();

    let project_id = filesystem.project_id_for_test();
    let copied = filesystem.root().join("copied");
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([SeedEntry {
            source: source.clone(),
            target: SandboxPath::at_root("copied"),
            access: SeedAccess::ReadWrite,
            placement: SeedPlacement::CreateNew,
        }]),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&copied).unwrap(), vec![0x5a; 8192]);
    let copied_descriptor = File::open(&copied).unwrap();
    assert_eq!(
        file_project_id(&copied_descriptor).unwrap(),
        Some(project_id)
    );
    drop(copied_descriptor);

    let allocation = filesystem.observe_allocation().await.unwrap();
    assert!(allocation.allocated_bytes >= 8192);
    assert!(allocation.filesystem_objects >= 2);
    let limits = FilesystemLimits {
        allocated_bytes: 128 * 1024 * 1024,
        filesystem_objects: 8192,
    };
    let installed = filesystem.install_limits(limits).await.unwrap();
    assert_eq!(installed.limits, limits);

    let sparse_path = filesystem.root().join("sparse");
    let sparse = File::create(&sparse_path).unwrap();
    sparse.set_len(16 * 1024 * 1024).unwrap();
    sparse.sync_all().unwrap();
    let sparse_allocation = filesystem.observe_allocation().await.unwrap();
    assert_eq!(
        std::fs::metadata(&sparse_path).unwrap().len(),
        16 * 1024 * 1024
    );
    assert!(
        sparse_allocation.allocated_bytes < 16 * 1024 * 1024,
        "sparse logical length was treated as allocated bytes"
    );
    drop(sparse);

    let mut copied_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&copied)
        .unwrap();
    copied_file.seek(SeekFrom::Start(0)).unwrap();
    copied_file.write_all(b"COW!").unwrap();
    copied_file.sync_all().unwrap();
    drop(copied_file);
    assert_eq!(&std::fs::read(source.as_path()).unwrap()[..4], b"ZZZZ");
    assert_eq!(&std::fs::read(&copied).unwrap()[..4], b"COW!");

    let before_open_unlinked = settled_allocation(&filesystem).await;
    let open_unlinked_path = filesystem.root().join("open-unlinked");
    let mut open_unlinked = File::create(&open_unlinked_path).unwrap();
    open_unlinked.write_all(&vec![0x3c; 1024 * 1024]).unwrap();
    open_unlinked.sync_all().unwrap();
    std::fs::remove_file(&open_unlinked_path).unwrap();
    let retained_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let while_open_unlinked = filesystem.observe_allocation().await.unwrap();
        if while_open_unlinked.allocated_bytes > before_open_unlinked.allocated_bytes
            && while_open_unlinked.filesystem_objects > before_open_unlinked.filesystem_objects
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < retained_deadline,
            "open-unlinked allocation was not retained while its descriptor remained open: before={before_open_unlinked:?}, current={while_open_unlinked:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    drop(open_unlinked);
    let release_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let after_close = filesystem.observe_allocation().await.unwrap();
        if after_close.allocated_bytes <= before_open_unlinked.allocated_bytes
            && after_close.filesystem_objects <= before_open_unlinked.filesystem_objects
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < release_deadline,
            "open-unlinked allocation was not released after closing its final descriptor: before={before_open_unlinked:?}, current={after_close:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let second = provisioning
        .create_fresh(
            SandboxFilesystemName::new(
                "native-test-environment".to_string(),
                "native-test-component".to_string(),
                format!("native-test-second-filesystem-{}", std::process::id()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let second_project_id = second.project_id_for_test();
    let hard_link_error = std::fs::hard_link(&copied, second.root().join("cross-project-link"))
        .expect_err("XFS must reject hard links across project ownership domains");
    assert!(matches!(hard_link_error.raw_os_error(), Some(libc::EXDEV)));

    let current = filesystem.observe_allocation().await.unwrap();
    let byte_limited = FilesystemLimits {
        allocated_bytes: current.allocated_bytes + 2 * 1024 * 1024,
        filesystem_objects: current.filesystem_objects + 32,
    };
    filesystem.install_limits(byte_limited).await.unwrap();
    let quota_path = filesystem.root().join("byte-quota");
    let mut quota_file = File::create(&quota_path).unwrap();
    let quota_error = quota_file
        .write_all(&vec![0x6b; 4 * 1024 * 1024])
        .expect_err("project byte quota did not stop allocation");
    assert!(matches!(
        quota_error.raw_os_error(),
        Some(libc::EDQUOT) | Some(libc::ENOSPC)
    ));

    let raised_byte_limit = FilesystemLimits {
        allocated_bytes: byte_limited.allocated_bytes + 4 * 1024 * 1024,
        filesystem_objects: byte_limited.filesystem_objects,
    };
    filesystem.install_limits(raised_byte_limit).await.unwrap();
    quota_file.write_all(&vec![0x7c; 512 * 1024]).unwrap();
    quota_file.sync_all().unwrap();
    drop(quota_file);

    let current = filesystem.observe_allocation().await.unwrap();
    let object_limited = FilesystemLimits {
        allocated_bytes: raised_byte_limit.allocated_bytes + 4 * 1024 * 1024,
        filesystem_objects: current.filesystem_objects + 2,
    };
    filesystem.install_limits(object_limited).await.unwrap();
    let mut object_quota_error = None;
    let mut object_quota_files = Vec::new();
    for index in 0..8 {
        let name = format!("object-quota-{index}");
        match File::create(filesystem.root().join(&name)) {
            Ok(file) => {
                drop(file);
                object_quota_files.push(name);
            }
            Err(error) => {
                object_quota_error = Some(error);
                break;
            }
        }
    }
    let object_quota_error =
        object_quota_error.expect("project filesystem-object quota did not stop creation");
    assert!(matches!(
        object_quota_error.raw_os_error(),
        Some(libc::EDQUOT) | Some(libc::ENOSPC)
    ));
    filesystem
        .install_limits(FilesystemLimits {
            allocated_bytes: object_limited.allocated_bytes,
            filesystem_objects: object_limited.filesystem_objects + 4,
        })
        .await
        .unwrap();
    let object_after_limit_raise =
        File::create(filesystem.root().join("object-after-limit-raise")).unwrap();
    drop(object_after_limit_raise);

    let mut expected_linked_names = BTreeSet::from([
        "byte-quota".to_string(),
        "copied".to_string(),
        "object-after-limit-raise".to_string(),
        "sparse".to_string(),
    ]);
    expected_linked_names.extend(object_quota_files);
    let linked_names = std::fs::read_dir(filesystem.root())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(linked_names, expected_linked_names);
    let after_descriptor_drops = filesystem.observe_allocation().await.unwrap();
    assert_eq!(
        after_descriptor_drops.filesystem_objects,
        u64::try_from(linked_names.len()).unwrap() + 1,
        "managed project usage retained an unlinked object after all test descriptors were dropped"
    );
    assert!(after_descriptor_drops.allocated_bytes >= 8192);

    let filesystem_root = filesystem.root().to_path_buf();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert!(!filesystem_root.exists());
    assert_eq!(
        provisioning
            .project_allocation_for_test(project_id)
            .unwrap(),
        FilesystemAllocation {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );
    let second_root = second.root().to_path_buf();
    SandboxFilesystem::delete_and_verify(second).await.unwrap();
    assert!(!second_root.exists());
    assert_eq!(
        provisioning
            .project_allocation_for_test(second_project_id)
            .unwrap(),
        FilesystemAllocation {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );
    sources.discard().await.unwrap();
}
