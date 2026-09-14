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

use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::serialization::{deserialize, serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OplogReadSource {
    Primary,
    Archive(usize),
    EphemeralBuffer,
    Other(&'static str),
}

impl Display for OplogReadSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => write!(f, "primary"),
            Self::Archive(level) => write!(f, "archive layer {level}"),
            Self::EphemeralBuffer => write!(f, "ephemeral buffer"),
            Self::Other(name) => f.write_str(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OplogReadError {
    InvalidRange {
        start: OplogIndex,
        count: u64,
    },
    Gap {
        start: OplogIndex,
        end: OplogIndex,
    },
    Corruption {
        source: OplogReadSource,
        detail: String,
    },
    SourceFailure {
        source: OplogReadSource,
        detail: String,
    },
}

impl OplogReadError {
    pub fn corruption(source: OplogReadSource, detail: impl Into<String>) -> Self {
        Self::Corruption {
            source,
            detail: detail.into(),
        }
    }

    pub fn source_failure(source: OplogReadSource, detail: impl Into<String>) -> Self {
        Self::SourceFailure {
            source,
            detail: detail.into(),
        }
    }
}

impl Display for OplogReadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRange { start, count } => {
                write!(
                    f,
                    "invalid oplog range starting at {start} with {count} entries"
                )
            }
            Self::Gap { start, end } => {
                write!(f, "missing oplog entries in range [{start}..={end}]")
            }
            Self::Corruption { source, detail } => {
                write!(f, "corrupt oplog data from {source}: {detail}")
            }
            Self::SourceFailure { source, detail } => {
                write!(f, "failed to read oplog data from {source}: {detail}")
            }
        }
    }
}

impl Error for OplogReadError {}

pub(crate) fn fail_stop<T>(result: Result<T, OplogReadError>) -> T {
    result.unwrap_or_else(|error| panic!("Oplog read failed: {error}"))
}

/// Merges contiguous suffixes returned by oplog storage tiers into one exact logical range.
///
/// Callers own source ordering and snapshot acquisition. Each successive source is asked for
/// [`Self::next_range`], so the state machine adds no storage probes or asynchronous dispatch.
pub struct OplogRead<T> {
    start: OplogIndex,
    end: Option<OplogIndex>,
    needed_end: Option<OplogIndex>,
    entries: BTreeMap<OplogIndex, T>,
}

pub(crate) fn checked_range_end(
    start: OplogIndex,
    count: u64,
) -> Result<Option<OplogIndex>, OplogReadError> {
    if count == 0 {
        Ok(None)
    } else {
        start
            .as_u64()
            .checked_add(count - 1)
            .map(OplogIndex::from_u64)
            .map(Some)
            .ok_or(OplogReadError::InvalidRange { start, count })
    }
}

pub fn exact_from_source<T: PartialEq>(
    source: OplogReadSource,
    start: OplogIndex,
    count: u64,
    entries: BTreeMap<OplogIndex, T>,
) -> Result<BTreeMap<OplogIndex, T>, OplogReadError> {
    let mut read = OplogRead::new(start, count)?;
    read.add_source(source, entries)?;
    read.finish()
}

pub fn verify_persisted_entries(
    source: OplogReadSource,
    expected: &[(OplogIndex, OplogEntry)],
    actual: BTreeMap<OplogIndex, OplogEntry>,
) -> Result<(), OplogReadError> {
    let Some((start, _)) = expected.first() else {
        return if actual.is_empty() {
            Ok(())
        } else {
            Err(OplogReadError::corruption(
                source,
                "persisted verification returned entries for an empty transfer",
            ))
        };
    };

    let actual = exact_from_source(source, *start, expected.len() as u64, actual)?;
    let mut requires_normalization = false;
    for ((expected_index, expected_entry), (actual_index, actual_entry)) in
        expected.iter().zip(actual.iter())
    {
        if expected_index != actual_index {
            return Err(OplogReadError::corruption(
                source,
                format!("persisted transfer differs at oplog index {expected_index}"),
            ));
        }
        requires_normalization |= expected_entry != actual_entry;
    }
    if !requires_normalization {
        return Ok(());
    }

    // Archive encoding is the source of truth for a persisted transfer. In-memory source caches
    // can retain values that the binary codec normalizes, such as sub-millisecond timestamps. Keep
    // the common already-normalized path allocation-free and only encode again after a mismatch.
    let expected_entries: Vec<_> = expected.iter().map(|(_, entry)| entry.clone()).collect();
    let encoded = serialize(&expected_entries).map_err(|error| {
        OplogReadError::corruption(
            source,
            format!("failed to encode transferred oplog entries: {error}"),
        )
    })?;
    let expected_entries: Vec<OplogEntry> = deserialize(&encoded).map_err(|error| {
        OplogReadError::corruption(
            source,
            format!("failed to decode transferred oplog entries: {error}"),
        )
    })?;

    for (((expected_index, _), expected_entry), (actual_index, actual_entry)) in expected
        .iter()
        .zip(expected_entries.iter())
        .zip(actual.iter())
    {
        if expected_index != actual_index || expected_entry != actual_entry {
            return Err(OplogReadError::corruption(
                source,
                format!("persisted transfer differs at oplog index {expected_index}"),
            ));
        }
    }
    Ok(())
}

impl<T: PartialEq> OplogRead<T> {
    pub fn new(start: OplogIndex, count: u64) -> Result<Self, OplogReadError> {
        let end = checked_range_end(start, count)?;

        Ok(Self {
            start,
            end,
            needed_end: end,
            entries: BTreeMap::new(),
        })
    }

    pub fn next_range(&self) -> Option<(OplogIndex, u64)> {
        self.needed_end.map(|end| {
            (
                self.start,
                end.as_u64().saturating_sub(self.start.as_u64()) + 1,
            )
        })
    }

    pub fn add_source(
        &mut self,
        source: OplogReadSource,
        entries: BTreeMap<OplogIndex, T>,
    ) -> Result<(), OplogReadError> {
        let Some(needed_end) = self.needed_end else {
            if entries.is_empty() {
                return Ok(());
            }
            return Err(OplogReadError::corruption(
                source,
                "returned entries after the requested range was complete",
            ));
        };
        let Some(end) = self.end else {
            return Err(OplogReadError::corruption(
                source,
                "returned entries for an empty request",
            ));
        };
        if entries.is_empty() {
            return Ok(());
        }

        let mut entries = entries;
        let first = *entries.first_key_value().unwrap().0;
        let last = *entries.last_key_value().unwrap().0;
        if first < self.start || last > end {
            return Err(OplogReadError::corruption(
                source,
                format!(
                    "returned range [{first}..={last}] outside requested range [{}..={end}]",
                    self.start
                ),
            ));
        }
        let span = last.as_u64() - first.as_u64() + 1;
        if span != entries.len() as u64 {
            return Err(OplogReadError::corruption(
                source,
                format!(
                    "returned {} entries for non-contiguous range [{first}..={last}]",
                    entries.len()
                ),
            ));
        }

        if last < needed_end {
            return Err(OplogReadError::Gap {
                start: last.next(),
                end: needed_end,
            });
        }

        if last > needed_end {
            let overlap = entries.split_off(&needed_end.next());
            for (index, entry) in overlap {
                match self.entries.get(&index) {
                    Some(existing) if existing == &entry => {}
                    Some(_) => {
                        return Err(OplogReadError::corruption(
                            source,
                            format!("conflicting copies at oplog index {index}"),
                        ));
                    }
                    None => {
                        return Err(OplogReadError::corruption(
                            source,
                            format!("returned unexpected already-covered index {index}"),
                        ));
                    }
                }
            }
        }

        if let Some((newly_covered_start, _)) = entries.first_key_value() {
            let newly_covered_start = *newly_covered_start;
            let newly_covered_end = *entries.last_key_value().unwrap().0;
            if newly_covered_end != needed_end {
                return Err(OplogReadError::corruption(
                    source,
                    format!(
                        "returned a range ending at {newly_covered_end}, expected a suffix ending at {needed_end}"
                    ),
                ));
            }
            self.entries.append(&mut entries);
            self.needed_end = if newly_covered_start == self.start {
                None
            } else {
                Some(newly_covered_start.previous())
            };
        }

        Ok(())
    }

    pub fn finish(self) -> Result<BTreeMap<OplogIndex, T>, OplogReadError> {
        if let Some(needed_end) = self.needed_end {
            Err(OplogReadError::Gap {
                start: self.start,
                end: needed_end,
            })
        } else {
            Ok(self.entries)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn entries(start: u64, end: u64) -> BTreeMap<OplogIndex, u64> {
        (start..=end)
            .map(|index| (OplogIndex::from_u64(index), index))
            .collect()
    }

    #[test]
    fn empty_read_succeeds() {
        let read = OplogRead::<u64>::new(OplogIndex::INITIAL, 0).unwrap();
        assert_eq!(read.next_range(), None);
        assert!(read.finish().unwrap().is_empty());
    }

    #[test]
    fn merges_tier_suffixes() {
        let mut read = OplogRead::new(OplogIndex::INITIAL, 10).unwrap();
        assert_eq!(read.next_range(), Some((OplogIndex::INITIAL, 10)));
        read.add_source(OplogReadSource::Primary, entries(8, 10))
            .unwrap();
        assert_eq!(read.next_range(), Some((OplogIndex::INITIAL, 7)));
        read.add_source(OplogReadSource::Archive(0), entries(4, 7))
            .unwrap();
        read.add_source(OplogReadSource::Archive(1), entries(1, 3))
            .unwrap();
        assert_eq!(read.finish().unwrap(), entries(1, 10));
    }

    #[test]
    fn accepts_identical_overlap_returned_by_a_source() {
        let mut read = OplogRead::new(OplogIndex::INITIAL, 10).unwrap();
        read.add_source(OplogReadSource::Primary, entries(8, 10))
            .unwrap();
        read.add_source(OplogReadSource::Archive(0), entries(1, 10))
            .unwrap();
        assert_eq!(read.finish().unwrap(), entries(1, 10));
    }

    #[test]
    fn rejects_conflicting_overlap() {
        let mut read = OplogRead::new(OplogIndex::INITIAL, 10).unwrap();
        read.add_source(OplogReadSource::Primary, entries(8, 10))
            .unwrap();
        let mut lower = entries(1, 10);
        lower.insert(OplogIndex::from_u64(9), 999);
        assert!(matches!(
            read.add_source(OplogReadSource::Archive(0), lower),
            Err(OplogReadError::Corruption { .. })
        ));
    }

    #[test]
    fn reports_gap_when_source_ends_before_requested_suffix() {
        let mut read = OplogRead::new(OplogIndex::INITIAL, 10).unwrap();
        assert_eq!(
            read.add_source(OplogReadSource::Primary, entries(5, 8)),
            Err(OplogReadError::Gap {
                start: OplogIndex::from_u64(9),
                end: OplogIndex::from_u64(10),
            })
        );
    }

    #[test]
    fn rejects_non_contiguous_source_response() {
        let mut read = OplogRead::new(OplogIndex::INITIAL, 10).unwrap();
        let mut source_entries = entries(5, 10);
        source_entries.remove(&OplogIndex::from_u64(8));
        assert!(matches!(
            read.add_source(OplogReadSource::Primary, source_entries),
            Err(OplogReadError::Corruption { .. })
        ));
    }

    #[test]
    fn reports_remaining_gap() {
        let mut read = OplogRead::new(OplogIndex::INITIAL, 10).unwrap();
        read.add_source(OplogReadSource::Primary, entries(8, 10))
            .unwrap();
        assert_eq!(
            read.finish(),
            Err(OplogReadError::Gap {
                start: OplogIndex::INITIAL,
                end: OplogIndex::from_u64(7),
            })
        );
    }
}
