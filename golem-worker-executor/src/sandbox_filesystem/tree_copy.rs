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
use cap_fs_ext::{FollowSymlinks, MetadataExt as _};
use std::ffi::{OsStr, OsString};
use std::time::SystemTime;

/// One object of a tree, listed for a copy.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TreeEntry {
    pub(super) relative: Box<Path>,
    pub(super) kind: TreeEntryKind,
    pub(super) permissions: cap_std::fs::Permissions,
    pub(super) modified: Option<SystemTime>,
    /// The identity of an entry that is not a directory and has more than one name, and `None` for
    /// another entry.
    pub(super) link: Option<NativeFileIdentity>,
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
    pub(crate) fn new(paths: impl IntoIterator<Item: AsRef<Path>>) -> Self {
        Self {
            paths: paths
                .into_iter()
                .filter_map(|path| normalize_exclusion(path.as_ref()))
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

fn normalize_exclusion(path: &Path) -> Option<Box<Path>> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return (!path.as_os_str().is_empty()).then(|| Box::from(path));
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
    (!normalized.as_os_str().is_empty()).then(|| normalized.into_boxed_path())
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
pub(super) fn child_path(relative: &Path, name: &OsStr) -> PathBuf {
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
    let link = (!file_type.is_dir() && metadata.nlink() > 1)
        .then(|| native_file_identity(&metadata))
        .transpose()?;
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
        link,
    })
}

/// Copies what is under `source` in `base`, minus `excluded`, into the empty host directory
/// `destination`.
///
/// An empty `source` names `base`. The paths in `excluded` are relative to `source`.
/// `destination` must be an empty directory and not a symlink: a missing path gives a `NotFound`
/// error, another kind of object gives a `NotADirectory` error, and a directory with an entry
/// gives a `DirectoryNotEmpty` error. Directories and symlinks are made again. Permissions and
/// modification times are copied, and `destination` gets the permissions and the modification
/// time of `source` after all entries are copied. Each regular file uses the selected
/// `transfer` action.
///
/// A regular file or a symlink with more than one name is copied once, at the first name that the
/// listing gives. The result gives the names of each such object as a [`LinkGroup`].
pub(super) fn copy_contents(
    base: &cap_std::fs::Dir,
    source: &Path,
    excluded: &TreeExclusions,
    destination: &Path,
    transfer: FileTransfer,
) -> std::io::Result<Box<[LinkGroup]>> {
    verify_empty_directory(destination)?;
    let opened;
    let source = if source.as_os_str().is_empty() {
        base
    } else {
        opened = base.open_dir_nofollow(source)?;
        &opened
    };
    let entries = list_tree(source, excluded)?;
    let plan = link_plan(&entries);
    entries
        .iter()
        .enumerate()
        .filter(|(position, _)| plan.copies(*position))
        .try_for_each(|(_, entry)| copy_out_entry(source, destination, entry, transfer))?;
    entries
        .iter()
        .rev()
        .filter(|entry| entry.kind == TreeEntryKind::Directory)
        .try_for_each(|entry| set_copied_directory_attributes(destination, entry))?;
    let source_metadata = source.dir_metadata()?;
    set_host_directory_attributes(
        destination,
        &source_metadata.permissions(),
        source_metadata.modified().ok().map(|time| time.into_std()),
    )?;
    Ok(plan.groups)
}

/// Which entries of a listing a copy makes, and the other names of each object that it makes once.
#[derive(Debug, PartialEq)]
struct LinkPlan {
    /// The positions in the listing of the names that the copy does not make, in ascending order.
    later: Box<[usize]>,
    /// A group for each object that the listing has at more than one name.
    groups: Box<[LinkGroup]>,
}

impl LinkPlan {
    /// Whether the copy makes the entry at `position` in the listing.
    fn copies(&self, position: usize) -> bool {
        self.later.binary_search(&position).is_err()
    }
}

/// Plans the copy of a listing that holds hard links.
///
/// An entry without a link identity is copied. A regular file or a symlink with more than one name
/// is copied once, at the first name that the listing gives, and its later names go into its
/// [`LinkGroup`] in the order of the listing. An identity that the listing gives at one name only
/// gives no group. A listing without hard links allocates nothing for the plan.
fn link_plan(entries: &[TreeEntry]) -> LinkPlan {
    let (_, later, groups) = entries.iter().enumerate().fold(
        (
            HashMap::<&NativeFileIdentity, usize>::new(),
            Vec::new(),
            Vec::<(&Path, Vec<&Path>)>::new(),
        ),
        |(mut positions, mut later, mut groups), (position, entry)| {
            if let Some(identity) = &entry.link {
                match positions.get(identity) {
                    Some(&group) => {
                        groups[group].1.push(&entry.relative);
                        later.push(position);
                    }
                    None => {
                        positions.insert(identity, groups.len());
                        groups.push((&entry.relative, Vec::new()));
                    }
                }
            }
            (positions, later, groups)
        },
    );
    LinkPlan {
        later: later.into_boxed_slice(),
        groups: groups
            .into_iter()
            .filter(|(_, others)| !others.is_empty())
            .map(|(first, others)| LinkGroup {
                first: Box::from(first),
                others: others.into_iter().map(Box::from).collect(),
            })
            .collect(),
    }
}

/// Checks that `path` is an empty directory and not a symlink.
fn verify_empty_directory(path: &Path) -> std::io::Result<()> {
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "copy target is not a directory",
        ));
    }
    if std::fs::read_dir(path)?.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::DirectoryNotEmpty,
            "copy target is not empty",
        ));
    }
    Ok(())
}

/// Makes one listed entry again under the host directory `destination`.
///
/// For a directory entry, this function makes an empty directory. A regular file is transferred
/// with `transfer` and gets the permissions and the modification time of the entry. A symlink is
/// made with the same target and gets the modification time of the entry.
fn copy_out_entry(
    source: &cap_std::fs::Dir,
    destination: &Path,
    entry: &TreeEntry,
    transfer: FileTransfer,
) -> std::io::Result<()> {
    let target = destination.join(&entry.relative);
    match &entry.kind {
        TreeEntryKind::Directory => std::fs::create_dir(&target),
        TreeEntryKind::File => {
            let source_file = open_file_nofollow(source, &entry.relative)?;
            let target_file = copy_new_file(transfer, &source_file, &target)?;
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

fn copy_new_file(transfer: FileTransfer, source: &File, target: &Path) -> std::io::Result<File> {
    match transfer {
        FileTransfer::Clone => clone_copied_file(source, target),
        _ => {
            let file = File::options().write(true).create_new(true).open(target)?;
            transfer_file(transfer, source, &file)?;
            Ok(file)
        }
    }
}

#[cfg(target_os = "macos")]
fn clone_copied_file(source: &File, target: &Path) -> std::io::Result<File> {
    let parent = File::open(
        target
            .parent()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
    )?;
    apfs::clone_file(
        source,
        &parent,
        target
            .file_name()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?
            .as_ref(),
    )?;
    File::open(target)
}

#[cfg(not(target_os = "macos"))]
fn clone_copied_file(_source: &File, _target: &Path) -> std::io::Result<File> {
    Err(macos_clone_unavailable())
}

/// Gives a directory under the host directory `destination` the permissions and the modification
/// time of its listed entry.
fn set_copied_directory_attributes(destination: &Path, entry: &TreeEntry) -> std::io::Result<()> {
    set_host_directory_attributes(
        &destination.join(&entry.relative),
        &entry.permissions,
        entry.modified,
    )
}

/// Gives the host directory at `directory` the permissions `permissions` and, where it is given,
/// the modification time `modified`.
fn set_host_directory_attributes(
    directory: &Path,
    permissions: &cap_std::fs::Permissions,
    modified: Option<SystemTime>,
) -> std::io::Result<()> {
    let directory = File::open(directory)?;
    directory.set_permissions(host_permissions(permissions, &directory)?)?;
    modified.map_or(Ok(()), |modified| directory.set_modified(modified))
}

/// What a seed entry needs to make objects in one sandbox.
#[derive(Clone, Copy)]
pub(super) struct SeedContext {
    pub(super) policy: SeedSyncPolicy,
    pub(super) access: SeedAccess,
    pub(super) placement: SeedPlacement,
}

/// Puts the host object at `source` into the sandbox at `destination` under `base`.
///
/// The source is read without following a symlink. A regular file becomes one file, a symlink
/// becomes a symlink, and a directory brings all that is under it. An object of another kind
/// gives an `InvalidData` error. Missing parent directories of `destination` are made through
/// the capability. A parent that is not a directory gives a `PermissionDenied` error.
pub(super) fn seed_entry(
    context: SeedContext,
    base: &cap_std::fs::Dir,
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    let (Some(source_parent), Some(source_name)) = (source.parent(), source.file_name()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "seed source has no parent directory",
        ));
    };
    let source_parent =
        cap_std::fs::Dir::open_ambient_dir(source_parent, cap_std::ambient_authority())?;
    let entry = tree_entry(&source_parent, source_name, PathBuf::from(source_name))?;
    match &entry.kind {
        TreeEntryKind::File => seed_file(context, &source_parent, &entry, base, destination),
        TreeEntryKind::Symlink(link_target) => {
            seed_symlink(context, base, destination, link_target, entry.modified)
        }
        TreeEntryKind::Directory => seed_directory(
            context,
            &source_parent.open_dir_nofollow(source_name)?,
            &entry,
            base,
            destination,
        ),
    }
}

/// Copies the regular file `source`, listed under `source_directory`, to `destination` under
/// `directory`.
///
/// The copy is written under a temporary name next to its target. Before it takes its name, it
/// gets the permissions of the source with the write permission that the access of `context`
/// sets, and the modification time of the source.
fn seed_file(
    context: SeedContext,
    source_directory: &cap_std::fs::Dir,
    source: &TreeEntry,
    directory: &cap_std::fs::Dir,
    destination: &Path,
) -> std::io::Result<()> {
    let (parent, name) = create_capability_copy_parent(directory, destination, context.policy)?;
    finalize_operation(
        || seed_file_resolved(context, source_directory, source, parent.as_dir(), &name),
        || {
            sync_seed_directories(
                context.policy.directory_action(SeedDirectoryStage::Entry),
                || Ok(()),
                || sync_ordinary_descriptor(parent.as_dir()),
            )
        },
    )
}

fn seed_file_resolved(
    context: SeedContext,
    source_directory: &cap_std::fs::Dir,
    source: &TreeEntry,
    parent: &cap_std::fs::Dir,
    name: &Path,
) -> std::io::Result<()> {
    let source_file = open_file_nofollow(source_directory, &source.relative)?;
    let temporary = create_seed_file(context.policy.transfer, parent, &source_file)?;
    temporary.finish(|temporary| {
        let action = context.policy.file;
        let temporary_file = match temporary.as_file().try_clone() {
            Ok(file) => file.into_std(),
            Err(error) => {
                if action == SeedFileSyncAction::Ordinary {
                    return finish_operation(Err(error), || temporary.sync_ordinary());
                }
                return Err(error);
            }
        };
        let transfer = || {
            transfer_seed_file(
                context.policy.transfer,
                &source_file,
                &temporary_file,
                temporary.as_file_mut(),
            )
        };
        let attributes = || {
            temporary_file.set_permissions(seeded_permissions(
                host_permissions(&source.permissions, &temporary_file)?,
                context.access,
            ))?;
            if let Some(modified) = source.modified {
                temporary_file.set_modified(modified)?;
            }
            Ok(())
        };
        prepare_seed_file(action, transfer, attributes, || {
            sync_seed_file(action, &temporary_file)
        })?;
        // `CreateNew` makes the name with one no-clobber call. Its `AlreadyExists` error applies the
        // `Refuse` row of `placement_action` for every object that is already there, atomically.
        match context.placement {
            SeedPlacement::CreateNew => temporary.persist_noclobber(name),
            SeedPlacement::Replace => temporary.persist_replacing(name),
        }
    })
}

/// Makes a symlink to `link_target` at `destination` under `directory`, with the modification
/// time `modified`.
///
/// With `CreateNew`, the symlink is made at its name in one call, and its `AlreadyExists` error
/// applies the `Refuse` row of [`placement_action`]. A symlink that takes the place of a target is
/// made under a temporary name first.
fn seed_symlink(
    context: SeedContext,
    directory: &cap_std::fs::Dir,
    destination: &Path,
    link_target: &Path,
    modified: Option<SystemTime>,
) -> std::io::Result<()> {
    let (parent, name) = create_capability_copy_parent(directory, destination, context.policy)?;
    finalize_operation(
        || seed_symlink_resolved(context, parent.as_dir(), &name, link_target, modified),
        || {
            sync_seed_directories(
                context.policy.directory_action(SeedDirectoryStage::Entry),
                || Ok(()),
                || sync_ordinary_descriptor(parent.as_dir()),
            )
        },
    )
}

fn seed_symlink_resolved(
    context: SeedContext,
    directory: &cap_std::fs::Dir,
    name: &Path,
    link_target: &Path,
    modified: Option<SystemTime>,
) -> std::io::Result<()> {
    match context.placement {
        SeedPlacement::CreateNew => make_symlink(directory, link_target, name, modified),
        SeedPlacement::Replace => {
            let temporary = temporary_copy_name();
            finalize_observed_operation(
                || {
                    make_symlink(directory, link_target, &temporary, modified)?;
                    clear_for_replacement(directory, name)?;
                    directory.rename(&temporary, directory, name)
                },
                |placement| {
                    cleanup_seed_symlink(
                        seed_symlink_cleanup_action(matches!(placement, Ok(Ok(())))),
                        directory,
                        &temporary,
                    )
                },
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedSymlinkCleanupAction {
    None,
    RemoveTemporary,
}

fn seed_symlink_cleanup_action(placed: bool) -> SeedSymlinkCleanupAction {
    if placed {
        SeedSymlinkCleanupAction::None
    } else {
        SeedSymlinkCleanupAction::RemoveTemporary
    }
}

fn cleanup_seed_symlink(
    action: SeedSymlinkCleanupAction,
    directory: &cap_std::fs::Dir,
    temporary: &Path,
) -> std::io::Result<()> {
    match action {
        SeedSymlinkCleanupAction::None => Ok(()),
        SeedSymlinkCleanupAction::RemoveTemporary => directory.remove_file(temporary),
    }
}

/// Makes a symlink to `link_target` at `link` in `directory`, with the modification time
/// `modified`.
fn make_symlink(
    directory: &cap_std::fs::Dir,
    link_target: &Path,
    link: &Path,
    modified: Option<SystemTime>,
) -> std::io::Result<()> {
    create_capability_symlink(directory, link_target, link)?;
    modified.map_or(Ok(()), |modified| {
        directory.set_symlink_times(link, None, Some(capability_time(modified)))
    })
}

/// Puts what is under the host directory `source` into the directory at `destination` under
/// `base`.
///
/// An empty `destination` names `base`. [`seed_directory_at`] makes the target directory, or
/// decides what happens to the object that is already there. A target directory that the seed
/// makes gets the permissions and the modification time of `source` after all that is under it
/// is made.
fn seed_directory(
    context: SeedContext,
    source: &cap_std::fs::Dir,
    source_entry: &TreeEntry,
    base: &cap_std::fs::Dir,
    destination: &Path,
) -> std::io::Result<()> {
    if destination.as_os_str().is_empty() {
        return seed_directory_contents(context, source, base);
    }
    let (parent, name) = create_capability_copy_parent(base, destination, context.policy)?;
    let seeded = seed_directory_at(context, parent.as_dir(), &name)?;
    let target = parent.as_dir().open_dir_nofollow(&name)?;
    seed_directory_contents(context, source, &target)?;
    if seeded == SeededDirectory::Made {
        set_seeded_directory_attributes(context.policy, parent.as_dir(), &name, source_entry)
    } else {
        Ok(())
    }
}

/// Makes what is under the host directory `source` again under `target`, parents first.
///
/// A directory that the walk makes gets the permissions and the modification time of its source
/// after all that is under it is made.
fn seed_directory_contents(
    context: SeedContext,
    source: &cap_std::fs::Dir,
    target: &cap_std::fs::Dir,
) -> std::io::Result<()> {
    let entries = list_tree(source, &TreeExclusions::default())?;
    let made = entries.iter().try_fold(Vec::new(), |made, entry| {
        seed_listed_entry(context, source, target, entry, made)
    })?;
    made.iter().rev().try_for_each(|entry| {
        set_seeded_directory_attributes(context.policy, target, &entry.relative, entry)
    })
}

/// Seeds one listed entry under `target`.
///
/// `made` holds the directories that the walk made, parents first. The result gives it back, with
/// the directory of `entry` added when the call made it.
fn seed_listed_entry<'a>(
    context: SeedContext,
    source: &cap_std::fs::Dir,
    target: &cap_std::fs::Dir,
    entry: &'a TreeEntry,
    mut made: Vec<&'a TreeEntry>,
) -> std::io::Result<Vec<&'a TreeEntry>> {
    match &entry.kind {
        TreeEntryKind::Directory => {
            let sync = context
                .policy
                .directory_action(SeedDirectoryStage::DirectoryPlacement(
                    DirectoryPlacementObservation::NotAttempted,
                ));
            let seeded = if sync == SeedDirectoryAction::Parent {
                let (parent, name) =
                    create_capability_copy_parent(target, &entry.relative, context.policy)?;
                seed_directory_at(context, parent.as_dir(), &name)?
            } else {
                seed_directory_at(context, target, &entry.relative)?
            };
            if seeded == SeededDirectory::Made {
                made.push(entry);
            }
        }
        TreeEntryKind::File => seed_file(context, source, entry, target, &entry.relative)?,
        TreeEntryKind::Symlink(link_target) => seed_symlink(
            context,
            target,
            &entry.relative,
            link_target,
            entry.modified,
        )?,
    }
    Ok(made)
}

/// What happened at the target path of a directory in a seed source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeededDirectory {
    /// The seed made the directory.
    Made,
    /// A directory was already there, and the contents of the source go into it.
    Merged,
}

/// Makes the directory at `path` in `directory` for a directory in a seed source.
///
/// A free path is made with one call and no check before it. When something is already at the
/// path, [`placement_action`] decides: a directory merges under every placement, and another kind
/// of object gives an `AlreadyExists` error with `CreateNew` and is removed with `Replace`. The
/// removal and the second creation of `Replace` are separate calls, so a concurrent change between
/// them gives an error.
fn seed_directory_at(
    context: SeedContext,
    directory: &cap_std::fs::Dir,
    path: &Path,
) -> std::io::Result<SeededDirectory> {
    finalize_observed_operation(
        || match directory.create_dir(path) {
            Ok(()) => Ok(SeededDirectory::Made),
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => Err(error),
            Err(error) => {
                let occupant = occupant(&directory.symlink_metadata(path)?);
                match placement_action(&TreeEntryKind::Directory, occupant, context.placement) {
                    PlacementAction::Merge => Ok(SeededDirectory::Merged),
                    PlacementAction::Refuse => Err(error),
                    PlacementAction::Overwrite => {
                        directory.remove_file(path)?;
                        directory.create_dir(path)?;
                        Ok(SeededDirectory::Made)
                    }
                    PlacementAction::RemoveDirectory => {
                        directory.remove_dir_all(path)?;
                        directory.create_dir(path)?;
                        Ok(SeededDirectory::Made)
                    }
                }
            }
        },
        |seeded| {
            let observed = match seeded {
                Ok(Ok(SeededDirectory::Made)) => DirectoryPlacementObservation::Made,
                Ok(Ok(SeededDirectory::Merged)) => DirectoryPlacementObservation::Merged,
                _ => DirectoryPlacementObservation::Failed,
            };
            sync_seed_directories(
                context
                    .policy
                    .directory_action(SeedDirectoryStage::DirectoryPlacement(observed)),
                || {
                    directory
                        .open_dir_nofollow(path)
                        .and_then(|child| sync_ordinary_descriptor(&child))
                },
                || sync_ordinary_descriptor(directory),
            )
        },
    )
}

/// What a seed finds at the target path of an entry, without following a symlink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Occupant {
    Directory,
    Other,
}

/// What a seed does at a target path that holds an object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlacementAction {
    /// Puts what is under the source directory into the directory that is there.
    Merge,
    /// Changes nothing. The `AlreadyExists` error of the attempt is the result.
    Refuse,
    /// Removes the directory that is there with all that is in it, then puts the object there.
    RemoveDirectory,
    /// Puts the object in place of the object that is there.
    Overwrite,
}

/// Decides what a seed of `source` with `placement` does at a target path that holds `occupant`.
///
/// A directory in the source merges into a directory that is there, under every placement.
/// Otherwise `CreateNew` refuses, and `Replace` takes the place of the object that is there, and
/// removes a directory that is there with all that is in it first.
fn placement_action(
    source: &TreeEntryKind,
    occupant: Occupant,
    placement: SeedPlacement,
) -> PlacementAction {
    match (source, occupant, placement) {
        (TreeEntryKind::Directory, Occupant::Directory, _) => PlacementAction::Merge,
        (_, _, SeedPlacement::CreateNew) => PlacementAction::Refuse,
        (TreeEntryKind::File | TreeEntryKind::Symlink(_), Occupant::Directory, _) => {
            PlacementAction::RemoveDirectory
        }
        (_, Occupant::Other, SeedPlacement::Replace) => PlacementAction::Overwrite,
    }
}

/// Gives what the metadata of an object, read without following a symlink, shows.
fn occupant(metadata: &cap_std::fs::Metadata) -> Occupant {
    if metadata.is_dir() {
        Occupant::Directory
    } else {
        Occupant::Other
    }
}

/// Clears `name` in `directory` for a rename of a file or a symlink onto it, as
/// [`placement_action`] decides for a source that is not a directory with `Replace`.
///
/// A free name and an object that the rename replaces stay as they are. A directory goes away with
/// all that is in it. A symlink is not followed. A file and a symlink have the same row in the
/// table, so the function asks the table for a file.
pub(super) fn clear_for_replacement(
    directory: &cap_std::fs::Dir,
    name: &Path,
) -> std::io::Result<()> {
    let found = match directory.symlink_metadata(name) {
        Ok(metadata) => occupant(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    match placement_action(&TreeEntryKind::File, found, SeedPlacement::Replace) {
        PlacementAction::RemoveDirectory => directory.remove_dir_all(name),
        PlacementAction::Overwrite => Ok(()),
        PlacementAction::Merge | PlacementAction::Refuse => {
            Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
        }
    }
}

/// Gives the directory at `path` in `directory` the permissions and the modification time of its
/// listed source entry.
fn set_seeded_directory_attributes(
    policy: SeedSyncPolicy,
    directory: &cap_std::fs::Dir,
    path: &Path,
    source: &TreeEntry,
) -> std::io::Result<()> {
    let action = policy.directory_action(SeedDirectoryStage::DirectoryAttributes);
    if action == SeedDirectoryAction::OpenedDirectory {
        // Permissions can prevent path-based time changes and reopening the directory for sync.
        let opened = directory.open_dir_nofollow(path)?;
        return finalize_operation(
            || {
                let file = opened.try_clone()?.into_std_file();
                file.set_permissions(host_permissions(&source.permissions, &file)?)?;
                source
                    .modified
                    .map_or(Ok(()), |modified| file.set_modified(modified))
            },
            || sync_seed_directories(action, || sync_ordinary_descriptor(&opened), || Ok(())),
        );
    }
    directory.set_permissions(path, source.permissions.clone())?;
    if let Some(modified) = source.modified {
        cap_fs_ext::DirExt::set_times(directory, path, None, Some(capability_time(modified)))?;
    }
    Ok(())
}

/// Opens a file in `directory` for reading, without following a symlink.
fn open_file_nofollow(directory: &cap_std::fs::Dir, path: &Path) -> std::io::Result<File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    Ok(directory.open_with(path, &options)?.into_std())
}

/// Gives `permissions` the write permission that `access` sets.
fn seeded_permissions(
    mut permissions: std::fs::Permissions,
    access: SeedAccess,
) -> std::fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = permissions.mode();
        match access {
            SeedAccess::FromSource => {}
            SeedAccess::ReadOnly => permissions.set_mode(mode & !0o222),
            SeedAccess::ReadWrite => permissions.set_mode(mode | 0o200),
        }
    }
    #[cfg(not(unix))]
    match access {
        SeedAccess::FromSource => {}
        SeedAccess::ReadOnly => permissions.set_readonly(true),
        SeedAccess::ReadWrite => permissions.set_readonly(false),
    }
    permissions
}

/// Makes `target` share the extents of `source` in the project `project_id` of the sandbox.
fn reflink_into_project(
    project_id: NonZeroU32,
    target: &File,
    source: &File,
) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        xfs::reflink_into_project(project_id, target, source)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (project_id, target, source);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "managed XFS is unavailable on this platform",
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SeedSyncPlatform {
    MacOs,
    Linux,
    Other,
}

pub(super) fn native_seed_sync_platform() -> SeedSyncPlatform {
    if cfg!(target_os = "macos") {
        SeedSyncPlatform::MacOs
    } else if cfg!(target_os = "linux") {
        SeedSyncPlatform::Linux
    } else {
        SeedSyncPlatform::Other
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedFileSyncAction {
    Standard,
    Ordinary,
}

fn seed_file_sync_action(platform: SeedSyncPlatform, transfer: FileTransfer) -> SeedFileSyncAction {
    match (platform, transfer) {
        (SeedSyncPlatform::MacOs, FileTransfer::Bytes | FileTransfer::Clone) => {
            SeedFileSyncAction::Ordinary
        }
        _ => SeedFileSyncAction::Standard,
    }
}

pub(super) fn file_transfer_action(
    platform: SeedSyncPlatform,
    transfer: FileTransfer,
) -> FileTransfer {
    match (platform, transfer) {
        (SeedSyncPlatform::MacOs, FileTransfer::Reflink) => FileTransfer::Clone,
        _ => transfer,
    }
}

fn create_seed_file<'a>(
    transfer: FileTransfer,
    parent: &'a cap_std::fs::Dir,
    source: &File,
) -> std::io::Result<CapabilityTempFile<'a>> {
    match transfer {
        FileTransfer::Clone => clone_seed_file(parent, source),
        _ => CapabilityTempFile::new(parent),
    }
}

#[cfg(target_os = "macos")]
fn clone_seed_file<'a>(
    parent: &'a cap_std::fs::Dir,
    source: &File,
) -> std::io::Result<CapabilityTempFile<'a>> {
    CapabilityTempFile::from_clone(parent, source)
}

#[cfg(not(target_os = "macos"))]
fn clone_seed_file<'a>(
    _parent: &'a cap_std::fs::Dir,
    _source: &File,
) -> std::io::Result<CapabilityTempFile<'a>> {
    Err(macos_clone_unavailable())
}

#[cfg(not(target_os = "macos"))]
fn macos_clone_unavailable() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "macOS file clone is unavailable on this platform",
    )
}

fn transfer_seed_file(
    transfer: FileTransfer,
    source: &File,
    target: &File,
    temporary: &mut cap_std::fs::File,
) -> std::io::Result<()> {
    match transfer {
        FileTransfer::Bytes => std::io::copy(&mut &*source, temporary).map(|_| ()),
        _ => transfer_file(transfer, source, target),
    }
}

#[derive(Clone, Copy)]
pub(super) struct SeedSyncPolicy {
    transfer: FileTransfer,
    file: SeedFileSyncAction,
    batch: SeedBatchAction,
}

impl SeedSyncPolicy {
    pub(super) fn new(platform: SeedSyncPlatform, transfer: FileTransfer) -> Self {
        let transfer = file_transfer_action(platform, transfer);
        let file = seed_file_sync_action(platform, transfer);
        Self {
            transfer,
            file,
            batch: seed_batch_action(platform, transfer, file),
        }
    }

    fn directory_action(self, stage: SeedDirectoryStage) -> SeedDirectoryAction {
        seed_directory_action(self.file, stage)
    }

    pub(super) fn requires_batch_root(self) -> bool {
        self.batch == SeedBatchAction::MacFullFlush
    }
}

fn sync_seed_file(action: SeedFileSyncAction, file: &File) -> std::io::Result<()> {
    match action {
        SeedFileSyncAction::Standard => file.sync_all(),
        SeedFileSyncAction::Ordinary => sync_ordinary_descriptor(file),
    }
}

#[cfg(unix)]
pub(super) fn sync_ordinary_descriptor<F: std::os::fd::AsFd + ?Sized>(
    file: &F,
) -> std::io::Result<()> {
    rustix::fs::fsync(file).map_err(std::io::Error::from)
}

#[cfg(not(unix))]
pub(super) fn sync_ordinary_descriptor<F: ?Sized>(_file: &F) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "ordinary filesystem sync is unavailable on this platform",
    ))
}

fn prepare_seed_file(
    action: SeedFileSyncAction,
    transfer: impl FnOnce() -> std::io::Result<()>,
    attributes: impl FnOnce() -> std::io::Result<()>,
    sync: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    match action {
        SeedFileSyncAction::Ordinary => finalize_operation(
            || {
                transfer()?;
                attributes()
            },
            sync,
        ),
        SeedFileSyncAction::Standard => {
            transfer()?;
            sync()?;
            attributes()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedBatchAction {
    None,
    LinuxVolume,
    MacFullFlush,
    Unavailable,
}

fn seed_batch_action(
    platform: SeedSyncPlatform,
    transfer: FileTransfer,
    file: SeedFileSyncAction,
) -> SeedBatchAction {
    if file == SeedFileSyncAction::Ordinary {
        return SeedBatchAction::MacFullFlush;
    }
    match (platform, transfer) {
        (SeedSyncPlatform::Linux, FileTransfer::Reflink | FileTransfer::ReflinkIntoProject(_)) => {
            SeedBatchAction::LinuxVolume
        }
        (SeedSyncPlatform::Other, FileTransfer::Reflink | FileTransfer::ReflinkIntoProject(_)) => {
            SeedBatchAction::Unavailable
        }
        _ => SeedBatchAction::None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectoryPlacementObservation {
    NotAttempted,
    Failed,
    Made,
    Merged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedDirectoryStage {
    Entry,
    DirectoryPlacement(DirectoryPlacementObservation),
    Ancestor { opened: bool },
    DirectoryAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedDirectoryAction {
    None,
    Parent,
    OpenedDirectory,
    ChildThenParent,
}

/// Chooses directory synchronization from facts and observations already made by a seed.
fn seed_directory_action(
    file: SeedFileSyncAction,
    stage: SeedDirectoryStage,
) -> SeedDirectoryAction {
    if file != SeedFileSyncAction::Ordinary {
        return SeedDirectoryAction::None;
    }
    match stage {
        SeedDirectoryStage::Entry
        | SeedDirectoryStage::DirectoryPlacement(
            DirectoryPlacementObservation::NotAttempted | DirectoryPlacementObservation::Failed,
        )
        | SeedDirectoryStage::Ancestor { opened: false } => SeedDirectoryAction::Parent,
        SeedDirectoryStage::DirectoryPlacement(DirectoryPlacementObservation::Made)
        | SeedDirectoryStage::Ancestor { opened: true } => SeedDirectoryAction::ChildThenParent,
        SeedDirectoryStage::DirectoryAttributes => SeedDirectoryAction::OpenedDirectory,
        SeedDirectoryStage::DirectoryPlacement(DirectoryPlacementObservation::Merged) => {
            SeedDirectoryAction::None
        }
    }
}

/// Attempts the finalizer even after a failed operation and keeps the earliest error.
pub(super) fn finish_operation<T, E>(
    result: Result<T, E>,
    finalize: impl FnOnce() -> Result<(), E>,
) -> Result<T, E> {
    finish_unwinding(Ok(result), finalize)
}

/// Runs cleanup before resuming the original panic, even if cleanup also panics.
/// An ordinary operation error also keeps priority over a later finalizer panic.
pub(super) fn finish_unwinding<T, E>(
    result: std::thread::Result<Result<T, E>>,
    finalize: impl FnOnce() -> Result<(), E>,
) -> Result<T, E> {
    let finalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(finalize));
    operation_result(result, finalized)
}

fn operation_result<T, E>(
    result: std::thread::Result<Result<T, E>>,
    finalized: std::thread::Result<Result<(), E>>,
) -> Result<T, E> {
    match (result, finalized) {
        (Err(panic), _) => std::panic::resume_unwind(panic),
        (Ok(Err(error)), _) => Err(error),
        (Ok(Ok(value)), Ok(finalized)) => finalized.map(|()| value),
        (Ok(Ok(_)), Err(panic)) => std::panic::resume_unwind(panic),
    }
}

pub(super) fn finalize_operation<T, E>(
    operation: impl FnOnce() -> Result<T, E>,
    finalize: impl FnOnce() -> Result<(), E>,
) -> Result<T, E> {
    finalize_observed_operation(operation, |_| finalize())
}

pub(super) fn finalize_observed_operation<T, E>(
    operation: impl FnOnce() -> Result<T, E>,
    finalize: impl FnOnce(&std::thread::Result<Result<T, E>>) -> Result<(), E>,
) -> Result<T, E> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
    let finalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| finalize(&result)));
    operation_result(result, finalized)
}

fn sync_seed_directories(
    action: SeedDirectoryAction,
    sync_opened: impl FnOnce() -> std::io::Result<()>,
    sync_parent: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    match action {
        SeedDirectoryAction::None => Ok(()),
        SeedDirectoryAction::Parent => sync_parent(),
        SeedDirectoryAction::OpenedDirectory => sync_opened(),
        SeedDirectoryAction::ChildThenParent => finalize_operation(sync_opened, sync_parent),
    }
}

/// Finalizes a missing ancestor through its opened child and actual parent, including failures.
pub(super) fn finish_seed_ancestor(
    policy: SeedSyncPolicy,
    open_child: impl FnOnce() -> std::io::Result<cap_std::fs::Dir>,
    parent: &cap_std::fs::Dir,
) -> std::io::Result<cap_std::fs::Dir> {
    finalize_observed_operation(open_child, |child| {
        sync_seed_directories(
            policy.directory_action(SeedDirectoryStage::Ancestor {
                opened: matches!(child, Ok(Ok(_))),
            }),
            || match child {
                Ok(Ok(child)) => sync_ordinary_descriptor(child),
                _ => Ok(()),
            },
            || sync_ordinary_descriptor(parent),
        )
    })
}

#[cfg(any(target_os = "macos", test))]
fn sync_seed_batch_submissions(
    sync_file: impl FnOnce() -> std::io::Result<()>,
    sync_root: impl FnOnce() -> std::io::Result<()>,
    full_flush: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    finalize_operation(|| finalize_operation(sync_file, sync_root), full_flush)
}

/// Executes the selected batch action after all entry finalizers, including on failure or unwind.
pub(super) fn sync_seed_batch(
    policy: SeedSyncPolicy,
    admitted: Option<&NativeRootHandles>,
    root_path: &Path,
) -> std::io::Result<()> {
    match policy.batch {
        SeedBatchAction::None => Ok(()),
        SeedBatchAction::LinuxVolume => sync_seed_linux_volume(root_path),
        SeedBatchAction::MacFullFlush => {
            let root = admitted.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "macOS seed batch root was not admitted",
                )
            })?;
            sync_seed_mac_full_flush(root)
        }
        SeedBatchAction::Unavailable => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "copy-on-write storage is unavailable on this platform",
        )),
    }
}

#[cfg(target_os = "linux")]
fn sync_seed_linux_volume(path: &Path) -> std::io::Result<()> {
    xfs::sync_volume(path)
}

#[cfg(not(target_os = "linux"))]
fn sync_seed_linux_volume(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Linux seed volume sync is unavailable on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn sync_seed_mac_full_flush(root: &NativeRootHandles) -> std::io::Result<()> {
    sync_seed_batch_submissions(
        || sync_ordinary_descriptor(root.seed_flusher.as_ref()),
        || sync_ordinary_descriptor(root.directory.as_ref()),
        || rustix::fs::fcntl_fullfsync(root.seed_flusher.as_ref()).map_err(std::io::Error::from),
    )
}

#[cfg(not(target_os = "macos"))]
fn sync_seed_mac_full_flush(_root: &NativeRootHandles) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "macOS fullflush is unavailable on this platform",
    ))
}

/// Transfers contents into a file whose clone, if selected, has already been created.
fn transfer_file(transfer: FileTransfer, source: &File, target: &File) -> std::io::Result<()> {
    match transfer {
        FileTransfer::Bytes => std::io::copy(&mut &*source, &mut &*target).map(|_| ()),
        FileTransfer::Clone => Ok(()),
        FileTransfer::Reflink => reflink_file(source, target),
        FileTransfer::ReflinkIntoProject(project) => reflink_into_project(project, target, source),
    }
}

#[cfg(target_os = "linux")]
fn reflink_file(source: &File, target: &File) -> std::io::Result<()> {
    xfs::clone_file(target, source)
}

#[cfg(not(target_os = "linux"))]
fn reflink_file(_source: &File, _target: &File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Linux file-descriptor reflink is unavailable on this platform",
    ))
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
    #[cfg(unix)]
    use cap_std::fs::PermissionsExt as _;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt as _;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, UNIX_EPOCH};
    use test_r::test;

    #[test]
    fn seed_file_submission_preserves_standard_order_and_finalizes_ordinary_failures() {
        use SeedFileSyncAction::{Ordinary, Standard};
        [
            (
                Standard,
                false,
                false,
                &["transfer", "sync", "attributes"][..],
            ),
            (
                Ordinary,
                false,
                false,
                &["transfer", "attributes", "sync"][..],
            ),
            (Standard, true, false, &["transfer"][..]),
            (Ordinary, true, false, &["transfer", "sync"][..]),
            (
                Standard,
                false,
                true,
                &["transfer", "sync", "attributes"][..],
            ),
            (
                Ordinary,
                false,
                true,
                &["transfer", "attributes", "sync"][..],
            ),
        ]
        .into_iter()
        .for_each(|(action, transfer_fails, attributes_fail, expected)| {
            let calls = std::cell::RefCell::new(Vec::new());
            let result = prepare_seed_file(
                action,
                || {
                    calls.borrow_mut().push("transfer");
                    if transfer_fails {
                        Err(std::io::Error::other("transfer failed"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("attributes");
                    if attributes_fail {
                        Err(std::io::Error::other("attributes failed"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("sync");
                    Ok(())
                },
            );
            assert_eq!(result.is_err(), transfer_fails || attributes_fail);
            assert_eq!(
                calls.borrow().as_slice(),
                expected,
                "{action:?} transfer failure={transfer_fails} attributes failure={attributes_fail}"
            );
        });
    }

    #[test]
    fn seed_file_sync_failure_stops_standard_attributes_and_keeps_ordinary_attribute_errors() {
        use SeedFileSyncAction::{Ordinary, Standard};
        [
            (Standard, false, &["transfer", "sync"][..], "sync failed"),
            (
                Ordinary,
                false,
                &["transfer", "attributes", "sync"][..],
                "sync failed",
            ),
            (
                Ordinary,
                true,
                &["transfer", "attributes", "sync"][..],
                "attributes failed",
            ),
        ]
        .into_iter()
        .for_each(
            |(action, attributes_fail, expected_calls, expected_error)| {
                let calls = std::cell::RefCell::new(Vec::new());
                let result = prepare_seed_file(
                    action,
                    || {
                        calls.borrow_mut().push("transfer");
                        Ok(())
                    },
                    || {
                        calls.borrow_mut().push("attributes");
                        if attributes_fail {
                            Err(std::io::Error::other("attributes failed"))
                        } else {
                            Ok(())
                        }
                    },
                    || {
                        calls.borrow_mut().push("sync");
                        Err(std::io::Error::other("sync failed"))
                    },
                );
                assert_eq!(calls.borrow().as_slice(), expected_calls, "{action:?}");
                assert_eq!(
                    result.unwrap_err().to_string(),
                    expected_error,
                    "{action:?}"
                );
            },
        );
    }

    #[cfg(unix)]
    #[test]
    #[test_r::tag(native_apfs)]
    fn ordinary_seed_file_sync_executes_the_selected_native_action() {
        use std::io::Write as _;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"ordinary seed synchronization").unwrap();
        sync_seed_file(SeedFileSyncAction::Ordinary, &file).unwrap();
        let capability_file = cap_std::fs::File::from_std(file.try_clone().unwrap());
        sync_ordinary_descriptor(&capability_file).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 29);
        assert_eq!(capability_file.metadata().unwrap().len(), 29);
    }

    #[cfg(not(unix))]
    #[test]
    fn ordinary_file_sync_is_unavailable_without_a_unix_descriptor() {
        let file = tempfile::tempfile().unwrap();
        assert_eq!(
            sync_ordinary_descriptor(&file).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
        let capability_file = cap_std::fs::File::from_std(file);
        assert_eq!(
            sync_ordinary_descriptor(&capability_file)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_seed_batch_actions_return_errors_without_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let root = NativeRoot::new(
            directory.path().to_path_buf(),
            File::open(directory.path()).unwrap(),
        )
        .unwrap();
        let admitted = root.directory.lock().unwrap().as_ref().unwrap().clone();
        let unavailable = SeedSyncPolicy::new(SeedSyncPlatform::Other, FileTransfer::Reflink);
        assert!(!unavailable.requires_batch_root());
        let error = sync_seed_batch(unavailable, None, Path::new("\0")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        #[cfg(target_os = "macos")]
        let wrong_platform = SeedSyncPolicy::new(SeedSyncPlatform::Linux, FileTransfer::Reflink);
        #[cfg(not(target_os = "macos"))]
        let wrong_platform = SeedSyncPolicy::new(SeedSyncPlatform::MacOs, FileTransfer::Bytes);
        let result = sync_seed_batch(wrong_platform, Some(&admitted), directory.path());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn seed_batch_requires_an_admitted_root_only_for_macos_fullflush() {
        let no_sync = SeedSyncPolicy::new(SeedSyncPlatform::Other, FileTransfer::Bytes);
        assert!(!no_sync.requires_batch_root());
        sync_seed_batch(no_sync, None, Path::new("\0")).unwrap();
        let full_flush = SeedSyncPolicy::new(SeedSyncPlatform::MacOs, FileTransfer::Bytes);
        assert!(full_flush.requires_batch_root());
        let error = sync_seed_batch(full_flush, None, Path::new("\0")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_volume_seed_batch_uses_the_root_path_without_root_admission() {
        let directory = tempfile::tempdir().unwrap();
        let policy = SeedSyncPolicy::new(SeedSyncPlatform::Linux, FileTransfer::Reflink);
        assert!(!policy.requires_batch_root());
        sync_seed_batch(policy, None, directory.path()).unwrap();
        let error = sync_seed_batch(policy, None, &directory.path().join("missing")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn seed_transfer_actions_convert_only_captured_macos_reflinks_to_clones() {
        use FileTransfer::{Bytes, Clone, Reflink, ReflinkIntoProject};
        use SeedSyncPlatform::{Linux, MacOs, Other};
        let project = ReflinkIntoProject(NonZeroU32::new(7).unwrap());
        [
            (MacOs, Bytes, Bytes),
            (MacOs, Clone, Clone),
            (MacOs, Reflink, Clone),
            (MacOs, project, project),
            (Linux, Bytes, Bytes),
            (Linux, Clone, Clone),
            (Linux, Reflink, Reflink),
            (Linux, project, project),
            (Other, Bytes, Bytes),
            (Other, Clone, Clone),
            (Other, Reflink, Reflink),
            (Other, project, project),
        ]
        .into_iter()
        .for_each(|(platform, transfer, expected)| {
            assert_eq!(
                SeedSyncPolicy::new(platform, transfer).transfer,
                expected,
                "{platform:?} {transfer:?}"
            );
            assert_eq!(
                file_transfer_action(platform, transfer),
                expected,
                "{platform:?} {transfer:?}"
            );
        });
    }

    #[test]
    fn seed_file_sync_actions_use_ordinary_fsync_on_macos_and_standard_sync_elsewhere() {
        use SeedFileSyncAction::{Ordinary, Standard};
        use SeedSyncPlatform::{Linux, MacOs, Other};
        let project = FileTransfer::ReflinkIntoProject(NonZeroU32::new(7).unwrap());
        [
            (MacOs, FileTransfer::Bytes, Ordinary),
            (MacOs, FileTransfer::Clone, Ordinary),
            (MacOs, FileTransfer::Reflink, Ordinary),
            (MacOs, project, Standard),
            (Linux, FileTransfer::Bytes, Standard),
            (Linux, FileTransfer::Clone, Standard),
            (Linux, FileTransfer::Reflink, Standard),
            (Linux, project, Standard),
            (Other, FileTransfer::Bytes, Standard),
            (Other, FileTransfer::Clone, Standard),
            (Other, FileTransfer::Reflink, Standard),
            (Other, project, Standard),
        ]
        .into_iter()
        .for_each(|(platform, transfer, expected)| {
            assert_eq!(
                SeedSyncPolicy::new(platform, transfer).file,
                expected,
                "{platform:?} {transfer:?}"
            );
        });
    }

    #[test]
    fn seed_batch_actions_fullflush_macos_and_preserve_other_platform_policies() {
        use SeedBatchAction::{LinuxVolume, MacFullFlush, None as NoSync, Unavailable};
        use SeedSyncPlatform::{Linux, MacOs, Other};
        let project = FileTransfer::ReflinkIntoProject(NonZeroU32::new(7).unwrap());
        [
            (MacOs, FileTransfer::Bytes, MacFullFlush),
            (MacOs, FileTransfer::Clone, MacFullFlush),
            (MacOs, FileTransfer::Reflink, MacFullFlush),
            (MacOs, project, NoSync),
            (Linux, FileTransfer::Bytes, NoSync),
            (Linux, FileTransfer::Clone, NoSync),
            (Linux, FileTransfer::Reflink, LinuxVolume),
            (Linux, project, LinuxVolume),
            (Other, FileTransfer::Bytes, NoSync),
            (Other, FileTransfer::Clone, NoSync),
            (Other, FileTransfer::Reflink, Unavailable),
            (Other, project, Unavailable),
        ]
        .into_iter()
        .for_each(|(platform, transfer, expected)| {
            let policy = SeedSyncPolicy::new(platform, transfer);
            assert_eq!(policy.batch, expected, "{platform:?} {transfer:?}");
            assert_eq!(
                policy.requires_batch_root(),
                expected == MacFullFlush,
                "{platform:?} {transfer:?}"
            );
        });
    }

    #[test]
    fn seed_directory_actions_follow_the_platform_transfer_and_observed_stage() {
        use DirectoryPlacementObservation::{Failed, Made, Merged, NotAttempted};
        use SeedDirectoryAction::{ChildThenParent, None as NoSync, OpenedDirectory, Parent};
        use SeedDirectoryStage::{Ancestor, DirectoryAttributes, DirectoryPlacement, Entry};
        use SeedSyncPlatform::{Linux, MacOs, Other};
        let project = FileTransfer::ReflinkIntoProject(NonZeroU32::new(7).unwrap());
        let stages = [
            Entry,
            DirectoryPlacement(Made),
            DirectoryPlacement(Merged),
            DirectoryPlacement(Failed),
            Ancestor { opened: true },
            Ancestor { opened: false },
            DirectoryAttributes,
            DirectoryPlacement(NotAttempted),
        ];
        [
            (
                MacOs,
                FileTransfer::Reflink,
                [
                    Parent,
                    ChildThenParent,
                    NoSync,
                    Parent,
                    ChildThenParent,
                    Parent,
                    OpenedDirectory,
                    Parent,
                ],
            ),
            (
                MacOs,
                FileTransfer::Bytes,
                [
                    Parent,
                    ChildThenParent,
                    NoSync,
                    Parent,
                    ChildThenParent,
                    Parent,
                    OpenedDirectory,
                    Parent,
                ],
            ),
            (
                MacOs,
                FileTransfer::Clone,
                [
                    Parent,
                    ChildThenParent,
                    NoSync,
                    Parent,
                    ChildThenParent,
                    Parent,
                    OpenedDirectory,
                    Parent,
                ],
            ),
            (MacOs, project, [NoSync; 8]),
            (Linux, FileTransfer::Reflink, [NoSync; 8]),
            (Linux, FileTransfer::Clone, [NoSync; 8]),
            (Linux, project, [NoSync; 8]),
            (Linux, FileTransfer::Bytes, [NoSync; 8]),
            (Other, FileTransfer::Bytes, [NoSync; 8]),
            (Other, FileTransfer::Clone, [NoSync; 8]),
            (Other, FileTransfer::Reflink, [NoSync; 8]),
            (Other, project, [NoSync; 8]),
        ]
        .into_iter()
        .for_each(|(platform, transfer, expected)| {
            let policy = SeedSyncPolicy::new(platform, transfer);
            assert_eq!(
                stages.map(|stage| policy.directory_action(stage)),
                expected,
                "{platform:?} {transfer:?}"
            );
        });
    }

    #[test]
    fn seed_directory_finalizers_attempt_the_parent_after_a_child_failure() {
        let attempts = std::cell::RefCell::new(Vec::new());
        let result = sync_seed_directories(
            SeedDirectoryAction::ChildThenParent,
            || {
                attempts.borrow_mut().push("child");
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "child sync",
                ))
            },
            || {
                attempts.borrow_mut().push("parent");
                Err(std::io::Error::other("parent sync"))
            },
        );
        assert_eq!(*attempts.borrow(), ["child", "parent"]);
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "child sync");
    }

    #[test]
    fn seed_directory_finalizers_do_only_the_selected_directory_work() {
        [
            (SeedDirectoryAction::None, &[][..]),
            (SeedDirectoryAction::Parent, &["parent"][..]),
            (SeedDirectoryAction::OpenedDirectory, &["opened"][..]),
            (
                SeedDirectoryAction::ChildThenParent,
                &["opened", "parent"][..],
            ),
        ]
        .into_iter()
        .for_each(|(action, expected)| {
            let attempts = std::cell::RefCell::new(Vec::new());
            sync_seed_directories(
                action,
                || {
                    attempts.borrow_mut().push("opened");
                    Ok(())
                },
                || {
                    attempts.borrow_mut().push("parent");
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(*attempts.borrow(), expected, "{action:?}");
        });
    }

    #[test]
    fn operation_finalization_keeps_the_first_error_and_runs_cleanup_before_sync() {
        ["transfer", "attributes", "file flush", "placement"]
            .into_iter()
            .for_each(|stage| {
                let attempts = std::cell::RefCell::new(Vec::new());
                let seeded = Err::<(), _>(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    stage,
                ));
                let cleaned = finish_operation(seeded, || {
                    attempts.borrow_mut().push("cleanup");
                    Err(std::io::Error::other("cleanup"))
                });
                let result = finish_operation(cleaned, || {
                    attempts.borrow_mut().push("parent sync");
                    Err(std::io::Error::other("parent sync"))
                });
                assert_eq!(*attempts.borrow(), ["cleanup", "parent sync"]);
                let error = result.unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), stage);
            });
        let result = finish_operation(Ok(17), || Err(std::io::Error::other("cleanup")));
        assert_eq!(result.unwrap_err().to_string(), "cleanup");
        let result = finish_operation(Ok(17), || Err(std::io::Error::other("sync")));
        assert_eq!(result.unwrap_err().to_string(), "sync");
        assert_eq!(
            finish_operation::<_, std::io::Error>(Ok(17), || Ok(())).unwrap(),
            17
        );
    }

    #[test]
    fn seed_batch_finalization_runs_after_failure_and_keeps_the_seed_error() {
        let synced = std::cell::Cell::new(false);
        let result = finish_operation(
            Err::<(), _>(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "seed",
            )),
            || {
                synced.set(true);
                Err(std::io::Error::other("volume sync"))
            },
        );
        assert!(synced.get());
        assert_eq!(result.unwrap_err().to_string(), "seed");
    }

    #[test]
    fn batch_submissions_attempt_every_stage_and_keep_the_earliest_error() {
        [
            "entry",
            "file submission",
            "root submission",
            "full flush",
            "success",
        ]
        .into_iter()
        .for_each(|failure| {
            let attempts = std::cell::RefCell::new(Vec::new());
            let stage = |name| {
                attempts.borrow_mut().push(name);
                if name == failure {
                    Err(std::io::Error::other(name))
                } else {
                    Ok(())
                }
            };
            let result = finalize_operation(
                || stage("entry"),
                || {
                    sync_seed_batch_submissions(
                        || stage("file submission"),
                        || stage("root submission"),
                        || stage("full flush"),
                    )
                },
            );
            assert_eq!(
                *attempts.borrow(),
                ["entry", "file submission", "root submission", "full flush"]
            );
            if failure == "success" {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().to_string(), failure);
            }
        });
        let result = finalize_operation(
            || Err::<(), _>(std::io::Error::other("entry")),
            || {
                sync_seed_batch_submissions(
                    || Err(std::io::Error::other("file submission")),
                    || Err(std::io::Error::other("root submission")),
                    || Err(std::io::Error::other("full flush")),
                )
            },
        );
        assert_eq!(result.unwrap_err().to_string(), "entry");
    }

    #[test]
    fn an_earlier_operation_error_survives_a_direct_finalizer_panic() {
        let attempts = std::cell::RefCell::new(Vec::new());
        let finalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finalize_operation(
                || {
                    finalize_operation(
                        || {
                            finish_operation(
                                Err::<(), _>(std::io::Error::other("earlier operation error")),
                                || {
                                    attempts.borrow_mut().push("cleanup");
                                    panic!("later cleanup panic")
                                },
                            )
                        },
                        || {
                            attempts.borrow_mut().push("parent sync");
                            Ok(())
                        },
                    )
                },
                || {
                    sync_seed_batch_submissions(
                        || {
                            attempts.borrow_mut().push("file submission");
                            Ok(())
                        },
                        || {
                            attempts.borrow_mut().push("root submission");
                            Ok(())
                        },
                        || {
                            attempts.borrow_mut().push("full flush");
                            Ok(())
                        },
                    )
                },
            )
        }));
        assert_eq!(
            *attempts.borrow(),
            [
                "cleanup",
                "parent sync",
                "file submission",
                "root submission",
                "full flush"
            ]
        );
        assert!(
            finalized.is_ok(),
            "a later finalizer panic must not replace the earlier ordinary error"
        );
        assert_eq!(
            finalized.unwrap().unwrap_err().to_string(),
            "earlier operation error"
        );
    }

    #[test]
    fn an_earlier_error_survives_later_panics_and_all_outer_finalizers_run() {
        let attempts = std::cell::RefCell::new(Vec::new());
        let result = finalize_operation(
            || {
                finalize_operation(
                    || {
                        finalize_operation(
                            || {
                                attempts.borrow_mut().push("operation");
                                Err::<(), _>(std::io::Error::other("earlier operation error"))
                            },
                            || {
                                attempts.borrow_mut().push("cleanup");
                                panic!("later cleanup panic")
                            },
                        )
                    },
                    || {
                        attempts.borrow_mut().push("parent sync");
                        Err(std::io::Error::other("parent sync"))
                    },
                )
            },
            || {
                sync_seed_batch_submissions(
                    || {
                        attempts.borrow_mut().push("file submission");
                        Err(std::io::Error::other("file submission"))
                    },
                    || {
                        attempts.borrow_mut().push("root submission");
                        panic!("later root submission panic")
                    },
                    || {
                        attempts.borrow_mut().push("full flush");
                        panic!("later full flush panic")
                    },
                )
            },
        );
        assert_eq!(result.unwrap_err().to_string(), "earlier operation error");
        assert_eq!(
            *attempts.borrow(),
            [
                "operation",
                "cleanup",
                "parent sync",
                "file submission",
                "root submission",
                "full flush"
            ]
        );
    }

    #[test]
    fn symlink_cleanup_actions_follow_observed_placement() {
        assert_eq!(
            seed_symlink_cleanup_action(false),
            SeedSymlinkCleanupAction::RemoveTemporary
        );
        assert_eq!(
            seed_symlink_cleanup_action(true),
            SeedSymlinkCleanupAction::None
        );
    }

    #[test]
    fn unwind_runs_local_and_batch_finalizers_before_resuming_the_original_panic() {
        let attempts = std::cell::RefCell::new(Vec::new());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finalize_operation(
                || {
                    finalize_operation(
                        || {
                            finalize_operation(
                                || {
                                    finalize_operation(
                                        || -> std::io::Result<()> {
                                            attempts.borrow_mut().push("transfer");
                                            panic!("original transfer panic")
                                        },
                                        || {
                                            attempts.borrow_mut().push("file sync");
                                            Ok(())
                                        },
                                    )
                                },
                                || {
                                    attempts.borrow_mut().push("cleanup");
                                    panic!("cleanup panic")
                                },
                            )
                        },
                        || {
                            attempts.borrow_mut().push("parent sync");
                            Err(std::io::Error::other("parent sync"))
                        },
                    )
                },
                || {
                    sync_seed_batch_submissions(
                        || {
                            attempts.borrow_mut().push("file submission");
                            panic!("submission panic")
                        },
                        || {
                            attempts.borrow_mut().push("root submission");
                            Ok(())
                        },
                        || {
                            attempts.borrow_mut().push("full flush");
                            Ok(())
                        },
                    )
                },
            )
        }))
        .unwrap_err();
        assert_eq!(
            panicked.downcast_ref::<&str>(),
            Some(&"original transfer panic")
        );
        assert_eq!(
            *attempts.borrow(),
            [
                "transfer",
                "file sync",
                "cleanup",
                "parent sync",
                "file submission",
                "root submission",
                "full flush"
            ]
        );
    }

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

    fn buffered_seed(placement: SeedPlacement) -> SeedContext {
        SeedContext {
            policy: SeedSyncPolicy::new(native_seed_sync_platform(), FileTransfer::Bytes),
            access: SeedAccess::FromSource,
            placement,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[test_r::tag(native_apfs)]
    fn a_supplied_linux_reflink_policy_does_not_execute_a_macos_clone() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let file = source.path().join("file").into_boxed_path();
        std::fs::write(&file, b"selected transfer").unwrap();
        let directory = open(target.path());
        let result = seed_entry(
            SeedContext {
                policy: SeedSyncPolicy::new(SeedSyncPlatform::Linux, FileTransfer::Reflink),
                access: SeedAccess::FromSource,
                placement: SeedPlacement::CreateNew,
            },
            &directory,
            &file,
            Path::new("file"),
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
        assert!(!target.path().join("file").exists());
        assert_eq!(directory.entries().unwrap().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[test_r::tag(native_apfs)]
    fn a_supplied_linux_capture_reflink_action_does_not_execute_a_macos_clone() {
        let source = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut &source, b"selected capture transfer").unwrap();
        let destination = tempfile::tempdir().unwrap();
        let target = destination.path().join("file").into_boxed_path();
        let transfer = file_transfer_action(SeedSyncPlatform::Linux, FileTransfer::Reflink);
        assert_eq!(transfer, FileTransfer::Reflink);
        let error = copy_new_file(transfer, &source, &target).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert_eq!(std::fs::metadata(&target).unwrap().len(), 0);
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
    }

    /// The rules of the `SeedPlacement` and `SeedEntry` docs, written as a table: the source, what
    /// is at the target, the action with `CreateNew`, and the action with `Replace`.
    const PLACEMENT_TABLE: [(&str, Occupant, PlacementAction, PlacementAction); 6] = [
        (
            "directory",
            Occupant::Directory,
            PlacementAction::Merge,
            PlacementAction::Merge,
        ),
        (
            "directory",
            Occupant::Other,
            PlacementAction::Refuse,
            PlacementAction::Overwrite,
        ),
        (
            "file",
            Occupant::Directory,
            PlacementAction::Refuse,
            PlacementAction::RemoveDirectory,
        ),
        (
            "file",
            Occupant::Other,
            PlacementAction::Refuse,
            PlacementAction::Overwrite,
        ),
        (
            "symlink",
            Occupant::Directory,
            PlacementAction::Refuse,
            PlacementAction::RemoveDirectory,
        ),
        (
            "symlink",
            Occupant::Other,
            PlacementAction::Refuse,
            PlacementAction::Overwrite,
        ),
    ];

    fn source_kind(name: &str) -> TreeEntryKind {
        match name {
            "directory" => TreeEntryKind::Directory,
            "file" => TreeEntryKind::File,
            _ => TreeEntryKind::Symlink(Box::from(Path::new("target"))),
        }
    }

    #[test]
    fn placement_action_follows_the_seed_placement_table() {
        PLACEMENT_TABLE
            .into_iter()
            .for_each(|(source, occupant, create_new, replace)| {
                [
                    (SeedPlacement::CreateNew, create_new),
                    (SeedPlacement::Replace, replace),
                ]
                .into_iter()
                .for_each(|(placement, expected)| {
                    assert_eq!(
                        placement_action(&source_kind(source), occupant, placement),
                        expected,
                        "a {source} source on {occupant:?} with {placement:?}"
                    );
                });
            });
    }

    fn listed(relative: &str, link: Option<&str>) -> TreeEntry {
        TreeEntry {
            relative: Box::from(Path::new(relative)),
            kind: TreeEntryKind::File,
            permissions: cap_std::fs::Permissions::from_std(
                std::fs::metadata(std::env::temp_dir())
                    .unwrap()
                    .permissions(),
            ),
            modified: None,
            link: link.map(|identity| NativeFileIdentity::Scripted(identity.to_string())),
        }
    }

    /// The names that a copy with `plan` makes, in the order of the listing.
    fn copied_names<'a>(plan: &LinkPlan, entries: &'a [TreeEntry]) -> Vec<&'a str> {
        entries
            .iter()
            .enumerate()
            .filter(|(position, _)| plan.copies(*position))
            .map(|(_, entry)| entry.relative.to_str().unwrap())
            .collect()
    }

    fn group(first: &str, others: &[&str]) -> LinkGroup {
        LinkGroup {
            first: Box::from(Path::new(first)),
            others: others
                .iter()
                .map(|other| Box::from(Path::new(other)))
                .collect(),
        }
    }

    #[test]
    fn link_plan_copies_each_object_once_at_its_first_name_and_groups_the_others() {
        let entries = [
            listed("a", None),
            listed("b1", Some("b")),
            listed("c1", Some("c")),
            listed("b2", Some("b")),
            listed("d", Some("d")),
            listed("e", None),
            listed("c2", Some("c")),
            listed("b3", Some("b")),
        ];

        let plan = link_plan(&entries);

        assert_eq!(copied_names(&plan, &entries), ["a", "b1", "c1", "d", "e"]);
        assert_eq!(plan.later, Box::from([3, 6, 7]));
        assert_eq!(
            plan.groups,
            Box::from([group("b1", &["b2", "b3"]), group("c1", &["c2"])])
        );
    }

    #[test]
    fn link_plan_of_a_listing_without_links_copies_everything_and_gives_no_group() {
        let entries = [listed("a", None), listed("b", None)];

        let plan = link_plan(&entries);

        assert_eq!(copied_names(&plan, &entries), ["a", "b"]);
        assert!(plan.later.is_empty());
        assert!(plan.groups.is_empty());
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
        let error = copy_contents(
            &open(source.path()),
            Path::new(""),
            &TreeExclusions::default(),
            destination.path(),
            FileTransfer::Bytes,
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn copy_contents_copies_everything_except_the_exclusions() {
        let source = tempfile::tempdir().unwrap();
        fixture_tree(source.path());
        std::fs::set_permissions(source.path(), std::fs::Permissions::from_mode(0o750)).unwrap();
        let root_modified = UNIX_EPOCH + Duration::from_secs(1_700_000_006);
        File::open(source.path())
            .unwrap()
            .set_modified(root_modified)
            .unwrap();
        let destination = tempfile::tempdir().unwrap();

        copy_contents(
            &open(source.path()),
            Path::new(""),
            &exclusions(&["static/asset.bin", "data/nested"]),
            destination.path(),
            FileTransfer::Bytes,
        )
        .unwrap();

        let root = std::fs::metadata(destination.path()).unwrap();
        assert_eq!(root.permissions().mode() & 0o777, 0o750);
        assert_eq!(root.modified().unwrap(), root_modified);
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

    #[cfg(unix)]
    #[test]
    fn seed_recreates_the_tree_through_the_capability() {
        let source = tempfile::tempdir().unwrap();
        fixture_tree(source.path());
        let destination = tempfile::tempdir().unwrap();

        seed_entry(
            buffered_seed(SeedPlacement::CreateNew),
            &open(destination.path()),
            source.path(),
            Path::new(""),
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

    #[cfg(unix)]
    #[test]
    fn seed_replaces_a_file_with_a_directory_that_has_the_source_attributes() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("data")).unwrap();
        std::fs::write(source.path().join("data/file"), b"new").unwrap();
        std::fs::set_permissions(
            source.path().join("data"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        File::open(source.path().join("data"))
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::fs::write(destination.path().join("data"), b"old file").unwrap();

        seed_entry(
            buffered_seed(SeedPlacement::Replace),
            &open(destination.path()),
            source.path(),
            Path::new(""),
        )
        .unwrap();

        let made = std::fs::metadata(destination.path().join("data")).unwrap();
        assert!(made.is_dir(), "the file must give way to the directory");
        assert_eq!(
            made.permissions().mode() & 0o777,
            0o750,
            "the directory that takes the place of a file must have the mode of its source"
        );
        assert_eq!(
            made.modified().unwrap(),
            modified,
            "the directory that takes the place of a file must have the time of its source"
        );
        assert_eq!(
            std::fs::read(destination.path().join("data/file")).unwrap(),
            b"new"
        );
    }

    #[cfg(unix)]
    #[test]
    fn seed_files_preserve_attributes_and_cleanup_after_placement_failure() {
        [
            FileTransfer::Bytes,
            #[cfg(target_os = "macos")]
            FileTransfer::Reflink,
        ]
        .into_iter()
        .for_each(|transfer| {
            let source = tempfile::tempdir().unwrap();
            let file = source.path().join("file").into_boxed_path();
            std::fs::write(&file, b"seeded file").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
            let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_007);
            File::open(&file).unwrap().set_modified(modified).unwrap();
            let destination = tempfile::tempdir().unwrap();
            let base = open(destination.path());
            let context = SeedContext {
                policy: SeedSyncPolicy::new(native_seed_sync_platform(), transfer),
                access: SeedAccess::ReadOnly,
                placement: SeedPlacement::CreateNew,
            };

            seed_entry(context, &base, &file, Path::new("ancestor/file")).unwrap();

            let target = destination.path().join("ancestor/file").into_boxed_path();
            let attributes = std::fs::metadata(&target).unwrap();
            assert_eq!(attributes.permissions().mode() & 0o777, 0o440);
            assert_eq!(attributes.modified().unwrap(), modified);
            [
                ("ancestor/file", std::io::ErrorKind::AlreadyExists),
                ("ancestor/\0", std::io::ErrorKind::InvalidInput),
            ]
            .into_iter()
            .for_each(|(path, expected)| {
                let error = seed_entry(context, &base, &file, Path::new(path)).unwrap_err();
                assert_eq!(error.kind(), expected, "{transfer:?}: {error}");
                assert_eq!(std::fs::read(&target).unwrap(), b"seeded file");
                assert_eq!(
                    sorted_names(&base.open_dir_nofollow("ancestor").unwrap()).unwrap(),
                    [OsString::from("file")],
                    "a failed placement must remove its temporary file"
                );
            });
        });
    }

    #[cfg(unix)]
    #[test]
    fn seed_symlinks_preserve_times_and_cleanup_after_placement_failure() {
        [
            FileTransfer::Bytes,
            #[cfg(target_os = "macos")]
            FileTransfer::Reflink,
        ]
        .into_iter()
        .for_each(|transfer| {
            let source = tempfile::tempdir().unwrap();
            let link = source.path().join("link").into_boxed_path();
            std::os::unix::fs::symlink("missing-target", &link).unwrap();
            let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_008);
            open(source.path())
                .set_symlink_times("link", None, Some(capability_time(modified)))
                .unwrap();
            let destination = tempfile::tempdir().unwrap();
            let base = open(destination.path());
            let context = SeedContext {
                policy: SeedSyncPolicy::new(native_seed_sync_platform(), transfer),
                access: SeedAccess::FromSource,
                placement: SeedPlacement::CreateNew,
            };

            seed_entry(context, &base, &link, Path::new("ancestor/link")).unwrap();
            let error = seed_entry(context, &base, &link, Path::new("ancestor/link")).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            let replacing = SeedContext {
                placement: SeedPlacement::Replace,
                ..context
            };
            let error = seed_entry(replacing, &base, &link, Path::new("ancestor/\0")).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            seed_entry(replacing, &base, &link, Path::new("ancestor/link")).unwrap();

            let target = destination.path().join("ancestor/link").into_boxed_path();
            assert_eq!(
                std::fs::read_link(&target).unwrap(),
                Path::new("missing-target")
            );
            assert_eq!(
                std::fs::symlink_metadata(&target)
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
            assert_eq!(
                sorted_names(&base.open_dir_nofollow("ancestor").unwrap()).unwrap(),
                [OsString::from("link")],
                "placement and error cleanup must leave no temporary symlink"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn seed_merges_an_empty_directory_without_opening_it_for_sync() {
        [
            FileTransfer::Bytes,
            #[cfg(target_os = "macos")]
            FileTransfer::Reflink,
        ]
        .into_iter()
        .for_each(|transfer| {
            let source = tempfile::tempdir().unwrap();
            let destination = tempfile::tempdir().unwrap();
            let base = open(destination.path());
            base.create_dir("merged").unwrap();
            base.set_permissions(
                "merged",
                cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o0)),
            )
            .unwrap();
            let mut entry = listed("merged", None);
            entry.kind = TreeEntryKind::Directory;
            let context = SeedContext {
                policy: SeedSyncPolicy::new(native_seed_sync_platform(), transfer),
                access: SeedAccess::FromSource,
                placement: SeedPlacement::CreateNew,
            };

            let result =
                seed_listed_entry(context, &open(source.path()), &base, &entry, Vec::new());
            std::fs::set_permissions(
                destination.path().join("merged"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();

            assert!(result.unwrap().is_empty(), "a merged directory is not made");
        });
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[test_r::tag(native_apfs)]
    fn macos_seed_syncs_directory_attributes_through_restrictive_permissions() {
        [FileTransfer::Bytes, FileTransfer::Reflink]
            .into_iter()
            .for_each(|transfer| {
                let destination = tempfile::tempdir().unwrap();
                let base = open(destination.path());
                base.create_dir("made").unwrap();
                let mut entry = listed("made", None);
                entry.kind = TreeEntryKind::Directory;
                entry.permissions =
                    cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o0));
                let modified = UNIX_EPOCH + Duration::from_secs(1_234_567);
                entry.modified = Some(modified);
                let result = set_seeded_directory_attributes(
                    SeedSyncPolicy::new(SeedSyncPlatform::MacOs, transfer),
                    &base,
                    Path::new("made"),
                    &entry,
                );
                let mode = base.symlink_metadata("made").unwrap().permissions().mode() & 0o777;
                std::fs::set_permissions(
                    destination.path().join("made"),
                    std::fs::Permissions::from_mode(0o700),
                )
                .unwrap();

                assert!(
                    result.is_ok(),
                    "sync must use the open directory: {result:?}"
                );
                assert_eq!(mode, 0);
                assert_eq!(
                    base.symlink_metadata("made")
                        .unwrap()
                        .modified()
                        .unwrap()
                        .into_std(),
                    modified
                );
            });
    }

    #[test]
    fn seed_fails_on_an_existing_target_path() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("data")).unwrap();
        std::fs::write(source.path().join("data/file"), b"new").unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::fs::create_dir(destination.path().join("data")).unwrap();
        std::fs::write(destination.path().join("data/file"), b"old").unwrap();

        let error = seed_entry(
            buffered_seed(SeedPlacement::CreateNew),
            &open(destination.path()),
            source.path(),
            Path::new(""),
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(destination.path().join("data/file")).unwrap(),
            b"old"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_contents_copies_a_symlink_with_several_names_once_and_reports_its_names() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("target"), b"target").unwrap();
        std::os::unix::fs::symlink("target", source.path().join("link")).unwrap();
        std::fs::hard_link(source.path().join("link"), source.path().join("link-name")).unwrap();
        let destination = tempfile::tempdir().unwrap();

        let groups = copy_contents(
            &open(source.path()),
            Path::new(""),
            &exclusions(&[]),
            destination.path(),
            FileTransfer::Bytes,
        )
        .unwrap();

        assert_eq!(
            groups.as_ref(),
            [LinkGroup {
                first: Path::new("link").into(),
                others: Box::new([Box::from(Path::new("link-name"))]),
            }]
        );
        assert_eq!(
            tree_listing(destination.path()),
            ["link", "target"].map(String::from).into()
        );
        assert_eq!(
            std::fs::read_link(destination.path().join("link")).unwrap(),
            Path::new("target")
        );
    }

    #[test]
    fn copy_contents_copies_a_file_with_several_names_once_and_reports_its_names() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("a")).unwrap();
        std::fs::create_dir(source.path().join("b")).unwrap();
        std::fs::write(source.path().join("a/file"), b"linked").unwrap();
        std::fs::hard_link(source.path().join("a/file"), source.path().join("b/second")).unwrap();
        std::fs::hard_link(source.path().join("a/file"), source.path().join("third")).unwrap();
        std::fs::write(source.path().join("single"), b"single").unwrap();
        std::fs::write(source.path().join("excluded"), b"excluded").unwrap();
        std::fs::hard_link(source.path().join("excluded"), source.path().join("kept")).unwrap();
        let destination = tempfile::tempdir().unwrap();

        let groups = copy_contents(
            &open(source.path()),
            Path::new(""),
            &exclusions(&["excluded"]),
            destination.path(),
            FileTransfer::Bytes,
        )
        .unwrap();

        assert_eq!(
            groups.as_ref(),
            [LinkGroup {
                first: Path::new("a/file").into(),
                others: [Path::new("b/second"), Path::new("third")]
                    .map(Box::<Path>::from)
                    .into(),
            }]
        );
        assert_eq!(
            tree_listing(destination.path()),
            ["a", "a/file", "b", "kept", "single"]
                .map(String::from)
                .into()
        );
        assert_eq!(
            std::fs::read(destination.path().join("a/file")).unwrap(),
            b"linked"
        );
        assert_eq!(
            std::fs::read(destination.path().join("kept")).unwrap(),
            b"excluded"
        );
    }
}
