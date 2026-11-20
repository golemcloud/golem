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

use crate::base_model::OplogIndex;
use std::fmt::{Display, Formatter};

/// The oplog indices that a read selects, from `start` to `end`, including both. The start is
/// never 0. A range without an end continues to the end of the oplog. An end after the end of
/// the oplog does not cause an error: the read stops at the end of the oplog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogRange {
    start: OplogIndex,
    end: Option<OplogIndex>,
}

/// The error of a range whose end is before its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogRangeEndsBeforeStart {
    pub start: OplogIndex,
    pub end: OplogIndex,
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

impl std::error::Error for OplogRangeEndsBeforeStart {}

/// The error of a cursor whose next index is not in the range of the read. The cursor came from
/// a read of another range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogCursorOutsideRange {
    pub next: OplogIndex,
    pub range: OplogRange,
}

impl Display for OplogCursorOutsideRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "the oplog cursor points to index {}, ", self.next)?;
        match self.range.end {
            Some(end) => write!(
                f,
                "outside the oplog range from {} to {end}",
                self.range.start
            ),
            None => write!(f, "before the oplog range from {}", self.range.start),
        }
    }
}

impl std::error::Error for OplogCursorOutsideRange {}

impl OplogRange {
    /// Every entry of the oplog.
    pub const ALL: OplogRange = OplogRange {
        start: OplogIndex::INITIAL,
        end: None,
    };

    /// The range from `from` to `to`. Without `from`, or with `from` at index 0, the range starts
    /// at the first entry. Without `to`, the range continues to the end of the oplog. A `to`
    /// before the start gives an error.
    pub fn new(
        from: Option<OplogIndex>,
        to: Option<OplogIndex>,
    ) -> Result<Self, OplogRangeEndsBeforeStart> {
        let start = from.unwrap_or(OplogIndex::INITIAL).max(OplogIndex::INITIAL);
        match to {
            Some(end) if end < start => Err(OplogRangeEndsBeforeStart { start, end }),
            end => Ok(Self { start, end }),
        }
    }

    /// The range of the `from` and `to` indices of a request, with the rules of [`Self::new`].
    pub fn from_indices(
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Self, OplogRangeEndsBeforeStart> {
        Self::new(from.map(OplogIndex::from_u64), to.map(OplogIndex::from_u64))
    }

    pub fn start(&self) -> OplogIndex {
        self.start
    }

    pub fn end(&self) -> Option<OplogIndex> {
        self.end
    }

    /// The rest of the range from the index `next` of a cursor. A cursor before the start or
    /// after the end of the range gives an error, because a page of this range never gives it.
    pub fn resume(&self, next: OplogIndex) -> Result<Self, OplogCursorOutsideRange> {
        let outside = next < self.start || self.end.is_some_and(|end| next > end);
        if outside {
            Err(OplogCursorOutsideRange { next, range: *self })
        } else {
            Ok(Self {
                start: next,
                end: self.end,
            })
        }
    }

    /// The count of the indices of the range from `index` to its end, including both. It is 0
    /// when `index` is after the end, and `u64::MAX` when the range has no end. The count
    /// saturates, so an end at `u64::MAX` does not overflow.
    pub fn entries_from(&self, index: OplogIndex) -> u64 {
        self.end.map_or(u64::MAX, |end| {
            end.as_u64()
                .checked_sub(index.as_u64())
                .map_or(0, |distance| distance.saturating_add(1))
        })
    }

    /// The last index of the range in an oplog whose last index is `oplog_last`: the end of the
    /// range, or `oplog_last` when the range has no end or ends after it.
    pub fn last_index_in(&self, oplog_last: OplogIndex) -> OplogIndex {
        self.end.map_or(oplog_last, |end| end.min(oplog_last))
    }

    /// The part of the range that a page of at most `count` entries reads: from the start to the
    /// start plus `count - 1`, or to the end of the range when that is earlier. A page reads at
    /// least one entry, so a `count` of 0 gives the first index only. When the start plus
    /// `count - 1` overflows, the page ends where the range ends.
    pub fn page(&self, count: u64) -> Self {
        let page_end = self
            .start
            .as_u64()
            .checked_add(count.saturating_sub(1))
            .map(OplogIndex::from_u64);
        let end = match (page_end, self.end) {
            (Some(page_end), Some(end)) => Some(page_end.min(end)),
            (page_end, end) => page_end.or(end),
        };
        Self {
            start: self.start,
            end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OplogCursorOutsideRange, OplogRange, OplogRangeEndsBeforeStart};
    use crate::base_model::OplogIndex;
    use test_r::test;

    fn index(value: u64) -> OplogIndex {
        OplogIndex::from_u64(value)
    }

    fn range(from: Option<u64>, to: Option<u64>) -> OplogRange {
        OplogRange::from_indices(from, to).unwrap()
    }

    #[test]
    fn a_range_starts_at_the_first_entry_and_refuses_an_end_before_its_start() {
        let parts = |from: Option<u64>, to: Option<u64>| {
            OplogRange::from_indices(from, to)
                .map(|range| (range.start().as_u64(), range.end().map(u64::from)))
        };
        let refused = |start, end| {
            Err(OplogRangeEndsBeforeStart {
                start: index(start),
                end: index(end),
            })
        };
        [
            ((None, None), Ok((1, None))),
            ((Some(0), None), Ok((1, None))),
            ((Some(7), None), Ok((7, None))),
            ((None, Some(1)), Ok((1, Some(1)))),
            ((Some(5), Some(5)), Ok((5, Some(5)))),
            ((Some(5), Some(u64::MAX)), Ok((5, Some(u64::MAX)))),
            ((Some(5), Some(4)), refused(5, 4)),
            ((None, Some(0)), refused(1, 0)),
            ((Some(0), Some(0)), refused(1, 0)),
        ]
        .into_iter()
        .for_each(|((from, to), expected)| {
            assert_eq!(parts(from, to), expected, "from {from:?} to {to:?}")
        });
        assert_eq!(OplogRange::new(None, None), Ok(OplogRange::ALL));
        assert_eq!(
            OplogRange::new(Some(index(3)), Some(index(9))),
            OplogRange::from_indices(Some(3), Some(9))
        );
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
            ((OplogRange::ALL, 1), Ok(OplogRange::ALL)),
            ((OplogRange::ALL, 0), outside(OplogRange::ALL, 0)),
        ]
        .into_iter()
        .for_each(|((range, next), expected)| {
            assert_eq!(range.resume(index(next)), expected, "{range:?} at {next}")
        });
    }

    #[test]
    fn a_range_counts_its_entries_from_an_index_without_overflow() {
        [
            ((range(None, None), 1), u64::MAX),
            ((range(None, Some(1)), 1), 1),
            ((range(None, Some(7)), 3), 5),
            ((range(None, Some(7)), 7), 1),
            ((range(None, Some(7)), 8), 0),
            ((range(None, Some(7)), 9), 0),
            ((range(None, Some(u64::MAX)), 1), u64::MAX),
            ((range(None, Some(u64::MAX)), 2), u64::MAX - 1),
            ((range(None, Some(u64::MAX)), u64::MAX), 1),
        ]
        .into_iter()
        .for_each(|((range, from), expected)| {
            assert_eq!(
                range.entries_from(index(from)),
                expected,
                "{range:?} from {from}"
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
    fn a_page_of_a_range_ends_at_its_count_or_at_the_end_of_the_range() {
        [
            ((range(None, None), 50), range(Some(1), Some(50))),
            ((range(Some(5), None), 3), range(Some(5), Some(7))),
            ((range(Some(5), None), 1), range(Some(5), Some(5))),
            ((range(Some(5), None), 0), range(Some(5), Some(5))),
            ((range(Some(5), Some(6)), 3), range(Some(5), Some(6))),
            ((range(Some(5), Some(7)), 3), range(Some(5), Some(7))),
            ((range(Some(5), Some(9)), 3), range(Some(5), Some(7))),
            (
                (range(Some(u64::MAX), None), 3),
                range(Some(u64::MAX), None),
            ),
            (
                (range(Some(u64::MAX - 1), Some(u64::MAX)), 3),
                range(Some(u64::MAX - 1), Some(u64::MAX)),
            ),
            ((range(Some(2), None), u64::MAX), range(Some(2), None)),
        ]
        .into_iter()
        .for_each(|((range, count), expected)| {
            assert_eq!(range.page(count), expected, "{range:?}, page of {count}")
        });
    }
}
