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

use golem_common::schema::schema_type::SchemaType;

fn is_path_leaf_type(typ: &SchemaType) -> bool {
    match typ {
        // Structural composites with nested children — non-leaf in the
        // type-location path the naming walker tracks.
        SchemaType::Variant { .. }
        | SchemaType::Result { .. }
        | SchemaType::Option { .. }
        | SchemaType::Record { .. }
        | SchemaType::Tuple { .. }
        | SchemaType::List { .. } => false,
        // A `Ref` resolves to a named definition in the surrounding graph;
        // the walker descends into the def body before recursing further,
        // so refs themselves are treated as non-leaf path elements.
        SchemaType::Ref { .. } => false,
        // Closed sums without nested children (enum/flags) and all
        // primitives are leaves.
        SchemaType::Enum { .. }
        | SchemaType::Flags { .. }
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
        | SchemaType::String { .. } => true,
        // The rich schema variants below have no legacy AnalysedType
        // counterpart and never appear when the bridge generator is fed
        // a legacy `AgentType` (the only construction path today). Treat
        // them as leaves so the walker doesn't try to descend into shapes
        // it doesn't know how to emit; the emission-time projection via
        // `schema_type_to_analysed_type` will surface a clear error if
        // such a type ever shows up.
        SchemaType::FixedList { .. }
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
        | SchemaType::Stream { .. } => true,
    }
}

fn can_be_named(typ: &SchemaType) -> bool {
    match typ {
        // Refs are always named: their identity is the def name in the
        // surrounding graph.
        SchemaType::Ref { .. } => true,
        // Composites without intrinsic names are candidates for synthetic
        // naming (anonymous bucket).
        SchemaType::Variant { .. }
        | SchemaType::Result { .. }
        | SchemaType::Option { .. }
        | SchemaType::Enum { .. }
        | SchemaType::Flags { .. }
        | SchemaType::Record { .. }
        | SchemaType::Tuple { .. }
        | SchemaType::List { .. } => true,
        // Primitives never need a generated type alias.
        SchemaType::Bool { .. }
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
        | SchemaType::String { .. } => false,
        // Rich variants without legacy counterparts: see comment in
        // `is_path_leaf_type`. Excluded from naming.
        SchemaType::FixedList { .. }
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

pub trait SchemaTypeExt {
    fn is_path_leaf_type(&self) -> bool;

    fn as_path_elem_type(&self) -> Option<&SchemaType>;

    fn can_be_named(&self) -> bool;
}

impl SchemaTypeExt for SchemaType {
    fn is_path_leaf_type(&self) -> bool {
        is_path_leaf_type(self)
    }

    fn as_path_elem_type(&self) -> Option<&SchemaType> {
        (!self.is_path_leaf_type()).then_some(self)
    }

    fn can_be_named(&self) -> bool {
        can_be_named(self)
    }
}

impl SchemaTypeExt for Option<SchemaType> {
    fn is_path_leaf_type(&self) -> bool {
        self.as_ref().is_none_or(SchemaType::is_path_leaf_type)
    }

    fn as_path_elem_type(&self) -> Option<&SchemaType> {
        self.as_ref().and_then(SchemaType::as_path_elem_type)
    }

    fn can_be_named(&self) -> bool {
        self.as_ref().is_some_and(SchemaType::can_be_named)
    }
}

impl SchemaTypeExt for Option<Box<SchemaType>> {
    fn is_path_leaf_type(&self) -> bool {
        self.as_ref().is_none_or(|typ| typ.is_path_leaf_type())
    }

    fn as_path_elem_type(&self) -> Option<&SchemaType> {
        self.as_ref().and_then(|typ| typ.as_path_elem_type())
    }

    fn can_be_named(&self) -> bool {
        self.as_ref().is_some_and(|typ| typ.can_be_named())
    }
}
