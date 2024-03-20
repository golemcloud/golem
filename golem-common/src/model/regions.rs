// Copyright 2024 Golem Cloud
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

use bincode::{Decode, Encode};
use range_set_blaze::RangeSetBlaze;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct OplogRegion {
    pub start: u64,
    pub end: u64,
}

impl OplogRegion {
    pub fn contains(&self, target: u64) -> bool {
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

    pub fn from_range(range: RangeInclusive<u64>) -> OplogRegion {
        OplogRegion {
            start: *range.start(),
            end: *range.end(),
        }
    }

    pub fn to_range(&self) -> RangeInclusive<u64> {
        self.start..=self.end
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

/// Structure holding all the regions deleted from the oplog by jumps
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct DeletedRegions {
    regions: BTreeMap<u64, OplogRegion>,
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
            regions: BTreeMap::new(),
        }
    }

    /// Initializes from known list of deleted oplog regions
    pub fn from_regions(regions: impl IntoIterator<Item = OplogRegion>) -> Self {
        Self {
            regions: BTreeMap::from_iter(regions.into_iter().map(|region| (region.start, region))),
        }
    }

    /// Adds a new region to the list of deleted regions
    pub fn add(&mut self, region: OplogRegion) {
        // We rebuild the map to make sure overlapping regions are properly merged
        let mut builder = DeletedRegionsBuilder::from_regions(self.regions.clone().into_values());
        builder.add(region);
        self.regions = builder.build().regions;
    }

    /// Returns the list of deleted regions
    pub fn regions(&self) -> Values<'_, u64, OplogRegion> {
        self.regions.values()
    }

    /// Becomes the list of deleted regions
    pub fn into_regions(self) -> IntoValues<u64, OplogRegion> {
        self.regions.into_values()
    }

    /// Gets the next deleted region after, possibly including, the given oplog index.
    /// Returns None if there are no more deleted regions.
    pub fn find_next_deleted_region(&self, from: u64) -> Option<OplogRegion> {
        self.regions
            .range((Included(from), Unbounded))
            .next()
            .map(|(_, region)| region.clone())
    }

    pub fn is_in_deleted_region(&self, oplog_index: u64) -> bool {
        self.regions
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
                .values()
                .map(|region| region.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::model::regions::{DeletedRegionsBuilder, OplogRegion};

    #[test]
    pub fn builder_from_overlapping_ranges() {
        let mut builder = DeletedRegionsBuilder::new();
        builder.add(OplogRegion { start: 2, end: 8 });
        builder.add(OplogRegion { start: 2, end: 14 });
        builder.add(OplogRegion { start: 20, end: 22 });
        let deleted_regions = builder.build();

        assert_eq!(
            deleted_regions.into_regions().collect::<Vec<_>>(),
            vec![
                OplogRegion { start: 2, end: 14 },
                OplogRegion { start: 20, end: 22 },
            ]
        );
    }

    #[test]
    pub fn builder_from_initial_state() {
        let mut builder = DeletedRegionsBuilder::from_regions(vec![
            OplogRegion { start: 2, end: 8 },
            OplogRegion { start: 20, end: 22 },
        ]);

        builder.add(OplogRegion { start: 20, end: 24 });
        builder.add(OplogRegion { start: 30, end: 40 });

        let deleted_regions = builder.build();

        assert_eq!(
            deleted_regions.into_regions().collect::<Vec<_>>(),
            vec![
                OplogRegion { start: 2, end: 8 },
                OplogRegion { start: 20, end: 24 },
                OplogRegion { start: 30, end: 40 },
            ]
        );
    }

    #[test]
    pub fn find_next_deleted_region() {
        let deleted_regions = DeletedRegionsBuilder::from_regions(vec![
            OplogRegion { start: 2, end: 8 },
            OplogRegion { start: 20, end: 22 },
        ])
        .build();

        assert_eq!(
            deleted_regions.find_next_deleted_region(0),
            Some(OplogRegion { start: 2, end: 8 })
        );
        assert_eq!(
            deleted_regions.find_next_deleted_region(2),
            Some(OplogRegion { start: 2, end: 8 })
        );
        assert_eq!(
            deleted_regions.find_next_deleted_region(8),
            Some(OplogRegion { start: 20, end: 22 })
        );
        assert_eq!(deleted_regions.find_next_deleted_region(22), None);
    }
}
