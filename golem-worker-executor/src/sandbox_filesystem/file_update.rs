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

pub(super) fn apply_update(
    root: PathBuf,
    copy_mode: FileCopyMode,
    quota_authority: QuotaAuthority,
    current: HashSet<PathBuf>,
    updates: Vec<SandboxFileUpdate>,
    removals: Vec<PathBuf>,
) -> Result<(), FilesystemStorageError> {
    let staging = create_staging_dir(&root, quota_authority).map_err(|error| {
        FilesystemStorageError::io("create file update staging directory", &root, error)
    })?;
    let mut staged = Vec::with_capacity(updates.len());
    for (index, update) in updates.into_iter().enumerate() {
        let staged_path = staging.path().join(index.to_string());
        copy_file_blocking(
            copy_mode,
            quota_authority,
            staging.path(),
            update.source.as_path(),
            &staged_path,
            update.permissions.read_only(),
        )
        .map_err(|error| {
            FilesystemStorageError::io("copy file update source", &root.join(&update.target), error)
        })?;
        staged.push((update.target, staged_path));
    }

    let backup_root = staging.path().join("backups");
    std::fs::create_dir(&backup_root).map_err(|error| {
        FilesystemStorageError::io("create file update backup directory", &backup_root, error)
    })?;
    let mut replacements = staged
        .iter()
        .map(|(target, _)| target.clone())
        .chain(removals)
        .collect::<Vec<_>>();
    replacements.sort();
    replacements.dedup();

    let mut transaction = FileUpdateTransaction::new(backup_root);
    for relative in &replacements {
        let target = root.join(relative);
        if let Err(error) = transaction
            .create_parent(&root, &target)
            .and_then(|_| validate_replaceable_target(&target))
        {
            return Err(transaction.fail("prepare file update target", &target, error));
        }
        let exists = std::fs::symlink_metadata(&target).is_ok();
        if exists && !current.contains(relative) {
            return Err(transaction.fail(
                "preserve existing guest filesystem target",
                &target,
                std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "file update target already exists",
                ),
            ));
        }
        if exists && let Err(error) = transaction.back_up(&target) {
            return Err(transaction.fail("stage existing file target", &target, error));
        }
    }

    for (relative, staged_path) in &staged {
        let target = root.join(relative);
        if let Err(error) = transaction.install(staged_path, &target) {
            return Err(transaction.fail("install file update target", &target, error));
        }
    }

    transaction.commit();
    let staging_path = staging.path().to_path_buf();
    staging.close().map_err(|error| {
        FilesystemStorageError::cleanup_io(
            "remove file update staging directory",
            &staging_path,
            error,
        )
    })
}

fn create_staging_dir(
    root: &Path,
    quota_authority: QuotaAuthority,
) -> std::io::Result<tempfile::TempDir> {
    let staging = tempfile::Builder::new()
        .prefix(".golem-file-update-")
        .tempdir_in(root)?;
    if let QuotaAuthority::Project { project_id, .. } = quota_authority {
        #[cfg(target_os = "linux")]
        xfs::assign_project(&File::open(staging.path())?, project_id)?;
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }
    Ok(staging)
}

fn validate_replaceable_target(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => Ok(()),
        Ok(metadata) if metadata.is_dir() => {
            if std::fs::read_dir(path)?.next().is_some() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "file update target directory is not empty",
                ))
            } else {
                Ok(())
            }
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file update target is not replaceable",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

struct FileUpdateTransaction {
    backup_root: PathBuf,
    backups: Vec<(PathBuf, PathBuf)>,
    installed: Vec<PathBuf>,
    created_directories: Vec<PathBuf>,
    committed: bool,
}

impl FileUpdateTransaction {
    fn new(backup_root: PathBuf) -> Self {
        Self {
            backup_root,
            backups: Vec::new(),
            installed: Vec::new(),
            created_directories: Vec::new(),
            committed: false,
        }
    }

    fn create_parent(&mut self, root: &Path, target: &Path) -> std::io::Result<()> {
        let parent = target.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "file update target has no parent",
            )
        })?;
        let relative = parent.strip_prefix(root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "file update target escapes the sandbox filesystem",
            )
        })?;
        let mut current = root.to_path_buf();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "file update target contains an invalid path component",
                ));
            };
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "file update parent is not a directory",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current)?;
                    self.created_directories.push(current.clone());
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn back_up(&mut self, path: &Path) -> std::io::Result<()> {
        let backup = self.backup_root.join(self.backups.len().to_string());
        std::fs::rename(path, &backup)?;
        self.backups.push((path.to_path_buf(), backup));
        Ok(())
    }

    fn install(&mut self, staged: &Path, target: &Path) -> std::io::Result<()> {
        std::fs::rename(staged, target)?;
        self.installed.push(target.to_path_buf());
        Ok(())
    }

    fn fail(
        &mut self,
        operation: &'static str,
        path: &Path,
        error: std::io::Error,
    ) -> FilesystemStorageError {
        match self.rollback() {
            Ok(()) => FilesystemStorageError::io(operation, path, error),
            Err((rollback_path, rollback_error)) => FilesystemStorageError::cleanup_io(
                "roll back failed file update",
                &rollback_path,
                rollback_error,
            ),
        }
    }

    fn rollback(&mut self) -> Result<(), (PathBuf, std::io::Error)> {
        let mut failure = None;
        for path in self.installed.drain(..).rev() {
            let result = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => std::fs::remove_dir(&path),
                Ok(_) => std::fs::remove_file(&path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                failure = Some((path, error));
            }
        }
        for (original, backup) in self.backups.drain(..).rev() {
            if let Err(error) = std::fs::rename(&backup, &original) {
                failure = Some((original, error));
            }
        }
        for path in self.created_directories.drain(..).rev() {
            if let Err(error) = std::fs::remove_dir(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                failure = Some((path, error));
            }
        }
        self.committed = true;
        failure.map_or(Ok(()), Err)
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for FileUpdateTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}
