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

use super::SandboxFollow;
use super::name_comparison::SandboxNameComparisonKey;
use super::object_identity::SandboxDirectoryId;
use super::open_handles::{SandboxDirectory, SandboxFilePermissions};
use crate::filesystem::sandbox::FilesystemStorageError;
use std::ffi::OsString;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct SandboxResolvedPathTarget {
    pub(in crate::filesystem::sandbox) parent: SandboxDirectory,
    pub(in crate::filesystem::sandbox) name: Box<std::ffi::OsStr>,
    pub(in crate::filesystem::sandbox) name_comparison_key: SandboxNameComparisonKey,
    pub(in crate::filesystem::sandbox) parent_byte_exact_lookup_proven: bool,
    pub(in crate::filesystem::sandbox) entry: ResolvedPathEntry,
}

#[derive(Clone)]
pub(in crate::filesystem::sandbox) enum ResolvedPathEntry {
    Missing,
    File(SandboxFilePermissions),
    Directory(SandboxDirectoryId),
    Symlink {
        referent_read_only: Result<bool, Arc<std::io::Error>>,
    },
    Other,
}

/// A path resolved relative to either the filesystem root or an open directory.
///
/// This never denotes an ambient host path. Absolute paths and parent traversal cannot grant
/// access outside the selected root or directory capability.
///
/// Use [`SandboxPath::at_root`] for root-relative operations. Use [`SandboxPath::at`] when the
/// operation must remain relative to an exact open directory even if its namespace entry moves or
/// is replaced.
#[derive(Clone, Debug)]
pub(crate) struct SandboxPath {
    pub(in crate::filesystem::sandbox) base: SandboxPathBase,
    pub(in crate::filesystem::sandbox) path: PathBuf,
}

#[derive(Clone, Debug)]
pub(in crate::filesystem::sandbox) enum SandboxPathBase {
    Root,
    Directory(SandboxDirectory),
}

impl SandboxPath {
    /// Creates a path relative to the runtime filesystem root.
    pub(crate) fn at_root(path: impl Into<PathBuf>) -> Self {
        Self {
            base: SandboxPathBase::Root,
            path: path.into(),
        }
    }

    /// Creates a path relative to an already-open directory capability.
    ///
    /// Use this for descriptor-relative operations that must survive rename or path replacement.
    pub(crate) fn at(directory: SandboxDirectory, path: impl Into<PathBuf>) -> Self {
        Self {
            base: SandboxPathBase::Directory(directory),
            path: path.into(),
        }
    }

    pub(in crate::filesystem::sandbox) fn operation_path(&self, root: &Path) -> PathBuf {
        match &self.base {
            SandboxPathBase::Root => root.join(&self.path),
            SandboxPathBase::Directory(directory) => directory.path.join(&self.path),
        }
    }
}

impl SandboxResolvedPathTarget {
    /// Reports a positive proof that lookup in this parent requires the exact stored spelling.
    /// False means no such proof. This does not establish any ancestor's spelling.
    pub(crate) fn has_byte_exact_lookup_proof(&self) -> bool {
        self.parent_byte_exact_lookup_proven
    }

    /// Returns the semantic key used to coordinate access to this namespace entry.
    pub(crate) fn name_comparison_key(&self) -> SandboxNameComparisonKey {
        self.name_comparison_key.clone()
    }

    /// Returns the identity of the final directory when the resolved entry is a directory.
    pub(crate) fn target_directory_id(&self) -> Option<SandboxDirectoryId> {
        match &self.entry {
            ResolvedPathEntry::Directory(key) => Some(key.clone()),
            _ => None,
        }
    }

    /// Rebuilds a path relative to the parent capability pinned during resolution.
    ///
    /// Namespace operations should use this instead of resolving the original ambient path again.
    pub(crate) fn target(&self) -> SandboxPath {
        SandboxPath::at(self.parent.clone(), PathBuf::from(self.name.as_ref()))
    }

    /// Tells whether the resolved object is a regular file without write permission.
    ///
    /// `follow` selects the final symlink or its referent. A missing object is not a read-only
    /// file. A failure to read the referent gives that failure as a storage error.
    pub(crate) fn is_read_only_file(
        &self,
        follow: SandboxFollow,
    ) -> Result<bool, FilesystemStorageError> {
        match (&self.entry, follow) {
            (ResolvedPathEntry::File(permissions), _) => {
                Ok(*permissions == SandboxFilePermissions::ReadOnly)
            }
            (
                ResolvedPathEntry::Symlink {
                    referent_read_only: Ok(read_only),
                },
                SandboxFollow::Yes,
            ) => Ok(*read_only),
            (
                ResolvedPathEntry::Symlink {
                    referent_read_only: Err(error),
                },
                SandboxFollow::Yes,
            ) => {
                let source = error.raw_os_error().map_or_else(
                    || std::io::Error::new(error.kind(), Arc::clone(error)),
                    std::io::Error::from_raw_os_error,
                );
                Err(FilesystemStorageError::io(
                    "resolve sandbox filesystem target permissions",
                    Some(&self.parent.path.join(self.name.as_ref())),
                    source,
                ))
            }
            _ => Ok(false),
        }
    }
}

pub(in crate::filesystem::sandbox) fn split_path_target(
    path: &Path,
) -> std::io::Result<(PathBuf, OsString)> {
    if path.as_os_str().is_empty() {
        return Ok((PathBuf::new(), OsString::from(".")));
    }
    let component = path.components().next_back().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "namespace target has no final component",
        )
    })?;
    let name = match component {
        std::path::Component::Normal(name) => name.to_os_string(),
        std::path::Component::CurDir => OsString::from("."),
        std::path::Component::ParentDir => OsString::from(".."),
        std::path::Component::Prefix(_) | std::path::Component::RootDir => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "namespace target must be relative to its directory capability",
            ));
        }
    };
    Ok((
        path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        name,
    ))
}

#[cfg(test)]
mod tests;
