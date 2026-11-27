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

use super::test_oplog::{at, idx};
use super::*;
use crate::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use std::sync::Arc;
use test_r::test;

fn range(start: u64, count: u64) -> Option<OplogRange> {
    Some(OplogRange {
        start: idx(start),
        count,
    })
}

fn expired(start: u64) -> Result<(), OplogReadError> {
    Err(OplogReadError::Expired {
        retained_start: idx(start),
    })
}

#[test]
fn the_start_never_moves_back() {
    assert_eq!(at(5).advanced_to(idx(4)).start(), idx(5));
    assert_eq!(at(5).advanced_to(idx(5)).start(), idx(5));
    assert_eq!(at(5).advanced_to(idx(6)).start(), idx(6));
    assert_eq!(OplogRetention::FULL.start(), OplogIndex::INITIAL);
}

#[test]
fn the_retention_round_trips_through_its_encoding() {
    let encoded = serialize(&at(7)).unwrap();
    let decoded: OplogRetention = deserialize(&encoded).unwrap();
    assert_eq!(decoded, at(7));
}

#[test]
fn the_clamp_moves_only_an_index_below_the_start() {
    assert_eq!(at(4).clamp(idx(3)), idx(4));
    assert_eq!(at(4).clamp(idx(4)), idx(4));
    assert_eq!(at(4).clamp(idx(5)), idx(5));
    assert_eq!(at(4).clamp(OplogIndex::NONE), idx(4));
}

#[test]
fn only_an_index_at_or_after_the_start_is_retained() {
    assert!(!at(4).retains(idx(3)));
    assert!(at(4).retains(idx(4)));
    assert!(at(4).retains(idx(5)));
    assert!(
        !at(4).retains(OplogIndex::INITIAL),
        "the `Create` entry below the start"
    );
    assert!(at(1).retains(OplogIndex::INITIAL));
    assert!(!at(1).retains(OplogIndex::NONE));
}

#[test]
fn the_range_holds_the_retained_entries_and_is_none_when_empty() {
    assert_eq!(at(4).range(idx(1), idx(10)), range(4, 7));
    assert_eq!(at(4).range(idx(3), idx(10)), range(4, 7));
    assert_eq!(at(4).range(idx(4), idx(10)), range(4, 7));
    assert_eq!(at(4).range(idx(5), idx(10)), range(5, 6));
    assert_eq!(at(4).range(idx(10), idx(10)), range(10, 1));
    assert_eq!(at(4).range(idx(11), idx(10)), None);
    assert_eq!(at(4).range(idx(1), idx(4)), range(4, 1));
    assert_eq!(at(4).range(idx(1), idx(3)), None);
    assert_eq!(at(1).range(OplogIndex::NONE, idx(2)), range(1, 2));
    assert_eq!(at(1).range(idx(1), OplogIndex::NONE), None);
}

#[test]
fn a_range_within_keeps_its_start_and_is_cut_to_the_maximum() {
    let within = |from, last, max| OplogRange::within(idx(from), idx(last), max);
    assert_eq!(within(4, 10, 3), range(4, 3));
    assert_eq!(within(2, 10, 3), range(2, 3));
    assert_eq!(within(8, 10, 3), range(8, 3));
    assert_eq!(within(9, 10, 3), range(9, 2));
    assert_eq!(within(10, 10, 3), range(10, 1));
    assert_eq!(within(11, 10, 3), None);
    assert_eq!(within(4, 10, 7), range(4, 7));
    assert_eq!(within(4, 10, 8), range(4, 7));
    assert_eq!(within(4, 10, 6), range(4, 6));
    assert_eq!(within(4, 10, 0), range(4, 1));
    assert_eq!(within(4, 10, 1), range(4, 1));
    assert_eq!(within(1, 10, u64::MAX), range(1, 10));
    assert_eq!(within(0, u64::MAX, u64::MAX), range(0, u64::MAX));
    assert_eq!(within(1, u64::MAX, u64::MAX), range(1, u64::MAX));
    assert_eq!(within(2, u64::MAX, u64::MAX), range(2, u64::MAX - 1));
}

#[test]
fn a_range_ends_before_the_index_after_its_last_entry() {
    assert_eq!(range(4, 7).unwrap().end(), idx(11));
    assert_eq!(range(4, 1).unwrap().end(), idx(5));
    assert_eq!(OplogRange::CREATE.end(), idx(2));
}

#[test]
fn a_chunk_is_the_retained_range_cut_to_the_maximum() {
    assert_eq!(at(4).chunk(idx(1), idx(10), 3), range(4, 3));
    assert_eq!(at(4).chunk(idx(3), idx(10), 3), range(4, 3));
    assert_eq!(at(4).chunk(idx(4), idx(10), 3), range(4, 3));
    assert_eq!(at(4).chunk(idx(5), idx(10), 3), range(5, 3));
    assert_eq!(at(4).chunk(idx(8), idx(10), 3), range(8, 3));
    assert_eq!(at(4).chunk(idx(9), idx(10), 3), range(9, 2));
    assert_eq!(at(4).chunk(idx(10), idx(10), 3), range(10, 1));
    assert_eq!(at(4).chunk(idx(11), idx(10), 3), None);
    assert_eq!(at(4).chunk(idx(1), idx(3), 3), None);
    assert_eq!(at(4).chunk(idx(4), idx(10), 7), range(4, 7));
    assert_eq!(at(4).chunk(idx(4), idx(10), 8), range(4, 7));
    assert_eq!(at(4).chunk(idx(4), idx(10), 0), range(4, 1));
    assert_eq!(at(1).chunk(OplogIndex::NONE, idx(2), 1024), range(1, 2));
}

#[test]
fn a_walk_of_chunks_reads_every_retained_entry_once() {
    let walk = |retention: OplogRetention, last: u64, max: u64| {
        std::iter::successors(retention.chunk(OplogIndex::NONE, idx(last), max), |chunk| {
            retention.chunk(chunk.end(), idx(last), max)
        })
        .collect::<Vec<_>>()
    };
    assert_eq!(
        walk(at(4), 10, 3),
        vec![range(4, 3), range(7, 3), range(10, 1)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        walk(at(4), 9, 3),
        vec![range(4, 3), range(7, 3)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    );
    assert_eq!(walk(at(4), 3, 3), Vec::new());
}

#[test]
fn a_page_keeps_its_start_and_is_expired_below_the_retained_start() {
    let agent_id = AgentId::from_parts(7, 1);
    let page = |retention: OplogRetention, start, last, max| {
        retention.page(&agent_id, idx(start), idx(last), max)
    };
    let refused = Err(UnreadableOplog::new(&agent_id, expired(4).unwrap_err()));
    assert_eq!(page(at(4), 4, 10, 3), Ok(range(4, 3)));
    assert_eq!(page(at(4), 5, 10, 3), Ok(range(5, 3)));
    assert_eq!(page(at(4), 9, 10, 3), Ok(range(9, 2)));
    assert_eq!(page(at(4), 10, 10, 3), Ok(range(10, 1)));
    assert_eq!(page(at(4), 11, 10, 3), Ok(None));
    assert_eq!(page(at(4), 4, 10, 0), Ok(None));
    assert_eq!(
        page(at(4), 3, 10, 0),
        Ok(None),
        "an empty page reads nothing"
    );
    assert_eq!(page(at(4), 3, 10, 3), refused);
    assert_eq!(page(at(4), 2, 10, 1), refused);
    assert_eq!(
        page(at(4), 1, 10, 1),
        refused,
        "a page of the `Create` entry alone has no fixed end"
    );
    assert_eq!(page(at(4), 1, 10, 2), refused);
    assert_eq!(page(at(4), 1, 10, 0), Ok(None));
    assert_eq!(page(at(1), 1, 10, 1), Ok(range(1, 1)));
    assert_eq!(page(at(1), 1, 10, 20), Ok(range(1, 10)));
}

#[test]
fn the_retained_length_counts_from_the_start_to_the_last_index() {
    assert_eq!(at(1).retained_length(idx(10)), 10);
    assert_eq!(at(4).retained_length(idx(10)), 7);
    assert_eq!(at(10).retained_length(idx(10)), 1);
    assert_eq!(at(11).retained_length(idx(10)), 0);
    assert_eq!(at(4).retained_length(OplogIndex::NONE), 0);
}

#[test]
fn the_create_entry_is_read_alone_only_when_it_is_below_the_start() {
    assert_eq!(at(1).create_read(), None);
    assert_eq!(at(2).create_read(), Some(OplogRange::CREATE));
    assert_eq!(OplogRange::CREATE, range(1, 1).unwrap());
}

fn checked_agent() -> AgentId {
    AgentId::from_parts(7, 2)
}

#[test]
fn a_read_below_the_start_is_expired_except_an_empty_read_and_the_create_entry_alone() {
    let agent_id = checked_agent();
    let check = |retention: OplogRetention, start, count| {
        retention
            .check_agent_read(&agent_id, start, count)
            .map_err(|unreadable| {
                assert_eq!(unreadable.agent_id, agent_id);
                unreadable.error
            })
    };
    assert_eq!(check(at(4), idx(4), 3), Ok(()));
    assert_eq!(check(at(4), idx(5), 3), Ok(()));
    assert_eq!(check(at(4), idx(3), 3), expired(4));
    assert_eq!(check(at(4), idx(3), 1), expired(4));
    assert_eq!(check(at(4), idx(1), 1), Ok(()));
    assert_eq!(check(at(4), idx(1), 2), expired(4));
    assert_eq!(check(at(4), idx(2), 1), expired(4));
    assert_eq!(check(at(4), idx(2), 0), Ok(()));
    assert_eq!(check(at(4), OplogIndex::NONE, 0), Ok(()));
    assert_eq!(check(at(4), OplogIndex::NONE, 1), expired(4));
    assert_eq!(check(at(1), idx(1), 10), Ok(()));
    assert_eq!(check(at(1), OplogIndex::NONE, 1), expired(1));
}

#[test]
fn a_read_without_an_end_below_the_start_is_expired_also_at_index_1() {
    let agent_id = checked_agent();
    let check = |retention: OplogRetention, start| {
        retention
            .check_agent_from(&agent_id, start)
            .map_err(|unreadable| {
                assert_eq!(unreadable.agent_id, agent_id);
                unreadable.error
            })
    };
    assert_eq!(check(at(4), idx(3)), expired(4));
    assert_eq!(check(at(4), idx(4)), Ok(()));
    assert_eq!(check(at(4), idx(5)), Ok(()));
    assert_eq!(check(at(4), idx(1)), expired(4));
    assert_eq!(check(at(1), idx(1)), Ok(()));
    assert_eq!(check(at(1), OplogIndex::NONE), expired(1));
}

#[test]
fn only_a_gap_that_ends_below_the_start_is_expired() {
    let agent_id = checked_agent();
    let classify = |retention: OplogRetention, error| {
        let unreadable = retention.classify_unreadable(UnreadableOplog::new(&agent_id, error));
        assert_eq!(unreadable.agent_id, agent_id);
        unreadable.error
    };
    let gap = |start, end| OplogReadError::Gap {
        start: idx(start),
        end: idx(end),
    };
    let expired = OplogReadError::Expired {
        retained_start: idx(4),
    };
    assert_eq!(classify(at(4), gap(1, 3)), expired);
    assert_eq!(classify(at(4), gap(1, 1)), expired);
    assert_eq!(classify(at(4), gap(2, 4)), gap(2, 4));
    assert_eq!(classify(at(4), gap(2, 5)), gap(2, 5));
    assert_eq!(classify(at(4), gap(4, 5)), gap(4, 5));
    assert_eq!(classify(at(4), gap(6, 8)), gap(6, 8));
    assert_eq!(classify(at(3), gap(1, 3)), gap(1, 3));
    let corrupt = OplogReadError::corruption(OplogReadSource::Primary, "bad");
    assert_eq!(classify(at(4), corrupt.clone()), corrupt);
}

/// A record that does not decode, and a record with a start of 0, are corrupt. A record with
/// a start of 1 is the full retention.
#[test]
async fn a_record_that_does_not_decode_or_has_a_start_of_0_is_corrupt() {
    let storage = InMemoryKeyValueStorage::new();
    let owned_agent_id = OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &AgentId::from_parts(7, 3),
    );
    let write = |encoded: Vec<u8>| {
        let storage = &storage;
        let owned_agent_id = &owned_agent_id;
        async move {
            storage
                .set(
                    SVC_NAME,
                    "test",
                    ENTITY_NAME,
                    namespace(owned_agent_id),
                    field(AgentMode::Durable),
                    &encoded,
                )
                .await
                .unwrap()
        }
    };
    let encoded = |start: u64| {
        serialize(&OplogRetention {
            retained_start: idx(start),
        })
        .unwrap()
    };
    let corrupt = |encoded: Vec<u8>| async {
        write(encoded).await;
        let error = read_oplog_retention(&storage, &owned_agent_id, AgentMode::Durable)
            .await
            .unwrap_err();
        assert!(
            matches!(error.error, OplogReadError::Corruption { .. }),
            "{error:?}"
        );
    };
    // The version byte of the encoding without the data that follows it.
    corrupt(encoded(1)[..1].to_vec()).await;
    corrupt(encoded(0)).await;
    write(encoded(1)).await;
    assert_eq!(
        read_oplog_retention(&storage, &owned_agent_id, AgentMode::Durable).await,
        Ok(OplogRetention::FULL)
    );
}

/// One race of two writers that advance the start to 3 and to 5. `order` gives the order of
/// the first record read (`false`) and the first compare-and-set (`true`) of each writer. Each
/// step waits until the storage call of the step before it is done. `lost` names the writer
/// whose first compare-and-set reaches the storage and then loses its answer. The race starts
/// from the stored start `initial`, or from an absent record.
async fn race(initial: Option<u64>, order: [(usize, bool); 4], lost: Option<usize>) {
    const TARGETS: [u64; 2] = [3, 5];
    let inner: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(InMemoryKeyValueStorage::new());
    let owned_agent_id = &OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &AgentId::from_parts(7, 3),
    );
    if let Some(initial) = initial {
        advance_oplog_retention(
            inner.as_ref(),
            owned_agent_id,
            AgentMode::Durable,
            idx(initial),
        )
        .await
        .unwrap();
    }
    let faults: [KeyValueStorageFaults; 2] = Default::default();
    let storages = faults
        .clone()
        .map(|faults| FaultInjectingKeyValueStorage::new(inner.clone(), faults));
    let reads = faults
        .each_ref()
        .map(|faults| faults.pause_next("read_retention"));
    let writes: [_; 2] = std::array::from_fn(|writer| {
        if lost == Some(writer) {
            faults[writer].apply_before_failing();
            faults[writer].gate_next(
                "advance_retention",
                KeyValueStorageError::other("the answer is lost"),
            )
        } else {
            faults[writer].pause_next("advance_retention")
        }
    });
    let done: [tokio::sync::Notify; 2] = Default::default();
    let (reads, writes, done) = (&reads, &writes, &done);
    let writer = |writer: usize| {
        let storage = &storages[writer];
        async move {
            let result = advance_oplog_retention(
                storage,
                owned_agent_id,
                AgentMode::Durable,
                idx(TARGETS[writer]),
            )
            .await;
            done[writer].notify_one();
            result
        }
    };
    // `parked` holds, for each writer, whether its first compare-and-set waits at its gate.
    // A writer that read a start at or after its target writes nothing and is done.
    let driver =
        futures::stream::iter(order).fold([false; 2], |mut parked, (writer, write)| async move {
            if write {
                if parked[writer] {
                    writes[writer].release();
                    done[writer].notified().await;
                }
            } else {
                reads[writer].entered().await;
                reads[writer].release();
                parked[writer] = matches!(
                    futures::future::select(
                        std::pin::pin!(writes[writer].entered()),
                        std::pin::pin!(done[writer].notified()),
                    )
                    .await,
                    futures::future::Either::Left(_)
                );
            }
            parked
        });
    let (first, second, _) = futures::join!(writer(0), writer(1), driver);
    let stored = read_oplog_retention(inner.as_ref(), owned_agent_id, AgentMode::Durable)
        .await
        .unwrap();
    let case = format!("initial {initial:?}, order {order:?}, lost answer {lost:?}");
    [&first, &second]
        .into_iter()
        .zip(TARGETS)
        .enumerate()
        .for_each(|(writer, (result, target))| match result {
            Ok(given) => {
                assert!(given.start() >= idx(target), "{case}: {given:?}");
                assert!(stored.start() >= given.start(), "{case}: {stored:?}");
            }
            Err(error) => assert_eq!(lost, Some(writer), "{case}: {error:?}"),
        });
    assert!(
        stored.start() >= idx(initial.unwrap_or(1)),
        "{case}: {stored:?}"
    );
    if lost.is_none() {
        assert_eq!(stored, at(5), "{case}");
    }
}

/// Two writers advance the start at the same time, in each order of their record reads and
/// compare-and-set writes, from an absent record and from a stored start of 2. In each order
/// the first compare-and-set of either writer can also lose its answer after the storage
/// applied it. The stored start never moves back: it is at or after the start that each
/// writer gave, and each writer that gave a start gave one at or after its target. The space
/// is small, so the test runs every case.
#[test]
async fn two_writers_never_move_the_start_back_in_any_order() {
    let orders = (0..4)
        .flat_map(|read| (read + 1..4).map(move |write| (read, write)))
        .map(|(read, write)| {
            let other_read = (0..4)
                .find(|position| *position != read && *position != write)
                .unwrap();
            std::array::from_fn(|position| match position {
                position if position == read => (0, false),
                position if position == write => (0, true),
                position if position == other_read => (1, false),
                _ => (1, true),
            })
        })
        .collect::<Vec<[(usize, bool); 4]>>();
    assert_eq!(orders.len(), 6);
    let cases = orders.into_iter().flat_map(|order| {
        [None, Some(2)].into_iter().flat_map(move |initial| {
            [None, Some(0), Some(1)]
                .into_iter()
                .map(move |lost| (initial, order, lost))
        })
    });
    futures::stream::iter(cases)
        .for_each(|(initial, order, lost)| race(initial, order, lost))
        .await;
}

/// The record holds a start of 2. A writer that read it before another writer moved the start
/// to 5 loses its compare-and-set, reads the record again, and keeps the start at 5 without a
/// second write.
#[test]
async fn a_stale_writer_does_not_move_the_start_back() {
    let faults = KeyValueStorageFaults::default();
    let inner: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(InMemoryKeyValueStorage::new());
    let storage = FaultInjectingKeyValueStorage::new(inner, faults.clone());
    let owned_agent_id = OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &AgentId::from_parts(7, 3),
    );
    // A record exists before the race, so both writers compare a stored start.
    advance_oplog_retention(&storage, &owned_agent_id, AgentMode::Durable, idx(2))
        .await
        .unwrap();
    let gate = faults.pause_next("advance_retention");
    let (stale, other) = futures::join!(
        advance_oplog_retention(&storage, &owned_agent_id, AgentMode::Durable, idx(3)),
        async {
            gate.entered().await;
            let other =
                advance_oplog_retention(&storage, &owned_agent_id, AgentMode::Durable, idx(5))
                    .await;
            gate.release();
            other
        }
    );
    assert_eq!(other.unwrap(), at(5));
    assert_eq!(stale.unwrap(), at(5));
    assert_eq!(
        faults.calls("advance_retention"),
        3,
        "the stale writer lost its compare-and-set and wrote nothing after it read 5"
    );
    assert_eq!(
        read_oplog_retention(&storage, &owned_agent_id, AgentMode::Durable)
            .await
            .unwrap(),
        at(5)
    );
    assert_eq!(
        read_oplog_retention(&storage, &owned_agent_id, AgentMode::Ephemeral)
            .await
            .unwrap(),
        OplogRetention::FULL,
        "each agent mode has its own record"
    );
}
