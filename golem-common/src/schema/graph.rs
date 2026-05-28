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
use serde::{Deserialize, Serialize};

/// A self-contained schema graph in the recursive in-memory form.
///
/// Anywhere a schema travels with a value (typed pair, oplog `custom`
/// payload, REST/RPC envelope, public oplog rendering), the payload owns its
/// own [`SchemaGraph`] — there is no implicit external registry that
/// consumers must look up.
///
/// Recursive references between types go through [`SchemaType::Ref`], pointing
/// at named definitions in [`SchemaGraph::defs`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Look up a named definition by its [`TypeId`].
    pub fn lookup(&self, id: &TypeId) -> Option<&SchemaTypeDef> {
        self.defs.iter().find(|d| &d.id == id)
    }
}

/// A named type definition inside a [`SchemaGraph`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaTypeDef {
    /// Stable identifier; unique within the enclosing graph.
    pub id: TypeId,
    /// Optional human-readable qualified name (display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Per-def metadata (docs, aliases, examples, deprecation, role).
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
    /// The body of this definition.
    pub body: SchemaType,
}

/// A typed value: a self-contained [`SchemaGraph`] paired with a value tree
/// built against that schema.
///
/// The pair is the only public form for typed values; there is no bare-value
/// overload of any walker / renderer / encoder API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
