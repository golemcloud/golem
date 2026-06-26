// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::schema_graph_root;
use crate::golem_agentic::golem::tool::common as wire;
use crate::schema::validation::validate_value;
use crate::schema::wit::GraphEncoder;
use crate::schema::{SchemaGraph, SchemaType, SchemaValue, merge_agent_graphs};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

pub type Tool = wire::Tool;

#[derive(Clone, Debug)]
pub struct ExtendedToolType {
    pub version: String,
    pub commands: Vec<ExtendedCommandNode>,
}

#[derive(Clone, Debug)]
pub struct ExtendedCommandNode {
    pub name: String,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub globals: ExtendedGlobals,
    pub subcommands: Vec<i32>,
    pub body: Option<ExtendedCommandBody>,
}

#[derive(Clone, Debug, Default)]
pub struct ExtendedGlobals {
    pub options: Vec<ExtendedOptionSpec>,
    pub flags: Vec<FlagSpec>,
}

#[derive(Clone, Debug)]
pub struct ExtendedCommandBody {
    pub positionals: ExtendedPositionals,
    pub options: Vec<ExtendedOptionSpec>,
    pub flags: Vec<FlagSpec>,
    pub constraints: Vec<ExtendedConstraint>,
    pub stdin: Option<StreamSpec>,
    pub stdout: Option<StreamSpec>,
    pub result: Option<ExtendedResultSpec>,
    pub errors: Vec<ExtendedErrorCase>,
    pub annotations: Option<CommandAnnotations>,
}

#[derive(Clone, Debug, Default)]
pub struct ExtendedPositionals {
    pub fixed: Vec<ExtendedPositional>,
    pub tail: Option<ExtendedTailPositional>,
}

#[derive(Clone, Debug)]
pub struct ExtendedPositional {
    pub name: String,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub type_: SchemaGraph,
    pub default: Option<SchemaValue>,
    pub required: bool,
    pub accepts_stdio: bool,
}

#[derive(Clone, Debug)]
pub struct ExtendedTailPositional {
    pub name: String,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub item_type: SchemaGraph,
    pub min: u32,
    pub max: Option<u32>,
    pub separator: Option<String>,
    pub verbatim: bool,
    pub accepts_stdio: bool,
}

#[derive(Clone, Debug)]
pub struct ExtendedOptionSpec {
    pub long: String,
    pub short: Option<char>,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub shape: ExtendedOptionShape,
    pub default: Option<SchemaValue>,
    pub required: bool,
    pub env_var: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ExtendedOptionShape {
    Scalar(SchemaGraph),
    OptionalScalar(SchemaGraph),
    RepeatableList(ExtendedRepeatableListShape),
    RepeatableMap(ExtendedRepeatableMapShape),
}

#[derive(Clone, Debug)]
pub struct ExtendedRepeatableListShape {
    pub repetition: wire::Repetition,
    pub item_type: SchemaGraph,
}

#[derive(Clone, Debug)]
pub struct ExtendedRepeatableMapShape {
    pub repetition: wire::Repetition,
    pub map_type: SchemaGraph,
    pub duplicate_key_policy: wire::DuplicateKeyPolicy,
}

pub type FlagSpec = wire::FlagSpec;
pub type CommandAnnotations = wire::CommandAnnotations;
pub type StreamSpec = wire::StreamSpec;
pub type ToolFormatter = wire::Formatter;
pub type Doc = wire::Doc;
pub type Example = wire::Example;

#[derive(Clone, Debug)]
pub struct ExtendedResultSpec {
    pub type_: SchemaGraph,
    pub doc: Doc,
    pub formatters: Vec<ToolFormatter>,
    pub default_formatter: String,
}

#[derive(Clone, Debug)]
pub struct ExtendedErrorCase {
    pub name: String,
    pub doc: Doc,
    pub kind: wire::ErrorKind,
    pub exit_code: u8,
    pub payload: Option<SchemaGraph>,
}

#[derive(Clone, Debug)]
pub enum ExtendedRef {
    Present(String),
    ValueIs(ExtendedValueIsRef),
}

#[derive(Clone, Debug)]
pub struct ExtendedValueIsRef {
    pub name: String,
    pub value: SchemaValue,
}

#[derive(Clone, Debug)]
pub enum ExtendedConstraint {
    RequiresAll(Vec<ExtendedRef>),
    AllOrNone(Vec<ExtendedRef>),
    RequiresAny(Vec<ExtendedRef>),
    MutexGroups(Vec<ExtendedRefGroup>),
    Implies(ExtendedImpliesC),
    Forbids(ExtendedForbidsC),
}

#[derive(Clone, Debug)]
pub struct ExtendedRefGroup {
    pub refs: Vec<ExtendedRef>,
}
#[derive(Clone, Debug)]
pub struct ExtendedImpliesC {
    pub lhs_quant: wire::Quantifier,
    pub lhs: Vec<ExtendedRef>,
    pub rhs_quant: wire::Quantifier,
    pub rhs: Vec<ExtendedRef>,
}
#[derive(Clone, Debug)]
pub struct ExtendedForbidsC {
    pub lhs_quant: wire::Quantifier,
    pub lhs: Vec<ExtendedRef>,
    pub rhs: Vec<ExtendedRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolBuildError {
    EmptyCommandTree,
    CommandIndexOutOfBounds { index: i32, len: usize },
    UnreachableCommandNode(i32),
    CommandTreeCycle(i32),
    DuplicateCommandParent(i32),
    InvalidIdentifier { kind: &'static str, value: String },
    SubtreeCycle(String),
    SubtreeRootHasBody(String),
    SubtreeRootNameMismatch { expected: String, actual: String },
    SubtreeAnnotationsUnsupported(String),
    DuplicateName(String),
    DuplicateShort(char),
    UnresolvedTypeRef { position: String, id: String },
    IllFormedSchema { position: String, detail: String },
    EncodeError(String),
    DefaultTypeMismatch(String),
    ValueIsTypeMismatch(String),
    RepeatableMapTypeNotMap(String),
    UnresolvedDefaultFormatter(String),
    VerbatimWithoutSeparator(String),
    VariantInInputPosition(String),
    CommandNotFound(String),
    UnresolvedConstraintRef(String),
}

impl Display for ToolBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolBuildError::EmptyCommandTree => write!(f, "the command tree is empty"),
            ToolBuildError::CommandIndexOutOfBounds { index, len } => write!(
                f,
                "command index {index} is out of bounds (tree has {len} nodes)"
            ),
            ToolBuildError::UnreachableCommandNode(i) => {
                write!(f, "command node {i} is not reachable from the root")
            }
            ToolBuildError::CommandTreeCycle(i) => {
                write!(f, "the command tree contains a cycle at node {i}")
            }
            ToolBuildError::DuplicateCommandParent(i) => {
                write!(f, "command node {i} has more than one parent")
            }
            ToolBuildError::InvalidIdentifier { kind, value } => {
                write!(f, "invalid {kind}: {value:?}")
            }
            ToolBuildError::SubtreeCycle(s) => write!(f, "subtree cycle detected: {s}"),
            ToolBuildError::SubtreeRootHasBody(s) => write!(f, "subtree root has a body: {s}"),
            ToolBuildError::SubtreeRootNameMismatch { expected, actual } => write!(
                f,
                "subtree root name {actual:?} does not match the parent command name {expected:?}"
            ),
            ToolBuildError::SubtreeAnnotationsUnsupported(s) => write!(
                f,
                "annotations are not supported on the pure-dispatcher subtree command {s:?} (the model places command-annotations on a command body)"
            ),
            ToolBuildError::DuplicateName(s) => write!(f, "duplicate tool metadata name: {s}"),
            ToolBuildError::DuplicateShort(c) => write!(f, "duplicate short form: {c:?}"),
            ToolBuildError::UnresolvedTypeRef { position, id } => write!(
                f,
                "type reference {id:?} at {position} does not resolve within its schema graph"
            ),
            ToolBuildError::IllFormedSchema { position, detail } => {
                write!(f, "schema at {position} is not well-formed: {detail}")
            }
            ToolBuildError::EncodeError(s) => write!(f, "tool metadata encode error: {s}"),
            ToolBuildError::DefaultTypeMismatch(s) => {
                write!(f, "default value does not match schema: {s}")
            }
            ToolBuildError::ValueIsTypeMismatch(s) => {
                write!(f, "value-is literal does not match the argument type: {s}")
            }
            ToolBuildError::RepeatableMapTypeNotMap(s) => {
                write!(f, "repeatable-map option does not collect into a map: {s}")
            }
            ToolBuildError::UnresolvedDefaultFormatter(s) => {
                write!(f, "default-formatter is not declared: {s}")
            }
            ToolBuildError::VerbatimWithoutSeparator(s) => {
                write!(f, "verbatim tail positional has no separator: {s}")
            }
            ToolBuildError::VariantInInputPosition(s) => {
                write!(f, "a variant type is reachable from input position: {s}")
            }
            ToolBuildError::CommandNotFound(s) => write!(f, "command not found: {s}"),
            ToolBuildError::UnresolvedConstraintRef(s) => {
                write!(f, "constraint references an unknown argument: {s}")
            }
        }
    }
}
impl std::error::Error for ToolBuildError {}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum EffectiveCommandField {
    Option(ExtendedOptionSpec),
    Flag(FlagSpec),
}

#[derive(Clone, Debug)]
pub struct EffectiveCommandBody {
    pub globals: Vec<EffectiveCommandField>,
    pub body: Option<ExtendedCommandBody>,
}

#[derive(Clone, Debug)]
pub struct CanonicalInputField {
    pub name: String,
    pub schema: SchemaGraph,
}

impl ExtendedToolType {
    pub fn tool_name(&self) -> &str {
        self.commands.first().map(|c| c.name.as_str()).unwrap_or("")
    }

    pub fn to_tool(&self) -> Tool {
        self.try_to_tool().expect("failed to build tool metadata")
    }

    pub fn try_to_tool(&self) -> Result<Tool, ToolBuildError> {
        validate_tool(self)?;
        let graph = merge_agent_graphs(collect_schema_graphs(self))
            .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?;
        let mut encoder = GraphEncoder::new(&graph.defs)
            .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?;
        let nodes = self
            .commands
            .iter()
            .map(|n| encode_node(n, &mut encoder))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Tool {
            version: self.version.clone(),
            commands: wire::CommandTree { nodes },
            schema: encoder.finish(),
        })
    }

    pub fn effective_globals(&self, command_index: usize) -> Vec<EffectiveCommandField> {
        let path = self.path_to(command_index).unwrap_or_default();
        let mut result = Vec::new();
        for idx in path {
            let Some(node) = self.commands.get(idx) else {
                continue;
            };
            let globals = &node.globals;
            result.extend(
                globals
                    .options
                    .iter()
                    .cloned()
                    .map(EffectiveCommandField::Option),
            );
            result.extend(
                globals
                    .flags
                    .iter()
                    .cloned()
                    .map(EffectiveCommandField::Flag),
            );
        }
        result
    }

    pub fn canonical_input_fields(&self, command_index: usize) -> Vec<CanonicalInputField> {
        let mut fields = Vec::new();
        for global in self.effective_globals(command_index) {
            match global {
                EffectiveCommandField::Option(o) => fields.push(CanonicalInputField {
                    name: o.long.clone(),
                    schema: option_collected_graph(&o.shape),
                }),
                EffectiveCommandField::Flag(f) => fields.push(CanonicalInputField {
                    name: f.long.clone(),
                    schema: flag_graph(&f),
                }),
            }
        }
        if let Some(body) = self
            .commands
            .get(command_index)
            .and_then(|c| c.body.as_ref())
        {
            fields.extend(body.positionals.fixed.iter().map(|p| CanonicalInputField {
                name: p.name.clone(),
                schema: p.type_.clone(),
            }));
            if let Some(t) = &body.positionals.tail {
                fields.push(CanonicalInputField {
                    name: t.name.clone(),
                    schema: list_wrapper_graph(&t.item_type),
                });
            }
            fields.extend(body.options.iter().map(|o| CanonicalInputField {
                name: o.long.clone(),
                schema: option_collected_graph(&o.shape),
            }));
            fields.extend(body.flags.iter().map(|f| CanonicalInputField {
                name: f.long.clone(),
                schema: flag_graph(f),
            }));
        }
        fields
    }

    fn path_to(&self, command_index: usize) -> Option<Vec<usize>> {
        fn visit(
            nodes: &[ExtendedCommandNode],
            cur: usize,
            target: usize,
            path: &mut Vec<usize>,
            on_stack: &mut BTreeSet<usize>,
        ) -> bool {
            // Guards against malformed (cyclic) command trees so this helper is
            // safe to call before [`validate_tool`] proves the tree acyclic.
            if !on_stack.insert(cur) {
                return false;
            }
            path.push(cur);
            if cur == target {
                return true;
            }
            for child in &nodes[cur].subcommands {
                if let Ok(child) = usize::try_from(*child)
                    && child < nodes.len()
                    && visit(nodes, child, target, path, on_stack)
                {
                    return true;
                }
            }
            path.pop();
            on_stack.remove(&cur);
            false
        }
        let mut path = Vec::new();
        let mut on_stack = BTreeSet::new();
        if !self.commands.is_empty()
            && visit(&self.commands, 0, command_index, &mut path, &mut on_stack)
        {
            Some(path)
        } else {
            None
        }
    }
}

/// Encodes a metadata-time literal (option/positional default, `value-is`
/// literal) into its self-contained `schema-value-tree` wire form. Type
/// conformance of these literals against their referenced type node is checked
/// separately by [`validate_tool`], which has the per-command field context
/// needed to resolve `value-is` references.
pub fn encode_schema_value_default(
    value: &SchemaValue,
) -> Result<crate::schema::wit::wire::SchemaValueTree, ToolBuildError> {
    crate::schema::wit::encode_value(value).map_err(|e| ToolBuildError::EncodeError(e.to_string()))
}

fn encode_node(
    node: &ExtendedCommandNode,
    encoder: &mut GraphEncoder,
) -> Result<wire::CommandNode, ToolBuildError> {
    Ok(wire::CommandNode {
        name: node.name.clone(),
        aliases: node.aliases.clone(),
        doc: node.doc.clone(),
        globals: encode_globals(&node.globals, encoder)?,
        subcommands: node.subcommands.clone(),
        body: node
            .body
            .as_ref()
            .map(|b| encode_body(b, encoder))
            .transpose()?,
    })
}
fn encode_globals(
    g: &ExtendedGlobals,
    e: &mut GraphEncoder,
) -> Result<wire::Globals, ToolBuildError> {
    Ok(wire::Globals {
        options: g
            .options
            .iter()
            .map(|o| encode_option(o, e))
            .collect::<Result<_, _>>()?,
        flags: g.flags.clone(),
    })
}
fn encode_body(
    b: &ExtendedCommandBody,
    e: &mut GraphEncoder,
) -> Result<wire::CommandBody, ToolBuildError> {
    Ok(wire::CommandBody {
        positionals: wire::Positionals {
            fixed: b
                .positionals
                .fixed
                .iter()
                .map(|p| encode_positional(p, e))
                .collect::<Result<_, _>>()?,
            tail: b
                .positionals
                .tail
                .as_ref()
                .map(|t| encode_tail(t, e))
                .transpose()?,
        },
        options: b
            .options
            .iter()
            .map(|o| encode_option(o, e))
            .collect::<Result<_, _>>()?,
        flags: b.flags.clone(),
        constraints: b
            .constraints
            .iter()
            .map(encode_constraint)
            .collect::<Result<_, _>>()?,
        stdin: b.stdin.clone(),
        stdout: b.stdout.clone(),
        result: b.result.as_ref().map(|r| encode_result(r, e)).transpose()?,
        errors: b
            .errors
            .iter()
            .map(|x| encode_error(x, e))
            .collect::<Result<_, _>>()?,
        annotations: b.annotations,
    })
}
fn encode_positional(
    p: &ExtendedPositional,
    e: &mut GraphEncoder,
) -> Result<wire::Positional, ToolBuildError> {
    Ok(wire::Positional {
        name: p.name.clone(),
        doc: p.doc.clone(),
        value_name: p.value_name.clone(),
        type_: e
            .encode_type(&schema_graph_root(&p.type_))
            .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
        default: p
            .default
            .as_ref()
            .map(encode_schema_value_default)
            .transpose()?,
        required: p.required,
        accepts_stdio: p.accepts_stdio,
    })
}
fn encode_tail(
    t: &ExtendedTailPositional,
    e: &mut GraphEncoder,
) -> Result<wire::TailPositional, ToolBuildError> {
    Ok(wire::TailPositional {
        name: t.name.clone(),
        doc: t.doc.clone(),
        value_name: t.value_name.clone(),
        item_type: e
            .encode_type(&schema_graph_root(&t.item_type))
            .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
        min: t.min,
        max: t.max,
        separator: t.separator.clone(),
        verbatim: t.verbatim,
        accepts_stdio: t.accepts_stdio,
    })
}
fn encode_option(
    o: &ExtendedOptionSpec,
    e: &mut GraphEncoder,
) -> Result<wire::OptionSpec, ToolBuildError> {
    Ok(wire::OptionSpec {
        long: o.long.clone(),
        short: o.short,
        aliases: o.aliases.clone(),
        doc: o.doc.clone(),
        value_name: o.value_name.clone(),
        shape: encode_option_shape(&o.shape, e)?,
        default: o
            .default
            .as_ref()
            .map(encode_schema_value_default)
            .transpose()?,
        required: o.required,
        env_var: o.env_var.clone(),
    })
}
fn encode_option_shape(
    s: &ExtendedOptionShape,
    e: &mut GraphEncoder,
) -> Result<wire::OptionShape, ToolBuildError> {
    Ok(match s {
        ExtendedOptionShape::Scalar(g) => wire::OptionShape::Scalar(
            e.encode_type(&schema_graph_root(g))
                .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
        ),
        ExtendedOptionShape::OptionalScalar(g) => wire::OptionShape::OptionalScalar(
            e.encode_type(&schema_graph_root(g))
                .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
        ),
        ExtendedOptionShape::RepeatableList(r) => {
            wire::OptionShape::RepeatableList(wire::RepeatableListShape {
                repetition: r.repetition,
                item_type: e
                    .encode_type(&schema_graph_root(&r.item_type))
                    .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
            })
        }
        ExtendedOptionShape::RepeatableMap(r) => {
            wire::OptionShape::RepeatableMap(wire::RepeatableMapShape {
                repetition: r.repetition,
                map_type: e
                    .encode_type(&schema_graph_root(&r.map_type))
                    .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
                duplicate_key_policy: r.duplicate_key_policy,
            })
        }
    })
}
fn encode_result(
    r: &ExtendedResultSpec,
    e: &mut GraphEncoder,
) -> Result<wire::ResultSpec, ToolBuildError> {
    Ok(wire::ResultSpec {
        type_: e
            .encode_type(&schema_graph_root(&r.type_))
            .map_err(|e| ToolBuildError::EncodeError(e.to_string()))?,
        doc: r.doc.clone(),
        formatters: r.formatters.clone(),
        default_formatter: r.default_formatter.clone(),
    })
}
fn encode_error(
    err: &ExtendedErrorCase,
    e: &mut GraphEncoder,
) -> Result<wire::ErrorCase, ToolBuildError> {
    Ok(wire::ErrorCase {
        name: err.name.clone(),
        doc: err.doc.clone(),
        kind: err.kind,
        exit_code: err.exit_code,
        payload: err
            .payload
            .as_ref()
            .map(|g| {
                e.encode_type(&schema_graph_root(g))
                    .map_err(|e| ToolBuildError::EncodeError(e.to_string()))
            })
            .transpose()?,
    })
}
fn encode_constraint(c: &ExtendedConstraint) -> Result<wire::Constraint, ToolBuildError> {
    Ok(match c {
        ExtendedConstraint::RequiresAll(v) => wire::Constraint::RequiresAll(encode_refs(v)?),
        ExtendedConstraint::AllOrNone(v) => wire::Constraint::AllOrNone(encode_refs(v)?),
        ExtendedConstraint::RequiresAny(v) => wire::Constraint::RequiresAny(encode_refs(v)?),
        ExtendedConstraint::MutexGroups(g) => wire::Constraint::MutexGroups(
            g.iter()
                .map(|g| {
                    Ok(wire::RefGroup {
                        refs: encode_refs(&g.refs)?,
                    })
                })
                .collect::<Result<_, ToolBuildError>>()?,
        ),
        ExtendedConstraint::Implies(i) => wire::Constraint::Implies(wire::ImpliesC {
            lhs_quant: i.lhs_quant,
            lhs: encode_refs(&i.lhs)?,
            rhs_quant: i.rhs_quant,
            rhs: encode_refs(&i.rhs)?,
        }),
        ExtendedConstraint::Forbids(f) => wire::Constraint::Forbids(wire::ForbidsC {
            lhs_quant: f.lhs_quant,
            lhs: encode_refs(&f.lhs)?,
            rhs: encode_refs(&f.rhs)?,
        }),
    })
}
fn encode_refs(r: &[ExtendedRef]) -> Result<Vec<wire::Ref>, ToolBuildError> {
    r.iter()
        .map(|r| {
            Ok(match r {
                ExtendedRef::Present(n) => wire::Ref::Present(n.clone()),
                ExtendedRef::ValueIs(v) => wire::Ref::ValueIs(wire::ValueIsRef {
                    name: v.name.clone(),
                    value: encode_schema_value_default(&v.value)?,
                }),
            })
        })
        .collect()
}

fn collect_schema_graphs(tool: &ExtendedToolType) -> Vec<SchemaGraph> {
    let mut v = Vec::new();
    for c in &tool.commands {
        collect_globals(&c.globals, &mut v);
        if let Some(b) = &c.body {
            for p in &b.positionals.fixed {
                v.push(p.type_.clone());
            }
            if let Some(t) = &b.positionals.tail {
                v.push(t.item_type.clone());
            }
            for o in &b.options {
                collect_option(&o.shape, &mut v);
            }
            for r in b.result.iter() {
                v.push(r.type_.clone());
            }
            for e in &b.errors {
                if let Some(p) = &e.payload {
                    v.push(p.clone());
                }
            }
        }
    }
    v
}
fn collect_globals(g: &ExtendedGlobals, v: &mut Vec<SchemaGraph>) {
    for o in &g.options {
        collect_option(&o.shape, v);
    }
}
fn collect_option(s: &ExtendedOptionShape, v: &mut Vec<SchemaGraph>) {
    match s {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => {
            v.push(g.clone())
        }
        ExtendedOptionShape::RepeatableList(r) => v.push(r.item_type.clone()),
        ExtendedOptionShape::RepeatableMap(r) => v.push(r.map_type.clone()),
    }
}
/// The whole collected value type of an option (used to validate an option's
/// `default`): a `repeatable-list` collects into `list<item>`, a
/// `repeatable-map` into its map node; scalar/optional-scalar use the value
/// type directly. Definition graphs are preserved so refs still resolve.
fn option_collected_graph(s: &ExtendedOptionShape) -> SchemaGraph {
    match s {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g.clone(),
        ExtendedOptionShape::RepeatableList(r) => list_wrapper_graph(&r.item_type),
        ExtendedOptionShape::RepeatableMap(r) => r.map_type.clone(),
    }
}

/// The full input value type of an option (used for the "no variant in input
/// position" check). A `repeatable-list` stores its element type; a
/// `repeatable-map` stores the whole map node so both key and value are reached.
fn option_input_graph(s: &ExtendedOptionShape) -> SchemaGraph {
    match s {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g.clone(),
        ExtendedOptionShape::RepeatableList(r) => r.item_type.clone(),
        ExtendedOptionShape::RepeatableMap(r) => r.map_type.clone(),
    }
}

/// Wrap a graph's root in a `list`, preserving the original definitions so any
/// `Ref` in the element type still resolves.
fn list_wrapper_graph(item: &SchemaGraph) -> SchemaGraph {
    SchemaGraph {
        defs: item.defs.clone(),
        root: SchemaType::list(item.root.clone()),
    }
}

/// The derived input-record field type for a flag (`bool` for a bool-flag,
/// `u32` for a count-flag). Flags carry no author-supplied value type, so this
/// is used only by [`ExtendedToolType::canonical_input_fields`]; a `value-is`
/// literal against a flag is rejected rather than checked against this type.
fn flag_graph(f: &FlagSpec) -> SchemaGraph {
    let ty = match f.shape {
        wire::FlagShape::BoolFlag(_) => SchemaType::bool(),
        wire::FlagShape::CountFlag(_) => SchemaType::u32(),
    };
    SchemaGraph::anonymous(ty)
}

/// The comparand graph a `value-is` literal for an option is checked against,
/// per the WIT "any occurrence / entry equals this literal" rule: a scalar
/// option uses its value type, a `repeatable-list` its element type, and a
/// `repeatable-map` its map value type. Definition graphs are preserved.
fn value_is_comparand_graph(shape: &ExtendedOptionShape) -> SchemaGraph {
    match shape {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g.clone(),
        ExtendedOptionShape::RepeatableList(r) => r.item_type.clone(),
        ExtendedOptionShape::RepeatableMap(r) => map_value_graph(&r.map_type),
    }
}

/// The value type of a `Map` graph (resolving the root through any `Ref`s),
/// preserving definitions. Falls back to the map's own root when it is not a
/// map (the repeatable-map shape check reports that case separately).
fn map_value_graph(map: &SchemaGraph) -> SchemaGraph {
    let root = match map.resolve_ref(&map.root) {
        Ok(SchemaType::Map { value, .. }) => (**value).clone(),
        _ => map.root.clone(),
    };
    SchemaGraph {
        defs: map.defs.clone(),
        root,
    }
}

/// Returns true if the graph's root resolves (through any `Ref`s) to a `Map`.
fn resolves_to_map(map: &SchemaGraph) -> bool {
    matches!(map.resolve_ref(&map.root), Ok(SchemaType::Map { .. }))
}

/// The authored, self-contained [`SchemaGraph`] backing an option's value
/// type, regardless of how the option collects on the command line.
fn option_authored_graph(shape: &ExtendedOptionShape) -> &SchemaGraph {
    match shape {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => g,
        ExtendedOptionShape::RepeatableList(s) => &s.item_type,
        ExtendedOptionShape::RepeatableMap(s) => &s.map_type,
    }
}

/// Validate a self-contained per-argument [`SchemaGraph`] for structural
/// well-formedness via [`validate_graph`]: every embedded type (root and every
/// definition body) is well-formed, every [`SchemaType::Ref`] resolves within
/// the graph's own `defs`, and inline restrictions (numeric bounds, text/binary
/// ranges, union discriminators, ...) are valid. A dangling reference is
/// surfaced as a position-aware [`ToolBuildError::UnresolvedTypeRef`]; any other
/// well-formedness failure becomes [`ToolBuildError::IllFormedSchema`].
///
/// Closedness (no dangling refs) is also what makes per-argument validation
/// equivalent to validation against the merged tool schema: [`merge_agent_graphs`]
/// only unions defs (rejecting id collisions with conflicting bodies), so once
/// each embedded graph is well-formed and closed, resolving a ref / validating a
/// default or `value-is` literal against the local graph yields the same result
/// as against the merged graph.
fn check_graph_closed(graph: &SchemaGraph, position: &str) -> Result<(), ToolBuildError> {
    let errors = match crate::schema::validation::validate_graph(graph) {
        Ok(()) => return Ok(()),
        Err(errors) => errors,
    };
    // Report the first error deterministically (validate_graph collects in a
    // stable discovery order), preferring the precise dangling-ref variant.
    let first = errors
        .iter()
        .find(|e| matches!(e, crate::schema::validation::SchemaError::DanglingRef(_)))
        .or_else(|| errors.first());
    match first {
        Some(crate::schema::validation::SchemaError::DanglingRef(id)) => {
            Err(ToolBuildError::UnresolvedTypeRef {
                position: position.to_string(),
                id: id.to_string(),
            })
        }
        Some(other) => Err(ToolBuildError::IllFormedSchema {
            position: position.to_string(),
            detail: other.to_string(),
        }),
        None => Ok(()),
    }
}

fn validate_default(value: &SchemaValue, graph: &SchemaGraph) -> Result<(), ToolBuildError> {
    validate_value(graph, &graph.root, value)
        .map_err(|e| ToolBuildError::DefaultTypeMismatch(format!("{e:?}")))
}

/// A `value-is` literal is compatible if it is a valid value for the comparand
/// type, or — for the "any element/occurrence" relaxation — for the element
/// type of a list-shaped (optionally `option`-wrapped) comparand.
fn value_is_compatible(graph: &SchemaGraph, value: &SchemaValue) -> bool {
    if validate_value(graph, &graph.root, value).is_ok() {
        return true;
    }
    let Ok(mut peeled) = graph.resolve_ref(&graph.root) else {
        return false;
    };
    while let SchemaType::Option { inner, .. } = peeled {
        match graph.resolve_ref(inner) {
            Ok(next) => peeled = next,
            Err(_) => return false,
        }
    }
    if let SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } = peeled {
        return validate_value(graph, element, value).is_ok();
    }
    false
}

/// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`: lowercase kebab-case starting with a letter,
/// no leading/trailing or doubled dashes. Hand-rolled to avoid a `regex`
/// dependency in the guest SDK.
fn is_valid_identifier(s: &str) -> bool {
    let mut prev_dash = false;
    !s.is_empty()
        && s.chars().enumerate().all(|(i, c)| {
            let ok = if i == 0 {
                c.is_ascii_lowercase()
            } else {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
            };
            let dash_ok = !(c == '-' && prev_dash);
            prev_dash = c == '-';
            ok && dash_ok
        })
        && !s.ends_with('-')
}

fn check_identifier(kind: &'static str, value: &str) -> Result<(), ToolBuildError> {
    if is_valid_identifier(value) {
        Ok(())
    } else {
        Err(ToolBuildError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        })
    }
}

fn insert_unique(set: &mut BTreeSet<String>, name: &str) -> Result<(), ToolBuildError> {
    if set.insert(name.to_string()) {
        Ok(())
    } else {
        Err(ToolBuildError::DuplicateName(name.to_string()))
    }
}

fn insert_unique_short(set: &mut BTreeSet<char>, short: char) -> Result<(), ToolBuildError> {
    if set.insert(short) {
        Ok(())
    } else {
        Err(ToolBuildError::DuplicateShort(short))
    }
}

/// Validate a [`Tool`] against the producer-side construction invariants
/// documented in `golem:tool/common`. Mirrors the canonical host-side
/// validator (`golem-common`'s `validate_tool`), adapted to the SDK's
/// per-argument [`SchemaGraph`] representation. Type/value well-formedness of
/// embedded schemas is delegated to [`validate_value`]; each embedded graph is
/// validated for structural well-formedness — including dangling
/// [`SchemaType::Ref`]s and ill-formed inline restrictions (e.g. numeric
/// `min > max`) — by [`check_graph_closed`] (the closedness that also makes
/// per-argument validation equivalent to validating against the merged tool
/// schema). Cross-graph id collisions with conflicting bodies surface later
/// during graph merge in [`ExtendedToolType::try_to_tool`].
///
/// Runs the structural command-tree check first so the subsequent recursive
/// traversal can index the tree without bounds/cycle hazards.
pub fn validate_tool(tool: &ExtendedToolType) -> Result<(), ToolBuildError> {
    check_command_tree_structure(tool)?;
    visit_command(tool, 0, &[])
}

/// Structural integrity of the command tree: non-empty, every subcommand index
/// in bounds, acyclic, single-rooted tree (no shared subcommands), all nodes
/// reachable from the root.
fn check_command_tree_structure(tool: &ExtendedToolType) -> Result<(), ToolBuildError> {
    let len = tool.commands.len();
    if len == 0 {
        return Err(ToolBuildError::EmptyCommandTree);
    }
    for node in &tool.commands {
        for sub in &node.subcommands {
            if *sub < 0 || (*sub as usize) >= len {
                return Err(ToolBuildError::CommandIndexOutOfBounds { index: *sub, len });
            }
        }
    }
    let mut visited = vec![false; len];
    let mut on_stack = vec![false; len];
    dfs_command_tree(tool, 0, &mut visited, &mut on_stack)?;
    for (i, seen) in visited.iter().enumerate() {
        if !seen {
            return Err(ToolBuildError::UnreachableCommandNode(i as i32));
        }
    }
    Ok(())
}

fn dfs_command_tree(
    tool: &ExtendedToolType,
    idx: usize,
    visited: &mut [bool],
    on_stack: &mut [bool],
) -> Result<(), ToolBuildError> {
    if on_stack[idx] {
        return Err(ToolBuildError::CommandTreeCycle(idx as i32));
    }
    if visited[idx] {
        return Err(ToolBuildError::DuplicateCommandParent(idx as i32));
    }
    visited[idx] = true;
    on_stack[idx] = true;
    for sub in &tool.commands[idx].subcommands {
        // Bounds were validated in check_command_tree_structure.
        dfs_command_tree(tool, *sub as usize, visited, on_stack)?;
    }
    on_stack[idx] = false;
    Ok(())
}

/// Recursive, scope-aware traversal mirroring the canonical validator.
/// `ancestor_globals` are the globals of strict ancestors (root excluded only
/// at the root call); the current node's own globals are appended to form the
/// in-scope set for its body and children.
fn visit_command(
    tool: &ExtendedToolType,
    index: usize,
    ancestor_globals: &[&ExtendedGlobals],
) -> Result<(), ToolBuildError> {
    let node = &tool.commands[index];
    check_identifier("command name", &node.name)?;
    for alias in &node.aliases {
        check_identifier("command alias", alias)?;
    }
    check_globals_decls(&node.globals)?;
    check_global_scope_uniqueness(ancestor_globals, &node.globals)?;

    let mut in_scope: Vec<&ExtendedGlobals> = ancestor_globals.to_vec();
    in_scope.push(&node.globals);

    if let Some(body) = &node.body {
        check_body(body, &in_scope)?;
    }
    check_subcommand_uniqueness(tool, node)?;

    for sub in &node.subcommands {
        visit_command(tool, *sub as usize, &in_scope)?;
    }
    Ok(())
}

/// Identifier, repeatable-map, default, and variant-in-input checks for the
/// declarations within one command's `globals` (uniqueness is handled by
/// [`check_global_scope_uniqueness`]).
fn check_globals_decls(globals: &ExtendedGlobals) -> Result<(), ToolBuildError> {
    for opt in &globals.options {
        check_option_decl(opt)?;
    }
    for flag in &globals.flags {
        check_flag_identifiers(flag)?;
    }
    Ok(())
}

fn check_option_decl(opt: &ExtendedOptionSpec) -> Result<(), ToolBuildError> {
    check_identifier("option long name", &opt.long)?;
    for alias in &opt.aliases {
        check_identifier("option alias", alias)?;
    }
    check_graph_closed(
        option_authored_graph(&opt.shape),
        &format!("option --{}", opt.long),
    )?;
    if let ExtendedOptionShape::RepeatableMap(shape) = &opt.shape
        && !resolves_to_map(&shape.map_type)
    {
        return Err(ToolBuildError::RepeatableMapTypeNotMap(opt.long.clone()));
    }
    if let Some(default) = &opt.default {
        validate_default(default, &option_collected_graph(&opt.shape))?;
    }
    let input = option_input_graph(&opt.shape);
    if graph_reaches_variant(&input) {
        return Err(ToolBuildError::VariantInInputPosition(opt.long.clone()));
    }
    Ok(())
}

fn check_flag_identifiers(flag: &FlagSpec) -> Result<(), ToolBuildError> {
    check_identifier("flag long name", &flag.long)?;
    for alias in &flag.aliases {
        check_identifier("flag alias", alias)?;
    }
    Ok(())
}

/// The current command's own globals must be unique among themselves and
/// against every ancestor global (long names, aliases, and short forms).
fn check_global_scope_uniqueness(
    ancestors: &[&ExtendedGlobals],
    own: &ExtendedGlobals,
) -> Result<(), ToolBuildError> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut shorts: BTreeSet<char> = BTreeSet::new();
    for globals in ancestors {
        seed_global_tokens(globals, &mut names, &mut shorts);
    }
    for opt in &own.options {
        insert_unique(&mut names, &opt.long)?;
        for alias in &opt.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = opt.short {
            insert_unique_short(&mut shorts, short)?;
        }
    }
    for flag in &own.flags {
        insert_unique(&mut names, &flag.long)?;
        for alias in &flag.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = flag.short {
            insert_unique_short(&mut shorts, short)?;
        }
    }
    Ok(())
}

fn seed_global_tokens(
    globals: &ExtendedGlobals,
    names: &mut BTreeSet<String>,
    shorts: &mut BTreeSet<char>,
) {
    for opt in &globals.options {
        names.insert(opt.long.clone());
        names.extend(opt.aliases.iter().cloned());
        if let Some(short) = opt.short {
            shorts.insert(short);
        }
    }
    for flag in &globals.flags {
        names.insert(flag.long.clone());
        names.extend(flag.aliases.iter().cloned());
        if let Some(short) = flag.short {
            shorts.insert(short);
        }
    }
}

/// Per-name comparand graph for `value-is` resolution (only value-typed names).
#[derive(Default)]
struct NameScope {
    names: BTreeSet<String>,
    typed: BTreeMap<String, SchemaGraph>,
}

fn check_body(
    body: &ExtendedCommandBody,
    in_scope: &[&ExtendedGlobals],
) -> Result<(), ToolBuildError> {
    // In-scope global tokens (for uniqueness) and resolution scope (for refs).
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut shorts: BTreeSet<char> = BTreeSet::new();
    let mut scope = NameScope::default();
    for globals in in_scope {
        seed_global_tokens(globals, &mut names, &mut shorts);
        for opt in &globals.options {
            register_option_scope(&mut scope, opt);
        }
        for flag in &globals.flags {
            scope.names.insert(flag.long.clone());
            scope.names.extend(flag.aliases.iter().cloned());
        }
    }

    for opt in &body.options {
        check_option_decl(opt)?;
        insert_unique(&mut names, &opt.long)?;
        for alias in &opt.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = opt.short {
            insert_unique_short(&mut shorts, short)?;
        }
        register_option_scope(&mut scope, opt);
    }

    for flag in &body.flags {
        check_flag_identifiers(flag)?;
        insert_unique(&mut names, &flag.long)?;
        for alias in &flag.aliases {
            insert_unique(&mut names, alias)?;
        }
        if let Some(short) = flag.short {
            insert_unique_short(&mut shorts, short)?;
        }
        scope.names.insert(flag.long.clone());
        scope.names.extend(flag.aliases.iter().cloned());
    }

    for positional in &body.positionals.fixed {
        check_identifier("positional name", &positional.name)?;
        check_graph_closed(
            &positional.type_,
            &format!("positional {}", positional.name),
        )?;
        insert_unique(&mut names, &positional.name)?;
        scope.names.insert(positional.name.clone());
        scope
            .typed
            .insert(positional.name.clone(), positional.type_.clone());
        if let Some(default) = &positional.default {
            validate_default(default, &positional.type_)?;
        }
        if graph_reaches_variant(&positional.type_) {
            return Err(ToolBuildError::VariantInInputPosition(
                positional.name.clone(),
            ));
        }
    }

    if let Some(tail) = &body.positionals.tail {
        check_identifier("positional name", &tail.name)?;
        check_graph_closed(&tail.item_type, &format!("tail {}", tail.name))?;
        insert_unique(&mut names, &tail.name)?;
        scope.names.insert(tail.name.clone());
        // A tail positional is list-like; a value-is literal matches an item.
        scope
            .typed
            .insert(tail.name.clone(), tail.item_type.clone());
        if graph_reaches_variant(&tail.item_type) {
            return Err(ToolBuildError::VariantInInputPosition(tail.name.clone()));
        }
        if tail.verbatim && tail.separator.is_none() {
            return Err(ToolBuildError::VerbatimWithoutSeparator(tail.name.clone()));
        }
    }

    for constraint in &body.constraints {
        check_constraint(constraint, &scope)?;
    }

    if let Some(result) = &body.result {
        check_graph_closed(&result.type_, "result")?;
        for formatter in &result.formatters {
            check_identifier("formatter name", &formatter.name)?;
        }
        if !result
            .formatters
            .iter()
            .any(|f| f.name == result.default_formatter)
        {
            return Err(ToolBuildError::UnresolvedDefaultFormatter(
                result.default_formatter.clone(),
            ));
        }
    }

    for error_case in &body.errors {
        check_identifier("error-case name", &error_case.name)?;
        if let Some(payload) = &error_case.payload {
            check_graph_closed(payload, &format!("error {}", error_case.name))?;
        }
    }

    Ok(())
}

fn register_option_scope(scope: &mut NameScope, opt: &ExtendedOptionSpec) {
    scope.names.insert(opt.long.clone());
    scope.names.extend(opt.aliases.iter().cloned());
    let comparand = value_is_comparand_graph(&opt.shape);
    scope.typed.insert(opt.long.clone(), comparand.clone());
    for alias in &opt.aliases {
        scope.typed.insert(alias.clone(), comparand.clone());
    }
}

fn check_subcommand_uniqueness(
    tool: &ExtendedToolType,
    node: &ExtendedCommandNode,
) -> Result<(), ToolBuildError> {
    let mut seen = BTreeSet::new();
    for sub in &node.subcommands {
        let child = &tool.commands[*sub as usize];
        insert_unique(&mut seen, &child.name)?;
        for alias in &child.aliases {
            insert_unique(&mut seen, alias)?;
        }
    }
    Ok(())
}

fn check_constraint(c: &ExtendedConstraint, scope: &NameScope) -> Result<(), ToolBuildError> {
    match c {
        ExtendedConstraint::RequiresAll(v)
        | ExtendedConstraint::AllOrNone(v)
        | ExtendedConstraint::RequiresAny(v) => check_refs(v, scope),
        ExtendedConstraint::MutexGroups(groups) => {
            for g in groups {
                check_refs(&g.refs, scope)?;
            }
            Ok(())
        }
        ExtendedConstraint::Implies(i) => {
            check_refs(&i.lhs, scope)?;
            check_refs(&i.rhs, scope)
        }
        ExtendedConstraint::Forbids(f) => {
            check_refs(&f.lhs, scope)?;
            check_refs(&f.rhs, scope)
        }
    }
}

fn check_refs(refs: &[ExtendedRef], scope: &NameScope) -> Result<(), ToolBuildError> {
    for r in refs {
        match r {
            ExtendedRef::Present(name) => {
                if !scope.names.contains(name) {
                    return Err(ToolBuildError::UnresolvedConstraintRef(name.clone()));
                }
            }
            ExtendedRef::ValueIs(v) => {
                if !scope.names.contains(&v.name) {
                    return Err(ToolBuildError::UnresolvedConstraintRef(v.name.clone()));
                }
                // A name with no value type (a flag) cannot carry a value-is.
                let graph = scope
                    .typed
                    .get(&v.name)
                    .ok_or_else(|| ToolBuildError::ValueIsTypeMismatch(v.name.clone()))?;
                if !value_is_compatible(graph, &v.value) {
                    return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone()));
                }
            }
        }
    }
    Ok(())
}

fn graph_reaches_variant(graph: &SchemaGraph) -> bool {
    let mut visited = BTreeSet::new();
    type_reaches_variant(graph, &graph.root, &mut visited)
}

/// Returns true if `ty` (resolving named references against `graph`) reaches a
/// [`SchemaType::Variant`]; `visited` guards recursive graphs.
fn type_reaches_variant(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visited: &mut BTreeSet<String>,
) -> bool {
    if let SchemaType::Ref { id, .. } = ty
        && !visited.insert(id.to_string())
    {
        return false;
    }
    let Ok(resolved) = graph.resolve_ref(ty) else {
        return false;
    };
    match resolved {
        SchemaType::Variant { .. } => true,
        SchemaType::Record { fields, .. } => fields
            .iter()
            .any(|f| type_reaches_variant(graph, &f.body, visited)),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            type_reaches_variant(graph, element, visited)
        }
        SchemaType::Option { inner, .. } => type_reaches_variant(graph, inner, visited),
        SchemaType::Map { key, value, .. } => {
            type_reaches_variant(graph, key, visited) || type_reaches_variant(graph, value, visited)
        }
        SchemaType::Tuple { elements, .. } => elements
            .iter()
            .any(|e| type_reaches_variant(graph, e, visited)),
        SchemaType::Result { spec, .. } => {
            spec.ok
                .as_deref()
                .is_some_and(|t| type_reaches_variant(graph, t, visited))
                || spec
                    .err
                    .as_deref()
                    .is_some_and(|t| type_reaches_variant(graph, t, visited))
        }
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter()
            .any(|b| type_reaches_variant(graph, &b.body, visited)),
        SchemaType::Future { inner: Some(t), .. } | SchemaType::Stream { inner: Some(t), .. } => {
            type_reaches_variant(graph, t, visited)
        }
        _ => false,
    }
}

/// Resolve a command path (names or aliases) to a command-tree index using
/// checked lookups, so a malformed tree cannot panic.
fn resolve_command_path(
    tool: &ExtendedToolType,
    command_path: &[String],
) -> Result<usize, ToolBuildError> {
    let mut idx = 0usize;
    if tool.commands.is_empty() {
        return Err(ToolBuildError::EmptyCommandTree);
    }
    for part in command_path {
        let node = tool
            .commands
            .get(idx)
            .ok_or_else(|| ToolBuildError::CommandNotFound(part.clone()))?;
        let next = node.subcommands.iter().find_map(|i| {
            let child_idx = usize::try_from(*i).ok()?;
            let child = tool.commands.get(child_idx)?;
            if &child.name == part || child.aliases.iter().any(|a| a == part) {
                Some(child_idx)
            } else {
                None
            }
        });
        idx = next.ok_or_else(|| ToolBuildError::CommandNotFound(part.clone()))?;
    }
    Ok(idx)
}

/// Render help text for a command node addressed by `command_path` (empty path
/// = root). Lists inherited globals, the body's positionals/options/flags, and
/// subcommands.
pub fn render_help(
    tool: &ExtendedToolType,
    command_path: &[String],
) -> Result<String, ToolBuildError> {
    let idx = resolve_command_path(tool, command_path)?;
    let n = tool
        .commands
        .get(idx)
        .ok_or(ToolBuildError::EmptyCommandTree)?;
    let mut out = format!(
        "Usage: {}\n\n{}\n{}\n",
        n.name, n.doc.summary, n.doc.description
    );
    let globals = tool.effective_globals(idx);
    if !globals.is_empty() {
        out.push_str("\nGlobals:\n");
        for g in globals {
            match g {
                EffectiveCommandField::Option(o) => {
                    out.push_str(&format!("  --{}\t{}\n", o.long, o.doc.summary))
                }
                EffectiveCommandField::Flag(f) => {
                    out.push_str(&format!("  --{}\t{}\n", f.long, f.doc.summary))
                }
            }
        }
    }
    if let Some(b) = &n.body {
        if !b.positionals.fixed.is_empty() {
            out.push_str("\nPositionals:\n");
            for p in &b.positionals.fixed {
                out.push_str(&format!("  {}\t{}\n", p.name, p.doc.summary));
            }
        }
        if let Some(t) = &b.positionals.tail {
            out.push_str("\nTail:\n");
            out.push_str(&format!("  {}...\t{}\n", t.name, t.doc.summary));
        }
        if !b.options.is_empty() {
            out.push_str("\nOptions:\n");
            for o in &b.options {
                out.push_str(&format!("  --{}\t{}\n", o.long, o.doc.summary));
            }
        }
        if !b.flags.is_empty() {
            out.push_str("\nFlags:\n");
            for f in &b.flags {
                out.push_str(&format!("  --{}\t{}\n", f.long, f.doc.summary));
            }
        }
    }
    if !n.subcommands.is_empty() {
        out.push_str("\nSubcommands:\n");
        for i in &n.subcommands {
            if let Some(c) = usize::try_from(*i).ok().and_then(|j| tool.commands.get(j)) {
                out.push_str(&format!("  {}\t{}\n", c.name, c.doc.summary));
            }
        }
    }
    Ok(out)
}

/// Render help text for a single argument of the command addressed by
/// `command_path`. Searches inherited globals, then the body's positionals,
/// tail, options, and flags (in canonical order), matching the long name or an
/// alias. Returns [`ToolBuildError::CommandNotFound`] if no such argument
/// exists on that command.
pub fn render_argument_help(
    tool: &ExtendedToolType,
    command_path: &[String],
    arg_name: &str,
) -> Result<String, ToolBuildError> {
    let idx = resolve_command_path(tool, command_path)?;

    for g in tool.effective_globals(idx) {
        match g {
            EffectiveCommandField::Option(o)
                if o.long == arg_name || o.aliases.iter().any(|a| a == arg_name) =>
            {
                return Ok(render_option_help(&o, true));
            }
            EffectiveCommandField::Flag(f)
                if f.long == arg_name || f.aliases.iter().any(|a| a == arg_name) =>
            {
                return Ok(render_flag_help(&f, true));
            }
            _ => {}
        }
    }

    if let Some(body) = tool.commands.get(idx).and_then(|c| c.body.as_ref()) {
        for p in &body.positionals.fixed {
            if p.name == arg_name {
                return Ok(format!(
                    "{} (positional{})\n{}\n{}\n",
                    p.name,
                    if p.required { ", required" } else { "" },
                    p.doc.summary,
                    p.doc.description
                ));
            }
        }
        if let Some(t) = &body.positionals.tail
            && t.name == arg_name
        {
            return Ok(format!(
                "{}... (tail positional)\n{}\n{}\n",
                t.name, t.doc.summary, t.doc.description
            ));
        }
        for o in &body.options {
            if o.long == arg_name || o.aliases.iter().any(|a| a == arg_name) {
                return Ok(render_option_help(o, false));
            }
        }
        for f in &body.flags {
            if f.long == arg_name || f.aliases.iter().any(|a| a == arg_name) {
                return Ok(render_flag_help(f, false));
            }
        }
    }

    Err(ToolBuildError::CommandNotFound(arg_name.to_string()))
}

fn render_option_help(o: &ExtendedOptionSpec, global: bool) -> String {
    format!(
        "--{} (option{})\n{}\n{}\n",
        o.long,
        if global { ", global" } else { "" },
        o.doc.summary,
        o.doc.description
    )
}

fn render_flag_help(f: &FlagSpec, global: bool) -> String {
    format!(
        "--{} (flag{})\n{}\n{}\n",
        f.long,
        if global { ", global" } else { "" },
        f.doc.summary,
        f.doc.description
    )
}

#[derive(Default)]
pub struct ToolBuildCtx {
    stack: Vec<String>,
    command_path: Vec<String>,
}
impl ToolBuildCtx {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push_descriptor(&mut self, identity: impl Into<String>) -> Result<(), ToolBuildError> {
        let id = identity.into();
        if self.stack.contains(&id) {
            return Err(ToolBuildError::SubtreeCycle(self.cycle_path(&id)));
        }
        self.stack.push(id);
        Ok(())
    }
    pub fn pop_descriptor(&mut self) {
        self.stack.pop();
    }
    /// Build a child descriptor while the given identity is pushed on the
    /// recursion stack, always popping afterwards. Preferred over manual
    /// [`Self::push_descriptor`] / [`Self::pop_descriptor`] so an early return
    /// inside `f` cannot leak a stack entry and falsely report a later cycle.
    pub fn with_descriptor<T>(
        &mut self,
        identity: impl Into<String>,
        f: impl FnOnce(&mut Self) -> Result<T, ToolBuildError>,
    ) -> Result<T, ToolBuildError> {
        self.push_descriptor(identity)?;
        let result = f(self);
        self.pop_descriptor();
        result
    }
    fn cycle_path(&self, repeated: &str) -> String {
        let mut chain: Vec<&str> = self.stack.iter().map(String::as_str).collect();
        chain.push(repeated);
        chain.join(" -> ")
    }
    pub fn push_command(&mut self, name: impl Into<String>) {
        self.command_path.push(name.into());
    }
    pub fn pop_command(&mut self) {
        self.command_path.pop();
    }
    pub fn command_path(&self) -> &[String] {
        &self.command_path
    }
}
pub trait ToolDefinitionDescriptor {
    fn metadata(ctx: &mut ToolBuildCtx) -> Result<ExtendedToolType, ToolBuildError>;
}

/// Turn a child subtree's command list into the graft-local nodes to splice
/// beneath a parent. The child root (index 0) becomes the pure-dispatcher
/// placeholder for the parent's subtree command: its `body` must be `None`, its
/// `globals` and `subcommands` are preserved (so recursive globals still apply
/// and the graft-local child indices stay valid), and its `name`/`doc`/
/// `aliases` may be overridden by the parent's `#[command(...)]`.
///
/// `expected_name` is the parent subtree method's command name; unless an
/// explicit `override_name` is given, the child root name must equal it
/// (`SubtreeRootNameMismatch`). Command-annotations are not supported on a pure
/// dispatcher (the model places them on a command body), so a non-`None`
/// `override_annotations` is rejected rather than silently dropped.
///
/// Command indices are returned unchanged (graft-local); the final offset into
/// the parent's command tree is applied by [`append_grafted_subtree`].
pub fn graft_subtree(
    child: ExtendedToolType,
    expected_name: &str,
    override_name: Option<String>,
    override_doc: Option<Doc>,
    override_aliases: Option<Vec<String>>,
    override_annotations: Option<CommandAnnotations>,
) -> Result<Vec<ExtendedCommandNode>, ToolBuildError> {
    let mut nodes = child.commands;
    let root = nodes.first_mut().ok_or(ToolBuildError::EmptyCommandTree)?;
    if root.body.is_some() {
        return Err(ToolBuildError::SubtreeRootHasBody(root.name.clone()));
    }
    if override_annotations.is_some() {
        return Err(ToolBuildError::SubtreeAnnotationsUnsupported(
            override_name.clone().unwrap_or_else(|| root.name.clone()),
        ));
    }
    if override_name.is_none() && root.name != expected_name {
        return Err(ToolBuildError::SubtreeRootNameMismatch {
            expected: expected_name.to_string(),
            actual: root.name.clone(),
        });
    }
    if let Some(name) = override_name {
        root.name = name;
    }
    if let Some(doc) = override_doc {
        root.doc = doc;
    }
    if let Some(aliases) = override_aliases {
        root.aliases = aliases;
    }
    // body stays None; globals and subcommands (graft-local indices) unchanged.
    Ok(nodes)
}

/// Append a graft (graft-local command nodes whose index 0 is the dispatcher
/// placeholder) to `parent`, offsetting every internal subcommand index, and
/// return the parent index of the placeholder. The caller links this index as a
/// subcommand of the hosting command.
pub fn append_grafted_subtree(
    parent: &mut Vec<ExtendedCommandNode>,
    mut graft: Vec<ExtendedCommandNode>,
) -> i32 {
    let offset = parent.len() as i32;
    for node in &mut graft {
        node.subcommands = node.subcommands.iter().map(|i| i + offset).collect();
    }
    parent.extend(graft);
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic::tool_refinement::{refine_numeric, refine_text, refine_url};
    use crate::schema::schema_type::{NumericBound, NumericRestrictions};
    use test_r::test;

    fn doc(summary: &str) -> Doc {
        Doc {
            summary: summary.to_string(),
            description: String::new(),
            examples: vec![],
        }
    }

    fn str_graph() -> SchemaGraph {
        SchemaGraph::anonymous(SchemaType::string())
    }
    fn u32_graph() -> SchemaGraph {
        SchemaGraph::anonymous(SchemaType::u32())
    }

    fn sample_tool() -> ExtendedToolType {
        ExtendedToolType {
            version: "0.1.0".to_string(),
            commands: vec![
                ExtendedCommandNode {
                    name: "root".to_string(),
                    aliases: vec![],
                    doc: doc("root"),
                    globals: ExtendedGlobals {
                        options: vec![ExtendedOptionSpec {
                            long: "verbose".to_string(),
                            short: None,
                            aliases: vec![],
                            doc: doc("global"),
                            value_name: None,
                            shape: ExtendedOptionShape::Scalar(u32_graph()),
                            default: None,
                            required: false,
                            env_var: None,
                        }],
                        flags: vec![],
                    },
                    subcommands: vec![1],
                    body: None,
                },
                ExtendedCommandNode {
                    name: "run".to_string(),
                    aliases: vec!["r".to_string()],
                    doc: doc("run"),
                    globals: ExtendedGlobals::default(),
                    subcommands: vec![],
                    body: Some(ExtendedCommandBody {
                        positionals: ExtendedPositionals {
                            fixed: vec![ExtendedPositional {
                                name: "input".to_string(),
                                doc: doc("input"),
                                value_name: None,
                                type_: str_graph(),
                                default: None,
                                required: true,
                                accepts_stdio: false,
                            }],
                            tail: None,
                        },
                        options: vec![ExtendedOptionSpec {
                            long: "config".to_string(),
                            short: None,
                            aliases: vec![],
                            doc: doc("config"),
                            value_name: None,
                            shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
                                repetition: wire::Repetition::Repeated,
                                map_type: SchemaGraph::anonymous(SchemaType::map(
                                    SchemaType::string(),
                                    SchemaType::string(),
                                )),
                                duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
                            }),
                            default: None,
                            required: false,
                            env_var: None,
                        }],
                        flags: vec![FlagSpec {
                            long: "force".to_string(),
                            short: None,
                            aliases: vec![],
                            doc: doc("force"),
                            shape: wire::FlagShape::BoolFlag(wire::BoolFlagShape {
                                default: false,
                                negatable: false,
                            }),
                            env_var: None,
                        }],
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        result: Some(ExtendedResultSpec {
                            type_: str_graph(),
                            doc: doc("result"),
                            formatters: vec![ToolFormatter {
                                name: "human".to_string(),
                                doc: doc("human"),
                            }],
                            default_formatter: "human".to_string(),
                        }),
                        errors: vec![],
                        annotations: None,
                    }),
                },
            ],
        }
    }

    #[test]
    fn builds_tool_and_orders_fields() {
        let tool = sample_tool();
        let wire = tool.to_tool();
        assert_eq!(wire.commands.nodes.len(), 2);
        assert_eq!(
            wire.commands.nodes[1].body.as_ref().unwrap().options.len(),
            1
        );
        let names: Vec<_> = tool
            .canonical_input_fields(1)
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, vec!["verbose", "input", "config", "force"]);
    }

    #[test]
    fn help_contains_names() {
        let help = render_help(&sample_tool(), &["run".to_string()]).unwrap();
        assert!(help.contains("run"));
        assert!(help.contains("--config"));
    }

    fn dispatcher_child() -> ExtendedToolType {
        ExtendedToolType {
            version: "x".into(),
            commands: vec![
                ExtendedCommandNode {
                    name: "child".into(),
                    aliases: vec![],
                    doc: doc(""),
                    globals: ExtendedGlobals::default(),
                    subcommands: vec![1],
                    body: None,
                },
                ExtendedCommandNode {
                    name: "leaf".into(),
                    aliases: vec![],
                    doc: doc(""),
                    globals: ExtendedGlobals::default(),
                    subcommands: vec![],
                    body: None,
                },
            ],
        }
    }

    #[test]
    fn graft_rejects_root_with_body() {
        let err = graft_subtree(
            leaf_tool_with_body(empty_body()),
            "t",
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, ToolBuildError::SubtreeRootHasBody(_)));
    }

    #[test]
    fn graft_preserves_local_indices() {
        // The child root stays at index 0 as a body-less placeholder; its
        // graft-local subcommand index (1) is unchanged.
        let graft = graft_subtree(dispatcher_child(), "child", None, None, None, None).unwrap();
        assert_eq!(graft.len(), 2);
        assert!(graft[0].body.is_none());
        assert_eq!(graft[0].subcommands, vec![1]);

        // Appending at offset N shifts the internal index to N + 1.
        let mut parent = vec![ExtendedCommandNode {
            name: "root".into(),
            aliases: vec![],
            doc: doc(""),
            globals: ExtendedGlobals::default(),
            subcommands: vec![],
            body: None,
        }];
        let offset = append_grafted_subtree(&mut parent, graft);
        assert_eq!(offset, 1);
        assert_eq!(parent[1].subcommands, vec![2]);
        assert_eq!(parent.len(), 3);
    }

    #[test]
    fn graft_enforces_name_rule_and_rejects_annotations() {
        let mismatch =
            graft_subtree(dispatcher_child(), "remote", None, None, None, None).unwrap_err();
        assert!(matches!(
            mismatch,
            ToolBuildError::SubtreeRootNameMismatch { .. }
        ));

        // An explicit override name bypasses the match rule.
        let ok = graft_subtree(
            dispatcher_child(),
            "remote",
            Some("remote".into()),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(ok[0].name, "remote");

        let ann = graft_subtree(
            dispatcher_child(),
            "child",
            None,
            None,
            None,
            Some(CommandAnnotations {
                read_only: true,
                destructive: false,
                idempotent: false,
                open_world: false,
            }),
        )
        .unwrap_err();
        assert!(matches!(
            ann,
            ToolBuildError::SubtreeAnnotationsUnsupported(_)
        ));
    }

    #[test]
    fn cycle_detection_and_refinements_work() {
        let mut ctx = ToolBuildCtx::new();
        ctx.push_descriptor("a").unwrap();
        assert!(matches!(
            ctx.push_descriptor("a"),
            Err(ToolBuildError::SubtreeCycle(_))
        ));
        let text = refine_text(SchemaType::string(), Some("x+".into()), Some(1), Some(3));
        assert!(matches!(text, SchemaType::Text { .. }));
        let url = refine_url(
            SchemaType::url(Default::default()),
            Some(vec!["https".into()]),
        );
        assert!(matches!(url, SchemaType::Url { .. }));
        let num = refine_numeric(
            SchemaType::u32(),
            Some(NumericBound::Unsigned(1)),
            None,
            Some("ms".into()),
        );
        assert_eq!(
            num.numeric_restrictions().unwrap().unit.as_deref(),
            Some("ms")
        );
    }

    #[test]
    fn refine_numeric_overlays_existing_restrictions() {
        // A type that already carries min + unit; refining only max must
        // preserve the existing min and unit instead of dropping them.
        let base = refine_numeric(
            SchemaType::u32(),
            Some(NumericBound::Unsigned(10)),
            None,
            Some("items".into()),
        );
        let refined = refine_numeric(base, None, Some(NumericBound::Unsigned(20)), None);
        let r = refined.numeric_restrictions().unwrap();
        assert_eq!(r.min, Some(NumericBound::Unsigned(10)));
        assert_eq!(r.max, Some(NumericBound::Unsigned(20)));
        assert_eq!(r.unit.as_deref(), Some("items"));
    }

    #[test]
    fn refine_numeric_preserves_unspecified_existing_restrictions() {
        let base = SchemaType::U32 {
            restrictions: NumericRestrictions {
                min: Some(NumericBound::Unsigned(10)),
                max: Some(NumericBound::Unsigned(100)),
                unit: Some("items".to_string()),
            }
            .normalize(),
            metadata: Default::default(),
        };

        let refined = refine_numeric(base, None, Some(NumericBound::Unsigned(200)), None);

        let restrictions = refined.numeric_restrictions().unwrap();
        assert_eq!(restrictions.min, Some(NumericBound::Unsigned(10)));
        assert_eq!(restrictions.max, Some(NumericBound::Unsigned(200)));
        assert_eq!(restrictions.unit.as_deref(), Some("items"));
    }

    #[test]
    fn numeric_value_validation_rejects_malformed_restrictions() {
        let ty = SchemaType::U32 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Unsigned(10)),
                max: Some(NumericBound::Unsigned(1)),
                unit: None,
            }),
            metadata: Default::default(),
        };
        let graph = SchemaGraph::anonymous(ty.clone());

        validate_value(&graph, &ty, &SchemaValue::U32(5))
            .expect_err("malformed numeric restrictions must not be ignored");
    }

    fn leaf_tool_with_body(body: ExtendedCommandBody) -> ExtendedToolType {
        ExtendedToolType {
            version: "0.1.0".to_string(),
            commands: vec![ExtendedCommandNode {
                name: "t".to_string(),
                aliases: vec![],
                doc: doc(""),
                globals: ExtendedGlobals::default(),
                subcommands: vec![],
                body: Some(body),
            }],
        }
    }

    fn empty_body() -> ExtendedCommandBody {
        ExtendedCommandBody {
            positionals: ExtendedPositionals::default(),
            options: vec![],
            flags: vec![],
            constraints: vec![],
            stdin: None,
            stdout: None,
            result: None,
            errors: vec![],
            annotations: None,
        }
    }

    fn map_config_option(constraints: Vec<ExtendedConstraint>) -> ExtendedCommandBody {
        let mut body = empty_body();
        body.options = vec![ExtendedOptionSpec {
            long: "config".to_string(),
            short: None,
            aliases: vec![],
            doc: doc(""),
            value_name: None,
            shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
                repetition: wire::Repetition::Repeated,
                map_type: SchemaGraph::anonymous(SchemaType::map(
                    SchemaType::string(),
                    SchemaType::u32(),
                )),
                duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
            }),
            default: None,
            required: false,
            env_var: None,
        }];
        body.constraints = constraints;
        body
    }

    #[test]
    fn default_type_mismatch_is_rejected() {
        let mut body = empty_body();
        body.positionals.fixed = vec![ExtendedPositional {
            name: "count".to_string(),
            doc: doc(""),
            value_name: None,
            type_: u32_graph(),
            default: Some(SchemaValue::String("not-a-number".to_string())),
            required: false,
            accepts_stdio: false,
        }];
        let err = leaf_tool_with_body(body).try_to_tool().unwrap_err();
        assert!(matches!(err, ToolBuildError::DefaultTypeMismatch(_)));
    }

    #[test]
    fn value_is_resolves_to_map_value_type() {
        // A `value-is` over a repeatable-map names the map's value type (u32).
        let ok = leaf_tool_with_body(map_config_option(vec![ExtendedConstraint::RequiresAll(
            vec![ExtendedRef::ValueIs(ExtendedValueIsRef {
                name: "config".to_string(),
                value: SchemaValue::U32(1),
            })],
        )]));
        assert!(ok.try_to_tool().is_ok());

        let bad = leaf_tool_with_body(map_config_option(vec![ExtendedConstraint::RequiresAll(
            vec![ExtendedRef::ValueIs(ExtendedValueIsRef {
                name: "config".to_string(),
                value: SchemaValue::String("x".to_string()),
            })],
        )]));
        assert!(matches!(
            bad.try_to_tool().unwrap_err(),
            ToolBuildError::ValueIsTypeMismatch(_)
        ));
    }

    #[test]
    fn unresolved_constraint_ref_is_rejected() {
        let body = map_config_option(vec![ExtendedConstraint::RequiresAll(vec![
            ExtendedRef::Present("missing".to_string()),
        ])]);
        assert!(matches!(
            leaf_tool_with_body(body).try_to_tool().unwrap_err(),
            ToolBuildError::UnresolvedConstraintRef(_)
        ));
    }

    fn bare_node(name: &str, subcommands: Vec<i32>) -> ExtendedCommandNode {
        ExtendedCommandNode {
            name: name.into(),
            aliases: vec![],
            doc: doc(""),
            globals: ExtendedGlobals::default(),
            subcommands,
            body: None,
        }
    }

    fn tool_with_nodes(nodes: Vec<ExtendedCommandNode>) -> ExtendedToolType {
        ExtendedToolType {
            version: "0.1.0".into(),
            commands: nodes,
        }
    }

    fn scalar_opt(long: &str, short: Option<char>) -> ExtendedOptionSpec {
        ExtendedOptionSpec {
            long: long.into(),
            short,
            aliases: vec![],
            doc: doc(""),
            value_name: None,
            shape: ExtendedOptionShape::Scalar(str_graph()),
            default: None,
            required: false,
            env_var: None,
        }
    }

    fn bool_flag(long: &str, short: Option<char>) -> FlagSpec {
        FlagSpec {
            long: long.into(),
            short,
            aliases: vec![],
            doc: doc(""),
            shape: wire::FlagShape::BoolFlag(wire::BoolFlagShape {
                default: false,
                negatable: false,
            }),
            env_var: None,
        }
    }

    fn variant_graph() -> SchemaGraph {
        // A single well-formed case so the graph passes well-formedness and the
        // tool-specific "variant in input position" rule is what rejects it.
        SchemaGraph::anonymous(SchemaType::Variant {
            cases: vec![crate::schema::schema_type::VariantCaseType {
                name: "case".into(),
                payload: None,
                metadata: Default::default(),
            }],
            metadata: Default::default(),
        })
    }

    #[test]
    fn empty_tree_is_rejected() {
        assert!(matches!(
            validate_tool(&tool_with_nodes(vec![])),
            Err(ToolBuildError::EmptyCommandTree)
        ));
    }

    #[test]
    fn out_of_bounds_subcommand_is_rejected() {
        assert!(matches!(
            validate_tool(&tool_with_nodes(vec![bare_node("root", vec![5])])),
            Err(ToolBuildError::CommandIndexOutOfBounds { .. })
        ));
        assert!(matches!(
            validate_tool(&tool_with_nodes(vec![bare_node("root", vec![-1])])),
            Err(ToolBuildError::CommandIndexOutOfBounds { .. })
        ));
    }

    #[test]
    fn cyclic_tree_is_rejected_without_panicking() {
        let tool = tool_with_nodes(vec![
            bare_node("root", vec![1]),
            bare_node("child", vec![0]),
        ]);
        assert!(matches!(
            validate_tool(&tool),
            Err(ToolBuildError::CommandTreeCycle(_))
        ));
        // Helpers must not panic / infinitely recurse on the malformed tree.
        assert!(render_help(&tool, &[]).is_ok());
        let _ = tool.effective_globals(1);
    }

    #[test]
    fn shared_subcommand_is_rejected() {
        let tool = tool_with_nodes(vec![
            bare_node("root", vec![1, 2]),
            bare_node("a", vec![3]),
            bare_node("b", vec![3]),
            bare_node("leaf", vec![]),
        ]);
        assert!(matches!(
            validate_tool(&tool),
            Err(ToolBuildError::DuplicateCommandParent(3))
        ));
    }

    #[test]
    fn unreachable_node_is_rejected() {
        let tool = tool_with_nodes(vec![bare_node("root", vec![]), bare_node("orphan", vec![])]);
        assert!(matches!(
            validate_tool(&tool),
            Err(ToolBuildError::UnreachableCommandNode(1))
        ));
    }

    #[test]
    fn invalid_identifier_is_rejected() {
        assert!(matches!(
            validate_tool(&tool_with_nodes(vec![bare_node("Root", vec![])])),
            Err(ToolBuildError::InvalidIdentifier { .. })
        ));
    }

    #[test]
    fn body_name_colliding_with_inherited_global_is_rejected() {
        let mut root = bare_node("root", vec![1]);
        root.globals.options = vec![scalar_opt("shared", None)];
        let mut child = bare_node("child", vec![]);
        let mut body = empty_body();
        body.options = vec![scalar_opt("shared", None)];
        child.body = Some(body);
        assert!(matches!(
            validate_tool(&tool_with_nodes(vec![root, child])),
            Err(ToolBuildError::DuplicateName(_))
        ));
    }

    #[test]
    fn duplicate_short_form_is_rejected() {
        let mut body = empty_body();
        body.options = vec![
            scalar_opt("alpha", Some('a')),
            scalar_opt("beta", Some('a')),
        ];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::DuplicateShort('a'))
        ));
    }

    #[test]
    fn verbatim_tail_without_separator_is_rejected() {
        let mut body = empty_body();
        body.positionals.tail = Some(ExtendedTailPositional {
            name: "args".into(),
            doc: doc(""),
            value_name: None,
            item_type: str_graph(),
            min: 0,
            max: None,
            separator: None,
            verbatim: true,
            accepts_stdio: false,
        });
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::VerbatimWithoutSeparator(_))
        ));
    }

    #[test]
    fn variant_in_input_position_is_rejected() {
        let mut body = empty_body();
        body.positionals.fixed = vec![ExtendedPositional {
            name: "choice".into(),
            doc: doc(""),
            value_name: None,
            type_: variant_graph(),
            default: None,
            required: true,
            accepts_stdio: false,
        }];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::VariantInInputPosition(_))
        ));
    }

    #[test]
    fn value_is_against_flag_is_rejected() {
        let mut body = empty_body();
        body.flags = vec![bool_flag("force", None)];
        body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "force".into(),
                value: SchemaValue::Bool(true),
            },
        )])];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::ValueIsTypeMismatch(_))
        ));
    }

    #[test]
    fn repeatable_map_with_non_map_type_is_rejected() {
        let mut body = empty_body();
        body.options = vec![ExtendedOptionSpec {
            long: "config".into(),
            short: None,
            aliases: vec![],
            doc: doc(""),
            value_name: None,
            shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
                repetition: wire::Repetition::Repeated,
                map_type: str_graph(),
                duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
            }),
            default: None,
            required: false,
            env_var: None,
        }];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::RepeatableMapTypeNotMap(_))
        ));
    }

    fn ref_graph(id: &str) -> SchemaGraph {
        SchemaGraph::anonymous(SchemaType::Ref {
            id: id.into(),
            metadata: Default::default(),
        })
    }

    #[test]
    fn dangling_type_ref_in_positional_is_rejected() {
        let mut body = empty_body();
        body.positionals.fixed = vec![ExtendedPositional {
            name: "thing".into(),
            doc: doc(""),
            value_name: None,
            type_: ref_graph("missing-type"),
            default: None,
            required: true,
            accepts_stdio: false,
        }];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::UnresolvedTypeRef { id, .. }) if id == "missing-type"
        ));
    }

    #[test]
    fn dangling_type_ref_in_option_is_rejected() {
        let mut body = empty_body();
        body.options = vec![scalar_opt("name", None)];
        body.options[0].shape = ExtendedOptionShape::Scalar(ref_graph("nope"));
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::UnresolvedTypeRef { position, id })
                if position == "option --name" && id == "nope"
        ));
    }

    #[test]
    fn dangling_type_ref_in_definition_body_is_rejected() {
        // The root resolves, but a definition body references a missing id.
        let graph = SchemaGraph {
            defs: vec![crate::schema::SchemaTypeDef {
                id: "rec".into(),
                name: Some("rec".into()),
                body: SchemaType::List {
                    element: Box::new(SchemaType::Ref {
                        id: "gone".into(),
                        metadata: Default::default(),
                    }),
                    metadata: Default::default(),
                },
            }],
            root: SchemaType::Ref {
                id: "rec".into(),
                metadata: Default::default(),
            },
        };
        let mut body = empty_body();
        body.positionals.fixed = vec![ExtendedPositional {
            name: "thing".into(),
            doc: doc(""),
            value_name: None,
            type_: graph,
            default: None,
            required: true,
            accepts_stdio: false,
        }];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::UnresolvedTypeRef { id, .. }) if id == "gone"
        ));
    }

    #[test]
    fn ill_formed_numeric_restriction_is_rejected() {
        // A u32 positional whose inline restrictions are unsatisfiable
        // (min > max) must be rejected at tool-build time by well-formedness.
        let mut body = empty_body();
        body.positionals.fixed = vec![ExtendedPositional {
            name: "count".into(),
            doc: doc(""),
            value_name: None,
            type_: SchemaGraph::anonymous(SchemaType::U32 {
                restrictions: Some(crate::schema::schema_type::NumericRestrictions {
                    min: Some(NumericBound::Unsigned(10)),
                    max: Some(NumericBound::Unsigned(1)),
                    unit: None,
                }),
                metadata: Default::default(),
            }),
            default: None,
            required: true,
            accepts_stdio: false,
        }];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::IllFormedSchema { position, .. }) if position == "positional count"
        ));
    }

    #[test]
    fn resolvable_type_ref_is_accepted() {
        // A self-contained graph whose root Ref resolves within its own defs
        // must pass the closedness check.
        let graph = SchemaGraph {
            defs: vec![crate::schema::SchemaTypeDef {
                id: "rec".into(),
                name: Some("rec".into()),
                body: SchemaType::string(),
            }],
            root: SchemaType::Ref {
                id: "rec".into(),
                metadata: Default::default(),
            },
        };
        let mut body = empty_body();
        body.positionals.fixed = vec![ExtendedPositional {
            name: "thing".into(),
            doc: doc(""),
            value_name: None,
            type_: graph,
            default: None,
            required: true,
            accepts_stdio: false,
        }];
        assert!(validate_tool(&leaf_tool_with_body(body)).is_ok());
    }

    #[test]
    fn argument_help_finds_global_and_body_args() {
        let tool = sample_tool();
        // `verbose` is a root global visible at the `run` subcommand.
        let global = render_argument_help(&tool, &["run".to_string()], "verbose").unwrap();
        assert!(global.contains("--verbose"));
        assert!(global.contains("global"));
        // `config` is a body option on `run`.
        let body = render_argument_help(&tool, &["run".to_string()], "config").unwrap();
        assert!(body.contains("--config"));
        // Unknown argument is an error, not a panic.
        assert!(matches!(
            render_argument_help(&tool, &["run".to_string()], "nope"),
            Err(ToolBuildError::CommandNotFound(_))
        ));
    }
}
