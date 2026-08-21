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

use super::failure::{
    FILESYSTEM_MUTATION_MAX_ATTEMPTS, FILESYSTEM_MUTATION_RETRY_TIMEOUT, MutationDecision,
    MutationEffect, native_write_failure_effect, proven_write_progress_effect,
};
use super::postcondition::{
    MutationPostcondition, PathObjectType, PathState, SymlinkState, TimesState, ambient_path_times,
    create_directory_postcondition, descriptor_state, descriptor_times, link_postcondition,
    open_postcondition, path_state, path_state_with_follow, path_times, remove_postcondition,
    rename_postcondition, resize_postcondition, restored_times_postcondition,
    symlink_postcondition, symlink_state, times_postcondition,
};
use super::*;
use async_trait::async_trait;
use bytes::Bytes;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::FileExt;
use fs_set_times::{SetTimes as _, SystemTimeSpec, set_symlink_times};
use golem_common::model::component::AgentFilePermissions;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;
use tokio::sync::OwnedRwLockReadGuard;
use wasmtime_wasi::filesystem::{Descriptor, Dir, File, OpenMode};
use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode;
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::runtime::spawn_blocking;
use wasmtime_wasi::{DirPerms, FilePerms};
use wasmtime_wasi::{StreamError, StreamResult};

#[allow(
    dead_code,
    reason = "the semantic mutation seam is exercised through its interface-level tests"
)]
mod protocol;

#[allow(
    unused_imports,
    reason = "the mutation module defines the semantic host-adapter boundary"
)]
pub(crate) use protocol::{
    AdmittedFilesystemWrite, AgentFilesystemMutationError, AgentFilesystemStreamSetupAdmission,
    AgentFilesystemWriteMode, AgentFilesystemWriter,
};
use protocol::{AgentFilesystemMutations, AgentFilesystemWriteCompletion};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestedTime {
    NoChange,
    Now,
    Timestamp { seconds: i128, nanoseconds: u32 },
}

// Terminal flag: once set, this runtime never admits another filesystem effect.
const FILESYSTEM_RUNTIME_SEALED: usize = 1 << (usize::BITS - 1);
// Temporary flag: reject new effects while an existing set drains for a consistent observation.
const FILESYSTEM_RUNTIME_ADMISSION_PAUSED: usize = 1 << (usize::BITS - 2);
// All remaining low bits form the active-effect reference count.
const FILESYSTEM_RUNTIME_ACTIVE_EFFECTS: usize =
    !(FILESYSTEM_RUNTIME_SEALED | FILESYSTEM_RUNTIME_ADMISSION_PAUSED);
// Bound short-effect observation lag while avoiding continuous quota probes at the executor epoch cadence.
const FILESYSTEM_USAGE_COMPLETION_DELAY: std::time::Duration = std::time::Duration::from_millis(10);
const FILESYSTEM_USAGE_SUSTAINED_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(100);

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

impl AgentFilesystemRuntime {
    pub(crate) fn is_read_only(&self, path: &Path) -> bool {
        self.is_read_only_path(path, true)
    }

    pub(crate) fn is_read_only_path(&self, path: &Path, follow_final_symlink: bool) -> bool {
        let path = initial_files::resolve_policy_path(path, follow_final_symlink);
        self.runtime_state
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .iter()
            .any(|(initial_path, file)| {
                file.permissions == AgentFilePermissions::ReadOnly
                    && initial_files::resolve_policy_path(initial_path, true) == path
            })
    }

    pub(crate) fn contains_read_only_path(&self, path: &Path, follow_final_symlink: bool) -> bool {
        let path = initial_files::resolve_policy_path(path, follow_final_symlink);
        self.runtime_state
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .iter()
            .any(|(initial_path, file)| {
                let initial_path = initial_files::resolve_policy_path(initial_path, true);
                file.permissions == AgentFilePermissions::ReadOnly
                    && (initial_path == path || initial_path.starts_with(&path))
            })
    }
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

pub(crate) fn restore_ambient_path_times(
    path: &Path,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> std::io::Result<()> {
    set_symlink_times(
        path,
        accessed.map(SystemTimeSpec::Absolute),
        modified.map(SystemTimeSpec::Absolute),
    )
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeMutationGuestError {
    Invalid,
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

pub(crate) fn validate_open_flags(
    options: NativeOpenOptions,
    unsupported_sync_flags: bool,
) -> Result<(), NativeMutationGuestError> {
    if unsupported_sync_flags {
        return Err(NativeMutationGuestError::Unsupported);
    }
    if options.directory && (options.create || options.exclusive || options.truncate) {
        return Err(NativeMutationGuestError::Invalid);
    }
    Ok(())
}

pub(crate) fn validate_open_capabilities(
    directory: &Dir,
    options: NativeOpenOptions,
) -> Result<(), NativeMutationGuestError> {
    if !directory.perms.contains(DirPerms::READ) {
        return Err(NativeMutationGuestError::NotPermitted);
    }
    if !directory.perms.contains(DirPerms::MUTATE)
        && (options.create || options.truncate || options.write)
    {
        return Err(NativeMutationGuestError::NotPermitted);
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
    #[allow(
        dead_code,
        reason = "the runtime exposes the semantic mutation boundary"
    )]
    pub(crate) fn mutations(&self) -> AgentFilesystemMutations {
        AgentFilesystemMutations::new(self.clone())
    }

    pub(crate) async fn begin_effect(&self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        self.admit_effect()?.begin().await
    }

    #[cfg(test)]
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
        let admission = loop {
            let mut admission_resumed = Box::pin(self.runtime_state.admission_resumed.notified());
            admission_resumed.as_mut().enable();
            match self.admit_effect() {
                Ok(admission) => break admission,
                Err(error) => {
                    if self.runtime_state.is_sealed() {
                        return Err(error);
                    }
                    if self.runtime_state.effect_admission_is_paused() {
                        admission_resumed.await;
                    }
                }
            }
        };
        let operation_guard = Arc::clone(&self.runtime_state.operations)
            .write_owned()
            .await;
        Ok(AgentFilesystemUpdateEffectLease {
            _lease_state: Arc::new(AgentFilesystemUpdateEffectLeaseState {
                _admission: admission,
                _operation_guard: operation_guard,
            }),
        })
    }

    pub(super) async fn begin_guest_update_effect(
        &self,
    ) -> Result<AgentFilesystemUpdateEffectLease, wasmtime::Error> {
        let admission = self.admit_effect()?;
        let operation_guard = Arc::clone(&self.runtime_state.operations)
            .write_owned()
            .await;
        admission.ensure_open()?;
        Ok(AgentFilesystemUpdateEffectLease {
            _lease_state: Arc::new(AgentFilesystemUpdateEffectLeaseState {
                _admission: admission,
                _operation_guard: operation_guard,
            }),
        })
    }

    pub(crate) fn admit_effect(&self) -> Result<AgentFilesystemEffectAdmission, wasmtime::Error> {
        self.runtime_state
            .try_admit_effect()
            .map_err(|error| match error {
                AgentFilesystemEffectAdmissionError::Sealed => {
                    wasmtime::Error::msg("agent filesystem is closing")
                }
                AgentFilesystemEffectAdmissionError::Paused => {
                    wasmtime::Error::msg("agent filesystem resource window is transitioning")
                }
            })?;
        self.runtime_state
            .usage_effect_epoch
            .fetch_add(1, Ordering::Release);
        self.runtime_state.schedule_usage_sampling();
        Ok(AgentFilesystemEffectAdmission {
            runtime_state: Arc::clone(&self.runtime_state),
        })
    }

    pub(crate) fn seal(&self) {
        self.runtime_state.seal();
    }

    pub(crate) fn pause_effect_admission(&self) -> AgentFilesystemEffectAdmissionPause {
        self.runtime_state.pause_effect_admission();
        AgentFilesystemEffectAdmissionPause {
            runtime_state: Arc::clone(&self.runtime_state),
        }
    }

    pub(crate) fn seal_if_no_active_effects(&self) -> bool {
        self.runtime_state.seal_if_no_active_effects()
    }

    pub(crate) async fn drain(&self) {
        while self.has_active_effects() {
            let mut drained = Box::pin(self.runtime_state.drained.notified());
            drained.as_mut().enable();
            if !self.has_active_effects() {
                break;
            }
            drained.await;
        }
    }

    pub(crate) async fn wait_for_usage_completion_debounce(&self) {
        debug_assert!(!self.has_active_effects());
        tokio::time::sleep(FILESYSTEM_USAGE_COMPLETION_DELAY).await;
    }

    pub(crate) fn has_active_effects(&self) -> bool {
        self.runtime_state.has_active_effects()
    }

    #[cfg(test)]
    pub(crate) fn effect_admission_is_paused(&self) -> bool {
        self.runtime_state.effect_admission_is_paused()
    }

    pub(crate) fn last_effect_completion_millis(&self) -> u64 {
        self.runtime_state
            .last_effect_completion_millis
            .load(Ordering::Acquire)
    }
}

enum AgentFilesystemEffectAdmissionError {
    Sealed,
    Paused,
}

impl AgentFilesystemRuntimeState {
    pub(super) fn is_sealed(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) & FILESYSTEM_RUNTIME_SEALED != 0
    }

    fn effect_admission_is_paused(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) & FILESYSTEM_RUNTIME_ADMISSION_PAUSED != 0
    }

    fn has_active_effects(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS != 0
    }

    fn try_admit_effect(&self) -> Result<(), AgentFilesystemEffectAdmissionError> {
        let mut lifecycle = self.lifecycle.load(Ordering::Acquire);
        loop {
            if lifecycle & FILESYSTEM_RUNTIME_SEALED != 0 {
                return Err(AgentFilesystemEffectAdmissionError::Sealed);
            }
            if lifecycle & FILESYSTEM_RUNTIME_ADMISSION_PAUSED != 0 {
                return Err(AgentFilesystemEffectAdmissionError::Paused);
            }
            let active_effects = lifecycle & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS;
            let next = active_effects
                .checked_add(1)
                .filter(|next| next & !FILESYSTEM_RUNTIME_ACTIVE_EFFECTS == 0)
                .expect("agent filesystem effect count overflowed");
            match self.lifecycle.compare_exchange_weak(
                lifecycle,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => lifecycle = observed,
            }
        }
    }

    fn seal(&self) {
        self.lifecycle
            .fetch_or(FILESYSTEM_RUNTIME_SEALED, Ordering::AcqRel);
    }

    fn pause_effect_admission(&self) {
        let previous = self
            .lifecycle
            .fetch_or(FILESYSTEM_RUNTIME_ADMISSION_PAUSED, Ordering::AcqRel);
        assert_eq!(
            previous & FILESYSTEM_RUNTIME_ADMISSION_PAUSED,
            0,
            "agent filesystem effect admission is already paused"
        );
    }

    fn resume_effect_admission(&self) {
        let previous = self
            .lifecycle
            .fetch_and(!FILESYSTEM_RUNTIME_ADMISSION_PAUSED, Ordering::AcqRel);
        debug_assert_ne!(previous & FILESYSTEM_RUNTIME_ADMISSION_PAUSED, 0);
    }

    fn seal_if_no_active_effects(&self) -> bool {
        self.lifecycle
            .compare_exchange(
                0,
                FILESYSTEM_RUNTIME_SEALED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn finish_effect(self: &Arc<Self>) {
        self.usage_effect_epoch.fetch_add(1, Ordering::Release);
        let previous = self.lifecycle.fetch_sub(1, Ordering::AcqRel);
        let previous_active = previous & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS;
        debug_assert!(previous_active > 0);
        if previous_active == 1 {
            let completed_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let completed_at = u64::try_from(completed_at).unwrap_or(u64::MAX);
            let _ = self.last_effect_completion_millis.fetch_update(
                Ordering::Release,
                Ordering::Acquire,
                |previous| Some(completed_at.max(previous.saturating_add(1))),
            );
            self.drained.notify_waiters();
        }
    }

    pub(super) fn schedule_usage_sampling(self: &Arc<Self>) {
        // Observer replacement is rare; this lock is held only for the presence check.
        // The atomic flag below coalesces all active effects onto one background sampler.
        if self
            .usage_observer
            .lock()
            .expect("agent filesystem usage-observer lock poisoned")
            .is_none()
            || self.usage_sampling.swap(true, Ordering::AcqRel)
        {
            return;
        }

        let runtime = AgentFilesystemRuntime {
            runtime_state: Arc::clone(self),
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.usage_sampling.store(false, Ordering::Release);
            return;
        };
        handle.spawn(async move {
            let mut sampled_effect_epoch = 0;
            let mut drained = Box::pin(runtime.runtime_state.drained.notified());
            drained.as_mut().enable();
            if runtime.has_active_effects() {
                tokio::select! {
                    _ = tokio::time::sleep(FILESYSTEM_USAGE_COMPLETION_DELAY) => {}
                    _ = &mut drained => {
                        tokio::time::sleep(FILESYSTEM_USAGE_COMPLETION_DELAY).await;
                    }
                }
            } else {
                tokio::time::sleep(FILESYSTEM_USAGE_COMPLETION_DELAY).await;
            }
            loop {
                if !runtime.usage_observer_is_active() {
                    break;
                }
                let effect_epoch = runtime
                    .runtime_state
                    .usage_effect_epoch
                    .load(Ordering::Acquire);
                if let Err(error) = runtime.observe_usage_for_billing().await {
                    tracing::error!(error = %error, "Failed to observe filesystem usage during an active resource window");
                    runtime.invalidate_runtime().await;
                    break;
                }
                sampled_effect_epoch = effect_epoch;
                if !runtime.has_active_effects()
                    && runtime
                        .runtime_state
                        .usage_effect_epoch
                        .load(Ordering::Acquire)
                        == sampled_effect_epoch
                {
                    break;
                }

                let mut drained = Box::pin(runtime.runtime_state.drained.notified());
                drained.as_mut().enable();
                if runtime.has_active_effects() {
                    tokio::select! {
                        _ = tokio::time::sleep(FILESYSTEM_USAGE_SUSTAINED_INTERVAL) => {}
                        _ = &mut drained => {
                            tokio::time::sleep(FILESYSTEM_USAGE_COMPLETION_DELAY).await;
                        }
                    }
                } else {
                    tokio::time::sleep(FILESYSTEM_USAGE_COMPLETION_DELAY).await;
                }
            }

            runtime
                .runtime_state
                .finish_usage_sampling(sampled_effect_epoch);
        });
    }

    pub(super) fn finish_usage_sampling(self: &Arc<Self>, sampled_effect_epoch: u64) {
        self.usage_sampling.store(false, Ordering::Release);
        let observer_active = self
            .usage_observer
            .lock()
            .expect("agent filesystem usage-observer lock poisoned")
            .as_ref()
            .is_some_and(|observer| observer.is_active());
        if observer_active
            && (self.has_active_effects()
                || self.usage_effect_epoch.load(Ordering::Acquire) != sampled_effect_epoch)
        {
            self.schedule_usage_sampling();
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
    _lease_state: Arc<AgentFilesystemUpdateEffectLeaseState>,
}

struct AgentFilesystemUpdateEffectLeaseState {
    _admission: AgentFilesystemEffectAdmission,
    _operation_guard: tokio::sync::OwnedRwLockWriteGuard<()>,
}

pub(crate) struct AgentFilesystemEffectAdmission {
    runtime_state: Arc<AgentFilesystemRuntimeState>,
}

#[must_use = "effect admission resumes when the pause is dropped"]
pub(crate) struct AgentFilesystemEffectAdmissionPause {
    runtime_state: Arc<AgentFilesystemRuntimeState>,
}

impl Drop for AgentFilesystemEffectAdmissionPause {
    fn drop(&mut self) {
        self.runtime_state.resume_effect_admission();
        self.runtime_state.admission_resumed.notify_waiters();
    }
}

impl Drop for AgentFilesystemEffectAdmission {
    fn drop(&mut self) {
        self.runtime_state.finish_effect();
    }
}

impl AgentFilesystemEffectAdmission {
    pub(crate) async fn begin(self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        let operation_guard = Arc::clone(&self.runtime_state.operations)
            .read_owned()
            .await;
        self.ensure_open()?;
        Ok(AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: None,
        })
    }

    pub(crate) async fn begin_append(self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        let guard = Arc::clone(&self.runtime_state.append).lock_owned().await;
        let operation_guard = Arc::clone(&self.runtime_state.operations)
            .read_owned()
            .await;
        self.ensure_open()?;
        Ok(AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: Some(guard),
            _namespace_guard: None,
        })
    }

    pub(crate) async fn begin_path(self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        let operation_guard = Arc::clone(&self.runtime_state.operations)
            .read_owned()
            .await;
        let namespace_guard = Arc::clone(&self.runtime_state.namespace).lock_owned().await;
        self.ensure_open()?;
        Ok(AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: Some(namespace_guard),
        })
    }

    fn ensure_open(&self) -> Result<(), wasmtime::Error> {
        if self.runtime_state.is_sealed() {
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
    writer: AgentFilesystemWriter,
    state: FilesystemOutputState,
}

enum FilesystemOutputState {
    Ready,
    Waiting {
        completion: AgentFilesystemWriteCompletion,
        cancellation: tokio_util::sync::CancellationToken,
    },
    Error(ClassifiedFilesystemStreamFailure),
    Closed,
}

#[derive(Debug)]
enum ClassifiedFilesystemStreamFailure {
    Guest(ErrorCode),
    Raw(std::io::Error),
    Trap(String),
}

#[derive(Debug)]
struct ClassifiedFilesystemErrorCode(ErrorCode);

impl std::fmt::Display for ClassifiedFilesystemErrorCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "filesystem error: {:?}", self.0)
    }
}

impl std::error::Error for ClassifiedFilesystemErrorCode {}

impl ClassifiedFileOutputStream {
    pub(crate) fn new(
        file: File,
        filesystem_runtime: AgentFilesystemRuntime,
        mode: FilesystemStreamMode,
    ) -> Self {
        let mode = match mode {
            FilesystemStreamMode::Position(position) => {
                AgentFilesystemWriteMode::Position(position)
            }
            FilesystemStreamMode::Append => AgentFilesystemWriteMode::Append,
        };
        Self {
            writer: filesystem_runtime.mutations().writer(file, mode),
            state: FilesystemOutputState::Ready,
        }
    }

    pub(crate) fn into_dyn(self) -> DynOutputStream {
        Box::new(self)
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self.state, FilesystemOutputState::Waiting { .. })
    }

    async fn wait_until_ready(&mut self) {
        let state = std::mem::replace(&mut self.state, FilesystemOutputState::Closed);
        let completion = match state {
            FilesystemOutputState::Waiting { completion, .. } => completion,
            state => {
                self.state = state;
                return;
            }
        };

        self.state = match completion.await {
            Err(AgentFilesystemMutationError::Guest(_)) => FilesystemOutputState::Error(
                ClassifiedFilesystemStreamFailure::Guest(ErrorCode::NotPermitted),
            ),
            Ok(_) | Err(AgentFilesystemMutationError::Cancelled { .. }) => {
                FilesystemOutputState::Ready
            }
            Err(AgentFilesystemMutationError::Native { error, .. }) => {
                FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Raw(
                    error.into_io_error(),
                ))
            }
            Err(AgentFilesystemMutationError::QuotaExhausted { .. }) => {
                FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Guest(
                    ErrorCode::Quota,
                ))
            }
            Err(AgentFilesystemMutationError::InsufficientSpace { .. }) => {
                FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Guest(
                    ErrorCode::InsufficientSpace,
                ))
            }
            Err(AgentFilesystemMutationError::RuntimeInvalidated { .. }) => {
                FilesystemOutputState::Error(ClassifiedFilesystemStreamFailure::Trap(
                    "agent filesystem mutation invalidated the runtime".to_string(),
                ))
            }
        };
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

        let cancellation = tokio_util::sync::CancellationToken::new();
        let completion = self
            .writer
            .admit(bytes)
            .map_err(|_| {
                StreamError::Trap(wasmtime::Error::msg(
                    "agent filesystem mutation invalidated the runtime",
                ))
            })?
            .execute(cancellation.clone());
        self.state = FilesystemOutputState::Waiting {
            completion,
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
    }
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;

    fn writable_file(path: &std::path::Path) -> File {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .unwrap();
        File::new(
            cap_std::fs::File::from_std(file),
            FilePerms::all(),
            OpenMode::READ | OpenMode::WRITE,
            false,
            path.to_path_buf(),
        )
    }

    #[test]
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

    #[test]
    fn open_flag_validation_is_independent_of_filesystem_capabilities() {
        let invalid_directory_open = NativeOpenOptions {
            create: true,
            directory: true,
            exclusive: false,
            truncate: false,
            follow: false,
            read: true,
            write: false,
        };

        assert_eq!(
            validate_open_flags(invalid_directory_open, true),
            Err(NativeMutationGuestError::Unsupported)
        );
        assert_eq!(
            validate_open_flags(invalid_directory_open, false),
            Err(NativeMutationGuestError::Invalid)
        );
    }

    #[test]
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

    #[test]
    async fn p2_filesystem_stream_idle_readiness_preserves_write_capacity() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("idle-readiness");
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut stream = ClassifiedFileOutputStream::new(
            writable_file(&path),
            runtime,
            FilesystemStreamMode::Position(0),
        );

        stream.ready().await;

        assert_eq!(stream.check_write().unwrap(), 1024 * 1024);
    }

    #[test]
    async fn p2_filesystem_stream_positions_sequential_writes() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("positioned-writes");
        std::fs::write(&path, b"0123456789").unwrap();
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut stream = ClassifiedFileOutputStream::new(
            writable_file(&path),
            runtime,
            FilesystemStreamMode::Position(2),
        );
        stream.write(Bytes::from_static(b"ab")).unwrap();
        stream.ready().await;
        stream.write(Bytes::from_static(b"cd")).unwrap();
        stream.ready().await;

        assert_eq!(stream.check_write().unwrap(), 1024 * 1024);
        assert_eq!(std::fs::read(path).unwrap(), b"01abcd6789");
    }

    #[test]
    async fn p2_filesystem_stream_appends_sequential_writes() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("append-writes");
        std::fs::write(&path, b"start").unwrap();
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut stream = ClassifiedFileOutputStream::new(
            writable_file(&path),
            runtime,
            FilesystemStreamMode::Append,
        );
        stream.write(Bytes::from_static(b"-one")).unwrap();
        stream.ready().await;
        stream.write(Bytes::from_static(b"-two")).unwrap();
        stream.ready().await;

        assert_eq!(stream.check_write().unwrap(), 1024 * 1024);
        assert_eq!(std::fs::read(path).unwrap(), b"start-one-two");
    }
}
