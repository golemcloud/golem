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

//! A directory tree as a value, and the two functions that read it from a disk and write it to
//! a disk. Both functions block, so they run on the blocking pool.

use super::super::SnapshotInfo;
use golem_common::model::Timestamp;
use std::ffi::OsString;
use std::fs::{OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

/// One entry below the root of a tree.
pub(super) struct TreeEntry {
    /// The path of the entry, relative to the root.
    path: Box<Path>,
    node: TreeNode,
    permissions: Permissions,
    modified: SystemTime,
}

/// What an entry of a tree is.
enum TreeNode {
    Directory,
    File(Box<[u8]>),
    Symlink(Box<Path>),
}

/// Reads the tree below the directory `root`.
///
/// A parent comes before its children, and the entries of a directory come in the order of
/// their names. Symlinks are read and not followed. Each name of a file gives its own entry, so
/// the tree does not keep hard links. An entry that is not a regular file, a directory or a
/// symlink gives an `InvalidInput` error, and the function does not open it. A `root` that is
/// not a directory gives a `NotADirectory` error.
pub(super) fn read_tree(root: &Path) -> io::Result<Arc<[TreeEntry]>> {
    if !std::fs::symlink_metadata(root)?.is_dir() {
        return Err(io::Error::new(
            ErrorKind::NotADirectory,
            format!("the tree {} is not a directory", root.display()),
        ));
    }
    add_entries(root, Path::new(""), Vec::new()).map(Arc::from)
}

/// Adds the entries below the directory `directory` to `entries`, and gives the list back.
///
/// `relative` is the path of `directory`, relative to the root of the tree.
fn add_entries(
    directory: &Path,
    relative: &Path,
    entries: Vec<TreeEntry>,
) -> io::Result<Vec<TreeEntry>> {
    sorted_names(directory)?
        .into_iter()
        .try_fold(entries, |mut entries, name| {
            let path = directory.join(&name);
            let entry = read_entry(&path, relative.join(&name).into_boxed_path())?;
            let is_directory = matches!(entry.node, TreeNode::Directory);
            let relative = entry.path.clone();
            entries.push(entry);
            if is_directory {
                add_entries(&path, &relative, entries)
            } else {
                Ok(entries)
            }
        })
}

/// Gives the names of the entries of a directory, in order.
fn sorted_names(directory: &Path) -> io::Result<Vec<OsString>> {
    let mut names = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Reads the entry at `path` without following a symlink. The entry gets `relative` as its path.
fn read_entry(path: &Path, relative: Box<Path>) -> io::Result<TreeEntry> {
    let metadata = std::fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let node = if file_type.is_dir() {
        TreeNode::Directory
    } else if file_type.is_symlink() {
        TreeNode::Symlink(std::fs::read_link(path)?.into_boxed_path())
    } else if file_type.is_file() {
        TreeNode::File(std::fs::read(path)?.into_boxed_slice())
    } else {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "the tree entry {} is not a regular file, a directory or a symlink",
                relative.display()
            ),
        ));
    };
    Ok(TreeEntry {
        path: relative,
        node,
        permissions: metadata.permissions(),
        modified: metadata.modified()?,
    })
}

/// Gives the info of a snapshot of the tree, with the time `created_at`.
pub(super) fn tree_info(tree: &[TreeEntry], created_at: Timestamp) -> SnapshotInfo {
    let (files, bytes) = tree
        .iter()
        .filter_map(|entry| match &entry.node {
            TreeNode::File(content) => Some(content.len() as u64),
            TreeNode::Directory | TreeNode::Symlink(_) => None,
        })
        .fold((0, 0), |(files, bytes), size| (files + 1, bytes + size));
    SnapshotInfo {
        created_at,
        files,
        bytes,
    }
}

/// Writes the tree into the empty directory `into`.
///
/// The function makes the entries in the order of the tree, so each directory is there before
/// its children. A file gets its permissions and its modification time when it is made, and a
/// symlink gets its modification time. The directories get their permissions and modification
/// times last, children before parents, so a read-only directory still takes its children, and
/// no later write changes the time of a directory. The tree does not keep the metadata of its
/// root, so the function does not set the permissions or the modification time of `into`.
///
/// An `into` that is missing gives a `NotFound` error, one that is not a directory gives a
/// `NotADirectory` error, and one that is not empty gives a `DirectoryNotEmpty` error. In these
/// cases the function writes nothing.
pub(super) fn write_tree(tree: &[TreeEntry], into: &Path) -> io::Result<()> {
    if !std::fs::symlink_metadata(into)?.is_dir() {
        return Err(io::Error::new(
            ErrorKind::NotADirectory,
            format!("the restore target {} is not a directory", into.display()),
        ));
    }
    if std::fs::read_dir(into)?.next().is_some() {
        return Err(io::Error::new(
            ErrorKind::DirectoryNotEmpty,
            format!("the restore target {} is not empty", into.display()),
        ));
    }
    tree.iter().try_for_each(|entry| make_entry(entry, into))?;
    tree.iter()
        .rev()
        .filter(|entry| matches!(entry.node, TreeNode::Directory))
        .try_for_each(|entry| set_directory_attributes(entry, into))
}

/// Makes one entry of the tree below `into`. A directory is made empty.
fn make_entry(entry: &TreeEntry, into: &Path) -> io::Result<()> {
    let target = into.join(&entry.path);
    match &entry.node {
        TreeNode::Directory => std::fs::create_dir(&target),
        TreeNode::File(content) => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            file.write_all(content)?;
            file.set_permissions(entry.permissions.clone())?;
            file.set_modified(entry.modified)
        }
        TreeNode::Symlink(link_target) => {
            create_symlink(link_target, &target)?;
            fs_set_times::set_symlink_times(
                &target,
                None,
                Some(fs_set_times::SystemTimeSpec::Absolute(entry.modified)),
            )
        }
    }
}

/// Gives the directory of the entry below `into` the modification time and the permissions of
/// the entry. The time comes first, because on Windows a read-only directory cannot take a new
/// time.
fn set_directory_attributes(entry: &TreeEntry, into: &Path) -> io::Result<()> {
    let target = into.join(&entry.path);
    fs_set_times::set_times(
        &target,
        None,
        Some(fs_set_times::SystemTimeSpec::Absolute(entry.modified)),
    )?;
    std::fs::set_permissions(&target, entry.permissions.clone())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Makes a symlink on Windows, where a symlink to a directory is another kind of symlink than a
/// symlink to a file. A target that is missing gets a symlink to a file.
#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    let resolved = link
        .parent()
        .map_or_else(|| target.to_path_buf(), |parent| parent.join(target));
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}
