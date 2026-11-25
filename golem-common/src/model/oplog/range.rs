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

/// The error of a cursor whose next index is not in the range of the read. The cursor came from
/// a read of another range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogCursorOutsideRange {
    next: OplogIndex,
    range: OplogRange,
}

impl OplogCursorOutsideRange {
    /// The next index of the cursor.
    pub fn next(&self) -> OplogIndex {
        self.next
    }
}

impl Display for OplogCursorOutsideRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let start = self.range.first_possible();
        write!(f, "the oplog cursor points to index {}, ", self.next)?;
        match self.range.end {
            Some(end) => write!(f, "outside the oplog range from {start} to {end}"),
            None => write!(f, "before the oplog range from {start}"),
        }
    }
}

impl std::error::Error for OplogCursorOutsideRange {}

impl OplogRange {
    /// Every entry of the oplog.
    pub const ALL: OplogRange = OplogRange {
        start: None,
        end: None,
    };

    /// The range from `from` to `to`. Without `from`, or with `from` at index 0, the range has no
    /// start. Without `to`, the range continues to the end of the oplog. A `to` before the start
    /// gives an error; a range without a start starts at 1 for this check, because an oplog has
    /// no entry before index 1.
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

    /// The first index of the range in an oplog whose first entry is at `oplog_first`: the start
    /// of the range, or `oplog_first` when the range has no start. An `oplog_first` of 0 counts as
    /// 1, because an oplog has no entry before index 1.
    pub fn first_index_in(&self, oplog_first: OplogIndex) -> OplogIndex {
        self.start.unwrap_or(oplog_first.max(OplogIndex::INITIAL))
    }

    /// The first index that the range can hold: its start, or 1 when it has no start, because an
    /// oplog has no entry before index 1.
    fn first_possible(&self) -> OplogIndex {
        self.first_index_in(OplogIndex::INITIAL)
    }

    /// The last index of the range in an oplog whose last index is `oplog_last`: the end of the
    /// range, or `oplog_last` when the range has no end or ends after it.
    pub fn last_index_in(&self, oplog_last: OplogIndex) -> OplogIndex {
        self.end.map_or(oplog_last, |end| end.min(oplog_last))
    }

    /// Whether the range holds no index of an oplog whose last index is `oplog_last`: the range
    /// starts after it. A range without a start starts at 1 for this rule, so it is empty only
    /// when `oplog_last` is 0.
    pub fn is_empty_in(&self, oplog_last: OplogIndex) -> bool {
        oplog_last < self.first_possible()
    }

    /// The rest of the range from the next index of `cursor`. A cursor before the start or after
    /// the end of the range gives an error, because a page of this range never gives it. A range
    /// without a start starts at 1 for this check. The rest of the range has a start.
    pub fn resume(&self, cursor: &OplogCursor) -> Result<Self, OplogCursorOutsideRange> {
        let next = OplogIndex::from_u64(cursor.next_oplog_index);
        let outside = next < self.first_possible() || self.end.is_some_and(|end| next > end);
        if outside {
            Err(OplogCursorOutsideRange { next, range: *self })
        } else {
            Ok(Self {
                start: Some(next),
                end: self.end,
            })
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

    /// The part of the range that a page of at most `count` entries reads in an oplog whose first
    /// entry is at `oplog_first`: from the first index of the range to the last entry of the
    /// page, as [`Self::page_count`] counts it. A page reads at least one entry, so a `count` of 0
    /// gives the first index only. In a range without an end, a page that reaches index
    /// `u64::MAX`, or whose end overflows, has no end either, so that a page to the end of such a
    /// range has one value. When the first index is after the end of the range, the range has no
    /// page in the oplog, and the result is `None`.
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
    use super::{OplogCursorOutsideRange, OplogRange, OplogRangeEndsBeforeStart};
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
            Err(OplogCursorOutsideRange {
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
            assert_eq!(range.resume(&cursor(next)), expected, "{range:?} at {next}")
        });
        assert_eq!(
            range(Some(5), None)
                .resume(&cursor(2))
                .map_err(|error| error.next()),
            Err(index(2))
        );
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
