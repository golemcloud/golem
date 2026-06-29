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
use crate::bridge_gen::type_naming::schema_type_ext::SchemaTypeExt;
use golem_common::schema::schema_type::SchemaType;
use heck::ToUpperCamelCase;
use itertools::Itertools;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd)]
pub struct TypeScriptTypeName(String);

impl From<String> for TypeScriptTypeName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for TypeScriptTypeName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl Display for TypeScriptTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TypeName for TypeScriptTypeName {
    fn from_schema_type(_typ: &SchemaType) -> Option<Self> {
        None
    }

    fn from_owner_and_name(
        owner: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        _same_language: bool,
    ) -> Self {
        // WIT type names are always kebab-case regardless of source language,
        // so we always need to convert to UpperCamelCase for TypeScript.
        match owner {
            Some(owner) => format!(
                "{}{}",
                owner.as_ref().to_upper_camel_case(),
                name.as_ref().to_upper_camel_case()
            )
            .into(),
            None => name.as_ref().to_upper_camel_case().into(),
        }
    }

    fn from_segments(
        segments: impl IntoIterator<Item = impl AsRef<str>>,
        _same_language: bool,
    ) -> Self {
        // WIT type names are always kebab-case regardless of source language,
        // so we always need to convert to UpperCamelCase for TypeScript.
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
            // Inline variants need a named definition only when at least
            // one case carries a payload that itself needs a name — this
            // mirrors the legacy `AnalysedType::Variant` behaviour.
            SchemaType::Variant { cases, .. } => {
                cases.iter().any(|case| case.payload.can_be_named())
            }
            SchemaType::Flags { .. } | SchemaType::Record { .. } => true,
            SchemaType::Enum { .. }
            | SchemaType::Result { .. }
            | SchemaType::Option { .. }
            | SchemaType::List { .. }
            | SchemaType::Tuple { .. }
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
            | SchemaType::Union { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => false,
        }
    }
}

impl TypeScriptTypeName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
