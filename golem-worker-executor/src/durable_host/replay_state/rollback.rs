use super::cursor::terminal_start_index;
use super::*;

impl ReplayState {
    /// Projects history abandoned by this entity's earliest uncommitted atomic block. Called
    /// before body reconstruction, so none of the affected descendants has a replay claim yet.
    pub(crate) async fn entity_atomic_rollback_regions(
        &self,
        entity_start_index: OplogIndex,
    ) -> Vec<OplogRegion> {
        let replay_target = self.replay_target();
        let skipped_regions = {
            let state = self.cursor.state.lock().await;
            state.skipped_regions.clone()
        };
        let mut projection = OplogScopeProjection::new(entity_start_index);
        let mut open_regions = std::collections::BTreeSet::new();
        let mut owned_indices = Vec::new();
        let mut next = entity_start_index;
        while next <= replay_target {
            let available = u64::from(replay_target) - u64::from(next) + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(available))
                .await;
            let last_read = *entries.last_key_value().unwrap().0;
            for (index, entry) in entries {
                if index > replay_target {
                    break;
                }
                // A crash can persist only some rollback Jumps. Deleted Starts still establish
                // ownership of descendants whose discontiguous regions have not been deleted yet.
                let included = projection.includes(index, &entry);
                if skipped_regions.is_in_deleted_region(index) {
                    continue;
                }
                match &entry {
                    OplogEntry::BeginAtomicRegion {
                        entity_parent_start_index: Some(parent),
                        ..
                    } if *parent == entity_start_index => {
                        open_regions.insert(index);
                    }
                    OplogEntry::EndAtomicRegion { begin_index, .. } => {
                        open_regions.remove(begin_index);
                    }
                    _ => {}
                }
                let entity_terminal = terminal_start_index(&entry) == Some(entity_start_index);
                if included && !entity_terminal && !matches!(entry, OplogEntry::Jump { .. }) {
                    owned_indices.push(index);
                }
            }
            next = last_read.next();
        }

        let Some(begin_index) = open_regions.first() else {
            return Vec::new();
        };
        let mut regions: Vec<OplogRegion> = Vec::new();
        for index in owned_indices
            .into_iter()
            .filter(|index| index > begin_index)
        {
            match regions.last_mut() {
                Some(region) if region.end.next() == index => region.end = index,
                _ => regions.push(OplogRegion {
                    start: index,
                    end: index,
                }),
            }
        }
        regions
    }
}
