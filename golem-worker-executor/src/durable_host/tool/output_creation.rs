// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use crate::durable_host::concurrent::DeferredCallReplayOutcome;
use golem_common::model::oplog::HostResponseToolAttachmentCreated;
use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};
use golem_common::model::oplog::host_functions::GolemToolCreateOutput;
use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentCreated;

/// Materialize the public endpoint before arming guest completion delivery.
/// Both constructors record the same attachment creation, independent of the
/// way the caller subsequently chooses to consume it.
pub(super) async fn create<U, Ctx, R>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    materialize: impl FnOnce(
        &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        Resource<ToolOutputEntry>,
        AttachmentConsumer,
    ) -> anyhow::Result<R>,
) -> anyhow::Result<R>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if accessor.with(|mut access| access.get().state.durability_is_suppressed()) {
        let (target, consumer) = create_output_attachment(accessor, false)?;
        return materialize(accessor, target, consumer);
    }
    let mut call = DurableCallSession::<GolemToolCreateOutput, NotCancellable>::start_access(
        accessor,
        accessor.getter(),
        HostRequestNoInput {},
        DurableFunctionType::WriteLocal,
    )
    .await?;
    let identity = ToolAttachmentId {
        creation_start: call.start_index(),
        slot: ToolAttachmentSlot::Output,
    };
    let (created, delivery) = 'creation: {
        if !call.is_live() {
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(response, delivery) => {
                    anyhow::ensure!(
                        response.created.attachment == identity,
                        "tool output identity changed during replay"
                    );
                    break 'creation (response.created, delivery);
                }
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let capacity_bytes = accessor
            .with(|mut access| access.get().state.config.limits.max_tool_attachment_bytes as u64);
        let (response, delivery) = call
            .complete_access_deferred(
                accessor,
                accessor.getter(),
                HostResponseToolAttachmentCreated {
                    created: ToolAttachmentCreated {
                        attachment: identity,
                        capacity_bytes,
                    },
                },
            )
            .await?;
        (response.created, delivery)
    };
    let output = accessor.with(|mut access| -> anyhow::Result<_> {
        let capacity = usize::try_from(created.capacity_bytes)?;
        let ctx = access.get();
        let memory = AttachmentMemory::for_store(
            ctx.public_state.worker().active_agents(),
            ctx.state.is_live(),
        );
        let (producer, consumer, _) = attachment_pair(capacity, memory);
        producer.writer().set_identity(created.attachment);
        ctx.state
            .replay_state
            .register_tool_attachment(created.attachment, &consumer.controller())?;
        let target = ctx.table().push(ToolOutputEntry {
            producer: Some(producer),
            completion_only: false,
        })?;
        Ok((target, consumer))
    });
    let output = output.and_then(|(target, consumer)| materialize(accessor, target, consumer));
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            delivery.suppress();
            return Err(error);
        }
    };
    if delivery.is_replay_discarded() {
        std::future::pending::<()>().await;
    }
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(output)
}
