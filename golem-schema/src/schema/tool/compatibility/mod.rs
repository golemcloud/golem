// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

//! Pure compiler for the directional value projections at a monomorphic
//! middleware edge. Applying these plans, including disposal and stream relay,
//! is a runtime responsibility.

use super::canonical::CanonicalSurfaceRef;
use super::{CommandBody, ErrorCase, FlagShape, OptionShape, Tool, validation::validate_tool};
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use crate::schema::schema_value::{ResultValuePayload, UnionValuePayload, VariantValuePayload};
use crate::schema::validation::subtyping::{is_assignable, is_equivalent_cross_graph};
use crate::schema::validation::validate_value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

mod compiler;
mod equality;
mod evaluator;

pub use compiler::compile_tool_compatibility;
pub use evaluator::apply_projection;

const MAX_PROJECTION_NODES: usize = 4096;
const MAX_PROJECTION_DEPTH: usize = 128;

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[serde(rename_all = "kebab-case")]
pub enum ToolCompatibilityMode {
    StrictEquality,
    #[default]
    StructuralSubtype,
    Nominal,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct CompiledToolCompatibility {
    pub mode: ToolCompatibilityMode,
    pub commands: Vec<CompiledCommandCompatibility>,
    pub warnings: Vec<ToolCompatibilityWarning>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ToolCompatibilityWarning {
    pub path: String,
    pub name: String,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct CompiledCommandCompatibility {
    pub expected_path: Vec<String>,
    pub inner_path: Vec<String>,
    pub input: ProjectionPlan,
    pub result: Option<ProjectionPlan>,
    pub errors: Vec<ErrorProjection>,
    pub forward_unknown_errors: bool,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ProjectionPlan {
    pub source_schema: SchemaGraph,
    pub target_schema: SchemaGraph,
    pub nodes: Vec<ProjectionNode>,
    pub root: usize,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProjectionNode {
    Identity,
    DynamicChecked,
    Record {
        fields: Vec<RecordFieldProjection>,
        discard: Vec<usize>,
    },
    Tuple {
        elements: Vec<usize>,
    },
    List {
        item: usize,
    },
    FixedList {
        item: usize,
        length: u32,
    },
    Map {
        key: usize,
        value: usize,
    },
    Option {
        some: usize,
    },
    Result {
        ok: Option<usize>,
        err: Option<usize>,
    },
    Variant {
        cases: Vec<CaseProjection>,
    },
    Enum {
        /// Target case index for each source case index.
        cases: Vec<usize>,
    },
    Flags {
        /// Target flag index for each source flag index.
        flags: Vec<usize>,
    },
    Union {
        branches: Vec<CaseProjection>,
    },
    Stream {
        item: Option<usize>,
    },
    Recursive {
        node: usize,
    },
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct RecordFieldProjection {
    pub target_name: String,
    pub source_index: Option<usize>,
    pub plan: Option<usize>,
    pub default: Option<SchemaValue>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct CaseProjection {
    pub name: String,
    pub source_index: usize,
    pub target_index: usize,
    pub payload: Option<usize>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ErrorProjection {
    pub name: String,
    pub inner_index: usize,
    pub expected_index: Option<usize>,
    pub payload: Option<ProjectionPlan>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ToolCompatibilityError {
    pub path: String,
    pub message: String,
}

/// Runtime-independent hooks needed while applying a projection. Streams are
/// affine, so the evaluator transfers each retained stream to `project_stream`
/// and every stream below a discarded value to `discard_stream`. On error,
/// `project_stream` must dispose the stream it consumed.
pub trait ProjectionStreamHandler {
    fn project_stream(
        &mut self,
        stream: crate::schema::SchemaValueStream,
        item_plan: Option<ProjectionPlan>,
    ) -> Result<crate::schema::SchemaValueStream, String>;

    fn discard_stream(&mut self, stream: crate::schema::SchemaValueStream);
}

fn resolve<'a>(g: &'a SchemaGraph, t: &'a SchemaType) -> (&'a SchemaType, Option<TypeId>) {
    let mut x = t;
    let mut id = None;
    let mut seen = HashSet::new();
    while let SchemaType::Ref { id: i, .. } = x {
        if !seen.insert(i.clone()) {
            break;
        }
        id = Some(i.clone());
        match g.defs.iter().find(|def| def.id == *i) {
            Some(def) => x = &def.body,
            None => break,
        }
    }
    (x, id)
}
fn graph_for(g: &SchemaGraph, t: &SchemaType) -> SchemaGraph {
    SchemaGraph {
        defs: crate::schema::graph::reachable_defs(g, t),
        root: t.clone(),
    }
}
fn err(path: impl Into<String>, message: impl Into<String>) -> ToolCompatibilityError {
    ToolCompatibilityError {
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
