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
use crate::filesystem::blocking::BlockingTask;
use crate::filesystem::host_directories::HostDirectory;
use crate::filesystem::host_directories::HostPath;
use crate::filesystem::sandbox::SandboxAccessMode;
use crate::filesystem::sandbox::SandboxFileDisposition;
use crate::filesystem::sandbox::SandboxFilesystemAdapter;
use crate::filesystem::sandbox::SandboxFlushLevel;
use crate::filesystem::sandbox::SandboxFollow;
use crate::filesystem::sandbox::SandboxObjectKind;
use crate::filesystem::sandbox::SandboxOpenOptions;
use crate::filesystem::sandbox::SandboxReadRange;
use crate::filesystem::sandbox::SandboxSymlinkTarget;
use crate::filesystem::sandbox::SandboxTimeChange;
use crate::filesystem::sandbox::SandboxTimeChanges;
use crate::filesystem::sandbox::SandboxWritePlacement;
use crate::filesystem::sandbox::SeedAccess;
use crate::filesystem::sandbox::SeedEntry;
use crate::filesystem::sandbox::SeedPlacement;
use crate::filesystem::sandbox::errors::FilesystemStorageError;
use crate::filesystem::sandbox::host_adapter::NativeOpenFlags;
use crate::filesystem::sandbox::host_adapter::OpenRequest;
use crate::filesystem::sandbox::host_adapter::SandboxFilesystem;
use crate::filesystem::sandbox::host_adapter::check_opened_kind;
use crate::filesystem::sandbox::host_adapter::directory_for;
use crate::filesystem::sandbox::host_adapter::is_single_normal_component;
use crate::filesystem::sandbox::host_adapter::native_open_flags;
use crate::filesystem::sandbox::host_adapter::open_request;
use crate::filesystem::sandbox::host_adapter::sandbox_provisioning::SandboxFilesystemName;
use crate::filesystem::sandbox::host_adapter::sandbox_provisioning::SandboxFilesystemProvisioning;
#[cfg(test)]
use crate::filesystem::sandbox::open_handles::AppendCoordinationCounts;
use crate::filesystem::sandbox::open_handles::SandboxDirectory;
use crate::filesystem::sandbox::open_handles::SandboxHandle;
use crate::filesystem::sandbox::sandbox_paths::SandboxPath;
#[cfg(target_os = "linux")]
use crate::filesystem::volume::FilesystemIdentity;
use crate::filesystem::volume::FilesystemVolume;
use crate::services::golem_config::FilesystemStorageMode;
use bytes::Bytes;
use golem_common::model::RetryConfig;
use std::ffi::OsString;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use test_r::test;

#[test]
#[test_r::timeout("30s")]
async fn only_bounded_work_on_known_local_storage_can_run_in_place() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory {
            root: parent.path().into(),
        },
        RetryConfig::default(),
    )
    .unwrap();
    let name = SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "scheduling".to_string(),
    )
    .unwrap();
    let mut filesystem = provisioning.create_fresh(name).await.unwrap();
    assert_eq!(filesystem.short_task(), BlockingTask::Spawn);
    for bytes in [0, 64 * 1024, 64 * 1024 + 1] {
        assert_eq!(filesystem.transfer_task(bytes), BlockingTask::Spawn);
    }

    // Only the volume mode is read by the scheduling policy.
    let root = tempfile::NamedTempFile::new().unwrap();
    filesystem.volume = FilesystemVolume::copy_on_write(
        Arc::new(root.reopen().unwrap()),
        #[cfg(target_os = "linux")]
        FilesystemIdentity { device: 1 },
    );
    assert_eq!(filesystem.short_task(), BlockingTask::InPlace);
    assert_eq!(filesystem.transfer_task(0), BlockingTask::InPlace);
    assert_eq!(filesystem.transfer_task(64 * 1024), BlockingTask::InPlace);
    assert_eq!(filesystem.transfer_task(64 * 1024 + 1), BlockingTask::Spawn);

    async fn spawned<F: Future>(future: F) -> F::Output {
        let (result, modes) = crate::filesystem::blocking::record_tasks(future).await;
        assert!(!modes.is_empty(), "the operation must dispatch native work");
        assert!(
            modes.iter().all(|mode| *mode == BlockingTask::Spawn),
            "{modes:?}"
        );
        result
    }

    let directory = filesystem
        .open(
            SandboxPath::at_root("."),
            existing_open(SandboxObjectKind::Directory, SandboxAccessMode::Read),
        )
        .await
        .unwrap();
    let SandboxHandle::Directory(opened) = &directory else {
        panic!("root must be a directory")
    };
    spawned(filesystem.read_directory(opened)).await.unwrap();
    let target = create_scratch_directory(parent.path()).await;
    spawned(filesystem.copy_contents(
        SandboxPath::at_root("."),
        Arc::new(TreeExclusions::new(std::iter::empty::<PathBuf>())),
        target.path(),
    ))
    .await
    .unwrap();
    spawned(filesystem.seed(Box::new([]))).await.unwrap();
    let file = filesystem
        .open(
            SandboxPath::at_root("flush"),
            file_open(
                SandboxAccessMode::ReadWrite,
                SandboxFileDisposition::CreateExclusive,
            ),
        )
        .await
        .unwrap();
    spawned(filesystem.flush(&file, SandboxFlushLevel::DataAndMetadata))
        .await
        .unwrap();
    filesystem.close(file).await.unwrap();
    filesystem.close(directory).await.unwrap();

    #[cfg(target_os = "linux")]
    {
        // The synthetic volume has no quota client. The calls fail after dispatch without issuing
        // a quota syscall, which still verifies the scheduling of all three quota entry points.
        filesystem.quota_authority = QuotaAuthority::Project {
            project_id: NonZeroU32::new(1).unwrap(),
            filesystem_block_bytes: NonZeroU64::new(4096).unwrap(),
        };
        assert!(spawned(filesystem.observe_allocation()).await.is_err());
        assert!(
            spawned(filesystem.allocation_reader().read_allocation())
                .await
                .is_err()
        );
        assert!(
            spawned(
                filesystem.install_limits(crate::filesystem::volume::FilesystemLimits {
                    allocated_bytes: 4096,
                    filesystem_objects: 1,
                })
            )
            .await
            .is_err()
        );
        filesystem.quota_authority = QuotaAuthority::Unsupported;
    }
    target.discard().await.unwrap();
    filesystem.delete_and_verify().await.unwrap();
}

fn existing_open(expected: SandboxObjectKind, access: SandboxAccessMode) -> SandboxOpenOptions {
    SandboxOpenOptions::Existing {
        expected,
        access,
        follow: SandboxFollow::No,
    }
}

fn file_open(access: SandboxAccessMode, disposition: SandboxFileDisposition) -> SandboxOpenOptions {
    SandboxOpenOptions::File {
        access,
        disposition,
        follow: SandboxFollow::No,
    }
}

fn open_flags(
    read: bool,
    write: bool,
    create: bool,
    create_new: bool,
    truncate: bool,
) -> NativeOpenFlags {
    NativeOpenFlags {
        read,
        write,
        create,
        create_new,
        truncate,
        nonblock: false,
        follow: false,
    }
}

fn host_child_path(directory: &HostDirectory, name: &str) -> HostPath {
    directory.path().child(std::ffi::OsStr::new(name)).unwrap()
}

/// Makes the host directories under `root` on development storage, and gives `.scratch`.
async fn create_scratch_directory(root: &Path) -> HostDirectory {
    crate::services::agent_filesystem::provision_executor_directories(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .await
    .unwrap()
    .1
    .scratch
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

fn make_seed_entry(
    source: HostPath,
    target: impl Into<PathBuf>,
    access: SeedAccess,
    placement: SeedPlacement,
) -> SeedEntry {
    SeedEntry {
        source,
        target: SandboxPath::at_root(target),
        access,
        placement,
    }
}

async fn seed_one_entry(
    filesystem: &SandboxFilesystem,
    entry: SeedEntry,
) -> Result<(), FilesystemStorageError> {
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(filesystem, Box::new([entry])).await
}

fn directory_sandbox_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
    SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .unwrap()
}

#[test]
fn open_request_follows_the_open_options() {
    assert_eq!(
        open_request(SandboxOpenOptions::Inspection {
            expected: SandboxObjectKind::Directory,
        }),
        OpenRequest {
            expected: SandboxObjectKind::Directory,
            access: SandboxAccessMode::Read,
            follow: SandboxFollow::No,
            disposition: None,
            inspection: true,
        },
        "an inspection reads, does not follow a symlink and creates nothing"
    );
    assert_eq!(
        open_request(SandboxOpenOptions::Existing {
            expected: SandboxObjectKind::File,
            access: SandboxAccessMode::Write,
            follow: SandboxFollow::Yes,
        }),
        OpenRequest {
            expected: SandboxObjectKind::File,
            access: SandboxAccessMode::Write,
            follow: SandboxFollow::Yes,
            disposition: None,
            inspection: false,
        },
        "an open of an existing object creates nothing"
    );
    assert_eq!(
        open_request(SandboxOpenOptions::File {
            access: SandboxAccessMode::ReadWrite,
            disposition: SandboxFileDisposition::CreateExclusive,
            follow: SandboxFollow::No,
        }),
        OpenRequest {
            expected: SandboxObjectKind::File,
            access: SandboxAccessMode::ReadWrite,
            follow: SandboxFollow::No,
            disposition: Some(SandboxFileDisposition::CreateExclusive),
            inspection: false,
        },
        "a file open expects a regular file"
    );
}

#[test]
fn an_inspection_names_exactly_one_normal_component() {
    [
        ("name", true),
        (".hidden", true),
        ("", false),
        (".", false),
        ("..", false),
        ("a/b", false),
        ("/a", false),
        ("a/", false),
        ("a//", false),
        ("a/.", false),
        ("./a", false),
    ]
    .into_iter()
    .for_each(|(path, expected)| {
        assert_eq!(
            is_single_normal_component(Path::new(path)),
            expected,
            "{path:?}"
        );
    });
}

#[test]
fn native_open_flags_follow_the_access_and_the_disposition() {
    use SandboxAccessMode::{Read, ReadAndSetTimes, ReadWrite, Write};
    use SandboxFileDisposition::{
        CreateExclusive, CreateIfMissing, CreateOrTruncate, TruncateExisting,
    };
    use SandboxObjectKind::{Directory, File};
    [
        (
            existing_open(Directory, Read),
            open_flags(true, false, false, false, false),
        ),
        (
            existing_open(Directory, Write),
            open_flags(true, false, false, false, false),
        ),
        (
            existing_open(Directory, ReadWrite),
            open_flags(true, false, false, false, false),
        ),
        (
            existing_open(File, Read),
            open_flags(true, false, false, false, false),
        ),
        (
            existing_open(File, ReadAndSetTimes),
            open_flags(true, false, false, false, false),
        ),
        (
            existing_open(File, Write),
            open_flags(false, true, false, false, false),
        ),
        (
            existing_open(File, ReadWrite),
            open_flags(true, true, false, false, false),
        ),
        (
            file_open(Read, CreateIfMissing),
            open_flags(true, true, true, false, false),
        ),
        (
            file_open(Write, CreateExclusive),
            open_flags(false, true, false, true, false),
        ),
        (
            file_open(Write, TruncateExisting),
            open_flags(false, true, false, false, true),
        ),
        (
            file_open(ReadWrite, CreateOrTruncate),
            open_flags(true, true, true, false, true),
        ),
    ]
    .into_iter()
    .for_each(|(options, expected)| {
        assert_eq!(native_open_flags(options), expected, "{options:?}");
    });
    let inspection = native_open_flags(SandboxOpenOptions::Inspection { expected: File });
    assert!(
        inspection.nonblock,
        "an inspection must not block on a special file"
    );
    assert!(inspection.read && !inspection.write && !inspection.follow);
    let followed = native_open_flags(SandboxOpenOptions::Existing {
        expected: File,
        access: Read,
        follow: SandboxFollow::Yes,
    });
    assert!(followed.follow && !followed.nonblock);
}

#[test]
fn the_opened_kind_must_be_the_expected_kind() {
    use SandboxObjectKind::{Directory, File, Symlink};
    [Directory, File, Symlink].into_iter().for_each(|kind| {
        check_opened_kind(kind, kind).unwrap();
    });
    let error = check_opened_kind(Directory, File).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(error.to_string(), "expected File, opened Directory");
}

#[test]
async fn production_adapter_executes_the_filesystem_method_families() {
    let parent = tempfile::tempdir().unwrap();
    let seed_sources = create_scratch_directory(parent.path()).await;
    std::fs::write(seed_sources.path().as_path().join("file"), b"seeded").unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let root = filesystem.root().to_path_buf();

    let directory = match <SandboxFilesystem as SandboxFilesystemAdapter>::open(
        &filesystem,
        SandboxPath::at_root("."),
        SandboxOpenOptions::Existing {
            expected: SandboxObjectKind::Directory,
            access: SandboxAccessMode::Read,
            follow: SandboxFollow::Yes,
        },
    )
    .await
    .unwrap()
    {
        SandboxHandle::Directory(directory) => directory,
        SandboxHandle::File(_) => panic!("root must open as a directory"),
    };
    let file = match <SandboxFilesystem as SandboxFilesystemAdapter>::open(
        &filesystem,
        SandboxPath::at_root("file"),
        SandboxOpenOptions::File {
            access: SandboxAccessMode::ReadWrite,
            disposition: SandboxFileDisposition::CreateOrTruncate,
            follow: SandboxFollow::Yes,
        },
    )
    .await
    .unwrap()
    {
        SandboxHandle::File(file) => file,
        SandboxHandle::Directory(_) => panic!("file must open as a file"),
    };

    let attempt = <SandboxFilesystem as SandboxFilesystemAdapter>::write(
        &filesystem,
        &file,
        SandboxWritePlacement::At(0),
        Bytes::from_static(b"contents"),
    )
    .await
    .unwrap();
    assert_eq!(attempt.written, 8);
    assert!(attempt.result.is_ok());
    assert_eq!(
        <SandboxFilesystem as SandboxFilesystemAdapter>::read(
            &filesystem,
            &file,
            SandboxReadRange {
                offset: 2,
                length: 4,
            },
        )
        .await
        .unwrap(),
        Bytes::from_static(b"nten")
    );
    <SandboxFilesystem as SandboxFilesystemAdapter>::set_size(&filesystem, &file, 5)
        .await
        .unwrap();
    let attributes = <SandboxFilesystem as SandboxFilesystemAdapter>::get_handle_attributes(
        &filesystem,
        SandboxHandle::File(file.clone()),
    )
    .await
    .unwrap();
    assert_eq!(attributes.kind, SandboxObjectKind::File);
    assert_eq!(attributes.size, 5);
    <SandboxFilesystem as SandboxFilesystemAdapter>::set_handle_times(
        &filesystem,
        SandboxHandle::File(file.clone()),
        SandboxTimeChanges {
            accessed: SandboxTimeChange::Keep,
            modified: SandboxTimeChange::Now,
        },
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::flush(
        &filesystem,
        &SandboxHandle::File(file.clone()),
        SandboxFlushLevel::DataAndMetadata,
    )
    .await
    .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::create_directory(
        &filesystem,
        SandboxPath::at(directory.clone(), "directory"),
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::create_symlink(
        &filesystem,
        SandboxPath::at(directory.clone(), "link"),
        SandboxSymlinkTarget(PathBuf::from("file")),
    )
    .await
    .unwrap();
    assert_eq!(
        <SandboxFilesystem as SandboxFilesystemAdapter>::read_link(
            &filesystem,
            SandboxPath::at(directory.clone(), "link"),
        )
        .await
        .unwrap(),
        SandboxSymlinkTarget(PathBuf::from("file"))
    );
    <SandboxFilesystem as SandboxFilesystemAdapter>::hard_link(
        &filesystem,
        SandboxPath::at_root("file"),
        SandboxPath::at_root("hard"),
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::rename(
        &filesystem,
        SandboxPath::at_root("hard"),
        SandboxPath::at_root("renamed"),
    )
    .await
    .unwrap();
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            host_child_path(&seed_sources, "file"),
            "seeded",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();

    let mut entries =
        <SandboxFilesystem as SandboxFilesystemAdapter>::read_directory(&filesystem, &directory)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            OsString::from("directory"),
            OsString::from("file"),
            OsString::from("link"),
            OsString::from("renamed"),
            OsString::from("seeded"),
        ]
    );
    assert!(
        <SandboxFilesystem as SandboxFilesystemAdapter>::observe_allocation(&filesystem)
            .await
            .is_err()
    );

    <SandboxFilesystem as SandboxFilesystemAdapter>::close(&filesystem, SandboxHandle::File(file))
        .await
        .unwrap();
    for path in ["file", "link", "renamed", "seeded"] {
        <SandboxFilesystem as SandboxFilesystemAdapter>::unlink_file(
            &filesystem,
            SandboxPath::at(directory.clone(), path),
        )
        .await
        .unwrap();
    }
    <SandboxFilesystem as SandboxFilesystemAdapter>::remove_directory(
        &filesystem,
        SandboxPath::at(directory.clone(), "directory"),
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::close(
        &filesystem,
        SandboxHandle::Directory(directory),
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert!(!root.exists());
}

#[test]
async fn hard_link_and_rename_keep_exact_roles_across_distinct_resolved_parents() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::create_dir(filesystem.root().join("source-parent")).unwrap();
    std::fs::create_dir(filesystem.root().join("destination-parent")).unwrap();
    std::fs::write(
        filesystem.root().join("source-parent/hard-source"),
        b"hard-link contents",
    )
    .unwrap();
    std::fs::write(
        filesystem.root().join("source-parent/rename-source"),
        b"rename contents",
    )
    .unwrap();
    let source_parent = open_sandbox_directory(&filesystem, "source-parent").await;
    let destination_parent = open_sandbox_directory(&filesystem, "destination-parent").await;

    let hard_source = filesystem
        .resolve_path_target(SandboxPath::at(source_parent.clone(), "hard-source"))
        .await
        .unwrap();
    let hard_destination = filesystem
        .resolve_path_target(SandboxPath::at(
            destination_parent.clone(),
            "hard-destination",
        ))
        .await
        .unwrap();
    filesystem
        .hard_link(hard_source.target(), hard_destination.target())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(
            filesystem
                .root()
                .join("destination-parent/hard-destination")
        )
        .unwrap(),
        b"hard-link contents"
    );
    assert!(filesystem.root().join("source-parent/hard-source").exists());

    let rename_source = filesystem
        .resolve_path_target(SandboxPath::at(source_parent.clone(), "rename-source"))
        .await
        .unwrap();
    let rename_destination = filesystem
        .resolve_path_target(SandboxPath::at(
            destination_parent.clone(),
            "rename-destination",
        ))
        .await
        .unwrap();
    filesystem
        .rename(rename_source.target(), rename_destination.target())
        .await
        .unwrap();
    assert!(
        !filesystem
            .root()
            .join("source-parent/rename-source")
            .exists()
    );
    assert_eq!(
        std::fs::read(
            filesystem
                .root()
                .join("destination-parent/rename-destination")
        )
        .unwrap(),
        b"rename contents"
    );

    drop((
        hard_source,
        hard_destination,
        rename_source,
        rename_destination,
        source_parent,
        destination_parent,
    ));
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn positioned_write_preserves_the_shared_file_cursor() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let file = match <SandboxFilesystem as SandboxFilesystemAdapter>::open(
        &filesystem,
        SandboxPath::at_root("positioned-write"),
        SandboxOpenOptions::File {
            access: SandboxAccessMode::ReadWrite,
            disposition: SandboxFileDisposition::CreateOrTruncate,
            follow: SandboxFollow::Yes,
        },
    )
    .await
    .unwrap()
    {
        SandboxHandle::File(file) => file,
        SandboxHandle::Directory(_) => panic!("positioned-write must open as a file"),
    };
    let mut cursor = file
        .host()
        .unwrap()
        .as_ref()
        .try_clone()
        .unwrap()
        .into_std();
    cursor.seek(SeekFrom::Start(11)).unwrap();
    assert_eq!(
        filesystem.append_coordinators.counts(),
        AppendCoordinationCounts {
            lookups: 0,
            allocations: 0,
            lock_acquisitions: 0,
            registered: 0,
            live: 0,
        }
    );

    let attempt = <SandboxFilesystem as SandboxFilesystemAdapter>::write(
        &filesystem,
        &file,
        SandboxWritePlacement::At(2),
        Bytes::from_static(b"data"),
    )
    .await
    .unwrap();

    assert_eq!(attempt.written, 4);
    assert!(attempt.result.is_ok());
    assert_eq!(cursor.stream_position().unwrap(), 11);
    assert_eq!(
        std::fs::read(filesystem.root().join("positioned-write")).unwrap(),
        b"\0\0data"
    );
    assert_eq!(
        filesystem.append_coordinators.counts(),
        AppendCoordinationCounts {
            lookups: 0,
            allocations: 0,
            lock_acquisitions: 0,
            registered: 0,
            live: 0,
        }
    );

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn path_attributes_report_write_permission_and_object_identity() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::write(root.join("file"), b"file").unwrap();
    std::fs::set_permissions(root.join("file"), std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::hard_link(root.join("file"), root.join("alias")).unwrap();
    std::fs::write(root.join("other"), b"other").unwrap();
    let read = |path: &'static str| {
        let filesystem = &filesystem;
        async move {
            <SandboxFilesystem as SandboxFilesystemAdapter>::get_path_attributes(
                filesystem,
                SandboxPath::at_root(path),
                SandboxFollow::No,
            )
            .await
            .unwrap()
        }
    };

    let (file, alias, other) = (read("file").await, read("alias").await, read("other").await);

    assert!(file.read_only);
    assert!(!other.read_only);
    assert_eq!(file.link_count, 2);
    assert_eq!(file.object, alias.object);
    assert_ne!(file.object, other.object);
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn capability_relative_path_operations_never_fall_back_to_root() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::create_dir(filesystem.root().join("base")).unwrap();
    std::fs::write(filesystem.root().join("same-name"), b"root").unwrap();
    std::fs::write(filesystem.root().join("base/same-name"), b"capability").unwrap();
    let base = open_sandbox_directory(&filesystem, "base").await;

    let file = match filesystem
        .open(
            SandboxPath::at(base.clone(), "same-name"),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::File,
                access: SandboxAccessMode::Read,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
    {
        SandboxHandle::File(file) => file,
        SandboxHandle::Directory(_) => panic!("same-name must open as a file"),
    };
    assert_eq!(
        filesystem
            .read(
                &file,
                SandboxReadRange {
                    offset: 0,
                    length: 10,
                },
            )
            .await
            .unwrap(),
        Bytes::from_static(b"capability")
    );

    drop((file, base));
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn empty_root_path_attributes_address_root() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();

    for follow in [SandboxFollow::No, SandboxFollow::Yes] {
        let attributes = filesystem
            .get_path_attributes(SandboxPath::at_root(""), follow)
            .await
            .unwrap();
        assert_eq!(attributes.kind, SandboxObjectKind::Directory);
    }
}

#[cfg(unix)]
#[test]
async fn attribute_path_follow_mode_is_precise() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("target"), b"contents").unwrap();
    std::os::unix::fs::symlink("target", filesystem.root().join("link")).unwrap();

    let link_attributes = filesystem
        .get_path_attributes(SandboxPath::at_root("link"), SandboxFollow::No)
        .await
        .unwrap();
    let followed_attributes = filesystem
        .get_path_attributes(SandboxPath::at_root("link"), SandboxFollow::Yes)
        .await
        .unwrap();
    assert_eq!(link_attributes.kind, SandboxObjectKind::Symlink);
    assert_eq!(followed_attributes.kind, SandboxObjectKind::File);

    let target_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
    let link_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20_000);
    filesystem
        .set_path_times(
            SandboxPath::at_root("link"),
            SandboxFollow::Yes,
            SandboxTimeChanges {
                accessed: SandboxTimeChange::Keep,
                modified: SandboxTimeChange::Set(target_time),
            },
        )
        .await
        .unwrap();
    filesystem
        .set_path_times(
            SandboxPath::at_root("link"),
            SandboxFollow::No,
            SandboxTimeChanges {
                accessed: SandboxTimeChange::Keep,
                modified: SandboxTimeChange::Set(link_time),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(filesystem.root().join("target"))
            .unwrap()
            .modified()
            .unwrap(),
        target_time
    );
    assert_eq!(
        std::fs::symlink_metadata(filesystem.root().join("link"))
            .unwrap()
            .modified()
            .unwrap(),
        link_time
    );

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn root_relative_operations_share_one_capability_without_descriptor_duplication() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let root_directory = filesystem.root_directory_state();
    let (first, second) = BlockingTask::Spawn
        .run(move || {
            let target = SandboxPath::at_root(".");
            Ok::<_, std::io::Error>((
                directory_for(&root_directory, &target)?,
                directory_for(&root_directory, &target)?,
            ))
        })
        .await
        .unwrap()
        .unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    drop((first, second));
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn shared_root_capability_keeps_root_relative_paths_confined() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let escaped = filesystem.root().parent().unwrap().join("escaped");

    let error = filesystem
        .create_directory(SandboxPath::at_root("../escaped"))
        .await
        .unwrap_err();

    assert!(matches!(
        error.io_kind(),
        Some(std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied)
    ));
    assert!(!escaped.exists());
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn concurrent_reads_and_independent_namespace_operations_share_the_root_capability() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    std::fs::write(filesystem.root().join("first"), b"first").unwrap();
    std::fs::write(filesystem.root().join("second"), b"second").unwrap();

    let (first, second, _, _) = tokio::try_join!(
        filesystem.get_path_attributes(SandboxPath::at_root("first"), SandboxFollow::Yes),
        filesystem.get_path_attributes(SandboxPath::at_root("second"), SandboxFollow::Yes),
        filesystem.create_directory(SandboxPath::at_root("left")),
        filesystem.create_directory(SandboxPath::at_root("right")),
    )
    .unwrap();

    assert_eq!(first.size, 5);
    assert_eq!(second.size, 6);
    assert!(filesystem.root().join("left").is_dir());
    assert!(filesystem.root().join("right").is_dir());
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}
