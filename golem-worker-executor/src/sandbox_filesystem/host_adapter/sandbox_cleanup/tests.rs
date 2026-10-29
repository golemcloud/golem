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

use super::super::QuotaAuthority;
use super::super::SandboxFilesystem;
use super::*;
use crate::sandbox_filesystem::SandboxAccessMode;
use crate::sandbox_filesystem::SandboxFilesystemAdapter;
use crate::sandbox_filesystem::SandboxFilesystemName;
use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
use crate::sandbox_filesystem::SandboxFollow;
use crate::sandbox_filesystem::SandboxObjectKind;
use crate::sandbox_filesystem::SandboxOpenOptions;
use crate::sandbox_filesystem::errors::FilesystemStorageError;
use crate::sandbox_filesystem::host_adapter::directory_for;
#[cfg(test)]
use crate::sandbox_filesystem::host_adapter::sandbox_cleanup::FilesystemLeaseProbe;
use crate::sandbox_filesystem::open_handles::SandboxDirectory;
use crate::sandbox_filesystem::open_handles::SandboxHandle;
use crate::sandbox_filesystem::sandbox_paths::SandboxPath;
use crate::sandbox_filesystem::scripted_adapter::ScriptedSandboxFilesystem;
use crate::sandbox_filesystem::scripted_adapter::ScriptedSandboxFilesystemProvisioning;
use crate::services::golem_config::FilesystemStorageMode;
use crate::storage_volume::FileCopyMode;
use crate::storage_volume::FilesystemLimits;
use crate::storage_volume::FilesystemSpace;
use crate::storage_volume::observe_space;
use golem_common::model::RetryConfig;
use std::path::PathBuf;
use std::sync::Arc;
use test_r::test;

fn name() -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "filesystem".to_string(),
    )
    .unwrap()
}

fn unmanaged_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
    SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .unwrap()
}

#[test]
fn creation_failure_selection_keeps_cleanup_failure_priority() {
    let creation = || {
        FilesystemStorageError::io(
            "create test filesystem",
            None,
            std::io::Error::other("creation failure"),
        )
    };
    let selected = creation_error_after_cleanup(creation(), Ok(()));
    assert_eq!(selected.io_error().unwrap().to_string(), "creation failure");
    assert!(!selected.cleanup_failed());
    let selected = creation_error_after_cleanup(
        creation(),
        Err(FilesystemStorageError::io(
            "remove test filesystem",
            None,
            std::io::Error::other("cleanup failure"),
        )
        .during_cleanup()),
    );
    assert_eq!(selected.io_error().unwrap().to_string(), "cleanup failure");
    assert!(selected.cleanup_failed());
    assert!(selected.to_string().contains("creation failure"));
    assert!(selected.to_string().contains("cleanup failure"));
}

#[test]
async fn unmanaged_creation_replaces_stale_contents_and_deletes_verified() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let stale = parent.path().join(name().relative_path());
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("garbage"), b"stale").unwrap();

    let filesystem = provisioning.create_fresh(name()).await.unwrap();

    assert!(
        std::fs::read_dir(filesystem.root())
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(filesystem.volume.file_copy_mode(), FileCopyMode::Buffered);
    assert!(matches!(
        filesystem.quota_authority,
        QuotaAuthority::Unsupported
    ));
    assert_eq!(
        observe_space(&filesystem.volume).await.unwrap(),
        FilesystemSpace::Unlimited
    );

    let root = filesystem.root().to_path_buf();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert!(!root.exists());
}

#[test]
async fn cancelling_armed_cleanup_restores_lease_state() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem = provisioning.create_fresh(name()).await.unwrap();

    let mut cleanup = Box::pin(async {
        let _state = RestoringLeaseState::take(&filesystem.lease.state).unwrap();
        std::future::pending::<()>().await;
    });
    assert!(futures::poll!(cleanup.as_mut()).is_pending());
    assert!(filesystem.lease.state.lock().unwrap().is_none());
    drop(cleanup);
    assert!(filesystem.lease.state.lock().unwrap().is_some());

    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn unmanaged_creation_serializes_the_same_native_name() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let first = provisioning.create_fresh(name()).await.unwrap();
    let second = tokio::spawn({
        let provisioning = provisioning.clone();
        async move { provisioning.create_fresh(name()).await }
    });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());

    SandboxFilesystem::delete_and_verify(first).await.unwrap();
    let second = second.await.unwrap().unwrap();
    SandboxFilesystem::delete_and_verify(second).await.unwrap();
}

async fn create_scripted_filesystem(
    provisioning: ScriptedSandboxFilesystemProvisioning,
) -> ScriptedSandboxFilesystem {
    <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap()
}

fn sandbox_name() -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "filesystem".to_string(),
    )
    .unwrap()
}

async fn open_sandbox_directory(
    filesystem: &SandboxFilesystem,
    path: impl Into<PathBuf>,
) -> SandboxDirectory {
    match filesystem
        .open(
            SandboxPath::at_root(path),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::Directory,
                access: SandboxAccessMode::ReadWrite,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
    {
        SandboxHandle::Directory(directory) => directory,
        SandboxHandle::File(_) => panic!("path must open as a directory"),
    }
}

fn scripted_storage_error(operation: &'static str) -> FilesystemStorageError {
    FilesystemStorageError::verification(operation, None)
}

fn directory_sandbox_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
    SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .unwrap()
}

#[test]
async fn failed_delete_returns_adapter_for_retry() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_delete_and_verify(Err(scripted_storage_error("first delete")));
    control.push_delete_and_verify(Ok(()));
    let filesystem = create_scripted_filesystem(provisioning).await;

    let failure = match <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(
        filesystem,
    )
    .await
    {
        Ok(()) => panic!("first deletion unexpectedly succeeded"),
        Err(failure) => failure,
    };
    let (filesystem, error) = failure.into_parts();
    assert_eq!(error.to_string(), "failed to first delete");

    assert!(
        <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .is_ok()
    );
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)",
            "delete_and_verify()",
            "delete_and_verify()",
        ]
    );
}

#[test]
async fn production_adapter_uses_concrete_static_dispatch_for_creation_and_deletion() {
    async fn create<A: SandboxFilesystemAdapter>(
        provisioning: A::Provisioning,
        name: SandboxFilesystemName,
        limits: Option<FilesystemLimits>,
    ) -> Result<A, FilesystemStorageError> {
        A::create_fresh(provisioning, name, limits).await
    }

    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = create::<SandboxFilesystem>(provisioning, sandbox_name(), None)
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert!(!root.exists());

    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let rejected_root = parent.path().join(sandbox_name().relative_path());
    let error = create::<SandboxFilesystem>(
        provisioning,
        sandbox_name(),
        Some(FilesystemLimits {
            allocated_bytes: 4096,
            filesystem_objects: 8,
        }),
    )
    .await
    .err()
    .expect("unmanaged creation must reject finite limits");
    assert_eq!(
        error.to_string(),
        format!(
            "failed to install limits without quota authority at {}",
            rejected_root.display()
        )
    );
    assert!(!rejected_root.exists());
}

#[test]
async fn root_close_linearizes_access_and_preserves_admitted_capability_lifetime() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("existing"), b"contents").unwrap();
    let descriptor = open_sandbox_directory(&filesystem, ".").await;
    let root_directory = filesystem.root_directory_state();
    let lookup_root = Arc::clone(&root_directory);
    let (admitted_tx, admitted_rx) = std::sync::mpsc::sync_channel(0);
    let (continue_tx, continue_rx) = std::sync::mpsc::sync_channel(0);
    let admitted_lookup = std::thread::spawn(move || {
        let admitted = directory_for(&lookup_root, &SandboxPath::at_root(".")).unwrap();
        admitted_tx.send(()).unwrap();
        continue_rx.recv().unwrap();
        admitted.metadata("existing").is_ok()
    });
    admitted_rx.recv().unwrap();

    filesystem.root.close();

    let rejected = directory_for(&root_directory, &SandboxPath::at_root("."));
    continue_tx.send(()).unwrap();
    let admitted_remained_usable = admitted_lookup.join().unwrap();
    assert_eq!(
        rejected.unwrap_err().kind(),
        std::io::ErrorKind::NotConnected,
        "lookup after close must fail"
    );
    assert!(
        admitted_remained_usable,
        "lookup admitted before close lost its capability"
    );
    filesystem
        .create_directory(SandboxPath::at(descriptor.clone(), "descriptor-relative"))
        .await
        .unwrap();
    let rejected = filesystem
        .create_directory(SandboxPath::at_root("root-relative"))
        .await
        .unwrap_err();
    assert_eq!(rejected.io_kind(), Some(std::io::ErrorKind::NotConnected));
    assert!(
        descriptor
            .host()
            .unwrap()
            .metadata("descriptor-relative")
            .is_ok()
    );

    drop(descriptor);
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn deletion_revokes_only_its_generation_root_and_retains_exclusive_ownership() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning.clone(),
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let released_root = filesystem.root_directory_state();
    let lease_probe = FilesystemLeaseProbe::install(filesystem.root());
    let second = tokio::spawn(async move {
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            sandbox_name(),
            None,
        )
        .await
    });
    lease_probe.wait_attempted().await;
    assert!(
        lease_probe.acquisition_is_pending(),
        "second provisioning acquired ownership before verified deletion"
    );

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
    lease_probe.wait_acquired().await;
    let second = second.await.unwrap().unwrap();

    let released = directory_for(&released_root, &SandboxPath::at_root(".")).unwrap_err();
    assert_eq!(released.kind(), std::io::ErrorKind::NotConnected);
    assert!(directory_for(&second.root_directory_state(), &SandboxPath::at_root(".")).is_ok());
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(second)
        .await
        .unwrap();
}
