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

use super::{OplogCursorRefused, OplogRange, OplogRangeEndsBeforeStart, OplogSearchResume};
use crate::base_model::OplogIndex;
use crate::model::oplog::OplogCursor;
use test_r::test;

fn index(value: u64) -> OplogIndex {
    OplogIndex::from_u64(value)
}

fn range(from: Option<u64>, to: Option<u64>) -> OplogRange {
    OplogRange::from_indices(from, to).unwrap()
}

fn cursor(next: u64) -> OplogCursor {
    OplogCursor {
        next_oplog_index: next,
        current_component_revision: 0,
        query_hash: String::new(),
        scan_to: None,
    }
}

/// The cursor of a search that scans on for the component revision from `next`, and then
/// searches from `scan_to`.
fn scan_cursor(next: u64, scan_to: u64) -> OplogCursor {
    OplogCursor {
        scan_to: Some(scan_to),
        ..cursor(next)
    }
}

#[test]
fn a_range_keeps_an_open_start_and_refuses_an_end_before_its_start() {
    let refused = |start, end| {
        Err(OplogRangeEndsBeforeStart {
            start: index(start),
            end: index(end),
        })
    };
    [
        ((None, None), Ok((None, None))),
        ((Some(0), None), Ok((None, None))),
        ((Some(1), None), Ok((Some(1), None))),
        ((Some(7), None), Ok((Some(7), None))),
        ((None, Some(1)), Ok((None, Some(1)))),
        ((Some(0), Some(1)), Ok((None, Some(1)))),
        ((Some(5), Some(5)), Ok((Some(5), Some(5)))),
        ((Some(5), Some(u64::MAX)), Ok((Some(5), Some(u64::MAX)))),
        ((Some(5), Some(4)), refused(5, 4)),
        ((None, Some(0)), refused(1, 0)),
        ((Some(0), Some(0)), refused(1, 0)),
    ]
    .into_iter()
    .for_each(|((from, to), expected)| {
        assert_eq!(
            OplogRange::from_indices(from, to).map(|range| range.to_indices()),
            expected,
            "from {from:?} to {to:?}"
        )
    });
    assert_eq!(
        OplogRange::from_indices(Some(5), Some(4)).map_err(|error| error.end()),
        Err(index(4))
    );
    assert_eq!(OplogRange::new(None, None), Ok(OplogRange::ALL));
    assert_eq!(
        OplogRange::new(Some(index(3)), Some(index(9))),
        OplogRange::from_indices(Some(3), Some(9))
    );
}

#[test]
fn a_read_of_a_range_can_start_only_at_its_start_or_anywhere_without_a_start() {
    [
        (range(None, None), vec![(1, true), (7, true)]),
        (range(Some(0), None), vec![(1, true)]),
        (range(None, Some(9)), vec![(1, true), (9, true)]),
        (
            range(Some(4), None),
            vec![(3, false), (4, true), (5, false)],
        ),
        (
            range(Some(4), Some(9)),
            vec![(1, false), (4, true), (9, false)],
        ),
    ]
    .into_iter()
    .for_each(|(range, first_entries)| {
        first_entries.into_iter().for_each(|(first, expected)| {
            assert_eq!(
                range.can_start_at(index(first)),
                expected,
                "{range:?} with a first entry at {first}"
            )
        });
    });
}

#[test]
fn a_range_starts_at_its_start_or_at_the_first_entry_of_the_oplog() {
    [
        ((range(None, None), 1), 1),
        ((range(None, None), 6), 6),
        ((range(None, Some(9)), 6), 6),
        ((range(Some(4), None), 1), 4),
        ((range(Some(4), None), 6), 4),
    ]
    .into_iter()
    .for_each(|((range, oplog_first), expected)| {
        assert_eq!(
            range.first_index_in(index(oplog_first)),
            index(expected),
            "{range:?} in an oplog that starts at {oplog_first}"
        )
    });
}

#[test]
fn a_range_resumes_at_a_cursor_within_it_and_refuses_a_cursor_outside_it() {
    let outside = |range: OplogRange, next| {
        Err(OplogCursorRefused::OutsideRange {
            next: index(next),
            range,
        })
    };
    [
        ((range(Some(5), Some(9)), 5), Ok(range(Some(5), Some(9)))),
        ((range(Some(5), Some(9)), 8), Ok(range(Some(8), Some(9)))),
        ((range(Some(5), Some(9)), 9), Ok(range(Some(9), Some(9)))),
        (
            (range(Some(5), Some(9)), 4),
            outside(range(Some(5), Some(9)), 4),
        ),
        (
            (range(Some(5), Some(9)), 10),
            outside(range(Some(5), Some(9)), 10),
        ),
        (
            (range(Some(5), Some(9)), u64::MAX),
            outside(range(Some(5), Some(9)), u64::MAX),
        ),
        ((range(Some(5), None), 1), outside(range(Some(5), None), 1)),
        (
            (range(Some(5), None), u64::MAX),
            Ok(range(Some(u64::MAX), None)),
        ),
        ((OplogRange::ALL, 1), Ok(range(Some(1), None))),
        ((OplogRange::ALL, 7), Ok(range(Some(7), None))),
        ((OplogRange::ALL, 0), outside(OplogRange::ALL, 0)),
        ((range(None, Some(9)), 9), Ok(range(Some(9), Some(9)))),
        (
            (range(None, Some(9)), 10),
            outside(range(None, Some(9)), 10),
        ),
    ]
    .into_iter()
    .for_each(|((range, next), expected)| {
        assert_eq!(range.resume(&cursor(next)), expected, "{range:?} at {next}");
        assert_eq!(
            range.resume_search(&cursor(next)),
            expected.map(OplogSearchResume::Read),
            "a search of {range:?} at {next}"
        )
    });
}

#[test]
fn a_search_continues_its_revision_scan_only_before_the_start_of_its_range() {
    let scan = |next, start, end| {
        Ok(OplogSearchResume::Scan {
            next: index(next),
            range: range(Some(start), end),
        })
    };
    let refused = |range: OplogRange, next, scan_to| {
        Err(OplogCursorRefused::ScanOutsideRange {
            next: index(next),
            scan_to,
            range,
        })
    };
    [
        // A range without a start takes the search start of the cursor as its start.
        ((range(None, None), 3, 10), scan(3, 10, None)),
        ((range(None, Some(20)), 3, 10), scan(3, 10, Some(20))),
        ((range(None, Some(10)), 3, 10), scan(3, 10, Some(10))),
        (
            (range(None, Some(9)), 3, 10),
            refused(range(None, Some(9)), 3, 10),
        ),
        // A range with a start must start at the search start of the cursor.
        ((range(Some(10), Some(20)), 3, 10), scan(3, 10, Some(20))),
        ((range(Some(10), None), 1, 10), scan(1, 10, None)),
        ((range(Some(10), None), 9, 10), scan(9, 10, None)),
        (
            (range(Some(11), None), 3, 10),
            refused(range(Some(11), None), 3, 10),
        ),
        (
            (range(Some(9), None), 3, 10),
            refused(range(Some(9), None), 3, 10),
        ),
        // The cursor points to an index from 1 to before its search start.
        (
            (range(Some(10), None), 10, 10),
            refused(range(Some(10), None), 10, 10),
        ),
        (
            (range(Some(10), None), 12, 10),
            refused(range(Some(10), None), 12, 10),
        ),
        (
            (range(Some(10), None), 0, 10),
            refused(range(Some(10), None), 0, 10),
        ),
        ((range(None, None), 0, 0), refused(range(None, None), 0, 0)),
    ]
    .into_iter()
    .for_each(|((range, next, scan_to), expected)| {
        assert_eq!(
            range.resume_search(&scan_cursor(next, scan_to)),
            expected,
            "a search of {range:?} at {next} to {scan_to}"
        )
    });
    // A page read refuses the cursor of a search that scans for the component revision.
    [
        (
            (range(None, None), 3, 10),
            Err(OplogCursorRefused::SearchScan {
                next: index(3),
                start: index(10),
            }),
        ),
        (
            (range(Some(10), Some(20)), 9, 10),
            Err(OplogCursorRefused::SearchScan {
                next: index(9),
                start: index(10),
            }),
        ),
        (
            (range(Some(11), None), 3, 10),
            Err(OplogCursorRefused::ScanOutsideRange {
                next: index(3),
                scan_to: 10,
                range: range(Some(11), None),
            }),
        ),
    ]
    .into_iter()
    .for_each(|((range, next, scan_to), expected)| {
        assert_eq!(
            range.resume(&scan_cursor(next, scan_to)),
            expected,
            "a page of {range:?} at {next} to {scan_to}"
        )
    });
}

#[test]
fn a_refused_cursor_tells_where_it_points_and_the_range_that_refused_it() {
    [
        (
            range(Some(5), Some(9)).resume(&cursor(4)),
            "the oplog cursor points to index 4, outside the oplog range from 5 to 9",
        ),
        (
            range(Some(5), None).resume(&cursor(4)),
            "the oplog cursor points to index 4, before the oplog range from 5",
        ),
        (
            range(None, None).resume(&scan_cursor(3, 10)),
            "the oplog cursor at index 3 continues a search from index 10, not a page",
        ),
        (
            range(Some(11), Some(20)).resume(&scan_cursor(3, 10)),
            "the oplog cursor at index 3, which continues a search from index 10, does not \
             fit the oplog range from 11 to 20",
        ),
        (
            range(Some(11), None).resume(&scan_cursor(3, 10)),
            "the oplog cursor at index 3, which continues a search from index 10, does not \
             fit the oplog range from 11",
        ),
        (
            range(None, Some(9)).resume(&scan_cursor(3, 10)),
            "the oplog cursor at index 3, which continues a search from index 10, does not \
             fit the oplog range to 9",
        ),
        (
            range(None, None).resume(&scan_cursor(10, 10)),
            "the oplog cursor at index 10, which continues a search from index 10, does not \
             fit the oplog range",
        ),
    ]
    .into_iter()
    .for_each(|(refused, message)| assert_eq!(refused.unwrap_err().to_string(), message));
}

#[test]
fn a_page_counts_its_limit_or_the_entries_up_to_the_end_of_the_range() {
    [
        ((range(None, None), 1, 50), 50),
        ((range(None, None), 1, u64::MAX), usize::MAX),
        ((range(None, Some(1)), 1, 50), 1),
        ((range(None, Some(7)), 3, 50), 5),
        ((range(None, Some(7)), 3, 2), 2),
        ((range(None, Some(7)), 7, 50), 1),
        ((range(None, Some(7)), 8, 50), 0),
        ((range(None, Some(7)), 9, 50), 0),
        ((range(None, Some(7)), 3, 0), 0),
        ((range(None, Some(u64::MAX)), 1, u64::MAX), usize::MAX),
        ((range(None, Some(u64::MAX)), u64::MAX, 50), 1),
    ]
    .into_iter()
    .for_each(|((range, from, limit), expected)| {
        assert_eq!(
            range.page_count(index(from), limit),
            expected,
            "{range:?} from {from}, at most {limit}"
        )
    });
}

#[test]
fn a_range_ends_at_its_end_or_at_the_end_of_the_oplog() {
    [
        ((range(None, None), 10), 10),
        ((range(None, Some(4)), 10), 4),
        ((range(None, Some(10)), 10), 10),
        ((range(None, Some(11)), 10), 10),
        ((range(None, Some(u64::MAX)), 10), 10),
    ]
    .into_iter()
    .for_each(|((range, oplog_last), expected)| {
        assert_eq!(
            range.last_index_in(index(oplog_last)),
            index(expected),
            "{range:?} in an oplog that ends at {oplog_last}"
        )
    });
}

#[test]
fn a_closed_range_ends_at_the_last_index_of_the_range_in_the_oplog() {
    [
        ((range(None, None), 10), Some(range(None, Some(10)))),
        ((range(Some(4), None), 10), Some(range(Some(4), Some(10)))),
        ((range(Some(4), Some(7)), 10), Some(range(Some(4), Some(7)))),
        (
            (range(Some(4), Some(12)), 10),
            Some(range(Some(4), Some(10))),
        ),
        ((range(Some(4), None), 4), Some(range(Some(4), Some(4)))),
        ((range(Some(4), None), 3), None),
        ((range(None, None), 0), None),
    ]
    .into_iter()
    .for_each(|((range, oplog_last), expected)| {
        assert_eq!(
            range.close_in(index(oplog_last)),
            expected,
            "{range:?} in an oplog that ends at {oplog_last}"
        )
    });
}

#[test]
fn a_range_is_empty_in_an_oplog_that_ends_before_its_start() {
    [
        ((range(Some(5), None), 4), true),
        ((range(Some(5), None), 5), false),
        ((range(Some(5), Some(9)), 4), true),
        ((range(Some(5), Some(9)), 7), false),
        ((range(None, None), 0), true),
        ((range(None, None), 1), false),
        ((range(None, Some(3)), 1), false),
    ]
    .into_iter()
    .for_each(|((range, oplog_last), expected)| {
        assert_eq!(
            range.is_empty_in(index(oplog_last)),
            expected,
            "{range:?} in an oplog that ends at {oplog_last}"
        )
    });
}
