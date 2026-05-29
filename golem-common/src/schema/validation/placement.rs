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

//! Placement-matrix checks for a [`SchemaGraph`].
//!
//! Encodes the per-[`SchemaType`]-case allow/deny table from the design.

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::schema_type::SchemaType;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Where a schema is being placed. Determines which per-node restrictions
/// apply.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SchemaScope {
    /// Constructor parameters / agent-id text strings.
    Constructor,
    /// Persisted oplog payloads.
    Persisted,
    /// REST / RPC boundary payloads.
    Boundary,
    /// Public docs / schema rendering.
    Docs,
    /// User-provided `Custom` durable payloads.
    Custom,
}

/// Errors raised by [`validate_placement`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// A [`SchemaType::Secret`] node appeared in a scope that forbids
    /// secrets (today: [`SchemaScope::Constructor`]).
    SecretNotAllowed { scope: SchemaScope },
    /// A [`SchemaType::QuotaToken`] node appeared in a scope that forbids
    /// quota tokens (today: [`SchemaScope::Constructor`]).
    QuotaTokenNotAllowed { scope: SchemaScope },
    /// A field / definition annotated with [`Role::Multimodal`] whose body
    /// is `list<union<…>>` appeared in [`SchemaScope::Constructor`].
    MultimodalListNotAllowedInConstructor,
}

impl Display for PlacementError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PlacementError::SecretNotAllowed { scope } => {
                write!(f, "secret values are not allowed in scope {scope:?}")
            }
            PlacementError::QuotaTokenNotAllowed { scope } => {
                write!(f, "quota-token values are not allowed in scope {scope:?}")
            }
            PlacementError::MultimodalListNotAllowedInConstructor => write!(
                f,
                "a multimodal `list<union<…>>` is not allowed in constructor scope"
            ),
        }
    }
}

impl Error for PlacementError {}

/// Validate that every node reachable from `graph.root` is allowed to appear
/// in `scope` according to the placement matrix.
///
/// Note on anonymous-root multimodal: an anonymous `list<union<…>>` placed
/// directly at `SchemaGraph::root` cannot be tagged with [`Role::Multimodal`]
/// because there is no metadata carrier on `SchemaGraph::root` and
/// [`SchemaType::List`] has no per-node metadata envelope. Users who want
/// constructor-time enforcement on a top-level multimodal payload must wrap
/// the root in a named [`crate::schema::graph::SchemaTypeDef`] whose
/// `metadata.role` is `Multimodal`.
///
// TODO: a future schema-model revision may attach metadata directly to
// `SchemaType::List` (or to `SchemaGraph::root`); when that lands this
// limitation goes away. No new schema-model fields are introduced here.
pub fn validate_placement(
    graph: &SchemaGraph,
    scope: SchemaScope,
) -> Result<(), Vec<PlacementError>> {
    let mut errors = Vec::new();
    let mut visited_defs: Vec<&TypeId> = Vec::new();

    walk_type(
        graph,
        &graph.root,
        &MetadataEnvelope::default(),
        scope,
        &mut errors,
        &mut visited_defs,
    );

    for def in &graph.defs {
        if !visited_defs.contains(&&def.id) {
            walk_type(
                graph,
                &def.body,
                &def.metadata,
                scope,
                &mut errors,
                &mut visited_defs,
            );
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn walk_type<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
    enclosing_metadata: &MetadataEnvelope,
    scope: SchemaScope,
    errors: &mut Vec<PlacementError>,
    visited: &mut Vec<&'a TypeId>,
) {
    // Constructor-scope check for the *enclosing* metadata: a list<union<…>>
    // tagged with role=Multimodal in the enclosing field/def metadata is
    // forbidden in Constructor. Refs are resolved (with cycle detection)
    // before deciding the body shape.
    if scope == SchemaScope::Constructor
        && matches!(enclosing_metadata.role, Some(Role::Multimodal))
        && is_list_of_union(graph, ty)
    {
        errors.push(PlacementError::MultimodalListNotAllowedInConstructor);
    }

    match ty {
        SchemaType::Secret(_) if scope == SchemaScope::Constructor => {
            errors.push(PlacementError::SecretNotAllowed { scope });
        }
        SchemaType::QuotaToken(_) if scope == SchemaScope::Constructor => {
            errors.push(PlacementError::QuotaTokenNotAllowed { scope });
        }

        SchemaType::Ref(id) => {
            if visited.contains(&id) {
                return;
            }
            if let Some(def) = graph.lookup(id) {
                visited.push(id);
                // Carry the *enclosing* metadata into the resolved body so the
                // multimodal-list check above can fire on `Ref`-wrapped
                // `list<union<…>>` bodies. The def's own metadata still
                // applies via the unvisited-defs sweep in
                // `validate_placement`.
                walk_type(graph, &def.body, enclosing_metadata, scope, errors, visited);
                visited.pop();
            }
        }

        SchemaType::Record { fields } => {
            for field in fields {
                walk_type(graph, &field.body, &field.metadata, scope, errors, visited);
            }
        }
        SchemaType::Variant { cases } => {
            for case in cases {
                if let Some(p) = &case.payload {
                    walk_type(graph, p, &case.metadata, scope, errors, visited);
                }
            }
        }
        SchemaType::Tuple { elements } => {
            for e in elements {
                walk_type(
                    graph,
                    e,
                    &MetadataEnvelope::default(),
                    scope,
                    errors,
                    visited,
                );
            }
        }
        SchemaType::List { element } | SchemaType::FixedList { element, .. } => {
            walk_type(
                graph,
                element,
                &MetadataEnvelope::default(),
                scope,
                errors,
                visited,
            );
        }
        SchemaType::Map { key, value } => {
            walk_type(
                graph,
                key,
                &MetadataEnvelope::default(),
                scope,
                errors,
                visited,
            );
            walk_type(
                graph,
                value,
                &MetadataEnvelope::default(),
                scope,
                errors,
                visited,
            );
        }
        SchemaType::Option { inner } => {
            walk_type(
                graph,
                inner,
                &MetadataEnvelope::default(),
                scope,
                errors,
                visited,
            );
        }
        SchemaType::Result(spec) => {
            if let Some(t) = &spec.ok {
                walk_type(
                    graph,
                    t,
                    &MetadataEnvelope::default(),
                    scope,
                    errors,
                    visited,
                );
            }
            if let Some(t) = &spec.err {
                walk_type(
                    graph,
                    t,
                    &MetadataEnvelope::default(),
                    scope,
                    errors,
                    visited,
                );
            }
        }
        SchemaType::Union(spec) => {
            for branch in &spec.branches {
                walk_type(
                    graph,
                    &branch.body,
                    &branch.metadata,
                    scope,
                    errors,
                    visited,
                );
            }
        }
        SchemaType::Future { inner } | SchemaType::Stream { inner } => {
            if let Some(t) = inner {
                walk_type(
                    graph,
                    t,
                    &MetadataEnvelope::default(),
                    scope,
                    errors,
                    visited,
                );
            }
        }

        // All other scalar / primitive cases are allowed in every scope.
        _ => {}
    }
}

/// Whether `ty` is a `list<union<…>>` or a `fixed-list<union<…>>`, looking
/// through any [`SchemaType::Ref`] chain (with cycle detection). The element
/// type may itself be a `Ref` resolving to a [`SchemaType::Union`].
fn is_list_of_union(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    let mut visited: Vec<TypeId> = Vec::new();
    let resolved = resolve_ref_chain(graph, ty, &mut visited);
    match resolved {
        Some(SchemaType::List { element }) | Some(SchemaType::FixedList { element, .. }) => {
            let mut visited_inner: Vec<TypeId> = Vec::new();
            matches!(
                resolve_ref_chain(graph, element.as_ref(), &mut visited_inner),
                Some(SchemaType::Union(_))
            )
        }
        _ => false,
    }
}

fn resolve_ref_chain<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
    visited: &mut Vec<TypeId>,
) -> Option<&'a SchemaType> {
    let mut current = ty;
    loop {
        match current {
            SchemaType::Ref(id) => {
                if visited.iter().any(|v| v == id) {
                    return None;
                }
                visited.push(id.clone());
                match graph.lookup(id) {
                    Some(def) => current = &def.body,
                    None => return None,
                }
            }
            other => return Some(other),
        }
    }
}
