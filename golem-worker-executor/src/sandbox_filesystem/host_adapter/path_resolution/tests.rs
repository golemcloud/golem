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

use crate::sandbox_filesystem::SandboxAccessMode;
#[cfg(target_os = "macos")]
use crate::sandbox_filesystem::SandboxFileDisposition;
use crate::sandbox_filesystem::SandboxFilesystemAdapter;
use crate::sandbox_filesystem::SandboxFollow;
use crate::sandbox_filesystem::SandboxObjectKind;
use crate::sandbox_filesystem::SandboxOpenOptions;
use crate::sandbox_filesystem::errors::FilesystemStorageError;
use crate::sandbox_filesystem::host_adapter::NativeNameModeSource;
use crate::sandbox_filesystem::host_adapter::NativeRoot;
use crate::sandbox_filesystem::host_adapter::QuotaAuthority;
use crate::sandbox_filesystem::host_adapter::SandboxFilesystem;
use crate::sandbox_filesystem::host_adapter::directory_for;
#[cfg(target_os = "linux")]
use crate::sandbox_filesystem::host_adapter::path_resolution::linux_name_comparison_mode;
use crate::sandbox_filesystem::host_adapter::sandbox_cleanup::LeaseState;
use crate::sandbox_filesystem::host_adapter::sandbox_cleanup::NativeCleanup;
use crate::sandbox_filesystem::host_adapter::sandbox_provisioning::SandboxFilesystemName;
use crate::sandbox_filesystem::host_adapter::sandbox_provisioning::SandboxFilesystemProvisioning;
use crate::sandbox_filesystem::host_adapter::tree_copy;
use crate::sandbox_filesystem::name_comparison::NativeNameComparisonMode;
use crate::sandbox_filesystem::object_identity::NativeFileIdentity;
use crate::sandbox_filesystem::object_identity::SandboxDirectoryId;
use crate::sandbox_filesystem::object_identity::directory_identity;
use crate::sandbox_filesystem::open_handles::NativeDirectoryHandle;
use crate::sandbox_filesystem::open_handles::SandboxDirectory;
use crate::sandbox_filesystem::open_handles::SandboxHandle;
use crate::sandbox_filesystem::sandbox_paths::SandboxPath;
use crate::services::golem_config::FilesystemStorageMode;
use crate::storage_volume::FilesystemVolume;
#[cfg(target_os = "linux")]
use crate::storage_volume::validated_xfs_name_mode_for_test;
use golem_common::model::RetryConfig;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use test_r::test;
use tokio::sync::Mutex as AsyncMutex;

#[cfg(target_os = "linux")]
fn open_host_directory(path: &Path) -> SandboxDirectory {
    let directory = Arc::new(cap_std::fs::Dir::from_std_file(File::open(path).unwrap()));
    let directory_id = directory_identity(&directory).unwrap();
    SandboxDirectory {
        handle: NativeDirectoryHandle::Host(directory),
        path: path.to_path_buf(),
        directory_id,
    }
}

#[cfg(target_os = "linux")]
fn linux_directory_id(device: u64) -> SandboxDirectoryId {
    SandboxDirectoryId(NativeFileIdentity::Unix { device, inode: 1 })
}

fn sandbox_name() -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "filesystem".to_string(),
    )
    .unwrap()
}

fn directory_sandbox_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
    SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .unwrap()
}

#[cfg(target_os = "linux")]
async fn sandbox_with_validated_name_mode() -> SandboxFilesystem {
    let root = tempfile::Builder::new()
        .prefix("golem-validated-name-mode")
        .tempdir()
        .unwrap()
        .keep();
    let directory = File::open(&root).unwrap();
    let device = std::os::unix::fs::MetadataExt::dev(&directory.metadata().unwrap());
    let lifecycle = Arc::new(AsyncMutex::new(()))
        .try_lock_owned()
        .expect("new name-mode test filesystem lease must be available");
    let native_root = NativeRoot::new(
        root.clone(),
        directory,
        tree_copy::native_seed_sync_platform(),
    )
    .unwrap();
    SandboxFilesystem::new(
        native_root,
        LeaseState {
            lifecycle,
            cleanup: NativeCleanup::Directory {
                path: root,
                cleanup_retry: RetryConfig::default(),
            },
        },
        FilesystemVolume::unmanaged_development(),
        QuotaAuthority::Unsupported,
        NativeNameModeSource::ValidatedXfs(validated_xfs_name_mode_for_test(device)),
    )
}

#[cfg(target_os = "linux")]
#[test]
fn validated_xfs_name_mode_skips_native_detection() {
    let parent = linux_directory_id(17);
    let source = NativeNameModeSource::ValidatedXfs(validated_xfs_name_mode_for_test(17));
    let probes = std::cell::Cell::new(0);

    assert_eq!(
        linux_name_comparison_mode(source, &parent, true, || {
            probes.set(probes.get() + 1);
            None
        }),
        NativeNameComparisonMode::Exact
    );
    assert_eq!(probes.get(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn unmanaged_linux_name_mode_retains_native_detection() {
    let parent = linux_directory_id(17);
    let probes = std::cell::Cell::new(0);

    assert_eq!(
        linux_name_comparison_mode(NativeNameModeSource::NativeDetection, &parent, true, || {
            probes.set(probes.get() + 1);
            Some(0)
        },),
        NativeNameComparisonMode::Exact
    );
    assert_eq!(probes.get(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn unknown_linux_name_mode_is_conservative_after_native_detection() {
    let parent = linux_directory_id(17);
    let probes = std::cell::Cell::new(0);

    assert_eq!(
        linux_name_comparison_mode(NativeNameModeSource::NativeDetection, &parent, true, || {
            probes.set(probes.get() + 1);
            None
        },),
        NativeNameComparisonMode::Conservative
    );
    assert_eq!(probes.get(), 1);
    assert_eq!(
        linux_name_comparison_mode(NativeNameModeSource::NativeDetection, &parent, true, || {
            Some(0x4000_0000)
        },),
        NativeNameComparisonMode::Conservative
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn sandbox_filesystem_constructor_preserves_validated_name_mode_for_same_filesystem_only() {
    let filesystem = sandbox_with_validated_name_mode().await;

    filesystem
        .resolve_path_target(SandboxPath::at_root("same-filesystem"))
        .await
        .unwrap();
    assert_eq!(filesystem.name_mode_probe_count(), 0);

    let foreign = open_host_directory(Path::new("/proc"));
    filesystem
        .resolve_path_target(SandboxPath::at(foreign, "self"))
        .await
        .unwrap();
    assert_eq!(filesystem.name_mode_probe_count(), 1);

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
async fn unmanaged_provisioning_reaches_real_namespace_name_mode_detection() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();

    filesystem
        .resolve_path_target(SandboxPath::at_root("unmanaged-name-mode-probe"))
        .await
        .unwrap();
    assert_eq!(filesystem.name_mode_probe_count(), 1);

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn benchmark_disable_control_restores_managed_xfs_native_detection() {
    let parent = linux_directory_id(17);
    let source = NativeNameModeSource::ValidatedXfs(validated_xfs_name_mode_for_test(17));
    let probes = std::cell::Cell::new(0);

    assert_eq!(
        linux_name_comparison_mode(source, &parent, false, || {
            probes.set(probes.get() + 1);
            Some(0)
        }),
        NativeNameComparisonMode::Exact
    );
    assert_eq!(probes.get(), 1);
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn apfs_native_coordination_preserves_filename_spelling_and_lookup() {
    let root = tempfile::tempdir().unwrap();
    let provisioning = SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Apfs {
            root: root.path().into(),
        },
        RetryConfig::default(),
    )
    .unwrap();
    let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
    std::fs::write(filesystem.root().join("CaseProbe"), b"case").unwrap();
    let case_alias_exists = filesystem.root().join("caseprobe").exists();
    let case_exact = filesystem
        .resolve_path_target(SandboxPath::at_root("CaseProbe"))
        .await
        .unwrap();
    let case_alias = filesystem
        .resolve_path_target(SandboxPath::at_root("caseprobe"))
        .await
        .unwrap();
    assert_eq!(
        case_exact.target().path.as_os_str(),
        std::ffi::OsStr::new("CaseProbe")
    );
    assert_eq!(
        case_alias.target().path.as_os_str(),
        std::ffi::OsStr::new("caseprobe")
    );
    let case_object = filesystem
        .get_path_attributes(case_exact.target(), SandboxFollow::No)
        .await
        .unwrap()
        .object;
    let alias_result = filesystem
        .get_path_attributes(case_alias.target(), SandboxFollow::No)
        .await;
    if case_alias_exists {
        assert_eq!(case_object, alias_result.unwrap().object);
    } else {
        assert_eq!(
            alias_result.unwrap_err().io_kind(),
            Some(std::io::ErrorKind::NotFound)
        );
    }
    let composed = "caf\u{00e9}.txt";
    let decomposed = "cafe\u{0301}.txt";
    let created = filesystem
        .open(
            SandboxPath::at_root(decomposed),
            SandboxOpenOptions::File {
                access: SandboxAccessMode::ReadWrite,
                disposition: SandboxFileDisposition::CreateExclusive,
                follow: SandboxFollow::No,
            },
        )
        .await
        .unwrap();
    filesystem.close(created).await.unwrap();
    std::fs::write(filesystem.root().join("notes.txt"), b"distinct").unwrap();
    let first = filesystem
        .resolve_path_target(SandboxPath::at_root(composed))
        .await
        .unwrap();
    let alias = filesystem
        .resolve_path_target(SandboxPath::at_root(decomposed))
        .await
        .unwrap();
    let sibling = filesystem
        .resolve_path_target(SandboxPath::at_root("notes.txt"))
        .await
        .unwrap();
    assert_eq!(
        first.target().path.as_os_str(),
        std::ffi::OsStr::new(composed)
    );
    assert_eq!(
        alias.target().path.as_os_str(),
        std::ffi::OsStr::new(decomposed)
    );
    assert!(first.name_comparison_key() == alias.name_comparison_key());
    assert!(
        first
            .name_comparison_key()
            .may_conflict_with(&sibling.name_comparison_key())
    );
    let first_object = filesystem
        .get_path_attributes(first.target(), SandboxFollow::No)
        .await
        .unwrap()
        .object;
    let alias_object = filesystem
        .get_path_attributes(alias.target(), SandboxFollow::No)
        .await
        .unwrap()
        .object;
    let sibling_object = filesystem
        .get_path_attributes(sibling.target(), SandboxFollow::No)
        .await
        .unwrap()
        .object;
    assert_eq!(first_object, alias_object);
    assert_ne!(first_object, sibling_object);
    assert!(
        std::fs::read_dir(filesystem.root())
            .unwrap()
            .any(|entry| { entry.unwrap().file_name() == std::ffi::OsStr::new(decomposed) })
    );
    assert_eq!(
        filesystem.root().join("caseprobe").exists(),
        case_alias_exists
    );
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn production_namespace_resolution_pins_semantic_parents_and_does_not_follow_final_symlinks()
{
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::create_dir_all(filesystem.root().join("real/parent")).unwrap();
    std::os::unix::fs::symlink("real/parent", filesystem.root().join("alias")).unwrap();

    let direct_absent = filesystem
        .resolve_path_target(SandboxPath::at_root("real/parent/child"))
        .await
        .unwrap();
    let alias_absent = filesystem
        .resolve_path_target(SandboxPath::at_root("alias/child"))
        .await
        .unwrap();
    assert!(direct_absent.name_comparison_key() == alias_absent.name_comparison_key());
    assert!(direct_absent.target_directory_id().is_none());

    std::fs::create_dir(filesystem.root().join("real/parent/child")).unwrap();
    let opened_parent = filesystem
        .open(
            SandboxPath::at_root("real/parent"),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::Directory,
                access: SandboxAccessMode::Read,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap();
    let parent_directory = match opened_parent {
        SandboxHandle::Directory(directory) => directory,
        SandboxHandle::File(_) => panic!("parent must open as a directory"),
    };
    let descriptor_relative = filesystem
        .resolve_path_target(SandboxPath::at(parent_directory.clone(), "child"))
        .await
        .unwrap();
    let alias_present = filesystem
        .resolve_path_target(SandboxPath::at_root("alias/child"))
        .await
        .unwrap();
    assert!(
        descriptor_relative
            .name_comparison_key()
            .may_conflict_with(&alias_present.name_comparison_key())
    );
    assert!(descriptor_relative.target_directory_id() == alias_present.target_directory_id());
    assert!(descriptor_relative.target_directory_id().is_some());

    std::fs::write(filesystem.root().join("real/parent/plain-file"), b"").unwrap();
    std::fs::set_permissions(
        filesystem.root().join("real/parent/plain-file"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    std::fs::write(filesystem.root().join("real/parent/writable-file"), b"").unwrap();
    let final_file = filesystem
        .resolve_path_target(SandboxPath::at_root("alias/plain-file"))
        .await
        .unwrap();
    assert!(final_file.target_directory_id().is_none());
    let descriptor_file = filesystem
        .resolve_path_target(SandboxPath::at(parent_directory, "plain-file"))
        .await
        .unwrap();
    std::os::unix::fs::symlink(
        "real/parent/plain-file",
        filesystem.root().join("plain-file-alias"),
    )
    .unwrap();
    let final_file_alias = filesystem
        .resolve_path_target(SandboxPath::at_root("plain-file-alias"))
        .await
        .unwrap();
    let writable_file = filesystem
        .resolve_path_target(SandboxPath::at_root("alias/writable-file"))
        .await
        .unwrap();
    assert!(final_file.is_read_only_file(SandboxFollow::Yes).unwrap());
    assert!(
        descriptor_file
            .is_read_only_file(SandboxFollow::No)
            .unwrap(),
        "a descriptor-relative target must give the permissions of the file"
    );
    assert!(
        final_file_alias
            .is_read_only_file(SandboxFollow::Yes)
            .unwrap(),
        "a followed final symlink must give the permissions of its referent"
    );
    assert!(
        !final_file_alias
            .is_read_only_file(SandboxFollow::No)
            .unwrap(),
        "a symlink that is not followed is not a read-only file"
    );
    assert!(!writable_file.is_read_only_file(SandboxFollow::Yes).unwrap());
    assert!(!alias_present.is_read_only_file(SandboxFollow::No).unwrap());

    std::os::unix::fs::symlink("child", filesystem.root().join("real/parent/final-link")).unwrap();
    let final_symlink = filesystem
        .resolve_path_target(SandboxPath::at_root("alias/final-link"))
        .await
        .unwrap();
    assert!(final_symlink.target_directory_id().is_none());

    drop(direct_absent);
    drop(alias_absent);
    drop(descriptor_relative);
    drop(alias_present);
    drop(final_file);
    drop(descriptor_file);
    drop(final_file_alias);
    drop(writable_file);
    drop(final_symlink);
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
#[test_r::tag(native_apfs)]
async fn production_namespace_resolution_does_not_require_final_directory_search_permission() {
    use std::os::unix::fs::PermissionsExt;

    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let target = filesystem.root().join("locked-directory");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();

    let search_error = std::fs::symlink_metadata(target.join("child")).unwrap_err();
    if search_error.kind() != std::io::ErrorKind::PermissionDenied {
        // Privileged test users may bypass the target directory's mode bits.
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir(&target).unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
        return;
    }

    let resolved = filesystem
        .resolve_path_target(SandboxPath::at_root("locked-directory"))
        .await
        .unwrap();
    assert!(resolved.target_directory_id().is_some());
    let source_parent = resolved.parent.host().unwrap();
    let destination_parent = directory_for(
        &filesystem.root_directory_state(),
        &SandboxPath::at_root("renamed-directory"),
    )
    .unwrap();

    let native_source = filesystem.root().join("native-locked-directory");
    let native_destination = filesystem.root().join("native-renamed-directory");
    std::fs::create_dir(&native_source).unwrap();
    std::fs::set_permissions(&native_source, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        std::fs::symlink_metadata(native_source.join("child"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied,
    );
    let native_result = rustix::fs::renameat(
        source_parent.as_ref(),
        "native-locked-directory",
        destination_parent.as_ref(),
        "native-renamed-directory",
    )
    .map_err(std::io::Error::from);
    let remaining_native = if native_result.is_ok() {
        &native_destination
    } else {
        &native_source
    };
    std::fs::set_permissions(remaining_native, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir(remaining_native).unwrap();

    let renamed_result = filesystem
        .rename(resolved.target(), SandboxPath::at_root("renamed-directory"))
        .await;
    assert_eq!(
        renamed_result.is_ok(),
        native_result.is_ok(),
        "the adapter rename must succeed exactly when the kernel rename succeeds: \
             adapter={renamed_result:?}, native={native_result:?}"
    );
    assert_eq!(
        renamed_result
            .as_ref()
            .err()
            .and_then(FilesystemStorageError::io_error)
            .map(std::io::Error::kind),
        native_result.as_ref().err().map(std::io::Error::kind),
    );

    match renamed_result {
        Ok(()) => {
            let renamed = filesystem
                .resolve_path_target(SandboxPath::at_root("renamed-directory"))
                .await
                .unwrap();
            filesystem.remove_directory(renamed.target()).await.unwrap();
            drop(renamed);
        }
        Err(_) => {
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
            filesystem
                .remove_directory(resolved.target())
                .await
                .unwrap();
        }
    }

    drop(resolved);
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn namespace_resolution_fails_when_the_parent_cannot_be_searched() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let locked = filesystem.root().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();

    let resolved = filesystem
        .resolve_path_target(SandboxPath::at_root("locked/child"))
        .await;

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        resolved.is_err(),
        "a metadata error other than not found must fail the resolution"
    );
    drop(resolved);
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn following_a_symlink_loop_fails_and_a_dangling_symlink_is_not_read_only() {
    let parent = tempfile::tempdir().unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    std::os::unix::fs::symlink("loop", filesystem.root().join("loop")).unwrap();
    std::os::unix::fs::symlink("missing", filesystem.root().join("dangling")).unwrap();

    let looped = filesystem
        .resolve_path_target(SandboxPath::at_root("loop"))
        .await
        .unwrap();
    let dangling = filesystem
        .resolve_path_target(SandboxPath::at_root("dangling"))
        .await
        .unwrap();

    assert!(
        looped.is_read_only_file(SandboxFollow::Yes).is_err(),
        "following a symlink loop must fail"
    );
    assert!(!looped.is_read_only_file(SandboxFollow::No).unwrap());
    assert!(
        !dangling
            .is_read_only_file(SandboxFollow::Yes)
            .expect("a dangling symlink must resolve to no file"),
    );
    drop(looped);
    drop(dangling);
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
fn namespace_resolution_final_identity_probe_is_metadata_only() {
    let source = include_str!("mod.rs");
    let resolver = source
        .split_once("fn resolve_host_path_target(")
        .unwrap()
        .1
        .split_once("fn native_name_comparison_mode(")
        .unwrap()
        .0;

    assert!(resolver.contains("parent.symlink_metadata(&name)"));
    assert!(!resolver.contains("parent.open_dir_nofollow(&name)"));
}
