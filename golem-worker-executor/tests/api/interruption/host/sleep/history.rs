use super::*;
use golem_common::model::oplog::host_functions::{
    host_request_from_typed_schema_value, host_response_from_typed_schema_value,
};
use golem_common::model::oplog::{
    HostRequestMonotonicClockDuration, HostRequestPollCount, HostResponseMonotonicClockTimestamp,
    HostResponseP3MonotonicClockUnit, HostResponsePollResult, PublicAgentInvocation,
    PublicDurableFunctionType, PublicOplogEntryWithIndex,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SleepRecord {
    pub invocation: OplogIndex,
    pub clock: OplogIndex,
    pub clock_end: OplogIndex,
    pub clock_delivery: OplogIndex,
    pub wait: OplogIndex,
    pub now: u64,
    pub deadline: u64,
}

pub(super) fn unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .try_into()
        .unwrap()
}

fn names(abi: Abi) -> (&'static str, &'static str) {
    match abi {
        Abi::P2 => ("monotonic_clock::subscribe_duration", "io::poll::poll"),
        Abi::P3 => (
            "clocks::monotonic-clock::now",
            "clocks::monotonic-clock::wait-for",
        ),
    }
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

fn deliveries(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == start => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn sleep_record(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    abi: Abi,
    finished: bool,
) -> anyhow::Result<Option<SleepRecord>> {
    let (clock_name, wait_name) = names(abi);
    let waits: Vec<_> = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == wait_name)).collect();
    if waits.is_empty() {
        return Ok(None);
    }
    ensure!(
        waits.len() == 1,
        "one original sleep wait, not a new replay Start: {waits:?}"
    );
    let invocation = invocation(entries, key, usize::from(finished))?;
    let clocks: Vec<_> = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == clock_name)).collect();
    ensure!(
        clocks.len() == 1,
        "recorded clock input must not be read again: {clocks:?}"
    );
    let clock = clocks[0].oplog_index;
    let wait = waits[0].oplog_index;
    ensure!(invocation < clock && clock < wait);
    for entry in [clocks[0], waits[0]] {
        let PublicOplogEntry::Start(start) = &entry.entry else {
            unreachable!()
        };
        ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
        ensure!(matches!(
            start.durable_function_type,
            PublicDurableFunctionType::ReadLocal(_)
        ));
    }
    let duration_start = if abi == Abi::P3 { waits[0] } else { clocks[0] };
    let PublicOplogEntry::Start(start) = &duration_start.entry else {
        unreachable!()
    };
    let request = host_request_from_typed_schema_value(
        &start.function_name,
        start.request.clone().context("sleep duration")?,
    )
    .map_err(anyhow::Error::msg)?;
    let duration = HostRequestMonotonicClockDuration::try_from(request)
        .map_err(anyhow::Error::msg)?
        .duration_in_nanos;
    ensure!(duration == GUEST_SLEEP.as_nanos() as u64);
    if abi == Abi::P2 {
        let PublicOplogEntry::Start(start) = &waits[0].entry else {
            unreachable!()
        };
        let request = host_request_from_typed_schema_value(
            &start.function_name,
            start.request.clone().context("poll count")?,
        )
        .map_err(anyhow::Error::msg)?;
        ensure!(
            HostRequestPollCount::try_from(request)
                .map_err(anyhow::Error::msg)?
                .count
                == 1
        );
    }
    let clock_ends: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == clock => {
                Some((entry.oplog_index, end))
            }
            _ => None,
        })
        .collect();
    ensure!(terminals(entries, clock) == 1 && clock_ends.len() == 1);
    let (clock_end, end) = clock_ends[0];
    let response = host_response_from_typed_schema_value(
        clock_name,
        end.response.clone().context("recorded clock now")?,
    )
    .map_err(anyhow::Error::msg)?;
    let now = HostResponseMonotonicClockTimestamp::try_from(response)
        .map_err(anyhow::Error::msg)?
        .nanos;
    let clock_deliveries = deliveries(entries, clock);
    ensure!(clock_deliveries.len() == 1);
    let clock_delivery = clock_deliveries[0];
    ensure!(clock < clock_end && clock_end < clock_delivery && clock_delivery < wait);
    ensure!(!entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDiscarded(d) if d.start_index == clock || d.start_index == wait)));
    ensure!(
        !entries.iter().any(|entry| matches!(
            entry.entry,
            PublicOplogEntry::Jump(_) | PublicOplogEntry::Revert(_)
        )),
        "sleep repairs its Start without rollback"
    );
    if finished {
        let ends: Vec<_> = entries
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::End(end) if end.start_index == wait => {
                    Some((entry.oplog_index, end))
                }
                _ => None,
            })
            .collect();
        ensure!(terminals(entries, wait) == 1 && ends.len() == 1);
        let (end_index, end) = ends[0];
        let response = host_response_from_typed_schema_value(
            wait_name,
            end.response.clone().context("sleep result")?,
        )
        .map_err(anyhow::Error::msg)?;
        match abi {
            Abi::P2 => ensure!(
                HostResponsePollResult::try_from(response)
                    .map_err(anyhow::Error::msg)?
                    .result
                    == Ok(vec![0])
            ),
            Abi::P3 => {
                HostResponseP3MonotonicClockUnit::try_from(response).map_err(anyhow::Error::msg)?;
            }
        }
        let delivered = deliveries(entries, wait);
        ensure!(delivered.len() == 1 && delivered[0] > end_index);
        let observation = delivered[0];
        let finish = entries
            .iter()
            .find(|entry| {
                entry.oplog_index > invocation
                    && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
            })
            .context("method Finished")?;
        ensure!(wait < end_index && observation < finish.oplog_index);
    } else {
        ensure!(
            terminals(entries, wait) == 0 && deliveries(entries, wait).is_empty(),
            "pending wait must have no invented terminal or observation"
        );
    }
    Ok(Some(SleepRecord {
        invocation,
        clock,
        clock_end,
        clock_delivery,
        wait,
        now,
        deadline: now
            .checked_add(duration)
            .context("clock deadline overflow")?,
    }))
}

pub(super) fn settled(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    ensure!(
        entries
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_)))
            .count()
            == 1,
        "only the accepted monthly stop may suspend this invocation"
    );
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
            | PublicOplogEntry::Exited(_) => {
                anyhow::bail!("unexpected terminal: {entry:?}")
            }
            _ => {}
        }
    }
    Ok(())
}
