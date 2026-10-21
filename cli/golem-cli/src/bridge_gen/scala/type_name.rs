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

/// A generated Scala type name, derived from the schema (owner + name, or
/// location segments). The generator emits a definition (case class / sealed
/// trait / type alias) for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScalaTypeName(pub String);

impl From<String> for ScalaTypeName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ScalaTypeName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl Display for ScalaTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TypeName for ScalaTypeName {
    fn from_owner_and_name(
        owner: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        _same_language: bool,
    ) -> Self {
        // WIT type names are kebab-case regardless of source language, so the
        // generated Scala type name is always UpperCamelCase. This matches the
        // Rust and TypeScript generators.
        Self(match owner {
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
            | SchemaType::Uuid { .. }
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
