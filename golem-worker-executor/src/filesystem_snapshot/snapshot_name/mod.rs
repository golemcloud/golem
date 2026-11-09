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

//! Snapshot names, their validation and invalid-name errors.

use std::fmt::{Display, Formatter};

/// The name of one filesystem snapshot of an agent.
///
/// A name has 1 to 64 characters, and each character is an ASCII letter, an ASCII digit, `-` or
/// `_`. So a name can be one segment of a path, and it can be a label.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SnapshotName(Box<str>);

impl SnapshotName {
    /// The largest number of characters in a name.
    const MAX_LENGTH: usize = 64;

    /// Gives the name with the text `text`, or an error when the text breaks a rule of a name.
    pub(crate) fn new(text: &str) -> Result<Self, InvalidSnapshotName> {
        let valid = (1..=Self::MAX_LENGTH).contains(&text.len())
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if valid {
            Ok(Self(text.into()))
        } else {
            Err(InvalidSnapshotName { text: text.into() })
        }
    }

    /// Gives the text of the name.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

// The hash and the order of a name are those of its text, so a set of names can be searched by
// the text.
impl std::borrow::Borrow<str> for SnapshotName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Display for SnapshotName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A text that breaks a rule of [`SnapshotName`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvalidSnapshotName {
    text: Box<str>,
}

impl Display for InvalidSnapshotName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the filesystem snapshot name {:?} does not have 1 to {} ASCII letters, digits, `-` or `_`",
            self.text,
            SnapshotName::MAX_LENGTH
        )
    }
}

impl std::error::Error for InvalidSnapshotName {}

#[cfg(test)]
mod tests;
