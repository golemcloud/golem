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

use super::DurableContext;
use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use futures::future::{Either, Map, Ready};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tokio::sync::oneshot;

pub(crate) type InitiationWindow<R> = Either<
    Ready<Result<R, WorkerExecutorError>>,
    Map<
        oneshot::Receiver<R>,
        fn(Result<R, oneshot::error::RecvError>) -> Result<R, WorkerExecutorError>,
    >,
>;

/// Short initiation windows with owned inputs and outputs, suitable for queued execution.
pub(crate) trait InitiationAccess<Ctx: WorkerCtx> {
    fn window<R, F>(&mut self, operation: F) -> InitiationWindow<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut DurableWorkerCtx<Ctx>) -> R + Send + 'static;
}

pub(crate) struct InlineInitiationAccess<'a, C: ?Sized> {
    context: &'a mut C,
}

impl<'a, C: ?Sized> InlineInitiationAccess<'a, C> {
    pub(crate) fn new(context: &'a mut C) -> Self {
        Self { context }
    }
}

impl<Ctx: WorkerCtx, C: DurableContext<Ctx> + ?Sized> InitiationAccess<Ctx>
    for InlineInitiationAccess<'_, C>
{
    fn window<R, F>(&mut self, operation: F) -> InitiationWindow<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut DurableWorkerCtx<Ctx>) -> R + Send + 'static,
    {
        let result = self.context.with_ctx(operation);
        Either::Left(futures::future::ready(Ok(result)))
    }
}
