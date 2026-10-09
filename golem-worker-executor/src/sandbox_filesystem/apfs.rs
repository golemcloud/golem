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
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;

pub(super) fn bind(
    root: &Path,
    cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, directories::DirectoryProvisioning), FilesystemStorageError> {
    std::fs::create_dir_all(root)
        .map_err(|error| FilesystemStorageError::io("create APFS development root", root, error))?;
    let directory = File::open(root)
        .map_err(|error| FilesystemStorageError::io("open APFS development root", root, error))?;
    probe_clone(root, probe_contents)?;
    let name_mode = name_mode(&directory).map_err(|error| {
        FilesystemStorageError::io("read APFS volume case sensitivity", root, error)
    })?;
    tracing::info!(root = %root.display(), "APFS storage is for local development and tests only");
    Ok((
        FilesystemVolume::copy_on_write(Arc::new(directory)),
        directories::DirectoryProvisioning::new(Some(Arc::from(root)), cleanup_retry, name_mode),
    ))
}

fn probe_clone(
    root: &Path,
    contents: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), FilesystemStorageError> {
    let probe = tempfile::Builder::new()
        .prefix(".golem-apfs-clone-probe-")
        .tempdir_in(root)
        .map_err(|error| FilesystemStorageError::io("create APFS clone probe", root, error))?;
    let result = contents(probe.path());
    probe.close().map_err(|error| {
        FilesystemStorageError::cleanup_io("remove APFS clone probe", root, error)
    })?;
    result.map_err(|error| {
        FilesystemStorageError::io("probe APFS clone for local development", root, error)
    })
}

fn probe_contents(path: &Path) -> std::io::Result<()> {
    let mut source = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path.join("source"))?;
    let contents = [0x5a; 4096];
    source.write_all(&contents)?;
    let parent = File::open(path)?;
    clone_file(&source, &parent, Path::new("clone"))?;
    if std::fs::read(path.join("clone"))? != contents {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "APFS clone probe changed the file contents",
        ));
    }
    Ok(())
}

fn name_mode(root: &File) -> std::io::Result<NativeNameModeSource> {
    // SAFETY: the descriptor is open, and the selector is valid on macOS.
    let case_sensitive = unsafe { libc::fpathconf(root.as_raw_fd(), libc::_PC_CASE_SENSITIVE) };
    apfs_name_source(
        case_sensitive,
        FilesystemIdentity {
            device: root.metadata()?.dev(),
        },
    )
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

/// Syncs directories without following symlinks or flushing the source of a clone.
pub(super) fn sync_directories(root: &Path) -> std::io::Result<()> {
    fn sync(directory: &cap_std::fs::Dir) -> std::io::Result<()> {
        directory.entries()?.try_for_each(|entry| {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                sync(&directory.open_dir_nofollow(entry.file_name())?)?;
            }
            Ok::<_, std::io::Error>(())
        })?;
        rustix::fs::fsync(directory).map_err(std::io::Error::from)
    }
    sync(&cap_std::fs::Dir::open_ambient_dir(
        root,
        cap_std::ambient_authority(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};
    use test_r::test;

    #[test]
    fn apfs_failed_clone_probe_reports_the_root_and_removes_its_files() {
        let root = tempfile::tempdir().unwrap();
        let error = probe_clone(root.path(), |probe| {
            std::fs::write(probe.join("source"), b"probe")?;
            Err(std::io::Error::from_raw_os_error(libc::ENOTSUP))
        })
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
    fn apfs_clone_across_system_and_data_volumes_fails() {
        let source = File::open("/System/Library/CoreServices/SystemVersion.plist").unwrap();
        let target = tempfile::tempdir().unwrap();
        let parent = File::open(target.path()).unwrap();
        let error = clone_file(&source, &parent, Path::new("clone")).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
        assert!(!target.path().join("clone").exists());
    }

    #[test]
    #[test_r::timeout("120s")]
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
        let path = filesystem.root().join("file");
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
