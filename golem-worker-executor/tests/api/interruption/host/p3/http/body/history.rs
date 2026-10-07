use super::*;
use golem_common::model::oplog::payload::types::SerializableP3HttpBodyChunk;
use golem_common::model::oplog::{
    HostResponse, HostResponseP3HttpClientConsumeBodyChunk, PublicAgentInvocation,
    PublicDurableFunctionType, PublicOplogEntryWithIndex,
};
use golem_common::schema::FromSchema;

pub(super) const SEND: &str = "http::client::send";
const BODY: &str = "http::types::response::consume-body";
const CHUNK: &str = "http::types::response::consume-body-chunk";

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) struct PendingBody {
    invocation: OplogIndex,
    send: OplogIndex,
    scope: OplogIndex,
    parent: OplogIndex,
    first: OplogIndex,
    next: OplogIndex,
}

pub(super) fn starts(entries: &[PublicOplogEntryWithIndex], name: &str) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == name => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn assert_invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    finished: usize,
) -> anyhow::Result<OplogIndex> {
    let mut current = false;
    let mut started = Vec::new();
    let mut terminals = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if &method.idempotency_key == key);
                if current {
                    started.push(entry.oplog_index);
                }
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => terminals += 1,
            _ => {}
        }
    }
    ensure!(
        started.len() == 1 && terminals == finished,
        "invocation {key}: {started:?} Started/{terminals} Finished"
    );
    Ok(started[0])
}

fn terminal_count(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    entries
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .count()
}

fn end_and_delivery(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> anyhow::Result<(OplogIndex, OplogIndex)> {
    let ends: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => Some(entry.oplog_index),
            _ => None,
        })
        .collect();
    let deliveries: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == start => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    ensure!(
        terminal_count(entries, start) == 1 && ends.len() == 1 && deliveries.len() == 1,
        "Start {start} needs exactly one End and delivery: {entries:#?}"
    );
    ensure!(start < ends[0] && ends[0] < deliveries[0]);
    Ok((ends[0], deliveries[0]))
}

fn chunk_response(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    chunk: SerializableP3HttpBodyChunk,
) -> anyhow::Result<()> {
    let expected: HostResponse = HostResponseP3HttpClientConsumeBodyChunk { chunk }.into();
    let expected = expected.into_typed_schema_value()?;
    let responses: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => Some(end.response.as_ref()),
            _ => None,
        })
        .collect();
    ensure!(
        responses == vec![Some(&expected)],
        "unexpected chunk response for {start}: {responses:?}"
    );
    Ok(())
}

fn assert_open(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> anyhow::Result<()> {
    ensure!(
        terminal_count(entries, start) == 0,
        "Start {start} must remain incomplete"
    );
    ensure!(
        !entries.iter().any(|entry| match &entry.entry {
            PublicOplogEntry::CompletionDelivered(delivery) => delivery.start_index == start,
            PublicOplogEntry::CompletionDiscarded(delivery) => delivery.start_index == start,
            _ => false,
        }),
        "open Start {start} must not have a delivery/discard"
    );
    Ok(())
}

fn start_parent(
    entries: &[PublicOplogEntryWithIndex],
    index: OplogIndex,
) -> anyhow::Result<Option<OplogIndex>> {
    let start = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if entry.oplog_index == index => Some(start),
            _ => None,
        })
        .context("missing Start")?;
    ensure!(start.observational_owner.is_none());
    Ok(start.parent_start_index)
}

fn inspect_pending(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
) -> anyhow::Result<Option<PendingBody>> {
    let parents = starts(entries, BODY);
    let chunks = starts(entries, CHUNK);
    if parents.len() != 1 || chunks.len() < 2 {
        return Ok(None);
    }
    ensure!(
        chunks.len() == 2,
        "peer has sent only one byte; extra chunk Start: {entries:#?}"
    );
    let invocation = assert_invocation(entries, key, 0)?;
    let sends = starts(entries, SEND);
    ensure!(sends.len() == 1);
    let send = sends[0];
    let parent = parents[0];
    let scope =
        start_parent(entries, parent)?.context("consume-body must nest under its batched scope")?;
    ensure!(
        invocation < send
            && send < scope
            && scope < parent
            && parent < chunks[0]
            && chunks[0] < chunks[1]
    );
    ensure!(start_parent(entries, send)?.is_none());
    ensure!(start_parent(entries, scope)?.is_none());
    for child in &chunks {
        ensure!(
            start_parent(entries, *child)? == Some(scope),
            "body children must share the parent batched scope"
        );
    }
    let scope_entry = entries
        .iter()
        .find(|entry| entry.oplog_index == scope)
        .unwrap();
    ensure!(
        matches!(&scope_entry.entry, PublicOplogEntry::Start(start)
        if start.function_name == format!("<scope:batched-write:consume-body:{send}>") && start.request.is_none()
        && matches!(&start.durable_function_type, PublicDurableFunctionType::WriteRemoteBatched(p) if p.index.is_none())),
        "wrong consume-body scope: {scope_entry:?}"
    );
    let (_, send_delivery) = end_and_delivery(entries, send)?;
    ensure!(send_delivery < chunks[0]);
    let (_, first_delivery) = end_and_delivery(entries, chunks[0])?;
    ensure!(first_delivery < chunks[1]);
    chunk_response(
        entries,
        chunks[0],
        SerializableP3HttpBodyChunk::Data(peer::FIRST.to_vec()),
    )?;
    let pending = PendingBody {
        invocation,
        send,
        scope,
        parent,
        first: chunks[0],
        next: chunks[1],
    };
    for open in [pending.scope, pending.parent, pending.next] {
        assert_open(entries, open)?;
    }
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_)
                if ![pending.scope, pending.parent, pending.next].contains(&entry.oplog_index) =>
            {
                ensure!(
                    terminal_count(entries, entry.oplog_index) == 1,
                    "unexpected additional open Start: {entry:?}"
                );
            }
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Interrupted(_) => anyhow::bail!("unexpected terminal: {entry:?}"),
            _ => {}
        }
    }
    Ok(Some(pending))
}

pub(super) fn pending_body(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
) -> anyhow::Result<Option<PendingBody>> {
    ensure!(
        !entries
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Jump(_))),
        "no recovery Jump before resume"
    );
    inspect_pending(entries, key)
}

fn assert_abandoned(
    entries: &[PublicOplogEntryWithIndex],
    original: PendingBody,
) -> anyhow::Result<()> {
    let jumps: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(jump) => Some((entry.oplog_index, &jump.jump)),
            _ => None,
        })
        .collect();
    ensure!(
        jumps.len() == 1,
        "one incomplete-body recovery Jump expected: {jumps:?}"
    );
    // Batched recovery retains the enclosing scope Start, jumping only its abandoned
    // contents. The consume-body host parent and chunk children get fresh Starts.
    for index in [original.parent, original.first, original.next] {
        ensure!(
            jumps[0].1.contains(index) && index < jumps[0].0,
            "abandoned body entry {index} must be in the Jump"
        );
    }
    ensure!(
        !jumps[0].1.contains(original.send)
            && !jumps[0].1.contains(original.invocation)
            && !jumps[0].1.contains(original.scope)
    );
    ensure!(jumps[0].1.start == original.scope.next());
    for open in [original.parent, original.next] {
        assert_open(entries, open)?;
    }
    end_and_delivery(entries, original.first)?;
    chunk_response(
        entries,
        original.first,
        SerializableP3HttpBodyChunk::Data(peer::FIRST.to_vec()),
    )?;
    end_and_delivery(entries, original.send)?;
    Ok(())
}

fn visible(entries: &[PublicOplogEntryWithIndex]) -> Vec<PublicOplogEntryWithIndex> {
    entries
        .iter()
        .filter(|candidate| {
            !entries.iter().any(|entry| {
                matches!(&entry.entry,
                    PublicOplogEntry::Jump(jump) if jump.jump.contains(candidate.oplog_index)
                )
            })
        })
        .cloned()
        .collect()
}

pub(super) fn reissued_pending(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    original: PendingBody,
) -> anyhow::Result<Option<PendingBody>> {
    assert_abandoned(entries, original)?;
    let pending = inspect_pending(&visible(entries), key)?;
    if let Some(fresh) = pending {
        ensure!(fresh.invocation == original.invocation && fresh.send == original.send);
        ensure!(fresh.scope == original.scope);
        ensure!(
            fresh.parent > original.next
                && fresh.first > original.next
                && fresh.next > original.next
        );
        ensure!(
            starts(entries, SEND) == vec![original.send],
            "body reissue must not append another send"
        );
    }
    Ok(pending)
}

pub(super) fn recovered_body(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    original: PendingBody,
    fresh: PendingBody,
) -> anyhow::Result<()> {
    assert_abandoned(entries, original)?;
    let visible = visible(entries);
    ensure!(starts(&visible, SEND) == vec![original.send]);
    ensure!(starts(&visible, BODY) == vec![fresh.parent]);
    ensure!(assert_invocation(&visible, key, 1)? == original.invocation);
    chunk_response(
        &visible,
        fresh.first,
        SerializableP3HttpBodyChunk::Data(peer::FIRST.to_vec()),
    )?;
    let children = starts(&visible, CHUNK);
    ensure!(
        children.len() >= 3 && children[..2] == [fresh.first, fresh.next],
        "first byte, remaining data and EOF: {children:?}"
    );
    let mut body = Vec::new();
    for (position, child) in children.iter().enumerate() {
        let response = visible
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::End(end) if end.start_index == *child => end.response.as_ref(),
                _ => None,
            })
            .context("missing chunk response")?;
        let chunk = HostResponseP3HttpClientConsumeBodyChunk::from_value(response.value())?;
        match chunk.chunk {
            SerializableP3HttpBodyChunk::Data(bytes) => {
                ensure!(!bytes.is_empty() && position + 1 < children.len());
                body.extend(bytes);
            }
            SerializableP3HttpBodyChunk::End => ensure!(position + 1 == children.len()),
            SerializableP3HttpBodyChunk::Cancelled => {
                anyhow::bail!("monthly recovery invented a cancelled chunk")
            }
        }
    }
    ensure!(
        body == peer::BODY,
        "exactly one resulting body, independent of native frame boundaries"
    );
    chunk_response(
        &visible,
        *children.last().unwrap(),
        SerializableP3HttpBodyChunk::End,
    )?;
    let parent_end = visible
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == fresh.parent => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .context("missing body parent End")?;
    let scope_end = visible
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == fresh.scope => Some(entry.oplog_index),
            _ => None,
        })
        .context("missing retained batched scope End")?;
    ensure!(
        parent_end < scope_end,
        "batched scope must close after its host parent"
    );
    for child in children {
        ensure!(start_parent(&visible, child)? == Some(fresh.scope));
        let (_, delivered) = end_and_delivery(&visible, child)?;
        ensure!(
            delivered < parent_end,
            "parent finalization must follow child delivery"
        );
    }
    assert_settled_history(entries, original)
}

pub(super) fn assert_settled_history(
    entries: &[PublicOplogEntryWithIndex],
    original: PendingBody,
) -> anyhow::Result<()> {
    assert_abandoned(entries, original)?;
    let visible = visible(entries);
    let last_finished = visible
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("missing invocation Finished")?
        .oplog_index;
    for entry in &visible {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(&visible, entry.oplog_index) == 1,
                    "unsettled/duplicate visible Start: {entry:?}"
                );
                ensure!(entry.oplog_index < last_finished);
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last_finished),
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Error(_) => anyhow::bail!("unexpected terminal: {entry:?}"),
            _ => {}
        }
    }
    Ok(())
}
