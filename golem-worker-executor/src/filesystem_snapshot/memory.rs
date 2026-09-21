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

//! A filesystem snapshot store that keeps each snapshot in the memory of the process.
//!
//! The snapshots of a scope are one immutable slice. Each change makes a new slice from the old
//! one with a pure function, and the store puts it in place under a lock that no await holds.
//! A restore keeps the tree that it read under the lock, so a delete at the same time cannot
//! change it, and two scopes that a copy made share trees that never change.

mod tree;

#[cfg(test)]
mod tests;

use super::{
    FilesystemSnapshotStore, SnapshotInfo, SnapshotName, SnapshotScope, SnapshotStoreError,
    newest_first, snapshot_time,
};
use async_trait::async_trait;
use golem_common::model::Timestamp;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tree::{TreeEntry, read_tree, tree_info, write_tree};

/// A filesystem snapshot store that keeps each snapshot in the memory of the process.
///
/// A clone of the store is one more store over the same snapshots, as another executor has.
#[derive(Clone, Default)]
pub(crate) struct InMemorySnapshotStore {
    scopes: Arc<Mutex<HashMap<SnapshotScope, Arc<[Stored]>>>>,
}

/// One snapshot of a scope.
#[derive(Clone)]
struct Stored {
    name: SnapshotName,
    info: SnapshotInfo,
    tree: Arc<[TreeEntry]>,
}

impl InMemorySnapshotStore {
    /// Makes a store that holds no snapshot.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Gives the snapshots of each scope. No lock is held across an await, so a panic cannot
    /// leave a change half made, and the store uses the map of a poisoned lock as it is.
    fn scopes(&self) -> MutexGuard<'_, HashMap<SnapshotScope, Arc<[Stored]>>> {
        self.scopes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives the snapshots of the scope. A scope that holds nothing gives an empty slice.
    fn snapshots_of(&self, scope: &SnapshotScope) -> Arc<[Stored]> {
        self.scopes().get(scope).cloned().unwrap_or_default()
    }
}

/// Gives the snapshot with the name.
fn found<'a>(snapshots: &'a [Stored], name: &SnapshotName) -> Option<&'a Stored> {
    snapshots.iter().find(|stored| stored.name == *name)
}

/// Gives the time of the newest snapshot.
fn newest(snapshots: &[Stored]) -> Option<Timestamp> {
    snapshots.iter().map(|stored| stored.info.created_at).max()
}

/// Gives the snapshots with `snapshot` added, or `AlreadyExists` when a snapshot has its name.
fn with_saved(snapshots: &[Stored], snapshot: Stored) -> Result<Arc<[Stored]>, SnapshotStoreError> {
    match found(snapshots, &snapshot.name) {
        Some(_) => Err(SnapshotStoreError::AlreadyExists),
        None => Ok(snapshots
            .iter()
            .cloned()
            .chain(std::iter::once(snapshot))
            .collect()),
    }
}

/// Gives the snapshots without the snapshot with the name.
fn without(snapshots: &[Stored], name: &SnapshotName) -> Arc<[Stored]> {
    snapshots
        .iter()
        .filter(|stored| stored.name != *name)
        .cloned()
        .collect()
}

/// Runs blocking work on a thread of the blocking pool, so the async runtime is not blocked.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, SnapshotStoreError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| SnapshotStoreError::Storage {
            retryable: false,
            source: anyhow::Error::new(error),
        })
}

#[async_trait]
impl FilesystemSnapshotStore for InMemorySnapshotStore {
    async fn save(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        tree: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let snapshots = self.snapshots_of(scope);
        if found(&snapshots, name).is_some() {
            return Err(SnapshotStoreError::AlreadyExists);
        }
        let newest = newest(&snapshots);

        let root = tree.to_path_buf();
        let tree = blocking(move || read_tree(&root))
            .await?
            .map_err(SnapshotStoreError::Source)?;
        let info = tree_info(&tree, snapshot_time(Timestamp::now_utc(), newest));

        // A save of the same name can finish during the read, so the check runs again in the
        // step that publishes the snapshot.
        let mut scopes = self.scopes();
        let current = scopes.get(scope).cloned().unwrap_or_default();
        let saved = with_saved(
            &current,
            Stored {
                name: name.clone(),
                info,
                tree,
            },
        )?;
        scopes.insert(scope.clone(), saved);
        Ok(info)
    }

    async fn restore(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let snapshots = self.snapshots_of(scope);
        let stored = found(&snapshots, name)
            .cloned()
            .ok_or(SnapshotStoreError::NotFound)?;

        let into = into.to_path_buf();
        let tree = stored.tree;
        blocking(move || write_tree(&tree, &into))
            .await?
            .map_err(SnapshotStoreError::Destination)?;
        Ok(stored.info)
    }

    async fn stat(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        Ok(found(&self.snapshots_of(scope), name).map(|stored| stored.info))
    }

    async fn list(
        &self,
        scope: &SnapshotScope,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        Ok(newest_first(
            self.snapshots_of(scope)
                .iter()
                .map(|stored| (stored.name.clone(), stored.info)),
        ))
    }

    async fn delete(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError> {
        let mut scopes = self.scopes();
        if let Some(snapshots) = scopes.get(scope) {
            let kept = without(snapshots, name);
            scopes.insert(scope.clone(), kept);
        }
        Ok(())
    }

    async fn delete_scope(&self, scope: &SnapshotScope) -> Result<(), SnapshotStoreError> {
        self.scopes().remove(scope);
        Ok(())
    }

    async fn copy_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        // The snapshots never change, so the two scopes can hold the same slice and stay
        // independent: a change of one scope puts a new slice in that scope only.
        let mut scopes = self.scopes();
        if let Some(snapshots) = scopes.get(from).cloned() {
            scopes.insert(to.clone(), snapshots);
        }
        Ok(())
    }
}
