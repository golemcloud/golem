use super::*;
use golem_common::model::oplog::host_functions::host_response_from_typed_schema_value;
use golem_common::model::oplog::{
    HostResponseGolemApiPromiseId, HostResponseGolemApiPromiseResult, PublicAgentInvocation,
    PublicDurableFunctionType, PublicOplogEntryWithIndex,
};

const CREATE: &str = "golem::api::create_promise";
const GET: &str = "golem::api::get_promise_result";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PromiseRecord {
    pub invocation: OplogIndex,
    pub promise: PromiseId,
    pub create_end: OplogIndex,
    pub create_delivery: OplogIndex,
    pub wait: OplogIndex,
}

pub(super) fn invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    finished: usize,
) -> anyhow::Result<OplogIndex> {
    let mut current = false;
    let mut starts = Vec::new();
    let mut ends = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if &method.idempotency_key == key);
                if current {
                    starts.push(entry.oplog_index);
                }
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => ends += 1,
            _ => {}
        }
    }
    ensure!(
        starts.len() == 1 && ends == finished,
        "{key}: {starts:?} Started/{ends} Finished"
    );
    Ok(starts[0])
}

fn terminals(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    entries
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(end) => end.start_index == start,
            _ => false,
        })
        .count()
}

fn completed(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> anyhow::Result<(
    OplogIndex,
    OplogIndex,
    golem_common::model::oplog::HostResponse,
)> {
    let ends: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => {
                Some((entry.oplog_index, end))
            }
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
    ensure!(ends.len() == 1 && terminals(entries, start) == 1 && deliveries.len() == 1);
    let (end_index, end) = ends[0];
    ensure!(start < end_index && end_index < deliveries[0]);
    let name = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(call) if entry.oplog_index == start => {
                Some(call.function_name.as_str())
            }
            _ => None,
        })
        .context("Start")?;
    let response = host_response_from_typed_schema_value(
        name,
        end.response.clone().context("promise response")?,
    )
    .map_err(anyhow::Error::msg)?;
    Ok((end_index, deliveries[0], response))
}

pub(super) fn promise_record(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    owner: &AgentId,
    ready: bool,
    finished: usize,
) -> anyhow::Result<Option<PromiseRecord>> {
    let waits: Vec<_> = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == GET)).collect();
    if waits.is_empty() {
        return Ok(None);
    }
    ensure!(
        waits.len() == 1,
        "original get-result Start must be repaired, not replaced: {waits:?}"
    );
    let invocation = invocation(entries, key, finished)?;
    let creates: Vec<_> = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == CREATE)).collect();
    ensure!(
        creates.len() == 1,
        "one real guest-created promise: {creates:?}"
    );
    let create = creates[0].oplog_index;
    let wait = waits[0].oplog_index;
    ensure!(invocation < create && create < wait);
    for entry in [creates[0], waits[0]] {
        let PublicOplogEntry::Start(start) = &entry.entry else {
            unreachable!()
        };
        ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
        ensure!(if entry.oplog_index == create {
            matches!(
                start.durable_function_type,
                PublicDurableFunctionType::WriteLocal(_)
            )
        } else {
            matches!(
                start.durable_function_type,
                PublicDurableFunctionType::ReadRemote(_)
            )
        });
    }
    let (create_end, create_delivery, response) = completed(entries, create)?;
    let promise = HostResponseGolemApiPromiseId::try_from(response)
        .map_err(anyhow::Error::msg)?
        .promise_id;
    ensure!(
        &promise.agent_id == owner && promise.oplog_idx == create,
        "guest promise must belong to the actual invocation owner: {promise:?}, {owner:?}"
    );
    ensure!(create_delivery < wait);
    ensure!(!entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDiscarded(d) if d.start_index == create || d.start_index == wait)));
    ensure!(!entries.iter().any(|entry| matches!(
        entry.entry,
        PublicOplogEntry::Jump(_) | PublicOplogEntry::Revert(_)
    )));
    if ready {
        let (_, delivery, response) = completed(entries, wait)?;
        ensure!(
            HostResponseGolemApiPromiseResult::try_from(response)
                .map_err(anyhow::Error::msg)?
                .result
                == Some(PAYLOAD.to_vec())
        );
        if finished == 1 {
            let finish = entries
                .iter()
                .find(|entry| {
                    entry.oplog_index > invocation
                        && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
                })
                .context("method Finished")?;
            ensure!(delivery < finish.oplog_index);
        }
    } else {
        ensure!(terminals(entries, wait) == 0);
        ensure!(!entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(d) if d.start_index == wait)), "no fabricated result or observation while pending");
    }
    Ok(Some(PromiseRecord {
        invocation,
        promise,
        create_end,
        create_delivery,
        wait,
    }))
}

pub(super) fn settled(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last_finish = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("Finished")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => ensure!(
                entry.oplog_index < last_finish && terminals(entries, entry.oplog_index) == 1,
                "unsettled Start: {entry:?}"
            ),
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last_finish),
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::Error(_)
            | PublicOplogEntry::Interrupted(_)
            | PublicOplogEntry::Exited(_) => anyhow::bail!("unexpected terminal: {entry:?}"),
            _ => {}
        }
    }
    Ok(())
}
