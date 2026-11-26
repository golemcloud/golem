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

use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};

/// A canonical path relative to the volume root. The empty path names the root.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "Box<str>", into = "Box<str>")]
pub struct VolumePath(Box<str>);

// An endpoint can reflect credentials into paths, file references and cursors.
// Keep their Debug output opaque without changing the values callers record.
impl Debug for VolumePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("VolumePath")
    }
}

impl VolumePath {
    pub fn new(path: impl Into<Box<str>>) -> Result<Self, VolumeError> {
        let path = path.into();
        if path.chars().any(char::is_control)
            || (!path.is_empty() && path.split('/').any(|part| matches!(part, "" | "." | "..")))
        {
            Err(VolumeError::UnrepresentablePath)
        } else {
            Ok(Self(path))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<Box<str>> for VolumePath {
    type Error = VolumeError;

    fn try_from(value: Box<str>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VolumePath> for Box<str> {
    fn from(value: VolumePath) -> Self {
        value.0
    }
}

/// The state of an open file. Its representation is interpreted by the store adapter.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    etag: Box<str>,
    version: Option<Box<str>>,
}

impl Debug for FileState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileState")
    }
}

impl FileState {
    /// Builds the opaque state from a store's reference metadata.
    pub fn from_reference(etag: Box<str>, version: Option<Box<str>>) -> Self {
        Self {
            etag,
            version: version.filter(|version| version.as_ref() != "null"),
        }
    }

    /// Exposes reference metadata for store adapters.
    pub fn reference(&self) -> (&str, Option<&str>) {
        (&self.etag, self.version.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    File { size: u64 },
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: VolumePath,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub entries: Box<[Entry]>,
    pub after: Option<Cursor>,
}

/// A store's opaque continuation of a directory listing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor(Box<str>);

impl Debug for Cursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cursor")
    }
}

impl Cursor {
    pub fn new(token: impl Into<Box<str>>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum VolumeError {
    #[error("the volume cannot represent the path")]
    UnrepresentablePath,
    #[error("nothing exists at the volume path")]
    NotFound,
    #[error("the volume file changed or its recorded state is no longer available")]
    Changed,
    #[error("the volume denied {action}")]
    Denied { action: Box<str> },
    #[error("the volume requires {requirement}")]
    Requirement { requirement: Box<str> },
    /// The request failed without a usable answer, including a complete backend failure response.
    #[error("the volume request failed without a usable answer")]
    Unanswered,
    #[error("the volume connection was refused; nothing was sent")]
    ConnectionRefused,
    #[error("invalid volume configuration or request")]
    InvalidRequest,
    #[error("the volume response is invalid")]
    InvalidResponse,
}

#[cfg(test)]
mod tests;
