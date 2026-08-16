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
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::OwnedRwLockReadGuard;
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::{StreamError, StreamResult};

const FILESYSTEM_RUNTIME_SEALED: usize = 1 << (usize::BITS - 1);
const FILESYSTEM_RUNTIME_ACTIVE_EFFECTS: usize = !FILESYSTEM_RUNTIME_SEALED;

impl AgentFilesystemRuntime {
    pub(crate) async fn begin_effect(&self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin().await)
    }

    pub(crate) async fn begin_append_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin_append().await)
    }

    pub(crate) async fn begin_path_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin_path().await)
    }

    pub(super) async fn begin_update_effect(
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

    pub(super) fn seal(&self) {
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
    pub(crate) async fn begin(self) -> AgentFilesystemEffectLease {
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: None,
        }
    }

    pub(crate) async fn begin_append(self) -> AgentFilesystemEffectLease {
        let guard = Arc::clone(&self.inner.append).lock_owned().await;
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: Some(guard),
            _namespace_guard: None,
        }
    }

    pub(crate) async fn begin_path(self) -> AgentFilesystemEffectLease {
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        let namespace_guard = Arc::clone(&self.inner.namespace).lock_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: Some(namespace_guard),
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
