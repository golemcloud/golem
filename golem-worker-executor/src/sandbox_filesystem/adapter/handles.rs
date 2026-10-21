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

impl NativeAppendCoordination {
    pub(super) fn new(
        identity: NativeFileIdentity,
        coordinators: Arc<AppendCoordinatorRegistry>,
    ) -> Self {
        #[cfg(test)]
        let benchmark_eager = benchmark_eager_append_coordination()
            .then(|| coordinators.coordinator(identity.clone()));
        Self {
            identity,
            coordinators,
            #[cfg(test)]
            benchmark_eager,
        }
    }
}

#[cfg(test)]
fn benchmark_eager_append_coordination() -> bool {
    static EAGER: OnceLock<bool> = OnceLock::new();
    *EAGER.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION").as_deref() == Ok("1")
    })
}

impl Debug for SandboxFile {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.handle {
            NativeFileHandle::Host(_) => formatter
                .debug_tuple("file")
                .field(&self.path.display().to_string())
                .finish(),
            #[cfg(test)]
            NativeFileHandle::Scripted(id) => formatter.debug_tuple("file").field(id).finish(),
        }
    }
}

impl SandboxFile {
    pub(super) fn host(&self) -> Result<&Arc<cap_std::fs::File>, FilesystemStorageError> {
        match &self.handle {
            NativeFileHandle::Host(file) => Ok(file),
            #[cfg(test)]
            NativeFileHandle::Scripted(_) => Err(scripted_handle_error("use scripted file")),
        }
    }

    /// Acquires the per-object lock required around a complete append operation.
    ///
    /// Callers use this only for append placement. Positioned writes do not need coordination.
    pub(crate) async fn coordinate_append(&self) -> OwnedMutexGuard<()> {
        let guard = self.append_coordinator().lock_owned().await;
        #[cfg(test)]
        self.append_coordination
            .coordinators
            .record_lock_acquisition();
        guard
    }

    fn append_coordinator(&self) -> Arc<AsyncMutex<()>> {
        #[cfg(test)]
        if let Some(coordinator) = &self.append_coordination.benchmark_eager {
            return Arc::clone(coordinator);
        }
        self.append_coordination
            .coordinators
            .coordinator(self.append_coordination.identity.clone())
    }

    #[cfg(test)]
    pub(crate) fn append_coordination_counts(&self) -> AppendCoordinationCounts {
        self.append_coordination.coordinators.counts()
    }

    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self {
            handle: NativeFileHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-file-{id}>")),
            append_coordination: NativeAppendCoordination::new(
                NativeFileIdentity::Scripted(format!("scripted-file-{id}")),
                Arc::new(AppendCoordinatorRegistry::default()),
            ),
        }
    }
}

impl Debug for SandboxDirectory {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.handle {
            NativeDirectoryHandle::Host(_) => formatter
                .debug_tuple("directory")
                .field(&self.path.display().to_string())
                .finish(),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(id) => {
                formatter.debug_tuple("directory").field(id).finish()
            }
        }
    }
}

impl SandboxDirectory {
    pub(super) fn host(&self) -> Result<&Arc<cap_std::fs::Dir>, FilesystemStorageError> {
        match &self.handle {
            NativeDirectoryHandle::Host(directory) => Ok(directory),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(_) => {
                Err(scripted_handle_error("use scripted directory"))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self::scripted_with_identity(id, format!("directory-{id}"))
    }

    #[cfg(test)]
    pub(super) fn scripted_with_identity(id: u64, identity: String) -> Self {
        Self {
            handle: NativeDirectoryHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-directory-{id}>")),
            coordination_key: SandboxDirectoryCoordinationKey(NativeFileIdentity::Scripted(
                identity,
            )),
        }
    }

    fn coordination_key(&self) -> SandboxDirectoryCoordinationKey {
        self.coordination_key.clone()
    }
}

impl SandboxOpened {
    /// Consumes the open result and returns its opaque file-or-directory handle.
    pub(crate) fn into_node(self) -> SandboxNode {
        self.node
    }

    /// Returns the opened directory's identity used for namespace coordination.
    ///
    /// File opens return `None`.
    pub(crate) fn directory_coordination_key(&self) -> Option<SandboxDirectoryCoordinationKey> {
        match &self.node {
            SandboxNode::Directory(directory) => Some(directory.coordination_key()),
            SandboxNode::File(_) => None,
        }
    }

    /// Tells whether the opened object is a regular file without write permission.
    pub(crate) fn is_read_only_file(&self) -> bool {
        self.read_only_file
    }

    #[cfg(test)]
    pub(crate) fn scripted_file(id: u64) -> Self {
        Self {
            node: SandboxNode::File(SandboxFile::scripted(id)),
            read_only_file: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_read_only_file(id: u64) -> Self {
        Self {
            node: SandboxNode::File(SandboxFile::scripted(id)),
            read_only_file: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_file_aliases(
        first_id: u64,
        second_id: u64,
        identity: u64,
    ) -> (Self, Self) {
        let coordinators = Arc::new(AppendCoordinatorRegistry::default());
        let identity = NativeFileIdentity::Scripted(format!("programmed-file-{identity}"));
        let file = |id, coordinators| SandboxFile {
            handle: NativeFileHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-file-{id}>")),
            append_coordination: NativeAppendCoordination::new(identity.clone(), coordinators),
        };
        (
            Self {
                node: SandboxNode::File(file(first_id, Arc::clone(&coordinators))),
                read_only_file: false,
            },
            Self {
                node: SandboxNode::File(file(second_id, coordinators)),
                read_only_file: false,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn scripted_directory(id: u64) -> Self {
        Self {
            node: SandboxNode::Directory(SandboxDirectory::scripted(id)),
            read_only_file: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_directory_with_identity(id: u64, identity: u64) -> Self {
        Self {
            node: SandboxNode::Directory(SandboxDirectory::scripted_with_identity(
                id,
                format!("programmed-directory-{identity}"),
            )),
            read_only_file: false,
        }
    }
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

    pub(super) fn operation_path(&self, root: &Path) -> PathBuf {
        match &self.base {
            SandboxPathBase::Root => root.join(&self.path),
            SandboxPathBase::Directory(directory) => directory.path.join(&self.path),
        }
    }
}

#[cfg(test)]
pub(super) fn scripted_handle_error(operation: &'static str) -> FilesystemStorageError {
    FilesystemStorageError::verification(operation, Path::new("<scripted-handle>"))
}
