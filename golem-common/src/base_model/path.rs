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

use derive_more::Display;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::Deref;
use std::str::FromStr;
use typed_path::Utf8UnixPathBuf;

/// A canonical, absolute, normalized file path.
/// Must be:
/// - absolute (start with '/')
/// - not contain ".." components
/// - not contain "." components
/// - use '/' as a separator
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Display)]
pub struct CanonicalFilePath(Utf8UnixPathBuf);

impl CanonicalFilePath {
    pub fn from_abs_str(s: &str) -> Result<Self, String> {
        let buf: Utf8UnixPathBuf = s.into();
        if !buf.is_absolute() {
            return Err("Path must be absolute".to_string());
        }
        Ok(CanonicalFilePath(buf.normalize()))
    }

    pub fn from_rel_str(s: &str) -> Result<Self, String> {
        Self::from_abs_str(&format!("/{s}"))
    }

    pub fn from_either_str(s: &str) -> Result<Self, String> {
        if s.starts_with('/') {
            Self::from_abs_str(s)
        } else {
            Self::from_rel_str(s)
        }
    }

    pub fn as_path(&self) -> &Utf8UnixPathBuf {
        &self.0
    }

    pub fn as_abs_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn to_abs_string(&self) -> String {
        self.0.to_string()
    }

    pub fn to_rel_string(&self) -> String {
        self.0.strip_prefix("/").unwrap().to_string()
    }

    pub fn extend(&mut self, path: &str) -> Result<(), String> {
        self.0.push_checked(path).map_err(|e| e.to_string())
    }
}

impl Serialize for CanonicalFilePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        String::serialize(&self.to_string(), serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalFilePath {
    fn deserialize<D>(deserializer: D) -> Result<CanonicalFilePath, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = String::deserialize(deserializer)?;
        Self::from_abs_str(&str).map_err(serde::de::Error::custom)
    }
}

impl FromStr for CanonicalFilePath {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_either_str(s)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for CanonicalFilePath {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&self.to_abs_string(), context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for CanonicalFilePath {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let s = <String as desert_rust::BinaryDeserializer>::deserialize(context)?;
        Self::from_abs_str(&s).map_err(|e| {
            desert_rust::Error::DeserializationFailure(format!("Invalid CanonicalFilePath: {e}"))
        })
    }
}

// ── ArchiveFilePath ──────────────────────────────────────────────────────────

/// A path inside an uploaded archive (the source side of a file mapping).
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Display)]
pub struct ArchiveFilePath(pub CanonicalFilePath);

impl ArchiveFilePath {
    pub fn from_abs_str(s: &str) -> Result<Self, String> {
        CanonicalFilePath::from_abs_str(s).map(ArchiveFilePath)
    }

    pub fn from_rel_str(s: &str) -> Result<Self, String> {
        CanonicalFilePath::from_rel_str(s).map(ArchiveFilePath)
    }

    pub fn from_either_str(s: &str) -> Result<Self, String> {
        CanonicalFilePath::from_either_str(s).map(ArchiveFilePath)
    }
}

impl Deref for ArchiveFilePath {
    type Target = CanonicalFilePath;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for ArchiveFilePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ArchiveFilePath {
    fn deserialize<D>(deserializer: D) -> Result<ArchiveFilePath, D::Error>
    where
        D: Deserializer<'de>,
    {
        CanonicalFilePath::deserialize(deserializer).map(ArchiveFilePath)
    }
}

impl FromStr for ArchiveFilePath {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_either_str(s)
    }
}

// ── AgentFilePath ────────────────────────────────────────────────────────────

/// A path in an agent's filesystem (the deployed target side of a file mapping).
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Display)]
pub struct AgentFilePath(pub CanonicalFilePath);

impl AgentFilePath {
    pub fn from_abs_str(s: &str) -> Result<Self, String> {
        CanonicalFilePath::from_abs_str(s).map(AgentFilePath)
    }

    pub fn from_rel_str(s: &str) -> Result<Self, String> {
        CanonicalFilePath::from_rel_str(s).map(AgentFilePath)
    }

    pub fn from_either_str(s: &str) -> Result<Self, String> {
        CanonicalFilePath::from_either_str(s).map(AgentFilePath)
    }
}

impl Deref for AgentFilePath {
    type Target = CanonicalFilePath;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for AgentFilePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AgentFilePath {
    fn deserialize<D>(deserializer: D) -> Result<AgentFilePath, D::Error>
    where
        D: Deserializer<'de>,
    {
        CanonicalFilePath::deserialize(deserializer).map(AgentFilePath)
    }
}

impl FromStr for AgentFilePath {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_either_str(s)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for AgentFilePath {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&self.0, context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for AgentFilePath {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        <CanonicalFilePath as desert_rust::BinaryDeserializer>::deserialize(context)
            .map(AgentFilePath)
    }
}
