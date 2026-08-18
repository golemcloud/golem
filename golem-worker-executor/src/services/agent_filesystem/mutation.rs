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
use async_trait::async_trait;
use bytes::Bytes;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::FileExt;
use fs_set_times::{SetTimes as _, SystemTimeSpec};
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::sync::OwnedRwLockReadGuard;
use wasmtime_wasi::filesystem::{Descriptor, Dir, File, OpenMode};
use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode;
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::runtime::spawn_blocking;
use wasmtime_wasi::{DirPerms, FilePerms};
use wasmtime_wasi::{StreamError, StreamResult};

pub(super) const FILESYSTEM_RUNTIME_SEALED: usize = 1 << (usize::BITS - 1);
const FILESYSTEM_RUNTIME_ACTIVE_EFFECTS: usize = !FILESYSTEM_RUNTIME_SEALED;

pub(crate) async fn run_blocking_filesystem_mutation<L, F, R>(lease: Arc<L>, operation: F) -> R
where
    L: Send + Sync + 'static,
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    spawn_blocking(move || {
        let _lease = lease;
        operation()
    })
    .await
}

pub(crate) fn sync_descriptor(descriptor: &Descriptor, data_only: bool) -> std::io::Result<()> {
    let result = match descriptor {
        Descriptor::File(file) => {
            if data_only {
                file.file.sync_data()
            } else {
                file.file.sync_all()
            }
        }
        Descriptor::Dir(directory) => {
            let directory = directory.dir.open(std::path::Component::CurDir)?;
            if data_only {
                directory.sync_data()
            } else {
                directory.sync_all()
            }
        }
    };
    #[cfg(windows)]
    if matches!(descriptor, Descriptor::File(_))
        && result.as_ref().is_err_and(|error| {
            error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
        })
    {
        return Ok(());
    }
    result
}

pub(crate) fn resize_file(file: &File, size: u64) -> std::io::Result<()> {
    file.file.set_len(size)
}

pub(crate) fn set_descriptor_times(
    descriptor: &Descriptor,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> std::io::Result<()> {
    let accessed = accessed.map(SystemTimeSpec::Absolute);
    let modified = modified.map(SystemTimeSpec::Absolute);
    match descriptor {
        Descriptor::File(file) => file.file.set_times(accessed, modified),
        Descriptor::Dir(directory) => directory.dir.set_times(accessed, modified),
    }
}

pub(crate) fn set_path_times(
    directory: &Dir,
    path: &str,
    follow: bool,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> std::io::Result<()> {
    let accessed = accessed.map(|time| {
        cap_fs_ext::SystemTimeSpec::Absolute(cap_std::time::SystemTime::from_std(time))
    });
    let modified = modified.map(|time| {
        cap_fs_ext::SystemTimeSpec::Absolute(cap_std::time::SystemTime::from_std(time))
    });
    if follow {
        cap_fs_ext::DirExt::set_times(directory.dir.as_ref(), path, accessed, modified)
    } else {
        directory.dir.set_symlink_times(path, accessed, modified)
    }
}

pub(crate) fn create_directory(directory: &Dir, path: &str) -> std::io::Result<()> {
    directory.dir.create_dir(path)
}

pub(crate) fn hard_link(
    source: &Dir,
    source_path: &str,
    destination: &Dir,
    destination_path: &str,
) -> std::io::Result<()> {
    source
        .dir
        .hard_link(source_path, &destination.dir, destination_path)
}

pub(crate) fn rename(
    source: &Dir,
    source_path: &str,
    destination: &Dir,
    destination_path: &str,
) -> std::io::Result<()> {
    source
        .dir
        .rename(source_path, &destination.dir, destination_path)
}

pub(crate) fn remove_directory(directory: &Dir, path: &str) -> std::io::Result<()> {
    directory.dir.remove_dir(path)
}

pub(crate) fn unlink_file(directory: &Dir, path: &str) -> std::io::Result<()> {
    directory.dir.remove_file_or_symlink(path)
}

pub(crate) fn symlink(directory: &Dir, source: &str, destination: &str) -> std::io::Result<()> {
    directory.dir.symlink(source, destination)
}

#[derive(Clone, Copy)]
pub(crate) enum NativeMutationGuestError {
    Invalid,
    NotDirectory,
    NotPermitted,
    Unsupported,
}

pub(crate) fn validate_resize(file: &File) -> Result<(), NativeMutationGuestError> {
    if file.perms.contains(FilePerms::WRITE) {
        Ok(())
    } else {
        Err(NativeMutationGuestError::NotPermitted)
    }
}

pub(crate) fn validate_descriptor_times(
    descriptor: &Descriptor,
) -> Result<(), NativeMutationGuestError> {
    let permitted = match descriptor {
        Descriptor::File(file) => file.perms.contains(FilePerms::WRITE),
        Descriptor::Dir(directory) => directory.perms.contains(DirPerms::MUTATE),
    };
    if permitted {
        Ok(())
    } else {
        Err(NativeMutationGuestError::NotPermitted)
    }
}

pub(crate) fn validate_directory_mutation(directory: &Dir) -> Result<(), NativeMutationGuestError> {
    if directory.perms.contains(DirPerms::MUTATE) {
        Ok(())
    } else {
        Err(NativeMutationGuestError::NotPermitted)
    }
}

pub(crate) fn validate_two_directory_mutation(
    source: &Dir,
    destination: &Dir,
) -> Result<(), NativeMutationGuestError> {
    validate_directory_mutation(source)?;
    validate_directory_mutation(destination)
}

#[derive(Clone, Copy)]
pub(crate) struct NativeOpenOptions {
    pub create: bool,
    pub directory: bool,
    pub exclusive: bool,
    pub truncate: bool,
    pub follow: bool,
    pub read: bool,
    pub write: bool,
}

pub(crate) fn validate_open(
    directory: &Dir,
    options: NativeOpenOptions,
    unsupported_sync_flags: bool,
) -> Result<(), NativeMutationGuestError> {
    if !directory.perms.contains(DirPerms::READ) {
        return Err(NativeMutationGuestError::NotPermitted);
    }
    if !directory.perms.contains(DirPerms::MUTATE)
        && (options.create || options.truncate || options.write)
    {
        return Err(NativeMutationGuestError::NotPermitted);
    }
    if unsupported_sync_flags {
        return Err(NativeMutationGuestError::Unsupported);
    }
    if options.directory && (options.create || options.exclusive || options.truncate) {
        return Err(NativeMutationGuestError::Invalid);
    }
    let opens_for_write = options.create || options.truncate || options.write;
    if opens_for_write && !directory.file_perms.contains(FilePerms::WRITE) {
        return Err(NativeMutationGuestError::NotPermitted);
    }
    Ok(())
}

pub(crate) enum NativeOpenResult {
    Descriptor(Descriptor),
    #[cfg(windows)]
    IsDirectory,
    NotDirectory,
}

pub(crate) fn open(
    directory: &Dir,
    path: &str,
    options: NativeOpenOptions,
) -> std::io::Result<NativeOpenResult> {
    let mut native = cap_std::fs::OpenOptions::new();
    native.maybe_dir(true);
    let mut mode = OpenMode::empty();

    if options.create {
        if options.exclusive {
            native.create_new(true);
        } else {
            native.create(true);
        }
        native.write(true);
        mode |= OpenMode::WRITE;
    }
    if options.truncate {
        native.truncate(true).write(true);
        mode |= OpenMode::WRITE;
    }
    if options.read {
        native.read(true);
        mode |= OpenMode::READ;
    }
    if options.write {
        native.write(true);
        mode |= OpenMode::WRITE;
    } else {
        native.read(true);
        mode |= OpenMode::READ;
    }
    native.follow(if options.follow {
        FollowSymlinks::Yes
    } else {
        FollowSymlinks::No
    });

    let opened = directory.dir.open_with(path, &native)?;
    let child_path = directory.path.join(path);
    if opened.metadata()?.is_dir() {
        #[cfg(windows)]
        if options.write {
            return Ok(NativeOpenResult::IsDirectory);
        }
        Ok(NativeOpenResult::Descriptor(Descriptor::Dir(Dir::new(
            cap_std::fs::Dir::from_std_file(opened.into_std()),
            directory.perms,
            directory.file_perms,
            mode,
            false,
            child_path.clone(),
        ))))
    } else if options.directory {
        Ok(NativeOpenResult::NotDirectory)
    } else {
        Ok(NativeOpenResult::Descriptor(Descriptor::File(File::new(
            opened,
            directory.file_perms,
            mode,
            false,
            child_path,
        ))))
    }
}

impl AgentFilesystemRuntime {
    pub(crate) async fn begin_effect(&self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        self.admit_effect()?.begin().await
    }

    pub(crate) async fn begin_append_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        self.admit_effect()?.begin_append().await
    }

    pub(crate) async fn begin_path_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        self.admit_effect()?.begin_path().await
    }

    pub(crate) async fn begin_update_effect(
        &self,
    ) -> Result<AgentFilesystemUpdateEffectLease, wasmtime::Error> {
        let admission = self.admit_effect()?;
        let operation_guard = Arc::clone(&self.inner.operations).write_owned().await;
        Ok(AgentFilesystemUpdateEffectLease {
            _inner: Arc::new(AgentFilesystemUpdateEffectLeaseInner {
                _admission: admission,
                _operation_guard: operation_guard,
            }),
        })
    }

    pub(crate) fn admit_effect(&self) -> Result<AgentFilesystemEffectAdmission, wasmtime::Error> {
        let mut state = self.inner.state.load(Ordering::Acquire);
        loop {
            if state & FILESYSTEM_RUNTIME_SEALED != 0 {
                return Err(wasmtime::Error::msg("agent filesystem is closing"));
            }
            let active_effects = state & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS;
            let next = active_effects
                .checked_add(1)
                .expect("agent filesystem effect count overflowed");
            match self.inner.state.compare_exchange_weak(
                state,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => state = observed,
            }
        }
        Ok(AgentFilesystemEffectAdmission {
            inner: Arc::clone(&self.inner),
        })
    }

    pub(crate) fn seal(&self) {
        self.inner
            .state
            .fetch_or(FILESYSTEM_RUNTIME_SEALED, Ordering::AcqRel);
    }

    pub(super) async fn drain(&self) {
        while self.has_active_effects() {
            let drained = self.inner.drained.notified();
            if !self.has_active_effects() {
                break;
            }
            drained.await;
        }
    }

    pub(super) fn has_active_effects(&self) -> bool {
        self.inner.state.load(Ordering::Acquire) & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS != 0
    }
}

impl AgentFilesystemRuntimeInner {
    fn finish_effect(&self) {
        let previous = self.state.fetch_sub(1, Ordering::AcqRel);
        let previous_active = previous & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS;
        debug_assert!(previous_active > 0);
        if previous_active == 1 {
            self.drained.notify_waiters();
        }
    }
}

#[derive(Debug)]
pub(crate) struct AgentFilesystemEffectLease {
    _admission: AgentFilesystemEffectAdmission,
    _operation_guard: OwnedRwLockReadGuard<()>,
    _append_guard: Option<OwnedMutexGuard<()>>,
    _namespace_guard: Option<OwnedMutexGuard<()>>,
}

#[derive(Clone)]
pub(crate) struct AgentFilesystemUpdateEffectLease {
    _inner: Arc<AgentFilesystemUpdateEffectLeaseInner>,
}

struct AgentFilesystemUpdateEffectLeaseInner {
    _admission: AgentFilesystemEffectAdmission,
    _operation_guard: tokio::sync::OwnedRwLockWriteGuard<()>,
}

pub(crate) struct AgentFilesystemEffectAdmission {
    inner: Arc<AgentFilesystemRuntimeInner>,
}

impl Drop for AgentFilesystemEffectAdmission {
    fn drop(&mut self) {
        self.inner.finish_effect();
    }
}

impl AgentFilesystemEffectAdmission {
    pub(crate) async fn begin(self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        self.ensure_open()?;
        Ok(AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: None,
        })
    }

    pub(crate) async fn begin_append(self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        let guard = Arc::clone(&self.inner.append).lock_owned().await;
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        self.ensure_open()?;
        Ok(AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: Some(guard),
            _namespace_guard: None,
        })
    }

    pub(crate) async fn begin_path(self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        let namespace_guard = Arc::clone(&self.inner.namespace).lock_owned().await;
        self.ensure_open()?;
        Ok(AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: Some(namespace_guard),
        })
    }

    fn ensure_open(&self) -> Result<(), wasmtime::Error> {
        if self.inner.state.load(Ordering::Acquire) & FILESYSTEM_RUNTIME_SEALED != 0 {
            Err(wasmtime::Error::msg("agent filesystem is closing"))
        } else {
            Ok(())
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

#[derive(Clone, Copy, Debug)]
pub(crate) enum FilesystemStreamMode {
    Position(u64),
    Append,
}

pub(crate) struct ClassifiedFileOutputStream {
    writer: Arc<dyn FilesystemStreamWriter>,
    filesystem_runtime: AgentFilesystemRuntime,
    mode: FilesystemStreamMode,
    state: FilesystemOutputState,
    prepared_effect: std::sync::Mutex<Option<AgentFilesystemEffectLease>>,
}

enum FilesystemOutputState {
    Ready,
    Waiting {
        task: tokio::task::JoinHandle<(usize, ClassifiedFilesystemStreamResult)>,
        cancellation: tokio_util::sync::CancellationToken,
    },
    Error(ClassifiedFilesystemStreamFailure),
    Closed,
}

struct FilesystemWriteAttempt {
    written: usize,
    result: std::io::Result<()>,
}

#[derive(Debug)]
enum ClassifiedFilesystemStreamFailure {
    Guest(ErrorCode),
    Raw(std::io::Error),
    Trap(String),
}

type ClassifiedFilesystemStreamResult = Result<(), ClassifiedFilesystemStreamFailure>;

#[derive(Debug)]
struct ClassifiedFilesystemErrorCode(ErrorCode);

impl std::fmt::Display for ClassifiedFilesystemErrorCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "filesystem error: {:?}", self.0)
    }
}

impl std::error::Error for ClassifiedFilesystemErrorCode {}

#[async_trait]
trait FilesystemStreamWriter: Send + Sync {
    async fn write(
        &self,
        mode: FilesystemStreamMode,
        contents: Bytes,
        start: usize,
        effect: Arc<AgentFilesystemEffectLease>,
    ) -> FilesystemWriteAttempt;
}

struct NativeFilesystemStreamWriter {
    file: File,
}

#[async_trait]
impl FilesystemStreamWriter for NativeFilesystemStreamWriter {
    async fn write(
        &self,
        mode: FilesystemStreamMode,
        contents: Bytes,
        start: usize,
        effect: Arc<AgentFilesystemEffectLease>,
    ) -> FilesystemWriteAttempt {
        let file = Arc::clone(&self.file.file);
        spawn_blocking(move || {
            let _effect = effect;
            let suffix = &contents[start..];
            let result = match mode {
                FilesystemStreamMode::Position(position) => file.write_at(suffix, position),
                FilesystemStreamMode::Append => {
                    let mut file = file.as_ref();
                    file.seek(SeekFrom::End(0)).and_then(|_| file.write(suffix))
                }
            };
            match result {
                Ok(written) => FilesystemWriteAttempt {
                    written,
                    result: Ok(()),
                },
                Err(error) => FilesystemWriteAttempt {
                    written: 0,
                    result: Err(error),
                },
            }
        })
        .await
    }
}

impl ClassifiedFileOutputStream {
    pub(crate) fn new(
        file: File,
        filesystem_runtime: AgentFilesystemRuntime,
        mode: FilesystemStreamMode,
    ) -> Self {
        Self::new_with_writer(
            Arc::new(NativeFilesystemStreamWriter { file }),
            filesystem_runtime,
            mode,
        )
    }

    fn new_with_writer(
        writer: Arc<dyn FilesystemStreamWriter>,
        filesystem_runtime: AgentFilesystemRuntime,
        mode: FilesystemStreamMode,
    ) -> Self {
        Self {
            writer,
            filesystem_runtime,
            mode,
            state: FilesystemOutputState::Ready,
            prepared_effect: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn new_for_test<W>(
        writer: Arc<W>,
        filesystem_runtime: AgentFilesystemRuntime,
        mode: FilesystemStreamMode,
    ) -> Self
    where
        W: FilesystemStreamWriter + 'static,
    {
        Self::new_with_writer(writer, filesystem_runtime, mode)
    }

    pub(crate) fn into_dyn(self) -> DynOutputStream {
        Box::new(self)
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self.state, FilesystemOutputState::Waiting { .. })
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

    async fn wait_until_ready(&mut self) {
        let state = std::mem::replace(&mut self.state, FilesystemOutputState::Closed);
        let task = match state {
            FilesystemOutputState::Waiting { task, .. } => task,
            state => {
                self.state = state;
                return;
            }
        };

        self.state = match task.await {
            Ok((written, result)) => {
                if let FilesystemStreamMode::Position(position) = &mut self.mode {
                    let Some(next_position) = u64::try_from(written)
                        .ok()
                        .and_then(|written| position.checked_add(written))
                    else {
                        let _ = self
                            .filesystem_runtime
                            .classify_mutation_failure::<ErrorCode>(
                                MutationFailure::Infrastructure(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "filesystem stream position overflowed",
                                )),
                                MutationEffect::Unknown,
                            )
                            .await;
                        return self.set_trap("filesystem stream position overflowed");
                    };
                    *position = next_position;
                }
                match result {
                    Ok(()) => FilesystemOutputState::Ready,
                    Err(error) => FilesystemOutputState::Error(error),
                }
            }
            Err(error) => {
                let message = format!("filesystem stream write task failed: {error}");
                let _ = self
                    .filesystem_runtime
                    .classify_mutation_failure::<ErrorCode>(
                        MutationFailure::Infrastructure(std::io::Error::other(message.clone())),
                        MutationEffect::Unknown,
                    )
                    .await;
                FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Trap(message))
            }
        };
    }

    fn set_trap(&mut self, message: &'static str) {
        self.state = FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Trap(
            message.to_string(),
        ));
    }

    fn take_error(&mut self) -> StreamResult<usize> {
        match std::mem::replace(&mut self.state, FilesystemOutputState::Closed) {
            FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Guest(error)) => Err(
                StreamError::LastOperationFailed(ClassifiedFilesystemErrorCode(error).into()),
            ),
            FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Raw(error)) => {
                Err(StreamError::LastOperationFailed(error.into()))
            }
            FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Trap(message)) => {
                Err(StreamError::Trap(wasmtime::Error::msg(message)))
            }
            _ => unreachable!("filesystem stream error state changed unexpectedly"),
        }
    }

    fn cancel_active_write(&self) {
        if let FilesystemOutputState::Waiting { cancellation, .. } = &self.state {
            cancellation.cancel();
        }
    }
}

#[async_trait]
impl OutputStream for ClassifiedFileOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        match self.state {
            FilesystemOutputState::Ready => {}
            FilesystemOutputState::Closed => return Err(StreamError::Closed),
            FilesystemOutputState::Waiting { .. } | FilesystemOutputState::Error(_) => {
                return Err(StreamError::Trap(wasmtime::Error::msg(
                    "write not permitted: check_write not called first",
                )));
            }
        }

        let effect = self
            .prepared_effect
            .lock()
            .expect("filesystem output stream effect lock poisoned")
            .take()
            .ok_or_else(|| {
                StreamError::Trap(wasmtime::Error::msg(
                    "filesystem output stream write has no effect lease",
                ))
            })?;
        let writer = Arc::clone(&self.writer);
        let filesystem_runtime = self.filesystem_runtime.clone();
        let mode = self.mode;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let write_cancellation = cancellation.clone();
        self.state = FilesystemOutputState::Waiting {
            task: tokio::spawn(async move {
                run_classified_filesystem_stream_write(
                    writer.as_ref(),
                    &filesystem_runtime,
                    mode,
                    bytes,
                    effect,
                    write_cancellation,
                )
                .await
            }),
            cancellation,
        };
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        match self.state {
            FilesystemOutputState::Ready | FilesystemOutputState::Waiting { .. } => Ok(()),
            FilesystemOutputState::Closed => Err(StreamError::Closed),
            FilesystemOutputState::Error(_) => self.take_error().map(|_| ()),
        }
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        match self.state {
            FilesystemOutputState::Ready => Ok(1024 * 1024),
            FilesystemOutputState::Waiting { .. } => Ok(0),
            FilesystemOutputState::Closed => Err(StreamError::Closed),
            FilesystemOutputState::Error(_) => self.take_error(),
        }
    }

    async fn cancel(&mut self) {
        self.cancel_active_write();
        if self.is_active() {
            self.wait_until_ready().await;
        }
        self.state = FilesystemOutputState::Closed;
        self.clear_unused_effect();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Pollable for ClassifiedFileOutputStream {
    async fn ready(&mut self) {
        self.wait_until_ready().await;
    }
}

impl Drop for ClassifiedFileOutputStream {
    fn drop(&mut self) {
        self.cancel_active_write();
        self.clear_unused_effect();
    }
}

async fn run_classified_filesystem_stream_write<W: FilesystemStreamWriter + ?Sized>(
    writer: &W,
    filesystem_runtime: &AgentFilesystemRuntime,
    mode: FilesystemStreamMode,
    contents: Bytes,
    effect: AgentFilesystemEffectLease,
    cancellation: tokio_util::sync::CancellationToken,
) -> (usize, ClassifiedFilesystemStreamResult) {
    let effect = Arc::new(effect);
    let started = Instant::now();
    let mut completed = 0usize;
    let mut failed_attempts = 0usize;

    while completed < contents.len() {
        if cancellation.is_cancelled() {
            return (completed, Ok(()));
        }
        let attempt_mode = match mode {
            FilesystemStreamMode::Position(position) => {
                let Some(position) = u64::try_from(completed)
                    .ok()
                    .and_then(|completed| position.checked_add(completed))
                else {
                    return invalidate_filesystem_stream(
                        filesystem_runtime,
                        completed,
                        "filesystem stream write offset overflowed",
                    )
                    .await;
                };
                FilesystemStreamMode::Position(position)
            }
            FilesystemStreamMode::Append => FilesystemStreamMode::Append,
        };
        let attempt = writer
            .write(
                attempt_mode,
                contents.clone(),
                completed,
                Arc::clone(&effect),
            )
            .await;
        let remaining = contents.len() - completed;
        if attempt.written > remaining {
            return invalidate_filesystem_stream(
                filesystem_runtime,
                completed,
                "filesystem stream writer reported more bytes than requested",
            )
            .await;
        }
        completed += attempt.written;

        let (error, effect) = match attempt.result {
            Ok(()) if attempt.written != 0 => {
                if cancellation.is_cancelled() {
                    return (completed, Ok(()));
                }
                continue;
            }
            Ok(()) => (
                std::io::Error::from(std::io::ErrorKind::WriteZero),
                proven_write_progress_effect(completed),
            ),
            Err(error) => {
                let effect = native_write_failure_effect(&error, completed);
                (error, effect)
            }
        };
        failed_attempts += 1;
        let raw_os_error = error.raw_os_error();
        let error_kind = error.kind();
        let error_message = error.to_string();
        let decision = filesystem_runtime
            .classify_mutation_failure_for::<ErrorCode>(
                MutationOperation::Write,
                MutationFailure::Io(error),
                effect,
            )
            .await;

        match decision {
            MutationDecision::BoundedRetry if cancellation.is_cancelled() => {
                return (completed, Ok(()));
            }
            MutationDecision::BoundedRetry
                if failed_attempts < 2 && started.elapsed() <= Duration::from_millis(250) => {}
            MutationDecision::BoundedRetry => {
                let error = raw_os_error.map_or_else(
                    || std::io::Error::new(error_kind, error_message),
                    std::io::Error::from_raw_os_error,
                );
                return (
                    completed,
                    Err(ClassifiedFilesystemStreamFailure::Raw(error)),
                );
            }
            MutationDecision::PreserveRaw => {
                let error = raw_os_error.map_or_else(
                    || std::io::Error::new(error_kind, error_message),
                    std::io::Error::from_raw_os_error,
                );
                return (
                    completed,
                    Err(ClassifiedFilesystemStreamFailure::Raw(error)),
                );
            }
            MutationDecision::PreserveGuest(error) => {
                return (
                    completed,
                    Err(ClassifiedFilesystemStreamFailure::Guest(error)),
                );
            }
            MutationDecision::Quota => {
                return (
                    completed,
                    Err(ClassifiedFilesystemStreamFailure::Guest(ErrorCode::Quota)),
                );
            }
            MutationDecision::InsufficientSpace => {
                return (
                    completed,
                    Err(ClassifiedFilesystemStreamFailure::Guest(
                        ErrorCode::InsufficientSpace,
                    )),
                );
            }
            MutationDecision::Success => return (completed, Ok(())),
            MutationDecision::Invalidate => {
                return (
                    completed,
                    Err(ClassifiedFilesystemStreamFailure::Trap(
                        "agent filesystem mutation invalidated the runtime".to_string(),
                    )),
                );
            }
        }
    }

    (completed, Ok(()))
}

async fn invalidate_filesystem_stream(
    filesystem_runtime: &AgentFilesystemRuntime,
    completed: usize,
    message: &'static str,
) -> (usize, ClassifiedFilesystemStreamResult) {
    let effect = u64::try_from(completed).map_or(MutationEffect::Unknown, |bytes| {
        if bytes == 0 {
            MutationEffect::ProvenNoEffect
        } else {
            MutationEffect::KnownCompletedPrefix { bytes }
        }
    });
    let _ = filesystem_runtime
        .classify_mutation_failure::<ErrorCode>(
            MutationFailure::Infrastructure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            )),
            effect,
        )
        .await;
    (
        completed,
        Err(ClassifiedFilesystemStreamFailure::Trap(message.to_string())),
    )
}

pub(crate) fn classified_filesystem_stream_error_code(
    error: &wasmtime::Error,
) -> Option<ErrorCode> {
    error
        .downcast_ref::<ClassifiedFilesystemErrorCode>()
        .map(|error| error.0)
}

#[cfg(test)]
mod classified_stream_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct InjectedFilesystemWriter {
        attempts: std::sync::Mutex<VecDeque<FilesystemWriteAttempt>>,
        suffixes: std::sync::Mutex<Vec<Vec<u8>>>,
        started: Option<Arc<tokio::sync::Notify>>,
        release: Option<Arc<tokio::sync::Semaphore>>,
    }

    impl InjectedFilesystemWriter {
        fn new(attempts: impl IntoIterator<Item = FilesystemWriteAttempt>) -> Self {
            Self {
                attempts: std::sync::Mutex::new(attempts.into_iter().collect()),
                suffixes: std::sync::Mutex::new(Vec::new()),
                started: None,
                release: None,
            }
        }

        fn delayed_first(
            attempts: impl IntoIterator<Item = FilesystemWriteAttempt>,
            started: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Semaphore>,
        ) -> Self {
            Self {
                attempts: std::sync::Mutex::new(attempts.into_iter().collect()),
                suffixes: std::sync::Mutex::new(Vec::new()),
                started: Some(started),
                release: Some(release),
            }
        }

        fn suffixes(&self) -> Vec<Vec<u8>> {
            self.suffixes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl FilesystemStreamWriter for InjectedFilesystemWriter {
        async fn write(
            &self,
            _mode: FilesystemStreamMode,
            contents: Bytes,
            start: usize,
            _effect: Arc<AgentFilesystemEffectLease>,
        ) -> FilesystemWriteAttempt {
            let attempt_index = {
                let mut suffixes = self.suffixes.lock().unwrap();
                let attempt_index = suffixes.len();
                suffixes.push(contents[start..].to_vec());
                attempt_index
            };
            if attempt_index == 0 {
                if let Some(started) = &self.started {
                    started.notify_one();
                }
                if let Some(release) = &self.release {
                    release.acquire().await.unwrap().forget();
                }
            }
            self.attempts.lock().unwrap().pop_front().unwrap()
        }
    }

    fn success(written: usize) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Ok(()),
        }
    }

    fn failure(written: usize, errno: i32) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Err(std::io::Error::from_raw_os_error(errno)),
        }
    }

    #[test_r::test]
    fn two_directory_mutation_allows_different_authority_sets() {
        let source_root = tempfile::TempDir::new().unwrap();
        let destination_root = tempfile::TempDir::new().unwrap();
        let source = Dir::new(
            cap_std::fs::Dir::open_ambient_dir(source_root.path(), cap_std::ambient_authority())
                .unwrap(),
            DirPerms::all(),
            FilePerms::all(),
            OpenMode::READ | OpenMode::WRITE,
            false,
            source_root.path().to_path_buf(),
        );
        let destination = Dir::new(
            cap_std::fs::Dir::open_ambient_dir(
                destination_root.path(),
                cap_std::ambient_authority(),
            )
            .unwrap(),
            DirPerms::MUTATE,
            FilePerms::READ,
            OpenMode::READ | OpenMode::WRITE,
            false,
            destination_root.path().to_path_buf(),
        );

        assert!(validate_two_directory_mutation(&source, &destination).is_ok());
    }

    #[test_r::test]
    async fn cancelled_blocking_mutation_keeps_effect_lease_until_native_completion() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let mutation = tokio::spawn({
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let lease = Arc::new(runtime.begin_effect().await.unwrap());
            async move {
                run_blocking_filesystem_mutation(lease, move || {
                    started.store(true, Ordering::Release);
                    while !release.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                })
                .await
            }
        });
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }

        mutation.abort();
        let _ = mutation.await;
        let update = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_update_effect().await }
        });
        tokio::task::yield_now().await;
        let update_finished_while_native_operation_was_running = update.is_finished();
        release.store(true, Ordering::Release);
        assert!(update.await.unwrap().is_ok());
        assert!(!update_finished_while_native_operation_was_running);
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_retries_transient_before_effect() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let writer = InjectedFilesystemWriter::new([failure(0, libc::EAGAIN), success(5)]);

        let (written, result) = run_classified_filesystem_stream_write(
            &writer,
            &runtime,
            FilesystemStreamMode::Position(7),
            Bytes::from_static(b"hello"),
            runtime.begin_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert_eq!(written, 5);
        assert!(result.is_ok());
        assert_eq!(writer.suffixes(), [b"hello".to_vec(), b"hello".to_vec()]);
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_retries_only_unwritten_suffix() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let writer = InjectedFilesystemWriter::new([failure(2, libc::EBUSY), success(3)]);

        let (written, result) = run_classified_filesystem_stream_write(
            &writer,
            &runtime,
            FilesystemStreamMode::Position(11),
            Bytes::from_static(b"hello"),
            runtime.begin_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert_eq!(written, 5);
        assert!(result.is_ok());
        assert_eq!(writer.suffixes(), [b"hello".to_vec(), b"llo".to_vec()]);
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_preserves_raw_error_after_retry_exhaustion() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let writer =
            InjectedFilesystemWriter::new([failure(0, libc::EBUSY), failure(0, libc::EBUSY)]);

        let (written, result) = run_classified_filesystem_stream_write(
            &writer,
            &runtime,
            FilesystemStreamMode::Append,
            Bytes::from_static(b"hello"),
            runtime.begin_append_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert_eq!(written, 0);
        assert!(matches!(
            result,
            Err(ClassifiedFilesystemStreamFailure::Raw(error))
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
        assert_eq!(writer.suffixes(), [b"hello".to_vec(), b"hello".to_vec()]);
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_terminal_failure_traps_and_seals_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let writer = InjectedFilesystemWriter::new([failure(2, libc::EIO)]);

        let (written, result) = run_classified_filesystem_stream_write(
            &writer,
            &runtime,
            FilesystemStreamMode::Position(0),
            Bytes::from_static(b"hello"),
            runtime.begin_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert_eq!(written, 2);
        assert!(matches!(
            result,
            Err(ClassifiedFilesystemStreamFailure::Trap(_))
        ));
        assert_eq!(writer.suffixes(), [b"hello".to_vec()]);
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_interruption_has_unknown_effect() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let writer = InjectedFilesystemWriter::new([failure(0, libc::EINTR)]);

        let (written, result) = run_classified_filesystem_stream_write(
            &writer,
            &runtime,
            FilesystemStreamMode::Position(0),
            Bytes::from_static(b"hello"),
            runtime.begin_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert_eq!(written, 0);
        assert!(matches!(
            result,
            Err(ClassifiedFilesystemStreamFailure::Trap(_))
        ));
        assert_eq!(writer.suffixes(), [b"hello".to_vec()]);
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_maps_classified_storage_exhaustion() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let quota_runtime = AgentFilesystemRuntime::new_for_test_with_observations(
            Some(AgentFilesystemUsage {
                allocated_bytes: 50,
                filesystem_objects: 10,
            }),
            Some(ResolvedAgentFilesystemLimits {
                allocated_bytes: 50,
                filesystem_objects: 10,
                filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
            }),
            capacity,
        );
        let quota_writer = InjectedFilesystemWriter::new([failure(0, libc::ENOSPC)]);
        let (_, quota_result) = run_classified_filesystem_stream_write(
            &quota_writer,
            &quota_runtime,
            FilesystemStreamMode::Position(0),
            Bytes::from_static(b"hello"),
            quota_runtime.begin_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            quota_result,
            Err(ClassifiedFilesystemStreamFailure::Guest(ErrorCode::Quota))
        ));

        let physical_runtime =
            AgentFilesystemRuntime::new_for_test_with_observations(None, None, capacity);
        let physical_writer = InjectedFilesystemWriter::new([failure(0, libc::ENOSPC)]);
        let (_, physical_result) = run_classified_filesystem_stream_write(
            &physical_writer,
            &physical_runtime,
            FilesystemStreamMode::Position(0),
            Bytes::from_static(b"hello"),
            physical_runtime.begin_effect().await.unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            physical_result,
            Err(ClassifiedFilesystemStreamFailure::Guest(
                ErrorCode::InsufficientSpace
            ))
        ));
    }

    #[test_r::test]
    async fn p2_filesystem_stream_idle_readiness_preserves_write_capacity() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let writer = Arc::new(InjectedFilesystemWriter::new([]));
        let mut stream = ClassifiedFileOutputStream::new_for_test(
            writer,
            runtime,
            FilesystemStreamMode::Position(0),
        );

        stream.ready().await;

        assert_eq!(stream.check_write().unwrap(), 1024 * 1024);
    }

    #[test_r::test]
    async fn p2_filesystem_stream_pending_flush_and_check_preserve_effect() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let writer = Arc::new(InjectedFilesystemWriter::delayed_first(
            [success(5)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let mut stream = ClassifiedFileOutputStream::new_for_test(
            writer,
            runtime.clone(),
            FilesystemStreamMode::Append,
        );
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"hello")).unwrap();
        started.notified().await;

        assert!(stream.flush().is_ok());
        assert_eq!(stream.check_write().unwrap(), 0);
        let next_append = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!next_append.is_finished());

        release.add_permits(1);
        stream.ready().await;
        stream.ready().await;
        assert_eq!(stream.check_write().unwrap(), 1024 * 1024);
        assert!(next_append.await.unwrap().is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_filesystem_stream_readiness_preserves_error_until_flush() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(None, None, capacity);
        let writer = Arc::new(InjectedFilesystemWriter::new([failure(0, libc::ENOSPC)]));
        let mut stream = ClassifiedFileOutputStream::new_for_test(
            writer,
            runtime.clone(),
            FilesystemStreamMode::Position(0),
        );
        stream.prepare_effect(runtime.begin_effect().await.unwrap());
        stream.write(Bytes::from_static(b"hello")).unwrap();
        stream.ready().await;

        stream.ready().await;
        let error = stream.flush().unwrap_err();
        assert!(matches!(
            error,
            StreamError::LastOperationFailed(error)
                if classified_filesystem_stream_error_code(&error)
                    == Some(ErrorCode::InsufficientSpace)
        ));
        assert!(matches!(stream.check_write(), Err(StreamError::Closed)));
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn cancelling_p2_filesystem_stream_keeps_lease_until_native_completion() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let writer = Arc::new(InjectedFilesystemWriter::delayed_first(
            [failure(0, libc::EAGAIN), success(5)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let mut stream = ClassifiedFileOutputStream::new_for_test(
            Arc::clone(&writer),
            runtime.clone(),
            FilesystemStreamMode::Append,
        );
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"hello")).unwrap();
        started.notified().await;

        let cancellation = tokio::spawn(async move {
            stream.cancel().await;
            stream
        });
        let next_append = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!cancellation.is_finished());
        assert!(!next_append.is_finished());

        release.add_permits(1);
        let _stream = cancellation.await.unwrap();
        assert!(next_append.await.unwrap().is_ok());
        assert_eq!(writer.suffixes(), [b"hello".to_vec()]);
    }

    #[test_r::test]
    async fn dropping_p2_filesystem_stream_keeps_lease_until_native_completion() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let writer = Arc::new(InjectedFilesystemWriter::delayed_first(
            [success(2), success(3)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let mut stream = ClassifiedFileOutputStream::new_for_test(
            Arc::clone(&writer),
            runtime.clone(),
            FilesystemStreamMode::Append,
        );
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"hello")).unwrap();
        started.notified().await;
        drop(stream);

        let next_append = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!next_append.is_finished());

        release.add_permits(1);
        assert!(next_append.await.unwrap().is_ok());
        assert_eq!(writer.suffixes(), [b"hello".to_vec()]);
    }
}
