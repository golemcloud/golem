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
use std::collections::HashMap;
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
    /// Waits until the test opens the gate of the storage, passes the listing, and leaves out of
    /// its result each path that the test hid, as a listing that a write and a delete tore does.
    /// Each other call passes.
    Torn,
    /// Gives an error at once to a write or a delete, as a call that got no answer, and makes the
    /// change in a task after the time, as a change that lands late. Each other call passes.
    LandAfter(std::time::Duration),
    /// Never answers a write or a delete, and makes the change in a task after the time, as a
    /// request that was sent, whose answer never comes, and that lands late. Each other call passes.
    HangThenLand(std::time::Duration),
}

impl Script {
    /// Whether a write or a delete with this script gives an error and lands later.
    fn is_late(self) -> bool {
        matches!(
            self,
            Script::Step { late: true, .. } | Script::LandAfter(_) | Script::HangThenLand(_)
        )
    }
}

/// One call of the storage: its namespace, its operation label, its path, the instant when it
/// started, and the instant when it ended. A late change is an event of its own, whose start and
/// end are the instant when it landed.
#[derive(Clone, Debug)]
pub(crate) struct CallEvent {
    pub(crate) namespace: BlobStorageNamespace,
    pub(crate) op_label: &'static str,
    pub(crate) path: Box<Path>,
    pub(crate) started: std::time::Instant,
    pub(crate) ended: std::time::Instant,
    pub(crate) landed_late: bool,
}

/// A rule that gives the script of a call from its namespace, its operation label and its path.
type Rule = Box<dyn Fn(&BlobStorageNamespace, &str, &Path) -> Script + Send + Sync>;

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
    /// The paths that a torn listing leaves out.
    hidden: Mutex<Vec<Box<Path>>>,
    /// The calls in flight for each namespace.
    in_flight: Mutex<HashMap<BlobStorageNamespace, usize>>,
    /// The most namespaces that had a call in flight at one time.
    most_namespaces: AtomicUsize,
    /// Each call that ended, and each late change that landed, in the order of their ends.
    events: Arc<Mutex<Vec<CallEvent>>>,
}

impl ScriptedBlobStorage {
    pub(crate) fn new(
        inner: Arc<InMemoryBlobStorage>,
        rule: impl Fn(&str, &Path) -> Script + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::in_namespaces(inner, move |_, op_label, path| rule(op_label, path))
    }

    /// A storage whose rule also reads the namespace of each call.
    pub(crate) fn in_namespaces(
        inner: Arc<InMemoryBlobStorage>,
        rule: impl Fn(&BlobStorageNamespace, &str, &Path) -> Script + Send + Sync + 'static,
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
            hidden: Mutex::new(Vec::new()),
            in_flight: Mutex::new(HashMap::new()),
            most_namespaces: AtomicUsize::new(0),
            events: Arc::default(),
        })
    }

    /// Gives the most namespaces that had a call in flight at one time.
    pub(crate) fn most_namespaces_at_once(&self) -> usize {
        self.most_namespaces.load(Ordering::SeqCst)
    }

    /// Gives each call that ended, and each late change that landed, in the order of their ends.
    pub(crate) fn events(&self) -> Vec<CallEvent> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Counts a call of `namespace` as in flight until the guard drops, and then records it.
    fn in_flight(
        &self,
        namespace: &BlobStorageNamespace,
        op_label: &'static str,
        path: &Path,
    ) -> InFlight<'_> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *in_flight.entry(namespace.clone()).or_default() += 1;
        self.most_namespaces
            .fetch_max(in_flight.len(), Ordering::SeqCst);
        InFlight {
            storage: self,
            namespace: namespace.clone(),
            op_label,
            path: path.into(),
            started: super::super::runs::now(),
        }
    }

    /// Makes each torn listing leave out `paths`.
    pub(crate) fn hide(&self, paths: impl IntoIterator<Item = Box<Path>>) {
        self.hidden
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(paths);
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

    /// Runs a late call with its `script`: the caller gets an error, and the call reaches the
    /// storage in a task. A step waits for a step of the test and lands when the test lands it,
    /// and a [`Script::LandAfter`] lands after its time.
    async fn late<T>(
        &self,
        script: Script,
        call: &InFlight<'_>,
        landing: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) -> anyhow::Result<T> {
        let (op_label, path) = (call.op_label, &*call.path);
        self.record(op_label, path);
        let landed = self.landed.clone();
        let events = self.events.clone();
        let (namespace, owned_path) = (call.namespace.clone(), Box::<Path>::from(path));
        let record_landing = move |landing_result: anyhow::Result<()>| {
            if landing_result.is_ok() {
                let at = super::super::runs::now();
                events
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(CallEvent {
                        namespace,
                        op_label,
                        path: owned_path,
                        started: at,
                        ended: at,
                        landed_late: true,
                    });
            }
            landed.fetch_add(1, Ordering::SeqCst);
        };
        if let Script::LandAfter(after) = script {
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                record_landing(landing.await);
            });
            return Err(anyhow::anyhow!(
                "the call got no answer within its deadline"
            ));
        }
        if let Script::HangThenLand(after) = script {
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                record_landing(landing.await);
            });
            return std::future::pending().await;
        }
        let stepped = self.wait_for_step(op_label, path).await;
        let landings = self.landings.clone();
        tokio::spawn(async move {
            if let Ok(permit) = landings.acquire().await {
                permit.forget();
            }
            record_landing(landing.await);
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
        namespace: &BlobStorageNamespace,
        op_label: &'static str,
        path: &Path,
        call: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        self.follow((self.rule)(namespace, op_label, path), op_label, path, call)
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
            Script::Vanish
            | Script::AnswerAlreadyExists
            | Script::Torn
            | Script::LandAfter(_)
            | Script::HangThenLand(_) => call.await,
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

/// Counts a call of a namespace as in flight, until the guard drops.
struct InFlight<'a> {
    storage: &'a ScriptedBlobStorage,
    namespace: BlobStorageNamespace,
    op_label: &'static str,
    path: Box<Path>,
    started: std::time::Instant,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut in_flight = self
            .storage
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(calls) = in_flight.get_mut(&self.namespace) {
            *calls -= 1;
            if *calls == 0 {
                in_flight.remove(&self.namespace);
            }
        }
        drop(in_flight);
        self.storage
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(CallEvent {
                namespace: self.namespace.clone(),
                op_label: self.op_label,
                path: self.path.clone(),
                started: self.started,
                ended: super::super::runs::now(),
                landed_late: false,
            });
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        match (self.rule)(&_in_flight.namespace, op_label, path) {
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        self.answer(
            &_in_flight.namespace,
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        self.answer(
            &_in_flight.namespace,
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        self.answer(
            &_in_flight.namespace,
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        let script = (self.rule)(&_in_flight.namespace, op_label, path);
        if script.is_late() {
            let (inner, owned_path, owned_data) =
                (self.inner.clone(), path.to_path_buf(), data.to_vec());
            return self
                .late(script, &_in_flight, async move {
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        let script = (self.rule)(&_in_flight.namespace, op_label, path);
        if script.is_late() {
            let (inner, owned_path, owned_data) =
                (self.inner.clone(), path.to_path_buf(), data.to_vec());
            return self
                .late(script, &_in_flight, async move {
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        let script = (self.rule)(&_in_flight.namespace, op_label, path);
        if script.is_late() {
            let (inner, owned_path) = (self.inner.clone(), path.to_path_buf());
            return self
                .late(script, &_in_flight, async move {
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        self.answer(
            &_in_flight.namespace,
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        self.answer(
            &_in_flight.namespace,
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        let script = (self.rule)(&_in_flight.namespace, op_label, path);
        if script == Script::Torn {
            self.record(op_label, path);
            self.gate.cancelled().await;
            let listed = self
                .inner
                .list_blobs_below_at(target_label, op_label, namespace, path)
                .await?;
            let hidden = self
                .hidden
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            return Ok(listed
                .iter()
                .filter(|blob| !hidden.contains(&blob.path))
                .cloned()
                .collect());
        }
        self.follow(
            script,
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        let script = (self.rule)(&_in_flight.namespace, op_label, path);
        if script.is_late() {
            let (inner, owned_path) = (self.inner.clone(), path.to_path_buf());
            return self
                .late(script, &_in_flight, async move {
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
        let _in_flight = self.in_flight(&from_namespace, op_label, from);
        let script = (self.rule)(&_in_flight.namespace, op_label, from);
        if script == Script::Vanish {
            self.record(op_label, from);
            return Ok(false);
        }
        if script.is_late() {
            let (inner, owned_from, owned_to) =
                (self.inner.clone(), from.to_path_buf(), to.to_path_buf());
            return self
                .late(script, &_in_flight, async move {
                    inner
                        .copy_between(
                            target_label,
                            op_label,
                            from_namespace,
                            &owned_from,
                            to_namespace,
                            &owned_to,
                        )
                        .await
                })
                .await;
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
        let _in_flight = self.in_flight(&namespace, op_label, path);
        self.answer(
            &_in_flight.namespace,
            op_label,
            path,
            self.inner
                .exists_at(target_label, op_label, namespace, path),
        )
        .await
    }
}
