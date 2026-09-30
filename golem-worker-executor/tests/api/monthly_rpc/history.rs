use super::*;
use golem_common::model::oplog::payload::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::{
    PublicAgentInvocation, PublicDurableFunctionType, PublicOplogEntryWithIndex,
};
use golem_common::schema::FromSchema;

const RPC: &str = "golem::rpc::wasm-rpc::invoke_and_await";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Call {
    pub start: OplogIndex,
    pub target_key: IdempotencyKey,
}

pub(super) fn invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    method: &str,
    finished: usize,
) -> anyhow::Result<OplogIndex> {
    let mut current = false;
    let mut starts = Vec::new();
    let mut ends = 0;
    let mut accepted = Vec::new();
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::PendingAgentInvocation(pending) => {
                if let PublicAgentInvocation::AgentMethodInvocation(call) = &pending.invocation
                    && &call.idempotency_key == key
                {
                    ensure!(call.method_name == method);
                    accepted.push(entry.oplog_index);
                }
            }
            PublicOplogEntry::AgentInvocationStarted(started) => {
                current = matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(call) if &call.idempotency_key == key && call.method_name == method);
                if current {
                    starts.push(entry.oplog_index);
                }
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => ends += 1,
            _ => {}
        }
    }
    ensure!(
        starts.len() == 1 && ends == finished && accepted.len() == 1,
        "{method}/{key}: {accepted:?} accepted, {starts:?} Started, {ends} Finished, expected {finished}"
    );
    ensure!(accepted[0] < starts[0]);
    Ok(starts[0])
}

pub(super) fn caller(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    target: &AgentId,
    asynchronous: bool,
    completed: bool,
    finished: usize,
) -> anyhow::Result<Option<Call>> {
    let calls: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == RPC => {
                Some((entry.oplog_index, start))
            }
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return Ok(None);
    }
    ensure!(
        calls.len() == 1,
        "repair must retain one caller Start: {calls:?}"
    );
    let (start, call) = calls[0];
    ensure!(call.parent_start_index.is_none() && call.observational_owner.is_none());
    ensure!(matches!(
        call.durable_function_type,
        PublicDurableFunctionType::WriteRemote(_)
    ));
    let request = HostRequestGolemRpcInvoke::from_value(
        call.request.as_ref().context("RPC request")?.value(),
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(&request.remote_agent_id == target && request.method_name == "inc_after_promise");
    // Sync appends Start immediately after begin; async appends its invocation span
    // between begin and Start. Both keys use the original pre-call position.
    if asynchronous {
        ensure!(
            entries
                .iter()
                .any(|entry| entry.oplog_index.as_u64() + 1 == start.as_u64()
                    && matches!(entry.entry, PublicOplogEntry::StartSpan(_)))
        );
    }
    let begin = OplogIndex::from_u64(start.as_u64() - if asynchronous { 2 } else { 1 });
    ensure!(
        request.idempotency_key == IdempotencyKey::derived(key, begin),
        "unexpected derived key: {request:?}, {start}"
    );
    let invocation = invocation(entries, key, METHOD, finished)?;
    ensure!(invocation < start);
    terminal(
        entries,
        start,
        completed,
        usize::from(asynchronous && completed),
    )?;
    ensure!(!entries.iter().any(|entry| matches!(
        entry.entry,
        PublicOplogEntry::Jump(_) | PublicOplogEntry::Revert(_)
    )));
    Ok(Some(Call {
        start,
        target_key: request.idempotency_key,
    }))
}

pub(super) fn terminal(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    completed: bool,
    deliveries: usize,
) -> anyhow::Result<()> {
    let mut ends = Vec::new();
    let mut delivered = Vec::new();
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => ends.push(entry.oplog_index),
            PublicOplogEntry::CompletionDelivered(d) if d.start_index == start => {
                delivered.push(entry.oplog_index)
            }
            PublicOplogEntry::Cancelled(c) if c.start_index == start => {
                bail!("fabricated cancellation: {entry:?}")
            }
            PublicOplogEntry::CompletionDiscarded(d) if d.start_index == start => {
                bail!("discarded result: {entry:?}")
            }
            _ => {}
        }
    }
    ensure!(
        ends.len() == usize::from(completed) && delivered.len() == deliveries,
        "{start}: End {ends:?}, delivery {delivered:?}, expected completed={completed}, deliveries={deliveries}"
    );
    if completed {
        ensure!(ends[0] > start);
        ensure!(delivered.iter().all(|index| *index > ends[0]));
    }
    Ok(())
}

pub(super) fn target(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    completed: bool,
) -> anyhow::Result<Option<OplogIndex>> {
    let waits: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == "golem::api::get_promise_result" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    if waits.is_empty() {
        return Ok(None);
    }
    ensure!(waits.len() == 1);
    let invocation = invocation(entries, key, "inc_after_promise", usize::from(completed))?;
    ensure!(invocation < waits[0]);
    terminal(entries, waits[0], completed, usize::from(completed))?;
    Ok(Some(waits[0]))
}

pub(super) fn settled(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("Finished")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                let terminals = entries.iter().filter(|end| matches!(&end.entry, PublicOplogEntry::End(end) if end.start_index == entry.oplog_index)).count();
                ensure!(
                    entry.oplog_index < last && terminals == 1,
                    "unsettled Start: {entry:?}"
                );
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last),
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Error(_)
            | PublicOplogEntry::Interrupted(_)
            | PublicOplogEntry::Exited(_) => bail!("unexpected terminal: {entry:?}"),
            _ => {}
        }
    }
    Ok(())
}
