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

//! Detectors for the canonical multimodal schema shape
//! `list<variant<… Role::Multimodal>>`.

use crate::schema::graph::{RefResolutionError, SchemaGraph};
use crate::schema::metadata::Role;
use crate::schema::schema_type::{SchemaType, VariantCaseType};

/// If `ty` (resolved against `graph`) is the structural multimodal form
/// `list<variant<… Role::Multimodal>>`, return the variant's cases (one per
/// named alternative, with the case `name` carrying the alternative name).
pub fn multimodal_variant_cases<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a [VariantCaseType]>, RefResolutionError> {
    as_multimodal_list_variant(graph, ty)
}

/// Whether `ty` (resolved against `graph`) is the structural multimodal form
/// `list<variant<… Role::Multimodal>>`.
pub fn is_multimodal_schema_type(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<bool, RefResolutionError> {
    Ok(as_multimodal_list_variant(graph, ty)?.is_some())
}

/// If `ty` (resolved against `graph`) is the structural multimodal form
/// `list<variant<… Role::Multimodal>>`, return the variant's cases.
fn as_multimodal_list_variant<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a [VariantCaseType]>, RefResolutionError> {
    if let SchemaType::List { element, metadata } = graph.resolve_ref(ty)?
        && metadata.role == Some(Role::Multimodal)
        && let SchemaType::Variant { cases, .. } = graph.resolve_ref(element)?
    {
        return Ok(Some(cases));
    }
    Ok(None)
}
