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
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::filesystem::sandbox) enum NativeNameComparisonMode {
    Exact,
    Conservative,
    #[cfg(any(test, windows))]
    WindowsInsensitive,
}

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

#[cfg(windows)]
pub(in crate::filesystem::sandbox) fn windows_names_equal(
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let left = left.encode_wide().collect::<Vec<_>>();
    let right = right.encode_wide().collect::<Vec<_>>();
    unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        ) == CSTR_EQUAL
    }
}

#[cfg(all(test, not(windows)))]
pub(in crate::filesystem::sandbox) fn windows_names_equal(
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

impl PartialEq for NativeNameCoordinationKey {
    fn eq(&self, other: &Self) -> bool {
        match (self.mode, other.mode) {
            (NativeNameComparisonMode::Exact, NativeNameComparisonMode::Exact) => {
                self.name == other.name
            }
            (NativeNameComparisonMode::Conservative, NativeNameComparisonMode::Conservative) => {
                true
            }
            #[cfg(any(test, windows))]
            (
                NativeNameComparisonMode::WindowsInsensitive,
                NativeNameComparisonMode::WindowsInsensitive,
            ) => windows_names_equal(&self.name, &other.name),
            _ => false,
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
        match (self.mode, other.mode) {
            (NativeNameComparisonMode::Exact, NativeNameComparisonMode::Exact) => {
                self.name == other.name
            }
            (NativeNameComparisonMode::Conservative, _)
            | (_, NativeNameComparisonMode::Conservative) => true,
            #[cfg(any(test, windows))]
            (
                NativeNameComparisonMode::WindowsInsensitive,
                NativeNameComparisonMode::WindowsInsensitive,
            ) => windows_names_equal(&self.name, &other.name),
            #[cfg(any(test, windows))]
            _ => true,
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
