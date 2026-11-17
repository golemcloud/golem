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

use super::initiation::{InitiationAccess, InitiationWindow};
use super::*;
use futures::{
    FutureExt,
    future::{Either, poll_fn, ready},
};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Notify, mpsc};

type Window<Ctx> = Box<dyn FnOnce(&mut DurableWorkerCtx<Ctx>) + Send>;

pub(crate) struct InitiationOwner<Ctx: WorkerCtx> {
    sender: mpsc::UnboundedSender<Window<Ctx>>,
    receiver: Mutex<mpsc::UnboundedReceiver<Window<Ctx>>>,
    pending: AtomicUsize,
    sealed: AtomicBool,
    failure: Mutex<Option<WorkerExecutorError>>,
    changed: Notify,
    closed: tokio_util::sync::CancellationToken,
}

impl<Ctx: WorkerCtx> InitiationOwner<Ctx> {
    pub(crate) fn new() -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            sender,
            receiver: Mutex::new(receiver),
            pending: AtomicUsize::new(0),
            sealed: AtomicBool::new(false),
            failure: Mutex::new(None),
            changed: Notify::new(),
            closed: tokio_util::sync::CancellationToken::new(),
        })
    }

    pub(crate) fn register(
        self: &Arc<Self>,
    ) -> Result<InitiationRegistration<Ctx>, WorkerExecutorError> {
        if self.sealed.load(Ordering::Acquire) {
            return Err(WorkerExecutorError::runtime("tool initiation after return"));
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        Ok(InitiationRegistration {
            owner: self.clone(),
            completed: false,
        })
    }

    pub(crate) fn fail(&self, error: WorkerExecutorError) {
        self.failure
            .lock()
            .expect("initiation failure poisoned")
            .get_or_insert(error);
        self.changed.notify_one();
    }

    pub(crate) fn access(self: &Arc<Self>) -> OwnedInitiationAccess<Ctx> {
        OwnedInitiationAccess(self.clone())
    }

    pub(crate) fn close(&self) {
        self.sealed.store(true, Ordering::Release);
        self.closed.cancel();
        let queued = {
            let mut receive = self.receiver.lock().expect("initiation queue poisoned");
            receive.close();
            let mut queued = Vec::new();
            while let Ok(window) = receive.try_recv() {
                queued.push(window);
            }
            queued
        };
        drop(queued);
        self.changed.notify_one();
    }

    pub(crate) async fn run<F: Future>(&self, work: F) -> Result<F::Output, WorkerExecutorError> {
        tokio::select! {
            biased;
            _ = self.closed.cancelled() => Err(WorkerExecutorError::Interrupted { kind: InterruptKind::Jump }),
            result = work => Ok(result),
        }
    }

    pub(crate) async fn wait_closed(&self) {
        loop {
            let changed = self.changed.notified();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }

    async fn next(&self) -> Option<Window<Ctx>> {
        poll_fn(|cx| {
            self.receiver
                .lock()
                .expect("initiation queue poisoned")
                .poll_recv(cx)
        })
        .await
    }

    pub(crate) async fn serve<T, D>(
        self: Arc<Self>,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> wasmtime::Result<()>
    where
        T: Send + 'static,
        D: HasData + ?Sized,
    {
        let _service = InitiationService(self.clone());
        while let Some(window) = self.next().await {
            store.with(|mut access| window(get_ctx(access.data_mut())));
        }
        Ok(())
    }

    pub(crate) async fn drain(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(), WorkerExecutorError> {
        self.sealed.store(true, Ordering::Release);
        loop {
            let changed = self.changed.notified();
            if self.pending.load(Ordering::Acquire) == 0 {
                if let Some(error) = self
                    .failure
                    .lock()
                    .expect("initiation failure poisoned")
                    .clone()
                {
                    return Err(error);
                }
                return Ok(());
            }
            tokio::select! {
                window = self.next() => {
                    let window = window.ok_or_else(|| WorkerExecutorError::runtime("initiation queue closed"))?;
                    window(ctx);
                }
                _ = changed => {}
            }
        }
    }
}

struct InitiationService<Ctx: WorkerCtx>(Arc<InitiationOwner<Ctx>>);

impl<Ctx: WorkerCtx> Drop for InitiationService<Ctx> {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(crate) struct InitiationRegistration<Ctx: WorkerCtx> {
    owner: Arc<InitiationOwner<Ctx>>,
    completed: bool,
}

impl<Ctx: WorkerCtx> InitiationRegistration<Ctx> {
    pub(crate) fn complete(mut self) {
        self.completed = true;
    }
}

impl<Ctx: WorkerCtx> Drop for InitiationRegistration<Ctx> {
    fn drop(&mut self) {
        if !self.completed {
            self.owner.fail(WorkerExecutorError::runtime(
                "host initiation stopped before settlement",
            ));
        }
        self.owner.pending.fetch_sub(1, Ordering::AcqRel);
        self.owner.changed.notify_one();
    }
}

pub(crate) struct OwnedInitiationAccess<Ctx: WorkerCtx>(Arc<InitiationOwner<Ctx>>);

impl<Ctx: WorkerCtx> Clone for OwnedInitiationAccess<Ctx> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<Ctx: WorkerCtx> InitiationAccess<Ctx> for OwnedInitiationAccess<Ctx> {
    fn window<R, F>(&mut self, operation: F) -> InitiationWindow<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut DurableWorkerCtx<Ctx>) -> R + Send + 'static,
    {
        let (send, receive) = oneshot::channel();
        if self
            .0
            .sender
            .send(Box::new(move |ctx| {
                let _ = send.send(operation(ctx));
            }))
            .is_err()
        {
            return Either::Left(ready(Err(WorkerExecutorError::runtime(
                "initiation context service stopped",
            ))));
        }
        Either::Right(receive.map(
            (|result: Result<R, oneshot::error::RecvError>| {
                result.map_err(|_| WorkerExecutorError::runtime("initiation window abandoned"))
            }) as fn(_) -> _,
        ))
    }
}

pub(crate) enum InitiationRequestContext<'a, T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx> {
    Inline(&'a Accessor<T, D>, fn(&mut T) -> &mut DurableWorkerCtx<Ctx>),
    Owned(OwnedInitiationAccess<Ctx>),
}

impl<T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx> InitiationAccess<Ctx>
    for InitiationRequestContext<'_, T, D, Ctx>
{
    fn window<R, F>(&mut self, operation: F) -> InitiationWindow<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut DurableWorkerCtx<Ctx>) -> R + Send + 'static,
    {
        match self {
            Self::Inline(store, get_ctx) => Either::Left(ready(Ok(
                store.with(|mut access| operation(get_ctx(access.data_mut())))
            ))),
            Self::Owned(access) => access.window(operation),
        }
    }
}
