// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use crate::durable_host::concurrent::DeferredCallReplayOutcome;
use crate::services::oplog::OplogOps;
use golem_common::model::OplogIndex;
use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};
use golem_common::model::oplog::host_functions::GolemToolCreateStdinFromBytes;
use golem_common::model::oplog::payload::tool_attachment::{
    ToolAttachmentCreated, ToolFiniteStdinCreated,
};
use golem_common::model::oplog::{
    HostRequestToolFiniteStdinCreate, HostResponseToolFiniteStdinCreated, OplogEntry,
};

pub(super) async fn create_finite<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    bytes: Vec<u8>,
) -> anyhow::Result<Result<Resource<ToolStdinEntry>, ToolRpcError>>
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
        let buffers = allocate(&memory, capacity, &bytes).await?;
        return materialize(accessor, capacity, None, memory, buffers);
    }
    // The guest regenerates these immutable bytes. Match all input, not just
    // its length, without persisting a second copy of the complete body.
    let request = HostRequestToolFiniteStdinCreate {
        length: bytes.len() as u64,
        digest: blake3::hash(&bytes).as_bytes().to_vec(),
    };
    let mut call =
        DurableCallSession::<GolemToolCreateStdinFromBytes, NotCancellable>::start_access(
            accessor,
            accessor.getter(),
            request.clone(),
            DurableFunctionType::WriteLocal,
        )
        .await?;
    if !call.is_live() {
        let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
        if let Err(error) = validate_replayed_input(oplog, call.start_index(), &request).await {
            return Err(call.trap(error));
        }
    }
    let identity = ToolAttachmentId {
        creation_start: call.start_index(),
        slot: ToolAttachmentSlot::Stdin,
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
                        "finite stdin identity changed during replay"
                    );
                    break 'creation (response.created, None, delivery);
                }
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let capacity = capacity();
        let memory = new_memory(true);
        let buffers = allocate(&memory, capacity, &bytes).await?;
        let (response, delivery) = call
            .complete_access_deferred(
                accessor,
                accessor.getter(),
                HostResponseToolFiniteStdinCreated {
                    created: ToolFiniteStdinCreated {
                        attachment: ToolAttachmentCreated {
                            attachment: identity,
                            capacity_bytes: capacity as u64,
                        },
                        admitted: buffers.is_some(),
                    },
                },
            )
            .await?;
        (response.created, Some((memory, buffers)), delivery)
    };
    let stdin = async {
        let capacity = usize::try_from(created.attachment.capacity_bytes)?;
        let (memory, buffers) = match allocation {
            Some(allocation) => allocation,
            None => {
                let memory = new_memory(false);
                let buffers = if created.admitted {
                    Some(
                        allocate(&memory, capacity, &bytes)
                            .await?
                            .ok_or_else(|| anyhow!("cannot reconstruct admitted finite stdin"))?,
                    )
                } else {
                    None
                };
                (memory, buffers)
            }
        };
        materialize(accessor, capacity, Some(identity), memory, buffers)
    }
    .await;
    let stdin = match stdin {
        Ok(stdin) => stdin,
        Err(error) => {
            delivery.suppress();
            return Err(error);
        }
    };
    drop(bytes);
    if delivery.is_replay_discarded() {
        std::future::pending::<()>().await;
    }
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(stdin)
}

async fn validate_replayed_input(
    oplog: Arc<dyn crate::services::oplog::Oplog>,
    start: OplogIndex,
    regenerated: &HostRequestToolFiniteStdinCreate,
) -> anyhow::Result<()> {
    let entry = oplog.read(start).await?;
    let OplogEntry::Start {
        request: Some(request),
        ..
    } = entry
    else {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            format!("finite stdin Start with input metadata at {start}"),
            format!("{entry:?}"),
        )
        .into());
    };
    let recorded = HostRequestToolFiniteStdinCreate::try_from(
        oplog
            .download_payload(request)
            .await
            .map_err(anyhow::Error::msg)?,
    )
    .map_err(anyhow::Error::msg)?;
    validate_regenerated_input(&recorded, regenerated)?;
    Ok(())
}

fn validate_regenerated_input(
    recorded: &HostRequestToolFiniteStdinCreate,
    regenerated: &HostRequestToolFiniteStdinCreate,
) -> Result<(), WorkerExecutorError> {
    if recorded.length != regenerated.length || recorded.digest != regenerated.digest {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "finite stdin matching its recorded length and digest",
            "different regenerated finite stdin",
        ));
    }
    Ok(())
}

async fn allocate(
    memory: &AttachmentMemory,
    capacity: usize,
    bytes: &[u8],
) -> anyhow::Result<Option<Vec<attachment::CollectionBuffer>>> {
    if capacity == 0 && !bytes.is_empty() {
        return Ok(None);
    }
    let chunk_size = capacity.clamp(1, 64 * 1024);
    let mut buffers = Vec::new();
    if buffers
        .try_reserve_exact(bytes.len().div_ceil(chunk_size))
        .is_err()
    {
        return Ok(None);
    }
    for chunk in bytes.chunks(chunk_size) {
        let Some(mut buffer) = memory.collection_buffer(chunk.len()).await? else {
            return Ok(None);
        };
        buffer.bytes.extend_from_slice(chunk);
        buffers.push(buffer);
    }
    Ok(Some(buffers))
}

fn materialize<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    capacity: usize,
    identity: Option<ToolAttachmentId>,
    memory: AttachmentMemory,
    buffers: Option<Vec<attachment::CollectionBuffer>>,
) -> anyhow::Result<Result<Resource<ToolStdinEntry>, ToolRpcError>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let Some(buffers) = buffers else {
        return Ok(Err(ToolRpcError::ResourceExhausted(
            "finite stdin memory admission refused".to_string(),
        )));
    };
    accessor.with(|mut access| {
        let consumer = attachment::finite_stdin(capacity, memory, identity, buffers);
        let ctx = access.get();
        if let Some(identity) = identity {
            ctx.state
                .replay_state
                .register_tool_attachment(identity, &consumer.controller())?;
        }
        Ok(Ok(ctx.table().push(ToolStdinEntry { consumer })?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::active_agents::MemoryGrant;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    #[test]
    fn finite_stdin_rejects_changed_regenerated_input() {
        let recorded = HostRequestToolFiniteStdinCreate {
            length: 3,
            digest: blake3::hash(b"abc").as_bytes().to_vec(),
        };
        assert!(validate_regenerated_input(&recorded, &recorded).is_ok());
        let changed = HostRequestToolFiniteStdinCreate {
            length: 3,
            digest: blake3::hash(b"xyz").as_bytes().to_vec(),
        };
        assert!(validate_regenerated_input(&recorded, &changed).is_err());
        let mut changed = recorded.clone();
        changed.length += 1;
        assert!(validate_regenerated_input(&recorded, &changed).is_err());
        let mut changed = recorded.clone();
        changed.digest.pop();
        assert!(validate_regenerated_input(&recorded, &changed).is_err());
    }

    #[test]
    async fn finite_stdin_admission_releases_partial_allocation_on_refusal() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let memory = AttachmentMemory::with_test_reservation(true, move |bytes| {
            let admitted = observed.fetch_add(1, Ordering::SeqCst) == 0;
            async move { admitted.then(|| MemoryGrant::inert(bytes)) }
        });
        assert!(allocate(&memory, 3, b"abcdefgh").await.unwrap().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    async fn finite_stdin_zero_capacity_accepts_only_empty_input() {
        let memory = AttachmentMemory::inert();
        assert!(allocate(&memory, 0, &[]).await.unwrap().unwrap().is_empty());
        assert!(allocate(&memory, 0, &[1]).await.unwrap().is_none());
    }

    #[test]
    async fn finite_stdin_historical_admission_does_not_consult_live_pressure() {
        let memory = AttachmentMemory::with_test_reservation(false, |_| async {
            panic!("completed admission must not consult current pressure")
        });
        let buffers = allocate(&memory, 3, b"abcdefgh").await.unwrap().unwrap();
        assert_eq!(
            buffers
                .iter()
                .map(|buffer| buffer.bytes.len())
                .collect::<Vec<_>>(),
            vec![3, 3, 2]
        );
    }
}
