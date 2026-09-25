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

//! The publish of a snapshot file.
//!
//! In a save of the store, the backend keeps the snapshot file in a [`SnapshotStage`] and does not
//! write it. The save writes it later with [`publish`], after the blocking work returns. That
//! write is the step that makes the snapshot visible. A publish that fails, or that the caller
//! drops, deletes the file again, because a write that the storage received can still complete.

use super::files::SnapshotFiles;
use bytes::Bytes;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::runtime::Handle;
use tokio_util::task::TaskTracker;
use tracing::warn;

/// A snapshot file that the backend kept and did not write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StagedSnapshot {
    /// The path of the file, relative to the root of the namespace. The drop guard of a publish
    /// and its delete task share it.
    pub(super) path: Arc<Path>,
    pub(super) content: Bytes,
}

/// The place where the backend of one save keeps its snapshot file.
#[derive(Debug, Default)]
pub(super) struct SnapshotStage(Mutex<Option<StagedSnapshot>>);

impl SnapshotStage {
    /// Keeps the file. A stage holds one file, so a second file gives it back as the error.
    pub(super) fn keep(&self, staged: StagedSnapshot) -> Result<(), StagedSnapshot> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match *slot {
            Some(_) => Err(staged),
            None => {
                *slot = Some(staged);
                Ok(())
            }
        }
    }

    /// Takes the file out of the stage.
    pub(super) fn take(&self) -> Option<StagedSnapshot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Writes the staged file only when its path has no blob, which makes the snapshot visible. The
/// name is the hash of the content, so a blob at the path is this file. A failed write deletes the
/// path before the error returns, and a dropped write deletes it in a task of `tracker`.
pub(super) async fn publish(
    files: &SnapshotFiles,
    staged: &StagedSnapshot,
    tracker: &TaskTracker,
) -> anyhow::Result<()> {
    let mut retraction = RetractOnDrop {
        files: files.clone(),
        path: staged.path.clone(),
        tracker: tracker.clone(),
        armed: true,
    };
    let written = files
        .put_if_absent("publish", &staged.path, &staged.content)
        .await;
    retraction.armed = false;
    match written {
        Ok(_) => Ok(()),
        Err(error) => {
            retract_or_warn(files, &staged.path).await;
            Err(error)
        }
    }
}

/// Deletes the snapshot file at the path. A path without a blob gives success.
pub(super) async fn retract(files: &SnapshotFiles, path: &Path) -> anyhow::Result<()> {
    files.delete("retract", path).await
}

async fn retract_or_warn(files: &SnapshotFiles, path: &Path) {
    if let Err(error) = retract(files, path).await {
        warn!(
            path = %path.display(),
            error = %format!("{error:#}"),
            "Failed to delete a filesystem snapshot file whose publish did not finish"
        );
    }
}

/// Deletes the path in a task of the tracker when it is dropped while it is armed.
struct RetractOnDrop {
    files: SnapshotFiles,
    path: Arc<Path>,
    tracker: TaskTracker,
    armed: bool,
}

impl Drop for RetractOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let files = self.files.clone();
        let path = self.path.clone();
        match Handle::try_current() {
            Ok(runtime) => {
                self.tracker.spawn_on(
                    async move { retract_or_warn(&files, &path).await },
                    &runtime,
                );
            }
            Err(_) => warn!(
                path = %path.display(),
                "Failed to delete a dropped filesystem snapshot file, because no runtime runs"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
