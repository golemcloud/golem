// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use crate::durable_host::concurrent::DeferredCallReplayOutcome;
use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};
use golem_common::model::oplog::host_functions::{
    GolemToolCreateCollectedOutput, GolemToolCreateOutput,
};
use golem_common::model::oplog::payload::tool_attachment::{
    ToolAttachmentCreated, ToolCollectedOutputCreated,
};
use golem_common::model::oplog::{
    HostRequestToolCollectedOutputCreate, HostResponseToolAttachmentCreated,
    HostResponseToolCollectedOutputCreated,
};

/// Materialize the public endpoint before arming guest completion delivery.
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

pub(super) async fn create_collected<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    max_bytes: u64,
) -> anyhow::Result<
    Result<(Resource<ToolOutputEntry>, Resource<OutputCollectionEntry>), ToolRpcError>,
>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let new_memory = |live| {
        accessor.with(|mut access| {
            AttachmentMemory::for_store(access.get().public_state.worker().active_agents(), live)
        })
    };
    let capacity =
        || accessor.with(|mut access| access.get().state.config.limits.max_tool_attachment_bytes);
    if accessor.with(|mut access| access.get().state.durability_is_suppressed()) {
        let capacity = capacity();
        let memory = new_memory(true);
        let buffer = memory.collection_buffer(limit(max_bytes, capacity)).await?;
        return materialize_collected(accessor, capacity, None, memory, buffer);
    }
    let mut call =
        DurableCallSession::<GolemToolCreateCollectedOutput, NotCancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestToolCollectedOutputCreate { max_bytes },
            DurableFunctionType::WriteLocal,
        )
        .await?;
    let identity = ToolAttachmentId {
        creation_start: call.start_index(),
        slot: ToolAttachmentSlot::Output,
    };
    let (created, allocation, delivery) = 'creation: {
        if !call.is_live() {
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(response, delivery) => {
                    anyhow::ensure!(
                        response.created.attachment.attachment == identity,
                        "collected output identity changed during replay"
                    );
                    break 'creation (response.created, None, delivery);
                }
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let capacity = capacity();
        let limit = limit(max_bytes, capacity);
        let memory = new_memory(true);
        let buffer = memory.collection_buffer(limit).await?;
        let (response, delivery) = call
            .complete_access_deferred(
                accessor,
                accessor.getter(),
                HostResponseToolCollectedOutputCreated {
                    created: ToolCollectedOutputCreated {
                        attachment: ToolAttachmentCreated {
                            attachment: identity,
                            capacity_bytes: capacity as u64,
                        },
                        limit_bytes: limit as u64,
                        admitted: buffer.is_some(),
                    },
                },
            )
            .await?;
        (response.created, Some((memory, buffer)), delivery)
    };
    let output = async {
        let capacity = usize::try_from(created.attachment.capacity_bytes)?;
        anyhow::ensure!(
            created.limit_bytes == limit(max_bytes, capacity) as u64,
            "collected output limit changed during replay"
        );
        let (memory, buffer) = match allocation {
            Some(allocation) => allocation,
            None => {
                let memory = new_memory(false);
                let buffer = if created.admitted {
                    // Completed admission is historical input. Its charge is
                    // upgraded with the attachment if execution becomes live.
                    Some(
                        memory
                            .collection_buffer(usize::try_from(created.limit_bytes)?)
                            .await?
                            .ok_or_else(|| {
                                anyhow!("cannot reconstruct admitted output collection")
                            })?,
                    )
                } else {
                    None
                };
                (memory, buffer)
            }
        };
        materialize_collected(accessor, capacity, Some(identity), memory, buffer)
    }
    .await;
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

fn limit(max_bytes: u64, capacity: usize) -> usize {
    usize::try_from(max_bytes)
        .unwrap_or(usize::MAX)
        .min(capacity)
}

fn materialize_collected<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    capacity: usize,
    identity: Option<ToolAttachmentId>,
    memory: AttachmentMemory,
    buffer: Option<attachment::CollectionBuffer>,
) -> anyhow::Result<
    Result<(Resource<ToolOutputEntry>, Resource<OutputCollectionEntry>), ToolRpcError>,
>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let Some(buffer) = buffer else {
        return Ok(Err(ToolRpcError::ResourceExhausted(
            "output collection memory admission refused".to_string(),
        )));
    };
    accessor.with(|mut access| {
        let ctx = access.get();
        let (producer, consumer, _) = attachment_pair(capacity, memory);
        if let Some(identity) = identity {
            producer.writer().set_identity(identity);
            ctx.state
                .replay_state
                .register_tool_attachment(identity, &consumer.controller())?;
        }
        let invocation = ctx.state.get_current_idempotency_key();
        let target = ctx.table().push(ToolOutputEntry {
            producer: Some(producer),
            completion_only: false,
        })?;
        match ctx.table().push(OutputCollectionEntry {
            consumer,
            invocation,
            buffer,
        }) {
            Ok(collection) => Ok(Ok((target, collection))),
            Err(error) => {
                ctx.table().delete(target)?;
                Err(error.into())
            }
        }
    })
}
