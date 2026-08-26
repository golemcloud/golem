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

use crate::bridge_gen::type_naming::TypeName;
use golem_common::schema::schema_type::SchemaType;
use heck::ToUpperCamelCase;
use itertools::Itertools;
use std::fmt::{Display, Formatter};

/// Canonical id of the cross-SDK UUID builtin record (two `u64` halves), shared
/// by all language SDKs. The Scala bridge remaps it onto the runtime `Uuid`
/// type instead of generating a structural record, matching the Scala SDK's
/// type mapping.
const UUID_TYPE_ID: &str = "uuid.Uuid";

/// A runtime type a generated name maps onto directly, instead of a generated
/// definition. Mirrors the Scala SDK, which surfaces these builtins as their
/// ergonomic runtime types rather than their structural schema bodies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RemappedType {
    /// The `uuid.Uuid` builtin record → `golem.bridge.runtime.Uuid`.
    Uuid,
}

impl RemappedType {
    /// The Scala type as it is referenced in generated code (the runtime
    /// package is wildcard-imported, so the simple name is enough).
    pub fn rendered(&self) -> &'static str {
        match self {
            RemappedType::Uuid => "Uuid",
        }
    }
}

/// A generated Scala type name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScalaTypeName {
    /// A name derived from the schema (owner + name, or location segments). The
    /// generator emits a definition (case class / sealed trait / type alias)
    /// for it.
    Derived(String),
    /// A name that maps onto an existing runtime type. No definition is emitted
    /// and the schema body is not walked.
    Remapped(RemappedType),
}

impl ScalaTypeName {
    pub fn as_remapped(&self) -> Option<&RemappedType> {
        match self {
            ScalaTypeName::Remapped(remapped) => Some(remapped),
            ScalaTypeName::Derived(_) => None,
        }
    }
}

impl From<String> for ScalaTypeName {
    fn from(value: String) -> Self {
        Self::Derived(value)
    }
}

impl From<&str> for ScalaTypeName {
    fn from(value: &str) -> Self {
        Self::Derived(value.to_string())
    }
}

impl Display for ScalaTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ScalaTypeName::Derived(string) => write!(f, "{string}"),
            ScalaTypeName::Remapped(remapped) => write!(f, "{}", remapped.rendered()),
        }
    }
}

impl TypeName for ScalaTypeName {
    fn from_schema_type(typ: &SchemaType) -> Option<Self> {
        match typ {
            SchemaType::Ref { id, .. } if id.0 == UUID_TYPE_ID => {
                Some(ScalaTypeName::Remapped(RemappedType::Uuid))
            }
            _ => None,
        }
    }

    fn from_owner_and_name(
        owner: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        _same_language: bool,
    ) -> Self {
        // WIT type names are kebab-case regardless of source language, so the
        // generated Scala type name is always UpperCamelCase. This matches the
        // Rust and TypeScript generators.
        Self::Derived(match owner {
            Some(owner) => format!(
                "{}{}",
                owner.as_ref().to_upper_camel_case(),
                name.as_ref().to_upper_camel_case()
            ),
            None => name.as_ref().to_upper_camel_case(),
        })
    }

    fn from_segments(
        segments: impl IntoIterator<Item = impl AsRef<str>>,
        _same_language: bool,
    ) -> Self {
        segments
            .into_iter()
            .map(|segment| segment.as_ref().to_upper_camel_case())
            .join("")
            .into()
    }

    fn requires_type_name(typ: &SchemaType) -> bool {
        match typ {
            // A ref always carries its own generated name.
            SchemaType::Ref { .. } => true,
            // Composites that become a generated case class / sealed trait.
            SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Record { .. }
            // A discriminated union becomes a generated sealed trait.
            | SchemaType::Union { .. } => true,
            SchemaType::Result { .. }
            | SchemaType::Option { .. }
            | SchemaType::Tuple { .. }
            | SchemaType::List { .. }
            | SchemaType::Bool { .. }
            | SchemaType::S8 { .. }
            | SchemaType::S16 { .. }
            | SchemaType::S32 { .. }
            | SchemaType::S64 { .. }
            | SchemaType::U8 { .. }
            | SchemaType::U16 { .. }
            | SchemaType::U32 { .. }
            | SchemaType::U64 { .. }
            | SchemaType::F32 { .. }
            | SchemaType::F64 { .. }
            | SchemaType::Char { .. }
            | SchemaType::String { .. }
            | SchemaType::FixedList { .. }
            | SchemaType::Map { .. }
            | SchemaType::Text { .. }
            | SchemaType::Binary { .. }
            | SchemaType::Path { .. }
            | SchemaType::Url { .. }
            | SchemaType::Datetime { .. }
            | SchemaType::Duration { .. }
            | SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => false,
        }
    }
}
