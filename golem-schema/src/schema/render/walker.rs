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

//! Dynamic walker skeleton operating over `(SchemaGraph, SchemaType,
//! SchemaValue)`.
//!
//! The walker is the single public traversal API consumed by every renderer
//! in this module tree. It enforces the rule that a value never travels
//! without the schema that types it; the walker resolves named
//! [`crate::schema::SchemaType::Ref`] hops transparently and protects
//! against reference cycles by tracking the set of currently-active type
//! identifiers.
//!
//! The same ref-resolution / cycle-protection logic is exposed as
//! [`resolve_ref`] so decoders that consume `serde_json::Value` (and other
//! non-`SchemaValue` inputs) can reuse it without duplicating the body of
//! the walker.

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use std::collections::HashSet;

/// Driver-friendly walker over a typed value tree.
///
/// Implementations focus on the actual per-shape logic; the top-level
/// [`walk`] function handles ref resolution and cycle protection before
/// dispatching back into [`SchemaWalker::walk`] on the resolved body.
pub trait SchemaWalker {
    /// The result produced when a value tree is fully consumed.
    type Output;
    /// The error produced when the walker rejects a sub-tree.
    type Error;

    /// Walk one (sub-)value of a given type in the context of `graph`.
    ///
    /// Implementations may freely call back into [`walk`] (the free function
    /// in this module) when they recurse into child nodes; that keeps ref
    /// resolution centralised.
    fn walk(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> Result<Self::Output, Self::Error>;
}

/// Drive a walker over `(graph, ty, value)`.
///
/// - [`SchemaType::Ref`] hops are resolved against `graph.defs` before
///   handing the body to the walker.
/// - A reference cycle (the same [`TypeId`] re-entered while still on the
///   stack) is reported via [`WalkerError::RefCycle`] so walkers do not
///   recurse forever on ill-formed graphs.
/// - A dangling reference (the named id is not present in `graph.defs`) is
///   reported via [`WalkerError::DanglingRef`].
/// - Any error produced by the walker itself is bubbled up unchanged via
///   [`WalkerError::Walker`].
pub fn walk<W: SchemaWalker>(
    walker: &mut W,
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<W::Output, WalkerError<W::Error>> {
    let mut visited: HashSet<TypeId> = HashSet::new();
    walk_inner(walker, graph, ty, value, &mut visited)
}

fn walk_inner<W: SchemaWalker>(
    walker: &mut W,
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    visited: &mut HashSet<TypeId>,
) -> Result<W::Output, WalkerError<W::Error>> {
    resolve_ref(graph, ty, visited, |graph, body| {
        walker.walk(graph, body, value).map_err(WalkerError::Walker)
    })
}

/// Resolve any leading [`SchemaType::Ref`] hops on `ty` (cycle-aware) and
/// hand the resulting non-ref body to `f`. Errors raised by ref handling
/// surface as [`WalkerError::RefCycle`] / [`WalkerError::DanglingRef`];
/// the caller's own error type is wrapped in [`WalkerError::Walker`].
///
/// Decoders that consume an input other than `SchemaValue` (for example
/// `serde_json::Value`) use this directly so the ref-resolution path is
/// shared with the walker rather than re-implemented.
pub fn resolve_ref<F, T, E>(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visited: &mut HashSet<TypeId>,
    f: F,
) -> Result<T, WalkerError<E>>
where
    F: FnOnce(&SchemaGraph, &SchemaType) -> Result<T, WalkerError<E>>,
{
    let mut current = ty;
    let mut entered: Vec<TypeId> = Vec::new();
    let result = loop {
        match current {
            SchemaType::Ref { id, .. } => {
                if !visited.insert(id.clone()) {
                    break Err(WalkerError::RefCycle(id.clone()));
                }
                entered.push(id.clone());
                match graph.lookup(id) {
                    Some(def) => current = &def.body,
                    None => break Err(WalkerError::DanglingRef(id.clone())),
                }
            }
            other => break f(graph, other),
        }
    };
    for id in entered {
        visited.remove(&id);
    }
    result
}

/// Errors raised by the [`walk`] driver. Walker-specific errors are wrapped
/// in [`WalkerError::Walker`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkerError<E> {
    /// A ref cycle was detected during traversal.
    RefCycle(TypeId),
    /// A named ref pointed at a type id that is not declared in the graph.
    DanglingRef(TypeId),
    /// The walker rejected the sub-tree.
    Walker(E),
}

impl<E: std::fmt::Display> std::fmt::Display for WalkerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalkerError::RefCycle(id) => write!(f, "reference cycle through `{id}`"),
            WalkerError::DanglingRef(id) => write!(f, "dangling reference `{id}`"),
            WalkerError::Walker(inner) => write!(f, "{inner}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for WalkerError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WalkerError::Walker(inner) => Some(inner),
            _ => None,
        }
    }
}
