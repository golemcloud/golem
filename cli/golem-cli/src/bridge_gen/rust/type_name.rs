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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RustTypeName {
    Derived(String),
    #[allow(dead_code)]
    Remapped(/* TODO */),
}

impl From<String> for RustTypeName {
    fn from(value: String) -> Self {
        Self::Derived(value)
    }
}

impl From<&str> for RustTypeName {
    fn from(value: &str) -> Self {
        Self::Derived(value.to_string())
    }
}

impl Display for RustTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RustTypeName::Derived(string) => {
                write!(f, "{}", string)
            }
            RustTypeName::Remapped() => {
                todo!()
            }
        }
    }
}

impl TypeName for RustTypeName {
    fn from_schema_type(_typ: &SchemaType) -> Option<Self> {
        // NOTE: custom remapping can be added here later
        None
    }

    fn from_owner_and_name(
        owner: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        _same_language: bool,
    ) -> Self {
        // WIT type names are always kebab-case regardless of source language,
        // so we always need to convert to UpperCamelCase for Rust.
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
        // WIT type names are always kebab-case regardless of source language,
        // so we always need to convert to UpperCamelCase for Rust.
        segments
            .into_iter()
            .map(|segment| segment.as_ref().to_upper_camel_case())
            .join("")
            .into()
    }

    fn requires_type_name(typ: &SchemaType) -> bool {
        match typ {
            // Refs always carry a generated name.
            SchemaType::Ref { .. } => true,
            // Inline composites that benefit from a generated alias /
            // struct / enum.
            SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Record { .. } => true,
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
            | SchemaType::Union { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => false,
        }
    }
}
