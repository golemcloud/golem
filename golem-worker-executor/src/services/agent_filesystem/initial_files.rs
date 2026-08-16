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
use crate::services::file_loader::InitialFileSource;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::component::AgentFilePermissions;
use golem_common::model::environment::EnvironmentId;

impl AgentFilesystemRuntime {
    pub(crate) fn is_read_only(&self, path: &Path) -> bool {
        self.is_read_only_path(path, true)
    }

    pub(crate) fn is_read_only_path(&self, path: &Path, follow_final_symlink: bool) -> bool {
        let path = resolve_policy_path(path, follow_final_symlink);
        self.inner
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .iter()
            .any(|(initial_path, file)| {
                file.permissions == AgentFilePermissions::ReadOnly
                    && resolve_policy_path(initial_path, true) == path
            })
    }

    pub(crate) fn contains_read_only_path(&self, path: &Path, follow_final_symlink: bool) -> bool {
        let path = resolve_policy_path(path, follow_final_symlink);
        self.inner
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .iter()
            .any(|(initial_path, file)| {
                let initial_path = resolve_policy_path(initial_path, true);
                file.permissions == AgentFilePermissions::ReadOnly
                    && (initial_path == path || initial_path.starts_with(&path))
            })
    }

    pub(crate) async fn replace_initial_files(
        &self,
        file_loader: &FileLoader,
        environment_id: EnvironmentId,
        files: &[InitialAgentFile],
    ) -> Result<(), FilesystemStorageError> {
        let effect = self.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "admit initial-file materialization",
                self.inner.materializer.root(),
                std::io::Error::other(error),
            )
        })?;
        let mut materialized = HashMap::new();
        let mut sources: HashMap<AgentFileContentHash, InitialFileSource> = HashMap::new();
        for file in files {
            let target = self.inner.materializer.target(file);
            let source = match sources.get(&file.content_hash) {
                Some(source) if source.size() == file.size => source,
                Some(_) => {
                    return Err(FilesystemStorageError::verification(
                        "verify consistent initial-file source size",
                        &target,
                    ));
                }
                None => {
                    let source = file_loader
                        .get_source(environment_id, file.content_hash, file.size)
                        .await
                        .map_err(|error| {
                            FilesystemStorageError::io(
                                "load verified initial-file source",
                                &target,
                                std::io::Error::other(error),
                            )
                        })?;
                    sources.entry(file.content_hash).or_insert(source)
                }
            };
            self.inner
                .materializer
                .materialize(source, file, effect.clone())
                .await?;
            if materialized.insert(target.clone(), file.clone()).is_some() {
                return Err(FilesystemStorageError::verification(
                    "materialize unique initial-file target",
                    &target,
                ));
            }
        }
        *self
            .inner
            .initial_files
            .write()
            .expect("initial-files policy lock poisoned") = materialized;
        Ok(())
    }

    pub(crate) async fn update_initial_files(
        &self,
        file_loader: &FileLoader,
        environment_id: EnvironmentId,
        files: &[InitialAgentFile],
    ) -> Result<AgentFilesystemUpdateEffectLease, FilesystemStorageError> {
        let effect = self.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "admit initial-file update",
                self.inner.materializer.root(),
                std::io::Error::other(error),
            )
        })?;
        let current = self
            .inner
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .clone();
        let update_result = async {
            let mut desired = HashMap::new();
            for file in files {
                let target = self.inner.materializer.target(file);
                if desired.insert(target.clone(), file.clone()).is_some() {
                    return Err(FilesystemStorageError::verification(
                        "materialize unique initial-file update target",
                        &target,
                    ));
                }
            }

            for (path, file) in &desired {
                if current.get(path).is_some_and(|existing| {
                    existing.permissions == AgentFilePermissions::ReadWrite
                        && file.permissions == AgentFilePermissions::ReadOnly
                }) {
                    return Err(FilesystemStorageError::verification(
                        "replace read-write initial file with read-only content",
                        path,
                    ));
                }
            }

            let staging = Arc::new(self.inner.materializer.create_staging_dir().map_err(
                |error| {
                    FilesystemStorageError::io(
                        "create initial-file update staging directory",
                        self.inner.materializer.root(),
                        error,
                    )
                },
            )?);
            let mut sources: HashMap<AgentFileContentHash, InitialFileSource> = HashMap::new();
            let mut staged = Vec::new();
            for (path, file) in &desired {
                match current.get(path) {
                    Some(existing)
                        if existing.permissions == AgentFilePermissions::ReadWrite
                            && file.permissions == AgentFilePermissions::ReadWrite => {}
                    Some(existing)
                        if existing.permissions == AgentFilePermissions::ReadOnly
                            && existing.content_hash == file.content_hash
                            && file.permissions == AgentFilePermissions::ReadOnly => {}
                    None if file.permissions == AgentFilePermissions::ReadWrite
                        && std::fs::symlink_metadata(path)
                            .is_ok_and(|metadata| metadata.is_file()) => {}
                    _ => {
                        let source = match sources.get(&file.content_hash) {
                            Some(source) if source.size() == file.size => source,
                            Some(_) => {
                                return Err(FilesystemStorageError::verification(
                                    "verify consistent initial-file update source size",
                                    path,
                                ));
                            }
                            None => {
                                let source = file_loader
                                    .get_source(environment_id, file.content_hash, file.size)
                                    .await
                                    .map_err(|error| {
                                        FilesystemStorageError::io(
                                            "load verified initial-file update source",
                                            path,
                                            std::io::Error::other(error),
                                        )
                                    })?;
                                sources.entry(file.content_hash).or_insert(source)
                            }
                        };
                        let staged_path = staging.path().join(staged.len().to_string());
                        self.inner
                            .materializer
                            .materialize_at(
                                source,
                                file,
                                staging.path(),
                                &staged_path,
                                effect.clone(),
                                Some(Arc::clone(&staging)),
                            )
                            .await?;
                        staged.push((path.clone(), staged_path));
                    }
                }
            }

            let backup_root = staging.path().join("backups");
            std::fs::create_dir(&backup_root).map_err(|error| {
                FilesystemStorageError::io(
                    "create initial-file update backup directory",
                    &backup_root,
                    error,
                )
            })?;
            let mut replacements: Vec<PathBuf> =
                staged.iter().map(|(path, _)| path.clone()).collect();
            replacements.extend(
                current
                    .iter()
                    .filter(|(path, existing)| {
                        existing.permissions == AgentFilePermissions::ReadOnly
                            && !desired.contains_key(*path)
                    })
                    .map(|(path, _)| path.clone()),
            );
            replacements.sort();
            replacements.dedup();

            let mut transaction = InitialFileUpdateTransaction::new(backup_root);
            for path in &replacements {
                if let Err(error) = transaction
                    .create_parent(self.inner.materializer.root(), path)
                    .and_then(|_| validate_replaceable_target(path))
                {
                    return Err(transaction.fail(
                        "prepare initial-file update target",
                        path,
                        error,
                    ));
                }
                let exists = std::fs::symlink_metadata(path).is_ok();
                if exists && !current.contains_key(path) {
                    return Err(transaction.fail(
                        "preserve existing guest filesystem target",
                        path,
                        std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "initial-file update target already exists",
                        ),
                    ));
                }
                if exists && let Err(error) = transaction.back_up(path) {
                    return Err(transaction.fail(
                        "stage existing initial-file target",
                        path,
                        error,
                    ));
                }
            }

            for (target, staged_path) in &staged {
                if let Err(error) = transaction.install(staged_path, target) {
                    return Err(transaction.fail(
                        "install initial-file update target",
                        target,
                        error,
                    ));
                }
            }

            transaction.commit();
            *self
                .inner
                .initial_files
                .write()
                .expect("initial-files policy lock poisoned") = desired;
            let staging_path = staging.path().to_path_buf();
            let staging = Arc::try_unwrap(staging)
                .expect("initial-file staging work must finish before commit cleanup");
            if let Err(error) = staging.close() {
                let error = FilesystemStorageError::cleanup_io(
                    "remove initial-file update staging directory",
                    &staging_path,
                    error,
                );
                self.seal();
                return Err(error);
            }
            Ok::<(), FilesystemStorageError>(())
        }
        .await;
        update_result?;
        Ok(effect)
    }
}

impl AgentFilesystemMaterializer {
    pub(super) fn root(&self) -> &Path {
        match self {
            Self::Unmanaged { root, .. } => root,
            #[cfg(target_os = "linux")]
            Self::Managed { root, .. } => root,
        }
    }

    fn create_staging_dir(&self) -> std::io::Result<tempfile::TempDir> {
        let staging = tempfile::Builder::new()
            .prefix(".golem-initial-files-update-")
            .tempdir_in(self.root())?;
        #[cfg(target_os = "linux")]
        if let Self::Managed {
            backend,
            project_id,
            ..
        } = self
        {
            let directory = File::open(staging.path())?;
            backend.assign_project(&directory, *project_id)?;
        }
        Ok(staging)
    }

    fn target(&self, file: &InitialAgentFile) -> PathBuf {
        self.root().join(file.path.to_rel_string())
    }

    async fn materialize(
        &self,
        source: &InitialFileSource,
        file: &InitialAgentFile,
        effect: AgentFilesystemUpdateEffectLease,
    ) -> Result<(), FilesystemStorageError> {
        let target = self.target(file);
        self.materialize_at(source, file, self.root(), &target, effect, None)
            .await
    }

    async fn materialize_at(
        &self,
        source: &InitialFileSource,
        file: &InitialAgentFile,
        materialization_root: &Path,
        target: &Path,
        effect: AgentFilesystemUpdateEffectLease,
        staging: Option<Arc<tempfile::TempDir>>,
    ) -> Result<(), FilesystemStorageError> {
        let target = target.to_path_buf();
        let source = source.clone();
        let read_only = file.permissions == AgentFilePermissions::ReadOnly;
        match self {
            Self::Unmanaged { .. } => {
                let root = materialization_root.to_path_buf();
                let operation_target = target.clone();
                tokio::task::spawn_blocking(move || {
                    let _effect = effect;
                    let _staging = staging;
                    materialize_unmanaged(&root, source.path(), &operation_target, read_only)
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "materialize unmanaged initial file",
                        &target,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("materialize unmanaged initial file", &target, error)
                })
            }
            #[cfg(target_os = "linux")]
            Self::Managed {
                backend,
                project_id,
                ..
            } => {
                let root = materialization_root.to_path_buf();
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                let operation_target = target.clone();
                tokio::task::spawn_blocking(move || {
                    let _effect = effect;
                    let _staging = staging;
                    backend.materialize_initial_file(
                        &root,
                        project_id,
                        source.path(),
                        &operation_target,
                        read_only,
                    )
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "reflink managed XFS initial file",
                        &target,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("reflink managed XFS initial file", &target, error)
                })
            }
        }
    }
}

fn materialize_unmanaged(
    root: &Path,
    source: &Path,
    target: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    let parent = create_materialization_parent(root, target)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut source = std::fs::File::open(source)?;
    std::io::copy(&mut source, &mut temporary)?;
    temporary.as_file().sync_all()?;
    set_initial_file_permissions(temporary.as_file(), read_only)?;
    temporary
        .persist_noclobber(target)
        .map_err(|error| error.error)?;
    Ok(())
}

fn validate_replaceable_target(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => Ok(()),
        Ok(metadata) if metadata.is_dir() => {
            if std::fs::read_dir(path)?.next().is_some() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "initial-file target directory is not empty",
                ))
            } else {
                Ok(())
            }
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "initial-file target is not replaceable",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) struct InitialFileUpdateTransaction {
    backup_root: PathBuf,
    backups: Vec<(PathBuf, PathBuf)>,
    installed: Vec<PathBuf>,
    created_directories: Vec<PathBuf>,
    committed: bool,
}

impl InitialFileUpdateTransaction {
    pub(super) fn new(backup_root: PathBuf) -> Self {
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
                "initial-file target has no parent",
            )
        })?;
        let relative = parent.strip_prefix(root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "initial-file target escapes the agent filesystem",
            )
        })?;
        let mut current = root.to_path_buf();
        for component in relative.components() {
            let std::path::Component::Normal(component) = component else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "initial-file target contains an invalid path component",
                ));
            };
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "initial-file parent is not a directory",
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

    pub(super) fn back_up(&mut self, path: &Path) -> std::io::Result<()> {
        let backup = self.backup_root.join(self.backups.len().to_string());
        std::fs::rename(path, &backup)?;
        self.backups.push((path.to_path_buf(), backup));
        Ok(())
    }

    pub(super) fn install(&mut self, staged: &Path, target: &Path) -> std::io::Result<()> {
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
                "roll back failed initial-file update",
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

impl Drop for InitialFileUpdateTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

fn resolve_policy_path(path: &Path, follow_final_symlink: bool) -> PathBuf {
    if !follow_final_symlink && let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        return resolve_policy_path(parent, true).join(name);
    }
    let mut unresolved = Vec::new();
    let mut current = path;
    loop {
        match std::fs::canonicalize(current) {
            Ok(mut resolved) => {
                for component in unresolved.iter().rev() {
                    resolved.push(component);
                }
                return resolved;
            }
            Err(_) => match (current.parent(), current.file_name()) {
                (Some(parent), Some(name)) => {
                    unresolved.push(name.to_os_string());
                    current = parent;
                }
                _ => return path.to_path_buf(),
            },
        }
    }
}
