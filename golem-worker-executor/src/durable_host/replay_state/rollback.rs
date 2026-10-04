use super::*;

/// Finds the complete suffix abandoned by an incomplete atomic region before any Store claims it.
pub(crate) async fn atomic_rollback_region(
    oplog: &dyn Oplog,
    skipped_regions: &DeletedRegions,
    horizon: OplogIndex,
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

    let mut begin = *regions.iter().find(|(_, end)| end.is_none())?.0;
    // Atomic regions from different Stores may cross rather than nest. Moving the cut backward
    // must include every earlier region whose End would otherwise disappear.
    for (&candidate, &end) in regions.range(..begin).rev() {
        if end.is_none_or(|end| end > begin) {
            begin = candidate;
        }
    }
    // A committed Jump leaves the Begin intact. Lifecycle hints alone do not create a new attempt.
    (last_work > begin).then_some(OplogRegion {
        start: begin.next(),
        end: horizon,
    })
}
