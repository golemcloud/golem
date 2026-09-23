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

use crate::bridge_gen::go::go::to_exported_ident;
use crate::bridge_gen::type_naming::TypeName;
use golem_common::schema::schema_type::SchemaType;
use itertools::Itertools;
use std::fmt::{Display, Formatter};

/// A generated Go type name.
///
/// Always exported, because the generated types cross a package boundary: the
/// client package holds them, and the consuming program uses them.
///
/// Go needs no reserved-runtime-name list, unlike the Scala and MoonBit
/// generators. The runtime is always referenced through its package qualifier,
/// so a generated `Option` is `client.Option` and never collides with
/// `bridge.Option`. Go's own predeclared names are all lowercase, so an exported
/// name cannot shadow one either.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GoTypeName {
    pub name: String,
}

impl From<String> for GoTypeName {
    fn from(value: String) -> Self {
        Self { name: value }
    }
}

impl From<&str> for GoTypeName {
    fn from(value: &str) -> Self {
        Self {
            name: value.to_string(),
        }
    }
}

impl Display for GoTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl TypeName for GoTypeName {
    fn from_schema_type(_typ: &SchemaType) -> Option<Self> {
        None
    }

    fn from_owner_and_name(
        owner: Option<impl AsRef<str>>,
        name: impl AsRef<str>,
        _same_language: bool,
    ) -> Self {
        // WIT type names are kebab-case whatever the source language, so a
        // generated Go type name is always UpperCamelCase — which is also what
        // exports it.
        match owner {
            Some(owner) => format!(
                "{}{}",
                to_exported_ident(owner.as_ref()),
                to_exported_ident(name.as_ref())
            ),
            None => to_exported_ident(name.as_ref()),
        }
        .into()
    }

    fn from_segments(
        segments: impl IntoIterator<Item = impl AsRef<str>>,
        _same_language: bool,
    ) -> Self {
        segments
            .into_iter()
            .map(|segment| to_exported_ident(segment.as_ref()))
            .join("")
            .into()
    }

    /// Which schema types become a named Go declaration.
    ///
    /// Go has no anonymous sum type, so a variant, a union, an enum and a set of
    /// flags each need one. Everything else Go can spell inline: a list is
    /// `[]T`, a fixed list is `[N]T`, an option is `*T`, a result is
    /// `Result[Ok, Err]`, a tuple is `TupleN[…]`, and a map is a slice of
    /// entries, since a schema map's keys are not restricted to strings.
    fn requires_type_name(typ: &SchemaType) -> bool {
        match typ {
            // A ref always carries its own generated name.
            SchemaType::Ref { .. } => true,
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
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

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn names_are_exported_upper_camel() {
        assert_eq!(
            GoTypeName::from_owner_and_name(None::<&str>, "order-line", false).to_string(),
            "OrderLine"
        );
        assert_eq!(
            GoTypeName::from_owner_and_name(Some("shop"), "order-line", false).to_string(),
            "ShopOrderLine"
        );
        assert_eq!(
            GoTypeName::from_segments(["shop", "order", "line"], false).to_string(),
            "ShopOrderLine"
        );
    }

    /// A schema name that is not a valid Go identifier still has to produce an
    /// exported one, or the generated package cannot expose the type at all.
    #[test]
    fn an_awkward_schema_name_still_exports() {
        for name in ["2nd-line", "", "a.b"] {
            let generated = GoTypeName::from_owner_and_name(None::<&str>, name, false).to_string();
            assert!(
                generated.starts_with(|ch: char| ch.is_ascii_uppercase()),
                "{name} produced {generated}, which is not exported"
            );
        }
    }
}
