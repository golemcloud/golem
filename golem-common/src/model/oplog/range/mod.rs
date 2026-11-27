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
    /// does not point to an index from 1 to before its search start. Or the range ends before the
    /// search start. The cursor came from a search of another range.
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
    /// the search in the range, with the rule of [`Self::resume`]. A cursor with `scan_to`
    /// continues the scan for the component revision. A range with a start must start at
    /// `scan_to`, and a range without a start takes `scan_to` as its start. The cursor must point
    /// to an index from 1 to before `scan_to`, and the range must not end before `scan_to`.
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

    /// The count of the entries that a page of at most `limit` entries reads from `from`. It is
    /// `limit`, or the count of the indices from `from` to the end of the range when that is
    /// smaller. It is 0 when `from` is after the end. A count that does not fit in `usize` gives
    /// `usize::MAX`.
    pub fn page_count(&self, from: OplogIndex, limit: u64) -> usize {
        let in_range = self.end.map_or(u64::MAX, |end| {
            end.as_u64()
                .checked_sub(from.as_u64())
                .map_or(0, |distance| distance.saturating_add(1))
        });
        usize::try_from(limit.min(in_range)).unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests;
