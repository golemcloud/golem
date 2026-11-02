// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::model::type_id::{TypeId, TypeIdKind};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Type ID space exhausted")]
pub struct TypeIdSpaceExhausted;

/// Returns only new identities, ordered by kind and name, above all historical IDs.
/// The stored rows must include removed names from both kinds in the environment.
pub fn allocate_type_ids(
    stored: &BTreeMap<(TypeIdKind, String), TypeId>,
    names: impl IntoIterator<Item = (TypeIdKind, String)>,
) -> Result<BTreeMap<(TypeIdKind, String), TypeId>, TypeIdSpaceExhausted> {
    let new_names: BTreeSet<_> = names
        .into_iter()
        .filter(|key| !stored.contains_key(key))
        .collect();
    let mut max_id = stored.values().map(|id| id.get()).max().unwrap_or(0);
    let mut allocations = BTreeMap::new();
    for key in new_names {
        max_id = max_id.checked_add(1).ok_or(TypeIdSpaceExhausted)?;
        let id = TypeId::try_from(max_id).map_err(|_| TypeIdSpaceExhausted)?;
        allocations.insert(key, id);
    }
    Ok(allocations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TypeIdKind::{AgentType, Tool};
    use test_r::test;

    fn key(kind: TypeIdKind, name: &str) -> (TypeIdKind, String) {
        (kind, name.to_string())
    }

    fn rows(entries: &[(TypeIdKind, &str, u32)]) -> BTreeMap<(TypeIdKind, String), TypeId> {
        entries
            .iter()
            .map(|&(kind, name, id)| (key(kind, name), TypeId::try_from(id).unwrap()))
            .collect()
    }

    #[test]
    fn redeploy_retains_agent_ids_across_addition_and_removal() {
        let mut stored = rows(&[(AgentType, "keep", 4), (AgentType, "remove", 9)]);
        let allocations =
            allocate_type_ids(&stored, [key(AgentType, "keep"), key(AgentType, "added")]).unwrap();
        assert_eq!(allocations, rows(&[(AgentType, "added", 10)]));
        stored.extend(allocations);
        assert!(
            allocate_type_ids(&stored, [key(AgentType, "added")])
                .unwrap()
                .is_empty()
        );
        assert_eq!(stored[&key(AgentType, "keep")].get(), 4);
        assert_eq!(stored[&key(AgentType, "added")].get(), 10);
    }

    #[test]
    fn removed_agent_added_back_recovers_its_original_id() {
        let mut stored = rows(&[(AgentType, "returning", 7)]);
        let allocations = allocate_type_ids(&stored, [key(AgentType, "other")]).unwrap();
        assert_eq!(allocations, rows(&[(AgentType, "other", 8)]));
        stored.extend(allocations);
        assert!(
            allocate_type_ids(&stored, [key(AgentType, "returning")])
                .unwrap()
                .is_empty()
        );
        assert_eq!(stored[&key(AgentType, "returning")].get(), 7);
    }

    #[test]
    fn rename_never_reuses_the_previous_id() {
        let mut stored = rows(&[(AgentType, "old", 5)]);
        let allocations = allocate_type_ids(&stored, [key(AgentType, "renamed")]).unwrap();
        assert_eq!(allocations, rows(&[(AgentType, "renamed", 6)]));
        stored.extend(allocations);
        let allocations = allocate_type_ids(&stored, [key(AgentType, "renamed-again")]).unwrap();
        assert_eq!(allocations, rows(&[(AgentType, "renamed-again", 7)]));
        assert_eq!(stored[&key(AgentType, "old")].get(), 5);
    }

    #[test]
    fn new_batch_is_distinct_and_above_historical_removed_maximum() {
        let stored = rows(&[
            (AgentType, "active", 3),
            (AgentType, "removed-agent", 80),
            (Tool, "removed-tool", 100),
        ]);
        let allocations = allocate_type_ids(
            &stored,
            [
                key(Tool, "new-tool"),
                key(AgentType, "z"),
                key(AgentType, "active"),
                key(AgentType, "a"),
            ],
        )
        .unwrap();
        assert_eq!(
            allocations,
            rows(&[
                (AgentType, "a", 101),
                (AgentType, "z", 102),
                (Tool, "new-tool", 103),
            ])
        );
    }

    #[test]
    fn first_id_is_one() {
        for kind in [AgentType, Tool] {
            assert_eq!(
                allocate_type_ids(&BTreeMap::new(), [key(kind, "first")]).unwrap(),
                rows(&[(kind, "first", 1)])
            );
        }
        assert!(allocate_type_ids(&BTreeMap::new(), []).unwrap().is_empty());
    }

    #[test]
    fn tools_and_agents_share_a_sequence_but_not_an_identity() {
        assert_eq!(
            allocate_type_ids(
                &BTreeMap::new(),
                [key(Tool, "same-name"), key(AgentType, "same-name")]
            )
            .unwrap(),
            rows(&[(AgentType, "same-name", 1), (Tool, "same-name", 2)])
        );
        for (historical_kind, new_kind) in [(AgentType, Tool), (Tool, AgentType)] {
            let stored = rows(&[(historical_kind, "same-name", 23)]);
            let allocations = allocate_type_ids(
                &stored,
                [
                    key(historical_kind, "same-name"),
                    key(new_kind, "same-name"),
                ],
            )
            .unwrap();
            assert_eq!(allocations, rows(&[(new_kind, "same-name", 24)]));
        }
    }

    #[test]
    fn tools_retain_ids_across_redeploy_removal_and_return() {
        let mut stored = rows(&[(Tool, "keep", 6), (Tool, "returning", 12)]);
        let allocations =
            allocate_type_ids(&stored, [key(Tool, "keep"), key(Tool, "added")]).unwrap();
        assert_eq!(allocations, rows(&[(Tool, "added", 13)]));
        stored.extend(allocations);
        assert!(
            allocate_type_ids(&stored, [key(Tool, "keep"), key(Tool, "added")])
                .unwrap()
                .is_empty()
        );
        assert!(
            allocate_type_ids(&stored, [key(Tool, "returning")])
                .unwrap()
                .is_empty()
        );
        assert_eq!(stored[&key(Tool, "keep")].get(), 6);
        assert_eq!(stored[&key(Tool, "returning")].get(), 12);
    }

    #[test]
    fn allocation_is_input_order_independent_and_deduplicates_keys() {
        let stored = rows(&[(Tool, "existing", 17)]);
        let names = [
            key(Tool, "z"),
            key(AgentType, "b"),
            key(Tool, "existing"),
            key(AgentType, "a"),
            key(AgentType, "b"),
            key(Tool, "z"),
        ];
        let expected = rows(&[(AgentType, "a", 18), (AgentType, "b", 19), (Tool, "z", 20)]);
        assert_eq!(allocate_type_ids(&stored, names.clone()).unwrap(), expected);
        assert_eq!(
            allocate_type_ids(&stored, names.into_iter().rev()).unwrap(),
            expected
        );
    }

    #[test]
    fn exhaustion_allows_last_id_and_existing_names_but_rejects_overflow() {
        let mut stored = rows(&[(Tool, "historical", u32::MAX - 1)]);
        let before = stored.clone();
        assert_eq!(
            allocate_type_ids(&stored, [key(AgentType, "a"), key(Tool, "b")]),
            Err(TypeIdSpaceExhausted)
        );
        assert_eq!(stored, before);
        let allocations =
            allocate_type_ids(&stored, [key(AgentType, "last"), key(AgentType, "last")]).unwrap();
        assert_eq!(allocations, rows(&[(AgentType, "last", u32::MAX)]));
        stored.extend(allocations);
        assert!(
            allocate_type_ids(&stored, [key(Tool, "historical"), key(AgentType, "last")],)
                .unwrap()
                .is_empty()
        );
        assert!(allocate_type_ids(&stored, []).unwrap().is_empty());
        assert_eq!(
            allocate_type_ids(&stored, [key(Tool, "overflow")]),
            Err(TypeIdSpaceExhausted)
        );
        assert_eq!(
            allocate_type_ids(&stored, [key(Tool, "historical"), key(Tool, "overflow")]),
            Err(TypeIdSpaceExhausted)
        );
    }
}
