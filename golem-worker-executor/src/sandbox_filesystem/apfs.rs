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
use rustix::fs::{CloneFlags, StatVfsMountFlags, fclonefileat, fstatvfs};
use std::io::Write;

pub(super) fn bind(
    root: &Path,
    cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, directories::DirectoryProvisioning), FilesystemStorageError> {
    bind_with_probe(
        root,
        cleanup_retry,
        |source, parent, name| clone_file(source, parent, name),
        tempfile::TempDir::close,
    )
}

fn bind_with_probe(
    root: &Path,
    cleanup_retry: RetryConfig,
    clone: impl FnOnce(&File, &File, &Path) -> std::io::Result<()>,
    close: impl FnOnce(tempfile::TempDir) -> std::io::Result<()>,
) -> Result<(FilesystemVolume, directories::DirectoryProvisioning), FilesystemStorageError> {
    std::fs::create_dir_all(root)
        .map_err(|error| FilesystemStorageError::io("create APFS development root", root, error))?;
    let directory = File::open(root)
        .map_err(|error| FilesystemStorageError::io("open APFS development root", root, error))?;
    probe_clone(root, clone, close)?;
    tracing::info!(root = %root.display(), "APFS storage is for local development and tests only");
    Ok((
        FilesystemVolume::copy_on_write(Arc::new(directory)),
        directories::DirectoryProvisioning::new(
            Some(Arc::from(root)),
            cleanup_retry,
            NativeNameModeSource::NativeDetection,
        ),
    ))
}

fn probe_clone(
    root: &Path,
    clone: impl FnOnce(&File, &File, &Path) -> std::io::Result<()>,
    close: impl FnOnce(tempfile::TempDir) -> std::io::Result<()>,
) -> Result<(), FilesystemStorageError> {
    let probe = tempfile::Builder::new()
        .prefix(".golem-apfs-clone-probe-")
        .tempdir_in(root)
        .map_err(|error| FilesystemStorageError::io("create APFS clone probe", root, error))?;
    let result = probe_contents(probe.path(), clone);
    let cleanup = close(probe);
    result.map_err(|error| {
        let operation = "probe APFS clone for local development";
        if cleanup.is_err() {
            FilesystemStorageError::cleanup_io(operation, root, error)
        } else {
            FilesystemStorageError::io(operation, root, error)
        }
    })?;
    cleanup
        .map_err(|error| FilesystemStorageError::cleanup_io("remove APFS clone probe", root, error))
}

fn probe_contents(
    path: &Path,
    clone: impl FnOnce(&File, &File, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut source = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path.join("source"))?;
    let contents = [0x5a; 4096];
    source.write_all(&contents)?;
    let parent = File::open(path)?;
    clone(&source, &parent, Path::new("clone"))?;
    if std::fs::read(path.join("clone"))? != contents {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "APFS clone probe changed the file contents",
        ));
    }
    Ok(())
}

pub(super) fn clone_file(
    source: &File,
    parent: impl std::os::fd::AsFd,
    name: &Path,
) -> std::io::Result<()> {
    fclonefileat(
        source,
        parent,
        name,
        CloneFlags::NOFOLLOW | CloneFlags::NOOWNERCOPY,
    )
    .map_err(std::io::Error::from)
}

pub(super) fn observe_space(root: &File) -> std::io::Result<FilesystemSpace> {
    let capacity = fstatvfs(root).map_err(std::io::Error::from)?;
    if capacity.f_flag.contains(StatVfsMountFlags::RDONLY) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ReadOnlyFilesystem,
            "APFS development volume is mounted read-only",
        ));
    }
    space_from_values(
        capacity.f_blocks,
        capacity.f_bavail,
        capacity.f_frsize,
        capacity.f_files,
        capacity.f_ffree,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};
    use test_r::test;

    #[test]
    #[test_r::tag(native_apfs)]
    #[ignore = "requires the native APFS runner's non-cloning filesystem fixture"]
    fn apfs_constructor_refuses_a_root_on_a_non_cloning_filesystem() {
        let parent = std::env::var_os("GOLEM_APFS_NON_CLONE_ROOT")
            .expect("the native APFS runner supplies the non-cloning filesystem root");
        let root = tempfile::tempdir_in(parent).unwrap();
        let error = SandboxFilesystemProvisioning::new(
            &FilesystemStorageMode::Apfs {
                root: Box::from(root.path()),
            },
            RetryConfig::default(),
        )
        .err()
        .expect("a non-cloning filesystem cannot provide APFS storage");
        assert!(
            error
                .to_string()
                .contains("probe APFS clone for local development")
        );
        assert!(error.to_string().contains(root.path().to_str().unwrap()));
        assert_eq!(
            error.inner.source.as_ref().unwrap().raw_os_error(),
            Some(libc::ENOTSUP),
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    #[test_r::tag(native_apfs)]
    fn apfs_temporary_clone_open_failure_removes_the_native_clone() {
        use std::os::unix::fs::PermissionsExt;

        let source_root = tempfile::tempdir().unwrap();
        let mut source = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(source_root.path().join("source"))
            .unwrap();
        source.write_all(&[b'Z'; 4096]).unwrap();
        source
            .set_permissions(std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let destination = tempfile::tempdir().unwrap();
        let directory = cap_std::fs::Dir::from_std_file(File::open(destination.path()).unwrap());
        let name = temporary_copy_name();

        clone_file(&source, &directory, &name).unwrap();
        assert_eq!(directory.read(&name).unwrap().as_slice(), &[b'Z'; 4096]);
        std::fs::set_permissions(
            destination.path().join(&name),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let control_error = directory.open(&name).unwrap_err();
        assert_eq!(control_error.kind(), std::io::ErrorKind::PermissionDenied);

        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(cap_fs_ext::FollowSymlinks::No);
        let error = open_cloned_file(
            || directory.open_with(&name, &options),
            || directory.remove_file(&name),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(error.raw_os_error(), control_error.raw_os_error());
        assert_eq!(directory.entries().unwrap().count(), 0);
        assert_eq!(
            source.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(source.metadata().unwrap().len(), 4096);
        assert_eq!(
            std::fs::read(source_root.path().join("source"))
                .unwrap()
                .as_slice(),
            &[b'Z'; 4096],
        );
    }

    #[test]
    #[test_r::tag(native_apfs)]
    fn apfs_failed_clone_probe_reports_the_root_and_removes_its_files() {
        let root = tempfile::tempdir().unwrap();
        let error = probe_clone(
            root.path(),
            |_, _, _| Err(std::io::Error::from_raw_os_error(libc::ENOTSUP)),
            tempfile::TempDir::close,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("probe APFS clone for local development")
        );
        assert!(error.to_string().contains(root.path().to_str().unwrap()));
        assert_eq!(
            error.io_kind(),
            Some(std::io::Error::from_raw_os_error(libc::ENOTSUP).kind())
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    #[test_r::tag(native_apfs)]
    fn apfs_bind_enotsup_clone_failure_precedes_cleanup_failure() {
        assert_bind_clone_failure_precedes_cleanup_failure(libc::ENOTSUP);
    }

    #[test]
    #[test_r::tag(native_apfs)]
    fn apfs_bind_exdev_clone_failure_precedes_cleanup_failure() {
        assert_bind_clone_failure_precedes_cleanup_failure(libc::EXDEV);
    }

    fn assert_bind_clone_failure_precedes_cleanup_failure(errno: i32) {
        use std::cell::Cell;
        use std::os::unix::fs::FileExt as _;

        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("development/root").into_boxed_path();
        assert!(!root.exists());
        let clone_attempts = Cell::new(0);
        let close_attempts = Cell::new(0);
        let result = bind_with_probe(
            &root,
            RetryConfig {
                max_attempts: 3,
                ..RetryConfig::default()
            },
            |source, directory, name| {
                clone_attempts.set(clone_attempts.get() + 1);
                assert!(root.is_dir());
                assert!(directory.metadata().unwrap().is_dir());
                assert_eq!(name, Path::new("clone"));
                let mut contents = [0; 4096];
                source.read_exact_at(&mut contents, 0).unwrap();
                assert_eq!(contents, [0x5a; 4096]);
                Err(std::io::Error::from_raw_os_error(errno))
            },
            |probe| {
                close_attempts.set(close_attempts.get() + 1);
                assert_eq!(probe.path().parent(), Some(root.as_ref()));
                assert!(probe.path().join("source").is_file());
                assert!(!probe.path().join("clone").exists());
                probe.close()?;
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            },
        );
        let Err(error) = result else {
            panic!("bind must not return a volume after the clone probe fails");
        };
        assert_eq!(clone_attempts.get(), 1);
        assert_eq!(close_attempts.get(), 1);
        assert!(root.is_dir());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(error.io_error().unwrap().raw_os_error(), Some(errno));
        let original = std::io::Error::from_raw_os_error(errno);
        assert_eq!(
            error.to_string(),
            format!(
                "failed to probe APFS clone for local development filesystem {}: {original}",
                root.display()
            )
        );
        assert_eq!(error.io_kind(), Some(original.kind()));
        assert!(error.cleanup_failed());
        assert!(!error.is_terminal_failure());
        assert!(!error.is_storage_exhaustion());
    }

    #[test]
    #[test_r::tag(native_apfs)]
    fn apfs_clone_across_system_and_data_volumes_fails() {
        let source = File::open("/System/Library/CoreServices/SystemVersion.plist").unwrap();
        let target = tempfile::tempdir().unwrap();
        let parent = File::open(target.path()).unwrap();
        let error = clone_file(&source, &parent, Path::new("clone")).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
        assert!(!target.path().join("clone").exists());
    }

    #[test]
    #[test_r::timeout("60s")]
    #[test_r::tag(native_apfs)]
    async fn apfs_seed_does_not_scan_an_unrelated_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
            &FilesystemStorageMode::Apfs {
                root: root.path().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
        let filesystem = provisioning
            .create_fresh(
                SandboxFilesystemName::new(
                    "environment".into(),
                    "component".into(),
                    "apfs-seed-scope".into(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let source_directory = HostDirectory::create_in(
            directories.scratch.path(),
            std::ffi::OsStr::new("seed-source"),
        )
        .await
        .unwrap();
        let source = source_directory
            .path()
            .child(std::ffi::OsStr::new("source"))
            .unwrap();
        std::fs::write(source.as_path(), b"seeded").unwrap();
        let unrelated = filesystem.root().join("unrelated").into_boxed_path();
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o0)).unwrap();
        let result = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
            &filesystem,
            Box::new([SeedEntry {
                source,
                target: SandboxPath::at_root("new/ancestor/file"),
                access: SeedAccess::ReadWrite,
                placement: SeedPlacement::CreateNew,
            }]),
        )
        .await;
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            result.is_ok(),
            "a seed must not scan unrelated directories: {result:?}"
        );
        assert_eq!(
            std::fs::read(filesystem.root().join("new/ancestor/file")).unwrap(),
            b"seeded"
        );
        source_directory.discard().await.unwrap();
        SandboxFilesystem::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    #[test_r::timeout("120s")]
    #[test_r::tag(native_apfs)]
    async fn apfs_copy_contents_and_seed_share_extents_and_preserve_unflushed_writes() {
        const FILE_BYTES: usize = 64 * 1024 * 1024;
        let root = tempfile::tempdir().unwrap();
        let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
            &FilesystemStorageMode::Apfs {
                root: root.path().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
        let filesystem = provisioning
            .create_fresh(
                SandboxFilesystemName::new(
                    "environment".into(),
                    "component".into(),
                    "apfs-copy".into(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let path = filesystem.root().join("file").into_boxed_path();
        let mut source = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        source
            .write_all(&vec![0x5a; FILE_BYTES].into_boxed_slice())
            .unwrap();
        source.sync_all().unwrap();
        source.seek(SeekFrom::Start(0)).unwrap();
        source.write_all(b"unflushed").unwrap();
        source.seek(SeekFrom::End(0)).unwrap();
        source.write_all(b"tail").unwrap();
        std::fs::hard_link(&path, filesystem.root().join("other-name")).unwrap();
        std::fs::write(filesystem.root().join("excluded"), b"excluded").unwrap();
        let capture =
            HostDirectory::create_in(directories.scratch.path(), std::ffi::OsStr::new("copy"))
                .await
                .unwrap();
        let available = |space| match space {
            FilesystemSpace::Observed {
                available_bytes, ..
            } => available_bytes,
            _ => panic!("APFS must report free bytes"),
        };
        let before = available(observe_space_blocking(provisioning.volume()).unwrap());
        let groups = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
            &filesystem,
            SandboxPath::at_root(""),
            Arc::new(TreeExclusions::new([PathBuf::from("excluded")])),
            capture.path(),
        )
        .await
        .unwrap();
        let after = available(observe_space_blocking(provisioning.volume()).unwrap());
        assert!(
            before.saturating_sub(after) < FILE_BYTES as u64 / 4,
            "a clone must not allocate the file bytes"
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].first.as_ref(), Path::new("file"));
        assert_eq!(
            groups[0].others.as_ref(),
            [Box::from(Path::new("other-name"))]
        );
        assert!(!capture.path().as_path().join("excluded").exists());
        assert!(!capture.path().as_path().join("other-name").exists());
        let copied = std::fs::read(capture.path().as_path().join("file"))
            .unwrap()
            .into_boxed_slice();
        assert_eq!(copied.len(), FILE_BYTES + 4);
        assert_eq!(&copied[..9], b"unflushed");
        assert_eq!(&copied[FILE_BYTES..], b"tail");
        let before_seed = available(observe_space_blocking(provisioning.volume()).unwrap());
        <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
            &filesystem,
            Box::new([SeedEntry {
                source: capture.path().clone(),
                target: SandboxPath::at_root("seeded"),
                access: SeedAccess::FromSource,
                placement: SeedPlacement::CreateNew,
            }]),
        )
        .await
        .unwrap();
        let after_seed = available(observe_space_blocking(provisioning.volume()).unwrap());
        assert!(
            before_seed.saturating_sub(after_seed) < FILE_BYTES as u64 / 4,
            "a seed clone must not allocate the file bytes"
        );
        source.seek(SeekFrom::Start(0)).unwrap();
        source.write_all(b"changed").unwrap();
        assert_eq!(
            std::fs::read(filesystem.root().join("seeded/file"))
                .unwrap()
                .as_slice(),
            copied.as_ref()
        );
        assert_eq!(
            std::fs::read(capture.path().as_path().join("file"))
                .unwrap()
                .as_slice(),
            copied.as_ref()
        );
        capture.discard().await.unwrap();
        SandboxFilesystem::delete_and_verify(filesystem)
            .await
            .unwrap();
    }
}
