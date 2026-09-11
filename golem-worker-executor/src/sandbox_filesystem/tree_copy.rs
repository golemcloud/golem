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
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};
use std::time::SystemTime;

/// One object of a tree, listed for a copy.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TreeEntry {
    pub(super) relative: PathBuf,
    pub(super) kind: TreeEntryKind,
    pub(super) permissions: cap_std::fs::Permissions,
    pub(super) modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TreeEntryKind {
    Directory,
    File,
    Symlink(PathBuf),
}

/// Keeps the exclusions that can name an entry of a tree.
///
/// A root-relative path keeps only its normal components. A leading root or current-directory
/// component is dropped. A path with a parent or prefix component names no entry and is dropped.
pub(super) fn normalize_exclusions(excluded: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    excluded
        .iter()
        .filter_map(|path| {
            let mut normalized = PathBuf::new();
            for component in path.components() {
                match component {
                    Component::Normal(component) => normalized.push(component),
                    Component::RootDir | Component::CurDir => {}
                    Component::ParentDir | Component::Prefix(_) => return None,
                }
            }
            (!normalized.as_os_str().is_empty()).then_some(normalized)
        })
        .collect()
}

/// Lists a tree, parents before children, without the excluded root-relative paths.
///
/// An excluded directory is absent together with its contents. Symlinks are listed, not followed.
/// Entries of one directory are in name order.
pub(super) fn list_tree(
    root: &cap_std::fs::Dir,
    excluded: &HashSet<PathBuf>,
) -> std::io::Result<Vec<TreeEntry>> {
    let excluded = normalize_exclusions(excluded);
    let mut entries = Vec::new();
    list_directory(root, &PathBuf::new(), &excluded, &mut entries)?;
    Ok(entries)
}

fn list_directory(
    directory: &cap_std::fs::Dir,
    relative: &Path,
    excluded: &HashSet<PathBuf>,
    entries: &mut Vec<TreeEntry>,
) -> std::io::Result<()> {
    let mut names = directory
        .entries()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    for name in names {
        let entry_relative = relative.join(&name);
        if excluded.contains(&entry_relative) {
            continue;
        }
        let metadata = directory.symlink_metadata(&name)?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            TreeEntryKind::Symlink(read_link_contents(directory, Path::new(&name))?)
        } else if file_type.is_dir() {
            TreeEntryKind::Directory
        } else if file_type.is_file() {
            TreeEntryKind::File
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "tree entry {} is not a regular file, a directory or a symlink",
                    entry_relative.display()
                ),
            ));
        };
        let is_directory = kind == TreeEntryKind::Directory;
        entries.push(TreeEntry {
            relative: entry_relative.clone(),
            kind,
            permissions: metadata.permissions(),
            modified: metadata.modified().ok().map(|time| time.into_std()),
        });
        if is_directory {
            let child = directory.open_dir_nofollow(&name)?;
            list_directory(&child, &entry_relative, excluded, entries)?;
        }
    }
    Ok(())
}

/// Copies the tree under `source`, minus `excluded`, into the empty host directory `destination`.
///
/// Directories and symlinks are made again. Permissions and modification times are copied. Each
/// regular file is transferred with `copy_mode`.
pub(super) fn capture(
    source: &cap_std::fs::Dir,
    destination: &Path,
    excluded: &HashSet<PathBuf>,
    copy_mode: FileCopyMode,
) -> std::io::Result<()> {
    let entries = list_tree(source, excluded)?;
    for entry in &entries {
        let target = destination.join(&entry.relative);
        match &entry.kind {
            TreeEntryKind::Directory => std::fs::create_dir(&target)?,
            TreeEntryKind::File => {
                let mut options = cap_std::fs::OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                let source_file = source.open_with(&entry.relative, &options)?.into_std();
                let target_file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                transfer_file(copy_mode, &source_file, &target_file)?;
                target_file.set_permissions(host_permissions(&entry.permissions, &target_file)?)?;
                if let Some(modified) = entry.modified {
                    target_file.set_modified(modified)?;
                }
            }
            TreeEntryKind::Symlink(link_target) => {
                create_host_symlink(link_target, &target)?;
                if let Some(modified) = entry.modified {
                    fs_set_times::set_symlink_times(
                        &target,
                        None,
                        Some(fs_set_times::SystemTimeSpec::Absolute(modified)),
                    )?;
                }
            }
        }
    }
    for entry in entries.iter().rev() {
        if entry.kind != TreeEntryKind::Directory {
            continue;
        }
        let target = destination.join(&entry.relative);
        let directory = File::open(&target)?;
        directory.set_permissions(host_permissions(&entry.permissions, &directory)?)?;
        if let Some(modified) = entry.modified {
            directory.set_modified(modified)?;
        }
    }
    Ok(())
}

/// Copies the host tree under `source` into `destination` through the capability.
///
/// Directories and symlinks are made again. Permissions and modification times are copied. Each
/// regular file goes through the same copy as a seeded file, so it takes the project of
/// `destination`. An existing target path is an `AlreadyExists` error.
pub(super) fn seed(
    source: &Path,
    destination: &cap_std::fs::Dir,
    copy_mode: FileCopyMode,
    quota_authority: QuotaAuthority,
    materialization_root: &Path,
) -> std::io::Result<()> {
    let source_directory =
        cap_std::fs::Dir::open_ambient_dir(source, cap_std::ambient_authority())?;
    let entries = list_tree(&source_directory, &HashSet::new())?;
    for entry in &entries {
        match &entry.kind {
            TreeEntryKind::Directory => destination.create_dir(&entry.relative)?,
            TreeEntryKind::File => {
                copy_file_at_blocking(
                    copy_mode,
                    quota_authority,
                    materialization_root,
                    &source.join(&entry.relative),
                    destination,
                    &entry.relative,
                    false,
                )?;
                destination.set_permissions(&entry.relative, entry.permissions.clone())?;
                if let Some(modified) = entry.modified {
                    cap_fs_ext::DirExt::set_times(
                        destination,
                        &entry.relative,
                        None,
                        Some(capability_time(modified)),
                    )?;
                }
            }
            TreeEntryKind::Symlink(link_target) => {
                create_capability_symlink(destination, link_target, &entry.relative)?;
                if let Some(modified) = entry.modified {
                    destination.set_symlink_times(
                        &entry.relative,
                        None,
                        Some(capability_time(modified)),
                    )?;
                }
            }
        }
    }
    for entry in entries.iter().rev() {
        if entry.kind != TreeEntryKind::Directory {
            continue;
        }
        destination.set_permissions(&entry.relative, entry.permissions.clone())?;
        if let Some(modified) = entry.modified {
            cap_fs_ext::DirExt::set_times(
                destination,
                &entry.relative,
                None,
                Some(capability_time(modified)),
            )?;
        }
    }
    Ok(())
}

fn transfer_file(copy_mode: FileCopyMode, source: &File, target: &File) -> std::io::Result<()> {
    match copy_mode {
        FileCopyMode::Buffered => {
            std::io::copy(&mut &*source, &mut &*target)?;
            Ok(())
        }
        FileCopyMode::Reflink => {
            #[cfg(target_os = "linux")]
            {
                xfs::clone_file(target, source)
            }
            #[cfg(not(target_os = "linux"))]
            unreachable!("managed XFS is unavailable on this platform")
        }
    }
}

fn capability_time(time: SystemTime) -> cap_fs_ext::SystemTimeSpec {
    cap_fs_ext::SystemTimeSpec::Absolute(cap_std::time::SystemTime::from_std(time))
}

fn host_permissions(
    permissions: &cap_std::fs::Permissions,
    file: &File,
) -> std::io::Result<std::fs::Permissions> {
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        let _ = file;
        Ok(std::fs::Permissions::from_mode(permissions.mode()))
    }
    #[cfg(not(unix))]
    {
        let mut host = file.metadata()?.permissions();
        host.set_readonly(permissions.readonly());
        Ok(host)
    }
}

#[cfg(unix)]
fn read_link_contents(directory: &cap_std::fs::Dir, name: &Path) -> std::io::Result<PathBuf> {
    directory.read_link_contents(name)
}

#[cfg(not(unix))]
fn read_link_contents(directory: &cap_std::fs::Dir, name: &Path) -> std::io::Result<PathBuf> {
    directory.read_link(name)
}

#[cfg(unix)]
fn create_host_symlink(link_target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link_target, link)
}

#[cfg(not(unix))]
fn create_host_symlink(_link_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "tree copy cannot recreate symlinks on this platform",
    ))
}

#[cfg(unix)]
fn create_capability_symlink(
    directory: &cap_std::fs::Dir,
    link_target: &Path,
    link: &Path,
) -> std::io::Result<()> {
    directory.symlink_contents(link_target, link)
}

#[cfg(not(unix))]
fn create_capability_symlink(
    _directory: &cap_std::fs::Dir,
    _link_target: &Path,
    _link: &Path,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "tree copy cannot recreate symlinks on this platform",
    ))
}

/// Lists every root-relative path under `root` through the ambient filesystem, for comparisons.
#[cfg(test)]
pub(super) fn tree_listing(root: &Path) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&relative)).unwrap() {
            let entry = entry.unwrap();
            let entry_relative = relative.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry_relative.clone());
            }
            found.insert(entry_relative.to_string_lossy().into_owned());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::fs::PermissionsExt as _;
    use std::os::unix::ffi::OsStringExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, UNIX_EPOCH};
    use test_r::test;

    fn paths(items: &[&str]) -> HashSet<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    fn open(path: &Path) -> cap_std::fs::Dir {
        cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap()
    }

    fn fixture_tree(root: &Path) {
        std::fs::create_dir_all(root.join("data/nested")).unwrap();
        std::fs::create_dir(root.join("static")).unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::write(root.join("data/db.sqlite"), vec![0x5a; 12_000]).unwrap();
        std::fs::write(root.join("data/nested/note.txt"), b"nested note").unwrap();
        std::fs::write(root.join("static/asset.bin"), b"asset").unwrap();
        std::fs::write(root.join("config.toml"), b"[config]").unwrap();
        std::fs::write(root.join("script.sh"), b"#!/bin/sh").unwrap();
        std::fs::set_permissions(
            root.join("script.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::set_permissions(root.join("data"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        std::os::unix::fs::symlink("data/db.sqlite", root.join("db-link")).unwrap();
        std::os::unix::fs::symlink("/absolute/outside", root.join("outside-link")).unwrap();
        for (relative, seconds) in [
            ("data/db.sqlite", 1_700_000_001u64),
            ("data/nested/note.txt", 1_700_000_002),
            ("data/nested", 1_700_000_003),
            ("data", 1_700_000_004),
            ("script.sh", 1_700_000_005),
        ] {
            let time = UNIX_EPOCH + Duration::from_secs(seconds);
            File::options()
                .write(true)
                .open(root.join(relative))
                .or_else(|_| File::open(root.join(relative)))
                .unwrap()
                .set_modified(time)
                .unwrap();
        }
    }

    #[test]
    fn exclusions_keep_only_paths_that_can_name_an_entry() {
        let normalized = normalize_exclusions(&paths(&[
            "/lib/data.txt",
            "./config.toml",
            "a//b",
            "../escape",
            "a/../b",
            "/",
            ".",
            "",
        ]));

        assert_eq!(normalized, paths(&["lib/data.txt", "config.toml", "a/b"]));
    }

    #[test]
    fn listing_skips_excluded_paths_and_their_contents() {
        let source = tempfile::tempdir().unwrap();
        fixture_tree(source.path());

        let entries = list_tree(
            &open(source.path()),
            &paths(&["/static/asset.bin", "data/nested", "missing"]),
        )
        .unwrap();

        let relatives = entries
            .iter()
            .map(|entry| entry.relative.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            relatives,
            [
                "config.toml",
                "data",
                "data/db.sqlite",
                "db-link",
                "empty",
                "outside-link",
                "script.sh",
                "static",
            ]
        );
        assert_eq!(
            entries[3].kind,
            TreeEntryKind::Symlink(PathBuf::from("data/db.sqlite"))
        );
        assert_eq!(
            entries[5].kind,
            TreeEntryKind::Symlink(PathBuf::from("/absolute/outside"))
        );
        assert_eq!(entries[6].permissions.mode() & 0o777, 0o755);
        assert_eq!(
            entries[2].modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_001))
        );
    }

    #[test]
    fn listing_refuses_a_file_that_is_not_regular() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("regular"), b"contents").unwrap();
        let fifo =
            std::ffi::CString::new(source.path().join("pipe").into_os_string().into_vec()).unwrap();
        // SAFETY: `fifo` is a valid NUL-terminated path that lives for the whole call.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let destination = tempfile::tempdir().unwrap();

        let error = list_tree(&open(source.path()), &HashSet::new()).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("pipe"), "{error}");
        let error = capture(
            &open(source.path()),
            destination.path(),
            &HashSet::new(),
            FileCopyMode::Buffered,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            std::fs::read_dir(destination.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn listing_does_not_follow_a_symlink_to_a_directory() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("real")).unwrap();
        std::fs::write(source.path().join("real/file"), b"contents").unwrap();
        std::os::unix::fs::symlink("real", source.path().join("alias")).unwrap();

        let entries = list_tree(&open(source.path()), &HashSet::new()).unwrap();

        let relatives = entries
            .iter()
            .map(|entry| entry.relative.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(relatives, ["alias", "real", "real/file"]);
        assert_eq!(
            entries[0].kind,
            TreeEntryKind::Symlink(PathBuf::from("real"))
        );
    }

    #[test]
    fn capture_copies_everything_except_the_exclusions() {
        let source = tempfile::tempdir().unwrap();
        fixture_tree(source.path());
        let destination = tempfile::tempdir().unwrap();

        capture(
            &open(source.path()),
            destination.path(),
            &paths(&["static/asset.bin", "data/nested"]),
            FileCopyMode::Buffered,
        )
        .unwrap();

        let mut expected = tree_listing(source.path());
        for absent in ["static/asset.bin", "data/nested", "data/nested/note.txt"] {
            assert!(expected.remove(absent));
        }
        assert_eq!(tree_listing(destination.path()), expected);
        assert_eq!(
            std::fs::read(destination.path().join("data/db.sqlite")).unwrap(),
            vec![0x5a; 12_000]
        );
        assert_eq!(
            std::fs::read_link(destination.path().join("db-link")).unwrap(),
            PathBuf::from("data/db.sqlite")
        );
        assert_eq!(
            std::fs::read_link(destination.path().join("outside-link")).unwrap(),
            PathBuf::from("/absolute/outside")
        );
        let script = std::fs::metadata(destination.path().join("script.sh")).unwrap();
        assert_eq!(script.permissions().mode() & 0o777, 0o755);
        assert_eq!(
            script.modified().unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_005)
        );
        let data = std::fs::metadata(destination.path().join("data")).unwrap();
        assert_eq!(data.permissions().mode() & 0o777, 0o750);
        assert_eq!(
            data.modified().unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_004)
        );
        assert_eq!(
            std::fs::metadata(destination.path().join("data/db.sqlite"))
                .unwrap()
                .modified()
                .unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_001)
        );
    }

    #[test]
    fn seed_recreates_the_tree_through_the_capability() {
        let source = tempfile::tempdir().unwrap();
        fixture_tree(source.path());
        let destination = tempfile::tempdir().unwrap();

        seed(
            source.path(),
            &open(destination.path()),
            FileCopyMode::Buffered,
            QuotaAuthority::Unsupported,
            destination.path(),
        )
        .unwrap();

        assert_eq!(
            tree_listing(destination.path()),
            tree_listing(source.path())
        );
        assert_eq!(
            std::fs::read(destination.path().join("data/nested/note.txt")).unwrap(),
            b"nested note"
        );
        assert_eq!(
            std::fs::read_link(destination.path().join("outside-link")).unwrap(),
            PathBuf::from("/absolute/outside")
        );
        let script = std::fs::metadata(destination.path().join("script.sh")).unwrap();
        assert_eq!(script.permissions().mode() & 0o777, 0o755);
        assert_eq!(
            script.modified().unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_005)
        );
        let nested = std::fs::metadata(destination.path().join("data/nested")).unwrap();
        assert_eq!(
            nested.modified().unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_003)
        );
        assert_eq!(
            std::fs::metadata(destination.path().join("data"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
    }

    #[test]
    fn seed_fails_on_an_existing_target_path() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("data")).unwrap();
        std::fs::write(source.path().join("data/file"), b"new").unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::fs::create_dir(destination.path().join("data")).unwrap();
        std::fs::write(destination.path().join("data/file"), b"old").unwrap();

        let error = seed(
            source.path(),
            &open(destination.path()),
            FileCopyMode::Buffered,
            QuotaAuthority::Unsupported,
            destination.path(),
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(destination.path().join("data/file")).unwrap(),
            b"old"
        );
    }
}
