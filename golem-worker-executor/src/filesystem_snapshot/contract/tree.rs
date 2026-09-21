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

//! The trees of the contract suite: a fixture that the suite writes, and a listing that the suite
//! reads back. Neither uses the code of an adapter, so a defect of an adapter cannot hide in the
//! check.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

/// A temporary directory that goes away with all that is in it, also when a directory or a file
/// below it is read-only.
pub(super) struct Scratch(TempDir);

impl Scratch {
    pub(super) fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    pub(super) fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        make_writable(self.0.path());
    }
}

/// Gives each directory below `directory`, and on Windows each file too, the permission that a
/// removal needs. A symlink is not followed.
fn make_writable(directory: &Path) {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .for_each(|entry| {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                return;
            };
            if metadata.is_dir() {
                let _ = std::fs::set_permissions(&path, writable(metadata.permissions()));
                make_writable(&path);
            } else if cfg!(windows) && metadata.is_file() {
                let _ = std::fs::set_permissions(&path, writable(metadata.permissions()));
            }
        });
}

#[cfg(unix)]
fn writable(permissions: std::fs::Permissions) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(permissions.mode() | 0o700)
}

#[cfg(not(unix))]
fn writable(mut permissions: std::fs::Permissions) -> std::fs::Permissions {
    permissions.set_readonly(false);
    permissions
}

/// One entry of a listing: its path below the root, what it is, its permission bits and its
/// modification time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Listed {
    pub(super) path: PathBuf,
    pub(super) kind: ListedKind,
    pub(super) mode: u32,
    pub(super) modified: SystemTime,
}

/// What an entry of a listing is. A file is its size and the hash of its content, so a
/// difference in a large file gives a short message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ListedKind {
    Directory,
    File { size: u64, hash: String },
    Symlink(PathBuf),
}

/// Lists each entry below `root`, in the order of the paths. Symlinks are not followed.
pub(super) fn listing(root: &Path) -> Vec<Listed> {
    let mut listed = add_listed(root, Path::new(""), Vec::new());
    listed.sort_by(|left, right| left.path.cmp(&right.path));
    listed
}

fn add_listed(directory: &Path, relative: &Path, listed: Vec<Listed>) -> Vec<Listed> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .fold(listed, |mut listed, name| {
            let path = directory.join(&name);
            let relative = relative.join(&name);
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let kind = if metadata.is_dir() {
                ListedKind::Directory
            } else if metadata.file_type().is_symlink() {
                ListedKind::Symlink(std::fs::read_link(&path).unwrap())
            } else {
                let content = std::fs::read(&path).unwrap();
                ListedKind::File {
                    size: content.len() as u64,
                    hash: blake3::hash(&content).to_hex().to_string(),
                }
            };
            let is_directory = kind == ListedKind::Directory;
            listed.push(Listed {
                path: relative.clone(),
                kind,
                mode: mode_of(&metadata),
                modified: metadata.modified().unwrap(),
            });
            if is_directory {
                add_listed(&path, &relative, listed)
            } else {
                listed
            }
        })
}

#[cfg(unix)]
pub(super) fn mode_of(metadata: &Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
pub(super) fn mode_of(metadata: &Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}

/// Gives the number of regular files in the listing and the sum of their sizes.
pub(super) fn files_and_bytes(listed: &[Listed]) -> (u64, u64) {
    listed
        .iter()
        .filter_map(|entry| match entry.kind {
            ListedKind::File { size, .. } => Some(size),
            ListedKind::Directory | ListedKind::Symlink(_) => None,
        })
        .fold((0, 0), |(files, bytes), size| (files + 1, bytes + size))
}

/// One entry of a tree that the suite writes.
pub(super) enum Spec {
    Directory { mode: u32 },
    File { content: Vec<u8>, mode: u32 },
    Symlink { target: &'static str },
}

/// Writes the entries into `root`, parents before children, and gives each entry its own
/// modification time with nanoseconds. The directories get their times and permission bits
/// last, children before parents, so no write changes them after that.
pub(super) fn write_tree(root: &Path, entries: &[(&str, Spec)]) {
    entries
        .iter()
        .enumerate()
        .for_each(|(index, (path, spec))| {
            let target = root.join(path);
            let modified = time_of(index);
            match spec {
                Spec::Directory { .. } => std::fs::create_dir(&target).unwrap(),
                Spec::File { content, mode } => {
                    std::fs::write(&target, content).unwrap();
                    let file = std::fs::File::options().write(true).open(&target).unwrap();
                    file.set_modified(modified).unwrap();
                    set_mode(&target, *mode);
                }
                Spec::Symlink { target: link } => {
                    make_symlink(link, &target);
                    fs_set_times::set_symlink_times(
                        &target,
                        None,
                        Some(fs_set_times::SystemTimeSpec::Absolute(modified)),
                    )
                    .unwrap();
                }
            }
        });
    entries
        .iter()
        .enumerate()
        .rev()
        .for_each(|(index, (path, spec))| {
            if let Spec::Directory { mode } = spec {
                let target = root.join(path);
                fs_set_times::set_times(
                    &target,
                    None,
                    Some(fs_set_times::SystemTimeSpec::Absolute(time_of(index))),
                )
                .unwrap();
                set_mode(&target, *mode);
            }
        });
}

/// The modification time of the entry with the index: a whole second for each entry, and
/// nanoseconds that a store which keeps only milliseconds loses.
fn time_of(index: usize) -> SystemTime {
    UNIX_EPOCH + Duration::new(1_700_000_000 + index as u64 * 1_000, 123_456_789)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(path: &Path, mode: u32) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(not(unix))]
fn make_symlink(_target: &str, _link: &Path) {
    unreachable!("the fixture has symlinks only on unix");
}

/// Gives `size` bytes that do not repeat in a short period, so a store that compresses or cuts
/// its data into parts cannot make them small.
pub(super) fn pattern(size: usize) -> Vec<u8> {
    (0..size)
        .scan(0x2545_f491_u32, |state, _| {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            Some(*state as u8)
        })
        .collect()
}

/// The fixture that holds each kind of entry and each attribute that a snapshot keeps.
#[cfg(unix)]
pub(super) fn fixture() -> Vec<(&'static str, Spec)> {
    vec![
        (
            "a.txt",
            Spec::File {
                content: b"alpha".to_vec(),
                mode: 0o644,
            },
        ),
        (
            "absolute-link",
            Spec::Symlink {
                target: "/absolute/target",
            },
        ),
        (
            "big.bin",
            Spec::File {
                content: pattern(3 * 1024 * 1024),
                mode: 0o600,
            },
        ),
        (
            "dangling",
            Spec::Symlink {
                target: "missing/target",
            },
        ),
        ("dir", Spec::Directory { mode: 0o755 }),
        ("dir/link-to-file", Spec::Symlink { target: "../a.txt" }),
        ("dir/nested", Spec::Directory { mode: 0o700 }),
        (
            "dir/nested/deep.txt",
            Spec::File {
                content: b"deep".to_vec(),
                mode: 0o640,
            },
        ),
        ("dir-link", Spec::Symlink { target: "dir" }),
        (
            "empty",
            Spec::File {
                content: Vec::new(),
                mode: 0o644,
            },
        ),
        ("empty-dir", Spec::Directory { mode: 0o755 }),
        ("locked", Spec::Directory { mode: 0o555 }),
        (
            "locked/inside.txt",
            Spec::File {
                content: b"inside".to_vec(),
                mode: 0o644,
            },
        ),
        (
            "name with space é.txt",
            Spec::File {
                content: b"unicode".to_vec(),
                mode: 0o644,
            },
        ),
        (
            "read-only.txt",
            Spec::File {
                content: b"read only".to_vec(),
                mode: 0o444,
            },
        ),
        (
            "run.sh",
            Spec::File {
                content: b"#!/bin/sh\n".to_vec(),
                mode: 0o755,
            },
        ),
    ]
}

/// The fixture on a platform without unix permissions and without symlinks for every user.
#[cfg(not(unix))]
pub(super) fn fixture() -> Vec<(&'static str, Spec)> {
    vec![
        (
            "a.txt",
            Spec::File {
                content: b"alpha".to_vec(),
                mode: 0o644,
            },
        ),
        (
            "big.bin",
            Spec::File {
                content: pattern(3 * 1024 * 1024),
                mode: 0o644,
            },
        ),
        ("dir", Spec::Directory { mode: 0o755 }),
        ("dir/nested", Spec::Directory { mode: 0o755 }),
        (
            "dir/nested/deep.txt",
            Spec::File {
                content: b"deep".to_vec(),
                mode: 0o644,
            },
        ),
        (
            "empty",
            Spec::File {
                content: Vec::new(),
                mode: 0o644,
            },
        ),
        ("empty-dir", Spec::Directory { mode: 0o755 }),
        (
            "read-only.txt",
            Spec::File {
                content: b"read only".to_vec(),
                mode: 0o444,
            },
        ),
    ]
}

/// A tree of one file with the content.
pub(super) fn one_file(content: &str) -> Vec<(&'static str, Spec)> {
    vec![(
        "file.txt",
        Spec::File {
            content: content.as_bytes().to_vec(),
            mode: 0o644,
        },
    )]
}
