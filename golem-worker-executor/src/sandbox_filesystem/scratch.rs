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
use std::fmt::Debug;

pub(super) const SCRATCH_DIRECTORY_NAME: &str = ".scratch";

/// The executor-owned scratch directory on a volume.
///
/// The directory is a plain directory. It has no project id, no quota and no metering. The
/// executor creates it empty at startup and removes what a previous process left in it.
#[derive(Clone)]
pub(crate) struct ScratchSpace {
    root: Box<Path>,
    _anchor: Option<Arc<File>>,
    cleanup_retry: RetryConfig,
}

impl ScratchSpace {
    /// Removes a stale scratch directory under `parent` and creates an empty one.
    ///
    /// `anchor` keeps a descriptor open while the space lives, so a `parent` that names a
    /// descriptor stays valid.
    pub(super) fn create(
        parent: &Path,
        anchor: Option<Arc<File>>,
        cleanup_retry: &RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        let root = parent.join(SCRATCH_DIRECTORY_NAME);
        remove_and_verify_blocking(&root, "remove stale scratch directory")?;
        std::fs::create_dir(&root).map_err(|error| {
            FilesystemStorageError::io("create scratch directory", &root, error)
        })?;
        Ok(Self {
            root: root.into_boxed_path(),
            _anchor: anchor,
            cleanup_retry: cleanup_retry.clone(),
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// Creates an empty scratch tree with a new unique name.
    pub(super) fn create_tree_blocking(&self) -> Result<ScratchTree, FilesystemStorageError> {
        let root = self.root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&root)
            .map_err(|error| FilesystemStorageError::io("create scratch tree", &root, error))?;
        Ok(ScratchTree {
            root: root.into_boxed_path(),
            space: self.clone(),
            removed: false,
        })
    }
}

/// A directory under the scratch space. The executor owns it. Discard or drop removes it.
pub(crate) struct ScratchTree {
    root: Box<Path>,
    space: ScratchSpace,
    removed: bool,
}

impl ScratchTree {
    /// The host path of the tree. It is stable while the value lives.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Removes the tree and all that is in it.
    #[allow(dead_code)]
    pub(crate) async fn discard(mut self) -> Result<(), FilesystemStorageError> {
        self.removed = true;
        remove_and_verify(
            &self.root,
            "discard scratch tree",
            &self.space.cleanup_retry,
        )
        .await
    }
}

impl Debug for ScratchTree {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScratchTree")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Drop for ScratchTree {
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        if let Err(error) = remove_and_verify_blocking(&self.root, "discard scratch tree") {
            tracing::error!(error = %error, "Failed to remove a dropped scratch tree");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn creation_replaces_stale_scratch_contents() {
        let parent = tempfile::tempdir().unwrap();
        let stale = parent.path().join(SCRATCH_DIRECTORY_NAME).join("old-tree");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("garbage"), b"stale").unwrap();

        let space = ScratchSpace::create(parent.path(), None, &RetryConfig::default()).unwrap();

        assert_eq!(space.root(), parent.path().join(SCRATCH_DIRECTORY_NAME));
        assert!(std::fs::read_dir(space.root()).unwrap().next().is_none());
    }

    #[test]
    async fn trees_are_unique_and_removed_on_discard_or_drop() {
        let parent = tempfile::tempdir().unwrap();
        let space = ScratchSpace::create(parent.path(), None, &RetryConfig::default()).unwrap();

        let discarded = space.create_tree_blocking().unwrap();
        let dropped = space.create_tree_blocking().unwrap();
        assert_ne!(discarded.root(), dropped.root());
        assert_eq!(discarded.root().parent(), Some(space.root()));
        std::fs::write(discarded.root().join("file"), b"contents").unwrap();
        std::fs::write(dropped.root().join("file"), b"contents").unwrap();

        let discarded_root = discarded.root().to_path_buf();
        discarded.discard().await.unwrap();
        assert!(!discarded_root.exists());

        let dropped_root = dropped.root().to_path_buf();
        drop(dropped);
        assert!(!dropped_root.exists());
        assert!(std::fs::read_dir(space.root()).unwrap().next().is_none());
    }

    #[test]
    async fn discard_reports_a_removal_failure() {
        use std::os::unix::fs::PermissionsExt;

        if rustix::process::geteuid().is_root() {
            return;
        }
        let parent = tempfile::tempdir().unwrap();
        let space = ScratchSpace::create(parent.path(), None, &RetryConfig::default()).unwrap();
        let tree = space.create_tree_blocking().unwrap();
        let locked = tree.root().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("file"), b"contents").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error = tree.discard().await.unwrap_err();

        assert!(error.cleanup_failed(), "{error}");
        assert!(
            error
                .to_string()
                .starts_with("failed to discard scratch tree"),
            "{error}"
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(locked.join("file").is_file());
    }
}
