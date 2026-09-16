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

use golem_common::model::OplogIndex;
use golem_common::model::durable_stream::StreamOffset;

/// An exact stream prefix. Complete earlier records keep their original positions; only the
/// boundary batch may be shortened. Empty prefixes retain the stream's registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamForkCut {
    /// Physical inclusive agent cut, including a terminal when that was the requested cursor.
    pub oplog_index: OplogIndex,
    /// Retained items in the batch identified by `last_item_offset`, not by `oplog_index`.
    pub retained_boundary_items: u32,
    pub retained_items: u64,
    pub last_item_offset: Option<StreamOffset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamForkCutError {
    UnknownOffset,
    BeyondHead,
    BeyondDataBoundary,
    InvalidHistory,
}

/// Resolve against one fixed, validated source snapshot, never a moving live tail. Batch lengths
/// count logical messages for Values and individual bytes for PackedU8. The caller supplies only
/// batches belonging to the selected stream, after applying its fork/revert lineage.
///
/// Public Golem cursors encode the last delivered event: reading *after* that cursor resumes at
/// the next item. Consequently an anchor retains that item, and the sub-offset counts additional
/// items after it. Terminal cursors preserve their physical cut; closure is reset by lineage,
/// not by moving the cut backwards. An omitted anchor resolves to the snapshot's tail.
/// The zero cursor denotes the empty prefix.
///
/// `max_sub_offset` is the semantic content extent after the anchor: binary sub-offsets cannot
/// cross the next protocol data boundary. That boundary need not equal an internal oplog batch.
/// JSON callers supply the remaining flattened message count. Tail bounds are also checked here.
pub fn resolve_stream_fork_cut(
    registration_index: OplogIndex,
    batches: &[(OplogIndex, u32)],
    terminal: Option<StreamOffset>,
    anchor: Option<StreamOffset>,
    sub_offset: u64,
    max_sub_offset: u64,
) -> Result<StreamForkCut, StreamForkCutError> {
    let origin = StreamOffset::new(OplogIndex::NONE, 0);
    let mut total = 0u64;
    let mut previous = registration_index;
    let mut anchor_items = (anchor == Some(origin)).then_some(0);
    for &(index, length) in batches {
        if index <= previous || length == 0 {
            return Err(StreamForkCutError::InvalidHistory);
        }
        if let Some(anchor) = anchor
            && anchor.producer_oplog_index() == index
            && anchor.sub_index() < length
        {
            anchor_items = total.checked_add(u64::from(anchor.sub_index()) + 1);
        }
        total = total
            .checked_add(u64::from(length))
            .ok_or(StreamForkCutError::InvalidHistory)?;
        previous = index;
    }
    if let Some(terminal) = terminal
        && (terminal.producer_oplog_index() <= previous || terminal.sub_index() != 0)
    {
        return Err(StreamForkCutError::InvalidHistory);
    }
    if anchor.is_none() || (terminal.is_some() && anchor == terminal) {
        anchor_items = Some(total);
    }
    let retained_items = anchor_items
        .ok_or(StreamForkCutError::UnknownOffset)?
        .checked_add(sub_offset)
        .filter(|count| *count <= total)
        .ok_or(StreamForkCutError::BeyondHead)?;
    if sub_offset > max_sub_offset {
        return Err(StreamForkCutError::BeyondDataBoundary);
    }
    let terminal_cut = terminal.filter(|_| anchor.is_none() || anchor == terminal);
    let mut remaining = retained_items;
    for &(index, length) in batches {
        if remaining == 0 {
            break;
        }
        if remaining <= u64::from(length) {
            let retained_boundary_items = remaining as u32;
            return Ok(StreamForkCut {
                oplog_index: terminal_cut.map_or(index, StreamOffset::producer_oplog_index),
                retained_boundary_items,
                retained_items,
                last_item_offset: Some(StreamOffset::new(index, retained_boundary_items - 1)),
            });
        }
        remaining -= u64::from(length);
    }
    Ok(StreamForkCut {
        oplog_index: terminal_cut.map_or(registration_index, StreamOffset::producer_oplog_index),
        retained_boundary_items: 0,
        retained_items: 0,
        last_item_offset: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn idx(value: u64) -> OplogIndex {
        OplogIndex::from_u64(value)
    }

    fn offset(index: u64, sub_index: u32) -> StreamOffset {
        StreamOffset::new(idx(index), sub_index)
    }

    fn resolve(
        registration_index: OplogIndex,
        batches: &[(OplogIndex, u32)],
        terminal: Option<StreamOffset>,
        anchor: Option<StreamOffset>,
        sub_offset: u64,
    ) -> Result<StreamForkCut, StreamForkCutError> {
        resolve_stream_fork_cut(
            registration_index,
            batches,
            terminal,
            anchor,
            sub_offset,
            u64::MAX,
        )
    }

    #[test]
    fn cursor_retains_delivered_item_and_sub_offset_crosses_oplog_gaps() {
        let batches = [(idx(7), 4), (idx(19), 3), (idx(31), 2)];
        let cut = resolve(idx(3), &batches, None, Some(offset(7, 1)), 0).unwrap();
        assert_eq!(cut.retained_items, 2);
        assert_eq!(cut.last_item_offset, Some(offset(7, 1)));
        assert_eq!(cut.retained_boundary_items, 2);

        let cut = resolve(idx(3), &batches, None, Some(offset(7, 1)), 4).unwrap();
        assert_eq!(cut.retained_items, 6);
        assert_eq!(cut.oplog_index, idx(19));
        assert_eq!(cut.retained_boundary_items, 2);
        assert_eq!(cut.last_item_offset, Some(offset(19, 1)));
    }

    #[test]
    fn empty_prefix_and_exact_batch_boundary_do_not_copy_next_batch() {
        let batches = [(idx(7), 4), (idx(19), 3)];
        let empty = resolve(idx(3), &batches, None, Some(offset(0, 0)), 0).unwrap();
        assert_eq!(empty.oplog_index, idx(3));
        assert_eq!(empty.retained_items, 0);
        assert_eq!(empty.last_item_offset, None);
        let cut = resolve(idx(3), &batches, None, Some(offset(0, 0)), 4).unwrap();
        assert_eq!(cut.oplog_index, idx(7));
        assert_eq!(cut.retained_boundary_items, 4);
        assert_eq!(cut.last_item_offset, Some(offset(7, 3)));
    }

    #[test]
    fn terminal_and_default_tail_retain_data_without_terminal() {
        let batches = [(idx(7), 4), (idx(19), 3)];
        for anchor in [None, Some(offset(22, 0)), Some(offset(19, 2))] {
            let cut = resolve(idx(3), &batches, Some(offset(22, 0)), anchor, 0).unwrap();
            assert_eq!(
                cut.oplog_index,
                if anchor == Some(offset(19, 2)) {
                    idx(19)
                } else {
                    idx(22)
                }
            );
            assert_eq!(cut.retained_items, 7);
            assert_eq!(cut.last_item_offset, Some(offset(19, 2)));
            assert_eq!(
                resolve(idx(3), &batches, Some(offset(22, 0)), anchor, 1),
                Err(StreamForkCutError::BeyondHead)
            );
        }
    }

    #[test]
    fn empty_closed_stream_retains_registration() {
        for anchor in [None, Some(offset(0, 0)), Some(offset(8, 0))] {
            let cut = resolve(idx(3), &[], Some(offset(8, 0)), anchor, 0).unwrap();
            assert_eq!(
                cut.oplog_index,
                if anchor == Some(offset(0, 0)) {
                    idx(3)
                } else {
                    idx(8)
                }
            );
            assert_eq!(cut.retained_items, 0);
            assert_eq!(cut.last_item_offset, None);
        }
    }

    #[test]
    fn rejects_unknown_cursor_overshoot_and_overflow() {
        let batches = [(idx(7), 4), (idx(19), 3)];
        for anchor in [offset(7, 4), offset(8, 0), offset(30, 0), offset(0, 1)] {
            assert_eq!(
                resolve(idx(3), &batches, None, Some(anchor), 0),
                Err(StreamForkCutError::UnknownOffset)
            );
        }
        for sub_offset in [6, u64::MAX] {
            assert_eq!(
                resolve(idx(3), &batches, None, Some(offset(7, 1)), sub_offset),
                Err(StreamForkCutError::BeyondHead)
            );
        }
    }

    #[test]
    fn rejects_invalid_batch_or_terminal_order() {
        for batches in [
            vec![(idx(3), 1)],
            vec![(idx(7), 0)],
            vec![(idx(7), 2), (idx(7), 1)],
            vec![(idx(9), 2), (idx(7), 1)],
        ] {
            assert_eq!(
                resolve(idx(3), &batches, None, None, 0),
                Err(StreamForkCutError::InvalidHistory)
            );
        }
        for terminal in [offset(7, 0), offset(9, 1)] {
            assert_eq!(
                resolve(idx(3), &[(idx(7), 2)], Some(terminal), None, 0),
                Err(StreamForkCutError::InvalidHistory)
            );
        }
    }

    #[test]
    fn semantic_byte_boundary_can_span_records_but_not_be_exceeded() {
        let batches = [(idx(7), 2), (idx(12), 3), (idx(18), 4)];
        let cut =
            resolve_stream_fork_cut(idx(3), &batches, None, Some(offset(0, 0)), 5, 5).unwrap();
        assert_eq!(cut.last_item_offset, Some(offset(12, 2)));
        assert_eq!(
            resolve_stream_fork_cut(idx(3), &batches, None, Some(offset(0, 0)), 6, 5),
            Err(StreamForkCutError::BeyondDataBoundary)
        );
    }
}
