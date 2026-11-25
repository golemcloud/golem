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

use super::OplogCursor;
use crate::base_model::OplogIndex;
use std::fmt::{Display, Formatter};

/// The oplog indices that a read selects, from `start` to `end`, including both. A range without
/// a start starts at the first entry that the oplog holds, and the reader of the oplog finds that
/// entry. A start is never 0. A range without an end continues to the end of the oplog. An end
/// after the end of the oplog does not cause an error: the read stops at the end of the oplog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogRange {
    start: Option<OplogIndex>,
    end: Option<OplogIndex>,
}

/// The error of a range whose end is before its start. A range without a start has the start 1
/// in this error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogRangeEndsBeforeStart {
    start: OplogIndex,
    end: OplogIndex,
}

impl Display for OplogRangeEndsBeforeStart {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the oplog range ends at {}, before its start at {}",
            self.end, self.start
        )
    }
}

impl OplogRangeEndsBeforeStart {
    /// The end of the range.
    pub fn end(&self) -> OplogIndex {
        self.end
    }
}

impl std::error::Error for OplogRangeEndsBeforeStart {}

/// The error of a cursor that a read of a range refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OplogCursorRefused {
    /// The next index of the cursor is not in the range of the read. The cursor came from a read
    /// of another range.
    OutsideRange { next: OplogIndex, range: OplogRange },
    /// A page read got the cursor of a search that scans for the component revision at `start`,
    /// the start of its range.
    SearchScan { next: OplogIndex, start: OplogIndex },
    /// The search start of the cursor is not the start of the range of the read. Or the cursor
    /// does not point to an index from 1 to before its search start. The cursor came from a
    /// search of another range.
    ScanOutsideRange {
        next: OplogIndex,
        scan_to: u64,
        range: OplogRange,
    },
}

impl Display for OplogCursorRefused {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideRange { next, range } => {
                let start = range.first_possible();
                write!(f, "the oplog cursor points to index {next}, ")?;
                match range.end {
                    Some(end) => write!(f, "outside the oplog range from {start} to {end}"),
                    None => write!(f, "before the oplog range from {start}"),
                }
            }
            Self::SearchScan { next, start } => write!(
                f,
                "the oplog cursor at index {next} continues a search from index {start}, not a \
                 page"
            ),
            Self::ScanOutsideRange {
                next,
                scan_to,
                range,
            } => {
                write!(
                    f,
                    "the oplog cursor at index {next}, which continues a search from index \
                     {scan_to}, does not fit the oplog range"
                )?;
                match (range.start, range.end) {
                    (Some(start), Some(end)) => write!(f, " from {start} to {end}"),
                    (Some(start), None) => write!(f, " from {start}"),
                    (None, Some(end)) => write!(f, " to {end}"),
                    (None, None) => Ok(()),
                }
            }
        }
    }
}

impl OplogCursorRefused {
    /// The next index of the cursor.
    pub fn next(&self) -> OplogIndex {
        match self {
            Self::OutsideRange { next, .. }
            | Self::SearchScan { next, .. }
            | Self::ScanOutsideRange { next, .. } => *next,
        }
    }
}

impl std::error::Error for OplogCursorRefused {}

/// Where a search of a range continues from a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OplogSearchResume {
    /// The search did not find the component revision at the start of `range` yet. It scans on
    /// for the revision from `next`, and then searches `range`, which has a start.
    Scan { next: OplogIndex, range: OplogRange },
    /// The search reads the rest of the range from the cursor. The rest has a start.
    Read(OplogRange),
}

impl OplogRange {
    /// Every entry of the oplog.
    pub const ALL: OplogRange = OplogRange {
        start: None,
        end: None,
    };

    /// The range from `from` to `to`. Without `from`, or with `from` at index 0, the range has no
    /// start. Without `to`, the range continues to the end of the oplog. A `to` before the start
    /// gives an error. For this check, a range without a start starts at 1, because an oplog has no
    /// entry before index 1.
    pub fn new(
        from: Option<OplogIndex>,
        to: Option<OplogIndex>,
    ) -> Result<Self, OplogRangeEndsBeforeStart> {
        let range = Self {
            start: from.filter(OplogIndex::is_defined),
            end: to,
        };
        let start = range.first_possible();
        match to {
            Some(end) if end < start => Err(OplogRangeEndsBeforeStart { start, end }),
            _ => Ok(range),
        }
    }

    /// The range of the `from` and `to` indices of a request, with the rules of [`Self::new`].
    pub fn from_indices(
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Self, OplogRangeEndsBeforeStart> {
        Self::new(from.map(OplogIndex::from_u64), to.map(OplogIndex::from_u64))
    }

    /// The `from` and `to` indices of a request for this range. [`Self::from_indices`] makes the
    /// same range from them.
    pub fn to_indices(&self) -> (Option<u64>, Option<u64>) {
        (
            self.start.map(|start| start.as_u64()),
            self.end.map(|end| end.as_u64()),
        )
    }

    /// Whether the range has a start. A range without a start starts at the first entry that the
    /// oplog holds.
    pub fn has_start(&self) -> bool {
        self.start.is_some()
    }

    /// Whether the range has an end. A range without an end continues to the end of the oplog.
    pub fn has_end(&self) -> bool {
        self.end.is_some()
    }

    /// Whether a read of the range can give its first entry at `index`. A range with a start gives
    /// its first entry only at its start. A range without a start can give it at any index. Only
    /// the reader of the oplog knows the first entry that the oplog holds.
    pub fn can_start_at(&self, index: OplogIndex) -> bool {
        self.start.is_none_or(|start| start == index)
    }

    /// The first index of the range in an oplog whose first entry is at `oplog_first`. It is the
    /// start of the range, or `oplog_first` when the range has no start. An `oplog_first` of 0
    /// counts as 1, because an oplog has no entry before index 1.
    pub fn first_index_in(&self, oplog_first: OplogIndex) -> OplogIndex {
        self.start.unwrap_or(oplog_first.max(OplogIndex::INITIAL))
    }

    /// The first index that the range can hold: its start, or 1 when it has no start. An oplog has
    /// no entry before index 1.
    fn first_possible(&self) -> OplogIndex {
        self.first_index_in(OplogIndex::INITIAL)
    }

    /// The last index of the range in an oplog whose last index is `oplog_last`. It is the end of
    /// the range. It is `oplog_last` when the range has no end or ends after it.
    pub fn last_index_in(&self, oplog_last: OplogIndex) -> OplogIndex {
        self.end.map_or(oplog_last, |end| end.min(oplog_last))
    }

    /// Whether the range holds no index of an oplog whose last index is `oplog_last`: the range
    /// starts after it. A range without a start starts at 1 for this rule, so it is empty only
    /// when `oplog_last` is 0.
    pub fn is_empty_in(&self, oplog_last: OplogIndex) -> bool {
        oplog_last < self.first_possible()
    }

    /// The range with the same start that ends at the last index of the range in an oplog whose
    /// last index is `oplog_last`. A read of the result stops there, also when the oplog grows
    /// later. The result is `None` when the range holds no index of the oplog.
    pub fn close_in(&self, oplog_last: OplogIndex) -> Option<Self> {
        (!self.is_empty_in(oplog_last)).then(|| Self {
            start: self.start,
            end: Some(self.last_index_in(oplog_last)),
        })
    }

    /// The rest of the range from the next index of `cursor`, for a page read. A cursor before
    /// the start or after the end of the range gives an error, because a page of this range never
    /// gives it. A range without a start starts at 1 for this check. The cursor of a search that
    /// scans for the component revision gives an error too. The rest of the range has a start.
    pub fn resume(&self, cursor: &OplogCursor) -> Result<Self, OplogCursorRefused> {
        match self.resume_search(cursor)? {
            OplogSearchResume::Read(rest) => Ok(rest),
            OplogSearchResume::Scan { next, range } => Err(OplogCursorRefused::SearchScan {
                next,
                start: range.first_possible(),
            }),
        }
    }

    /// Where a search of the range continues from `cursor`. A cursor without `scan_to` continues
    /// the search in the range, with the rule of [`Self::resume`]. A cursor with `scan_to` continues
    /// the scan for the component revision. A range with a start must start at `scan_to`, and a
    /// range without a start takes `scan_to` as its start. The cursor must point to an index from 1
    /// to before `scan_to`, and the range must not end before `scan_to`.
    pub fn resume_search(
        &self,
        cursor: &OplogCursor,
    ) -> Result<OplogSearchResume, OplogCursorRefused> {
        let next = OplogIndex::from_u64(cursor.next_oplog_index);
        match cursor.scan_to {
            None => {
                let outside =
                    next < self.first_possible() || self.end.is_some_and(|end| next > end);
                if outside {
                    Err(OplogCursorRefused::OutsideRange { next, range: *self })
                } else {
                    Ok(OplogSearchResume::Read(Self {
                        start: Some(next),
                        end: self.end,
                    }))
                }
            }
            Some(scan_to) => {
                let start = OplogIndex::from_u64(scan_to);
                let fits = self.can_start_at(start)
                    && OplogIndex::INITIAL <= next
                    && next < start
                    && self.end.is_none_or(|end| start <= end);
                if fits {
                    Ok(OplogSearchResume::Scan {
                        next,
                        range: Self {
                            start: Some(start),
                            end: self.end,
                        },
                    })
                } else {
                    Err(OplogCursorRefused::ScanOutsideRange {
                        next,
                        scan_to,
                        range: *self,
                    })
                }
            }
        }
    }

    /// The count of the entries that a page of at most `limit` entries reads from `from`: `limit`,
    /// or the count of the indices from `from` to the end of the range, when that is smaller. It
    /// is 0 when `from` is after the end. A count that does not fit in `usize` gives `usize::MAX`.
    pub fn page_count(&self, from: OplogIndex, limit: u64) -> usize {
        let in_range = self.end.map_or(u64::MAX, |end| {
            end.as_u64()
                .checked_sub(from.as_u64())
                .map_or(0, |distance| distance.saturating_add(1))
        });
        usize::try_from(limit.min(in_range)).unwrap_or(usize::MAX)
    }

    /// The part of the range that a page of at most `count` entries reads, in an oplog whose first
    /// entry is at `oplog_first`. The page starts at the first index of the range. It ends at the
    /// last entry of the page, as [`Self::page_count`] counts it. A page reads at least one entry,
    /// so a `count` of 0 gives the first index only. A range without an end can have a page that
    /// reaches index `u64::MAX`, or a page whose end overflows. Such a page has no end either, so
    /// a page to the end of such a range has one value. When the first index is after the end of
    /// the range, the range has no page in the oplog, and the result is `None`.
    pub fn page_in(&self, oplog_first: OplogIndex, count: u64) -> Option<Self> {
        let start = self.first_index_in(oplog_first);
        if self.end.is_some_and(|end| end < start) {
            return None;
        }
        let entries = u64::try_from(self.page_count(start, count))
            .unwrap_or(u64::MAX)
            .max(1);
        let end = start
            .as_u64()
            .checked_add(entries - 1)
            .filter(|page_end| self.end.is_some() || *page_end < u64::MAX)
            .map(OplogIndex::from_u64)
            .or(self.end);
        Some(Self {
            start: Some(start),
            end,
        })
    }
}

#[cfg(test)]
mod tests {
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
    fn a_range_tells_if_it_has_a_start_and_an_end_and_where_a_read_of_it_can_start() {
        [
            (
                range(None, None),
                (false, false),
                vec![(1, true), (7, true)],
            ),
            (range(Some(0), None), (false, false), vec![(1, true)]),
            (
                range(None, Some(9)),
                (false, true),
                vec![(1, true), (9, true)],
            ),
            (
                range(Some(4), None),
                (true, false),
                vec![(3, false), (4, true), (5, false)],
            ),
            (
                range(Some(4), Some(9)),
                (true, true),
                vec![(1, false), (4, true), (9, false)],
            ),
        ]
        .into_iter()
        .for_each(|(range, (start, end), first_entries)| {
            assert_eq!(
                (range.has_start(), range.has_end()),
                (start, end),
                "{range:?}"
            );
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

    #[test]
    fn a_range_without_a_start_never_starts_at_0() {
        [
            (range(None, None), 0, 3),
            (range(None, Some(9)), 0, 3),
            (range(None, None), 0, 0),
        ]
        .into_iter()
        .for_each(|(range, oplog_first, count)| {
            assert_eq!(
                range.first_index_in(index(oplog_first)),
                index(1),
                "{range:?}"
            );
            let page = range
                .page_in(index(oplog_first), count)
                .expect("the range has a page");
            assert_eq!(
                page.first_index_in(index(oplog_first)),
                index(1),
                "{page:?}"
            );
            let (start, end) = page.to_indices();
            assert_eq!(
                OplogRange::from_indices(start, end).unwrap(),
                page,
                "{page:?} keeps its value through its indices"
            );
        });
    }

    #[test]
    fn a_page_of_a_range_ends_at_its_count_or_at_the_end_of_the_range() {
        [
            ((range(None, None), 1, 50), Some(range(Some(1), Some(50)))),
            ((range(None, None), 4, 50), Some(range(Some(4), Some(53)))),
            ((range(None, Some(9)), 4, 50), Some(range(Some(4), Some(9)))),
            ((range(Some(5), None), 1, 3), Some(range(Some(5), Some(7)))),
            ((range(Some(5), None), 1, 1), Some(range(Some(5), Some(5)))),
            ((range(Some(5), None), 1, 0), Some(range(Some(5), Some(5)))),
            (
                (range(Some(5), Some(6)), 1, 3),
                Some(range(Some(5), Some(6))),
            ),
            (
                (range(Some(5), Some(7)), 1, 3),
                Some(range(Some(5), Some(7))),
            ),
            (
                (range(Some(5), Some(9)), 1, 3),
                Some(range(Some(5), Some(7))),
            ),
            (
                (range(Some(u64::MAX), None), 1, 3),
                Some(range(Some(u64::MAX), None)),
            ),
            (
                (range(Some(u64::MAX - 1), Some(u64::MAX)), 1, 3),
                Some(range(Some(u64::MAX - 1), Some(u64::MAX))),
            ),
            (
                (range(Some(2), None), 1, u64::MAX),
                Some(range(Some(2), None)),
            ),
            (
                (range(Some(1), None), 1, u64::MAX),
                Some(range(Some(1), None)),
            ),
            (
                (range(Some(1), Some(u64::MAX)), 1, u64::MAX),
                Some(range(Some(1), Some(u64::MAX))),
            ),
            (
                (range(None, None), u64::MAX - 1, 2),
                Some(range(Some(u64::MAX - 1), None)),
            ),
            (
                (range(None, None), u64::MAX - 2, 2),
                Some(range(Some(u64::MAX - 2), Some(u64::MAX - 1))),
            ),
            // The oplog starts at the end of the range, so the page holds that one index.
            ((range(None, Some(9)), 9, 50), Some(range(Some(9), Some(9)))),
            (
                (range(Some(5), Some(5)), 1, 3),
                Some(range(Some(5), Some(5))),
            ),
            // The oplog starts after the end of the range, so the range has no page in it.
            ((range(None, Some(9)), 10, 50), None),
        ]
        .into_iter()
        .for_each(|((range, oplog_first, count), expected)| {
            assert_eq!(
                range.page_in(index(oplog_first), count),
                expected,
                "{range:?} in an oplog that starts at {oplog_first}, page of {count}"
            )
        });
    }
}
