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
use crate::filesystem::sandbox::SandboxAccessMode;
use crate::filesystem::sandbox::SandboxFilesystemAdapter;
use crate::filesystem::sandbox::SandboxFollow;
use crate::filesystem::sandbox::SandboxObjectKind;
use crate::filesystem::sandbox::SandboxOpenOptions;
use crate::filesystem::sandbox::host_adapter::SandboxFilesystem;
use crate::filesystem::sandbox::host_adapter::sandbox_provisioning::SandboxFilesystemName;
use crate::filesystem::sandbox::host_adapter::sandbox_provisioning::SandboxFilesystemProvisioning;
use crate::filesystem::sandbox::open_handles::SandboxFile;
use crate::filesystem::sandbox::open_handles::SandboxHandle;
use crate::filesystem::sandbox::sandbox_paths::SandboxPath;
use crate::services::golem_config::FilesystemStorageMode;
use golem_common::model::RetryConfig;
use std::path::PathBuf;
use std::sync::Arc;
use test_r::test;
use tokio::sync::Mutex as AsyncMutex;

#[test]
fn append_coordinator_decision_reuses_only_an_observed_live_coordinator() {
    assert!(matches!(
        decide_append_coordinator(None),
        AppendCoordinatorAction::Allocate
    ));
    let observed = Arc::new(AsyncMutex::new(()));
    let AppendCoordinatorAction::Reuse(reused) =
        decide_append_coordinator(Some(Arc::clone(&observed)))
    else {
        panic!("an observed live coordinator must be reused");
    };
    assert!(Arc::ptr_eq(&reused, &observed));
}

fn existing_open(expected: SandboxObjectKind, access: SandboxAccessMode) -> SandboxOpenOptions {
    SandboxOpenOptions::Existing {
        expected,
        access,
        follow: SandboxFollow::No,
    }
}

fn sandbox_name() -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "filesystem".to_string(),
    )
    .unwrap()
}

async fn open_sandbox_file(
    filesystem: &SandboxFilesystem,
    path: impl Into<PathBuf>,
) -> SandboxFile {
    match filesystem
        .open(
            SandboxPath::at_root(path),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::File,
                access: SandboxAccessMode::ReadWrite,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
    {
        SandboxHandle::File(file) => file,
        SandboxHandle::Directory(_) => panic!("path must open as a file"),
    }
}

fn directory_sandbox_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
    SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
async fn opened_nodes_keep_file_permissions_and_directory_identity() {
    let parent = tempfile::tempdir().unwrap();
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        directory_sandbox_provisioning(parent.path().to_path_buf()),
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("file"), b"contents").unwrap();
    std::fs::create_dir(filesystem.root().join("directory")).unwrap();

    for (name, kind) in [
        ("file", SandboxObjectKind::File),
        ("directory", SandboxObjectKind::Directory),
    ] {
        let path = filesystem.root().join(name);
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).unwrap();
        let node = filesystem
            .open(
                SandboxPath::at_root(name),
                existing_open(kind, SandboxAccessMode::Read),
            )
            .await
            .unwrap();
        assert_eq!(node.is_read_only_file(), kind == SandboxObjectKind::File);
        assert_eq!(node.clone().is_read_only_file(), node.is_read_only_file());
        assert_eq!(
            node.directory_id().is_some(),
            kind == SandboxObjectKind::Directory
        );
        std::fs::set_permissions(&path, original).unwrap();
        filesystem.close(node).await.unwrap();
    }

    let writable = filesystem
        .open(
            SandboxPath::at_root("file"),
            existing_open(SandboxObjectKind::File, SandboxAccessMode::Read),
        )
        .await
        .unwrap();
    assert!(!writable.is_read_only_file());
    assert!(writable.directory_id().is_none());
    filesystem.close(writable).await.unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(any(unix, windows))]
#[test]
async fn concurrent_first_append_coordination_converges_across_hard_links() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("file"), b"").unwrap();
    std::fs::hard_link(
        filesystem.root().join("file"),
        filesystem.root().join("hard-link"),
    )
    .unwrap();
    let files = vec![
        open_sandbox_file(&filesystem, "file").await,
        open_sandbox_file(&filesystem, "file").await,
        open_sandbox_file(&filesystem, "hard-link").await,
        open_sandbox_file(&filesystem, "hard-link").await,
    ];
    assert_eq!(filesystem.append_coordinators.counts().lookups, 0);
    assert_eq!(filesystem.append_coordinators.counts().allocations, 0);

    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let tasks = files
        .into_iter()
        .map(|file| {
            let release = Arc::clone(&release);
            tokio::spawn(async move {
                let _guard = file.coordinate_append().await;
                release.acquire().await.unwrap().forget();
            })
        })
        .collect::<Vec<_>>();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while filesystem.append_coordinators.counts().lookups < tasks.len() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let racing = filesystem.append_coordinators.counts();
    assert_eq!(racing.allocations, 1);
    assert_eq!(racing.lock_acquisitions, 1);
    assert_eq!(racing.registered, 1);
    assert_eq!(racing.live, 1);

    release.add_permits(tasks.len());
    for task in tasks {
        task.await.unwrap();
    }
    let completed = filesystem.append_coordinators.counts();
    assert_eq!(completed.allocations, 1);
    assert_eq!(completed.lock_acquisitions, 4);
    assert_eq!(completed.live, 0);

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(any(unix, windows))]
#[test]
async fn distinct_append_identities_are_independent() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("first"), b"").unwrap();
    std::fs::write(filesystem.root().join("second"), b"").unwrap();
    let first = open_sandbox_file(&filesystem, "first").await;
    let second = open_sandbox_file(&filesystem, "second").await;

    let first_guard = first.coordinate_append().await;
    let second_guard = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        second.coordinate_append(),
    )
    .await
    .expect("distinct native identities must not share append coordination");
    let counts = filesystem.append_coordinators.counts();
    assert_eq!(counts.allocations, 2);
    assert_eq!(counts.lock_acquisitions, 2);

    drop((first_guard, second_guard));
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(any(unix, windows))]
#[test]
async fn append_coordinator_registry_cleans_expired_weak_entries() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("first"), b"").unwrap();
    std::fs::write(filesystem.root().join("second"), b"").unwrap();
    let first = open_sandbox_file(&filesystem, "first").await;
    let second = open_sandbox_file(&filesystem, "second").await;

    let first_guard = first.coordinate_append().await;
    assert_eq!(filesystem.append_coordinators.counts().live, 1);
    drop(first_guard);
    let expired = filesystem.append_coordinators.counts();
    assert_eq!(expired.registered, 1);
    assert_eq!(expired.live, 0);

    let second_guard = second.coordinate_append().await;
    let cleaned = filesystem.append_coordinators.counts();
    assert_eq!(cleaned.allocations, 2);
    assert_eq!(cleaned.registered, 1);
    assert_eq!(cleaned.live, 1);
    drop(second_guard);

    let first_guard = first.coordinate_append().await;
    let reused_descriptor = filesystem.append_coordinators.counts();
    assert_eq!(reused_descriptor.allocations, 3);
    assert_eq!(reused_descriptor.registered, 1);
    assert_eq!(reused_descriptor.live, 1);
    drop(first_guard);

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(any(unix, windows))]
#[test]
async fn append_coordination_tracks_identity_through_rename_unlink_and_path_reuse() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let original_path = filesystem.root().join("object");
    let reused_path = filesystem.root().join("reused");
    std::fs::write(&original_path, b"original").unwrap();
    let original = open_sandbox_file(&filesystem, "object").await;
    let original_guard = original.coordinate_append().await;

    std::fs::rename(&original_path, &reused_path).unwrap();
    let renamed = open_sandbox_file(&filesystem, "reused").await;
    let renamed_acquired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let renamed_task = tokio::spawn({
        let renamed_acquired = Arc::clone(&renamed_acquired);
        async move {
            let _guard = renamed.coordinate_append().await;
            renamed_acquired.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while filesystem.append_coordinators.counts().lookups < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!renamed_acquired.load(std::sync::atomic::Ordering::Relaxed));

    std::fs::remove_file(&reused_path).unwrap();
    std::fs::write(&reused_path, b"replacement").unwrap();
    let replacement = open_sandbox_file(&filesystem, "reused").await;
    let replacement_guard = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        replacement.coordinate_append(),
    )
    .await
    .expect("path reuse must not inherit the unlinked object's coordinator");
    assert_eq!(filesystem.append_coordinators.counts().allocations, 2);

    drop(replacement_guard);
    drop(original_guard);
    renamed_task.await.unwrap();
    let counts = filesystem.append_coordinators.counts();
    assert_eq!(counts.lookups, 3);
    assert_eq!(counts.allocations, 2);
    assert_eq!(counts.lock_acquisitions, 3);

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}
