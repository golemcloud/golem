// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::btree_map::{IntoValues, Values};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::ops::Bound::{Included, Unbounded};
use std::ops::RangeInclusive;

use crate::model::oplog::OplogIndex;
use bincode::{Decode, Encode};
use range_set_blaze::RangeSetBlaze;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct OplogRegion {
    pub start: OplogIndex,
    pub end: OplogIndex,
}

impl OplogRegion {
    pub fn contains(&self, target: OplogIndex) -> bool {
        target > self.end && target <= self.start
    }

    pub fn union(&self, other: &OplogRegion) -> Option<OplogRegion> {
        if self.contains(other.start) || other.contains(self.start) {
            Some(OplogRegion {
                start: self.start.min(other.start),
                end: self.end.max(other.end),
            })
        } else {
            None
        }
    }

    pub fn from_index_range(range: RangeInclusive<OplogIndex>) -> OplogRegion {
        OplogRegion {
            start: *range.start(),
            end: *range.end(),
        }
    }

    pub fn from_range(range: RangeInclusive<u64>) -> OplogRegion {
        OplogRegion {
            start: OplogIndex::from_u64(*range.start()),
            end: OplogIndex::from_u64(*range.end()),
        }
    }

    pub fn to_range(&self) -> RangeInclusive<u64> {
        self.start.into()..=self.end.into()
    }
}

impl Display for OplogRegion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} => {}>", self.start, self.end)
    }
}

/// Temporary builder for building up a `DeletedRegions` structure using an efficient data
/// structure for merging ranges as they are added.
pub struct DeletedRegionsBuilder {
    regions: RangeSetBlaze<u64>,
}

impl Default for DeletedRegionsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DeletedRegionsBuilder {
    pub fn new() -> Self {
        Self {
            regions: RangeSetBlaze::new(),
        }
    }

    pub fn from_regions(regions: impl IntoIterator<Item = OplogRegion>) -> Self {
        Self {
            regions: RangeSetBlaze::from_iter(regions.into_iter().map(|region| region.to_range())),
        }
    }

    /// Adds a new region to the list of deleted regions
    pub fn add(&mut self, region: OplogRegion) {
        self.regions.ranges_insert(region.to_range());
    }

    pub fn build(self) -> DeletedRegions {
        DeletedRegions::from_regions(self.regions.into_ranges().map(OplogRegion::from_range))
    }
}

/// Structure holding all the regions deleted from the oplog by jumps. Deleted regions
/// can be stacked to introduce temporary overrides.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct DeletedRegions {
    regions: Vec<BTreeMap<OplogIndex, OplogRegion>>,
}

impl Default for DeletedRegions {
    fn default() -> Self {
        Self::new()
    }
}

impl DeletedRegions {
    /// Constructs an empty set of deleted regions
    pub fn new() -> Self {
        Self {
            regions: vec![BTreeMap::new()],
        }
    }

    /// Initializes from known list of deleted oplog regions
    pub fn from_regions(regions: impl IntoIterator<Item = OplogRegion>) -> Self {
        Self {
            regions: vec![BTreeMap::from_iter(
                regions.into_iter().map(|region| (region.start, region)),
            )],
        }
    }

    /// Adds a new region to the list of deleted regions
    pub fn add(&mut self, region: OplogRegion) {
        // We rebuild the map to make sure overlapping regions are properly merged
        let current = self.regions.pop().unwrap();
        let mut builder = DeletedRegionsBuilder::from_regions(current.into_values());
        builder.add(region);
        let mut temp = builder.build().regions;
        self.regions.push(temp.pop().unwrap());
    }

    /// Sets an override of the deleted regions.This is not stacked, if there was an override already
    /// it is going to be replaced. The override can be dropped using `drop_override`.
    pub fn set_override(&mut self, other: DeletedRegions) {
        if self.is_overridden() {
            self.drop_override();
        }

        if !other.is_empty() {
            let current = self.regions.last().unwrap();
            let mut builder = DeletedRegionsBuilder::from_regions(current.clone().into_values());
            for region in current.values() {
                builder.add(region.clone());
            }
            for region in other.into_regions() {
                builder.add(region);
            }

            self.regions.push(builder.build().regions.pop().unwrap());
        }
    }

    pub fn drop_override(&mut self) {
        self.regions.pop();
    }

    pub fn is_empty(&self) -> bool {
        self.regions.last().unwrap().is_empty()
    }

    pub fn is_overridden(&self) -> bool {
        self.regions.len() > 1
    }

    /// Returns the list of deleted regions
    pub fn regions(&self) -> Values<'_, OplogIndex, OplogRegion> {
        self.regions.last().unwrap().values()
    }

    /// Becomes the list of deleted regions
    pub fn into_regions(mut self) -> IntoValues<OplogIndex, OplogRegion> {
        self.regions.pop().unwrap().into_values()
    }

    /// Gets the next deleted region after, possibly including, the given oplog index.
    /// Returns None if there are no more deleted regions.
    pub fn find_next_deleted_region(&self, from: OplogIndex) -> Option<OplogRegion> {
        self.regions
            .last()
            .unwrap()
            .range((Included(from), Unbounded))
            .next()
            .map(|(_, region)| region.clone())
    }

    pub fn is_in_deleted_region(&self, oplog_index: OplogIndex) -> bool {
        self.regions
            .last()
            .unwrap()
            .values()
            .any(|region| region.contains(oplog_index))
    }
}

impl Display for DeletedRegions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]",
            self.regions
                .last()
                .unwrap()
                .values()
                .map(|region| region.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::model::oplog::OplogIndex;
    use crate::model::regions::{DeletedRegionsBuilder, OplogRegion};

    fn oplog_region(start: u64, end: u64) -> OplogRegion {
        OplogRegion {
            start: OplogIndex::from_u64(start),
            end: OplogIndex::from_u64(end),
        }
    }

    #[test]
    pub fn builder_from_overlapping_ranges() {
        let mut builder = DeletedRegionsBuilder::new();
        builder.add(oplog_region(2, 8));
        builder.add(oplog_region(2, 14));
        builder.add(oplog_region(20, 22));
        let deleted_regions = builder.build();

        assert_eq!(
            deleted_regions.into_regions().collect::<Vec<_>>(),
            vec![oplog_region(2, 14), oplog_region(20, 22),]
        );
    }

    #[test]
    pub fn builder_from_initial_state() {
        let mut builder =
            DeletedRegionsBuilder::from_regions(vec![oplog_region(2, 8), oplog_region(20, 22)]);

        builder.add(oplog_region(20, 24));
        builder.add(oplog_region(30, 40));

        let deleted_regions = builder.build();

        assert_eq!(
            deleted_regions.into_regions().collect::<Vec<_>>(),
            vec![
                oplog_region(2, 8),
                oplog_region(20, 24),
                oplog_region(30, 40),
            ]
        );
    }

    #[test]
    pub fn find_next_deleted_region() {
        let deleted_regions =
            DeletedRegionsBuilder::from_regions(vec![oplog_region(2, 8), oplog_region(20, 22)])
                .build();

        assert_eq!(
            deleted_regions.find_next_deleted_region(OplogIndex::from_u64(0)),
            Some(oplog_region(2, 8))
        );
        assert_eq!(
            deleted_regions.find_next_deleted_region(OplogIndex::from_u64(2)),
            Some(oplog_region(2, 8))
        );
        assert_eq!(
            deleted_regions.find_next_deleted_region(OplogIndex::from_u64(8)),
            Some(oplog_region(20, 22))
        );
        assert_eq!(
            deleted_regions.find_next_deleted_region(OplogIndex::from_u64(22)),
            None
        );
    }
}
