use super::*;
use golem_common::model::AtomicRollbackState;

/// Plans a suffix cut using the folded status only. `skipped_regions` may additionally hide a
/// snapshot prefix for this startup attempt; it never mutates the reusable status summary.
pub fn suffix_rollback_region(
    state: &AtomicRollbackState,
    skipped_regions: &DeletedRegions,
    horizon: OplogIndex,
    requested_cut: Option<OplogIndex>,
) -> Result<Option<OplogRegion>, WorkerExecutorError> {
    if requested_cut.is_some_and(|cut| cut <= state.retired_through) {
        return Err(WorkerExecutorError::runtime(format!(
            "Rollback cut {:?} precedes retired recovery history through {}",
            requested_cut, state.retired_through
        )));
    }
    let atomic_cut = state
        .regions
        .iter()
        .find(|(begin, end)| !skipped_regions.is_in_deleted_region(**begin) && end.is_none())
        .map(|(&begin, _)| begin.next());
    let mut cut = match (requested_cut, atomic_cut) {
        (Some(requested), Some(atomic)) => requested.min(atomic),
        (Some(requested), None) => requested,
        (None, Some(atomic)) => atomic,
        (None, None) => return Ok(None),
    };
    // Atomic regions from different Stores may cross rather than nest. Moving the cut backward
    // must include every earlier region whose End would otherwise disappear.
    for (&begin, &end) in state.regions.range(..cut).rev() {
        if !skipped_regions.is_in_deleted_region(begin) && end.is_none_or(|end| end >= cut) {
            cut = begin.next();
        }
    }
    // A committed Jump leaves the Begin intact. Lifecycle hints alone do not create a new attempt.
    Ok(
        (state.last_work >= cut && !skipped_regions.is_in_deleted_region(state.last_work))
            .then_some(OplogRegion {
                start: cut,
                end: horizon,
            }),
    )
}

/// Finds the complete suffix abandoned by an incomplete atomic region before any Store claims it.
#[cfg(test)]
pub async fn atomic_rollback_region(
    oplog: &dyn Oplog,
    skipped_regions: &DeletedRegions,
    horizon: OplogIndex,
) -> Option<OplogRegion> {
    folded_suffix_rollback_region(oplog, skipped_regions, horizon, None).await
}

#[cfg(test)]
pub async fn folded_suffix_rollback_region(
    oplog: &dyn Oplog,
    skipped_regions: &DeletedRegions,
    horizon: OplogIndex,
    requested_cut: Option<OplogIndex>,
) -> Option<OplogRegion> {
    let mut state = AtomicRollbackState::default();
    for (index, entry) in oplog
        .read_exact(OplogIndex::INITIAL, horizon.as_u64())
        .await
    {
        if !skipped_regions.is_in_deleted_region(index) {
            state.observe(index, &entry);
        }
    }
    let result = suffix_rollback_region(&state, skipped_regions, horizon, requested_cut).unwrap();
    assert_eq!(
        result,
        scan_suffix_rollback_region(oplog, skipped_regions, horizon, requested_cut).await
    );
    result
}

/// Independent reference implementation for the scan-free planner's regression tests.
#[cfg(test)]
async fn scan_suffix_rollback_region(
    oplog: &dyn Oplog,
    skipped_regions: &DeletedRegions,
    horizon: OplogIndex,
    requested_cut: Option<OplogIndex>,
) -> Option<OplogRegion> {
    let mut regions = std::collections::BTreeMap::new();
    let mut last_work = OplogIndex::INITIAL;
    let mut next = OplogIndex::INITIAL;
    while next <= horizon {
        let count = CHUNK_SIZE.min(u64::from(horizon) - u64::from(next) + 1);
        let entries = oplog.read_exact(next, count).await;
        next = entries.last_key_value().unwrap().0.next();
        for (index, entry) in entries {
            if skipped_regions.is_in_deleted_region(index) {
                continue;
            }
            if !matches!(
                entry,
                OplogEntry::Jump { .. }
                    | OplogEntry::Suspend { .. }
                    | OplogEntry::Interrupted { .. }
                    | OplogEntry::Restart { .. }
                    | OplogEntry::Error { .. }
                    | OplogEntry::RecoverySucceeded { .. }
            ) {
                last_work = index;
            }
            match entry {
                OplogEntry::BeginAtomicRegion { .. } => {
                    regions.insert(index, None);
                }
                OplogEntry::EndAtomicRegion { begin_index, .. } => {
                    if let Some(end) = regions.get_mut(&begin_index) {
                        *end = Some(index);
                    }
                }
                _ => {}
            }
        }
    }

    let atomic_cut = regions
        .iter()
        .find(|(_, end)| end.is_none())
        .map(|(&begin, _)| begin.next());
    let mut cut = match (requested_cut, atomic_cut) {
        (Some(requested), Some(atomic)) => requested.min(atomic),
        (Some(requested), None) => requested,
        (None, Some(atomic)) => atomic,
        (None, None) => return None,
    };
    // Atomic regions from different Stores may cross rather than nest. Moving the cut backward
    // must include every earlier region whose End would otherwise disappear.
    for (&candidate, &end) in regions.range(..cut).rev() {
        if end.is_none_or(|end| end >= cut) {
            cut = candidate.next();
        }
    }
    // A committed Jump leaves the Begin intact. Lifecycle hints alone do not create a new attempt.
    (last_work >= cut).then_some(OplogRegion {
        start: cut,
        end: horizon,
    })
}
