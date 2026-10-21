// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::equality::strictly_equal;
use super::*;

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
        if !stream_specs_equal(&eb.stderr, &nb.stderr) {
            errors.push(err(
                format!("{}.stderr", format_path(path)),
                "standard-error stream contracts differ",
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
                forward_unknown_errors: false,
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

fn stream_specs_equal(
    a: &Option<super::super::StreamSpec>,
    b: &Option<super::super::StreamSpec>,
) -> bool {
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
        for inner_error in &nb.errors {
            if !eb
                .errors
                .iter()
                .any(|expected_error| expected_error.name == inner_error.name)
            {
                errors.push(err(
                    format!("{}.error.{}", format_path(path), inner_error.name),
                    "inner tool declares an error case absent from the expected tool",
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

pub(super) fn compile_plan(
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

pub(super) fn dynamic_plan(
    source_schema: SchemaGraph,
    target_schema: SchemaGraph,
) -> ProjectionPlan {
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
