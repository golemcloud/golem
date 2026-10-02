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

//! A blob storage for tests that records each call and follows a script for each call.
//!
//! This module is test code, and it compiles only for tests.

use async_trait::async_trait;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob,
    NormalizedBlobPath, PutIfAbsent,
};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// What the storage does with one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Script {
    /// Passes the call to the in-memory storage.
    Pass,
    /// Gives an error and does not pass the call.
    Refuse,
    /// Passes the call, and then gives an error in place of its answer.
    LoseTheAnswer,
    /// Passes the call, and then never answers.
    NeverAnswer,
    /// Waits until the test opens the gate of the storage, and then passes the call.
    WaitForGate,
    /// Waits for the time, and then passes the call.
    Delay(std::time::Duration),
    /// Waits for the time, and then gives an error and does not pass the call.
    RefuseAfter(std::time::Duration),
    /// Gives no blob to a read of a whole blob or to the source of a copy, as a delete after a
    /// listing does. Each other call passes.
    Vanish,
    /// Passes the call, and then answers a write if absent with `AlreadyExists`, as a new try of a
    /// call whose first answer was lost does. Each other call passes.
    AnswerAlreadyExists,
    /// Waits until the test gives the storage one step, and then passes the call, or refuses it
    /// when `refuse` is true. A `late` write or delete gives an error at its step, as a call that
    /// got no answer within its deadline, and it reaches the storage when the test lands it.
    Step { refuse: bool, late: bool },
}

/// A rule that gives the script of a call from its operation label and its path.
type Rule = Box<dyn Fn(&str, &Path) -> Script + Send + Sync>;

/// A blob storage that records the operation label and the path of each call, and does with each
/// call what its rule gives.
pub(crate) struct ScriptedBlobStorage {
    inner: Arc<InMemoryBlobStorage>,
    rule: Rule,
    calls: Mutex<Vec<(&'static str, Box<Path>)>>,
    gate: CancellationToken,
    /// The steps that the test gave and that no call took yet.
    steps: Semaphore,
    /// The calls that wait for a step.
    waiting: AtomicUsize,
    /// The calls that took a step and ended, or that dropped after they took a step.
    stepped: AtomicUsize,
    /// The operation label and the path of each call that took a step, in the order of the steps.
    took: Mutex<Vec<(&'static str, Box<Path>)>>,
    /// The landings that the test gave and that no late call took yet.
    landings: Arc<Semaphore>,
    /// The late calls that reached the storage.
    landed: Arc<AtomicUsize>,
}

impl ScriptedBlobStorage {
    pub(crate) fn new(
        inner: Arc<InMemoryBlobStorage>,
        rule: impl Fn(&str, &Path) -> Script + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            rule: Box::new(rule),
            calls: Mutex::new(Vec::new()),
            gate: CancellationToken::new(),
            steps: Semaphore::new(0),
            waiting: AtomicUsize::new(0),
            stepped: AtomicUsize::new(0),
            took: Mutex::new(Vec::new()),
            landings: Arc::new(Semaphore::new(0)),
            landed: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Lets each call that waits for the gate, and each later such call, go on.
    pub(crate) fn open_gate(&self) {
        self.gate.cancel();
    }

    /// Lets one call that waits for a step, now or later, go on.
    pub(crate) fn step(&self) {
        self.steps.add_permits(1);
    }

    /// Takes back one step that no call took, and tells whether one was there.
    pub(crate) fn take_back_step(&self) -> bool {
        self.steps
            .try_acquire()
            .map(tokio::sync::SemaphorePermit::forget)
            .is_ok()
    }

    /// Gives the number of calls that wait for a step.
    pub(crate) fn waiting_steps(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    /// Gives the number of calls that took a step and ended.
    pub(crate) fn stepped(&self) -> usize {
        self.stepped.load(Ordering::SeqCst)
    }

    /// Gives the operation label and the path of each call that took a step, in the order of the
    /// steps. Two calls can wait for a step at one time, and the one that waited first takes it.
    pub(crate) fn took(&self) -> Vec<(&'static str, String)> {
        self.took
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(op_label, path)| (*op_label, path.display().to_string()))
            .collect()
    }

    /// Waits until the test gives the call a step. The call counts as waiting until it takes the
    /// step or drops, and it counts as stepped when the returned guard drops.
    async fn wait_for_step(&self, op_label: &'static str, path: &Path) -> Stepped<'_> {
        let waiting = Waiting::new(&self.waiting);
        if let Ok(permit) = self.steps.acquire().await {
            permit.forget();
        }
        // The call is in the steps that were taken before it stops waiting, so a test never sees
        // a call that neither waits nor took its step.
        self.took
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((op_label, path.into()));
        drop(waiting);
        Stepped(&self.stepped)
    }

    /// Lets one late call, now or later, reach the storage.
    pub(crate) fn land_late(&self) {
        self.landings.add_permits(1);
    }

    /// Gives the number of late calls that reached the storage.
    pub(crate) fn landed(&self) -> usize {
        self.landed.load(Ordering::SeqCst)
    }

    /// Takes one step for a late call: the caller gets an error, and the call reaches the storage
    /// in a task when the test lands it.
    async fn late<T>(
        &self,
        op_label: &'static str,
        path: &Path,
        landing: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) -> anyhow::Result<T> {
        self.record(op_label, path);
        let stepped = self.wait_for_step(op_label, path).await;
        let (landings, landed) = (self.landings.clone(), self.landed.clone());
        tokio::spawn(async move {
            if let Ok(permit) = landings.acquire().await {
                permit.forget();
            }
            let _ = landing.await;
            landed.fetch_add(1, Ordering::SeqCst);
        });
        drop(stepped);
        Err(anyhow::anyhow!(
            "the call got no answer within its deadline"
        ))
    }

    /// Gives the operation label and the path of each call, in the order of the calls.
    pub(crate) fn calls(&self) -> Vec<(&'static str, String)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(op_label, path)| (*op_label, path.display().to_string()))
            .collect()
    }

    fn record(&self, op_label: &'static str, path: &Path) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((op_label, path.into()));
    }

    async fn answer<T>(
        &self,
        op_label: &'static str,
        path: &Path,
        call: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        self.follow((self.rule)(op_label, path), op_label, path, call)
            .await
    }

    /// Records the call and does what the script says. The rule runs one time for each call.
    async fn follow<T>(
        &self,
        script: Script,
        op_label: &'static str,
        path: &Path,
        call: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        self.record(op_label, path);
        match script {
            Script::Pass => call.await,
            Script::Refuse => Err(anyhow::anyhow!("the storage refused the call")),
            Script::LoseTheAnswer => {
                call.await?;
                Err(anyhow::anyhow!("the answer of the call was lost"))
            }
            Script::NeverAnswer => {
                call.await?;
                std::future::pending().await
            }
            Script::WaitForGate => {
                self.gate.cancelled().await;
                call.await
            }
            Script::Vanish | Script::AnswerAlreadyExists => call.await,
            Script::Delay(time) => {
                tokio::time::sleep(time).await;
                call.await
            }
            Script::RefuseAfter(time) => {
                tokio::time::sleep(time).await;
                Err(anyhow::anyhow!("the storage refused the call"))
            }
            Script::Step { refuse, .. } => {
                let _stepped = self.wait_for_step(op_label, path).await;
                if refuse {
                    Err(anyhow::anyhow!("the storage refused the call"))
                } else {
                    call.await
                }
            }
        }
    }
}

/// Counts a call that waits for a step, until the guard drops.
struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    fn new(waiting: &'a AtomicUsize) -> Self {
        waiting.fetch_add(1, Ordering::SeqCst);
        Self(waiting)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Counts a call that took a step as stepped when the guard drops.
struct Stepped<'a>(&'a AtomicUsize);

impl Drop for Stepped<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Debug for ScriptedBlobStorage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ScriptedBlobStorage")
    }
}

#[async_trait]
impl BlobStorageBackend for ScriptedBlobStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        match (self.rule)(op_label, path) {
            Script::Vanish => {
                self.record(op_label, path);
                Ok(None)
            }
            script => {
                self.follow(
                    script,
                    op_label,
                    path,
                    self.inner
                        .get_raw_at(target_label, op_label, namespace, path),
                )
                .await
            }
        }
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<golem_service_base::storage::blob::BlobRangeStream>> {
        self.answer(
            op_label,
            path,
            self.inner
                .get_range_stream_at(target_label, op_label, namespace, path, offset, length),
        )
        .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.answer(
            op_label,
            path,
            self.inner
                .get_raw_slice_at(target_label, op_label, namespace, path, start, end),
        )
        .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.answer(
            op_label,
            path,
            self.inner
                .get_metadata_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let script = (self.rule)(op_label, path);
        if let Script::Step { late: true, .. } = script {
            let (inner, owned_path, owned_data) =
                (self.inner.clone(), path.to_path_buf(), data.to_vec());
            return self
                .late(op_label, path, async move {
                    inner
                        .put_raw(target_label, op_label, namespace, &owned_path, &owned_data)
                        .await
                })
                .await;
        }
        self.follow(
            script,
            op_label,
            path,
            self.inner
                .put_raw_at(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        let script = (self.rule)(op_label, path);
        if let Script::Step { late: true, .. } = script {
            let (inner, owned_path, owned_data) =
                (self.inner.clone(), path.to_path_buf(), data.to_vec());
            return self
                .late(op_label, path, async move {
                    inner
                        .put_raw_if_absent(
                            target_label,
                            op_label,
                            namespace,
                            &owned_path,
                            &owned_data,
                        )
                        .await
                        .map(|_| ())
                })
                .await;
        }
        if script == Script::AnswerAlreadyExists {
            self.record(op_label, path);
            let _: PutIfAbsent = self
                .inner
                .put_raw_if_absent_at(target_label, op_label, namespace, path, data)
                .await?;
            return Ok(PutIfAbsent::AlreadyExists);
        }
        self.follow(
            script,
            op_label,
            path,
            self.inner
                .put_raw_if_absent_at(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        let script = (self.rule)(op_label, path);
        if let Script::Step { late: true, .. } = script {
            let (inner, owned_path) = (self.inner.clone(), path.to_path_buf());
            return self
                .late(op_label, path, async move {
                    inner
                        .delete(target_label, op_label, namespace, &owned_path)
                        .await
                })
                .await;
        }
        self.follow(
            script,
            op_label,
            path,
            self.inner
                .delete_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.answer(
            op_label,
            path,
            self.inner
                .create_dir_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.answer(
            op_label,
            path,
            self.inner
                .list_dir_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.answer(
            op_label,
            path,
            self.inner
                .list_blobs_below_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        let script = (self.rule)(op_label, path);
        if let Script::Step { late: true, .. } = script {
            let (inner, owned_path) = (self.inner.clone(), path.to_path_buf());
            return self
                .late(op_label, path, async move {
                    inner
                        .delete_dir(target_label, op_label, namespace, &owned_path)
                        .await
                        .map(|_| ())
                })
                .await;
        }
        self.follow(
            script,
            op_label,
            path,
            self.inner
                .delete_dir_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        let script = (self.rule)(op_label, from);
        if script == Script::Vanish {
            self.record(op_label, from);
            return Ok(false);
        }
        self.follow(
            script,
            op_label,
            from,
            self.inner.copy_between_at(
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            ),
        )
        .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.answer(
            op_label,
            path,
            self.inner
                .exists_at(target_label, op_label, namespace, path),
        )
        .await
    }
}
