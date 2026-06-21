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

use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A self-contained schema graph in the recursive in-memory form.
///
/// Anywhere a schema travels with a value (typed pair, oplog `custom`
/// payload, REST/RPC envelope, public oplog rendering), the payload owns its
/// own [`SchemaGraph`] — there is no implicit external registry that
/// consumers must look up.
///
/// Recursive references between types go through [`SchemaType::Ref`], pointing
/// at named definitions in [`SchemaGraph::defs`].
///
/// ## Single-root vs multi-root carriers
///
/// The common case is single-root: one `SchemaGraph` describes one
/// payload, with `root` as the entry type and `defs` as the named-type
/// registry reachable from it.
///
/// Multi-root carriers (today, agent-layer carriers such as
/// [`crate::schema::agent::AgentTypeSchema`] and
/// [`crate::schema::agent::AgentDependencySchema`]) use the same shape
/// purely as a definition registry: many roots embedded elsewhere in the
/// carrier reference shared types in `defs`, and the `SchemaGraph::root`
/// field is a sentinel (see [`SchemaGraph::empty`]). Such carriers must
/// not be passed to root-oriented walkers/renderers as if `root` were the
/// payload root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[schema(named = "schema-graph")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct SchemaGraph {
    /// Named type definitions in this graph. The defining set is exactly the
    /// types reachable from `root` (directly or transitively) that need to be
    /// named — typically because they participate in a recursive cycle or are
    /// referenced multiple times.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defs: Vec<SchemaTypeDef>,
    /// Root schema type. Anonymous types appear inline; named types appear
    /// as [`SchemaType::Ref`].
    pub root: SchemaType,
}

impl SchemaGraph {
    /// Convenience: an anonymous root schema with no named definitions.
    pub fn anonymous(root: SchemaType) -> Self {
        Self {
            defs: Vec::new(),
            root,
        }
    }

    /// Convenience: an empty graph with no definitions and a sentinel root.
    ///
    /// Used as the initial value for multi-root carriers such as
    /// [`crate::schema::agent::AgentTypeSchema`] and
    /// [`crate::schema::agent::AgentDependencySchema`], where the agent's
    /// constructor and methods each act as their own root and only the
    /// `defs` registry is consulted. The `root` field is a placeholder
    /// (empty record) and is not consumed by agent-layer helpers.
    pub fn empty() -> Self {
        Self {
            defs: Vec::new(),
            root: SchemaType::Record {
                fields: Vec::new(),
                metadata: MetadataEnvelope::default(),
            },
        }
    }

    /// Look up a named definition by its [`TypeId`].
    pub fn lookup(&self, id: &TypeId) -> Option<&SchemaTypeDef> {
        self.defs.iter().find(|d| &d.id == id)
    }

    /// Walk through any number of [`SchemaType::Ref`] indirections and return
    /// the first non-ref body in this graph. Detects recursive cycles
    /// ([`RefResolutionError::RecursiveRef`]) and dangling references
    /// ([`RefResolutionError::DanglingRef`]).
    pub fn resolve_ref<'a>(
        &'a self,
        ty: &'a SchemaType,
    ) -> Result<&'a SchemaType, RefResolutionError> {
        let mut visiting: Vec<TypeId> = Vec::new();
        let mut current = ty;
        loop {
            match current {
                SchemaType::Ref { id, .. } => {
                    if visiting.iter().any(|x| x == id) {
                        return Err(RefResolutionError::RecursiveRef(id.clone()));
                    }
                    let def = self
                        .lookup(id)
                        .ok_or_else(|| RefResolutionError::DanglingRef(id.clone()))?;
                    visiting.push(id.clone());
                    current = &def.body;
                }
                other => return Ok(other),
            }
        }
    }
}

/// A borrowed, per-traversal accelerator for [`SchemaGraph::lookup`].
///
/// [`SchemaGraph::lookup`] is a linear scan over `defs`. When a single
/// traversal (value validation, rendering, ref resolution) resolves many
/// [`SchemaType::Ref`]s against a wide graph, those repeated scans dominate.
/// `GraphIndex` borrows a graph once and, for graphs above a small size
/// threshold, builds a one-shot hash index so each subsequent lookup is O(1).
/// Small graphs keep the plain linear scan, because building and hashing an
/// index is a net loss there (and would otherwise regress narrow inputs such
/// as a typical agent method parameter record).
///
/// It only borrows the graph and never mutates it, so [`SchemaGraph`] stays a
/// pure data/wire model (no embedded lazy cache that would travel through
/// serde / desert / protobuf / poem derives).
pub struct GraphIndex<'a> {
    defs: &'a [SchemaTypeDef],
    index: Option<HashMap<&'a str, &'a SchemaTypeDef>>,
}

impl<'a> GraphIndex<'a> {
    /// Graphs with at most this many definitions use a linear scan instead of
    /// building a hash index.
    const INDEX_THRESHOLD: usize = 8;

    /// Borrow `graph` and prepare a lookup accelerator for it.
    pub fn new(graph: &'a SchemaGraph) -> Self {
        let defs = graph.defs.as_slice();
        let index = (defs.len() > Self::INDEX_THRESHOLD).then(|| {
            let mut map = HashMap::with_capacity(defs.len());
            for def in defs {
                // First definition wins on duplicate ids, matching the
                // `defs.iter().find(...)` scan in `SchemaGraph::lookup`.
                map.entry(def.id.as_str()).or_insert(def);
            }
            map
        });
        Self { defs, index }
    }

    /// Number of named definitions in the indexed graph.
    pub fn defs_len(&self) -> usize {
        self.defs.len()
    }

    /// Look up a named definition by its [`TypeId`], with the same semantics as
    /// [`SchemaGraph::lookup`].
    pub fn lookup(&self, id: &TypeId) -> Option<&'a SchemaTypeDef> {
        match &self.index {
            Some(map) => map.get(id.as_str()).copied(),
            None => self.defs.iter().find(|d| &d.id == id),
        }
    }
}

/// Error from resolving a chain of [`SchemaType::Ref`] indirections in a
/// [`SchemaGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefResolutionError {
    /// The schema graph has a recursive cycle.
    RecursiveRef(TypeId),
    /// A [`SchemaType::Ref`] pointed at a [`TypeId`] not present in the graph.
    DanglingRef(TypeId),
}

impl std::fmt::Display for RefResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecursiveRef(id) => {
                write!(f, "schema graph contains a recursive reference: {id}")
            }
            Self::DanglingRef(id) => write!(f, "schema graph contains dangling reference: {id}"),
        }
    }
}

impl std::error::Error for RefResolutionError {}

/// A named type definition inside a [`SchemaGraph`].
///
/// The def itself does not carry metadata; metadata lives on the
/// [`SchemaType`] body so there is one source of truth for docs / aliases /
/// examples / deprecation / role per type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[schema(named = "schema-type-def")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct SchemaTypeDef {
    /// Stable identifier; unique within the enclosing graph.
    pub id: TypeId,
    /// Optional human-readable qualified name (display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The body of this definition. Its metadata envelope is the def's
    /// metadata.
    pub body: SchemaType,
}

/// A typed value: a self-contained [`SchemaGraph`] paired with a value tree
/// built against that schema.
///
/// The pair is the only public form for typed values; there is no bare-value
/// overload of any walker / renderer / encoder API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[schema(named = "typed-schema-value")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct TypedSchemaValue {
    graph: SchemaGraph,
    value: SchemaValue,
}

impl TypedSchemaValue {
    pub fn new(graph: SchemaGraph, value: SchemaValue) -> Self {
        Self { graph, value }
    }

    pub fn graph(&self) -> &SchemaGraph {
        &self.graph
    }

    pub fn value(&self) -> &SchemaValue {
        &self.value
    }

    pub fn root_type(&self) -> &SchemaType {
        &self.graph.root
    }

    pub fn into_parts(self) -> (SchemaGraph, SchemaValue) {
        (self.graph, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn def(id: &str, name: Option<&str>) -> SchemaTypeDef {
        SchemaTypeDef {
            id: TypeId::new(id),
            name: name.map(|n| n.to_string()),
            body: SchemaType::bool(),
        }
    }

    /// `GraphIndex::lookup` must agree with `SchemaGraph::lookup` for every id,
    /// on both the linear (small) and indexed (wide) branches, including the
    /// first-def-wins rule for duplicate ids.
    fn assert_index_matches_linear(graph: &SchemaGraph) {
        let index = GraphIndex::new(graph);
        // Probe every present id plus a guaranteed-absent one.
        let mut ids: Vec<TypeId> = graph.defs.iter().map(|d| d.id.clone()).collect();
        ids.push(TypeId::new("definitely-absent-id"));
        for id in &ids {
            assert_eq!(
                index.lookup(id),
                graph.lookup(id),
                "GraphIndex and SchemaGraph disagree on id {id:?}"
            );
        }
    }

    #[test]
    fn graph_index_matches_linear_small_with_duplicates() {
        // 3 defs (<= threshold => linear branch), with a duplicate id whose
        // first occurrence must win.
        let graph = SchemaGraph {
            defs: vec![
                def("a", Some("first-a")),
                def("b", Some("b")),
                def("a", Some("second-a")),
            ],
            root: SchemaType::bool(),
        };
        let index = GraphIndex::new(&graph);
        assert!(index.index.is_none(), "small graph must use linear fallback");
        assert_eq!(
            index.lookup(&TypeId::new("a")).and_then(|d| d.name.as_deref()),
            Some("first-a"),
            "first definition must win on duplicate ids"
        );
        assert_index_matches_linear(&graph);
    }

    #[test]
    fn graph_index_matches_linear_wide_with_duplicates() {
        // > threshold defs => indexed branch; include a duplicate id.
        let mut defs: Vec<SchemaTypeDef> = (0..20)
            .map(|i| def(&format!("t{i:02}"), Some(&format!("name-{i:02}"))))
            .collect();
        defs.push(def("t05", Some("duplicate-of-t05")));
        let graph = SchemaGraph {
            defs,
            root: SchemaType::bool(),
        };
        let index = GraphIndex::new(&graph);
        assert!(index.index.is_some(), "wide graph must build an index");
        assert_eq!(
            index
                .lookup(&TypeId::new("t05"))
                .and_then(|d| d.name.as_deref()),
            Some("name-05"),
            "first definition must win on duplicate ids in the indexed branch"
        );
        assert_index_matches_linear(&graph);
    }
}
