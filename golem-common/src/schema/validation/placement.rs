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
//!
//! Two entry points:
//!
//! - [`validate_placement`] walks `graph.root` plus every named definition
//!   reachable in `graph.defs` and validates against a single
//!   [`SchemaScope`]. This is the right call for single-root carriers
//!   (typed values, `Custom` oplog payloads, REST/RPC envelopes).
//! - [`validate_agent_type_placement`] walks an [`AgentTypeSchema`]: the
//!   constructor input fields are validated against
//!   [`SchemaScope::Constructor`], method inputs and outputs against
//!   [`SchemaScope::Boundary`], the agent's named defs against
//!   [`SchemaScope::Boundary`], and each dependency recursively against
//!   its own graph. The sentinel `graph.root` on agent carriers is not
//!   walked.

use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema, InputSchema,
    OutputSchema,
};
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
/// Multimodal detection: a `list<union<…>>` is treated as multimodal when
/// any of the following carry `metadata.role == Some(Role::Multimodal)`:
/// the enclosing field/def metadata, the list node's own metadata, the
/// inner element `Ref`'s metadata, or the inner union node's metadata.
/// Refs are resolved with cycle detection before the shape is classified.
pub fn validate_placement(
    graph: &SchemaGraph,
    scope: SchemaScope,
) -> Result<(), Vec<PlacementError>> {
    let mut errors = Vec::new();
    let mut visited_defs: Vec<&TypeId> = Vec::new();

    let root_metadata = graph.root.metadata().clone();
    walk_type(
        graph,
        &graph.root,
        &root_metadata,
        scope,
        &mut errors,
        &mut visited_defs,
    );

    for def in &graph.defs {
        if !visited_defs.contains(&&def.id) {
            let body_metadata = def.body.metadata().clone();
            walk_type(
                graph,
                &def.body,
                &body_metadata,
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
    // Constructor-scope check: a list<union<…>> tagged anywhere on its
    // metadata-carrying nodes (enclosing field/def metadata, list node
    // metadata, inner element Ref metadata, or inner union metadata) with
    // role=Multimodal is forbidden. Refs are resolved with cycle detection.
    if scope == SchemaScope::Constructor
        && is_multimodal_list_of_union(graph, ty, enclosing_metadata)
    {
        errors.push(PlacementError::MultimodalListNotAllowedInConstructor);
    }

    match ty {
        SchemaType::Secret { .. } if scope == SchemaScope::Constructor => {
            errors.push(PlacementError::SecretNotAllowed { scope });
        }
        SchemaType::QuotaToken { .. } if scope == SchemaScope::Constructor => {
            errors.push(PlacementError::QuotaTokenNotAllowed { scope });
        }

        SchemaType::Ref { id, .. } => {
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

        SchemaType::Record { fields, .. } => {
            for field in fields {
                walk_type(graph, &field.body, &field.metadata, scope, errors, visited);
            }
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let Some(p) = &case.payload {
                    walk_type(graph, p, &case.metadata, scope, errors, visited);
                }
            }
        }
        SchemaType::Tuple { elements, .. } => {
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
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            walk_type(
                graph,
                element,
                &MetadataEnvelope::default(),
                scope,
                errors,
                visited,
            );
        }
        SchemaType::Map { key, value, .. } => {
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
        SchemaType::Option { inner, .. } => {
            walk_type(
                graph,
                inner,
                &MetadataEnvelope::default(),
                scope,
                errors,
                visited,
            );
        }
        SchemaType::Result { spec, .. } => {
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
        SchemaType::Union { spec, .. } => {
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
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
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

fn has_multimodal_role(metadata: &MetadataEnvelope) -> bool {
    matches!(metadata.role, Some(Role::Multimodal))
}

/// Whether `ty` is a `list<union<…>>` (or `fixed-list<union<…>>`) tagged
/// as multimodal somewhere on its metadata-carrying nodes:
///
/// - the enclosing field / def metadata,
/// - the list (or fixed-list) node's own metadata,
/// - the inner element `Ref`'s metadata, or
/// - the inner union node's metadata.
///
/// Refs are resolved with cycle detection before the shape is classified.
fn is_multimodal_list_of_union(
    graph: &SchemaGraph,
    ty: &SchemaType,
    enclosing_metadata: &MetadataEnvelope,
) -> bool {
    let outer_role = has_multimodal_role(enclosing_metadata) || has_multimodal_role(ty.metadata());

    let mut visited: Vec<TypeId> = Vec::new();
    let Some(resolved) = resolve_ref_chain(graph, ty, &mut visited) else {
        return false;
    };

    let (list_metadata, element) = match resolved {
        SchemaType::List { element, metadata } => (metadata, element),
        SchemaType::FixedList {
            element, metadata, ..
        } => (metadata, element),
        _ => return false,
    };

    let list_role = outer_role || has_multimodal_role(list_metadata);
    let element_ref_role = has_multimodal_role(element.metadata());

    let mut visited_inner: Vec<TypeId> = Vec::new();
    match resolve_ref_chain(graph, element.as_ref(), &mut visited_inner) {
        Some(SchemaType::Union { metadata, .. }) => {
            list_role || element_ref_role || has_multimodal_role(metadata)
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
            SchemaType::Ref { id, .. } => {
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

// --------------------------------------------------------------------------
// Agent-aware placement
// --------------------------------------------------------------------------

/// Validate placement of every schema node carried by an [`AgentTypeSchema`].
///
/// Walks:
/// - the constructor input fields against [`SchemaScope::Constructor`]
/// - each method's input and output bodies against [`SchemaScope::Boundary`]
/// - every named def in [`AgentTypeSchema::schema`] against
///   [`SchemaScope::Boundary`]
/// - each dependency, recursively, against its own
///   [`AgentDependencySchema::schema`]
///
/// The sentinel `graph.root` carried by agent-layer [`SchemaGraph`]s is
/// **not** walked (see §4.22).
///
/// This is a **placement-only** validator. Structural well-formedness
/// (dangling refs, duplicate ids, …) is the responsibility of
/// [`crate::schema::validation::validate_graph`] / the structural
/// validators; callers that need both must invoke both.
pub fn validate_agent_type_placement(ty: &AgentTypeSchema) -> Result<(), Vec<PlacementError>> {
    let mut errors = Vec::new();

    walk_agent_constructor(&ty.schema, &ty.constructor, &mut errors);
    for method in &ty.methods {
        walk_agent_method(&ty.schema, method, &mut errors);
    }
    walk_agent_graph_defs(&ty.schema, &mut errors);

    for dep in &ty.dependencies {
        walk_agent_dependency(dep, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate placement of every schema node carried by a single
/// [`AgentDependencySchema`].
///
/// Same scope rules as [`validate_agent_type_placement`], but refs resolve
/// against the dependency's own graph and the dependency declares no
/// sub-dependencies.
pub fn validate_agent_dependency_placement(
    dep: &AgentDependencySchema,
) -> Result<(), Vec<PlacementError>> {
    let mut errors = Vec::new();
    walk_agent_dependency(dep, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn walk_agent_dependency(dep: &AgentDependencySchema, errors: &mut Vec<PlacementError>) {
    walk_agent_constructor(&dep.schema, &dep.constructor, errors);
    for method in &dep.methods {
        walk_agent_method(&dep.schema, method, errors);
    }
    walk_agent_graph_defs(&dep.schema, errors);
}

fn walk_agent_constructor(
    graph: &SchemaGraph,
    ctor: &AgentConstructorSchema,
    errors: &mut Vec<PlacementError>,
) {
    walk_input_schema(graph, &ctor.input_schema, SchemaScope::Constructor, errors);
}

fn walk_agent_method(
    graph: &SchemaGraph,
    method: &AgentMethodSchema,
    errors: &mut Vec<PlacementError>,
) {
    walk_input_schema(graph, &method.input_schema, SchemaScope::Boundary, errors);
    walk_output_schema(graph, &method.output_schema, SchemaScope::Boundary, errors);
}

fn walk_input_schema(
    graph: &SchemaGraph,
    input: &InputSchema,
    scope: SchemaScope,
    errors: &mut Vec<PlacementError>,
) {
    match input {
        InputSchema::Parameters(fields) => {
            for f in fields {
                let mut visited: Vec<&TypeId> = Vec::new();
                walk_type(graph, &f.schema, &f.metadata, scope, errors, &mut visited);
            }
        }
    }
}

fn walk_output_schema(
    graph: &SchemaGraph,
    output: &OutputSchema,
    scope: SchemaScope,
    errors: &mut Vec<PlacementError>,
) {
    match output {
        OutputSchema::Unit => {}
        OutputSchema::Single(ty) => {
            let enclosing = ty.metadata().clone();
            let mut visited: Vec<&TypeId> = Vec::new();
            walk_type(graph, ty, &enclosing, scope, errors, &mut visited);
        }
    }
}

/// Walk every def in `graph.defs` at [`SchemaScope::Boundary`].
///
/// Defs are validated at Boundary because shared defs may legitimately be
/// used by method inputs/outputs, where Constructor-only restrictions
/// (e.g. `Secret`) do not apply. Constructor-specific restrictions are
/// still enforced at constructor use sites: when a constructor parameter
/// is a [`SchemaType::Ref`], the constructor walk resolves the ref and
/// re-checks the resolved body under [`SchemaScope::Constructor`].
fn walk_agent_graph_defs(graph: &SchemaGraph, errors: &mut Vec<PlacementError>) {
    for def in &graph.defs {
        let body_metadata = def.body.metadata().clone();
        let mut visited: Vec<&TypeId> = Vec::new();
        walk_type(
            graph,
            &def.body,
            &body_metadata,
            SchemaScope::Boundary,
            errors,
            &mut visited,
        );
    }
}
