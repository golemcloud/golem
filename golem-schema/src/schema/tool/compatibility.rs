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
use crate::schema::validation::subtyping::{is_assignable, is_equivalent_cross_graph};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

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
        cases: Vec<usize>,
    },
    Flags {
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

/// Compiles `expected` middleware-facing inputs to `inner` inputs and `inner`
/// results/errors back to `expected`. Both descriptors are validated first.
pub fn compile_tool_compatibility(
    expected: &Tool,
    inner: &Tool,
    mode: ToolCompatibilityMode,
) -> Result<CompiledToolCompatibility, Vec<ToolCompatibilityError>> {
    // Nominal compatibility is deliberately names-only. Do not validate or
    // inspect unrelated command and schema details in this mode.
    if mode == ToolCompatibilityMode::Nominal {
        if expected.name() != inner.name() {
            return Err(vec![err("name", "tool names differ")]);
        }
        return Ok(CompiledToolCompatibility {
            mode,
            commands: Vec::new(),
        });
    }

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

    if mode == ToolCompatibilityMode::StrictEquality && !strictly_equal(expected, inner) {
        return Err(vec![err("tool", "tool descriptors are not strictly equal")]);
    }

    let expected_commands = command_paths(expected);
    let inner_commands = command_paths(inner);
    let mut commands = Vec::new();
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
        if eb.stdin != nb.stdin {
            errors.push(err(
                format!("{}.stdin", format_path(path)),
                "standard-input stream contracts differ",
            ));
            continue;
        }
        if eb.stdout != nb.stdout {
            errors.push(err(
                format!("{}.stdout", format_path(path)),
                "standard-output stream contracts differ",
            ));
            continue;
        }
        let input = compile_inputs(expected, ei, inner, ni, mode, path, &mut errors);
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
        Ok(CompiledToolCompatibility { mode, commands })
    } else {
        Err(errors)
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
    let discard = (0..ef.len()).filter(|i| !retained.contains(i)).collect();
    let root = compiler.push(ProjectionNode::Record { fields, discard });
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
    if mode == ToolCompatibilityMode::StructuralSubtype {
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
                Some((j, e)) => (
                    Some(j),
                    compile_error_payload(inner, n, expected, e, mode, path, errors),
                ),
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
}
impl<'a> Compiler<'a> {
    fn new(sg: &'a SchemaGraph, tg: &'a SchemaGraph, mode: ToolCompatibilityMode) -> Self {
        Self {
            sg,
            tg,
            mode,
            nodes: Vec::new(),
            active: HashMap::new(),
        }
    }
    fn push(&mut self, n: ProjectionNode) -> usize {
        let i = self.nodes.len();
        self.nodes.push(n);
        i
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
        if self.mode == ToolCompatibilityMode::StrictEquality
            && is_equivalent_cross_graph(self.sg, source, self.tg, target)
        {
            return Some(self.push(ProjectionNode::Identity));
        }
        let (s, sid) = resolve(self.sg, source);
        let (t, tid) = resolve(self.tg, target);
        if let (Some(a), Some(b)) = (sid, tid) {
            if let Some(&node) = self.active.get(&(a.clone(), b.clone())) {
                return Some(self.push(ProjectionNode::Recursive { node }));
            }
            let placeholder = self.push(ProjectionNode::Identity);
            self.active.insert((a.clone(), b.clone()), placeholder);
            let body = self.compile_resolved(s, t, path, errors)?;
            self.nodes[placeholder] = ProjectionNode::Recursive { node: body };
            self.active.remove(&(a, b));
            return Some(placeholder);
        }
        self.compile_resolved(s, t, path, errors)
    }
    fn compile_resolved(
        &mut self,
        s: &SchemaType,
        t: &SchemaType,
        path: &str,
        errors: &mut Vec<ToolCompatibilityError>,
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
                    let p =
                        self.compile(&af.body, &bf.body, &format!("{path}.{}", bf.name), errors)?;
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
                        .map(|(i, (x, y))| self.compile(x, y, &format!("{path}[{i}]"), errors))
                        .collect::<Option<_>>()?,
                }
            }
            (SchemaType::List { element: a, .. }, SchemaType::List { element: b, .. }) => {
                ProjectionNode::List {
                    item: self.compile(a, b, &format!("{path}[]"), errors)?,
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
                item: self.compile(a, b, &format!("{path}[]"), errors)?,
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
                    self.push(ProjectionNode::Identity)
                } else {
                    errors.push(err(
                        format!("{path}.key"),
                        "map key types are not equivalent",
                    ));
                    return None;
                },
                value: self.compile(av, bv, &format!("{path}.value"), errors)?,
            },
            (SchemaType::Option { inner: a, .. }, SchemaType::Option { inner: b, .. }) => {
                ProjectionNode::Option {
                    some: self.compile(a, b, &format!("{path}.some"), errors)?,
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
                    )?,
                    err: pair(
                        self,
                        a.err.as_deref(),
                        b.err.as_deref(),
                        &format!("{path}.err"),
                        errors,
                    )?,
                }
            }
            (SchemaType::Variant { cases: a, .. }, SchemaType::Variant { cases: b, .. }) => {
                ProjectionNode::Variant {
                    cases: cases(self, a, b, path, errors)?,
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
                    flags: b
                        .iter()
                        .map(|n| a.iter().position(|x| x == n).unwrap_or(usize::MAX))
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
                    branches: cases(self, &ac, &bc, path, errors)?,
                }
            }
            (SchemaType::Stream { inner: a, .. }, SchemaType::Stream { inner: b, .. }) => {
                ProjectionNode::Stream {
                    item: pair(
                        self,
                        a.as_deref(),
                        b.as_deref(),
                        &format!("{path}.item"),
                        errors,
                    )?,
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
        Some(self.push(node))
    }
}

fn pair(
    c: &mut Compiler<'_>,
    a: Option<&SchemaType>,
    b: Option<&SchemaType>,
    path: &str,
    e: &mut Vec<ToolCompatibilityError>,
) -> Option<Option<usize>> {
    match (a, b) {
        (None, None) => Some(None),
        (Some(x), Some(y)) => c.compile(x, y, path, e).map(Some),
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

fn canonical_tool(tool: &Tool) -> Option<Tool> {
    fn discover(
        ty: &SchemaType,
        defs: &HashMap<TypeId, &SchemaTypeDef>,
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

    let defs: HashMap<_, _> = tool
        .schema
        .defs
        .iter()
        .map(|def| (def.id.clone(), def))
        .collect();
    let mut ids = HashMap::new();
    let mut order = Vec::new();
    for root in tool_schema_roots(tool) {
        discover(root, &defs, &mut ids, &mut order);
    }
    discover(&tool.schema.root, &defs, &mut ids, &mut order);

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
                let mut body = defs.get(original)?.body.clone();
                rewrite_refs(&mut body, &local_ids);
                bodies.push(body);
            }
            candidates.push((serde_json::to_string(&bodies).ok()?, local_ids, local_order));
        }
        let (_, next_ids, next_order) = candidates.into_iter().min_by(|a, b| a.0.cmp(&b.0))?;
        ids = next_ids;
        order = next_order;
    }

    let mut result = tool.clone();
    for root in tool_schema_roots_mut(&mut result) {
        rewrite_refs(root, &ids);
    }
    rewrite_refs(&mut result.schema.root, &ids);
    result.schema.defs = order
        .into_iter()
        .map(|original| {
            let mut def = (*defs.get(&original)?).clone();
            def.id = ids.get(&original)?.clone();
            rewrite_refs(&mut def.body, &ids);
            Some(def)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(result)
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
mod tests {
    use super::*;
    use crate::schema::metadata::MetadataEnvelope;
    use crate::schema::schema_type::NamedFieldType;
    use crate::schema::tool::{
        CommandAnnotations, CommandNode, CommandTree, ErrorKind, Globals, Positional, Positionals,
    };

    test_r::enable!();
    use test_r::test;

    fn tool(result: SchemaType) -> Tool {
        Tool {
            version: "1".into(),
            schema: SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "demo".into(),
                    aliases: vec![],
                    doc: Default::default(),
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: Some(CommandBody {
                        positionals: Positionals::default(),
                        options: vec![],
                        flags: vec![],
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        result: Some(super::super::ResultSpec {
                            type_: result,
                            doc: Default::default(),
                            formatters: vec![super::super::Formatter {
                                name: "json".into(),
                                doc: Default::default(),
                            }],
                            default_formatter: "json".into(),
                        }),
                        errors: vec![],
                        annotations: None,
                    }),
                }],
            },
        }
    }

    fn record(names: &[&str]) -> SchemaType {
        SchemaType::record(
            names
                .iter()
                .map(|name| NamedFieldType {
                    name: (*name).into(),
                    body: SchemaType::string(),
                    metadata: MetadataEnvelope::default(),
                })
                .collect(),
        )
    }

    #[test]
    fn result_record_projection_reorders_positional_values_by_name() {
        let expected = tool(record(&["first", "second"]));
        let inner = tool(record(&["second", "first", "discarded"]));
        let compiled =
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
                .unwrap();
        let plan = compiled.commands[0].result.as_ref().unwrap();
        let ProjectionNode::Record { fields, discard } = &plan.nodes[plan.root] else {
            panic!("record plan expected")
        };
        assert_eq!(
            fields.iter().map(|f| f.source_index).collect::<Vec<_>>(),
            vec![Some(1), Some(0)]
        );
        assert_eq!(discard, &vec![2]);
    }

    #[test]
    fn strict_equality_ignores_named_definition_allocation_ids() {
        let expected = tool(record(&["value"]));
        let inner = tool(record(&["value"]));
        assert!(
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StrictEquality)
                .is_ok()
        );
    }

    fn recursive_tool(first: &str, second: &str, reverse_defs: bool) -> Tool {
        let first_def = SchemaTypeDef {
            id: TypeId::new(first),
            name: Some("First".into()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "next".into(),
                body: SchemaType::option(SchemaType::ref_to(TypeId::new(second))),
                metadata: Default::default(),
            }]),
        };
        let second_def = SchemaTypeDef {
            id: TypeId::new(second),
            name: Some("Second".into()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "next".into(),
                body: SchemaType::option(SchemaType::ref_to(TypeId::new(first))),
                metadata: Default::default(),
            }]),
        };
        let mut result = tool(SchemaType::ref_to(TypeId::new(first)));
        result.schema.defs = if reverse_defs {
            vec![second_def, first_def]
        } else {
            vec![first_def, second_def]
        };
        result
    }

    #[test]
    fn strict_equality_canonicalizes_reordered_alpha_renamed_recursive_defs() {
        let expected = recursive_tool("left-a", "left-b", false);
        let inner = recursive_tool("right-a", "right-b", true);
        assert!(strictly_equal(&expected, &inner));
    }

    #[test]
    fn strict_equality_preserves_docs_and_default_literal_strings_matching_ids() {
        let mut expected = recursive_tool("literal-id", "other-id", false);
        expected.commands.nodes[0].doc.summary = "literal-id".into();
        expected.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed
            .push(Positional {
                name: "input".into(),
                doc: Default::default(),
                value_name: None,
                type_: SchemaType::string(),
                default: Some(SchemaValue::String("literal-id".into())),
                required: false,
                accepts_stdio: false,
            });

        let mut inner = recursive_tool("$def0", "$def1", false);
        inner.commands.nodes[0].doc.summary = "$def0".into();
        inner.commands.nodes[0].body.as_mut().unwrap().positionals = expected.commands.nodes[0]
            .body
            .as_ref()
            .unwrap()
            .positionals
            .clone();
        inner.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed[0]
            .default = Some(SchemaValue::String("$def0".into()));

        assert!(!strictly_equal(&expected, &inner));
        inner.commands.nodes[0].doc.summary = expected.commands.nodes[0].doc.summary.clone();
        assert!(!strictly_equal(&expected, &inner));
        inner.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed[0]
            .default = expected.commands.nodes[0]
            .body
            .as_ref()
            .unwrap()
            .positionals
            .fixed[0]
            .default
            .clone();
        inner.commands.nodes[0].doc.summary = "$def0".into();
        assert!(!strictly_equal(&expected, &inner));
    }

    #[test]
    fn strict_equality_does_not_ignore_other_descriptor_fields() {
        let expected = tool(SchemaType::string());
        let mut inner = expected.clone();
        inner.commands.nodes[0].body.as_mut().unwrap().annotations = Some(CommandAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        });
        assert!(!strictly_equal(&expected, &inner));
    }

    #[test]
    fn nominal_emits_checked_dynamic_plans() {
        let expected = tool(record(&["left"]));
        let inner = tool(record(&["right"]));
        let compiled =
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::Nominal).unwrap();
        assert!(compiled.commands.is_empty());
    }

    #[test]
    fn nominal_is_names_only_and_rejects_a_different_name() {
        let expected = tool(record(&["left"]));
        let mut unrelated = tool(SchemaType::bool());
        unrelated.version = "anything".into();
        unrelated.commands.nodes[0].body = None;
        assert!(
            compile_tool_compatibility(&expected, &unrelated, ToolCompatibilityMode::Nominal)
                .is_ok()
        );

        unrelated.commands.nodes[0].name = "other".into();
        assert!(
            compile_tool_compatibility(&expected, &unrelated, ToolCompatibilityMode::Nominal)
                .is_err()
        );
    }

    #[test]
    fn strict_equality_compares_version_and_documentation() {
        let expected = tool(SchemaType::string());
        let mut inner = expected.clone();
        inner.version = "2".into();
        assert!(
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StrictEquality)
                .is_err()
        );
        inner = expected.clone();
        inner.commands.nodes[0].doc.summary = "changed".into();
        assert!(
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StrictEquality)
                .is_err()
        );
    }

    #[test]
    fn structural_requires_expected_error_vocabulary_but_allows_additions() {
        fn error(name: &str) -> ErrorCase {
            ErrorCase {
                name: name.into(),
                doc: Default::default(),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: None,
            }
        }
        let mut expected = tool(SchemaType::string());
        expected.commands.nodes[0].body.as_mut().unwrap().errors = vec![error("expected")];
        let missing = tool(SchemaType::string());
        assert!(
            compile_tool_compatibility(
                &expected,
                &missing,
                ToolCompatibilityMode::StructuralSubtype
            )
            .is_err()
        );

        let mut additional = expected.clone();
        additional.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .errors
            .push(error("additional"));
        assert!(
            compile_tool_compatibility(
                &expected,
                &additional,
                ToolCompatibilityMode::StructuralSubtype
            )
            .is_ok()
        );
    }
}
