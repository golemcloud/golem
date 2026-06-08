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
use std::fmt::{Display, Formatter};

/// Stable, language-independent identifier for a named type definition. Must
/// be unique within the enclosing [`super::SchemaGraph`]. Conventional format
/// is a dot-separated namespace path (e.g., `"myapp.users.user"`). Each SDK
/// provides a default derivation rule (typically based on the local language's
/// type name); cross-language interop requires the same `TypeId` on every side,
/// which users can pin via the SDK's `named` attribute.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct TypeId(pub String);

impl TypeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for TypeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TypeId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for TypeId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Typed metadata envelope. Holds non-validation, non-rendering-critical
/// information (docs, aliases, examples, deprecation, role). Per-scalar
/// validation constraints live on the relevant scalar's typed substructure,
/// not here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct MetadataEnvelope {
    /// Free-form documentation string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Alternative names this type is also known by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Canonical-encoded example values, each as a JSON string. Empty = no
    /// examples. Stored as strings so metadata is self-contained on the type
    /// side and does not have to cross-reference an accompanying value tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// Deprecation message; `None` means not deprecated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    /// Optional role annotation tagging a type with a consumer-facing intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

impl MetadataEnvelope {
    pub fn is_empty(&self) -> bool {
        self.doc.is_none()
            && self.aliases.is_empty()
            && self.examples.is_empty()
            && self.deprecated.is_none()
            && self.role.is_none()
    }
}

/// Open registry of consumer-facing roles a type may carry. Unknown roles are
/// preserved as [`Role::Other`] so the producer's intent is not lost when a
/// receiver does not understand the role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
pub enum Role {
    Multimodal,
    Other(String),
}
