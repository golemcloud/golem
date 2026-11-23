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

use super::object_identity::SandboxDirectoryId;
use std::ffi::OsString;
use std::hash::{Hash, Hasher};

#[cfg(any(test, windows))]
mod insensitive_mode;

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(all(test, not(windows)))]
#[path = "scripted.rs"]
mod platform;
#[cfg(all(not(test), not(windows)))]
#[path = "other.rs"]
mod platform;

pub(in crate::filesystem::sandbox) use platform::NativeNameComparisonMode;

#[derive(Clone)]
pub(in crate::filesystem::sandbox) struct NativeNameCoordinationKey {
    pub(in crate::filesystem::sandbox) name: OsString,
    pub(in crate::filesystem::sandbox) mode: NativeNameComparisonMode,
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct SandboxNameComparisonKey {
    pub(in crate::filesystem::sandbox) parent: SandboxDirectoryId,
    pub(in crate::filesystem::sandbox) name: NativeNameCoordinationKey,
}

impl PartialEq for NativeNameCoordinationKey {
    fn eq(&self, other: &Self) -> bool {
        if self.mode != other.mode {
            false
        } else if self.mode == NativeNameComparisonMode::Exact {
            self.name == other.name
        } else if self.mode == NativeNameComparisonMode::Conservative {
            true
        } else {
            platform::names_equal(&self.name, &other.name)
        }
    }
}

impl Eq for NativeNameCoordinationKey {}

impl Hash for NativeNameCoordinationKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mode.hash(state);
        if self.mode == NativeNameComparisonMode::Exact {
            self.name.hash(state);
        }
    }
}

impl NativeNameCoordinationKey {
    fn may_be_equivalent(&self, other: &Self) -> bool {
        if self.mode == NativeNameComparisonMode::Exact
            && other.mode == NativeNameComparisonMode::Exact
        {
            self.name == other.name
        } else if self.mode == NativeNameComparisonMode::Conservative
            || other.mode == NativeNameComparisonMode::Conservative
            || self.mode != other.mode
        {
            true
        } else {
            platform::names_equal(&self.name, &other.name)
        }
    }
}

impl SandboxNameComparisonKey {
    /// Reports whether two resolved names may denote the same entry in one parent directory.
    ///
    /// Conservative name modes return conflicts rather than risk concurrent aliasing edits.
    pub(crate) fn may_conflict_with(&self, other: &Self) -> bool {
        self.parent == other.parent && self.name.may_be_equivalent(&other.name)
    }
}

impl PartialEq<SandboxDirectoryId> for SandboxNameComparisonKey {
    fn eq(&self, other: &SandboxDirectoryId) -> bool {
        self.parent == *other
    }
}

impl PartialEq<SandboxNameComparisonKey> for SandboxDirectoryId {
    fn eq(&self, other: &SandboxNameComparisonKey) -> bool {
        other == self
    }
}

#[cfg(test)]
mod tests;
