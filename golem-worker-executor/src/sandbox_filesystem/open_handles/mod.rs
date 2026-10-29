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

use super::object_identity::{NativeFileIdentity, SandboxDirectoryId};
#[cfg(test)]
use super::scripted_adapter::scripted_handle_error;
use crate::sandbox_filesystem::FilesystemStorageError;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::PathBuf;
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub(in crate::sandbox_filesystem) fn keep_append_coordinator(strong_count: usize) -> bool {
    strong_count != 0
}

pub(in crate::sandbox_filesystem) enum AppendCoordinatorAction {
    Reuse(Arc<AsyncMutex<()>>),
    Allocate,
}

pub(in crate::sandbox_filesystem) fn decide_append_coordinator(
    observed: Option<Arc<AsyncMutex<()>>>,
) -> AppendCoordinatorAction {
    match observed {
        Some(coordinator) => AppendCoordinatorAction::Reuse(coordinator),
        None => AppendCoordinatorAction::Allocate,
    }
}

#[cfg(test)]
pub(in crate::sandbox_filesystem) fn live_append_coordinators(
    strong_counts: impl Iterator<Item = usize>,
) -> usize {
    strong_counts
        .filter(|count| keep_append_coordinator(*count))
        .count()
}

#[derive(Default)]
pub(in crate::sandbox_filesystem) struct AppendCoordinatorRegistry {
    pub(in crate::sandbox_filesystem) coordinators:
        Mutex<HashMap<NativeFileIdentity, Weak<AsyncMutex<()>>>>,
    #[cfg(test)]
    pub(in crate::sandbox_filesystem) lookups: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pub(in crate::sandbox_filesystem) allocations: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pub(in crate::sandbox_filesystem) lock_acquisitions: std::sync::atomic::AtomicUsize,
}

impl AppendCoordinatorRegistry {
    fn coordinator(&self, identity: NativeFileIdentity) -> Arc<AsyncMutex<()>> {
        let mut coordinators = self
            .coordinators
            .lock()
            .expect("sandbox filesystem append coordinator lock poisoned");
        #[cfg(test)]
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        coordinators.retain(|_, coordinator| keep_append_coordinator(coordinator.strong_count()));
        match decide_append_coordinator(coordinators.get(&identity).and_then(Weak::upgrade)) {
            AppendCoordinatorAction::Reuse(coordinator) => coordinator,
            AppendCoordinatorAction::Allocate => {
                let coordinator = Arc::new(AsyncMutex::new(()));
                coordinators.insert(identity, Arc::downgrade(&coordinator));
                #[cfg(test)]
                self.allocations
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                coordinator
            }
        }
    }

    #[cfg(test)]
    fn record_lock_acquisition(&self) {
        self.lock_acquisitions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(in crate::sandbox_filesystem) fn counts(&self) -> AppendCoordinationCounts {
        use std::sync::atomic::Ordering;

        let coordinators = self
            .coordinators
            .lock()
            .expect("sandbox filesystem append coordinator lock poisoned");
        AppendCoordinationCounts {
            lookups: self.lookups.load(Ordering::Relaxed),
            allocations: self.allocations.load(Ordering::Relaxed),
            lock_acquisitions: self.lock_acquisitions.load(Ordering::Relaxed),
            registered: coordinators.len(),
            live: live_append_coordinators(coordinators.values().map(Weak::strong_count)),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AppendCoordinationCounts {
    pub(crate) lookups: usize,
    pub(crate) allocations: usize,
    pub(crate) lock_acquisitions: usize,
    pub(crate) registered: usize,
    pub(crate) live: usize,
}

#[derive(Clone)]
pub(crate) struct SandboxFile {
    pub(in crate::sandbox_filesystem) handle: NativeFileHandle,
    pub(in crate::sandbox_filesystem) path: PathBuf,
    pub(in crate::sandbox_filesystem) append_coordination: NativeAppendCoordination,
    /// The regular file's permissions observed when this handle was opened.
    pub(in crate::sandbox_filesystem) permissions: SandboxFilePermissions,
}

#[derive(Clone)]
pub(in crate::sandbox_filesystem) enum NativeFileHandle {
    Host(Arc<cap_std::fs::File>),
    #[cfg(test)]
    Scripted(u64),
}

#[derive(Clone)]
pub(in crate::sandbox_filesystem) struct NativeAppendCoordination {
    pub(in crate::sandbox_filesystem) identity: NativeFileIdentity,
    pub(in crate::sandbox_filesystem) coordinators: Arc<AppendCoordinatorRegistry>,
    #[cfg(test)]
    pub(in crate::sandbox_filesystem) benchmark_eager: Option<Arc<AsyncMutex<()>>>,
}

#[derive(Clone)]
pub(crate) struct SandboxDirectory {
    pub(in crate::sandbox_filesystem) handle: NativeDirectoryHandle,
    pub(in crate::sandbox_filesystem) path: PathBuf,
    pub(in crate::sandbox_filesystem) directory_id: SandboxDirectoryId,
}

#[derive(Clone)]
pub(in crate::sandbox_filesystem) enum NativeDirectoryHandle {
    Host(Arc<cap_std::fs::Dir>),
    #[cfg(test)]
    Scripted(u64),
}

#[derive(Clone, Debug)]
pub(crate) enum SandboxHandle {
    File(SandboxFile),
    Directory(SandboxDirectory),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sandbox_filesystem) enum SandboxFilePermissions {
    ReadOnly,
    Writable,
}

impl NativeAppendCoordination {
    pub(in crate::sandbox_filesystem) fn new(
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
pub(in crate::sandbox_filesystem) fn benchmark_eager_append_coordination() -> bool {
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
    pub(in crate::sandbox_filesystem) fn host(
        &self,
    ) -> Result<&Arc<cap_std::fs::File>, FilesystemStorageError> {
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
    pub(in crate::sandbox_filesystem) fn host(
        &self,
    ) -> Result<&Arc<cap_std::fs::Dir>, FilesystemStorageError> {
        match &self.handle {
            NativeDirectoryHandle::Host(directory) => Ok(directory),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(_) => {
                Err(scripted_handle_error("use scripted directory"))
            }
        }
    }

    fn directory_id(&self) -> SandboxDirectoryId {
        self.directory_id.clone()
    }
}

impl SandboxHandle {
    /// Returns the opened directory's identity used for namespace coordination.
    ///
    /// File opens return `None`.
    pub(crate) fn directory_id(&self) -> Option<SandboxDirectoryId> {
        match self {
            SandboxHandle::Directory(directory) => Some(directory.directory_id()),
            SandboxHandle::File(_) => None,
        }
    }

    /// Tells whether the opened object is a regular file without write permission.
    pub(crate) fn is_read_only_file(&self) -> bool {
        matches!(self, Self::File(file) if file.permissions == SandboxFilePermissions::ReadOnly)
    }
}

#[cfg(test)]
mod tests;
