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

/// Applies a compiled directional projection, validating both boundary values.
pub fn apply_projection(
    plan: &ProjectionPlan,
    value: SchemaValue,
    streams: &mut impl ProjectionStreamHandler,
) -> Result<SchemaValue, ToolCompatibilityError> {
    if let Err(errors) = validate_value(&plan.source_schema, &plan.source_schema.root, &value) {
        let error = err(
            "source",
            format!("value does not match source schema: {errors:?}"),
        );
        Evaluator { plan, streams }.discard_value(value);
        return Err(error);
    }
    let mut evaluator = Evaluator { plan, streams };
    let result = evaluator.apply(
        plan.root,
        &plan.source_schema.root,
        &plan.target_schema.root,
        value,
    )?;
    if let Err(errors) = validate_value(&plan.target_schema, &plan.target_schema.root, &result) {
        let error = err(
            "target",
            format!("projected value does not match target schema: {errors:?}"),
        );
        evaluator.discard_value(result);
        return Err(error);
    }
    Ok(result)
}

struct Evaluator<'a, H> {
    plan: &'a ProjectionPlan,
    streams: &'a mut H,
}

impl<H: ProjectionStreamHandler> Evaluator<'_, H> {
    fn apply(
        &mut self,
        node: usize,
        st: &SchemaType,
        tt: &SchemaType,
        value: SchemaValue,
    ) -> Result<SchemaValue, ToolCompatibilityError> {
        let Some(projection) = self.plan.nodes.get(node).cloned() else {
            self.discard_value(value);
            return Err(err("plan", format!("invalid projection node {node}")));
        };
        let (st, _) = resolve(&self.plan.source_schema, st);
        let (tt, _) = resolve(&self.plan.target_schema, tt);
        let bad = || {
            err(
                "value",
                format!("value shape does not match projection node {node}"),
            )
        };
        let shapes_match = match &projection {
            ProjectionNode::Identity
            | ProjectionNode::DynamicChecked
            | ProjectionNode::Recursive { .. } => true,
            ProjectionNode::Record { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Record { .. }, SchemaType::Record { .. })
                )
            }
            ProjectionNode::Tuple { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Tuple { .. }, SchemaType::Tuple { .. })
                )
            }
            ProjectionNode::List { .. } => {
                matches!((st, tt), (SchemaType::List { .. }, SchemaType::List { .. }))
            }
            ProjectionNode::FixedList { .. } => matches!(
                (st, tt),
                (SchemaType::FixedList { .. }, SchemaType::FixedList { .. })
            ),
            ProjectionNode::Map { .. } => {
                matches!((st, tt), (SchemaType::Map { .. }, SchemaType::Map { .. }))
            }
            ProjectionNode::Option { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Option { .. }, SchemaType::Option { .. })
                )
            }
            ProjectionNode::Result { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Result { .. }, SchemaType::Result { .. })
                )
            }
            ProjectionNode::Variant { .. } => matches!(
                (st, tt),
                (SchemaType::Variant { .. }, SchemaType::Variant { .. })
            ),
            ProjectionNode::Enum { .. } => {
                matches!((st, tt), (SchemaType::Enum { .. }, SchemaType::Enum { .. }))
            }
            ProjectionNode::Flags { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Flags { .. }, SchemaType::Flags { .. })
                )
            }
            ProjectionNode::Union { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Union { .. }, SchemaType::Union { .. })
                )
            }
            ProjectionNode::Stream { item } => matches!(
                (item, st, tt),
                (
                    Some(_),
                    SchemaType::Stream { inner: Some(_), .. },
                    SchemaType::Stream { inner: Some(_), .. },
                ) | (None, SchemaType::Stream { .. }, SchemaType::Stream { .. })
            ),
        };
        if !shapes_match {
            self.discard_value(value);
            return Err(err("plan", "projection node does not match its schemas"));
        }
        match projection {
            ProjectionNode::Identity => Ok(value),
            ProjectionNode::DynamicChecked => {
                if let Err(errors) = validate_value(&self.plan.target_schema, tt, &value) {
                    let error = err(
                        "value",
                        format!("dynamic value does not match target subtree: {errors:?}"),
                    );
                    self.discard_value(value);
                    return Err(error);
                }
                Ok(value)
            }
            ProjectionNode::Recursive { node } => self.apply(node, st, tt, value),
            ProjectionNode::Record { fields, discard } => {
                let (SchemaType::Record { fields: sf, .. }, SchemaType::Record { fields: tf, .. }) =
                    (st, tt)
                else {
                    self.discard_value(value);
                    return Err(bad());
                };
                let SchemaValue::Record { fields: values } = value else {
                    self.discard_value(value);
                    return Err(bad());
                };
                let mut values = values.into_iter().map(Some).collect::<Vec<_>>();
                let mut claimed = HashSet::new();
                let mut discarded = HashSet::new();
                if fields.len() != tf.len()
                    || discard.iter().any(|&index| index >= values.len())
                    || discard.iter().any(|&index| !discarded.insert(index))
                    || fields.iter().enumerate().any(|(target_index, field)| {
                        target_index >= tf.len()
                            || match field.source_index {
                                Some(index) => {
                                    index >= values.len()
                                        || field.plan.is_none()
                                        || field.default.is_some()
                                        || !claimed.insert(index)
                                }
                                None => field.plan.is_some() || field.default.is_none(),
                            }
                    })
                    || discard.iter().any(|index| claimed.contains(index))
                    || claimed.len() + discarded.len() != values.len()
                    || sf.len() != values.len()
                {
                    self.discard_remaining(values);
                    return Err(err("plan", "invalid record projection"));
                }
                for index in discard {
                    self.discard_value(values[index].take().unwrap());
                }
                let mut output = Vec::with_capacity(fields.len());
                for (target_index, field) in fields.into_iter().enumerate() {
                    let value = match (field.source_index, field.plan, field.default) {
                        (Some(source_index), Some(child), None) => match self.apply(
                            child,
                            &sf[source_index].body,
                            &tf[target_index].body,
                            values[source_index].take().unwrap(),
                        ) {
                            Ok(value) => value,
                            Err(error) => {
                                self.discard_remaining(output);
                                self.discard_remaining(values);
                                return Err(error);
                            }
                        },
                        (None, None, Some(default)) => default,
                        _ => unreachable!("record projection was validated"),
                    };
                    output.push(value);
                }
                Ok(SchemaValue::Record { fields: output })
            }
            ProjectionNode::Tuple { elements } => {
                let (
                    SchemaType::Tuple { elements: ss, .. },
                    SchemaType::Tuple { elements: ts, .. },
                    SchemaValue::Tuple { elements: values },
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                Ok(SchemaValue::Tuple {
                    elements: self.sequence(elements, ss, ts, values)?,
                })
            }
            ProjectionNode::List { item } | ProjectionNode::FixedList { item, .. } => {
                let (ss, ts, values, fixed) = match (st, tt, value) {
                    (
                        SchemaType::List { element: ss, .. },
                        SchemaType::List { element: ts, .. },
                        SchemaValue::List { elements },
                    ) => (ss.as_ref(), ts.as_ref(), elements, false),
                    (
                        SchemaType::FixedList { element: ss, .. },
                        SchemaType::FixedList { element: ts, .. },
                        SchemaValue::FixedList { elements },
                    ) => (ss.as_ref(), ts.as_ref(), elements, true),
                    _ => return Err(bad()),
                };
                let output = self.project_sequence(
                    std::iter::repeat_n(item, values.len()).collect(),
                    std::iter::repeat_n(ss, values.len()).collect(),
                    std::iter::repeat_n(ts, values.len()).collect(),
                    values,
                )?;
                Ok(if fixed {
                    SchemaValue::FixedList { elements: output }
                } else {
                    SchemaValue::List { elements: output }
                })
            }
            ProjectionNode::Map { key, value: vp } => {
                let (
                    SchemaType::Map {
                        key: sk, value: sv, ..
                    },
                    SchemaType::Map {
                        key: tk, value: tv, ..
                    },
                    SchemaValue::Map { entries },
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let mut remaining = entries.into_iter().map(Some).collect::<Vec<_>>();
                let mut output = Vec::with_capacity(remaining.len());
                for i in 0..remaining.len() {
                    let (k, v) = remaining[i].take().unwrap();
                    let projected_key = match self.apply(key, sk, tk, k) {
                        Ok(key) => key,
                        Err(error) => {
                            self.discard_value(v);
                            self.discard_map_entries(output);
                            self.discard_map_entries(remaining.into_iter().flatten());
                            return Err(error);
                        }
                    };
                    match self.apply(vp, sv, tv, v) {
                        Ok(value) => output.push((projected_key, value)),
                        Err(error) => {
                            self.discard_value(projected_key);
                            self.discard_map_entries(output);
                            self.discard_map_entries(remaining.into_iter().flatten());
                            return Err(error);
                        }
                    }
                }
                Ok(SchemaValue::Map { entries: output })
            }
            ProjectionNode::Option { some } => {
                let (
                    SchemaType::Option { inner: ss, .. },
                    SchemaType::Option { inner: ts, .. },
                    SchemaValue::Option { inner },
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                Ok(SchemaValue::Option {
                    inner: inner
                        .map(|v| self.apply(some, ss, ts, *v).map(Box::new))
                        .transpose()?,
                })
            }
            ProjectionNode::Result { ok, err: ep } => {
                let (
                    SchemaType::Result { spec: ss, .. },
                    SchemaType::Result { spec: ts, .. },
                    SchemaValue::Result(payload),
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let payload = match payload {
                    ResultValuePayload::Ok { value } => ResultValuePayload::Ok {
                        value: self.optional_payload(
                            ok,
                            ss.ok.as_deref(),
                            ts.ok.as_deref(),
                            value,
                        )?,
                    },
                    ResultValuePayload::Err { value } => ResultValuePayload::Err {
                        value: self.optional_payload(
                            ep,
                            ss.err.as_deref(),
                            ts.err.as_deref(),
                            value,
                        )?,
                    },
                };
                Ok(SchemaValue::Result(payload))
            }
            ProjectionNode::Variant { cases } => {
                let (
                    SchemaType::Variant { cases: ss, .. },
                    SchemaType::Variant { cases: ts, .. },
                    SchemaValue::Variant(payload),
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let case = cases
                    .iter()
                    .find(|c| c.source_index == payload.case as usize)
                    .cloned();
                let Some(case) = case else {
                    self.discard_remaining(payload.payload.map(|value| *value));
                    return Err(bad());
                };
                let (Some(source_case), Some(target_case)) =
                    (ss.get(case.source_index), ts.get(case.target_index))
                else {
                    self.discard_remaining(payload.payload.map(|value| *value));
                    return Err(err("plan", "invalid variant case projection"));
                };
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: case.target_index as u32,
                    payload: self.optional_payload(
                        case.payload,
                        source_case.payload.as_ref(),
                        target_case.payload.as_ref(),
                        payload.payload,
                    )?,
                }))
            }
            ProjectionNode::Union { branches } => {
                let (
                    SchemaType::Union { spec: ss, .. },
                    SchemaType::Union { spec: ts, .. },
                    SchemaValue::Union(payload),
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let case = branches.iter().find(|c| {
                    ss.branches
                        .get(c.source_index)
                        .is_some_and(|branch| branch.tag == payload.tag)
                });
                let Some(case) = case else {
                    self.discard_value(*payload.body);
                    return Err(bad());
                };
                let (Some(source_branch), Some(target_branch)) = (
                    ss.branches.get(case.source_index),
                    ts.branches.get(case.target_index),
                ) else {
                    self.discard_value(*payload.body);
                    return Err(err("plan", "invalid union branch projection"));
                };
                let Some(child) = case.payload else {
                    self.discard_value(*payload.body);
                    return Err(err("plan", "union branch has no payload plan"));
                };
                Ok(SchemaValue::Union(UnionValuePayload {
                    tag: target_branch.tag.clone(),
                    body: Box::new(self.apply(
                        child,
                        &source_branch.body,
                        &target_branch.body,
                        *payload.body,
                    )?),
                }))
            }
            ProjectionNode::Enum { cases } => match value {
                SchemaValue::Enum { case } => Ok(SchemaValue::Enum {
                    case: *cases.get(case as usize).ok_or_else(bad)? as u32,
                }),
                _ => Err(bad()),
            },
            ProjectionNode::Flags { flags } => match value {
                SchemaValue::Flags { bits } => {
                    let target_len = match tt {
                        SchemaType::Flags { flags, .. } => flags.len(),
                        _ => return Err(bad()),
                    };
                    let mut out = vec![false; target_len];
                    for (i, bit) in bits.into_iter().enumerate() {
                        if bit {
                            let Some(&target) = flags.get(i) else {
                                return Err(bad());
                            };
                            let Some(target_bit) = out.get_mut(target) else {
                                return Err(err("plan", "invalid target flag index"));
                            };
                            *target_bit = true;
                        }
                    }
                    Ok(SchemaValue::Flags { bits: out })
                }
                _ => Err(bad()),
            },
            ProjectionNode::Stream { item } => match value {
                SchemaValue::Stream(stream) => {
                    let item_plan = match (item, st, tt) {
                        (
                            Some(root),
                            SchemaType::Stream {
                                inner: Some(ss), ..
                            },
                            SchemaType::Stream {
                                inner: Some(ts), ..
                            },
                        ) => Some(ProjectionPlan {
                            source_schema: graph_for(&self.plan.source_schema, ss),
                            target_schema: graph_for(&self.plan.target_schema, ts),
                            nodes: self.plan.nodes.clone(),
                            root,
                        }),
                        (None, _, _) => {
                            return Ok(SchemaValue::Stream(stream));
                        }
                        _ => {
                            self.streams.discard_stream(stream);
                            return Err(err(
                                "plan",
                                "stream item projection does not match stream schemas",
                            ));
                        }
                    };
                    self.streams
                        .project_stream(stream, item_plan)
                        .map(SchemaValue::Stream)
                        .map_err(|e| err("stream", e))
                }
                _ => Err(bad()),
            },
        }
    }

    fn sequence(
        &mut self,
        plans: Vec<usize>,
        ss: &[SchemaType],
        ts: &[SchemaType],
        values: Vec<SchemaValue>,
    ) -> Result<Vec<SchemaValue>, ToolCompatibilityError> {
        if plans.len() != values.len() || ss.len() != values.len() || ts.len() != values.len() {
            self.discard_remaining(values);
            return Err(err("value", "tuple length does not match projection"));
        }
        self.project_sequence(plans, ss.iter().collect(), ts.iter().collect(), values)
    }
    fn project_sequence(
        &mut self,
        plans: Vec<usize>,
        ss: Vec<&SchemaType>,
        ts: Vec<&SchemaType>,
        values: Vec<SchemaValue>,
    ) -> Result<Vec<SchemaValue>, ToolCompatibilityError> {
        let mut remaining = values.into_iter().map(Some).collect::<Vec<_>>();
        let mut output = Vec::with_capacity(remaining.len());
        for i in 0..remaining.len() {
            match self.apply(plans[i], ss[i], ts[i], remaining[i].take().unwrap()) {
                Ok(value) => output.push(value),
                Err(error) => {
                    self.discard_remaining(output);
                    self.discard_remaining(remaining);
                    return Err(error);
                }
            }
        }
        Ok(output)
    }
    fn discard_remaining<T: IntoIterator<Item = V>, V: IntoRemainingValue>(&mut self, values: T) {
        for value in values {
            if let Some(value) = value.into_remaining_value() {
                self.discard_value(value);
            }
        }
    }
    fn discard_map_entries(
        &mut self,
        entries: impl IntoIterator<Item = (SchemaValue, SchemaValue)>,
    ) {
        for (key, value) in entries {
            self.discard_value(key);
            self.discard_value(value);
        }
    }
    fn optional_payload(
        &mut self,
        plan: Option<usize>,
        ss: Option<&SchemaType>,
        ts: Option<&SchemaType>,
        value: Option<Box<SchemaValue>>,
    ) -> Result<Option<Box<SchemaValue>>, ToolCompatibilityError> {
        match (plan, ss, ts, value) {
            (None, None, None, None) => Ok(None),
            (Some(p), Some(s), Some(t), Some(v)) => self.apply(p, s, t, *v).map(Box::new).map(Some),
            (_, _, _, value) => {
                self.discard_remaining(value.map(|value| *value));
                Err(err("value", "payload presence does not match projection"))
            }
        }
    }
    fn discard_value(&mut self, value: SchemaValue) {
        match value {
            SchemaValue::Stream(stream) => self.streams.discard_stream(stream),
            SchemaValue::Record { fields }
            | SchemaValue::Tuple { elements: fields }
            | SchemaValue::List { elements: fields }
            | SchemaValue::FixedList { elements: fields } => {
                for v in fields {
                    self.discard_value(v);
                }
            }
            SchemaValue::Map { entries } => {
                for (k, v) in entries {
                    self.discard_value(k);
                    self.discard_value(v);
                }
            }
            SchemaValue::Variant(v) => {
                if let Some(value) = v.payload {
                    self.discard_value(*value);
                }
            }
            SchemaValue::Option { inner: Some(value) } => {
                self.discard_value(*value);
            }
            SchemaValue::Result(
                ResultValuePayload::Ok { value } | ResultValuePayload::Err { value },
            ) => {
                if let Some(value) = value {
                    self.discard_value(*value);
                }
            }
            SchemaValue::Union(v) => self.discard_value(*v.body),
            _ => {}
        }
    }
}

trait IntoRemainingValue {
    fn into_remaining_value(self) -> Option<SchemaValue>;
}

impl IntoRemainingValue for SchemaValue {
    fn into_remaining_value(self) -> Option<SchemaValue> {
        Some(self)
    }
}

impl IntoRemainingValue for Option<SchemaValue> {
    fn into_remaining_value(self) -> Option<SchemaValue> {
        self
    }
}

/// Compiles `expected` middleware-facing inputs to `inner` inputs and `inner`
/// results/errors back to `expected`. Both descriptors are validated first.
pub fn compile_tool_compatibility(
    expected: &Tool,
    inner: &Tool,
    mode: ToolCompatibilityMode,
) -> Result<CompiledToolCompatibility, Vec<ToolCompatibilityError>> {
    let mut errors = Vec::new();
    if let Err(es) = validate_tool(expected) {
        errors.extend(es.into_iter().map(|e| err("expected", e.to_string())));
    }
    if let Err(es) = validate_tool(inner) {
        errors.extend(es.into_iter().map(|e| err("inner", e.to_string())));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    if expected.name() != inner.name() {
        return Err(vec![err("name", "tool names differ")]);
    }

    if mode == ToolCompatibilityMode::StrictEquality && !strictly_equal(expected, inner) {
        return Err(vec![err("tool", "tool descriptors are not strictly equal")]);
    }

    let expected_commands = command_paths(expected);
    let inner_commands = command_paths(inner);
    let mut commands = Vec::new();
    let mut warnings = Vec::new();
    for (path, &ei) in &expected_commands {
        let Some(&ni) = inner_commands.get(path) else {
            errors.push(err(
                format_path(path),
                "inner tool does not implement command",
            ));
            continue;
        };
        let eb = expected.commands.nodes[ei]
            .body
            .as_ref()
            .expect("body command");
        let nb = inner.commands.nodes[ni]
            .body
            .as_ref()
            .expect("body command");
        if mode == ToolCompatibilityMode::StrictEquality && eb.constraints != nb.constraints {
            errors.push(err(format_path(path), "command constraints differ"));
            continue;
        }
        if mode == ToolCompatibilityMode::StructuralSubtype
            && !nb.constraints.iter().all(|c| eb.constraints.contains(c))
        {
            errors.push(err(
                format_path(path),
                "inner command adds a constraint not guaranteed by the expected command",
            ));
            continue;
        }
        if !stream_specs_equal(&eb.stdin, &nb.stdin) {
            errors.push(err(
                format!("{}.stdin", format_path(path)),
                "standard-input stream contracts differ",
            ));
            continue;
        }
        if !stream_specs_equal(&eb.stdout, &nb.stdout) {
            errors.push(err(
                format!("{}.stdout", format_path(path)),
                "standard-output stream contracts differ",
            ));
            continue;
        }
        let input = compile_inputs(
            expected,
            ei,
            inner,
            ni,
            mode,
            path,
            &mut errors,
            &mut warnings,
        );
        let result = compile_results(expected, eb, inner, nb, mode, path, &mut errors);
        let mapped_errors = compile_errors(expected, eb, inner, nb, mode, path, &mut errors);
        if let Some(input) = input {
            commands.push(CompiledCommandCompatibility {
                expected_path: path.clone(),
                inner_path: path.clone(),
                input,
                result,
                errors: mapped_errors,
                forward_unknown_errors: true,
            });
        }
    }
    if mode == ToolCompatibilityMode::StrictEquality {
        for path in inner_commands.keys() {
            if !expected_commands.contains_key(path) {
                errors.push(err(
                    format_path(path),
                    "inner tool has an additional command",
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(CompiledToolCompatibility {
            mode,
            commands,
            warnings,
        })
    } else {
        Err(errors)
    }
}

fn stream_specs_equal(a: &Option<super::StreamSpec>, b: &Option<super::StreamSpec>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.mime == b.mime && a.required == b.required,
        _ => false,
    }
}

fn compile_inputs(
    expected: &Tool,
    ei: usize,
    inner: &Tool,
    ni: usize,
    mode: ToolCompatibilityMode,
    path: &[String],
    errors: &mut Vec<ToolCompatibilityError>,
    warnings: &mut Vec<ToolCompatibilityWarning>,
) -> Option<ProjectionPlan> {
    let ef = expected.canonical_input_fields(ei);
    let nf = inner.canonical_input_fields(ni);
    let defaults = surface_defaults(inner, ni);
    let egraph = match expected.canonical_input_record_schema(ei) {
        Ok(graph) => graph,
        Err(error) => {
            errors.push(err(
                format_path(path),
                format!("failed to build expected canonical input: {error}"),
            ));
            return None;
        }
    };
    let ngraph = match inner.canonical_input_record_schema(ni) {
        Ok(graph) => graph,
        Err(error) => {
            errors.push(err(
                format_path(path),
                format!("failed to build inner canonical input: {error}"),
            ));
            return None;
        }
    };
    if mode == ToolCompatibilityMode::StrictEquality
        && ef.iter().map(|f| &f.name).ne(nf.iter().map(|f| &f.name))
    {
        errors.push(err(format_path(path), "canonical input fields differ"));
        return None;
    }
    let mut compiler = Compiler::new(&egraph, &ngraph, mode);
    let mut fields = Vec::new();
    let mut retained = HashSet::new();
    for (target, default) in nf.iter().zip(defaults) {
        if let Some((source_index, source)) =
            ef.iter().enumerate().find(|(_, f)| f.name == target.name)
        {
            retained.insert(source_index);
            let plan = compiler.compile(
                &source.type_,
                &target.type_,
                &format!("{}.input.{}", format_path(path), target.name),
                errors,
            )?;
            fields.push(RecordFieldProjection {
                target_name: target.name.clone(),
                source_index: Some(source_index),
                plan: Some(plan),
                default: None,
            });
        } else if let Some(default) = default.or_else(|| option_none(&target.type_, &ngraph)) {
            fields.push(RecordFieldProjection {
                target_name: target.name.clone(),
                source_index: None,
                plan: None,
                default: Some(default),
            });
        } else {
            errors.push(err(
                format!("{}.input.{}", format_path(path), target.name),
                "inner-only input has no representable default",
            ));
            return None;
        }
    }
    let discard: Vec<_> = (0..ef.len()).filter(|i| !retained.contains(i)).collect();
    warnings.extend(discard.iter().map(|&index| ToolCompatibilityWarning {
        path: format_path(path),
        name: ef[index].name.clone(),
    }));
    let root = compiler.push(
        ProjectionNode::Record { fields, discard },
        &format!("{}.input", format_path(path)),
        errors,
    )?;
    Some(compiler.finish(egraph.clone(), ngraph.clone(), root))
}

fn compile_results(
    expected: &Tool,
    eb: &CommandBody,
    inner: &Tool,
    nb: &CommandBody,
    mode: ToolCompatibilityMode,
    path: &[String],
    errors: &mut Vec<ToolCompatibilityError>,
) -> Option<ProjectionPlan> {
    match (&nb.result, &eb.result) {
        (None, None) => None,
        (Some(n), Some(e)) => compile_plan(
            &inner.schema,
            &n.type_,
            &expected.schema,
            &e.type_,
            mode,
            &format!("{}.result", format_path(path)),
            errors,
        ),
        _ => {
            errors.push(err(
                format!("{}.result", format_path(path)),
                "result presence differs",
            ));
            None
        }
    }
}

fn compile_errors(
    expected: &Tool,
    eb: &CommandBody,
    inner: &Tool,
    nb: &CommandBody,
    mode: ToolCompatibilityMode,
    path: &[String],
    errors: &mut Vec<ToolCompatibilityError>,
) -> Vec<ErrorProjection> {
    if mode == ToolCompatibilityMode::StrictEquality
        && eb
            .errors
            .iter()
            .map(|e| &e.name)
            .ne(nb.errors.iter().map(|e| &e.name))
    {
        errors.push(err(
            format!("{}.errors", format_path(path)),
            "declared error cases differ",
        ));
        return Vec::new();
    }
    if matches!(
        mode,
        ToolCompatibilityMode::StructuralSubtype | ToolCompatibilityMode::Nominal
    ) {
        for expected_error in &eb.errors {
            if !nb
                .errors
                .iter()
                .any(|inner_error| inner_error.name == expected_error.name)
            {
                errors.push(err(
                    format!("{}.error.{}", format_path(path), expected_error.name),
                    "inner tool lacks an expected error case",
                ));
            }
        }
    }
    nb.errors
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let found = eb.errors.iter().enumerate().find(|(_, e)| e.name == n.name);
            let (expected_index, payload) = match found {
                Some((j, e)) => {
                    if mode != ToolCompatibilityMode::Nominal
                        && (n.kind != e.kind || n.exit_code != e.exit_code)
                    {
                        errors.push(err(
                            format!("{}.error.{}", format_path(path), n.name),
                            "error kind or exit code differs",
                        ));
                    }
                    (
                        Some(j),
                        compile_error_payload(inner, n, expected, e, mode, path, errors),
                    )
                }
                None => (None, None),
            };
            ErrorProjection {
                name: n.name.clone(),
                inner_index: i,
                expected_index,
                payload,
            }
        })
        .collect()
}

fn compile_error_payload(
    source: &Tool,
    s: &ErrorCase,
    target: &Tool,
    t: &ErrorCase,
    mode: ToolCompatibilityMode,
    path: &[String],
    errors: &mut Vec<ToolCompatibilityError>,
) -> Option<ProjectionPlan> {
    match (&s.payload, &t.payload) {
        (None, None) => None,
        (Some(a), Some(b)) => compile_plan(
            &source.schema,
            a,
            &target.schema,
            b,
            mode,
            &format!("{}.error.{}", format_path(path), s.name),
            errors,
        ),
        _ => {
            errors.push(err(
                format!("{}.error.{}", format_path(path), s.name),
                "known error payload presence differs",
            ));
            None
        }
    }
}

fn compile_plan(
    sg: &SchemaGraph,
    s: &SchemaType,
    tg: &SchemaGraph,
    t: &SchemaType,
    mode: ToolCompatibilityMode,
    path: &str,
    errors: &mut Vec<ToolCompatibilityError>,
) -> Option<ProjectionPlan> {
    if mode == ToolCompatibilityMode::Nominal {
        return Some(dynamic_plan(graph_for(sg, s), graph_for(tg, t)));
    }
    let mut c = Compiler::new(sg, tg, mode);
    let root = c.compile(s, t, path, errors)?;
    Some(c.finish(graph_for(sg, s), graph_for(tg, t), root))
}

struct Compiler<'a> {
    sg: &'a SchemaGraph,
    tg: &'a SchemaGraph,
    mode: ToolCompatibilityMode,
    nodes: Vec<ProjectionNode>,
    active: HashMap<(TypeId, TypeId), usize>,
    completed: HashMap<(TypeId, TypeId), usize>,
}
impl<'a> Compiler<'a> {
    fn new(sg: &'a SchemaGraph, tg: &'a SchemaGraph, mode: ToolCompatibilityMode) -> Self {
        Self {
            sg,
            tg,
            mode,
            nodes: Vec::new(),
            active: HashMap::new(),
            completed: HashMap::new(),
        }
    }
    fn push(
        &mut self,
        n: ProjectionNode,
        path: &str,
        errors: &mut Vec<ToolCompatibilityError>,
    ) -> Option<usize> {
        if self.nodes.len() >= MAX_PROJECTION_NODES {
            errors.push(err(path, "projection plan exceeds node limit"));
            return None;
        }
        let i = self.nodes.len();
        self.nodes.push(n);
        Some(i)
    }
    fn finish(
        self,
        source_schema: SchemaGraph,
        target_schema: SchemaGraph,
        root: usize,
    ) -> ProjectionPlan {
        ProjectionPlan {
            source_schema,
            target_schema,
            nodes: self.nodes,
            root,
        }
    }
    fn compile(
        &mut self,
        source: &SchemaType,
        target: &SchemaType,
        path: &str,
        errors: &mut Vec<ToolCompatibilityError>,
    ) -> Option<usize> {
        self.compile_at(source, target, path, errors, 0)
    }
    fn compile_at(
        &mut self,
        source: &SchemaType,
        target: &SchemaType,
        path: &str,
        errors: &mut Vec<ToolCompatibilityError>,
        depth: usize,
    ) -> Option<usize> {
        if depth >= MAX_PROJECTION_DEPTH {
            errors.push(err(path, "projection plan exceeds depth limit"));
            return None;
        }
        if self.mode == ToolCompatibilityMode::Nominal {
            return self.push(ProjectionNode::DynamicChecked, path, errors);
        }
        if self.mode == ToolCompatibilityMode::StrictEquality
            && is_equivalent_cross_graph(self.sg, source, self.tg, target)
        {
            return self.push(ProjectionNode::Identity, path, errors);
        }
        let (s, sid) = resolve(self.sg, source);
        let (t, tid) = resolve(self.tg, target);
        if let (Some(a), Some(b)) = (sid, tid) {
            let pair = (a.clone(), b.clone());
            if let Some(&node) = self.completed.get(&pair) {
                return Some(node);
            }
            if let Some(&node) = self.active.get(&(a.clone(), b.clone())) {
                return self.push(ProjectionNode::Recursive { node }, path, errors);
            }
            let placeholder = self.push(ProjectionNode::Identity, path, errors)?;
            self.active.insert(pair.clone(), placeholder);
            let body = self.compile_resolved(s, t, path, errors, depth + 1)?;
            self.nodes[placeholder] = ProjectionNode::Recursive { node: body };
            self.active.remove(&pair);
            self.completed.insert(pair, placeholder);
            return Some(placeholder);
        }
        self.compile_resolved(s, t, path, errors, depth + 1)
    }
    fn compile_resolved(
        &mut self,
        s: &SchemaType,
        t: &SchemaType,
        path: &str,
        errors: &mut Vec<ToolCompatibilityError>,
        depth: usize,
    ) -> Option<usize> {
        if self.mode == ToolCompatibilityMode::StrictEquality {
            errors.push(err(path, "types are not strictly equal"));
            return None;
        }
        let node = match (s, t) {
            (SchemaType::Record { fields: a, .. }, SchemaType::Record { fields: b, .. }) => {
                let mut fields = Vec::new();
                let mut used = HashSet::new();
                for bf in b {
                    let Some((i, af)) = a.iter().enumerate().find(|(_, f)| f.name == bf.name)
                    else {
                        errors.push(err(
                            format!("{path}.{}", bf.name),
                            "source record lacks target field",
                        ));
                        return None;
                    };
                    used.insert(i);
                    let p = self.compile_at(
                        &af.body,
                        &bf.body,
                        &format!("{path}.{}", bf.name),
                        errors,
                        depth,
                    )?;
                    fields.push(RecordFieldProjection {
                        target_name: bf.name.clone(),
                        source_index: Some(i),
                        plan: Some(p),
                        default: None,
                    });
                }
                ProjectionNode::Record {
                    fields,
                    discard: (0..a.len()).filter(|i| !used.contains(i)).collect(),
                }
            }
            (SchemaType::Tuple { elements: a, .. }, SchemaType::Tuple { elements: b, .. })
                if a.len() == b.len() =>
            {
                ProjectionNode::Tuple {
                    elements: a
                        .iter()
                        .zip(b)
                        .enumerate()
                        .map(|(i, (x, y))| {
                            self.compile_at(x, y, &format!("{path}[{i}]"), errors, depth)
                        })
                        .collect::<Option<_>>()?,
                }
            }
            (SchemaType::List { element: a, .. }, SchemaType::List { element: b, .. }) => {
                ProjectionNode::List {
                    item: self.compile_at(a, b, &format!("{path}[]"), errors, depth)?,
                }
            }
            (
                SchemaType::FixedList {
                    element: a,
                    length: x,
                    ..
                },
                SchemaType::FixedList {
                    element: b,
                    length: y,
                    ..
                },
            ) if x == y => ProjectionNode::FixedList {
                item: self.compile_at(a, b, &format!("{path}[]"), errors, depth)?,
                length: *x,
            },
            (
                SchemaType::Map {
                    key: ak, value: av, ..
                },
                SchemaType::Map {
                    key: bk, value: bv, ..
                },
            ) => ProjectionNode::Map {
                // Key conversion must preserve equality and therefore map
                // uniqueness. Numeric widening and other merely assignable
                // scalar conversions are not safe in this position.
                key: if is_equivalent_cross_graph(self.sg, ak, self.tg, bk) {
                    self.push(ProjectionNode::Identity, &format!("{path}.key"), errors)?
                } else {
                    errors.push(err(
                        format!("{path}.key"),
                        "map key types are not equivalent",
                    ));
                    return None;
                },
                value: self.compile_at(av, bv, &format!("{path}.value"), errors, depth)?,
            },
            (SchemaType::Option { inner: a, .. }, SchemaType::Option { inner: b, .. }) => {
                ProjectionNode::Option {
                    some: self.compile_at(a, b, &format!("{path}.some"), errors, depth)?,
                }
            }
            (SchemaType::Result { spec: a, .. }, SchemaType::Result { spec: b, .. }) => {
                ProjectionNode::Result {
                    ok: pair(
                        self,
                        a.ok.as_deref(),
                        b.ok.as_deref(),
                        &format!("{path}.ok"),
                        errors,
                        depth,
                    )?,
                    err: pair(
                        self,
                        a.err.as_deref(),
                        b.err.as_deref(),
                        &format!("{path}.err"),
                        errors,
                        depth,
                    )?,
                }
            }
            (SchemaType::Variant { cases: a, .. }, SchemaType::Variant { cases: b, .. }) => {
                ProjectionNode::Variant {
                    cases: cases(self, a, b, path, errors, depth)?,
                }
            }
            (SchemaType::Enum { cases: a, .. }, SchemaType::Enum { cases: b, .. })
                if a.iter().all(|n| b.contains(n)) =>
            {
                ProjectionNode::Enum {
                    cases: a
                        .iter()
                        .map(|n| b.iter().position(|x| x == n).unwrap())
                        .collect(),
                }
            }
            (SchemaType::Flags { flags: a, .. }, SchemaType::Flags { flags: b, .. })
                if a.iter().all(|n| b.contains(n)) =>
            {
                ProjectionNode::Flags {
                    flags: a
                        .iter()
                        .map(|n| b.iter().position(|x| x == n).unwrap())
                        .collect(),
                }
            }
            (SchemaType::Union { spec: a, .. }, SchemaType::Union { spec: b, .. }) => {
                let ac: Vec<_> = a
                    .branches
                    .iter()
                    .map(|x| crate::schema::schema_type::VariantCaseType {
                        name: x.tag.clone(),
                        payload: Some(x.body.clone()),
                        metadata: Default::default(),
                    })
                    .collect();
                let bc: Vec<_> = b
                    .branches
                    .iter()
                    .map(|x| crate::schema::schema_type::VariantCaseType {
                        name: x.tag.clone(),
                        payload: Some(x.body.clone()),
                        metadata: Default::default(),
                    })
                    .collect();
                ProjectionNode::Union {
                    branches: cases(self, &ac, &bc, path, errors, depth)?,
                }
            }
            (SchemaType::Stream { inner: a, .. }, SchemaType::Stream { inner: b, .. }) => {
                if matches!((a, b), (None, None))
                    || matches!((a, b), (Some(a), Some(b)) if is_equivalent_cross_graph(self.sg, a, self.tg, b))
                {
                    ProjectionNode::Identity
                } else {
                    ProjectionNode::Stream {
                        item: pair(
                            self,
                            a.as_deref(),
                            b.as_deref(),
                            &format!("{path}.item"),
                            errors,
                            depth,
                        )?,
                    }
                }
            }
            (SchemaType::Future { .. }, SchemaType::Future { .. }) => {
                errors.push(err(
                    path,
                    "future projection is unsupported at tool invocation boundaries",
                ));
                return None;
            }
            (SchemaType::Secret { spec: a, .. }, SchemaType::Secret { spec: b, .. }) if a == b => {
                ProjectionNode::Identity
            }
            (SchemaType::QuotaToken { spec: a, .. }, SchemaType::QuotaToken { spec: b, .. })
                if a == b =>
            {
                ProjectionNode::Identity
            }
            (
                SchemaType::PermissionCard { spec: a, .. },
                SchemaType::PermissionCard { spec: b, .. },
            ) if a == b => ProjectionNode::Identity,
            _ if scalar_assignable(s, t) => ProjectionNode::Identity,
            _ => {
                errors.push(err(
                    path,
                    "source type is not substitutable for target type",
                ));
                return None;
            }
        };
        self.push(node, path, errors)
    }
}

fn pair(
    c: &mut Compiler<'_>,
    a: Option<&SchemaType>,
    b: Option<&SchemaType>,
    path: &str,
    e: &mut Vec<ToolCompatibilityError>,
    depth: usize,
) -> Option<Option<usize>> {
    match (a, b) {
        (None, None) => Some(None),
        (Some(x), Some(y)) => c.compile_at(x, y, path, e, depth).map(Some),
        _ => {
            e.push(err(path, "payload presence differs"));
            None
        }
    }
}
fn cases(
    c: &mut Compiler<'_>,
    a: &[crate::schema::schema_type::VariantCaseType],
    b: &[crate::schema::schema_type::VariantCaseType],
    path: &str,
    e: &mut Vec<ToolCompatibilityError>,
    depth: usize,
) -> Option<Vec<CaseProjection>> {
    a.iter()
        .enumerate()
        .map(|(i, x)| {
            let Some((j, y)) = b.iter().enumerate().find(|(_, z)| z.name == x.name) else {
                e.push(err(
                    format!("{path}.{}", x.name),
                    "target lacks source case",
                ));
                return None;
            };
            let payload = pair(
                c,
                x.payload.as_ref(),
                y.payload.as_ref(),
                &format!("{path}.{}", x.name),
                e,
                depth,
            )?;
            Some(CaseProjection {
                name: x.name.clone(),
                source_index: i,
                target_index: j,
                payload,
            })
        })
        .collect()
}
fn scalar_assignable(s: &SchemaType, t: &SchemaType) -> bool {
    let g = SchemaGraph::anonymous(s.clone());
    is_assignable(&g, s, t)
}

fn strictly_equal(expected: &Tool, inner: &Tool) -> bool {
    canonical_tool(expected) == canonical_tool(inner)
}

fn canonical_tool(tool: &Tool) -> Option<serde_json::Value> {
    fn discover(
        ty: &SchemaType,
        defs: &HashMap<TypeId, SchemaTypeDef>,
        ids: &mut HashMap<TypeId, TypeId>,
        order: &mut Vec<TypeId>,
    ) {
        walk_refs(ty, &mut |id| {
            if !ids.contains_key(id) {
                let canonical = TypeId::new(format!("$def{}", ids.len()));
                ids.insert(id.clone(), canonical);
                order.push(id.clone());
                if let Some(def) = defs.get(id) {
                    discover(&def.body, defs, ids, order);
                }
            }
        });
    }

    let mut result = tool.clone();
    erase_ignored_tool_documentation(&mut result);
    let defs: HashMap<_, _> = result
        .schema
        .defs
        .iter()
        .map(|def| (def.id.clone(), def.clone()))
        .collect();
    let mut ids = HashMap::new();
    let mut order = Vec::new();
    for root in tool_schema_roots(&result) {
        discover(root, &defs, &mut ids, &mut order);
    }
    discover(&result.schema.root, &defs, &mut ids, &mut order);

    // Tool validation permits unused definitions, and strict equality includes
    // them. Pick disconnected graph components by their smallest rooted
    // serialization, rather than by their allocation IDs or input order.
    while ids.len() < defs.len() {
        let mut candidates = Vec::new();
        for id in defs.keys().filter(|id| !ids.contains_key(*id)) {
            let mut local_ids = ids.clone();
            let mut local_order = order.clone();
            discover(
                &SchemaType::ref_to(id.clone()),
                &defs,
                &mut local_ids,
                &mut local_order,
            );
            let mut bodies = Vec::new();
            for original in &local_order[order.len()..] {
                let mut def = defs.get(original)?.clone();
                def.id = local_ids.get(original)?.clone();
                rewrite_refs(&mut def.body, &local_ids);
                bodies.push(def);
            }
            candidates.push((serde_json::to_string(&bodies).ok()?, local_ids, local_order));
        }
        let (_, next_ids, next_order) = candidates.into_iter().min_by(|a, b| a.0.cmp(&b.0))?;
        ids = next_ids;
        order = next_order;
    }

    for root in tool_schema_roots_mut(&mut result) {
        rewrite_refs(root, &ids);
    }
    rewrite_refs(&mut result.schema.root, &ids);
    result.schema.defs = order
        .into_iter()
        .map(|original| {
            let mut def = defs.get(&original)?.clone();
            def.id = ids.get(&original)?.clone();
            rewrite_refs(&mut def.body, &ids);
            Some(def)
        })
        .collect::<Option<Vec<_>>>()?;
    serde_json::to_value(result).ok()
}

fn erase_ignored_tool_documentation(tool: &mut Tool) {
    tool.version.clear();
    for node in &mut tool.commands.nodes {
        node.doc = Default::default();
        for option in &mut node.globals.options {
            option.doc = Default::default();
        }
        for flag in &mut node.globals.flags {
            flag.doc = Default::default();
        }
        if let Some(body) = &mut node.body {
            for positional in &mut body.positionals.fixed {
                positional.doc = Default::default();
            }
            if let Some(tail) = &mut body.positionals.tail {
                tail.doc = Default::default();
            }
            for option in &mut body.options {
                option.doc = Default::default();
            }
            for flag in &mut body.flags {
                flag.doc = Default::default();
            }
            if let Some(stdin) = &mut body.stdin {
                stdin.doc = Default::default();
            }
            if let Some(stdout) = &mut body.stdout {
                stdout.doc = Default::default();
            }
            if let Some(result) = &mut body.result {
                result.doc = Default::default();
                for formatter in &mut result.formatters {
                    formatter.doc = Default::default();
                }
            }
            for error in &mut body.errors {
                error.doc = Default::default();
            }
        }
    }
    for root in tool_schema_roots_mut(tool) {
        erase_schema_documentation(root);
    }
    erase_schema_documentation(&mut tool.schema.root);
    for def in &mut tool.schema.defs {
        erase_schema_documentation(&mut def.body);
    }
}

fn erase_schema_documentation(ty: &mut SchemaType) {
    ty.metadata_mut().doc = None;
    match ty {
        SchemaType::Record { fields, .. } => fields.iter_mut().for_each(|field| {
            field.metadata.doc = None;
            erase_schema_documentation(&mut field.body);
        }),
        SchemaType::Variant { cases, .. } => cases.iter_mut().for_each(|case| {
            case.metadata.doc = None;
            if let Some(payload) = &mut case.payload {
                erase_schema_documentation(payload);
            }
        }),
        SchemaType::Tuple { elements, .. } => {
            elements.iter_mut().for_each(erase_schema_documentation)
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            erase_schema_documentation(element)
        }
        SchemaType::Map { key, value, .. } => {
            erase_schema_documentation(key);
            erase_schema_documentation(value);
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: crate::schema::schema_type::SecretSpec { inner, .. },
            ..
        } => erase_schema_documentation(inner),
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = &mut spec.ok {
                erase_schema_documentation(ok);
            }
            if let Some(err) = &mut spec.err {
                erase_schema_documentation(err);
            }
        }
        SchemaType::Union { spec, .. } => spec.branches.iter_mut().for_each(|branch| {
            branch.metadata.doc = None;
            erase_schema_documentation(&mut branch.body);
        }),
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(inner) = inner {
                erase_schema_documentation(inner);
            }
        }
        _ => {}
    }
}

fn walk_refs(ty: &SchemaType, visit: &mut impl FnMut(&TypeId)) {
    match ty {
        SchemaType::Ref { id, .. } => visit(id),
        SchemaType::Record { fields, .. } => fields.iter().for_each(|f| walk_refs(&f.body, visit)),
        SchemaType::Variant { cases, .. } => cases
            .iter()
            .filter_map(|c| c.payload.as_ref())
            .for_each(|t| walk_refs(t, visit)),
        SchemaType::Tuple { elements, .. } => elements.iter().for_each(|t| walk_refs(t, visit)),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            walk_refs(element, visit)
        }
        SchemaType::Map { key, value, .. } => {
            walk_refs(key, visit);
            walk_refs(value, visit);
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: crate::schema::schema_type::SecretSpec { inner, .. },
            ..
        } => walk_refs(inner, visit),
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &spec.ok {
                walk_refs(t, visit)
            }
            if let Some(t) = &spec.err {
                walk_refs(t, visit)
            }
        }
        SchemaType::Union { spec, .. } => {
            spec.branches.iter().for_each(|b| walk_refs(&b.body, visit))
        }
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                walk_refs(t, visit)
            }
        }
        _ => {}
    }
}

fn rewrite_refs(ty: &mut SchemaType, ids: &HashMap<TypeId, TypeId>) {
    match ty {
        SchemaType::Ref { id, .. } => {
            if let Some(new) = ids.get(id) {
                *id = new.clone();
            }
        }
        SchemaType::Record { fields, .. } => fields
            .iter_mut()
            .for_each(|f| rewrite_refs(&mut f.body, ids)),
        SchemaType::Variant { cases, .. } => cases
            .iter_mut()
            .filter_map(|c| c.payload.as_mut())
            .for_each(|t| rewrite_refs(t, ids)),
        SchemaType::Tuple { elements, .. } => {
            elements.iter_mut().for_each(|t| rewrite_refs(t, ids))
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            rewrite_refs(element, ids)
        }
        SchemaType::Map { key, value, .. } => {
            rewrite_refs(key, ids);
            rewrite_refs(value, ids);
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: crate::schema::schema_type::SecretSpec { inner, .. },
            ..
        } => rewrite_refs(inner, ids),
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &mut spec.ok {
                rewrite_refs(t, ids)
            }
            if let Some(t) = &mut spec.err {
                rewrite_refs(t, ids)
            }
        }
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter_mut()
            .for_each(|b| rewrite_refs(&mut b.body, ids)),
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                rewrite_refs(t, ids)
            }
        }
        _ => {}
    }
}

fn tool_schema_roots(tool: &Tool) -> Vec<&SchemaType> {
    let mut roots = Vec::new();
    for node in &tool.commands.nodes {
        collect_command_roots(&node.globals, node.body.as_ref(), &mut roots);
    }
    roots
}

fn tool_schema_roots_mut(tool: &mut Tool) -> Vec<&mut SchemaType> {
    let mut roots = Vec::new();
    for node in &mut tool.commands.nodes {
        collect_command_roots_mut(&mut node.globals, node.body.as_mut(), &mut roots);
    }
    roots
}

fn collect_command_roots<'a>(
    globals: &'a super::Globals,
    body: Option<&'a CommandBody>,
    roots: &mut Vec<&'a SchemaType>,
) {
    for option in &globals.options {
        roots.push(option_type(&option.shape));
    }
    if let Some(body) = body {
        roots.extend(body.positionals.fixed.iter().map(|p| &p.type_));
        if let Some(tail) = &body.positionals.tail {
            roots.push(&tail.item_type);
        }
        roots.extend(body.options.iter().map(|o| option_type(&o.shape)));
        if let Some(result) = &body.result {
            roots.push(&result.type_);
        }
        roots.extend(body.errors.iter().filter_map(|e| e.payload.as_ref()));
    }
}

fn collect_command_roots_mut<'a>(
    globals: &'a mut super::Globals,
    body: Option<&'a mut CommandBody>,
    roots: &mut Vec<&'a mut SchemaType>,
) {
    for option in &mut globals.options {
        roots.push(option_type_mut(&mut option.shape));
    }
    if let Some(body) = body {
        roots.extend(body.positionals.fixed.iter_mut().map(|p| &mut p.type_));
        if let Some(tail) = &mut body.positionals.tail {
            roots.push(&mut tail.item_type);
        }
        roots.extend(
            body.options
                .iter_mut()
                .map(|o| option_type_mut(&mut o.shape)),
        );
        if let Some(result) = &mut body.result {
            roots.push(&mut result.type_);
        }
        roots.extend(body.errors.iter_mut().filter_map(|e| e.payload.as_mut()));
    }
}

fn option_type(shape: &OptionShape) -> &SchemaType {
    match shape {
        OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => ty,
        OptionShape::RepeatableList(shape) => &shape.item_type,
        OptionShape::RepeatableMap(shape) => &shape.map_type,
    }
}

fn option_type_mut(shape: &mut OptionShape) -> &mut SchemaType {
    match shape {
        OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => ty,
        OptionShape::RepeatableList(shape) => &mut shape.item_type,
        OptionShape::RepeatableMap(shape) => &mut shape.map_type,
    }
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
fn dynamic_plan(source_schema: SchemaGraph, target_schema: SchemaGraph) -> ProjectionPlan {
    ProjectionPlan {
        source_schema,
        target_schema,
        nodes: vec![ProjectionNode::DynamicChecked],
        root: 0,
    }
}
fn option_none(t: &SchemaType, g: &SchemaGraph) -> Option<SchemaValue> {
    matches!(resolve(g, t).0, SchemaType::Option { .. })
        .then(|| SchemaValue::Option { inner: None })
}
fn surface_defaults(tool: &Tool, index: usize) -> Vec<Option<SchemaValue>> {
    tool.canonical_input_surfaces(index)
        .into_iter()
        .map(|s| match s {
            CanonicalSurfaceRef::GlobalOption { node, index } => {
                let option = &tool.commands.nodes[node].globals.options[index];
                option
                    .default
                    .clone()
                    .or_else(|| collection_default(&option.shape))
            }
            CanonicalSurfaceRef::GlobalFlag { node, index } => Some(flag_default(
                tool.commands.nodes[node].globals.flags[index].shape,
            )),
            CanonicalSurfaceRef::BodyPositional { index: i } => tool.commands.nodes[index]
                .body
                .as_ref()
                .unwrap()
                .positionals
                .fixed[i]
                .default
                .clone(),
            CanonicalSurfaceRef::BodyOption { index: i } => {
                let option = &tool.commands.nodes[index].body.as_ref().unwrap().options[i];
                option
                    .default
                    .clone()
                    .or_else(|| collection_default(&option.shape))
            }
            CanonicalSurfaceRef::BodyFlag { index: i } => Some(flag_default(
                tool.commands.nodes[index].body.as_ref().unwrap().flags[i].shape,
            )),
            CanonicalSurfaceRef::BodyTail => tool.commands.nodes[index]
                .body
                .as_ref()
                .unwrap()
                .positionals
                .tail
                .as_ref()
                .filter(|tail| tail.min == 0)
                .map(|_| SchemaValue::List { elements: vec![] }),
        })
        .collect()
}
fn collection_default(shape: &OptionShape) -> Option<SchemaValue> {
    match shape {
        OptionShape::RepeatableList(_) => Some(SchemaValue::List { elements: vec![] }),
        OptionShape::RepeatableMap(_) => Some(SchemaValue::Map { entries: vec![] }),
        OptionShape::Scalar(_) | OptionShape::OptionalScalar(_) => None,
    }
}
fn flag_default(shape: FlagShape) -> SchemaValue {
    match shape {
        FlagShape::BoolFlag(shape) => SchemaValue::Bool(shape.default),
        FlagShape::CountFlag(_) => SchemaValue::U32(0),
    }
}
fn command_paths(tool: &Tool) -> BTreeMap<Vec<String>, usize> {
    fn walk(t: &Tool, i: usize, p: &mut Vec<String>, r: &mut BTreeMap<Vec<String>, usize>) {
        let n = &t.commands.nodes[i];
        if n.body.is_some() {
            r.insert(p.clone(), i);
        }
        for c in &n.subcommands {
            let j = c.as_usize().unwrap();
            p.push(t.commands.nodes[j].name.clone());
            walk(t, j, p, r);
            p.pop();
        }
    }
    let mut r = BTreeMap::new();
    if !tool.commands.nodes.is_empty() {
        walk(tool, 0, &mut Vec::new(), &mut r)
    }
    r
}
fn format_path(p: &[String]) -> String {
    if p.is_empty() {
        "root".into()
    } else {
        p.join(".")
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
