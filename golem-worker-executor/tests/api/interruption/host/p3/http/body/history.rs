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
    ensure!(invocation < send && send < parent && parent < chunks[0] && chunks[0] < chunks[1]);
    ensure!(start_parent(entries, send)?.is_none());
    ensure!(start_parent(entries, parent)?.is_none());
    for child in &chunks {
        ensure!(
            start_parent(entries, *child)? == Some(parent),
            "body chunks must name the consume-body call as their parent"
        );
    }
    let parent_entry = entries
        .iter()
        .find(|entry| entry.oplog_index == parent)
        .unwrap();
    ensure!(
        matches!(&parent_entry.entry, PublicOplogEntry::Start(start)
        if start.function_name == BODY && start.request.is_some()
        && matches!(&start.durable_function_type, PublicDurableFunctionType::WriteRemote(_))),
        "wrong consume-body parent: {parent_entry:?}"
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
        parent,
        first: chunks[0],
        next: chunks[1],
    };
    for open in [pending.parent, pending.next] {
        assert_open(entries, open)?;
    }
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_)
                if ![pending.parent, pending.next].contains(&entry.oplog_index) =>
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

fn assert_retained(
    entries: &[PublicOplogEntryWithIndex],
    original: PendingBody,
) -> anyhow::Result<()> {
    ensure!(
        !entries.iter().any(|entry| matches!(
            entry.entry,
            PublicOplogEntry::Jump(_) | PublicOplogEntry::Revert(_)
        )),
        "body recovery must preserve neighboring history"
    );
    end_and_delivery(entries, original.first)?;
    chunk_response(
        entries,
        original.first,
        SerializableP3HttpBodyChunk::Data(peer::FIRST.to_vec()),
    )?;
    end_and_delivery(entries, original.send)?;
    Ok(())
}

pub(super) fn retained_pending(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    original: PendingBody,
) -> anyhow::Result<Option<PendingBody>> {
    assert_retained(entries, original)?;
    let pending = inspect_pending(entries, key)?;
    if let Some(retained) = pending {
        ensure!(
            retained == original,
            "body repair must retain every recorded Start"
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
    retained: PendingBody,
) -> anyhow::Result<()> {
    assert_retained(entries, original)?;
    ensure!(retained == original);
    ensure!(starts(entries, SEND) == vec![original.send]);
    ensure!(starts(entries, BODY) == vec![original.parent]);
    ensure!(assert_invocation(entries, key, 1)? == original.invocation);
    chunk_response(
        entries,
        original.first,
        SerializableP3HttpBodyChunk::Data(peer::FIRST.to_vec()),
    )?;
    let children = starts(entries, CHUNK);
    ensure!(
        children.len() >= 3 && children[..2] == [original.first, original.next],
        "first byte, remaining data and EOF: {children:?}"
    );
    let mut body = Vec::new();
    for (position, child) in children.iter().enumerate() {
        let response = entries
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
        entries,
        *children.last().unwrap(),
        SerializableP3HttpBodyChunk::End,
    )?;
    let parent_end = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == original.parent => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .context("missing body parent End")?;
    for child in children {
        ensure!(start_parent(entries, child)? == Some(original.parent));
        let (_, delivered) = end_and_delivery(entries, child)?;
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
    assert_retained(entries, original)?;
    let last_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("missing invocation Finished")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(entries, entry.oplog_index) == 1,
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
