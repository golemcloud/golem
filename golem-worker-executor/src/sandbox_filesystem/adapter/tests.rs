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

#[cfg(target_os = "linux")]
use super::namespace::native_directory_coordination_key;
#[cfg(target_os = "macos")]
use super::native::admit_root;
#[cfg(target_os = "linux")]
use super::native::namespace_operation_error;
use super::*;
#[cfg(target_os = "macos")]
use std::io::Write;
use test_r::test;

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

fn existing(expected: SandboxObjectKind, access: SandboxAccessMode) -> SandboxOpenOptions {
    SandboxOpenOptions::Existing {
        expected,
        access,
        follow: SandboxFollow::No,
    }
}

fn file(access: SandboxAccessMode, disposition: SandboxFileDisposition) -> SandboxOpenOptions {
    SandboxOpenOptions::File {
        access,
        disposition,
        follow: SandboxFollow::No,
    }
}

fn flags(
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

#[test]
fn native_open_flags_follow_the_access_and_the_disposition() {
    use SandboxAccessMode::{Read, ReadAndSetTimes, ReadWrite, Write};
    use SandboxFileDisposition::{
        CreateExclusive, CreateIfMissing, CreateOrTruncate, TruncateExisting,
    };
    use SandboxObjectKind::{Directory, File};
    [
        (
            existing(Directory, Read),
            flags(true, false, false, false, false),
        ),
        (
            existing(Directory, Write),
            flags(true, false, false, false, false),
        ),
        (
            existing(Directory, ReadWrite),
            flags(true, false, false, false, false),
        ),
        (
            existing(File, Read),
            flags(true, false, false, false, false),
        ),
        (
            existing(File, ReadAndSetTimes),
            flags(true, false, false, false, false),
        ),
        (
            existing(File, Write),
            flags(false, true, false, false, false),
        ),
        (
            existing(File, ReadWrite),
            flags(true, true, false, false, false),
        ),
        (
            file(Read, CreateIfMissing),
            flags(true, true, true, false, false),
        ),
        (
            file(Write, CreateExclusive),
            flags(false, true, false, true, false),
        ),
        (
            file(Write, TruncateExisting),
            flags(false, true, false, false, true),
        ),
        (
            file(ReadWrite, CreateOrTruncate),
            flags(true, true, true, false, true),
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

/// Makes the host directories under `root` on development storage, and gives `.scratch`.
async fn host_directory_at(root: &Path) -> HostDirectory {
    SandboxFilesystemProvisioning::provision(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .await
    .unwrap()
    .1
    .scratch
}

fn host_child(directory: &HostDirectory, name: &str) -> HostPath {
    directory.path().child(std::ffi::OsStr::new(name)).unwrap()
}

fn seed_entry(
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

async fn seed_one(
    filesystem: &SandboxFilesystem,
    entry: SeedEntry,
) -> Result<(), FilesystemStorageError> {
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(filesystem, Box::new([entry])).await
}

#[cfg(target_os = "linux")]
#[test]
async fn an_empty_standard_seed_succeeds_after_root_retirement() {
    let parent = tempfile::tempdir().unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    filesystem.root.close();
    filesystem.seed(Box::default()).await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "macos")]
fn macos_seed_provisionings(parent: &Path) -> Box<[SandboxFilesystemProvisioning]> {
    [
        FilesystemStorageMode::Temporary,
        FilesystemStorageMode::Directory {
            root: parent.join("directory-agents").into_boxed_path(),
        },
        FilesystemStorageMode::Apfs {
            root: parent.join("apfs-agents").into_boxed_path(),
        },
    ]
    .iter()
    .map(|storage| SandboxFilesystemProvisioning::new(storage, RetryConfig::default()).unwrap())
    .collect()
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_empty_seed_preserves_read_only_root_namespace_and_mtime() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    futures::future::join_all(
        macos_seed_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| async move {
                let filesystem = provisioning.create_fresh(name()).await.unwrap();
                let root = filesystem.root().to_path_buf().into_boxed_path();
                let modified =
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_100);
                File::open(&root).unwrap().set_modified(modified).unwrap();
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
                let result = filesystem.seed(Box::new([])).await;
                let attributes = std::fs::metadata(&root).unwrap();
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
                result.unwrap();
                assert_eq!(attributes.permissions().mode() & 0o777, 0);
                assert_eq!(attributes.modified().unwrap(), modified);
                assert!(std::fs::read_dir(&root).unwrap().next().is_none());
                SandboxFilesystem::delete_and_verify(filesystem)
                    .await
                    .unwrap();
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_seed_uses_a_writable_child_capability_under_an_unsearchable_root() {
    use cap_std::fs::PermissionsExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let source = host_child(&sources, "file");
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_101);
    std::fs::write(source.as_path(), b"seeded through child").unwrap();
    std::fs::set_permissions(source.as_path(), std::fs::Permissions::from_mode(0o640)).unwrap();
    File::open(source.as_path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    futures::future::join_all(
        macos_seed_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let source = source.clone();
                async move {
                    let filesystem = provisioning.create_fresh(name()).await.unwrap();
                    let root = filesystem.root().to_path_buf().into_boxed_path();
                    std::fs::create_dir(root.join("child")).unwrap();
                    let child = open_native_directory(&filesystem, "child").await;
                    File::open(&root).unwrap().set_modified(modified).unwrap();
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                    let result = filesystem
                        .seed(Box::new([SeedEntry {
                            source,
                            target: SandboxPath::at(child.clone(), "file"),
                            access: SeedAccess::ReadOnly,
                            placement: SeedPlacement::CreateNew,
                        }]))
                        .await;
                    let root_attributes = std::fs::metadata(&root).unwrap();
                    let child_attributes = child.host().unwrap().metadata("file");
                    let contents = child.host().unwrap().read("file");
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    result.unwrap();
                    let child_attributes = child_attributes.unwrap();
                    assert_eq!(child_attributes.permissions().mode() & 0o777, 0o440);
                    assert_eq!(child_attributes.modified().unwrap().into_std(), modified);
                    assert_eq!(contents.unwrap(), b"seeded through child");
                    assert_eq!(root_attributes.permissions().mode() & 0o777, 0);
                    assert_eq!(root_attributes.modified().unwrap(), modified);
                    assert_eq!(
                        tree_copy::tree_listing(&root),
                        ["child", "child/file"].map(String::from).into()
                    );
                    drop(child);
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_seed_and_batch_flush_follow_the_pinned_root_after_path_replacement() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let source = host_child(&sources, "file");
    std::fs::write(source.as_path(), b"pinned root content").unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_102);
    File::open(source.as_path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    futures::future::join_all(
        macos_seed_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let source = source.clone();
                async move {
                    let filesystem = provisioning.create_fresh(name()).await.unwrap();
                    let root = filesystem.root().to_path_buf().into_boxed_path();
                    let moved = root.with_extension("moved").into_boxed_path();
                    std::fs::rename(&root, &moved).unwrap();
                    std::fs::create_dir(&root).unwrap();
                    std::fs::write(root.join("decoy"), b"unchanged").unwrap();
                    File::open(&root).unwrap().set_modified(modified).unwrap();
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                    let result = seed_one(
                        &filesystem,
                        seed_entry(
                            source,
                            "file",
                            SeedAccess::ReadOnly,
                            SeedPlacement::CreateNew,
                        ),
                    )
                    .await;
                    let replacement_attributes = std::fs::metadata(&root).unwrap();
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    let replacement_listing = tree_copy::tree_listing(&root);
                    let seeded_content = std::fs::read(moved.join("file"));
                    let seeded_attributes = std::fs::metadata(moved.join("file"));
                    std::fs::remove_dir_all(&root).unwrap();
                    std::fs::rename(&moved, &root).unwrap();
                    result.unwrap();
                    assert_eq!(replacement_attributes.modified().unwrap(), modified);
                    assert_eq!(replacement_listing, ["decoy"].map(String::from).into());
                    assert_eq!(seeded_content.unwrap(), b"pinned root content");
                    let seeded_attributes = seeded_attributes.unwrap();
                    assert_eq!(seeded_attributes.modified().unwrap(), modified);
                    assert_eq!(seeded_attributes.permissions().mode() & 0o222, 0);
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_seed_flushes_directory_only_and_symlink_only_batches() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let tree = host_child(&sources, "tree");
    std::fs::create_dir_all(tree.as_path().join("nested/empty")).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_103);
    File::open(tree.as_path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let link = host_child(&sources, "link");
    std::os::unix::fs::symlink("missing", link.as_path()).unwrap();
    futures::future::join_all(
        macos_seed_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let tree = tree.clone();
                let link = link.clone();
                async move {
                    let filesystem = provisioning.create_fresh(name()).await.unwrap();
                    seed_one(
                        &filesystem,
                        seed_entry(
                            tree,
                            "tree",
                            SeedAccess::FromSource,
                            SeedPlacement::CreateNew,
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        std::fs::metadata(filesystem.root().join("tree"))
                            .unwrap()
                            .modified()
                            .unwrap(),
                        modified
                    );
                    seed_one(
                        &filesystem,
                        seed_entry(
                            link,
                            "link",
                            SeedAccess::FromSource,
                            SeedPlacement::CreateNew,
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        std::fs::read_link(filesystem.root().join("link")).unwrap(),
                        Path::new("missing")
                    );
                    assert_eq!(
                        tree_copy::tree_listing(filesystem.root()),
                        ["link", "tree", "tree/nested", "tree/nested/empty"]
                            .map(String::from)
                            .into()
                    );
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_partial_seed_keeps_prior_entries_and_removes_temporary_names() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let source = host_child(&sources, "file");
    std::fs::write(source.as_path(), b"prior entry").unwrap();
    let link = host_child(&sources, "link");
    std::os::unix::fs::symlink("file", link.as_path()).unwrap();
    futures::future::join_all(
        macos_seed_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let source = source.clone();
                let link = link.clone();
                async move {
                    let filesystem = provisioning.create_fresh(name()).await.unwrap();
                    let result = filesystem
                        .seed(Box::new([
                            seed_entry(
                                source.clone(),
                                "ancestor/file",
                                SeedAccess::ReadOnly,
                                SeedPlacement::CreateNew,
                            ),
                            seed_entry(
                                link,
                                "ancestor/link",
                                SeedAccess::FromSource,
                                SeedPlacement::CreateNew,
                            ),
                            seed_entry(
                                source.clone(),
                                "ancestor/file",
                                SeedAccess::ReadOnly,
                                SeedPlacement::CreateNew,
                            ),
                            seed_entry(
                                source,
                                "not-visited",
                                SeedAccess::FromSource,
                                SeedPlacement::CreateNew,
                            ),
                        ]))
                        .await;
                    assert_eq!(
                        result.unwrap_err().io_kind(),
                        Some(std::io::ErrorKind::AlreadyExists)
                    );
                    assert_eq!(
                        std::fs::read(filesystem.root().join("ancestor/file")).unwrap(),
                        b"prior entry"
                    );
                    assert_eq!(
                        std::fs::read_link(filesystem.root().join("ancestor/link")).unwrap(),
                        Path::new("file")
                    );
                    assert_eq!(
                        tree_copy::tree_listing(filesystem.root()),
                        ["ancestor", "ancestor/file", "ancestor/link"]
                            .map(String::from)
                            .into()
                    );
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn apfs_seed_clones_unflushed_source_content_without_changing_its_attributes() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let source = host_child(&sources, "unflushed");
    let contents = (0..128_000)
        .map(|position| (position % 251) as u8)
        .collect::<Box<[u8]>>();
    let mut file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(source.as_path())
        .unwrap();
    file.write_all(&contents).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_104);
    file.set_modified(modified).unwrap();
    let before = file.metadata().unwrap();
    let provisioning = SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Apfs {
            root: parent.path().join("apfs-agents").into_boxed_path(),
        },
        RetryConfig::default(),
    )
    .unwrap();
    let filesystem = provisioning.create_fresh(name()).await.unwrap();
    seed_one(
        &filesystem,
        seed_entry(
            source,
            "clone",
            SeedAccess::ReadOnly,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(filesystem.root().join("clone")).unwrap(),
        contents.as_ref()
    );
    assert_eq!(
        std::fs::metadata(filesystem.root().join("clone"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    let after = file.metadata().unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.permissions(), before.permissions());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn an_admitted_macos_root_retains_its_unlinked_regular_flusher_after_retirement() {
    let parent = tempfile::tempdir().unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let state = filesystem.root_directory_state();
    let admitted = admit_root(&state).unwrap();
    let metadata = admitted
        .seed_flusher
        .as_deref()
        .unwrap()
        .metadata()
        .unwrap();
    assert!(metadata.is_file());
    assert_eq!(cap_fs_ext::MetadataExt::nlink(&metadata), 0);
    assert!(
        std::fs::read_dir(filesystem.root())
            .unwrap()
            .next()
            .is_none()
    );
    filesystem.root.close();
    assert_eq!(
        admit_root(&state).err().unwrap().kind(),
        std::io::ErrorKind::NotConnected
    );
    tree_copy::sync_seed_batch(
        tree_copy::SeedSyncPolicy::new(tree_copy::SeedSyncPlatform::MacOs, FileTransfer::Bytes),
        Some(&admitted),
        filesystem.root(),
    )
    .unwrap();
    assert!(
        admitted
            .seed_flusher
            .as_deref()
            .unwrap()
            .metadata()
            .unwrap()
            .is_file()
    );
    drop(admitted);
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[cfg(target_os = "linux")]
fn linux_parent_key(device: u64) -> SandboxDirectoryCoordinationKey {
    SandboxDirectoryCoordinationKey(NativeFileIdentity::Unix { device, inode: 1 })
}

#[cfg(target_os = "linux")]
async fn validated_name_mode_test_filesystem() -> SandboxFilesystem {
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
        NativeNameModeSource::ValidatedXfs(xfs::validated_xfs_name_mode_for_test(device)),
    )
}

#[cfg(target_os = "linux")]
fn host_directory(path: &Path) -> SandboxDirectory {
    let directory = Arc::new(cap_std::fs::Dir::from_std_file(File::open(path).unwrap()));
    let coordination_key = native_directory_coordination_key(&directory).unwrap();
    SandboxDirectory {
        handle: NativeDirectoryHandle::Host(directory),
        path: path.to_path_buf(),
        coordination_key,
    }
}

fn scripted_error(operation: &'static str) -> FilesystemStorageError {
    FilesystemStorageError::verification(operation, Path::new("<scripted-test>"))
}

#[cfg(target_os = "linux")]
#[test]
fn namespace_operation_errors_preserve_raw_terminal_errno() {
    for (operation, errno) in [
        ("hard-link sandbox filesystem path", libc::EIO),
        ("rename sandbox filesystem path", libc::ESTALE),
        ("rename sandbox filesystem path", libc::ENODEV),
    ] {
        let error = namespace_operation_error(
            operation,
            Path::new("<destination>"),
            std::io::Error::from_raw_os_error(errno),
        );
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(errno)
        );
        assert!(error.is_terminal_failure());
    }

    let cross_device = namespace_operation_error(
        "hard-link sandbox filesystem path",
        Path::new("<destination>"),
        std::io::Error::from_raw_os_error(libc::EXDEV),
    );
    assert_eq!(
        cross_device
            .io_error()
            .and_then(std::io::Error::raw_os_error),
        Some(libc::EXDEV)
    );
}

async fn create_scripted(
    provisioning: ScriptedSandboxFilesystemProvisioning,
) -> ScriptedSandboxFilesystem {
    <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        name(),
        None,
    )
    .await
    .unwrap()
}

async fn open_native_directory(
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
        .into_node()
    {
        SandboxNode::Directory(directory) => directory,
        SandboxNode::File(_) => panic!("path must open as a directory"),
    }
}

async fn open_native_file(filesystem: &SandboxFilesystem, path: impl Into<PathBuf>) -> SandboxFile {
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
        .into_node()
    {
        SandboxNode::File(file) => file,
        SandboxNode::Directory(_) => panic!("path must open as a file"),
    }
}

#[test]
async fn scripted_adapter_returns_programmed_outcomes_and_records_exact_order() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let seed_parent = tempfile::tempdir().unwrap();
    let seed_directory = host_directory_at(seed_parent.path()).await;
    let seed_source = host_child(&seed_directory, "seed-source");
    let seed_call = format!(
        "seed(entries=[{{source={}, target=SandboxPath {{ base: Root, path: \"seed-destination\" }}, access=ReadWrite, placement=CreateNew}}])",
        seed_source.as_path().display()
    );
    let file = SandboxFile::scripted(7);
    let directory = SandboxDirectory::scripted(9);
    let attributes = SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 12,
        accessed: None,
        modified: None,
        read_only: false,
        object: SandboxObjectId::scripted(12),
    };
    let limits = FilesystemLimits {
        allocated_bytes: 4096,
        filesystem_objects: 8,
    };
    let allocation = FilesystemAllocation {
        allocated_bytes: 1024,
        filesystem_objects: 3,
    };
    control.push_open(Ok(SandboxOpened::scripted_file(7)));
    control.push_read(Ok(Bytes::from_static(b"programmed")));
    control.push_read_directory(Ok(vec![SandboxDirectoryEntry {
        name: OsString::from("entry"),
        kind: SandboxObjectKind::Directory,
    }]));
    control.push_read_link(Ok(SandboxSymlinkTarget(PathBuf::from("target"))));
    control.push_write(Ok(SandboxWriteAttempt::completed(4)));
    control.push_get_attributes(Ok(attributes.clone()));
    control.push_set_size(Ok(()));
    control.push_set_times(Ok(()));
    control.push_create_directory(Ok(()));
    control.push_create_symlink(Ok(()));
    control.push_hard_link(Ok(()));
    control.push_rename(Ok(()));
    control.push_remove_directory(Ok(()));
    control.push_unlink_file(Ok(()));
    control.push_flush(Ok(()));
    control.push_close(Ok(()));
    control.push_seed(Ok(()));
    control.push_observe_allocation(Ok(allocation));
    control.push_install_limits(Ok(InstalledLimits { limits, allocation }));
    control.push_delete_and_verify(Ok(()));

    let filesystem = create_scripted(provisioning).await;
    let opened = filesystem
        .open(
            SandboxPath::at_root("opened"),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::File,
                access: SandboxAccessMode::Read,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap();
    assert!(matches!(opened.into_node(), SandboxNode::File(_)));
    assert_eq!(
        filesystem
            .read(
                &file,
                SandboxReadRange {
                    offset: 2,
                    length: 10,
                },
            )
            .await
            .unwrap(),
        Bytes::from_static(b"programmed")
    );
    assert_eq!(
        filesystem.read_directory(&directory).await.unwrap(),
        vec![SandboxDirectoryEntry {
            name: OsString::from("entry"),
            kind: SandboxObjectKind::Directory,
        }]
    );
    assert_eq!(
        filesystem
            .read_link(SandboxPath::at(directory.clone(), "link"))
            .await
            .unwrap(),
        SandboxSymlinkTarget(PathBuf::from("target"))
    );
    let attempt = filesystem
        .write(
            &file,
            SandboxWritePlacement::At(3),
            Bytes::from_static(b"data"),
        )
        .await
        .unwrap();
    assert_eq!(attempt.written, 4);
    assert!(attempt.result.is_ok());
    assert_eq!(
        filesystem
            .get_node_attributes(SandboxNode::File(file.clone()))
            .await
            .unwrap(),
        attributes
    );
    filesystem.set_size(&file, 20).await.unwrap();
    filesystem
        .set_node_times(
            SandboxNode::File(file.clone()),
            SandboxTimeChanges {
                accessed: SandboxTimeChange::Keep,
                modified: SandboxTimeChange::Now,
            },
        )
        .await
        .unwrap();
    filesystem
        .create_directory(SandboxPath::at(directory.clone(), "new-dir"))
        .await
        .unwrap();
    filesystem
        .create_symlink(
            SandboxPath::at(directory.clone(), "new-link"),
            SandboxSymlinkTarget(PathBuf::from("target")),
        )
        .await
        .unwrap();
    filesystem
        .hard_link(
            SandboxPath::at(directory.clone(), "source"),
            SandboxPath::at(directory.clone(), "hard"),
        )
        .await
        .unwrap();
    filesystem
        .rename(
            SandboxPath::at(directory.clone(), "before"),
            SandboxPath::at(directory.clone(), "after"),
        )
        .await
        .unwrap();
    filesystem
        .remove_directory(SandboxPath::at(directory.clone(), "old-dir"))
        .await
        .unwrap();
    filesystem
        .unlink_file(SandboxPath::at(directory.clone(), "old-file"))
        .await
        .unwrap();
    filesystem
        .flush(
            &SandboxNode::File(file.clone()),
            SandboxFlushLevel::DataAndMetadata,
        )
        .await
        .unwrap();
    filesystem
        .close(SandboxNode::File(file.clone()))
        .await
        .unwrap();
    filesystem
        .seed(Box::new([seed_entry(
            seed_source,
            "seed-destination",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        )]))
        .await
        .unwrap();
    assert_eq!(filesystem.observe_allocation().await.unwrap(), allocation);
    assert_eq!(
        filesystem.install_limits(limits).await.unwrap(),
        InstalledLimits { limits, allocation }
    );
    <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();

    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)",
            "open(target=SandboxPath { base: Root, path: \"opened\" }, options=Existing { expected: File, access: Read, follow: Yes })",
            "read(file=file(7), range=SandboxReadRange { offset: 2, length: 10 })",
            "read_directory(directory=directory(9))",
            "read_link(path=SandboxPath { base: Directory(directory(9)), path: \"link\" })",
            "write(file=file(7), placement=At(3), bytes=b\"data\")",
            "get_node_attributes(node=File(file(7)))",
            "set_size(file=file(7), size=20)",
            "set_node_times(node=File(file(7)), times=SandboxTimeChanges { accessed: Keep, modified: Now })",
            "create_directory(path=SandboxPath { base: Directory(directory(9)), path: \"new-dir\" })",
            "create_symlink(path=SandboxPath { base: Directory(directory(9)), path: \"new-link\" }, target=SandboxSymlinkTarget(\"target\"))",
            "hard_link(source=SandboxPath { base: Directory(directory(9)), path: \"source\" }, destination=SandboxPath { base: Directory(directory(9)), path: \"hard\" })",
            "rename(source=SandboxPath { base: Directory(directory(9)), path: \"before\" }, destination=SandboxPath { base: Directory(directory(9)), path: \"after\" })",
            "remove_directory(path=SandboxPath { base: Directory(directory(9)), path: \"old-dir\" })",
            "unlink_file(path=SandboxPath { base: Directory(directory(9)), path: \"old-file\" })",
            "flush(node=File(file(7)), level=DataAndMetadata)",
            "close(node=File(file(7)))",
            seed_call.as_str(),
            "observe_allocation()",
            "install_limits(limits=FilesystemLimits { allocated_bytes: 4096, filesystem_objects: 8 })",
            "delete_and_verify()",
        ]
    );
}

#[test]
async fn scripted_adapter_surfaces_each_programmed_native_outcome() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_read(Ok(Bytes::from_static(b"first")));
    control.push_read(Err(scripted_error("changed read outcome")));
    let filesystem = create_scripted(provisioning).await;
    let file = SandboxFile::scripted(1);
    let range = SandboxReadRange {
        offset: 0,
        length: 5,
    };

    assert_eq!(
        filesystem.read(&file, range).await.unwrap(),
        Bytes::from_static(b"first")
    );
    assert_eq!(
        filesystem.read(&file, range).await.unwrap_err().to_string(),
        "failed to changed read outcome filesystem <scripted-test>"
    );
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)",
            "read(file=file(1), range=SandboxReadRange { offset: 0, length: 5 })",
            "read(file=file(1), range=SandboxReadRange { offset: 0, length: 5 })",
        ]
    );
}

#[test]
async fn failed_delete_returns_adapter_for_retry() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_delete_and_verify(Err(scripted_error("first delete")));
    control.push_delete_and_verify(Ok(()));
    let filesystem = create_scripted(provisioning).await;

    let failure = match <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(
        filesystem,
    )
    .await
    {
        Ok(()) => panic!("first deletion unexpectedly succeeded"),
        Err(failure) => failure,
    };
    let (filesystem, error) = failure.into_parts();
    assert_eq!(
        error.to_string(),
        "failed to first delete filesystem <scripted-test>"
    );

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
async fn scripted_namespace_resolution_exposes_only_opaque_semantic_facts() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_namespace_resolution(41, "equivalent", Some(42));
    control.push_namespace_resolution(41, "equivalent", Some(42));
    let filesystem = create_scripted(provisioning).await;

    let first = filesystem
        .resolve_namespace_target(SandboxPath::at_root("First"))
        .await
        .unwrap();
    let second = filesystem
        .resolve_namespace_target(SandboxPath::at_root("second"))
        .await
        .unwrap();

    assert!(first.coordination_key() == second.coordination_key());
    assert!(first.final_directory_key() == second.final_directory_key());
    assert_eq!(first.target().path, PathBuf::from("First"));
    assert!(
        !first.has_byte_exact_lookup_proof(),
        "a canonical coordination label is not a lookup proof"
    );
    assert!(!second.has_byte_exact_lookup_proof());

    let default = filesystem
        .resolve_namespace_target(SandboxPath::at_root("default"))
        .await
        .unwrap();
    assert!(default.has_byte_exact_lookup_proof());
    control.push_namespace_resolution(41, "First", Some(42));
    let same_label = filesystem
        .resolve_namespace_target(SandboxPath::at_root("First"))
        .await
        .unwrap();
    assert!(
        !same_label.has_byte_exact_lookup_proof(),
        "matching a canonical label does not prove the parent's lookup"
    );
}

fn coordination_key_hash(key: &SandboxNamespaceCoordinationKey) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn exact_native_name_mode_distinguishes_siblings() {
    let first = SandboxNamespaceCoordinationKey::scripted_exact(1, "first");
    let same = SandboxNamespaceCoordinationKey::scripted_exact(1, "first");
    let sibling = SandboxNamespaceCoordinationKey::scripted_exact(1, "second");

    assert!(first == same);
    assert_eq!(coordination_key_hash(&first), coordination_key_hash(&same));
    assert!(first != sibling);
    assert!(!first.may_conflict_with(&sibling));
}

#[test]
fn windows_insensitive_native_name_mode_compares_equivalent_names() {
    let first = SandboxNamespaceCoordinationKey::scripted_windows_insensitive(1, "CaseName");
    let equivalent = SandboxNamespaceCoordinationKey::scripted_windows_insensitive(1, "casename");
    let distinct = SandboxNamespaceCoordinationKey::scripted_windows_insensitive(1, "other");

    assert!(first == equivalent);
    assert_eq!(
        coordination_key_hash(&first),
        coordination_key_hash(&equivalent)
    );
    assert!(first != distinct);
    assert!(!first.may_conflict_with(&distinct));
}

#[test]
fn conservative_native_name_mode_is_scoped_to_one_parent() {
    let first = SandboxNamespaceCoordinationKey::scripted_conservative(1, "first");
    let sibling = SandboxNamespaceCoordinationKey::scripted_conservative(1, "second");
    let other_parent = SandboxNamespaceCoordinationKey::scripted_conservative(2, "first");
    let exact = SandboxNamespaceCoordinationKey::scripted_exact(1, "third");

    assert!(first == sibling);
    assert_eq!(
        coordination_key_hash(&first),
        coordination_key_hash(&sibling)
    );
    assert!(first != other_parent);
    assert!(!first.may_conflict_with(&other_parent));
    assert!(first != exact);
    assert!(first.may_conflict_with(&exact));
}

#[test]
fn apfs_conservative_keys_cover_normalization_aliases_and_parent_scope() {
    let composed = SandboxNamespaceCoordinationKey::scripted_conservative(1, "caf\u{00e9}.txt");
    let decomposed = SandboxNamespaceCoordinationKey::scripted_conservative(1, "cafe\u{0301}.txt");
    let sibling = SandboxNamespaceCoordinationKey::scripted_conservative(1, "notes.txt");
    let other_parent =
        SandboxNamespaceCoordinationKey::scripted_conservative(2, "cafe\u{0301}.txt");

    assert!(composed == decomposed);
    assert_eq!(
        coordination_key_hash(&composed),
        coordination_key_hash(&decomposed)
    );
    assert!(composed.may_conflict_with(&decomposed));
    assert!(composed.may_conflict_with(&sibling));
    assert!(!composed.may_conflict_with(&other_parent));
}

#[cfg(target_os = "linux")]
#[test]
fn validated_xfs_name_mode_skips_native_detection() {
    let parent = linux_parent_key(17);
    let source = NativeNameModeSource::ValidatedXfs(xfs::validated_xfs_name_mode_for_test(17));
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
    let parent = linux_parent_key(17);
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
    let parent = linux_parent_key(17);
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
fn benchmark_disable_control_restores_managed_xfs_native_detection() {
    let parent = linux_parent_key(17);
    let source = NativeNameModeSource::ValidatedXfs(xfs::validated_xfs_name_mode_for_test(17));
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
    let filesystem = provisioning.create_fresh(name()).await.unwrap();
    std::fs::write(filesystem.root().join("CaseProbe"), b"case").unwrap();
    let case_alias_exists = filesystem.root().join("caseprobe").exists();
    let case_exact = filesystem
        .resolve_namespace_target(SandboxPath::at_root("CaseProbe"))
        .await
        .unwrap();
    let case_alias = filesystem
        .resolve_namespace_target(SandboxPath::at_root("caseprobe"))
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
    filesystem.close(created.into_node()).await.unwrap();
    std::fs::write(filesystem.root().join("notes.txt"), b"distinct").unwrap();
    let first = filesystem
        .resolve_namespace_target(SandboxPath::at_root(composed))
        .await
        .unwrap();
    let alias = filesystem
        .resolve_namespace_target(SandboxPath::at_root(decomposed))
        .await
        .unwrap();
    let sibling = filesystem
        .resolve_namespace_target(SandboxPath::at_root("notes.txt"))
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
    assert!(first.coordination_key() == alias.coordination_key());
    assert!(
        first
            .coordination_key()
            .may_conflict_with(&sibling.coordination_key())
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

#[cfg(target_os = "linux")]
#[test]
async fn sandbox_filesystem_constructor_preserves_validated_name_mode_for_same_filesystem_only() {
    let filesystem = validated_name_mode_test_filesystem().await;

    filesystem
        .resolve_namespace_target(SandboxPath::at_root("same-filesystem"))
        .await
        .unwrap();
    assert_eq!(filesystem.name_mode_probe_count(), 0);

    let foreign = host_directory(Path::new("/proc"));
    filesystem
        .resolve_namespace_target(SandboxPath::at(foreign, "self"))
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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();

    filesystem
        .resolve_namespace_target(SandboxPath::at_root("unmanaged-name-mode-probe"))
        .await
        .unwrap();
    assert_eq!(filesystem.name_mode_probe_count(), 1);

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn scripted_adapter_programs_creation_through_associated_provisioning() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.set_creation(Err(scripted_error("programmed creation failure")));

    let error = <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        name(),
        None,
    )
    .await
    .expect_err("scripted creation must return the programmed failure");

    assert_eq!(
        error.to_string(),
        "failed to programmed creation failure filesystem <scripted-test>"
    );
    assert_eq!(
        control.calls(),
        vec!["create_fresh(name=environment/component/filesystem, limits=None)"]
    );
}

#[test]
async fn scripted_adapter_programs_copy_contents_with_gates() {
    let parent = tempfile::tempdir().unwrap();
    let copies = host_directory_at(parent.path()).await;
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_copy_contents(Ok(Box::new([])));
    control.push_copy_contents(Err(scripted_error("programmed copy failure")));
    let filesystem = create_scripted(provisioning).await;
    let target = host_child(&copies, "copy");
    let excluded = Arc::new(TreeExclusions::new([
        PathBuf::from("lib/b.txt"),
        PathBuf::from("a.txt"),
    ]));

    let gate = control.block("copy_contents");
    let copying = tokio::spawn({
        let filesystem = filesystem.clone();
        let excluded = Arc::clone(&excluded);
        let target = target.clone();
        async move {
            filesystem
                .copy_contents(SandboxPath::at_root("data"), excluded, &target)
                .await
        }
    });
    gate.wait_started().await;
    assert!(!copying.is_finished());
    gate.release();
    gate.wait_completed().await;
    copying.await.unwrap().unwrap();
    let failed = filesystem
        .copy_contents(
            SandboxPath::at_root(""),
            Arc::new(TreeExclusions::default()),
            &target,
        )
        .await
        .unwrap_err();
    let unprogrammed = filesystem
        .copy_contents(SandboxPath::at_root(""), excluded, &target)
        .await
        .unwrap_err();

    assert_eq!(
        failed.to_string(),
        "failed to programmed copy failure filesystem <scripted-test>"
    );
    assert_eq!(
        unprogrammed.to_string(),
        "failed to consume programmed scripted sandbox filesystem outcome filesystem <scripted-sandbox-filesystem>"
    );
    let target_path = target.as_path().display();
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)".to_string(),
            format!(
                "copy_contents(source=SandboxPath {{ base: Root, path: \"data\" }}, excluded=[\"a.txt\", \"lib/b.txt\"], target={target_path})"
            ),
            format!(
                "copy_contents(source=SandboxPath {{ base: Root, path: \"\" }}, excluded=[], target={target_path})"
            ),
            format!(
                "copy_contents(source=SandboxPath {{ base: Root, path: \"\" }}, excluded=[\"a.txt\", \"lib/b.txt\"], target={target_path})"
            ),
        ]
    );
}

#[cfg(unix)]
#[test]
async fn copy_contents_copies_what_is_under_the_source_minus_the_exclusions() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let copies = host_directory_at(parent.path()).await;
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/nested/note"), b"note").unwrap();
    std::fs::write(root.join("data/db"), b"db").unwrap();
    std::fs::write(root.join("static.bin"), b"static").unwrap();
    std::fs::set_permissions(root.join("data/db"), std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("data/db", root.join("link")).unwrap();
    let whole = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("whole"))
        .await
        .unwrap();
    let part = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("part"))
        .await
        .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::new([
            PathBuf::from("static.bin"),
            PathBuf::from("data/nested"),
        ])),
        whole.path(),
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root("data"),
        Arc::new(TreeExclusions::new([PathBuf::from("db")])),
        part.path(),
    )
    .await
    .unwrap();

    let whole_root = whole.path().as_path();
    assert_eq!(
        tree_copy::tree_listing(whole_root),
        std::collections::BTreeSet::from(["data", "data/db", "link"].map(String::from))
    );
    assert_eq!(std::fs::read(whole_root.join("data/db")).unwrap(), b"db");
    assert_eq!(mode(&whole_root.join("data/db")), 0o640);
    assert_eq!(
        std::fs::read_link(whole_root.join("link")).unwrap(),
        PathBuf::from("data/db")
    );
    assert_eq!(
        tree_copy::tree_listing(part.path().as_path()),
        std::collections::BTreeSet::from(["nested", "nested/note"].map(String::from))
    );
    assert_eq!(
        std::fs::read(root.join("static.bin")).unwrap(),
        b"static",
        "the sandbox must stay as it is"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn path_attributes_report_write_permission_and_object_identity() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
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

#[cfg(unix)]
#[test]
async fn copy_contents_refuses_a_target_that_is_not_an_empty_directory() {
    let parent = tempfile::tempdir().unwrap();
    let copies = host_directory_at(parent.path()).await;
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    std::fs::write(filesystem.root().join("file"), b"file").unwrap();
    let copies_root = copies.path().as_path().to_path_buf();
    std::fs::create_dir(copies_root.join("full")).unwrap();
    std::fs::write(copies_root.join("full/kept"), b"kept").unwrap();
    std::fs::write(copies_root.join("plain-file"), b"plain").unwrap();
    std::fs::create_dir(copies_root.join("real")).unwrap();
    std::os::unix::fs::symlink("real", copies_root.join("alias")).unwrap();

    let results = futures::future::join_all(
        [
            ("full", std::io::ErrorKind::DirectoryNotEmpty),
            ("absent", std::io::ErrorKind::NotFound),
            ("plain-file", std::io::ErrorKind::NotADirectory),
            ("alias", std::io::ErrorKind::NotADirectory),
        ]
        .into_iter()
        .map(|(name, expected)| {
            let target = host_child(&copies, name);
            let filesystem = &filesystem;
            async move {
                let result = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
                    filesystem,
                    SandboxPath::at_root(""),
                    Arc::new(TreeExclusions::default()),
                    &target,
                )
                .await;
                (name, expected, result)
            }
        }),
    )
    .await;

    results.into_iter().for_each(|(name, expected, result)| {
        assert_eq!(
            result.unwrap_err().io_kind(),
            Some(expected),
            "a copy into {name} must be refused"
        );
    });
    assert_eq!(
        tree_copy::tree_listing(&copies_root),
        std::collections::BTreeSet::from(
            ["alias", "full", "full/kept", "plain-file", "real"].map(String::from)
        )
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn scripted_adapter_programs_seed_with_gates() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    control.push_seed(Ok(()));
    control.push_seed(Err(scripted_error("programmed seed failure")));
    let filesystem = create_scripted(provisioning).await;
    let entries = || -> Box<[SeedEntry]> {
        Box::new([
            seed_entry(
                host_child(&sources, "tree"),
                "",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "file"),
                "lib/file",
                SeedAccess::ReadOnly,
                SeedPlacement::Replace,
            ),
        ])
    };

    let gate = control.block("seed");
    let seeding = tokio::spawn({
        let filesystem = filesystem.clone();
        let entries = entries();
        async move { filesystem.seed(entries).await }
    });
    gate.wait_started().await;
    assert!(!seeding.is_finished());
    gate.release();
    gate.wait_completed().await;
    seeding.await.unwrap().unwrap();
    let failed = filesystem.seed(entries()).await.unwrap_err();
    let unprogrammed = filesystem.seed(entries()).await.unwrap_err();

    assert_eq!(
        failed.to_string(),
        "failed to programmed seed failure filesystem <scripted-test>"
    );
    assert_eq!(
        unprogrammed.to_string(),
        "failed to consume programmed scripted sandbox filesystem outcome filesystem <scripted-sandbox-filesystem>"
    );
    let call = format!(
        "seed(entries=[{{source={}, target=SandboxPath {{ base: Root, path: \"\" }}, access=FromSource, placement=CreateNew}}, {{source={}, target=SandboxPath {{ base: Root, path: \"lib/file\" }}, access=ReadOnly, placement=Replace}}])",
        sources.path().as_path().join("tree").display(),
        sources.path().as_path().join("file").display()
    );
    assert_eq!(
        control.calls(),
        vec![
            "create_fresh(name=environment/component/filesystem, limits=None)".to_string(),
            call.clone(),
            call.clone(),
            call,
        ]
    );
}

#[cfg(unix)]
#[test]
async fn seed_puts_entries_in_order_and_stops_at_the_first_failure() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    std::fs::write(sources.path().as_path().join("file"), b"first").unwrap();
    std::fs::write(sources.path().as_path().join("after"), b"after").unwrap();
    std::os::unix::fs::symlink("file", sources.path().as_path().join("link")).unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();

    let error = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            seed_entry(
                host_child(&sources, "file"),
                "nested/file",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "link"),
                "nested/link",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "after"),
                "nested/file",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "after"),
                "after",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap_err();

    let root = filesystem.root();
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert!(
        error.to_string().contains("nested/file"),
        "the error must name the target of the failed entry: {error}"
    );
    assert_eq!(std::fs::read(root.join("nested/file")).unwrap(), b"first");
    assert_eq!(
        std::fs::read_link(root.join("nested/link")).unwrap(),
        PathBuf::from("file")
    );
    assert!(
        !root.join("after").exists(),
        "an entry after the failed entry must not be seeded"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_access_sets_the_write_permission_of_seeded_files() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_010);
    [
        ("tool", 0o755),
        ("locked", 0o555),
        ("private", 0o640),
        ("writable", 0o644),
    ]
    .into_iter()
    .for_each(|(name, mode)| {
        let path = sources.path().as_path().join(name);
        std::fs::write(&path, name).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    });
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            seed_entry(
                host_child(&sources, "tool"),
                "tool",
                SeedAccess::ReadOnly,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "locked"),
                "locked",
                SeedAccess::ReadWrite,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "private"),
                "private",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "writable"),
                "writable",
                SeedAccess::ReadWrite,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap();

    let root = filesystem.root();
    [
        ("tool", 0o555),
        ("locked", 0o755),
        ("private", 0o640),
        ("writable", 0o644),
    ]
    .into_iter()
    .for_each(|(name, expected)| {
        assert_eq!(
            mode(&root.join(name)),
            expected,
            "{name} must have mode {expected:o}"
        );
        assert_eq!(
            std::fs::metadata(root.join(name))
                .unwrap()
                .modified()
                .unwrap(),
            modified,
            "{name} must have the modification time of its source"
        );
    });
    assert_eq!(std::fs::read(root.join("tool")).unwrap(), b"tool");
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_gives_a_made_target_directory_the_source_attributes_and_keeps_a_merged_one() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let tree = sources.path().as_path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("file"), b"file").unwrap();
    std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o750)).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    File::open(&tree).unwrap().set_modified(modified).unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::create_dir(root.join("merged")).unwrap();
    std::fs::set_permissions(root.join("merged"), std::fs::Permissions::from_mode(0o700)).unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            seed_entry(
                host_child(&sources, "tree"),
                "made",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "tree"),
                "merged",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap();

    assert_eq!(
        mode(&root.join("made")),
        0o750,
        "the made directory must have the mode of its source"
    );
    assert_eq!(
        std::fs::metadata(root.join("made"))
            .unwrap()
            .modified()
            .unwrap(),
        modified,
        "the made directory must have the modification time of its source"
    );
    assert_eq!(
        mode(&root.join("merged")),
        0o700,
        "the merged directory must keep its own mode"
    );
    assert_eq!(std::fs::read(root.join("merged/file")).unwrap(), b"file");
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_reports_the_error_of_a_directory_that_it_cannot_make() {
    use std::os::unix::fs::PermissionsExt as _;

    if running_as_root() {
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    std::fs::create_dir(sources.path().as_path().join("tree")).unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let locked = filesystem.root().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let error = seed_one(
        &filesystem,
        seed_entry(
            host_child(&sources, "tree"),
            "locked/new",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        error.io_kind(),
        Some(std::io::ErrorKind::PermissionDenied),
        "{error}"
    );
    assert!(!locked.join("new").exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_file_entries_follow_the_placement() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    std::fs::write(sources.path().as_path().join("new"), b"new").unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    ["create-new", "replace"].into_iter().for_each(|name| {
        std::fs::write(root.join(name), b"old").unwrap();
    });
    std::fs::create_dir(root.join("directory")).unwrap();
    std::fs::write(root.join("directory/child"), b"child").unwrap();
    let new = || host_child(&sources, "new");

    let failed = seed_one(
        &filesystem,
        seed_entry(
            new(),
            "create-new",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    seed_one(
        &filesystem,
        seed_entry(
            new(),
            "absent",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();
    seed_one(
        &filesystem,
        seed_entry(
            new(),
            "replace",
            SeedAccess::ReadOnly,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();
    let failed_over_directory = seed_one(
        &filesystem,
        seed_entry(
            new(),
            "directory",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert!(root.join("directory/child").is_file());
    seed_one(
        &filesystem,
        seed_entry(
            new(),
            "directory",
            SeedAccess::ReadWrite,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();

    assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(root.join("create-new")).unwrap(), b"old");
    assert_eq!(std::fs::read(root.join("absent")).unwrap(), b"new");
    assert_eq!(std::fs::read(root.join("replace")).unwrap(), b"new");
    assert_eq!(mode(&root.join("replace")) & 0o222, 0);
    assert_eq!(
        failed_over_directory.io_kind(),
        Some(std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(root.join("directory")).unwrap(), b"new");
    assert!(
        std::fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".golem-copy-")),
        "a temporary file must not stay in the root"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_symlink_entries_follow_the_placement() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    std::os::unix::fs::symlink("new-target", sources.path().as_path().join("link")).unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    ["create-new", "replace"].into_iter().for_each(|name| {
        std::os::unix::fs::symlink("old-target", root.join(name)).unwrap();
    });
    std::fs::create_dir(root.join("directory")).unwrap();
    std::fs::write(root.join("directory/child"), b"child").unwrap();
    let link = || host_child(&sources, "link");

    let failed = seed_one(
        &filesystem,
        seed_entry(
            link(),
            "create-new",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();

    seed_one(
        &filesystem,
        seed_entry(
            link(),
            "replace",
            SeedAccess::FromSource,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();
    seed_one(
        &filesystem,
        seed_entry(
            link(),
            "absent",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();
    seed_one(
        &filesystem,
        seed_entry(
            link(),
            "directory",
            SeedAccess::FromSource,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();

    assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    [
        ("create-new", "old-target"),
        ("replace", "new-target"),
        ("absent", "new-target"),
        ("directory", "new-target"),
    ]
    .into_iter()
    .for_each(|(name, expected)| {
        assert_eq!(
            std::fs::read_link(root.join(name)).unwrap(),
            PathBuf::from(expected),
            "{name} must point to {expected}"
        );
    });
    assert!(
        std::fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".golem-copy-")),
        "a temporary symlink must not stay in the root"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_reads_a_source_symlink_without_following_it() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"secret").unwrap();
    std::fs::create_dir(sources.path().as_path().join("real")).unwrap();
    std::fs::write(sources.path().as_path().join("real/file"), b"file").unwrap();
    std::os::unix::fs::symlink("real", sources.path().as_path().join("alias")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        sources.path().as_path().join("outside"),
    )
    .unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            seed_entry(
                host_child(&sources, "alias"),
                "alias",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            seed_entry(
                host_child(&sources, "outside"),
                "outside",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap();

    let root = filesystem.root();
    assert!(
        std::fs::symlink_metadata(root.join("alias"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(root.join("alias")).unwrap(),
        PathBuf::from("real")
    );
    assert!(
        std::fs::symlink_metadata(root.join("outside"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(root.join("outside")).unwrap(),
        outside.path().join("secret")
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_cannot_write_outside_the_root() {
    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    std::fs::write(sources.path().as_path().join("file"), b"file").unwrap();
    std::os::unix::fs::symlink("file", sources.path().as_path().join("link")).unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("data")).unwrap();

    let errors = futures::future::join_all(
        [
            ("file", "../escaped", SeedPlacement::CreateNew),
            ("file", "data/file", SeedPlacement::CreateNew),
            ("file", "data/file", SeedPlacement::Replace),
            ("file", "data/nested/file", SeedPlacement::CreateNew),
            ("link", "data/link", SeedPlacement::CreateNew),
            ("link", "data/link", SeedPlacement::Replace),
        ]
        .into_iter()
        .map(|(source, target, placement)| {
            let entry = seed_entry(
                host_child(&sources, source),
                target,
                SeedAccess::FromSource,
                placement,
            );
            let filesystem = &filesystem;
            async move { (target, placement, seed_one(filesystem, entry).await) }
        }),
    )
    .await;

    errors.into_iter().for_each(|(target, placement, result)| {
        assert_eq!(
            result.unwrap_err().io_kind(),
            Some(std::io::ErrorKind::PermissionDenied),
            "seed to {target} with {placement:?} must be refused"
        );
    });
    let tree = host_child(&sources, "tree");
    std::fs::create_dir_all(tree.as_path().join("data/nested")).unwrap();
    std::fs::write(tree.as_path().join("data/nested/file"), b"file").unwrap();
    let fail = seed_one(
        &filesystem,
        seed_entry(
            tree.clone(),
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert!(
        std::fs::symlink_metadata(root.join("data"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "a failed seed must leave the symlink in place"
    );
    seed_one(
        &filesystem,
        seed_entry(tree, "", SeedAccess::FromSource, SeedPlacement::Replace),
    )
    .await
    .unwrap();

    assert_eq!(fail.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert!(root.join("data/nested/file").is_file());
    assert!(
        !std::fs::symlink_metadata(root.join("data"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "replace must put a directory in place of the symlink"
    );
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    assert!(!root.parent().unwrap().join("escaped").exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_directory_entries_merge_and_follow_the_placement() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let sources = host_directory_at(parent.path()).await;
    let tree = sources.path().as_path().join("tree");
    std::fs::create_dir_all(tree.join("data")).unwrap();
    std::fs::write(tree.join("data/added"), b"added").unwrap();
    std::fs::write(tree.join("data/same"), b"new").unwrap();
    std::fs::create_dir_all(tree.join("fresh")).unwrap();
    std::fs::write(tree.join("fresh/deep"), b"deep").unwrap();
    std::fs::set_permissions(tree.join("fresh"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::set_permissions(tree.join("data"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::os::unix::fs::symlink("new", tree.join("link")).unwrap();
    std::fs::write(tree.join("was-directory"), b"file").unwrap();
    std::fs::create_dir(tree.join("was-file")).unwrap();
    std::fs::write(tree.join("was-file/inner"), b"inner").unwrap();
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    ["create-new", "replace"].into_iter().for_each(|placement| {
        let target = root.join(placement);
        std::fs::create_dir_all(target.join("data")).unwrap();
        std::fs::set_permissions(target.join("data"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::write(target.join("data/same"), b"old").unwrap();
        std::fs::write(target.join("data/only-sandbox"), b"sandbox").unwrap();
        std::os::unix::fs::symlink("old", target.join("link")).unwrap();
        std::fs::create_dir(target.join("was-directory")).unwrap();
        std::fs::write(target.join("was-directory/child"), b"child").unwrap();
        std::fs::write(target.join("was-file"), b"file").unwrap();
    });
    std::fs::write(root.join("top-file"), b"top").unwrap();
    let seed = |target: &'static str, placement| {
        let entry = seed_entry(
            host_child(&sources, "tree"),
            target,
            SeedAccess::FromSource,
            placement,
        );
        let filesystem = &filesystem;
        async move { seed_one(filesystem, entry).await }
    };

    let failed = seed("create-new", SeedPlacement::CreateNew)
        .await
        .unwrap_err();

    seed("replace", SeedPlacement::Replace).await.unwrap();
    seed("absent", SeedPlacement::CreateNew).await.unwrap();
    let failed_top = seed("top-file", SeedPlacement::CreateNew)
        .await
        .unwrap_err();
    assert_eq!(std::fs::read(root.join("top-file")).unwrap(), b"top");
    seed("top-file", SeedPlacement::Replace).await.unwrap();

    assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(
        std::fs::read(root.join("create-new/data/same")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(root.join("create-new/data/added")).unwrap(),
        b"added",
        "a path before the failed path must stay"
    );
    assert!(
        !root.join("create-new/fresh").exists(),
        "a path after the failed path must not be seeded"
    );
    assert_eq!(
        failed_top.io_kind(),
        Some(std::io::ErrorKind::AlreadyExists)
    );

    let read = |path: &str| std::fs::read(root.join(path)).unwrap();

    assert_eq!(read("replace/data/same"), b"new");
    assert_eq!(read("replace/data/added"), b"added");
    assert_eq!(read("replace/data/only-sandbox"), b"sandbox");
    assert_eq!(read("replace/fresh/deep"), b"deep");
    assert_eq!(read("replace/was-file/inner"), b"inner");
    assert_eq!(read("replace/was-directory"), b"file");
    assert_eq!(
        std::fs::read_link(root.join("replace/link")).unwrap(),
        PathBuf::from("new")
    );

    assert_eq!(
        tree_copy::tree_listing(&root.join("absent")),
        tree_copy::tree_listing(&tree)
    );
    assert_eq!(read("top-file/data/same"), b"new");
    assert_eq!(
        mode(&root.join("replace/data")),
        0o700,
        "a merged directory must keep its permissions"
    );
    assert_eq!(
        mode(&root.join("replace/fresh")),
        0o750,
        "a made directory must get the permissions of its source"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem = create::<SandboxFilesystem>(provisioning, name(), None)
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();

    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert!(!root.exists());

    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let rejected_root = parent.path().join(name().relative_path());
    let error = create::<SandboxFilesystem>(
        provisioning,
        name(),
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
            "failed to install limits without quota authority filesystem {}",
            rejected_root.display()
        )
    );
    assert!(!rejected_root.exists());
}

#[test]
async fn production_adapter_executes_the_filesystem_method_families() {
    let parent = tempfile::tempdir().unwrap();
    let seed_sources = host_directory_at(parent.path()).await;
    std::fs::write(seed_sources.path().as_path().join("file"), b"seeded").unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
    .into_node()
    {
        SandboxNode::Directory(directory) => directory,
        SandboxNode::File(_) => panic!("root must open as a directory"),
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
    .into_node()
    {
        SandboxNode::File(file) => file,
        SandboxNode::Directory(_) => panic!("file must open as a file"),
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
    let attributes = <SandboxFilesystem as SandboxFilesystemAdapter>::get_node_attributes(
        &filesystem,
        SandboxNode::File(file.clone()),
    )
    .await
    .unwrap();
    assert_eq!(attributes.kind, SandboxObjectKind::File);
    assert_eq!(attributes.size, 5);
    <SandboxFilesystem as SandboxFilesystemAdapter>::set_node_times(
        &filesystem,
        SandboxNode::File(file.clone()),
        SandboxTimeChanges {
            accessed: SandboxTimeChange::Keep,
            modified: SandboxTimeChange::Now,
        },
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::flush(
        &filesystem,
        &SandboxNode::File(file.clone()),
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
    seed_one(
        &filesystem,
        seed_entry(
            host_child(&seed_sources, "file"),
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

    <SandboxFilesystem as SandboxFilesystemAdapter>::close(&filesystem, SandboxNode::File(file))
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
        SandboxNode::Directory(directory),
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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
    let source_parent = open_native_directory(&filesystem, "source-parent").await;
    let destination_parent = open_native_directory(&filesystem, "destination-parent").await;

    let hard_source = filesystem
        .resolve_namespace_target(SandboxPath::at(source_parent.clone(), "hard-source"))
        .await
        .unwrap();
    let hard_destination = filesystem
        .resolve_namespace_target(SandboxPath::at(
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
        .resolve_namespace_target(SandboxPath::at(source_parent.clone(), "rename-source"))
        .await
        .unwrap();
    let rename_destination = filesystem
        .resolve_namespace_target(SandboxPath::at(
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
async fn capability_relative_path_operations_never_fall_back_to_root() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    std::fs::create_dir(filesystem.root().join("base")).unwrap();
    std::fs::write(filesystem.root().join("same-name"), b"root").unwrap();
    std::fs::write(filesystem.root().join("base/same-name"), b"capability").unwrap();
    let base = open_native_directory(&filesystem, "base").await;

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
        .into_node()
    {
        SandboxNode::File(file) => file,
        SandboxNode::Directory(_) => panic!("same-name must open as a file"),
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
async fn external_seed_keeps_the_pinned_destination_after_directory_rename() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    assert_pinned_seed_after_directory_rename(parent.path(), provisioning).await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn apfs_seed_keeps_the_pinned_destination_after_directory_rename() {
    let parent = tempfile::tempdir().unwrap();
    let storage = FilesystemStorageMode::Apfs {
        root: parent.path().join("agents").into_boxed_path(),
    };
    let provisioning =
        SandboxFilesystemProvisioning::new(&storage, RetryConfig::default()).unwrap();
    assert_pinned_seed_after_directory_rename(parent.path(), provisioning).await;
}

async fn assert_pinned_seed_after_directory_rename(
    parent: &Path,
    provisioning: SandboxFilesystemProvisioning,
) {
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::create_dir(root.join("pinned")).unwrap();
    let pinned = open_native_directory(&filesystem, "pinned").await;
    std::fs::rename(root.join("pinned"), root.join("renamed")).unwrap();
    std::fs::create_dir(root.join("pinned")).unwrap();

    let host_source = host_directory_at(parent).await;
    std::fs::write(host_source.path().as_path().join("source"), b"seeded").unwrap();
    let source = host_child(&host_source, "source");
    seed_one(
        &filesystem,
        SeedEntry {
            source: source.clone(),
            target: SandboxPath::at(pinned.clone(), "destination"),
            access: SeedAccess::ReadOnly,
            placement: SeedPlacement::CreateNew,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(root.join("renamed/destination")).unwrap(),
        b"seeded"
    );
    assert!(
        std::fs::metadata(root.join("renamed/destination"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert!(!root.join("pinned/destination").exists());
    let existing = seed_one(
        &filesystem,
        SeedEntry {
            source,
            target: SandboxPath::at(pinned, "destination"),
            access: SeedAccess::ReadOnly,
            placement: SeedPlacement::CreateNew,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn empty_root_path_attributes_address_root() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    let root_directory = filesystem.root_directory_state();
    let storage_profile = storage_profile(&filesystem.volume);
    let (first, second) = execute_native(storage_profile, NativeOperation::Open, move || {
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
async fn root_relative_seed_reuses_root_without_descriptor_duplication() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    let host_source = host_directory_at(parent.path()).await;
    std::fs::write(
        host_source.path().as_path().join("source"),
        b"seeded contents",
    )
    .unwrap();
    let source = || host_child(&host_source, "source");
    let root_directory = directory_for(
        &filesystem.root_directory_state(),
        &SandboxPath::at_root("."),
    )
    .unwrap();
    let probe = CapabilityCopyParentProbe::install(root_directory);

    seed_one(
        &filesystem,
        seed_entry(
            source(),
            "destination",
            SeedAccess::ReadOnly,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();

    assert_eq!(
        probe.reused_base(),
        Some(true),
        "root-relative seed duplicated the root capability"
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("destination")).unwrap(),
        b"seeded contents"
    );
    assert!(
        std::fs::metadata(filesystem.root().join("destination"))
            .unwrap()
            .permissions()
            .readonly()
    );

    let existing = seed_one(
        &filesystem,
        seed_entry(
            source(),
            "destination",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(
        std::fs::read(filesystem.root().join("destination")).unwrap(),
        b"seeded contents"
    );
    let escaped_path = filesystem.root().parent().unwrap().join("escaped-seed");
    let escaped = seed_one(
        &filesystem,
        seed_entry(
            source(),
            "../escaped-seed",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        escaped.io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(!escaped_path.exists());
    assert!(std::fs::read_dir(filesystem.root()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".golem-copy-")
    }));

    drop(probe);
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn root_close_linearizes_access_and_preserves_admitted_capability_lifetime() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    std::fs::write(filesystem.root().join("existing"), b"contents").unwrap();
    let descriptor = open_native_directory(&filesystem, ".").await;
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
async fn shared_root_capability_keeps_root_relative_paths_confined() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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

#[test]
async fn deletion_revokes_only_its_generation_root_and_retains_exclusive_ownership() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning.clone(),
        name(),
        None,
    )
    .await
    .unwrap();
    let released_root = filesystem.root_directory_state();
    let lease_probe = FilesystemLeaseProbe::install(filesystem.root());
    let second = tokio::spawn(async move {
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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

#[cfg(target_os = "linux")]
#[test]
fn deferred_followed_permissions_preserve_raw_terminal_errno() {
    let mut target =
        scripted::default_scripted_namespace_resolution(SandboxPath::at_root("link")).unwrap();
    let source = std::io::Error::from_raw_os_error(libc::EIO);
    target.followed_read_only_file = NativeReadOnlyResolution::Failed {
        kind: source.kind(),
        raw_os_error: source.raw_os_error(),
        message: source.to_string(),
    };

    assert!(target.is_read_only_file(SandboxFollow::No).is_ok());
    let Err(error) = target.is_read_only_file(SandboxFollow::Yes) else {
        panic!("followed permissions unexpectedly discarded their deferred failure")
    };
    assert_eq!(
        error.io_error().and_then(std::io::Error::raw_os_error),
        Some(libc::EIO)
    );
    assert!(error.is_terminal_failure());

    let source = include_str!("namespace.rs");
    assert!(source.contains(concat!("raw_os_error: error.", "raw_os_error()")));
    assert!(source.contains(concat!("std::io::Error::from_", "raw_os_error")));
}

#[test]
async fn positioned_write_preserves_the_shared_file_cursor() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
    .into_node()
    {
        SandboxNode::File(file) => file,
        SandboxNode::Directory(_) => panic!("positioned-write must open as a file"),
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

#[cfg(unix)]
#[test]
async fn production_namespace_resolution_pins_semantic_parents_and_does_not_follow_final_symlinks()
{
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    std::fs::create_dir_all(filesystem.root().join("real/parent")).unwrap();
    std::os::unix::fs::symlink("real/parent", filesystem.root().join("alias")).unwrap();

    let direct_absent = filesystem
        .resolve_namespace_target(SandboxPath::at_root("real/parent/child"))
        .await
        .unwrap();
    let alias_absent = filesystem
        .resolve_namespace_target(SandboxPath::at_root("alias/child"))
        .await
        .unwrap();
    assert!(direct_absent.coordination_key() == alias_absent.coordination_key());
    assert!(direct_absent.final_directory_key().is_none());

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
    let parent_directory = match opened_parent.into_node() {
        SandboxNode::Directory(directory) => directory,
        SandboxNode::File(_) => panic!("parent must open as a directory"),
    };
    let descriptor_relative = filesystem
        .resolve_namespace_target(SandboxPath::at(parent_directory.clone(), "child"))
        .await
        .unwrap();
    let alias_present = filesystem
        .resolve_namespace_target(SandboxPath::at_root("alias/child"))
        .await
        .unwrap();
    assert!(
        descriptor_relative
            .coordination_key()
            .may_conflict_with(&alias_present.coordination_key())
    );
    assert!(descriptor_relative.final_directory_key() == alias_present.final_directory_key());
    assert!(descriptor_relative.final_directory_key().is_some());

    std::fs::write(filesystem.root().join("real/parent/plain-file"), b"").unwrap();
    std::fs::set_permissions(
        filesystem.root().join("real/parent/plain-file"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    std::fs::write(filesystem.root().join("real/parent/writable-file"), b"").unwrap();
    let final_file = filesystem
        .resolve_namespace_target(SandboxPath::at_root("alias/plain-file"))
        .await
        .unwrap();
    assert!(final_file.final_directory_key().is_none());
    let descriptor_file = filesystem
        .resolve_namespace_target(SandboxPath::at(parent_directory, "plain-file"))
        .await
        .unwrap();
    std::os::unix::fs::symlink(
        "real/parent/plain-file",
        filesystem.root().join("plain-file-alias"),
    )
    .unwrap();
    let final_file_alias = filesystem
        .resolve_namespace_target(SandboxPath::at_root("plain-file-alias"))
        .await
        .unwrap();
    let writable_file = filesystem
        .resolve_namespace_target(SandboxPath::at_root("alias/writable-file"))
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
        .resolve_namespace_target(SandboxPath::at_root("alias/final-link"))
        .await
        .unwrap();
    assert!(final_symlink.final_directory_key().is_none());

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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
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
        .resolve_namespace_target(SandboxPath::at_root("locked-directory"))
        .await
        .unwrap();
    assert!(resolved.final_directory_key().is_some());
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
                .resolve_namespace_target(SandboxPath::at_root("renamed-directory"))
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
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    let locked = filesystem.root().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();

    let resolved = filesystem
        .resolve_namespace_target(SandboxPath::at_root("locked/child"))
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
    let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
        .create_fresh(name())
        .await
        .unwrap();
    std::os::unix::fs::symlink("loop", filesystem.root().join("loop")).unwrap();
    std::os::unix::fs::symlink("missing", filesystem.root().join("dangling")).unwrap();

    let looped = filesystem
        .resolve_namespace_target(SandboxPath::at_root("loop"))
        .await
        .unwrap();
    let dangling = filesystem
        .resolve_namespace_target(SandboxPath::at_root("dangling"))
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
    let source = include_str!("namespace.rs");
    let resolver = source
        .split_once("fn resolve_host_namespace_target(")
        .unwrap()
        .1
        .split_once("fn split_namespace_target(")
        .unwrap()
        .0;

    assert!(resolver.contains("parent.symlink_metadata(&name)"));
    assert!(!resolver.contains("parent.open_dir_nofollow(&name)"));
}

#[cfg(any(unix, windows))]
#[test]
async fn concurrent_first_append_coordination_converges_across_hard_links() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    std::fs::write(filesystem.root().join("file"), b"").unwrap();
    std::fs::hard_link(
        filesystem.root().join("file"),
        filesystem.root().join("hard-link"),
    )
    .unwrap();
    let files = vec![
        open_native_file(&filesystem, "file").await,
        open_native_file(&filesystem, "file").await,
        open_native_file(&filesystem, "hard-link").await,
        open_native_file(&filesystem, "hard-link").await,
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
async fn append_coordination_tracks_identity_through_rename_unlink_and_path_reuse() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    let original_path = filesystem.root().join("object");
    let reused_path = filesystem.root().join("reused");
    std::fs::write(&original_path, b"original").unwrap();
    let original = open_native_file(&filesystem, "object").await;
    let original_guard = original.coordinate_append().await;

    std::fs::rename(&original_path, &reused_path).unwrap();
    let renamed = open_native_file(&filesystem, "reused").await;
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
    let replacement = open_native_file(&filesystem, "reused").await;
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

#[cfg(any(unix, windows))]
#[test]
async fn distinct_append_identities_are_independent() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    std::fs::write(filesystem.root().join("first"), b"").unwrap();
    std::fs::write(filesystem.root().join("second"), b"").unwrap();
    let first = open_native_file(&filesystem, "first").await;
    let second = open_native_file(&filesystem, "second").await;

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
    let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
    let filesystem =
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(provisioning, name(), None)
            .await
            .unwrap();
    std::fs::write(filesystem.root().join("first"), b"").unwrap();
    std::fs::write(filesystem.root().join("second"), b"").unwrap();
    let first = open_native_file(&filesystem, "first").await;
    let second = open_native_file(&filesystem, "second").await;

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
