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
    seed_in_parent(context, directory, destination, |parent, name| {
        seed_file_resolved(context, source_directory, source, parent, name)
    })
}

/// Makes the missing parent directories of `destination` under `directory`, and runs `seed` with
/// the parent and the name of `destination`. Then, also when `seed` fails, it syncs the parent as
/// the policy of `context` sets for a seed entry.
fn seed_in_parent(
    context: SeedContext,
    directory: &cap_std::fs::Dir,
    destination: &Path,
    seed: impl FnOnce(&cap_std::fs::Dir, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let (parent, name) = create_capability_copy_parent(directory, destination, context.policy)?;
    finalize_operation(
        || seed(parent.as_dir(), &name),
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
        let file = temporary.as_file();
        let transfer = || {
            let target = file.try_clone()?.into_std();
            transfer_file(context.policy.transfer, &source_file, &target)?;
            Ok(target)
        };
        let attributes = |target: &File| {
            target.set_permissions(seeded_permissions(
                host_permissions(&source.permissions, target)?,
                context.access,
            ))?;
            if let Some(modified) = source.modified {
                target.set_modified(modified)?;
            }
            Ok(())
        };
        prepare_seed_file(action, transfer, attributes, || {
            sync_seed_file(action, file)
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
    seed_in_parent(context, directory, destination, |parent, name| {
        seed_symlink_resolved(context, parent, name, link_target, modified)
    })
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

/// Whether a sandbox root on `platform` keeps a seed_flusher, the unlinked regular file that the
/// full flush of a seed batch goes through.
pub(super) fn root_keeps_seed_flusher(platform: SeedSyncPlatform) -> bool {
    platform == SeedSyncPlatform::MacOs
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

pub(super) fn platform_file_transfer(
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

#[derive(Clone, Copy)]
pub(super) struct SeedSyncPolicy {
    transfer: FileTransfer,
    file: SeedFileSyncAction,
    batch: SeedBatchAction,
}

impl SeedSyncPolicy {
    /// The policy of a seed on `platform`. `transfer` is what [`platform_file_transfer`] gives for
    /// that platform.
    pub(super) fn new(platform: SeedSyncPlatform, transfer: FileTransfer) -> Self {
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

fn sync_seed_file(action: SeedFileSyncAction, file: &cap_std::fs::File) -> std::io::Result<()> {
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

/// Runs the steps of one seeded file in the order that `action` sets. `transfer` gives the handle
/// that `attributes` changes, so a failure to make that handle is a transfer failure.
///
/// `Standard` runs transfer, sync, attributes, and a failed step stops the steps after it.
/// `Ordinary` runs the sync after the transfer and the attributes, also when one of them fails, and
/// keeps the first error.
fn prepare_seed_file<T>(
    action: SeedFileSyncAction,
    transfer: impl FnOnce() -> std::io::Result<T>,
    attributes: impl FnOnce(&T) -> std::io::Result<()>,
    sync: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    match action {
        SeedFileSyncAction::Ordinary => finalize_operation(|| attributes(&transfer()?), sync),
        SeedFileSyncAction::Standard => {
            let target = transfer()?;
            sync()?;
            attributes(&target)
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
            let (root, seed_flusher) = admitted
                .and_then(|root| Some((root, root.seed_flusher.as_deref()?)))
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "macOS seed batch root was not admitted with a seed_flusher",
                    )
                })?;
            sync_seed_mac_full_flush(&root.directory, seed_flusher)
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
fn sync_seed_mac_full_flush(
    directory: &cap_std::fs::Dir,
    seed_flusher: &cap_std::fs::File,
) -> std::io::Result<()> {
    sync_seed_batch_submissions(
        || sync_ordinary_descriptor(seed_flusher),
        || sync_ordinary_descriptor(directory),
        || rustix::fs::fcntl_fullfsync(seed_flusher).map_err(std::io::Error::from),
    )
}

#[cfg(not(target_os = "macos"))]
fn sync_seed_mac_full_flush(
    _directory: &cap_std::fs::Dir,
    _seed_flusher: &cap_std::fs::File,
) -> std::io::Result<()> {
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
mod tests;
