use super::*;
use golem_common::model::oplog::host_functions::{
    host_request_from_typed_schema_value, host_response_from_typed_schema_value,
};
use golem_common::model::oplog::{
    HostRequestPollCount, HostResponsePollResult, PublicAgentInvocation, PublicDurableFunctionType,
};

const POLL: &str = "io::poll::poll";

fn invocation_entries<'a>(
    entries: &'a [PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
) -> anyhow::Result<&'a [PublicOplogEntryWithIndex]> {
    let starts: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                if &method.idempotency_key == key))
            .then_some(index)
        })
        .collect();
    ensure!(
        starts.len() == 1,
        "one method Started for {key}: {starts:?}"
    );
    let start = starts[0];
    let end = entries
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, entry)| {
            matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)).then_some(index)
        })
        .unwrap_or(entries.len());
    Ok(&entries[start..end])
}

pub(super) fn assert_invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    finished: usize,
) -> anyhow::Result<()> {
    let method = invocation_entries(entries, key)?;
    let accepted: Vec<_> = entries.iter().filter(|entry| matches!(&entry.entry,
        PublicOplogEntry::PendingAgentInvocation(pending)
        if matches!(&pending.invocation, PublicAgentInvocation::AgentMethodInvocation(call) if &call.idempotency_key == key)
    )).collect();
    ensure!(accepted.len() == 1 && accepted[0].oplog_index < method[0].oplog_index);
    ensure!(
        method
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count()
            == finished
    );
    Ok(())
}

pub(super) fn poll_starts(entries: &[PublicOplogEntryWithIndex]) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start)
        if start.function_name == POLL)
            .then_some(entry.oplog_index)
        })
        .collect()
}

pub(super) fn poll_result(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> anyhow::Result<Vec<u32>> {
    let ends: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => Some(end),
            _ => None,
        })
        .collect();
    ensure!(ends.len() == 1, "one End for {start}");
    let response = host_response_from_typed_schema_value(
        POLL,
        ends[0].response.clone().context("poll response")?,
    )
    .map_err(anyhow::Error::msg)?;
    HostResponsePollResult::try_from(response)
        .map_err(anyhow::Error::msg)?
        .result
        .map_err(anyhow::Error::msg)
}

pub(super) fn input_poll(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    count: usize,
    result: Option<&[u32]>,
) -> anyhow::Result<OplogIndex> {
    let method = invocation_entries(entries, key)?;
    let polls = poll_starts(method);
    for entry in method {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
            if start.function_name != POLL {
                ensure!(method.iter().filter(|end| matches!(&end.entry, PublicOplogEntry::End(end) if end.start_index == entry.oplog_index)).count() == 1);
            }
        }
        ensure!(!matches!(entry.entry, PublicOplogEntry::Cancelled(_)));
    }
    ensure!(
        polls.len() == 3,
        "two completed connects and one multi-poll: {polls:?}"
    );
    for (ordinal, index) in polls.iter().enumerate() {
        let entry = method
            .iter()
            .find(|entry| entry.oplog_index == *index)
            .unwrap();
        let PublicOplogEntry::Start(start) = &entry.entry else {
            unreachable!()
        };
        ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
        ensure!(matches!(
            start.durable_function_type,
            PublicDurableFunctionType::ReadLocal(_)
        ));
        let request = host_request_from_typed_schema_value(
            POLL,
            start.request.clone().context("poll request")?,
        )
        .map_err(anyhow::Error::msg)?;
        ensure!(
            HostRequestPollCount::try_from(request)
                .map_err(anyhow::Error::msg)?
                .count
                == if ordinal < 2 { 1 } else { count }
        );
        ensure!(
            !method.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::Cancelled(c) => c.start_index == *index,
                PublicOplogEntry::CompletionDelivered(d) => d.start_index == *index,
                PublicOplogEntry::CompletionDiscarded(d) => d.start_index == *index,
                _ => false,
            }),
            "serialized poll has no cancellation or accessor delivery"
        );
        if ordinal < 2 {
            ensure!(poll_result(method, *index)? == [0]);
            let end = method.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == *index)).unwrap();
            ensure!(end.oplog_index < polls[ordinal + 1]);
        } else if let Some(expected) = result {
            ensure!(
                poll_result(method, *index)? == expected,
                "exact recorded ready indices"
            );
            let end = method.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == *index)).unwrap();
            if let Some(finish) = method
                .iter()
                .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            {
                ensure!(end.oplog_index < finish.oplog_index);
            }
        } else {
            ensure!(!method.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == *index)));
        }
    }
    ensure!(!method.iter().any(|entry| matches!(
        entry.entry,
        PublicOplogEntry::Jump(_) | PublicOplogEntry::Revert(_)
    )));
    Ok(polls[2])
}

pub(super) fn settled_history(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("Finished")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(entries.iter().filter(|end| matches!(&end.entry, PublicOplogEntry::End(end) if end.start_index == entry.oplog_index)).count() == 1);
                ensure!(entry.oplog_index < last);
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last),
            PublicOplogEntry::Cancelled(_) | PublicOplogEntry::Error(_) => {
                anyhow::bail!("unexpected terminal: {entry:?}")
            }
            _ => {}
        }
    }
    Ok(())
}
