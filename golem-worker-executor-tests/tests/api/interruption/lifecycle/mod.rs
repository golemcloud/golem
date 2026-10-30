use super::*;
use golem_common::model::oplog::{OplogEntry, OplogPayload, PayloadId};
use std::collections::BTreeMap;

fn completed_invocation_payload(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    key: &IdempotencyKey,
) -> anyhow::Result<(OplogIndex, OplogIndex, PayloadId)> {
    let starts: Vec<_> = entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key,
                payload,
                ..
            } if idempotency_key == key => Some((*index, payload)),
            _ => None,
        })
        .collect();
    anyhow::ensure!(
        starts.len() == 1,
        "one original Started for {key}: {starts:?}"
    );
    let (start, payload) = starts[0];
    let mut finished = None;
    for (index, entry) in entries.range(start.next()..) {
        match entry {
            OplogEntry::AgentInvocationStarted { .. } => break,
            OplogEntry::AgentInvocationFinished { .. } => {
                finished = Some(*index);
                break;
            }
            _ => {}
        }
    }
    let finished = finished.ok_or_else(|| anyhow::anyhow!("{key} has no recorded Finished"))?;
    anyhow::ensure!(
        !entries
            .values()
            .any(|entry| matches!(entry, OplogEntry::Jump { .. })),
        "completed reconstruction must not depend on discarded history"
    );
    let OplogPayload::External {
        payload_id, cached, ..
    } = payload
    else {
        anyhow::bail!("{key} must use a real external invocation payload");
    };
    anyhow::ensure!(
        cached.is_none(),
        "the stored payload must require actual acquisition"
    );
    Ok((start, finished, payload_id.clone()))
}

mod admission;
mod custom_durability;
mod health;
mod reconstruction;
mod retirement;
mod windows;

test_r::tag_suite!(admission, group5);
test_r::tag_suite!(custom_durability, group5);
test_r::tag_suite!(health, group5);
test_r::tag_suite!(reconstruction, group5);
test_r::tag_suite!(retirement, group5);
test_r::tag_suite!(windows, group5);
