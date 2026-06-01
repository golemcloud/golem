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

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use std::fmt::{Display, Formatter};

/// Error returned by the temporary adapter layer between the legacy types
/// (`AnalysedType`, `Value`, `ValueAndType`, `DataSchema`, `ElementSchema`)
/// and the new schema layer (`SchemaType`, `SchemaValue`, `TypedSchemaValue`,
/// `InputSchema`, `OutputSchema`, `AgentTypeSchema`).
///
/// The new schema layer is a strict superset of the legacy form. Forward
/// (legacy → new) conversion can still fail for shapes that are
/// inexpressible in the new layer (handle types) or metadata that has no
/// home (owner without name). Reverse (new → legacy) is partial: rich
/// scalars / unions / capability nodes have no legacy counterpart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaAdapterError {
    /// `AnalysedType::Handle` cannot be represented in the new schema layer;
    /// resource handles are explicitly excluded.
    LegacyHandle,
    /// Legacy metadata that the adapter cannot encode. Examples: a legacy
    /// composite type with `owner = Some(_)` but `name = None` (no place
    /// to anchor the dotted [`TypeId`]).
    UnsupportedLegacyMetadata(String),
    /// A new schema type case has no legacy counterpart and cannot be
    /// projected back into `AnalysedType` / legacy `Value`.
    LossySchemaType(String),
    /// Value shape does not match the type that was supposed to drive
    /// decoding (e.g. record arity mismatch, variant case out of range).
    ValueShapeMismatch(String),
    /// The schema graph has a recursive cycle. Legacy `AnalysedType` is
    /// purely tree-shaped and cannot represent the cycle.
    RecursiveRef(TypeId),
    /// `SchemaType::Ref` pointed at a `TypeId` not present in the graph.
    DanglingRef(TypeId),
    /// A `TypeId` was registered twice with conflicting bodies. (Same body
    /// is deduplicated silently; different bodies are an error.)
    DuplicateTypeIdConflict(TypeId),
}

impl Display for SchemaAdapterError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LegacyHandle => write!(
                f,
                "legacy `AnalysedType::Handle` is not representable in the schema layer"
            ),
            Self::UnsupportedLegacyMetadata(reason) => {
                write!(f, "legacy type metadata cannot be represented: {reason}")
            }
            Self::LossySchemaType(reason) => {
                write!(
                    f,
                    "schema type cannot be projected back to legacy form: {reason}"
                )
            }
            Self::ValueShapeMismatch(reason) => write!(f, "value shape mismatch: {reason}"),
            Self::RecursiveRef(id) => write!(
                f,
                "schema graph contains a recursive reference not representable by legacy types: {id}"
            ),
            Self::DanglingRef(id) => write!(f, "schema graph contains dangling reference: {id}"),
            Self::DuplicateTypeIdConflict(id) => write!(
                f,
                "two distinct types registered under the same TypeId: {id}"
            ),
        }
    }
}

impl std::error::Error for SchemaAdapterError {}

/// Build a dotted [`TypeId`] from legacy `owner` / `name` pair, following
/// §4.20.
///
/// Rules:
/// - `name = None, owner = None` → `Ok(None)`: caller emits inline anonymous
///   form.
/// - `name = Some(n), owner = None` → `Ok(Some(TypeId(n)))`: bare name.
/// - `name = Some(n), owner = Some(o)` → `Ok(Some(TypeId(o.n)))`: dotted
///   composite. `::` in either part is normalised to `.`.
/// - `name = None, owner = Some(_)` →
///   [`SchemaAdapterError::UnsupportedLegacyMetadata`]: owner cannot be
///   preserved without a name.
pub fn legacy_type_id(
    owner: Option<&str>,
    name: Option<&str>,
) -> Result<Option<TypeId>, SchemaAdapterError> {
    match (owner, name) {
        (None, None) => Ok(None),
        (None, Some(name)) => Ok(Some(TypeId(normalise(name)))),
        (Some(owner), Some(name)) => {
            Ok(Some(TypeId(format!("{}.{}", normalise(owner), normalise(name)))))
        }
        (Some(owner), None) => Err(SchemaAdapterError::UnsupportedLegacyMetadata(format!(
            "owner `{owner}` provided without a name"
        ))),
    }
}

fn normalise(s: &str) -> String {
    s.replace("::", ".")
}

/// Walk through any number of [`SchemaType::Ref`] indirections and return the
/// first non-ref body in `graph`. Detects recursive cycles
/// ([`SchemaAdapterError::RecursiveRef`]) and dangling references
/// ([`SchemaAdapterError::DanglingRef`]).
pub fn resolve_ref<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<&'a SchemaType, SchemaAdapterError> {
    let mut visiting: Vec<TypeId> = Vec::new();
    let mut current = ty;
    loop {
        match current {
            SchemaType::Ref { id, .. } => {
                if visiting.iter().any(|x| x == id) {
                    return Err(SchemaAdapterError::RecursiveRef(id.clone()));
                }
                let def = graph
                    .lookup(id)
                    .ok_or_else(|| SchemaAdapterError::DanglingRef(id.clone()))?;
                visiting.push(id.clone());
                current = &def.body;
            }
            other => return Ok(other),
        }
    }
}
