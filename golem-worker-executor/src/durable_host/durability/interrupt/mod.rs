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

use crate::durable_host::DurableWorkerCtx;
use crate::model::ExecutionStatus;
use crate::workerctx::WorkerCtx;
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn interrupt_sources(&self) -> InterruptSources<'_> {
        InterruptSources::new(
            &self.execution_status,
            &self.state.invocation_deadline_exceeded,
            &self.state.tail_work_deadline_exceeded,
            self.owner_execution.tool_operations(),
            self.entity_cancellation(),
        )
    }
}

/// Borrow the latches, rather than their values, so subscription precedes every latch check.
#[derive(Clone)]
pub(crate) struct InterruptSources<'a> {
    execution_status: &'a std::sync::RwLock<ExecutionStatus>,
    invocation_deadline_exceeded: &'a AtomicBool,
    tail_work_deadline_exceeded: &'a AtomicBool,
    owner_operations: Arc<crate::durable_host::tool::operation::OwnerToolOperations>,
    entity_cancellation: Option<tokio_util::sync::CancellationToken>,
}

impl<'a> InterruptSources<'a> {
    pub(crate) fn new(
        execution_status: &'a std::sync::RwLock<ExecutionStatus>,
        invocation_deadline_exceeded: &'a AtomicBool,
        tail_work_deadline_exceeded: &'a AtomicBool,
        owner_operations: Arc<crate::durable_host::tool::operation::OwnerToolOperations>,
        entity_cancellation: Option<tokio_util::sync::CancellationToken>,
    ) -> Self {
        Self {
            execution_status,
            invocation_deadline_exceeded,
            tail_work_deadline_exceeded,
            owner_operations,
            entity_cancellation,
        }
    }

    pub(crate) fn subscribe(self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        let interrupt_signal = subscribe_interrupt_with_latch(self.execution_status, || {
            self.invocation_deadline_exceeded.load(Ordering::Acquire)
                || self.tail_work_deadline_exceeded.load(Ordering::Acquire)
                || self
                    .owner_operations
                    .interruptible_owner_failure()
                    .is_some()
        });
        match self.entity_cancellation {
            Some(cancellation) => Box::pin(async move {
                tokio::select! {
                    biased;
                    interrupt = interrupt_signal => interrupt,
                    _ = cancellation.cancelled() => InterruptKind::EntityCancellation,
                }
            }),
            None => interrupt_signal,
        }
    }
}

fn subscribe_interrupt_with_latch(
    execution_status: &std::sync::RwLock<ExecutionStatus>,
    interrupted: impl FnOnce() -> bool,
) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
    // Subscribe before checking synthetic interrupts: either the latch or the broadcast wins.
    let interrupt_signal = execution_status
        .read()
        .unwrap()
        .create_await_interrupt_signal();
    if interrupted() {
        let status = execution_status.read().unwrap();
        if matches!(&*status, ExecutionStatus::Interrupting { .. }) {
            return status.create_await_interrupt_signal();
        }
        return Box::pin(std::future::ready(InterruptKind::Interrupt(
            Timestamp::now_utc(),
        )));
    }
    interrupt_signal
}

#[cfg(test)]
mod tests;
