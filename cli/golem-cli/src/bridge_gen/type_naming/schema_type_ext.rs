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
use golem_common::schema::unstructured::is_unstructured_variant;

fn is_path_leaf_type(typ: &SchemaType) -> bool {
    // A role-marked unstructured-text/binary variant renders inline as the
    // ergonomic wrapper type, so it is a leaf — the walker must not descend
    // into its `inline` / `url` cases.
    if is_unstructured_variant(typ) {
        return true;
    }
    match typ {
        // Structural composites with nested children — non-leaf in the
        // type-location path the naming walker tracks.
        SchemaType::Variant { .. }
        | SchemaType::Result { .. }
        | SchemaType::Option { .. }
        | SchemaType::Record { .. }
        | SchemaType::Tuple { .. }
        | SchemaType::List { .. }
        // Rich containers carry nested user `SchemaType`s; the walker
        // descends into them, so they are non-leaf path elements.
        | SchemaType::FixedList { .. }
        | SchemaType::Map { .. }
        | SchemaType::Union { .. }
        | SchemaType::Future { .. }
        | SchemaType::Stream { .. } => false,
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
        | SchemaType::String { .. }
        // Rich scalar / capability types whose payloads carry no nested
        // user `SchemaType` are leaves.
        | SchemaType::Text { .. }
        | SchemaType::Binary { .. }
        | SchemaType::Path { .. }
        | SchemaType::Url { .. }
        | SchemaType::Datetime { .. }
        | SchemaType::Duration { .. }
        | SchemaType::Quantity { .. }
        | SchemaType::Secret { .. }
        | SchemaType::QuotaToken { .. } => true,
    }
}

fn can_be_named(typ: &SchemaType) -> bool {
    // A role-marked unstructured-text/binary variant renders inline as the
    // ergonomic wrapper type (`UnstructuredText` / `UnstructuredBinary`); it
    // must never be hoisted into a generated nominal enum.
    if is_unstructured_variant(typ) {
        return false;
    }
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
        | SchemaType::List { .. }
        // A discriminated union is a tagged sum like `Variant`: it can carry
        // a generated name (a TS tagged-union alias / a Rust enum).
        | SchemaType::Union { .. } => true,
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
        // Rich containers/scalars that render inline in both target
        // languages (`Map<K,V>`/`Vec<(K,V)>`, `T[]`/`Vec<T>`, string,
        // bigint, wrapper structs, …) never need a generated alias.
        // `Future`/`Stream` have no value surface and error at emission.
        SchemaType::FixedList { .. }
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
