// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_wasm_rpc::Value;
use std::collections::VecDeque;
use std::ops::DerefMut;
use std::sync::Weak;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::invocation::invoke_worker;
use crate::services::oplog::Oplog;
use crate::services::HasOplog;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{OplogEntry, TimestampedUpdateDescription, UpdateDescription};
use golem_common::model::{
    CallingConvention, ComponentVersion, InvocationKey, TimestampedWorkerInvocation, WorkerId,
    WorkerInvocation,
};

/// Per-worker invocation queue service
///
/// It is responsible for receiving incoming worker invocations in a non-blocking way,
/// persisting them and also making sure that all the enqueued invocations eventually get
/// processed, in the same order as they came in.
///
/// If the queue is empty, the service can trigger invocations directly as an optimization.
///
/// Every worker invocation should be done through this service.
pub struct InvocationQueue<Ctx: WorkerCtx> {
    worker_id: WorkerId,
    oplog: Arc<dyn Oplog + Send + Sync>,
    queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    pending_updates: Arc<RwLock<VecDeque<TimestampedUpdateDescription>>>,
    running: Arc<Mutex<Option<RunningInvocationQueue<Ctx>>>>,
}

impl<Ctx: WorkerCtx> InvocationQueue<Ctx> {
    pub fn new(
        worker_id: WorkerId,
        oplog: Arc<dyn Oplog + Send + Sync>,
        initial_pending_invocations: &[TimestampedWorkerInvocation],
        initial_pending_updates: &[TimestampedUpdateDescription],
    ) -> Self {
        let queue = Arc::new(RwLock::new(VecDeque::from_iter(
            initial_pending_invocations.iter().cloned(),
        )));
        let pending_updates = Arc::new(RwLock::new(VecDeque::from_iter(
            initial_pending_updates.iter().cloned(),
        )));

        InvocationQueue {
            worker_id,
            oplog,
            queue,
            pending_updates,
            running: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn attach(&self, worker: Arc<Worker<Ctx>>) {
        let mut running = self.running.lock().await;
        assert!(running.is_none());
        *running = Some(RunningInvocationQueue::new(worker, self.queue.clone()));
    }

    /// Enqueue invocation of an exported function
    pub async fn enqueue(
        &self,
        invocation_key: InvocationKey,
        full_function_name: String,
        function_input: Vec<Value>,
        calling_convention: CallingConvention,
    ) {
        match self.running.lock().await.as_ref() {
            Some(running) => {
                running
                    .enqueue(
                        invocation_key,
                        full_function_name,
                        function_input,
                        calling_convention,
                    )
                    .await;
            }
            None => {
                debug!(
                    "Worker {} is initializing, persisting pending invocation",
                    self.worker_id
                );
                let invocation = WorkerInvocation::ExportedFunction {
                    invocation_key,
                    full_function_name,
                    function_input,
                    calling_convention,
                };
                let entry = OplogEntry::pending_worker_invocation(invocation.clone());
                let timestamped_invocation = TimestampedWorkerInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                };
                self.queue
                    .write()
                    .unwrap()
                    .push_back(timestamped_invocation);
                self.oplog.add(entry).await;
                self.oplog.commit().await;
            }
        }
    }

    /// Enqueue attempting an update.
    ///
    /// The update itself is not performed by the invocation queue's processing loop,
    /// it is going to affect how the worker is recovered next time.
    pub async fn enqueue_update(&self, update_description: UpdateDescription) {
        let entry = OplogEntry::pending_update(update_description.clone());
        let timestamped_update = TimestampedUpdateDescription {
            timestamp: entry.timestamp(),
            description: update_description,
        };
        self.pending_updates
            .write()
            .unwrap()
            .push_back(timestamped_update);
        self.oplog.add(entry).await;
        self.oplog.commit().await;
    }

    /// Enqueues a manual update.
    ///
    /// This enqueues a special function invocation that saves the component's state and
    /// triggers a restart immediately.
    pub async fn enqueue_manual_update(&self, target_version: ComponentVersion) {
        match self.running.lock().await.as_ref() {
            Some(running) => {
                running.enqueue_manual_update(target_version).await;
            }
            None => {
                debug!(
                    "Worker {} is initializing, persisting manual update request",
                    self.worker_id
                );
                let invocation = WorkerInvocation::ManualUpdate { target_version };
                let entry = OplogEntry::pending_worker_invocation(invocation.clone());
                let timestamped_invocation = TimestampedWorkerInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                };
                self.queue
                    .write()
                    .unwrap()
                    .push_back(timestamped_invocation);
                self.oplog.add(entry).await;
                self.oplog.commit().await;
            }
        }
    }

    pub fn pending_invocations(&self) -> Vec<TimestampedWorkerInvocation> {
        self.queue.read().unwrap().iter().cloned().collect()
    }

    pub fn pending_updates(&self) -> VecDeque<TimestampedUpdateDescription> {
        self.pending_updates.read().unwrap().clone()
    }
}

struct RunningInvocationQueue<Ctx: WorkerCtx> {
    _handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<()>,
    queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    worker: Weak<Worker<Ctx>>,
}

impl<Ctx: WorkerCtx> RunningInvocationQueue<Ctx> {
    pub fn new(
        worker: Arc<Worker<Ctx>>,
        queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    ) -> Self {
        let worker_id = worker.metadata.worker_id.clone();

        let worker = Arc::downgrade(&worker);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        // Preload
        for _ in 0..queue.read().unwrap().len() {
            sender.send(()).unwrap();
        }

        let worker_clone = worker.clone();
        let active_clone = queue.clone();
        let handle = tokio::task::spawn(async move {
            RunningInvocationQueue::invocation_loop(
                receiver,
                active_clone,
                worker_clone,
                worker_id,
            )
            .await;
        });

        RunningInvocationQueue {
            _handle: Some(handle),
            sender,
            queue,
            worker,
        }
    }

    pub async fn enqueue(
        &self,
        invocation_key: InvocationKey,
        full_function_name: String,
        function_input: Vec<Value>,
        calling_convention: CallingConvention,
    ) {
        let invocation = WorkerInvocation::ExportedFunction {
            invocation_key,
            full_function_name,
            function_input,
            calling_convention,
        };
        self.enqueue_worker_invocation(invocation).await;
    }

    pub async fn enqueue_manual_update(&self, target_version: ComponentVersion) {
        let invocation = WorkerInvocation::ManualUpdate { target_version };
        self.enqueue_worker_invocation(invocation).await;
    }

    async fn enqueue_worker_invocation(&self, invocation: WorkerInvocation) {
        let entry = OplogEntry::pending_worker_invocation(invocation.clone());
        let timestamped_invocation = TimestampedWorkerInvocation {
            timestamp: entry.timestamp(),
            invocation,
        };
        if let Some(worker) = self.worker.upgrade() {
            if worker.store.try_lock().is_none() {
                debug!(
                    "Worker {} is busy, persisting pending invocation",
                    worker.metadata.worker_id
                );
                // The worker is currently busy, so we write the pending worker invocation to the oplog
                worker.public_state.oplog().add(entry).await;
                worker.public_state.oplog().commit().await;
            }
        }
        self.queue
            .write()
            .unwrap()
            .push_back(timestamped_invocation);
        self.sender.send(()).unwrap()
    }

    async fn invocation_loop(
        mut receiver: UnboundedReceiver<()>,
        active: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
        worker: Weak<Worker<Ctx>>,
        worker_id: WorkerId,
    ) {
        debug!("Invocation queue loop for {worker_id} started");

        while receiver.recv().await.is_some() {
            let message = active
                .write()
                .unwrap()
                .pop_front()
                .expect("Message should be present");
            if let Some(worker) = worker.upgrade() {
                debug!("Invocation queue processing {message:?} for {worker_id}");

                let instance = &worker.instance;
                let store = &worker.store;
                let mut store_mutex = store.lock().await;
                let store = store_mutex.deref_mut();

                match message.invocation {
                    WorkerInvocation::ExportedFunction {
                        invocation_key,
                        full_function_name,
                        function_input,
                        calling_convention,
                    } => {
                        store
                            .data_mut()
                            .set_current_invocation_key(invocation_key)
                            .await;

                        // Make sure to update the pending invocation queue in the status record before
                        // the invocation writes the invocation start oplog entry
                        store.data_mut().update_pending_invocations().await;

                        let _ = invoke_worker(
                            full_function_name,
                            function_input,
                            store,
                            instance,
                            calling_convention,
                            true, // Invocation queue is always initialized _after_ the worker recovery
                        )
                        .await;
                    }
                    WorkerInvocation::ManualUpdate { target_version: _ } => {
                        // TODO: invoke snapshot save function, write pending update oplog entry and deactivate the worker
                        todo!()
                    }
                }
            } else {
                warn!(
                    "Lost invocation message because the worker {worker_id} was dropped: {message:?}"
                );
                break;
            }
        }
        debug!("Invocation queue loop for {worker_id} finished");
    }
}
