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

use async_trait::async_trait;
use golem_wasm_rpc::Value;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Weak;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::invocation::invoke_worker;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::model::{CallingConvention, InvocationKey, WorkerId, WorkerInvocation};

/// Per-worker invocation queue service
///
/// It is responsible for receiving incoming worker invocations in a non-blocking way,
/// persisting them and also making sure that all the enqueued invocations eventually get
/// processed, in the same order as they came in.
///
/// If the queue is empty, the service can trigger invocations directly as an optimization.
///
/// Every worker invocation should be done through this service.
#[async_trait]
pub trait InvocationQueue: Send + Sync {
    async fn enqueue(
        &self,
        invocation_key: Option<InvocationKey>,
        full_function_name: String,
        function_input: Vec<Value>,
        calling_convention: CallingConvention,
    );

    /// Gets the currently enqueued invocations
    fn pending_invocations(&self) -> Vec<WorkerInvocation>;
}

pub struct DefaultInvocationQueue<Ctx: WorkerCtx> {
    _handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<WorkerInvocation>,
    _worker: Weak<Worker<Ctx>>,
}

impl<Ctx: WorkerCtx> DefaultInvocationQueue<Ctx> {
    pub fn new(worker: Arc<Worker<Ctx>>) -> Arc<dyn InvocationQueue> {
        let worker_id = worker.metadata.worker_id.worker_id.clone();

        let worker = Arc::downgrade(&worker);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        let worker_clone = worker.clone();
        let handle = tokio::task::spawn(async move {
            DefaultInvocationQueue::invocation_loop(receiver, worker_clone, worker_id).await;
        });

        Arc::new(DefaultInvocationQueue {
            _handle: Some(handle),
            sender,
            _worker: worker,
        })
    }

    async fn invocation_loop(
        mut receiver: UnboundedReceiver<WorkerInvocation>,
        worker: Weak<Worker<Ctx>>,
        worker_id: WorkerId,
    ) {
        debug!("Invocation queue loop for {worker_id} started");

        while let Some(message) = receiver.recv().await {
            if let Some(worker) = worker.upgrade() {
                debug!("Invocation queue processing {message:?} for {worker_id}");

                let instance = &worker.instance;
                let store = &worker.store;
                let mut store_mutex = store.lock().await;
                let store = store_mutex.deref_mut();

                store
                    .data_mut()
                    .set_current_invocation_key(message.invocation_key)
                    .await;

                // TODO: remove invocation from the worker status record

                let _ = invoke_worker(
                    message.full_function_name,
                    message.function_input,
                    store,
                    instance,
                    message.calling_convention,
                    true, // Invocation queue is always initialized _after_ the worker recovery
                )
                .await;
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

#[async_trait]
impl<Ctx: WorkerCtx> InvocationQueue for DefaultInvocationQueue<Ctx> {
    async fn enqueue(
        &self,
        invocation_key: Option<InvocationKey>,
        full_function_name: String,
        function_input: Vec<Value>,
        calling_convention: CallingConvention,
    ) {
        // TODO: direct invocation
        // TODO: write to oplog and status record
        self.sender
            .send(WorkerInvocation {
                invocation_key,
                full_function_name,
                function_input,
                calling_convention,
            })
            .unwrap()
    }

    fn pending_invocations(&self) -> Vec<WorkerInvocation> {
        // TODO: we need to use some observable queue instead of the mpsc channel
        todo!()
    }
}
