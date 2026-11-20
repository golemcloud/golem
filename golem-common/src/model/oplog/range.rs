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
/// always an index of an entry. A range without an end continues to the end of the oplog. An end
/// after the end of the oplog does not cause an error: the read stops at the end of the oplog.
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

    pub fn start(&self) -> OplogIndex {
        self.start
    }

    pub fn end(&self) -> Option<OplogIndex> {
        self.end
    }
}

#[cfg(test)]
mod tests {
    use super::{OplogRange, OplogRangeEndsBeforeStart};
    use crate::base_model::OplogIndex;
    use test_r::test;

    #[test]
    fn a_range_starts_at_the_first_entry_and_refuses_an_end_before_its_start() {
        let index = OplogIndex::from_u64;
        let range = |from: Option<u64>, to: Option<u64>| {
            OplogRange::new(from.map(index), to.map(index))
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
            assert_eq!(range(from, to), expected, "from {from:?} to {to:?}")
        });
        assert_eq!(OplogRange::new(None, None), Ok(OplogRange::ALL));
    }
}
