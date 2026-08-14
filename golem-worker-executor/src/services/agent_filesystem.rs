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

use crate::metrics::workers::record_agent_filesystem_lifecycle;
use crate::services::golem_config::FilesystemStorageConfig;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::{OwnedAgentId, RetryConfig};
use golem_common::retries::RetryState;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Instant;
use tokio::sync::{Mutex, OwnedMutexGuard};
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::{StreamError, StreamResult};

static LIFECYCLE_LOCKS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    OnceLock::new();

#[derive(Debug)]
pub(crate) struct FilesystemStorageError {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
}

impl FilesystemStorageError {
    fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: false,
        }
    }

    fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: false,
        }
    }

    fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: true,
        }
    }

    fn cleanup_verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: true,
        }
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }
}

impl Display for FilesystemStorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to {} agent filesystem {}",
            self.operation,
            self.path.display()
        )?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone)]
pub(crate) struct AgentFilesystems {
    deterministic_root: Option<PathBuf>,
    cleanup_retry: RetryConfig,
}

impl AgentFilesystems {
    pub(crate) fn new(settings: &FilesystemStorageConfig) -> Self {
        Self {
            deterministic_root: settings.deterministic_root_dir.clone(),
            cleanup_retry: settings.cleanup_retry.clone(),
        }
    }

    pub(crate) async fn create_fresh(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let started = Instant::now();
        let result = match &self.deterministic_root {
            Some(root) => self.create_deterministic_filesystem(root, agent_id).await,
            None => self.create_temporary_filesystem().await,
        };
        record_agent_filesystem_lifecycle("create", result.is_ok(), started.elapsed());
        result
    }

    async fn create_temporary_filesystem(&self) -> Result<AgentFilesystem, FilesystemStorageError> {
        let directory = tempfile::Builder::new()
            .prefix("golem")
            .tempdir()
            .map_err(|error| {
                FilesystemStorageError::io("create temporary directory", Path::new("<temp>"), error)
            })?;
        let lifecycle = Arc::new(Mutex::new(()))
            .try_lock_owned()
            .expect("new temporary filesystem lifecycle lock must be available");
        let path = directory.keep();
        let filesystem = AgentFilesystem {
            path,
            runtime: AgentFilesystemRuntime::new(),
            cleanup_retry: self.cleanup_retry.clone(),
            _lifecycle: lifecycle,
            delete_on_drop: true,
        };

        if let Err(error) = verify_fresh_directory(filesystem.path()).await {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }

        Ok(filesystem)
    }

    async fn create_deterministic_filesystem(
        &self,
        root: &Path,
        agent_id: &OwnedAgentId,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let path = root
            .join(agent_id.environment_id.to_string())
            .join(agent_id.agent_id.component_id.to_string())
            .join(agent_id.agent_id.agent_name_encoded());

        let lifecycle = acquire_lifecycle_lock(&path).await;

        remove_and_verify(&path, "remove stale runtime directory", &self.cleanup_retry).await?;
        let parent = path
            .parent()
            .expect("deterministic agent filesystem path must have a parent");
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return Err(rollback_creation(
                &path,
                FilesystemStorageError::io("create runtime directory parent", parent, error),
                &self.cleanup_retry,
            )
            .await);
        }
        if let Err(error) = tokio::fs::create_dir(&path).await {
            return Err(rollback_creation(
                &path,
                FilesystemStorageError::io("create fresh runtime directory", &path, error),
                &self.cleanup_retry,
            )
            .await);
        }

        let filesystem = AgentFilesystem {
            path,
            runtime: AgentFilesystemRuntime::new(),
            cleanup_retry: self.cleanup_retry.clone(),
            _lifecycle: lifecycle,
            delete_on_drop: true,
        };

        if let Err(error) = verify_fresh_directory(filesystem.path()).await {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }

        Ok(filesystem)
    }
}

pub(crate) struct AgentFilesystem {
    path: PathBuf,
    runtime: AgentFilesystemRuntime,
    cleanup_retry: RetryConfig,
    _lifecycle: OwnedMutexGuard<()>,
    delete_on_drop: bool,
}

impl AgentFilesystem {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn runtime(&self) -> AgentFilesystemRuntime {
        self.runtime.clone()
    }

    pub(crate) fn seal(&self) {
        self.runtime.seal();
    }

    pub(crate) async fn close_and_delete(mut self) -> Result<(), FilesystemStorageError> {
        let started = Instant::now();
        self.seal();
        let result =
            remove_and_verify(&self.path, "delete runtime directory", &self.cleanup_retry).await;
        // A completed explicit attempt is authoritative. The fallback is only
        // for owners dropped before explicit cleanup can finish.
        self.delete_on_drop = false;
        record_agent_filesystem_lifecycle("delete", result.is_ok(), started.elapsed());
        result
    }
}

impl Drop for AgentFilesystem {
    fn drop(&mut self) {
        if self.delete_on_drop {
            let started = Instant::now();
            let result = remove_and_verify_blocking(&self.path);
            record_agent_filesystem_lifecycle("delete_fallback", result.is_ok(), started.elapsed());
            if let Err(error) = result {
                tracing::error!(error = %error, "Failed to delete agent runtime filesystem during fallback cleanup");
            }
        }
    }
}

#[derive(Clone)]
pub struct AgentFilesystemRuntime {
    inner: Arc<AgentFilesystemRuntimeInner>,
}

struct AgentFilesystemRuntimeInner {
    sealed: AtomicBool,
    append: Arc<Mutex<()>>,
}

impl AgentFilesystemRuntime {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(AgentFilesystemRuntimeInner {
                sealed: AtomicBool::new(false),
                append: Arc::new(Mutex::new(())),
            }),
        }
    }

    pub(crate) async fn begin_effect(&self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin().await)
    }

    pub(crate) async fn begin_append_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin_append().await)
    }

    pub(crate) fn admit_effect(&self) -> Result<AgentFilesystemEffectAdmission, wasmtime::Error> {
        if self.inner.sealed.load(Ordering::Acquire) {
            return Err(wasmtime::Error::msg("agent filesystem is closing"));
        }
        Ok(AgentFilesystemEffectAdmission {
            inner: Arc::clone(&self.inner),
        })
    }

    fn seal(&self) {
        self.inner.sealed.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub(crate) struct AgentFilesystemEffectLease {
    _admission: AgentFilesystemEffectAdmission,
    _append_guard: Option<OwnedMutexGuard<()>>,
}

pub(crate) struct AgentFilesystemEffectAdmission {
    inner: Arc<AgentFilesystemRuntimeInner>,
}

impl AgentFilesystemEffectAdmission {
    pub(crate) async fn begin(self) -> AgentFilesystemEffectLease {
        AgentFilesystemEffectLease {
            _admission: self,
            _append_guard: None,
        }
    }

    pub(crate) async fn begin_append(self) -> AgentFilesystemEffectLease {
        let guard = Arc::clone(&self.inner.append).lock_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _append_guard: Some(guard),
        }
    }
}

impl std::fmt::Debug for AgentFilesystemEffectAdmission {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFilesystemEffectAdmission")
            .finish_non_exhaustive()
    }
}

pub(crate) struct CoordinatedFileOutputStream {
    inner: Arc<Mutex<DynOutputStream>>,
    active: Arc<AtomicBool>,
    ready: Arc<tokio::sync::Notify>,
    cancel: Arc<std::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    prepared_effect: std::sync::Mutex<Option<AgentFilesystemEffectLease>>,
}

struct ActiveFilesystemEffect {
    active: Arc<AtomicBool>,
    ready: Arc<tokio::sync::Notify>,
    _effect: Option<AgentFilesystemEffectLease>,
}

impl Drop for ActiveFilesystemEffect {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.ready.notify_waiters();
    }
}

impl CoordinatedFileOutputStream {
    pub(crate) fn new(inner: DynOutputStream) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            active: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(tokio::sync::Notify::new()),
            cancel: Arc::new(std::sync::Mutex::new(None)),
            prepared_effect: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn into_dyn(self) -> DynOutputStream {
        Box::new(self)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn prepare_effect(&self, effect: AgentFilesystemEffectLease) {
        let previous = self
            .prepared_effect
            .lock()
            .expect("filesystem output stream effect lock poisoned")
            .replace(effect);
        debug_assert!(previous.is_none());
    }

    pub(crate) fn clear_unused_effect(&self) {
        self.prepared_effect
            .lock()
            .expect("filesystem output stream effect lock poisoned")
            .take();
    }

    async fn wait_until_ready(&self) {
        while self.is_active() {
            let notified = self.ready.notified();
            if !self.is_active() {
                break;
            }
            notified.await;
        }
    }
}

#[async_trait]
impl OutputStream for CoordinatedFileOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if self.is_active() {
            return Err(StreamError::Trap(wasmtime::Error::msg(
                "write not permitted: check_write not called first",
            )));
        }

        let result = self
            .inner
            .try_lock()
            .expect("inactive filesystem output stream must not be locked")
            .write(bytes);
        if result.is_ok() {
            let effect = self
                .prepared_effect
                .lock()
                .expect("filesystem output stream effect lock poisoned")
                .take();
            self.active.store(true, Ordering::Release);
            let cancellation = tokio_util::sync::CancellationToken::new();
            self.cancel
                .lock()
                .expect("filesystem output stream cancellation lock poisoned")
                .replace(cancellation.clone());
            let inner = Arc::clone(&self.inner);
            let active = Arc::clone(&self.active);
            let ready = Arc::clone(&self.ready);
            tokio::spawn(async move {
                let _effect = ActiveFilesystemEffect {
                    active,
                    ready,
                    _effect: effect,
                };
                let mut inner = inner.lock().await;
                tokio::select! {
                    _ = inner.ready() => {}
                    _ = cancellation.cancelled() => inner.cancel().await,
                }
            });
        } else {
            self.clear_unused_effect();
        }
        result
    }

    fn flush(&mut self) -> StreamResult<()> {
        if self.is_active() {
            // Native file streams buffer only in the active write task, so their
            // flush operation is a successful no-op while that task is pending.
            Ok(())
        } else {
            self.inner
                .try_lock()
                .expect("inactive filesystem output stream must not be locked")
                .flush()
        }
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if self.is_active() {
            Ok(0)
        } else {
            self.inner
                .try_lock()
                .expect("inactive filesystem output stream must not be locked")
                .check_write()
        }
    }

    async fn cancel(&mut self) {
        let was_active = self.is_active();
        if was_active
            && let Some(cancellation) = self
                .cancel
                .lock()
                .expect("filesystem output stream cancellation lock poisoned")
                .as_ref()
        {
            cancellation.cancel();
        }
        self.wait_until_ready().await;
        if !was_active {
            self.inner.lock().await.cancel().await;
        }
        self.clear_unused_effect();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Pollable for CoordinatedFileOutputStream {
    async fn ready(&mut self) {
        self.wait_until_ready().await;
        self.inner.lock().await.ready().await;
    }
}

impl Drop for CoordinatedFileOutputStream {
    fn drop(&mut self) {
        if let Some(cancellation) = self
            .cancel
            .lock()
            .expect("filesystem output stream cancellation lock poisoned")
            .as_ref()
        {
            cancellation.cancel();
        }
        self.clear_unused_effect();
    }
}

async fn acquire_lifecycle_lock(path: &Path) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = LIFECYCLE_LOCKS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .expect("agent filesystem lifecycle lock registry poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(path).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    lock.lock_owned().await
}

async fn verify_fresh_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }

    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", path, error)
    })?;
    let empty = entries
        .next_entry()
        .await
        .map_err(|error| FilesystemStorageError::io("verify empty runtime directory", path, error))?
        .is_none();
    if !empty {
        return Err(FilesystemStorageError::verification(
            "verify empty runtime directory",
            path,
        ));
    }

    Ok(())
}

async fn rollback_creation(
    path: &Path,
    creation_error: FilesystemStorageError,
    cleanup_retry: &RetryConfig,
) -> FilesystemStorageError {
    match remove_and_verify(path, "roll back runtime directory", cleanup_retry).await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

async fn rollback_owned_filesystem(
    filesystem: AgentFilesystem,
    creation_error: FilesystemStorageError,
) -> FilesystemStorageError {
    match filesystem.close_and_delete().await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

async fn remove_and_verify(
    path: &Path,
    operation: &'static str,
    cleanup_retry: &RetryConfig,
) -> Result<(), FilesystemStorageError> {
    let mut retry = RetryState::new(cleanup_retry);
    loop {
        retry.start_attempt();
        match remove_and_verify_once(path, operation).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !retry.failed_attempt().await {
                    return Err(error);
                }
            }
        }
    }
}

async fn remove_and_verify_once(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Ok(_) => {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }

    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            operation, path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }
}

fn remove_and_verify_blocking(path: &Path) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|error| {
                FilesystemStorageError::cleanup_io("delete runtime directory", path, error)
            })?;
        }
        Ok(_) => {
            std::fs::remove_file(path).map_err(|error| {
                FilesystemStorageError::cleanup_io("delete runtime directory", path, error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilesystemStorageError::cleanup_io(
                "delete runtime directory",
                path,
                error,
            ));
        }
    }

    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            "delete runtime directory",
            path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(
            "delete runtime directory",
            path,
            error,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use test_r::test;

    struct DelayedOutputStream {
        ready: Arc<tokio::sync::Semaphore>,
        cancelled: Arc<AtomicBool>,
    }

    #[async_trait]
    impl OutputStream for DelayedOutputStream {
        fn write(&mut self, _bytes: Bytes) -> StreamResult<()> {
            Ok(())
        }

        fn flush(&mut self) -> StreamResult<()> {
            Ok(())
        }

        fn check_write(&mut self) -> StreamResult<usize> {
            Ok(1024)
        }

        async fn cancel(&mut self) {
            self.cancelled.store(true, Ordering::Release);
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[async_trait]
    impl Pollable for DelayedOutputStream {
        async fn ready(&mut self) {
            self.ready
                .acquire()
                .await
                .expect("test readiness semaphore closed")
                .forget();
        }
    }

    fn agent_id() -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId::from_agent_name_string(ComponentId::new(), "agent").unwrap(),
        )
    }

    #[test]
    async fn deterministic_creation_removes_existing_garbage() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings);
        let id = agent_id();

        let filesystem = filesystems.create_fresh(&id).await.unwrap();
        let path = filesystem.path().to_path_buf();
        tokio::fs::write(path.join("garbage"), b"old")
            .await
            .unwrap();
        drop(filesystem);
        tokio::fs::create_dir_all(&path).await.unwrap();
        tokio::fs::write(path.join("garbage"), b"old")
            .await
            .unwrap();

        let filesystem = filesystems.create_fresh(&id).await.unwrap();
        assert!(!filesystem.path().join("garbage").exists());
        filesystem.close_and_delete().await.unwrap();
        assert!(!path.exists());
    }

    #[test]
    async fn seal_rejects_new_effects_without_waiting_for_existing_effects() {
        let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default());
        let filesystem = filesystems.create_fresh(&agent_id()).await.unwrap();
        let runtime = filesystem.runtime();
        let effect = runtime.begin_effect().await.unwrap();

        filesystem.seal();
        assert!(runtime.begin_effect().await.is_err());
        assert!(filesystem.path().exists());
        drop(effect);
        filesystem.close_and_delete().await.unwrap();
    }

    #[test]
    async fn close_deletes_without_waiting_for_an_existing_effect() {
        let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default());
        let filesystem = filesystems.create_fresh(&agent_id()).await.unwrap();
        let path = filesystem.path().to_path_buf();
        let effect = filesystem.runtime().begin_effect().await.unwrap();

        filesystem.close_and_delete().await.unwrap();
        assert!(!path.exists());
        drop(effect);
    }

    #[test]
    async fn deterministic_creation_is_exclusive_for_the_full_owner_lifetime() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings);
        let id = agent_id();
        let first = filesystems.create_fresh(&id).await.unwrap();
        tokio::fs::write(first.path().join("owned"), b"first")
            .await
            .unwrap();

        let second = tokio::spawn({
            let filesystems = filesystems.clone();
            let id = id.clone();
            async move { filesystems.create_fresh(&id).await }
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());
        assert!(first.path().join("owned").exists());

        first.close_and_delete().await.unwrap();
        let second = second.await.unwrap().unwrap();
        assert!(!second.path().join("owned").exists());
        second.close_and_delete().await.unwrap();
    }

    #[test]
    async fn append_effect_ends_at_native_completion_without_waiting_for_guest_polling() {
        let runtime = AgentFilesystemRuntime::new();
        let ready = Arc::new(tokio::sync::Semaphore::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
            ready: Arc::clone(&ready),
            cancelled,
        }));
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"first")).unwrap();

        assert!(stream.is_active());
        assert!(stream.write(Bytes::from_static(b"second")).is_err());
        let next_effect = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!next_effect.is_finished());

        ready.add_permits(1);
        let next_effect = next_effect.await.unwrap().unwrap();
        assert!(!stream.is_active());
        drop(next_effect);
    }

    #[test]
    async fn positioned_effect_does_not_wait_for_active_append() {
        let runtime = AgentFilesystemRuntime::new();
        let append = runtime.begin_append_effect().await.unwrap();

        let positioned = runtime.begin_effect().await.unwrap();

        drop(positioned);
        drop(append);
    }

    #[test]
    async fn cancelling_p2_stream_forwards_cancellation_and_releases_the_effect() {
        let runtime = AgentFilesystemRuntime::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
            ready: Arc::new(tokio::sync::Semaphore::new(0)),
            cancelled: Arc::clone(&cancelled),
        }));
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"write")).unwrap();

        let next_append = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!next_append.is_finished());

        stream.cancel().await;

        assert!(cancelled.load(Ordering::Acquire));
        assert!(!stream.is_active());
        assert!(next_append.await.unwrap().is_ok());
    }

    #[test]
    async fn dropping_p2_stream_requests_cancellation() {
        let runtime = AgentFilesystemRuntime::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
            ready: Arc::new(tokio::sync::Semaphore::new(0)),
            cancelled: Arc::clone(&cancelled),
        }));
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"write")).unwrap();

        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !cancelled.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stream drop did not request cancellation");
        assert!(runtime.begin_append_effect().await.is_ok());
    }
}
