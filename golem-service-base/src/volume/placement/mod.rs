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

use super::VolumeError;
use std::fmt::{Debug, Formatter};

#[derive(Clone, PartialEq, Eq)]
pub enum BucketVersioning {
    Unversioned,
    Enabled,
    Suspended,
    Unknown(Box<str>),
}

// An endpoint can reflect credentials in unknown statuses or lifecycle prefixes.
// Keep that text out of Debug while preserving the observed values.
impl Debug for BucketVersioning {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unversioned => "Unversioned",
            Self::Enabled => "Enabled",
            Self::Suspended => "Suspended",
            Self::Unknown(_) => "Unknown",
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum LifecycleRuleStatus {
    Enabled,
    Disabled,
    Unknown(Box<str>),
}

impl Debug for LifecycleRuleStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
            Self::Unknown(_) => "Unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoncurrentExpiration {
    pub days: i32,
    pub newer_versions: Option<i32>,
}

/// Rules that can expire retained states. Filters do not rule out possible applicability.
#[derive(Clone, PartialEq, Eq)]
pub struct LifecycleRule {
    pub status: LifecycleRuleStatus,
    pub prefix: Option<Box<str>>,
    pub noncurrent_expiration: Option<NoncurrentExpiration>,
}

impl Debug for LifecycleRule {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleRule")
            .field("status", &self.status)
            .field("noncurrent_expiration", &self.noncurrent_expiration)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRead {
    NoObject,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementObservations {
    pub versioning: Result<BucketVersioning, VolumeError>,
    pub version_read: Result<VersionRead, VolumeError>,
    pub lifecycle: Result<Box<[LifecycleRule]>, VolumeError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub version_read: VersionRead,
    /// An unavailable lifecycle observation does not fail a required placement check.
    pub lifecycle: Result<Box<[LifecycleRule]>, VolumeError>,
}

/// Applies the read-only Rule 13 requirements to the observations.
pub fn verdict(observed: PlacementObservations) -> Result<Placement, VolumeError> {
    if observed.versioning? != BucketVersioning::Enabled {
        return Err(VolumeError::Requirement {
            requirement: "bucket versioning enabled".into(),
        });
    }
    Ok(Placement {
        version_read: observed.version_read?,
        lifecycle: observed.lifecycle,
    })
}

#[cfg(test)]
mod tests;
