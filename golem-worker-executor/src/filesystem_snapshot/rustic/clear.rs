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

//! The clear of the directory of a restore before a new run of the restore.
//!
//! The clear opens `into` by its path, and below `into` it never follows a symbolic link and never
//! touches a path outside `into`. It walks with descriptors: each directory below `into` is opened
//! relative to the descriptor of the directory above it, never by a path, and never through a
//! link. It reads the kind of each entry without following a link (`statat` with
//! `AT_SYMLINK_NOFOLLOW`). It removes a link, a file or any entry that is not a directory with
//! `unlinkat`, which removes the link itself. Before it lists a real directory below `into`, it
//! gives the directory the mode `0o700`. It removes a directory with `unlinkat(.., AT_REMOVEDIR)`
//! after its entries.
//!
//! On Linux, the clear opens each directory with `O_PATH | O_DIRECTORY | O_NOFOLLOW`, which needs
//! no permission on the directory and cannot open a link. It changes the mode through
//! `/proc/self/fd/N`, which names the very inode that the descriptor holds, and opens that inode
//! for reading through the same name. So a directory at mode `0000` clears, and a swap of the
//! entry after the open cannot redirect the change. On another unix system, the clear opens each
//! directory with `O_DIRECTORY | O_NOFOLLOW` and changes the mode with `fchmod`, so a directory at
//! mode `0000` gives `EACCES`. The clear does not enter a directory of another device than `into`,
//! and a tree deeper than [`MOST_CLEAR_DEPTH`] levels makes it fail.

/// The deepest level of directories that the clear enters. The clear holds one descriptor for
/// each level.
pub(super) const MOST_CLEAR_DEPTH: usize = 256;

/// What the clear does with one entry, from its kind as `statat` gives it without following a
/// link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClearEntry {
    /// Removes the entry with `unlinkat`: a link, a file, or any entry that is not a directory.
    Unlink,
    /// Gives the directory the mode `0o700`, clears it, and removes it.
    ChmodThenEnter,
    /// Fails the clear: the directory is on another device than `into`.
    OtherDevice,
}

/// Gives what the clear does with an entry whose kind is a directory when `directory`, and whose
/// device is the device of `into` when `same_device`.
pub(super) fn clear_entry(directory: bool, same_device: bool) -> ClearEntry {
    match (directory, same_device) {
        (false, _) => ClearEntry::Unlink,
        (true, true) => ClearEntry::ChmodThenEnter,
        (true, false) => ClearEntry::OtherDevice,
    }
}

/// The points of the walk where a test acts.
#[cfg(test)]
pub(super) trait ClearHook: Send + Sync {
    /// Runs after the clear removed an entry. An error ends the clear with that error.
    fn after_entry(&self) -> std::io::Result<()> {
        Ok(())
    }

    /// Runs after the clear read the kind of a directory entry and before it opens the entry.
    fn before_open(&self, _parent: &std::path::Path, _name: &std::ffi::OsStr) {}

    /// Runs after the clear opened a directory entry and before it changes its mode.
    fn before_mode(&self, _parent: &std::path::Path, _name: &std::ffi::OsStr) {}
}

/// No hook.
#[cfg(test)]
pub(super) struct NoHook;

#[cfg(test)]
impl ClearHook for NoHook {}

/// The hook of a walk: the hook of a test, and nothing in production code.
#[derive(Clone, Copy)]
struct Hooks<'a> {
    #[cfg(test)]
    hook: &'a dyn ClearHook,
    #[cfg(not(test))]
    _none: std::marker::PhantomData<&'a ()>,
}

impl Hooks<'_> {
    fn after_entry(&self) -> std::io::Result<()> {
        #[cfg(test)]
        {
            self.hook.after_entry()
        }
        #[cfg(not(test))]
        {
            Ok(())
        }
    }

    #[cfg_attr(not(test), allow(unused_variables))]
    fn before_open(&self, parent: &std::path::Path, name: &std::ffi::OsStr) {
        #[cfg(test)]
        self.hook.before_open(parent, name);
    }

    #[cfg_attr(not(test), allow(unused_variables))]
    fn before_mode(&self, parent: &std::path::Path, name: &std::ffi::OsStr) {
        #[cfg(test)]
        self.hook.before_mode(parent, name);
    }
}

/// Tells whether `into` has no entry.
pub(super) fn is_empty(into: &std::path::Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(into)?.next().transpose()?.is_none())
}

/// Removes every entry of `into` and keeps `into`, as the doc of this module says.
pub(super) fn clear(into: &std::path::Path) -> std::io::Result<()> {
    walk(
        into,
        Hooks {
            #[cfg(test)]
            hook: &NoHook,
            #[cfg(not(test))]
            _none: std::marker::PhantomData,
        },
    )
}

/// Removes every entry of `into` as [`clear`] does, with `hook` at the points of the walk.
#[cfg(test)]
pub(super) fn clear_with(into: &std::path::Path, hook: &dyn ClearHook) -> std::io::Result<()> {
    walk(into, Hooks { hook })
}

#[cfg(unix)]
fn walk(into: &std::path::Path, hooks: Hooks<'_>) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags};
    let root = rustix::fs::open(
        into,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let device = rustix::fs::fstat(&root)?.st_dev;
    unix::clear_tree(root, into, device, hooks)
}

/// The clear is not built for this platform, so it gives an error of the kind `Unsupported`.
#[cfg(not(unix))]
fn walk(_into: &std::path::Path, _hooks: Hooks<'_>) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the clear of the directory of a restore runs only on unix",
    ))
}

#[cfg(unix)]
mod unix {
    use super::{ClearEntry, Hooks, MOST_CLEAR_DEPTH, clear_entry};
    use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::ffi::{CString, OsStr};
    use std::ops::ControlFlow;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    /// The mode that a directory gets before the clear lists it: read, write and search for the
    /// owner, which a listing, an unlink and an entry need.
    const CLEAR_MODE: u32 = 0o700;

    /// One open directory of the walk: its descriptor, its path for the hooks and the messages,
    /// the names of its entries that the walk did not remove yet, and its own name in the
    /// directory above it.
    struct Level {
        directory: OwnedFd,
        path: PathBuf,
        names: std::vec::IntoIter<CString>,
        name: Option<CString>,
    }

    impl Level {
        /// Opens the level of `directory`. The names are read first, so the removes do not change
        /// the listing that the walk reads.
        fn new(directory: OwnedFd, path: PathBuf, name: Option<CString>) -> std::io::Result<Self> {
            let names = rustix::fs::Dir::read_from(&directory)?.try_fold(
                Vec::new(),
                |mut names, entry| {
                    let entry = entry?;
                    let name = entry.file_name();
                    if name != c"." && name != c".." {
                        names.push(CString::from(name));
                    }
                    Ok::<_, rustix::io::Errno>(names)
                },
            )?;
            Ok(Self {
                directory,
                path,
                names: names.into_iter(),
                name,
            })
        }
    }

    /// Removes each entry of the tree below `root`, which is at `path`, with a stack of open
    /// directories: one owned accumulator, at most [`MOST_CLEAR_DEPTH`] levels below `root`. The
    /// walk does not enter a directory of another device than `device`.
    pub(super) fn clear_tree(
        root: OwnedFd,
        path: &Path,
        device: u64,
        hooks: Hooks<'_>,
    ) -> std::io::Result<()> {
        let stack = vec![Level::new(root, path.to_path_buf(), None)?];
        match std::iter::repeat(()).try_fold(stack, |stack, ()| step(stack, device, hooks)) {
            ControlFlow::Break(result) => result,
            ControlFlow::Continue(_) => Ok(()),
        }
    }

    /// Takes one step of the walk: removes or enters the next entry of the deepest directory, or
    /// removes that directory when it has no entry left. Breaks with the end of the walk.
    fn step(
        mut stack: Vec<Level>,
        device: u64,
        hooks: Hooks<'_>,
    ) -> ControlFlow<std::io::Result<()>, Vec<Level>> {
        let depth = stack.len();
        let Some(level) = stack.last_mut() else {
            return ControlFlow::Break(Ok(()));
        };
        match level.names.next() {
            Some(name) => match entered(level, &name, device, depth, hooks) {
                Ok(Some(below)) => {
                    stack.push(below);
                    ControlFlow::Continue(stack)
                }
                Ok(None) => match hooks.after_entry() {
                    Ok(()) => ControlFlow::Continue(stack),
                    Err(error) => ControlFlow::Break(Err(error)),
                },
                Err(error) => ControlFlow::Break(Err(error)),
            },
            None => {
                let Some(done) = stack.pop() else {
                    return ControlFlow::Break(Ok(()));
                };
                match (done.name, stack.last()) {
                    (Some(name), Some(parent)) => {
                        let removed = rustix::fs::unlinkat(
                            parent.directory.as_fd(),
                            &name,
                            AtFlags::REMOVEDIR,
                        )
                        .map_err(std::io::Error::from)
                        .and_then(|()| hooks.after_entry());
                        match removed {
                            Ok(()) => ControlFlow::Continue(stack),
                            Err(error) => ControlFlow::Break(Err(error)),
                        }
                    }
                    _ => ControlFlow::Break(Ok(())),
                }
            }
        }
    }

    /// Removes the entry `name` of `level`, or opens it when it is a directory and gives its
    /// level. `depth` is the number of open levels.
    fn entered(
        level: &Level,
        name: &CString,
        device: u64,
        depth: usize,
        hooks: Hooks<'_>,
    ) -> std::io::Result<Option<Level>> {
        let directory = level.directory.as_fd();
        let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        let os_name = OsStr::from_bytes(name.as_bytes());
        match clear_entry(kind == FileType::Directory, stat.st_dev == device) {
            ClearEntry::Unlink => {
                rustix::fs::unlinkat(directory, name, AtFlags::empty())?;
                Ok(None)
            }
            ClearEntry::OtherDevice => Err(std::io::Error::other(format!(
                "the directory {} of the restore is on another device",
                level.path.join(os_name).display()
            ))),
            ClearEntry::ChmodThenEnter if depth > MOST_CLEAR_DEPTH => Err(std::io::Error::other(
                format!("the tree of the restore is deeper than {MOST_CLEAR_DEPTH} levels"),
            )),
            ClearEntry::ChmodThenEnter => {
                hooks.before_open(&level.path, os_name);
                let below = open_for_clear(directory, name, &level.path, os_name, hooks)?;
                Level::new(below, level.path.join(os_name), Some(name.clone())).map(Some)
            }
        }
    }

    /// Opens the directory `name` of `directory` for reading, with the mode [`CLEAR_MODE`], and
    /// never through a link.
    #[cfg(target_os = "linux")]
    fn open_for_clear(
        directory: BorrowedFd<'_>,
        name: &CString,
        path: &Path,
        os_name: &OsStr,
        hooks: Hooks<'_>,
    ) -> std::io::Result<OwnedFd> {
        let held = rustix::fs::openat(
            directory,
            name,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        hooks.before_mode(path, os_name);
        let inode = format!("/proc/self/fd/{}", rustix::fd::AsRawFd::as_raw_fd(&held));
        rustix::fs::chmod(inode.as_str(), Mode::from_raw_mode(CLEAR_MODE))?;
        let opened = rustix::fs::open(
            inode.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        drop(held);
        Ok(opened)
    }

    /// Opens the directory `name` of `directory` for reading, with the mode [`CLEAR_MODE`], and
    /// never through a link.
    #[cfg(not(target_os = "linux"))]
    fn open_for_clear(
        directory: BorrowedFd<'_>,
        name: &CString,
        path: &Path,
        os_name: &OsStr,
        hooks: Hooks<'_>,
    ) -> std::io::Result<OwnedFd> {
        let opened = rustix::fs::openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        hooks.before_mode(path, os_name);
        rustix::fs::fchmod(&opened, Mode::from_raw_mode(CLEAR_MODE))?;
        Ok(opened)
    }
}

#[cfg(test)]
mod tests;
