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
use std::ffi::{OsStr, OsString};
use std::time::SystemTime;

/// One object of a tree, listed for a copy.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TreeEntry {
    pub(super) relative: Box<Path>,
    pub(super) kind: TreeEntryKind,
    pub(super) permissions: cap_std::fs::Permissions,
    pub(super) modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TreeEntryKind {
    Directory,
    File,
    Symlink(Box<Path>),
}

/// The root-relative paths that a tree walk skips.
///
/// The walk also skips the contents of a skipped directory. The set does not change after it is
/// made. A lookup compares paths by their components, so a redundant separator in a stored path
/// does not stop a match.
#[derive(Debug, Default)]
pub(crate) struct TreeExclusions {
    paths: HashSet<Box<Path>>,
}

impl TreeExclusions {
    /// Makes the set from root-relative paths.
    ///
    /// A path that has only normal components stays as it is. A leading root component and
    /// current-directory components are removed from a path. A path with a parent or prefix
    /// component names no entry, so the set does not keep it. The set also does not keep a path
    /// that is empty after the removal.
    #[allow(dead_code)]
    pub(crate) fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            paths: paths
                .into_iter()
                .filter_map(normalize_exclusion)
                .map(PathBuf::into_boxed_path)
                .collect(),
        }
    }

    /// Tells whether the set holds a root-relative path.
    fn contains(&self, relative: &Path) -> bool {
        self.paths.contains(relative)
    }

    #[cfg(test)]
    pub(super) fn paths(&self) -> &HashSet<Box<Path>> {
        &self.paths
    }
}

fn normalize_exclusion(path: PathBuf) -> Option<PathBuf> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return (!path.as_os_str().is_empty()).then_some(path);
    }
    let mut normalized = PathBuf::new();
    path.components()
        .try_for_each(|component| match component {
            Component::Normal(component) => {
                normalized.push(component);
                Some(())
            }
            Component::RootDir | Component::CurDir => Some(()),
            Component::ParentDir | Component::Prefix(_) => None,
        })?;
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

/// Lists a tree, parents before children, without the excluded root-relative paths.
///
/// An excluded directory is absent together with its contents. Symlinks are listed, not followed.
/// Entries of one directory are in name order.
pub(super) fn list_tree(
    root: &cap_std::fs::Dir,
    excluded: &TreeExclusions,
) -> std::io::Result<Box<[TreeEntry]>> {
    list_directory(root, Path::new(""), excluded, Vec::new()).map(Vec::into_boxed_slice)
}

/// Adds the entries under `directory` to `listed` and gives the list back.
///
/// `relative` is the root-relative path of `directory`. A directory entry comes before its
/// contents.
fn list_directory(
    directory: &cap_std::fs::Dir,
    relative: &Path,
    excluded: &TreeExclusions,
    listed: Vec<TreeEntry>,
) -> std::io::Result<Vec<TreeEntry>> {
    sorted_names(directory)?
        .into_iter()
        .map(|name| (child_path(relative, &name), name))
        .filter(|(path, _)| !excluded.contains(path))
        .try_fold(listed, |mut listed, (path, name)| {
            let entry = tree_entry(directory, &name, path)?;
            let child = match entry.kind {
                TreeEntryKind::Directory => {
                    Some((directory.open_dir_nofollow(&name)?, entry.relative.clone()))
                }
                _ => None,
            };
            listed.push(entry);
            match child {
                Some((child, child_relative)) => {
                    list_directory(&child, &child_relative, excluded, listed)
                }
                None => Ok(listed),
            }
        })
}

/// Makes the root-relative path of an entry from the path of its directory and its name.
///
/// The path is made in one allocation of its final size.
fn child_path(relative: &Path, name: &OsStr) -> PathBuf {
    let separator = usize::from(!relative.as_os_str().is_empty());
    let mut path = PathBuf::with_capacity(relative.as_os_str().len() + separator + name.len());
    path.push(relative);
    path.push(name);
    path
}

/// Reads the names of the entries in a directory and sorts them.
fn sorted_names(directory: &cap_std::fs::Dir) -> std::io::Result<Vec<OsString>> {
    let mut names = directory
        .entries()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Reads one entry of a directory for a tree listing.
///
/// The entry gets `relative` as its root-relative path. A symlink is read and not followed. An
/// object that is not a regular file, a directory or a symlink gives an `InvalidData` error that
/// names `relative`.
fn tree_entry(
    directory: &cap_std::fs::Dir,
    name: &OsStr,
    relative: PathBuf,
) -> std::io::Result<TreeEntry> {
    let metadata = directory.symlink_metadata(name)?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        TreeEntryKind::Symlink(read_link_contents(directory, Path::new(name))?.into_boxed_path())
    } else if file_type.is_dir() {
        TreeEntryKind::Directory
    } else if file_type.is_file() {
        TreeEntryKind::File
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "tree entry {} is not a regular file, a directory or a symlink",
                relative.display()
            ),
        ));
    };
    Ok(TreeEntry {
        relative: relative.into_boxed_path(),
        kind,
        permissions: metadata.permissions(),
        modified: metadata.modified().ok().map(|time| time.into_std()),
    })
}

/// Copies the tree under `source`, minus `excluded`, into the empty host directory `destination`.
///
/// Directories and symlinks are made again. Permissions and modification times are copied. Each
/// regular file is transferred with `copy_mode`.
pub(super) fn capture(
    source: &cap_std::fs::Dir,
    destination: &Path,
    excluded: &TreeExclusions,
    copy_mode: FileCopyMode,
) -> std::io::Result<()> {
    let entries = list_tree(source, excluded)?;
    entries
        .iter()
        .try_for_each(|entry| capture_entry(source, destination, entry, copy_mode))?;
    entries
        .iter()
        .rev()
        .filter(|entry| entry.kind == TreeEntryKind::Directory)
        .try_for_each(|entry| set_captured_directory_attributes(destination, entry))
}

/// Makes one listed entry again under the host directory `destination`.
///
/// For a directory entry, this function makes an empty directory. A regular file is transferred
/// with `copy_mode` and gets the permissions and the modification time of the entry. A symlink is
/// made with the same target and gets the modification time of the entry.
fn capture_entry(
    source: &cap_std::fs::Dir,
    destination: &Path,
    entry: &TreeEntry,
    copy_mode: FileCopyMode,
) -> std::io::Result<()> {
    let target = destination.join(&entry.relative);
    match &entry.kind {
        TreeEntryKind::Directory => std::fs::create_dir(&target),
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
            Ok(())
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
            Ok(())
        }
    }
}

/// Gives a directory under the host directory `destination` the permissions and the modification
/// time of its listed entry.
fn set_captured_directory_attributes(destination: &Path, entry: &TreeEntry) -> std::io::Result<()> {
    let directory = File::open(destination.join(&entry.relative))?;
    directory.set_permissions(host_permissions(&entry.permissions, &directory)?)?;
    if let Some(modified) = entry.modified {
        directory.set_modified(modified)?;
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
    let entries = list_tree(&source_directory, &TreeExclusions::default())?;
    entries.iter().try_for_each(|entry| {
        seed_entry(
            source,
            destination,
            entry,
            copy_mode,
            quota_authority,
            materialization_root,
        )
    })?;
    entries
        .iter()
        .rev()
        .filter(|entry| entry.kind == TreeEntryKind::Directory)
        .try_for_each(|entry| set_seeded_directory_attributes(destination, entry))
}

/// Makes one listed entry again in `destination` through the capability.
///
/// For a directory entry, this function makes an empty directory. A regular file goes through the
/// same copy as a seeded file and gets the permissions and the modification time of the entry. A
/// symlink is made with the same target and gets the modification time of the entry.
fn seed_entry(
    source: &Path,
    destination: &cap_std::fs::Dir,
    entry: &TreeEntry,
    copy_mode: FileCopyMode,
    quota_authority: QuotaAuthority,
    materialization_root: &Path,
) -> std::io::Result<()> {
    match &entry.kind {
        TreeEntryKind::Directory => destination.create_dir(&entry.relative),
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
            Ok(())
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
            Ok(())
        }
    }
}

/// Gives a directory in `destination` the permissions and the modification time of its listed
/// entry.
fn set_seeded_directory_attributes(
    destination: &cap_std::fs::Dir,
    entry: &TreeEntry,
) -> std::io::Result<()> {
    destination.set_permissions(&entry.relative, entry.permissions.clone())?;
    if let Some(modified) = entry.modified {
        cap_fs_ext::DirExt::set_times(
            destination,
            &entry.relative,
            None,
            Some(capability_time(modified)),
        )?;
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
    list_into(root, PathBuf::new(), std::collections::BTreeSet::new())
}

/// Adds every root-relative path under `relative` to `found` and gives the set back.
#[cfg(test)]
fn list_into(
    root: &Path,
    relative: PathBuf,
    found: std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(root.join(&relative))
        .unwrap()
        .map(|entry| entry.unwrap())
        .fold(found, |mut found, entry| {
            let entry_relative = relative.join(entry.file_name());
            found.insert(entry_relative.to_string_lossy().into_owned());
            match entry.file_type().unwrap().is_dir() {
                true => list_into(root, entry_relative, found),
                false => found,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::fs::PermissionsExt as _;
    use std::os::unix::ffi::OsStringExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, UNIX_EPOCH};
    use test_r::test;

    fn paths(items: &[&str]) -> HashSet<Box<Path>> {
        items
            .iter()
            .map(|item| Box::from(Path::new(item)))
            .collect()
    }

    fn exclusions(items: &[&str]) -> TreeExclusions {
        TreeExclusions::new(items.iter().map(PathBuf::from))
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
        [
            ("data/db.sqlite", 1_700_000_001u64),
            ("data/nested/note.txt", 1_700_000_002),
            ("data/nested", 1_700_000_003),
            ("data", 1_700_000_004),
            ("script.sh", 1_700_000_005),
        ]
        .into_iter()
        .for_each(|(relative, seconds)| {
            let time = UNIX_EPOCH + Duration::from_secs(seconds);
            File::options()
                .write(true)
                .open(root.join(relative))
                .or_else(|_| File::open(root.join(relative)))
                .unwrap()
                .set_modified(time)
                .unwrap();
        });
    }

    #[test]
    fn exclusions_keep_only_paths_that_can_name_an_entry() {
        let excluded = exclusions(&[
            "/lib/data.txt",
            "./config.toml",
            "a//b",
            "../escape",
            "a/../b",
            "/",
            ".",
            "",
        ]);

        assert_eq!(
            excluded.paths(),
            &paths(&["lib/data.txt", "config.toml", "a/b"])
        );
    }

    #[test]
    fn exclusions_keep_a_root_relative_path_as_given() {
        let given = ["lib/data.txt", "lib//separator.txt", "directory/"];

        let excluded = exclusions(&given);

        let mut kept = excluded
            .paths()
            .iter()
            .map(|path| path.as_os_str().to_owned())
            .collect::<Vec<_>>();
        kept.sort();
        let mut expected = given.map(std::ffi::OsString::from).to_vec();
        expected.sort();
        assert_eq!(kept, expected);
        assert!(excluded.contains(Path::new("lib/data.txt")));
        assert!(excluded.contains(Path::new("lib/separator.txt")));
        assert!(excluded.contains(Path::new("directory")));
    }

    #[test]
    fn listing_skips_excluded_paths_and_their_contents() {
        let source = tempfile::tempdir().unwrap();
        fixture_tree(source.path());

        let entries = list_tree(
            &open(source.path()),
            &exclusions(&["/static/asset.bin", "data/nested", "missing"]),
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
            TreeEntryKind::Symlink(Path::new("data/db.sqlite").into())
        );
        assert_eq!(
            entries[5].kind,
            TreeEntryKind::Symlink(Path::new("/absolute/outside").into())
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

        let error = list_tree(&open(source.path()), &TreeExclusions::default()).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("pipe"), "{error}");
        let error = capture(
            &open(source.path()),
            destination.path(),
            &TreeExclusions::default(),
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

        let entries = list_tree(&open(source.path()), &TreeExclusions::default()).unwrap();

        let relatives = entries
            .iter()
            .map(|entry| entry.relative.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(relatives, ["alias", "real", "real/file"]);
        assert_eq!(
            entries[0].kind,
            TreeEntryKind::Symlink(Path::new("real").into())
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
            &exclusions(&["static/asset.bin", "data/nested"]),
            FileCopyMode::Buffered,
        )
        .unwrap();

        let mut expected = tree_listing(source.path());
        ["static/asset.bin", "data/nested", "data/nested/note.txt"]
            .into_iter()
            .for_each(|absent| {
                assert!(
                    expected.remove(absent),
                    "{absent} must be in the source listing"
                );
            });
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
