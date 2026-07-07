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
use crate::agentic::tool_literal::{ToolLiteral, value_is_literal_to_schema_value};
use crate::golem_agentic::golem::tool::common as wire;
use crate::schema::tool as native;
use crate::schema::tool::validation::ToolValidationError;
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
    /// The body's positional-eligible parameters, in declaration order, used to
    /// finalize the tail positional after inherited-global de-projection (see
    /// [`reinfer_body_tail`]). The final surface of a `Vec<T>` candidate (tail
    /// positional vs repeatable-list option) depends on which following
    /// re-declarations survive composition, so the macro records the authored
    /// facts needed to finalize it; an explicit tail additionally carries its full
    /// authored spec so promotion is lossless. Empty for hand-built bodies and
    /// ignored by canonical conversion.
    pub positional_plan: Vec<PositionalCandidate>,
}

/// One positional-eligible parameter of a command body, recorded by the macro in
/// declaration order so the runtime can finalize the tail positional after
/// inherited-global de-projection. See [`ExtendedCommandBody::positional_plan`]
/// and [`reinfer_body_tail`].
#[derive(Clone, Debug)]
pub enum PositionalCandidate {
    /// A parameter that can never be the tail (a fixed scalar positional, or an
    /// explicit `#[arg(... = "positional")]`). It is recorded only so the
    /// declaration order of surviving candidates is known.
    Plain { name: String },
    /// A `Vec<T>` whose final surface — the tail positional or a repeatable-list
    /// option — depends on which following re-declarations survive de-projection
    /// (§5.8: the *last* positional `Vec<T>` is the tail, an earlier one is a
    /// repeatable-list option). The macro emits only the selected surface into the
    /// body. When demoting an inferred tail to an option the finalizer
    /// reconstructs it by copying the in-body spec (the value graph carries over
    /// unchanged), using these authored facts to reject a re-projection the target
    /// surface cannot represent. An explicit tail instead carries its full
    /// authored spec in `authored_tail_surrogate`, so promoting it back from a
    /// surrogate option is lossless.
    VecCandidate {
        name: String,
        /// Whether the tail was explicitly authored (`#[arg(... = "tail")]`); an
        /// explicit tail is never silently demoted to a repeatable-list option.
        explicit_tail: bool,
        /// Whether the parameter was `Option<Vec<T>>`; it can never become the
        /// tail (a tail is already variadic and has no representable absent state).
        optional_vec: bool,
        /// Whether `min`/`max` were authored. They are overloaded — occurrence
        /// bounds on a tail, item numeric bounds on a repeatable-list option — so
        /// an *inferred* candidate carrying them cannot switch surface without
        /// changing their meaning, and re-projection is rejected rather than
        /// silently reinterpreted. (Ignored for an explicit tail, whose authored
        /// occurrence bounds are preserved verbatim via `authored_tail_surrogate`.)
        has_min_or_max_attr: bool,
        /// For an explicit `#[arg(... = "tail")]` that the macro lowered to an
        /// inherited-global option surrogate (so it would not steal a genuine tail
        /// slot before de-projection): the full authored tail spec. When the
        /// surrogate survives and becomes the last positional, the finalizer
        /// installs this verbatim instead of reconstructing a tail from the option
        /// (which has no `separator`/`verbatim`/`accepts_stdio`/occurrence-bound
        /// fields). `None` for inferred candidates, which keep the
        /// reconstruct-from-spec path. Boxed to keep this variant compact.
        authored_tail_surrogate: Option<Box<ExtendedTailPositional>>,
        /// Long names of the body options declared after this `Vec<T>`, in
        /// declaration order. When this candidate is demoted from the tail to a
        /// repeatable-list option, it is inserted before the first of these that
        /// survived de-projection (preserving declaration order, §D7); otherwise
        /// it is appended.
        later_option_names: Vec<String>,
    },
}

impl PositionalCandidate {
    fn name(&self) -> &str {
        match self {
            PositionalCandidate::Plain { name }
            | PositionalCandidate::VecCandidate { name, .. } => name,
        }
    }
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

/// Implemented by `#[derive(ToolError)]` enums. A tool method returning
/// `Result<T, E>` reads its declared error cases from `E::error_cases()`.
///
/// Resolving an error case can fail the same way any other tool value type can:
/// a variant payload whose type resolves to an auto-injected schema has no value
/// graph, so this returns a [`ToolBuildError`] rather than panicking during
/// descriptor synthesis.
pub trait ToolErrorSchema {
    fn error_cases() -> Result<Vec<ExtendedErrorCase>, ToolBuildError>;

    fn to_error_payload_value(&self) -> Result<crate::TypedSchemaValue, String>;

    fn from_error_payload_value(value: crate::TypedSchemaValue) -> Result<Self, String>
    where
        Self: Sized;
}

#[derive(Clone, Debug)]
pub enum ExtendedRef {
    Present(String),
    ValueIs(ExtendedValueIsRef),
}

#[derive(Clone, Debug)]
pub struct ExtendedValueIsRef {
    pub name: String,
    pub value: ExtendedValueIsLiteral,
}

/// The literal a `value-is` constraint compares against.
///
/// The descriptor macro always emits the raw, un-typed
/// [`ExtendedValueIsLiteral::Deferred`] literal; it never re-derives a comparand
/// graph (doing so duplicated the runtime's option/list/map/tail and
/// refinement-placement rules and drifted from them). Every deferred literal is
/// resolved against the effective constraint scope by
/// [`normalize_inherited_globals`] — which runs inside the generated descriptor
/// fn — and is type-checked there, becoming [`ExtendedValueIsLiteral::Resolved`].
/// A literal naming a locally known argument is therefore still resolved (and any
/// type/refinement mismatch reported) when the descriptor is built; one naming a
/// global supplied only by an ancestor subtree method is resolved once that
/// global is in scope during composition. A deferred literal that survives
/// composition (the standalone subtree-child case) is reported as an unresolved
/// constraint reference by validation rather than silently accepted.
#[derive(Clone, Debug)]
pub enum ExtendedValueIsLiteral {
    Resolved(SchemaValue),
    Deferred(ToolLiteral),
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
    CommandIndexOutOfBounds {
        index: i32,
        len: usize,
    },
    UnreachableCommandNode(i32),
    CommandTreeCycle(i32),
    DuplicateCommandParent(i32),
    InvalidIdentifier {
        kind: &'static str,
        value: String,
    },
    SubtreeCycle(String),
    SubtreeRootNameMismatch {
        expected: String,
        actual: String,
    },
    SubtreeAnnotationsUnsupported(String),
    DuplicateName(String),
    DuplicateShort(char),
    InheritedGlobalConflict {
        name: String,
        inherited: String,
        command: String,
    },
    UnresolvedTypeRef {
        position: String,
        id: String,
    },
    IllFormedSchema {
        position: String,
        detail: String,
    },
    EncodeError(String),
    DefaultTypeMismatch(String),
    ValueIsTypeMismatch(String),
    RepeatableMapTypeNotMap(String),
    UnresolvedDefaultFormatter(String),
    VerbatimWithoutSeparator(String),
    VariantInInputPosition(String),
    CommandNotFound(String),
    UnresolvedConstraintRef(String),
    AutoInjectedToolParameter(String),
    InvalidNumericBound(String),
    RefinementTypeMismatch {
        refinement: &'static str,
        actual: &'static str,
    },
    UnresolvedValueIsLiteral(String),
    InvalidTailOccurrenceBounds {
        name: String,
        min: u32,
        max: u32,
    },
    RequiredPositionalAfterOptional(String),
    FixedPositionalAfterTail(String),
    VecSurfaceConflict {
        name: String,
        reason: String,
    },
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
            ToolBuildError::SubtreeRootNameMismatch { expected, actual } => write!(
                f,
                "subtree root name {actual:?} does not match the parent command name {expected:?}"
            ),
            ToolBuildError::SubtreeAnnotationsUnsupported(s) => write!(
                f,
                "annotations are not supported on a #[command(subtree = ...)] method {s:?} (the model places command-annotations on a command body)"
            ),
            ToolBuildError::DuplicateName(s) => write!(f, "duplicate tool metadata name: {s}"),
            ToolBuildError::DuplicateShort(c) => write!(f, "duplicate short form: {c:?}"),
            ToolBuildError::InheritedGlobalConflict {
                name,
                inherited,
                command,
            } => write!(
                f,
                "parameter surface name {name:?} on command {command:?} conflicts with inherited \
                 global {inherited:?}: it either has an incompatible shape or collides with more \
                 than one distinct inherited global; rename the parameter or align it with a \
                 single compatible inherited global"
            ),
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
            ToolBuildError::AutoInjectedToolParameter(s) => write!(
                f,
                "auto-injected types are not valid tool value parameters or results: {s}"
            ),
            ToolBuildError::InvalidNumericBound(s) => {
                write!(f, "invalid numeric bound: {s}")
            }
            ToolBuildError::RefinementTypeMismatch { refinement, actual } => write!(
                f,
                "{refinement} refinement cannot apply to a {actual} schema; the parameter's type \
                 resolves to a schema kind that has no {refinement} restrictions to set"
            ),
            ToolBuildError::UnresolvedValueIsLiteral(s) => write!(
                f,
                "value-is literal for argument {s:?} was not resolved against its comparand type \
                 during composition"
            ),
            ToolBuildError::InvalidTailOccurrenceBounds { name, min, max } => write!(
                f,
                "tail positional {name:?} has an impossible occurrence range: min {min} is greater \
                 than max {max}"
            ),
            ToolBuildError::RequiredPositionalAfterOptional(name) => write!(
                f,
                "required positional {name:?} cannot appear after an optional positional; optional \
                 positionals must be trailing"
            ),
            ToolBuildError::FixedPositionalAfterTail(name) => write!(
                f,
                "fixed positional {name:?} cannot appear after a tail positional; the tail \
                 positional must be the last positional"
            ),
            ToolBuildError::VecSurfaceConflict { name, reason } => write!(
                f,
                "the `Vec<_>` parameter {name:?} cannot be re-projected after inherited-global \
                 de-projection: {reason}"
            ),
        }
    }
}
impl std::error::Error for ToolBuildError {}

/// Build the value schema graph for a tool parameter/result Rust type,
/// returning a [`ToolBuildError`] (instead of panicking) when the type resolves
/// to an auto-injected schema, which has no value graph. Used by macro-generated
/// descriptors so an auto-injected param/result is a clean descriptor-build
/// error rather than a panic at registration time.
pub fn tool_value_schema<T: crate::agentic::Schema>(
    position: &str,
) -> Result<SchemaGraph, ToolBuildError> {
    T::get_type()
        .get_schema_graph()
        .ok_or_else(|| ToolBuildError::AutoInjectedToolParameter(position.to_string()))
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalInputField {
    pub name: String,
    pub aliases: Vec<String>,
    pub schema: SchemaGraph,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalInputModel {
    pub fields: Vec<CanonicalInputField>,
    pub record_schema: SchemaGraph,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalInputValue {
    pub name: String,
    pub aliases: Vec<String>,
    pub schema: SchemaGraph,
    pub value: SchemaValue,
}

impl CanonicalInputModel {
    pub fn from_fields(fields: Vec<CanonicalInputField>) -> Result<Self, ToolBuildError> {
        let record_schema = canonical_input_record_schema(&fields)?;
        Ok(Self {
            fields,
            record_schema,
        })
    }

    pub fn decode_record(
        &self,
        value: SchemaValue,
    ) -> Result<Vec<CanonicalInputValue>, CanonicalInputDecodeError> {
        let SchemaValue::Record { fields: values } = value else {
            return Err(CanonicalInputDecodeError::ExpectedRecord);
        };
        if values.len() != self.fields.len() {
            return Err(CanonicalInputDecodeError::FieldCountMismatch {
                expected: self.fields.len(),
                actual: values.len(),
            });
        }
        Ok(self
            .fields
            .iter()
            .cloned()
            .zip(values)
            .map(|(field, value)| CanonicalInputValue {
                name: field.name,
                aliases: field.aliases,
                schema: field.schema,
                value,
            })
            .collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalInputDecodeError {
    Model(ToolBuildError),
    ExpectedRecord,
    FieldCountMismatch { expected: usize, actual: usize },
}

impl Display for CanonicalInputDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanonicalInputDecodeError::Model(error) => write!(f, "{error}"),
            CanonicalInputDecodeError::ExpectedRecord => {
                write!(f, "tool input must be a positional record")
            }
            CanonicalInputDecodeError::FieldCountMismatch { expected, actual } => write!(
                f,
                "tool input record has {actual} fields, expected {expected} canonical fields"
            ),
        }
    }
}

impl std::error::Error for CanonicalInputDecodeError {}

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

    pub fn command_index_by_path(&self, command_path: &[String]) -> Option<usize> {
        let mut current = 0usize;
        if self.commands.is_empty() {
            return None;
        }
        if command_path.is_empty() {
            return self.commands[current].body.as_ref().map(|_| current);
        }
        for segment in command_path {
            let next = self.commands[current].subcommands.iter().find_map(|idx| {
                let idx = usize::try_from(*idx).ok()?;
                let node = self.commands.get(idx)?;
                (node.name == *segment || node.aliases.iter().any(|alias| alias == segment))
                    .then_some(idx)
            })?;
            current = next;
        }
        self.commands[current].body.as_ref().map(|_| current)
    }

    /// Projects the canonical-relevant subset of this descriptor onto the
    /// shared native tool model: command tree topology, surface names/aliases,
    /// and the input surface shapes (positionals, tail, options, flags).
    /// Docs, constraints, results, errors, annotations, and the shared
    /// definition registry are not part of the projection: it is used only to
    /// resolve the shared canonical *ordering* (surface references), which
    /// never inspects definitions. The per-field schema graphs stay the
    /// original embedded graphs of this descriptor.
    fn canonical_projection(&self) -> native::Tool {
        fn option_projection(option: &ExtendedOptionSpec) -> native::OptionSpec {
            native::OptionSpec {
                long: option.long.clone(),
                short: option.short,
                aliases: option.aliases.clone(),
                doc: native::Doc::default(),
                value_name: None,
                shape: match &option.shape {
                    ExtendedOptionShape::Scalar(g) => {
                        native::OptionShape::Scalar(schema_graph_root(g))
                    }
                    ExtendedOptionShape::OptionalScalar(g) => {
                        native::OptionShape::OptionalScalar(schema_graph_root(g))
                    }
                    ExtendedOptionShape::RepeatableList(r) => {
                        native::OptionShape::RepeatableList(native::RepeatableListShape {
                            repetition: repetition_projection(&r.repetition),
                            item_type: schema_graph_root(&r.item_type),
                        })
                    }
                    ExtendedOptionShape::RepeatableMap(r) => {
                        native::OptionShape::RepeatableMap(native::RepeatableMapShape {
                            repetition: repetition_projection(&r.repetition),
                            map_type: schema_graph_root(&r.map_type),
                            duplicate_key_policy: match r.duplicate_key_policy {
                                wire::DuplicateKeyPolicy::Reject => {
                                    native::DuplicateKeyPolicy::Reject
                                }
                                wire::DuplicateKeyPolicy::LastWins => {
                                    native::DuplicateKeyPolicy::LastWins
                                }
                            },
                        })
                    }
                },
                default: None,
                required: option.required,
                env_var: None,
            }
        }

        fn repetition_projection(repetition: &wire::Repetition) -> native::Repetition {
            match repetition {
                wire::Repetition::Repeated => native::Repetition::Repeated,
                wire::Repetition::Delimited(c) => native::Repetition::Delimited(*c),
                wire::Repetition::Either(c) => native::Repetition::Either(*c),
            }
        }

        fn flag_projection(flag: &FlagSpec) -> native::FlagSpec {
            native::FlagSpec {
                long: flag.long.clone(),
                short: flag.short,
                aliases: flag.aliases.clone(),
                doc: native::Doc::default(),
                shape: match &flag.shape {
                    wire::FlagShape::BoolFlag(shape) => {
                        native::FlagShape::BoolFlag(native::BoolFlagShape {
                            default: shape.default,
                            negatable: shape.negatable,
                        })
                    }
                    wire::FlagShape::CountFlag(max) => native::FlagShape::CountFlag(*max),
                },
                env_var: None,
            }
        }

        let nodes = self
            .commands
            .iter()
            .map(|node| native::CommandNode {
                name: node.name.clone(),
                aliases: node.aliases.clone(),
                doc: native::Doc::default(),
                globals: native::Globals {
                    options: node.globals.options.iter().map(option_projection).collect(),
                    flags: node.globals.flags.iter().map(flag_projection).collect(),
                },
                subcommands: node
                    .subcommands
                    .iter()
                    .map(|idx| native::CommandIndex(*idx))
                    .collect(),
                body: node.body.as_ref().map(|body| native::CommandBody {
                    positionals: native::Positionals {
                        fixed: body
                            .positionals
                            .fixed
                            .iter()
                            .map(|p| native::Positional {
                                name: p.name.clone(),
                                doc: native::Doc::default(),
                                value_name: None,
                                type_: schema_graph_root(&p.type_),
                                default: None,
                                required: p.required,
                                accepts_stdio: p.accepts_stdio,
                            })
                            .collect(),
                        tail: body
                            .positionals
                            .tail
                            .as_ref()
                            .map(|t| native::TailPositional {
                                name: t.name.clone(),
                                doc: native::Doc::default(),
                                value_name: None,
                                item_type: schema_graph_root(&t.item_type),
                                min: t.min,
                                max: t.max,
                                separator: t.separator.clone(),
                                verbatim: t.verbatim,
                                accepts_stdio: t.accepts_stdio,
                            }),
                    },
                    options: body.options.iter().map(option_projection).collect(),
                    flags: body.flags.iter().map(flag_projection).collect(),
                    constraints: Vec::new(),
                    stdin: None,
                    stdout: None,
                    result: None,
                    errors: Vec::new(),
                    annotations: None,
                }),
            })
            .collect();

        native::Tool {
            version: self.version.clone(),
            commands: native::CommandTree { nodes },
            schema: SchemaGraph::empty(),
        }
    }

    pub fn canonical_input_fields(&self, command_index: usize) -> Vec<CanonicalInputField> {
        let projection = self.canonical_projection();
        projection
            .canonical_input_surfaces(command_index)
            .into_iter()
            .map(|surface| {
                self.canonical_field_for_surface(command_index, surface)
                    .expect("canonical_input_surfaces returned an unresolved surface")
            })
            .collect()
    }

    /// The SDK-side field for one shared canonical surface reference: the
    /// name/aliases from the descriptor plus the *original* embedded
    /// self-contained graph of that surface (collected form for repeatable
    /// options and the tail).
    fn canonical_field_for_surface(
        &self,
        command_index: usize,
        surface: native::canonical::CanonicalSurfaceRef,
    ) -> Option<CanonicalInputField> {
        use native::canonical::CanonicalSurfaceRef;
        let body = || {
            self.commands
                .get(command_index)
                .and_then(|c| c.body.as_ref())
        };
        match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => {
                let option = self.commands.get(node)?.globals.options.get(index)?;
                Some(CanonicalInputField {
                    name: option.long.clone(),
                    aliases: option.aliases.clone(),
                    schema: option_collected_graph(&option.shape),
                })
            }
            CanonicalSurfaceRef::GlobalFlag { node, index } => {
                let flag = self.commands.get(node)?.globals.flags.get(index)?;
                Some(CanonicalInputField {
                    name: flag.long.clone(),
                    aliases: flag.aliases.clone(),
                    schema: flag_graph(flag),
                })
            }
            CanonicalSurfaceRef::BodyPositional { index } => {
                let positional = body()?.positionals.fixed.get(index)?;
                Some(CanonicalInputField {
                    name: positional.name.clone(),
                    aliases: Vec::new(),
                    schema: positional.type_.clone(),
                })
            }
            CanonicalSurfaceRef::BodyTail => {
                let tail = body()?.positionals.tail.as_ref()?;
                Some(CanonicalInputField {
                    name: tail.name.clone(),
                    aliases: Vec::new(),
                    schema: list_wrapper_graph(&tail.item_type),
                })
            }
            CanonicalSurfaceRef::BodyOption { index } => {
                let option = body()?.options.get(index)?;
                Some(CanonicalInputField {
                    name: option.long.clone(),
                    aliases: option.aliases.clone(),
                    schema: option_collected_graph(&option.shape),
                })
            }
            CanonicalSurfaceRef::BodyFlag { index } => {
                let flag = body()?.flags.get(index)?;
                Some(CanonicalInputField {
                    name: flag.long.clone(),
                    aliases: flag.aliases.clone(),
                    schema: flag_graph(flag),
                })
            }
        }
    }

    pub fn canonical_input_model(
        &self,
        command_index: usize,
    ) -> Result<CanonicalInputModel, ToolBuildError> {
        self.check_canonical_input_command_index(command_index)?;
        CanonicalInputModel::from_fields(self.canonical_input_fields(command_index))
    }

    pub fn canonical_input_record_schema(
        &self,
        command_index: usize,
    ) -> Result<SchemaGraph, ToolBuildError> {
        self.check_canonical_input_command_index(command_index)?;
        canonical_input_record_schema(&self.canonical_input_fields(command_index))
    }

    fn check_canonical_input_command_index(
        &self,
        command_index: usize,
    ) -> Result<(), ToolBuildError> {
        check_command_tree_structure(self)?;
        if command_index >= self.commands.len() {
            return Err(ToolBuildError::CommandIndexOutOfBounds {
                index: command_index as i32,
                len: self.commands.len(),
            });
        }
        if self.path_to(command_index).is_none() {
            return Err(ToolBuildError::UnreachableCommandNode(command_index as i32));
        }
        Ok(())
    }

    pub fn decode_canonical_input_record(
        &self,
        command_index: usize,
        value: SchemaValue,
    ) -> Result<Vec<CanonicalInputValue>, CanonicalInputDecodeError> {
        let model = self
            .canonical_input_model(command_index)
            .map_err(CanonicalInputDecodeError::Model)?;
        model.decode_record(value)
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

fn canonical_input_record_schema(
    fields: &[CanonicalInputField],
) -> Result<SchemaGraph, ToolBuildError> {
    native::canonical::record_schema_from_field_graphs(
        fields
            .iter()
            .map(|field| (field.name.as_str(), &field.schema)),
    )
    .map_err(canonical_error_to_build_error)
}

/// Builds the wire input record for a tool invocation from a canonical input
/// model and the caller-supplied `(canonical name, value)` pairs.
///
/// When the pairs already match the model's field order the values are used
/// directly; otherwise each model field takes the *last* pair with a matching
/// canonical name. A model field with no matching pair is an error.
pub fn build_canonical_input(
    model: &CanonicalInputModel,
    mut params: Vec<(&str, SchemaValue)>,
) -> Result<crate::TypedSchemaValue, String> {
    let fields = if model.fields.len() == params.len()
        && model
            .fields
            .iter()
            .zip(params.iter())
            .all(|(field, (name, _))| field.name.as_str() == *name)
    {
        params.into_iter().map(|(_, value)| value).collect()
    } else {
        let mut fields: Vec<SchemaValue> = Vec::with_capacity(model.fields.len());
        for field in &model.fields {
            let index = params
                .iter()
                .rposition(|(name, _)| *name == field.name.as_str())
                .ok_or_else(|| format!("missing canonical tool input field `{}`", field.name))?;
            fields.push(params.remove(index).1);
        }
        fields
    };
    Ok(crate::TypedSchemaValue::new(
        model.record_schema.clone(),
        SchemaValue::Record { fields },
    ))
}

/// Builds the wire input record for a tool invocation whose leading fields are
/// inherited values captured by a parent subtree client.
///
/// The effective canonical input model is the inherited values (as fields, in
/// capture order) followed by `command_fields` minus any field whose surface
/// names (name or aliases) collide with an inherited surface name. The record
/// values are the inherited values followed by the caller-supplied pairs
/// matched by canonical name, exactly as in [`build_canonical_input`].
pub fn build_canonical_input_with_prefix(
    command_fields: Vec<CanonicalInputField>,
    inherited_prefix: &[CanonicalInputValue],
    mut params: Vec<(&str, SchemaValue)>,
) -> Result<crate::TypedSchemaValue, String> {
    let mut canonical_fields: Vec<CanonicalInputField> = inherited_prefix
        .iter()
        .map(|value| CanonicalInputField {
            name: value.name.clone(),
            aliases: value.aliases.clone(),
            schema: value.schema.clone(),
        })
        .collect();
    let inherited_names: BTreeSet<&str> = inherited_prefix
        .iter()
        .flat_map(|value| {
            std::iter::once(value.name.as_str()).chain(value.aliases.iter().map(String::as_str))
        })
        .collect();
    canonical_fields.extend(command_fields.into_iter().filter(|field| {
        !inherited_names.contains(field.name.as_str())
            && !field
                .aliases
                .iter()
                .any(|alias| inherited_names.contains(alias.as_str()))
    }));
    let model =
        CanonicalInputModel::from_fields(canonical_fields).map_err(|error| error.to_string())?;

    let mut fields: Vec<SchemaValue> = inherited_prefix
        .iter()
        .map(|value| value.value.clone())
        .collect();
    for field in model.fields.iter().skip(inherited_prefix.len()) {
        let index = params
            .iter()
            .rposition(|(name, _)| *name == field.name.as_str())
            .ok_or_else(|| format!("missing canonical tool input field `{}`", field.name))?;
        fields.push(params.remove(index).1);
    }
    Ok(crate::TypedSchemaValue::new(
        model.record_schema,
        SchemaValue::Record { fields },
    ))
}

/// Maps the shared canonical/validation error type onto the SDK's
/// [`ToolBuildError`]. Variants that cannot arise from canonical input model
/// construction fall back to [`ToolBuildError::EncodeError`].
fn canonical_error_to_build_error(error: ToolValidationError) -> ToolBuildError {
    match error {
        ToolValidationError::EmptyCommandTree => ToolBuildError::EmptyCommandTree,
        ToolValidationError::CommandIndexOutOfBounds { index, len } => {
            ToolBuildError::CommandIndexOutOfBounds { index, len }
        }
        ToolValidationError::UnreachableCommandNode { index } => {
            ToolBuildError::UnreachableCommandNode(index)
        }
        ToolValidationError::CommandTreeCycle { index } => ToolBuildError::CommandTreeCycle(index),
        ToolValidationError::DuplicateCommandParent { index } => {
            ToolBuildError::DuplicateCommandParent(index)
        }
        ToolValidationError::UnresolvedTypeRef { position, id, .. } => {
            ToolBuildError::UnresolvedTypeRef { position, id }
        }
        ToolValidationError::IllFormedSchema {
            position, detail, ..
        } => ToolBuildError::IllFormedSchema { position, detail },
        other => ToolBuildError::EncodeError(other.to_string()),
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
                ExtendedRef::ValueIs(v) => {
                    let value = match &v.value {
                        ExtendedValueIsLiteral::Resolved(sv) => encode_schema_value_default(sv)?,
                        // A deferred literal reaching encoding means composition
                        // never resolved it against a comparand type; the wire
                        // model only carries resolved values.
                        ExtendedValueIsLiteral::Deferred(_) => {
                            return Err(ToolBuildError::UnresolvedValueIsLiteral(v.name.clone()));
                        }
                    };
                    wire::Ref::ValueIs(wire::ValueIsRef {
                        name: v.name.clone(),
                        value,
                    })
                }
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
pub fn option_collected_graph(s: &ExtendedOptionShape) -> SchemaGraph {
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

/// How a `value-is` literal is matched against its comparand graph.
///
/// The distinction is whether the referenced surface *collects* multiple CLI
/// occurrences into a container. The spec says a `value-is` against a "repeated
/// or list-typed name means any occurrence / element equals this literal", so a
/// collecting surface compares exactly one collected occurrence, never the whole
/// container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueIsMode {
    /// The literal must be a valid value for the comparand graph exactly, with
    /// no element/value relaxation. Used for *collecting* surfaces — a
    /// repeatable-list option, a repeatable-map option, or a tail positional —
    /// whose comparand is already the per-occurrence type (list item, map value,
    /// tail item). A `Vec<String>` repeatable option compares against `String`
    /// (accepting `"x"`, rejecting `["x"]`); a `Vec<Vec<u32>>` repeatable option
    /// compares against `list<u32>` (accepting `[1]`, rejecting `1` and `[[1]]`).
    Exact,
    /// The literal may be a valid value for the comparand graph as a whole, or —
    /// after peeling leading `option` wrappers — for exactly one element of a
    /// list/fixed-list comparand or one value of a map comparand. Used for
    /// *non-collecting* value surfaces: scalar / optional-scalar options and
    /// fixed positionals, whose declared value is the comparand itself. A
    /// `BTreeMap<String,u32>` positional accepts the whole map or a single `u32`
    /// value; a `FixedList<u32,2>` positional accepts the whole `[1,2]` or a
    /// single element.
    WholeOrOnePeel,
}

/// A `value-is` comparand: the graph a literal is matched against, plus the
/// [`ValueIsMode`] controlling whether the one-level element/value relaxation
/// applies.
#[derive(Clone, Debug)]
struct ValueIsComparand {
    graph: SchemaGraph,
    mode: ValueIsMode,
}

/// The `value-is` comparand recorded for a referenceable name. Mirrors the host
/// validator's `ValueComparand`: a value-carrying name maps to a typed
/// comparand; a name whose declared type cannot yield a comparable value (a
/// repeatable-map whose map type does not resolve to a map) is
/// [`ValueComparand::BlockedByTypeError`] so `value-is` checking is suppressed
/// and the underlying type error is reported by [`validate_tool`] instead of a
/// misleading cascading mismatch. A name absent from the scope's comparand map
/// is a flag (no value type), against which a `value-is` is a genuine mismatch.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
enum ValueComparand {
    Type(ValueIsComparand),
    BlockedByTypeError,
}

/// The `value-is` comparand for an option, keyed by whether the option collects
/// occurrences. A scalar / optional-scalar option is non-collecting: its
/// declared value graph is matched with the whole-or-one-peel relaxation. A
/// repeatable-list option collects into a list, so its comparand is the
/// per-occurrence item type matched exactly; a repeatable-map option collects
/// into a map, so its comparand is the per-entry map *value* type matched
/// exactly. A repeatable-map whose map type does not resolve to a map yields
/// [`ValueComparand::BlockedByTypeError`] (the malformed type is reported by
/// [`validate_tool`]). Definition graphs are preserved so any `Ref` still
/// resolves.
fn option_value_is_comparand(shape: &ExtendedOptionShape) -> ValueComparand {
    match shape {
        ExtendedOptionShape::Scalar(g) | ExtendedOptionShape::OptionalScalar(g) => {
            ValueComparand::Type(ValueIsComparand {
                graph: g.clone(),
                mode: ValueIsMode::WholeOrOnePeel,
            })
        }
        ExtendedOptionShape::RepeatableList(r) => ValueComparand::Type(ValueIsComparand {
            graph: r.item_type.clone(),
            mode: ValueIsMode::Exact,
        }),
        ExtendedOptionShape::RepeatableMap(r) if resolves_to_map(&r.map_type) => {
            ValueComparand::Type(ValueIsComparand {
                graph: map_value_graph(&r.map_type),
                mode: ValueIsMode::Exact,
            })
        }
        ExtendedOptionShape::RepeatableMap(_) => ValueComparand::BlockedByTypeError,
    }
}

/// Whether a comparand graph is structurally sound (no dangling references or
/// pure-alias cycles). When it is not, [`validate_tool`] reports the schema
/// error (an [`ToolBuildError::UnresolvedTypeRef`] / [`ToolBuildError::IllFormedSchema`]),
/// so `value-is` resolution must not also report a cascading mismatch against a
/// graph that cannot be resolved.
fn comparand_graph_is_sound(graph: &SchemaGraph) -> bool {
    crate::schema::validation::validate_graph(graph).is_ok()
}

/// The per-entry value comparand for a map: a graph whose root is the map's
/// value type, with the map graph's definitions preserved so any `Ref` in the
/// value type still resolves. Falls back to the original graph when the root
/// does not resolve to a `Map` (a not-a-map repeatable-map type is reported
/// separately, so this avoids fabricating the value comparand).
fn map_value_graph(map: &SchemaGraph) -> SchemaGraph {
    match map.resolve_ref(&map.root) {
        Ok(SchemaType::Map { value, .. }) => SchemaGraph {
            defs: map.defs.clone(),
            root: (**value).clone(),
        },
        _ => map.clone(),
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

/// Whether a `value-is` literal is compatible with its [`ValueIsComparand`].
///
/// The literal is always compatible if it is a valid value for the comparand
/// graph as a whole. For a [`ValueIsMode::WholeOrOnePeel`] comparand (a
/// non-collecting value surface) it is *also* compatible — under the WIT "any
/// element / entry equals this literal" relaxation — if it is a valid value for
/// the element type of a list-shaped, or the value type of a map-shaped,
/// (optionally `option`-wrapped) comparand. A [`ValueIsMode::Exact`] comparand
/// (a collecting surface, whose graph is already the per-occurrence type) gets
/// no relaxation: matching one more level would descend past a single
/// occurrence (e.g. accept `1` against a `Vec<Vec<u32>>` option whose occurrence
/// is `list<u32>`).
fn value_is_compatible(comparand: &ValueIsComparand, value: &SchemaValue) -> bool {
    let graph = &comparand.graph;
    if validate_value(graph, &graph.root, value).is_ok() {
        return true;
    }
    if comparand.mode == ValueIsMode::Exact {
        return false;
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
    match peeled {
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            validate_value(graph, element, value).is_ok()
        }
        SchemaType::Map {
            value: map_value, ..
        } => validate_value(graph, map_value, value).is_ok(),
        _ => false,
    }
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

/// Per-name `value-is` comparand for constraint resolution (only value-typed
/// names; a name in `names` but absent from `typed` is a flag).
#[derive(Default)]
struct NameScope {
    names: BTreeSet<String>,
    typed: BTreeMap<String, ValueComparand>,
}

/// Registers a flag's referenceable names. A flag carries no value type, so it
/// is never added to [`NameScope::typed`]; a `value-is` against it is rejected.
fn register_flag_scope(scope: &mut NameScope, flag: &FlagSpec) {
    scope.names.insert(flag.long.clone());
    scope.names.extend(flag.aliases.iter().cloned());
}

/// Registers a fixed positional's name and its whole-or-one-peel comparand (a
/// fixed positional is a non-collecting value surface: its declared type is the
/// comparand).
fn register_fixed_positional_scope(scope: &mut NameScope, positional: &ExtendedPositional) {
    scope.names.insert(positional.name.clone());
    scope.typed.insert(
        positional.name.clone(),
        ValueComparand::Type(ValueIsComparand {
            graph: positional.type_.clone(),
            mode: ValueIsMode::WholeOrOnePeel,
        }),
    );
}

/// Registers a tail positional's name and its per-occurrence comparand. A tail
/// collects occurrences into a `list<item>`, so a `value-is` literal matches one
/// item exactly (the tail's `item_type`), never the whole collected list.
fn register_tail_scope(scope: &mut NameScope, tail: &ExtendedTailPositional) {
    scope.names.insert(tail.name.clone());
    scope.typed.insert(
        tail.name.clone(),
        ValueComparand::Type(ValueIsComparand {
            graph: tail.item_type.clone(),
            mode: ValueIsMode::Exact,
        }),
    );
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
            register_flag_scope(&mut scope, flag);
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
        register_flag_scope(&mut scope, flag);
    }

    // Optional fixed positionals must be trailing: once an optional one appears,
    // no required one may follow, or the boundary between them is ambiguous. The
    // macro enforces this for locally declared positionals, but inherited-global
    // de-projection can leave a re-declared optional positional local at runtime
    // (its inherited global is itself de-projected against a strict ancestor), so
    // the surviving order is re-checked here.
    let mut saw_optional_positional = false;
    for positional in &body.positionals.fixed {
        check_identifier("positional name", &positional.name)?;
        check_graph_closed(
            &positional.type_,
            &format!("positional {}", positional.name),
        )?;
        insert_unique(&mut names, &positional.name)?;
        register_fixed_positional_scope(&mut scope, positional);
        if let Some(default) = &positional.default {
            validate_default(default, &positional.type_)?;
        }
        if graph_reaches_variant(&positional.type_) {
            return Err(ToolBuildError::VariantInInputPosition(
                positional.name.clone(),
            ));
        }
        if positional.required {
            if saw_optional_positional {
                return Err(ToolBuildError::RequiredPositionalAfterOptional(
                    positional.name.clone(),
                ));
            }
        } else {
            saw_optional_positional = true;
        }
    }

    if let Some(tail) = &body.positionals.tail {
        check_identifier("positional name", &tail.name)?;
        check_graph_closed(&tail.item_type, &format!("tail {}", tail.name))?;
        insert_unique(&mut names, &tail.name)?;
        register_tail_scope(&mut scope, tail);
        if graph_reaches_variant(&tail.item_type) {
            return Err(ToolBuildError::VariantInInputPosition(tail.name.clone()));
        }
        if tail.verbatim && tail.separator.is_none() {
            return Err(ToolBuildError::VerbatimWithoutSeparator(tail.name.clone()));
        }
        if let Some(max) = tail.max
            && tail.min > max
        {
            return Err(ToolBuildError::InvalidTailOccurrenceBounds {
                name: tail.name.clone(),
                min: tail.min,
                max,
            });
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
    let comparand = option_value_is_comparand(&opt.shape);
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
                match scope.typed.get(&v.name) {
                    // A name with no value type (a flag) cannot carry a value-is.
                    None => return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone())),
                    // A repeatable-map whose type is not a map: the malformed
                    // type is reported by the structural checks; suppress the
                    // cascading value-is mismatch.
                    Some(ValueComparand::BlockedByTypeError) => {}
                    Some(ValueComparand::Type(comparand)) => match &v.value {
                        ExtendedValueIsLiteral::Resolved(value) => {
                            if !value_is_compatible(comparand, value) {
                                return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone()));
                            }
                        }
                        // The name is in scope, so composition (which carries the
                        // comparand type) should have resolved this literal. A
                        // surviving deferred literal is a resolution gap, not a
                        // silently acceptable ref.
                        ExtendedValueIsLiteral::Deferred(_) => {
                            return Err(ToolBuildError::UnresolvedValueIsLiteral(v.name.clone()));
                        }
                    },
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
    inherited_globals: Vec<EffectiveCommandField>,
    graft_roots: Vec<PendingGraftRoot>,
}

#[derive(Clone)]
struct PendingGraftRoot {
    expected_name: String,
    override_name: Option<String>,
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
    /// True when no ancestor descriptor is currently on the recursion stack —
    /// i.e. this is the outermost `__golem_tool_descriptor_for_*` invocation.
    /// Must be called from inside [`Self::with_descriptor`] (the current
    /// descriptor's identity is then on top of the stack), so a value of `1`
    /// means there is exactly one descriptor in flight: this one.
    ///
    /// A nested subtree child descriptor (called from a parent's subtree link)
    /// returns `false`. The child therefore skips composition/normalization and
    /// returns its raw command tree, with `value-is` literals still deferred;
    /// the outermost descriptor normalizes the fully grafted tree once, when all
    /// ancestor subtree globals and inherited-global de-projections are in scope.
    /// Normalizing a nested child against its standalone scope would resolve (and
    /// type-check) its constraints against child-local declarations that parent
    /// composition may widen or replace, rejecting a constraint that is valid in
    /// the composed tool.
    pub fn is_outermost_descriptor(&self) -> bool {
        self.stack.len() == 1
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
    pub fn inherited_globals(&self) -> &[EffectiveCommandField] {
        &self.inherited_globals
    }
    pub fn with_inherited_globals<T>(
        &mut self,
        globals: Vec<EffectiveCommandField>,
        f: impl FnOnce(&mut Self) -> Result<T, ToolBuildError>,
    ) -> Result<T, ToolBuildError> {
        let old_len = self.inherited_globals.len();
        self.inherited_globals.extend(globals);
        let result = f(self);
        self.inherited_globals.truncate(old_len);
        result
    }
    pub fn with_graft_root<T>(
        &mut self,
        expected_name: String,
        override_name: Option<String>,
        f: impl FnOnce(&mut Self) -> Result<T, ToolBuildError>,
    ) -> Result<T, ToolBuildError> {
        self.graft_roots.push(PendingGraftRoot {
            expected_name,
            override_name,
        });
        let result = f(self);
        self.graft_roots.pop();
        result
    }
    pub fn apply_pending_graft_root(
        &self,
        root: &mut ExtendedCommandNode,
    ) -> Result<(), ToolBuildError> {
        let Some(pending) = self.graft_roots.last() else {
            return Ok(());
        };
        if pending.override_name.is_none() && root.name != pending.expected_name {
            return Err(ToolBuildError::SubtreeRootNameMismatch {
                expected: pending.expected_name.clone(),
                actual: root.name.clone(),
            });
        }
        if let Some(name) = &pending.override_name {
            root.name = name.clone();
        }
        Ok(())
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

pub fn reconcile_subtree_parent_globals(
    mut parent_globals: ExtendedGlobals,
    strict_ancestor_globals: &[EffectiveCommandField],
    command_name: &str,
) -> Result<ExtendedGlobals, ToolBuildError> {
    reconcile_globals(&mut parent_globals, strict_ancestor_globals, command_name)?;
    Ok(parent_globals)
}

pub fn reconcile_command_inherited_globals(
    node: &mut ExtendedCommandNode,
    strict_ancestor_globals: &[EffectiveCommandField],
    command_name: &str,
) -> Result<(), ToolBuildError> {
    reconcile_globals(&mut node.globals, strict_ancestor_globals, command_name)?;
    if let Some(body) = node.body.as_mut() {
        reconcile_body(body, strict_ancestor_globals, command_name)?;
    }
    Ok(())
}

/// Turn a child subtree's command list into the graft-local nodes to splice
/// beneath a parent. The child root (index 0) becomes the parent's subtree
/// command: its `globals` and `subcommands` are preserved (so recursive
/// globals still apply and the graft-local child indices stay valid), and its
/// `name`/`doc`/`aliases` may be overridden by the parent's `#[command(...)]`.
///
/// The child root may carry a body — the child trait's implicit-body method
/// (e.g. `git stash` runs the `stash` body while `git stash pop` walks to the
/// `pop` child). The subtree method's propagating params (`parent_globals`)
/// are first reconciled against `strict_ancestor_globals`, then the child root's
/// own globals and body are reconciled against the full inherited set. A
/// compatible same-name re-declaration is de-projected onto the inherited
/// global; an incompatible one is rejected as `InheritedGlobalConflict`.
/// Surviving `parent_globals` are then prepended onto the grafted root's
/// globals so they propagate to every descendant subcommand.
///
/// `expected_name` is the parent subtree method's command name; unless an
/// explicit `override_name` is given, the child root name must equal it
/// (`SubtreeRootNameMismatch`). Command-annotations are not supported on a
/// subtree command (the model places them on a command body), so a non-`None`
/// `override_annotations` is rejected rather than silently dropped.
///
/// Command indices are returned unchanged (graft-local); the final offset into
/// the parent's command tree is applied by [`append_grafted_subtree`].
pub fn graft_subtree(
    child: ExtendedToolType,
    expected_name: &str,
    parent_globals: ExtendedGlobals,
    strict_ancestor_globals: &[EffectiveCommandField],
    override_name: Option<String>,
    override_doc: Option<Doc>,
    override_aliases: Option<Vec<String>>,
    override_annotations: Option<CommandAnnotations>,
) -> Result<Vec<ExtendedCommandNode>, ToolBuildError> {
    let mut nodes = child.commands;
    let root = nodes.first_mut().ok_or(ToolBuildError::EmptyCommandTree)?;
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
    // Apply overrides first so any reconciliation error reports the final
    // grafted command name rather than the standalone child root name.
    if let Some(name) = override_name {
        root.name = name;
    }
    if let Some(doc) = override_doc {
        root.doc = doc;
    }
    if let Some(aliases) = override_aliases {
        root.aliases = aliases;
    }

    let command_name = root.name.clone();

    // The subtree method's params become propagating globals on the grafted
    // root. They are inherited from the parent command, so first reconcile them
    // against strict ancestors above the graft point, then reconcile the grafted
    // root's own globals/body against the full inherited set. Doing both here
    // preserves the normal inherited-global contract even though the parent
    // globals will be stored as same-node globals on the grafted root.
    let parent_globals =
        reconcile_subtree_parent_globals(parent_globals, strict_ancestor_globals, &command_name)?;

    let mut inherited = strict_ancestor_globals.to_vec();
    inherited.extend(
        parent_globals
            .options
            .iter()
            .cloned()
            .map(EffectiveCommandField::Option),
    );
    inherited.extend(
        parent_globals
            .flags
            .iter()
            .cloned()
            .map(EffectiveCommandField::Flag),
    );

    reconcile_globals(&mut root.globals, &inherited, &command_name)?;
    if let Some(body) = root.body.as_mut() {
        reconcile_body(body, &inherited, &command_name)?;
    }

    // Prepend the parent globals so they propagate to every descendant
    // subcommand of the grafted root. Globals and subcommands keep their
    // graft-local indices; `append_grafted_subtree` shifts them on append.
    let mut opts = parent_globals.options;
    opts.append(&mut root.globals.options);
    root.globals.options = opts;

    let mut flags = parent_globals.flags;
    flags.append(&mut root.globals.flags);
    root.globals.flags = flags;

    Ok(nodes)
}

/// Normalize a whole tool's command tree against inherited globals.
///
/// A subtree child trait is synthesized independently, so a child command whose
/// Rust signature repeats a parameter an ancestor declares as a global projects
/// it as a body option/flag/positional (or as its own global) in the standalone
/// descriptor. Likewise a leaf command in the same trait as the root may repeat
/// a root global. Once composed under the ancestor that supplies that name as a
/// global, the local re-declaration must be reconciled, otherwise the canonical
/// shape carries a body-local (or nested-global) name colliding with an
/// effective inherited global.
///
/// This is the single source of truth for that reconciliation. It traverses the
/// tree root→leaf, carrying the *strict ancestor* globals in scope. For every
/// node it reconciles the node's own globals and its body arguments against the
/// strict-ancestor globals:
///
/// * a same-name re-declaration whose canonical input shape is *compatible* with
///   the inherited global is removed — the ancestor global is the single source
///   of truth for docs, defaults, requiredness, aliases, and parse behavior;
/// * a same-name re-declaration whose shape is *incompatible* is an
///   [`ToolBuildError::InheritedGlobalConflict`]: the composition is invalid and
///   is rejected rather than silently dropping or replacing the local parameter.
///
/// Body arguments are reconciled only against *strict ancestors*, never the
/// node's own globals; a body argument colliding with a global declared on the
/// same command is an ordinary authoring error left for [`validate_tool`].
///
/// The traversal guards against malformed (cyclic / out-of-bounds) trees so it
/// is safe to run before [`validate_tool`] proves the tree well-formed.
pub fn normalize_inherited_globals(tool: &mut ExtendedToolType) -> Result<(), ToolBuildError> {
    if tool.commands.is_empty() {
        return Ok(());
    }
    let mut visited = vec![false; tool.commands.len()];
    normalize_command(tool, 0, &[], &mut visited)
}

fn normalize_command(
    tool: &mut ExtendedToolType,
    index: usize,
    ancestor_globals: &[EffectiveCommandField],
    visited: &mut [bool],
) -> Result<(), ToolBuildError> {
    if index >= tool.commands.len() || visited[index] {
        return Ok(());
    }
    visited[index] = true;

    let command_name = tool.commands[index].name.clone();

    // Reconcile this node's own globals and body args against strict ancestors.
    {
        let node = &mut tool.commands[index];
        reconcile_globals(&mut node.globals, ancestor_globals, &command_name)?;
        if let Some(body) = node.body.as_mut() {
            reconcile_body(body, ancestor_globals, &command_name)?;
        }
    }

    // Resolve any deferred `value-is` literals now that the constraint scope —
    // strict-ancestor globals plus this node's own (surviving) globals and body
    // arguments — is known. A subtree child trait names a constraint against a
    // global supplied by an ancestor subtree method; the standalone child could
    // not type the literal, so it was deferred until this composition step.
    if tool.commands[index].body.is_some() {
        let node = &mut tool.commands[index];
        let scope = value_is_scope(ancestor_globals, &node.globals, node.body.as_ref().unwrap());
        resolve_deferred_value_is(node.body.as_mut().unwrap(), &scope)?;
    }

    // Children inherit the strict-ancestor globals plus this node's surviving
    // globals (the ones not removed as compatible re-declarations).
    let mut child_globals = ancestor_globals.to_vec();
    {
        let node = &tool.commands[index];
        child_globals.extend(
            node.globals
                .options
                .iter()
                .cloned()
                .map(EffectiveCommandField::Option),
        );
        child_globals.extend(
            node.globals
                .flags
                .iter()
                .cloned()
                .map(EffectiveCommandField::Flag),
        );
    }

    let subcommands = tool.commands[index].subcommands.clone();
    for sub in subcommands {
        if sub >= 0 {
            normalize_command(tool, sub as usize, &child_globals, visited)?;
        }
    }
    Ok(())
}

/// Builds the `value-is` resolution scope for a command body: strict-ancestor
/// globals, the node's own globals, and the body's own arguments. This mirrors
/// the constraint scope assembled by [`check_body`] (via [`register_option_scope`]
/// and the positional/tail/flag handling) so deferred-literal resolution and
/// validation agree on which names are value-carrying and on each name's
/// comparand graph.
fn value_is_scope(
    ancestors: &[EffectiveCommandField],
    node_globals: &ExtendedGlobals,
    body: &ExtendedCommandBody,
) -> NameScope {
    let mut scope = NameScope::default();
    for field in ancestors {
        match field {
            EffectiveCommandField::Option(opt) => register_option_scope(&mut scope, opt),
            EffectiveCommandField::Flag(flag) => register_flag_scope(&mut scope, flag),
        }
    }
    for opt in &node_globals.options {
        register_option_scope(&mut scope, opt);
    }
    for flag in &node_globals.flags {
        register_flag_scope(&mut scope, flag);
    }
    for opt in &body.options {
        register_option_scope(&mut scope, opt);
    }
    for flag in &body.flags {
        register_flag_scope(&mut scope, flag);
    }
    for positional in &body.positionals.fixed {
        register_fixed_positional_scope(&mut scope, positional);
    }
    if let Some(tail) = &body.positionals.tail {
        register_tail_scope(&mut scope, tail);
    }
    scope
}

/// Resolves every [`ExtendedValueIsLiteral::Deferred`] literal in `body`'s
/// constraints against `scope`, the effective constraint scope assembled by
/// [`value_is_scope`] (and mirrored by [`check_body`]). This is the single
/// source of truth for typing a `value-is` literal: the descriptor macro carries
/// the raw literal and never re-derives a comparand graph, so resolution always
/// agrees with the validation performed by [`check_refs`].
///
/// For a name with a value-carrying comparand graph the literal is interpreted
/// into a [`SchemaValue`] and then checked against the graph with
/// [`value_is_compatible`], so a literal whose *value* is incompatible (a wrong
/// type or one that violates the option's refinements — e.g. a regex/numeric
/// bound) is rejected here rather than slipping through to a later stage. A name
/// in scope but without a comparand (a flag) is a
/// [`ToolBuildError::ValueIsTypeMismatch`]. A name not in scope is left deferred
/// — it is reported as an unresolved constraint reference by [`check_refs`] (the
/// standalone subtree-child case where the ancestor global is not present).
fn resolve_deferred_value_is(
    body: &mut ExtendedCommandBody,
    scope: &NameScope,
) -> Result<(), ToolBuildError> {
    for constraint in &mut body.constraints {
        for_each_ref_mut(constraint, &mut |r| {
            let ExtendedRef::ValueIs(v) = r else {
                return Ok(());
            };
            let ExtendedValueIsLiteral::Deferred(lit) = &v.value else {
                return Ok(());
            };
            match scope.typed.get(&v.name) {
                Some(ValueComparand::Type(comparand)) => {
                    // The structural validator owns schema soundness. If the
                    // comparand graph is unsound (a dangling or pure-alias-cycle
                    // ref) leave the literal deferred so `validate_tool` reports
                    // the real schema error instead of a cascading value-is
                    // mismatch against a graph that cannot be resolved.
                    if comparand_graph_is_sound(&comparand.graph) {
                        let value = value_is_literal_to_schema_value(&comparand.graph, lit)
                            .map_err(|_| ToolBuildError::ValueIsTypeMismatch(v.name.clone()))?;
                        if !value_is_compatible(comparand, &value) {
                            return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone()));
                        }
                        v.value = ExtendedValueIsLiteral::Resolved(value);
                    }
                }
                // A repeatable-map whose type is not a map: `validate_tool`
                // reports the malformed type. Leave the literal deferred and
                // suppress the cascading value-is mismatch.
                Some(ValueComparand::BlockedByTypeError) => {}
                // A flag (in scope, no value type) cannot carry a value-is. A
                // name not in scope is left deferred — `check_refs` reports it as
                // an unresolved constraint reference.
                None => {
                    if scope.names.contains(&v.name) {
                        return Err(ToolBuildError::ValueIsTypeMismatch(v.name.clone()));
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Applies `f` to every [`ExtendedRef`] referenced by a constraint, regardless of
/// its variant, short-circuiting on the first error.
fn for_each_ref_mut(
    constraint: &mut ExtendedConstraint,
    f: &mut impl FnMut(&mut ExtendedRef) -> Result<(), ToolBuildError>,
) -> Result<(), ToolBuildError> {
    match constraint {
        ExtendedConstraint::RequiresAll(v)
        | ExtendedConstraint::AllOrNone(v)
        | ExtendedConstraint::RequiresAny(v) => {
            for r in v.iter_mut() {
                f(r)?;
            }
        }
        ExtendedConstraint::MutexGroups(groups) => {
            for group in groups.iter_mut() {
                for r in group.refs.iter_mut() {
                    f(r)?;
                }
            }
        }
        ExtendedConstraint::Implies(i) => {
            for r in i.lhs.iter_mut() {
                f(r)?;
            }
            for r in i.rhs.iter_mut() {
                f(r)?;
            }
        }
        ExtendedConstraint::Forbids(fb) => {
            for r in fb.lhs.iter_mut() {
                f(r)?;
            }
            for r in fb.rhs.iter_mut() {
                f(r)?;
            }
        }
    }
    Ok(())
}

fn reconcile_globals(
    globals: &mut ExtendedGlobals,
    ancestors: &[EffectiveCommandField],
    command: &str,
) -> Result<(), ToolBuildError> {
    if ancestors.is_empty() {
        return Ok(());
    }
    let mut kept_options = Vec::with_capacity(globals.options.len());
    for opt in std::mem::take(&mut globals.options) {
        let shape = FieldShape::Value(option_collected_graph(&opt.shape));
        if !reconcile_local(&option_surface_names(&opt), &shape, ancestors, command)? {
            kept_options.push(opt);
        }
    }
    globals.options = kept_options;

    let mut kept_flags = Vec::with_capacity(globals.flags.len());
    for flag in std::mem::take(&mut globals.flags) {
        let shape = flag_field_shape(&flag);
        if !reconcile_local(&flag_surface_names(&flag), &shape, ancestors, command)? {
            kept_flags.push(flag);
        }
    }
    globals.flags = kept_flags;
    Ok(())
}

fn reconcile_body(
    body: &mut ExtendedCommandBody,
    ancestors: &[EffectiveCommandField],
    command: &str,
) -> Result<(), ToolBuildError> {
    if ancestors.is_empty() {
        return Ok(());
    }
    let mut kept_options = Vec::with_capacity(body.options.len());
    for opt in std::mem::take(&mut body.options) {
        let shape = FieldShape::Value(option_collected_graph(&opt.shape));
        if !reconcile_local(&option_surface_names(&opt), &shape, ancestors, command)? {
            kept_options.push(opt);
        }
    }
    body.options = kept_options;

    let mut kept_flags = Vec::with_capacity(body.flags.len());
    for flag in std::mem::take(&mut body.flags) {
        let shape = flag_field_shape(&flag);
        if !reconcile_local(&flag_surface_names(&flag), &shape, ancestors, command)? {
            kept_flags.push(flag);
        }
    }
    body.flags = kept_flags;

    let mut kept_fixed = Vec::with_capacity(body.positionals.fixed.len());
    for positional in std::mem::take(&mut body.positionals.fixed) {
        let shape = FieldShape::Value(positional.type_.clone());
        if !reconcile_local(
            std::slice::from_ref(&positional.name),
            &shape,
            ancestors,
            command,
        )? {
            kept_fixed.push(positional);
        }
    }
    body.positionals.fixed = kept_fixed;

    if let Some(tail) = body.positionals.tail.take() {
        let shape = FieldShape::Value(list_wrapper_graph(&tail.item_type));
        if !reconcile_local(std::slice::from_ref(&tail.name), &shape, ancestors, command)? {
            body.positionals.tail = Some(tail);
        }
    }

    // De-projection may have removed the parameter that was the tail (or a later
    // positional that kept an earlier `Vec<T>` out of tail position), so re-infer
    // the tail against the parameters that actually survived.
    reinfer_body_tail(body)?;
    Ok(())
}

/// Finalize a command body's tail positional after [`reconcile_body`] removed the
/// inherited re-declarations that did not survive in scope.
///
/// The macro emits only each `Vec<T>` candidate's *selected* surface into the body
/// (tail positional or repeatable-list option) and records the candidate, in
/// declaration order, in [`ExtendedCommandBody::positional_plan`] (§5.8: the
/// *last* positional `Vec<T>` is the tail, an earlier one is a repeatable-list
/// option). Because the selection assumed the macro-known inherited
/// re-declarations would de-project, this pass repairs it against the parameters
/// that actually survived. It:
///
/// * **demotes** an installed tail into a repeatable-list option (reconstructed by
///   copying the tail's value graph) when another positional survived after it —
///   rejecting an explicitly authored tail, or an inferred tail carrying
///   occurrence bounds / tail-only attributes a repeatable-list option cannot
///   represent; and
/// * **promotes** the last surviving `Vec<T>` candidate's repeatable-list option
///   into the tail (reconstructed likewise) when its natural tail was
///   de-projected — rejecting one carrying option-only attributes a tail cannot
///   represent.
///
/// It is a no-op for hand-built bodies (empty plan) and whenever the natural tail
/// is already the last surviving positional.
fn reinfer_body_tail(body: &mut ExtendedCommandBody) -> Result<(), ToolBuildError> {
    if body.positional_plan.is_empty() {
        return Ok(());
    }
    // The last surviving positional-eligible candidate, in declaration order.
    let mut last = None;
    for (idx, candidate) in body.positional_plan.iter().enumerate() {
        if body_contains_positional(body, candidate.name()) {
            last = Some(idx);
        }
    }
    let Some(last_idx) = last else { return Ok(()) };
    let last_name = body.positional_plan[last_idx].name().to_string();

    // An explicitly-authored tail that survives de-projection must be the last
    // positional-eligible candidate. If a positional survives after it, its
    // authored order is violated — whether it still holds the tail slot or was
    // lowered to an inherited-global surrogate option (a form invisible to the
    // demote path below, which only sees an installed tail). An explicit tail that
    // was de-projected entirely is gone and no longer constrains: a genuine later
    // `Vec<T>` tail may legitimately take the slot.
    if body.positional_plan[..last_idx].iter().any(|c| {
        matches!(
            c,
            PositionalCandidate::VecCandidate {
                explicit_tail: true,
                ..
            }
        ) && body_contains_positional(body, c.name())
    }) {
        return Err(ToolBuildError::FixedPositionalAfterTail(last_name));
    }

    // Demote first: an installed inferred tail whose declaration precedes a
    // surviving positional is no longer last (§5.8). Doing this before promotion
    // means that if the last candidate is itself promoted (and its option removed)
    // the demoted option is still inserted relative to the remaining later options.
    if let Some(tail_name) = body.positionals.tail.as_ref().map(|t| t.name.clone())
        && let Some(tail_idx) = body
            .positional_plan
            .iter()
            .position(|c| c.name() == tail_name)
        && tail_idx < last_idx
    {
        demote_tail_to_option(body, tail_idx)?;
    }

    // Promote: the last surviving candidate must hold the tail slot. When it is a
    // `Vec<T>` currently projected as a repeatable-list option (its natural tail
    // was de-projected), move it into the tail slot.
    if matches!(
        body.positional_plan[last_idx],
        PositionalCandidate::VecCandidate { .. }
    ) {
        promote_option_to_tail(body, last_idx)?;
    }
    Ok(())
}

/// Demote the body's installed inferred tail (the candidate at `tail_idx`) into a
/// repeatable-list option, because a positional survives after it. The option is
/// reconstructed from the tail's value graph and inserted in declaration order
/// among the body options. Rejects an inferred tail whose authored occurrence
/// bounds (`min`/`max`) or tail-only attributes a repeatable-list option cannot
/// represent. (An explicit tail before a survivor is rejected earlier, in
/// [`reinfer_body_tail`].)
fn demote_tail_to_option(
    body: &mut ExtendedCommandBody,
    tail_idx: usize,
) -> Result<(), ToolBuildError> {
    let PositionalCandidate::VecCandidate {
        name,
        has_min_or_max_attr,
        later_option_names,
        ..
    } = &body.positional_plan[tail_idx]
    else {
        return Ok(());
    };
    let name = name.clone();
    let has_min_or_max_attr = *has_min_or_max_attr;
    let later_option_names = later_option_names.clone();

    // Only act on the candidate that actually holds the tail slot.
    if body.positionals.tail.as_ref().map(|t| &t.name) != Some(&name) {
        return Ok(());
    }

    // `min`/`max` are overloaded between surfaces — occurrence bounds on a tail,
    // item numeric bounds on a repeatable-list option — so a candidate that
    // authored either cannot switch surface without changing their meaning. This
    // consults the authored fact rather than the materialized tail shape because
    // an authored `min = 0` coincides with the tail default and so leaves no trace
    // in the tail's `min`/`max` fields.
    if has_min_or_max_attr {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it authored a `min`/`max` bound, which means occurrence count \
                     on a tail positional but item count on a repeatable-list \
                     option; a parameter now follows it so it must become a \
                     repeatable-list option, which would change that meaning"
                .to_string(),
        });
    }

    let tail = body
        .positionals
        .tail
        .as_ref()
        .expect("tail name matched above");
    // A repeatable-list option has no separator/verbatim/stdio handling, so a
    // tail using any of those cannot be represented as one.
    if tail.separator.is_some() || tail.verbatim || tail.accepts_stdio {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it has a tail-only attribute (`separator`/`verbatim`/`accepts_stdio`) \
                     that a repeatable-list option cannot express, but a parameter now \
                     follows it so it must become a repeatable-list option"
                .to_string(),
        });
    }

    let tail = body
        .positionals
        .tail
        .take()
        .expect("tail name matched above");
    let option = ExtendedOptionSpec {
        long: tail.name,
        short: None,
        aliases: Vec::new(),
        doc: tail.doc,
        value_name: tail.value_name,
        shape: ExtendedOptionShape::RepeatableList(ExtendedRepeatableListShape {
            repetition: wire::Repetition::Repeated,
            item_type: tail.item_type,
        }),
        default: None,
        required: false,
        env_var: None,
    };
    // §D7: body options are in declaration order. Insert before the first option
    // declared after this `Vec<T>` that survived de-projection; otherwise append.
    let insert_at = later_option_names
        .iter()
        .find_map(|later| body.options.iter().position(|o| &o.long == later));
    match insert_at {
        Some(pos) => body.options.insert(pos, option),
        None => body.options.push(option),
    }
    Ok(())
}

/// Promote the last surviving `Vec<T>` candidate (at `last_idx`) from a
/// repeatable-list option into the tail positional, because its natural tail was
/// de-projected. An explicit tail lowered to an inherited-global surrogate option
/// installs its full authored spec (`authored_tail_surrogate`) verbatim, so its
/// tail-only attributes are preserved; an inferred candidate is reconstructed
/// from the option's value graph instead. A no-op if it already holds the tail
/// slot or never projected to an option; rejects an inferred candidate carrying
/// option-only attributes (or an `Option<Vec<T>>`, or authored `min`/`max`) that
/// a tail cannot represent.
fn promote_option_to_tail(
    body: &mut ExtendedCommandBody,
    last_idx: usize,
) -> Result<(), ToolBuildError> {
    let PositionalCandidate::VecCandidate {
        name,
        optional_vec,
        has_min_or_max_attr,
        authored_tail_surrogate,
        ..
    } = &body.positional_plan[last_idx]
    else {
        return Ok(());
    };
    let name = name.clone();
    let optional_vec = *optional_vec;
    let has_min_or_max_attr = *has_min_or_max_attr;
    let authored_tail_surrogate = authored_tail_surrogate.clone();

    // Already the tail (its natural tail survived): nothing to do.
    if body
        .positionals
        .tail
        .as_ref()
        .is_some_and(|t| t.name == name)
    {
        return Ok(());
    }
    let Some(pos) = body.options.iter().position(|o| o.long == name) else {
        return Ok(());
    };

    // An explicit tail lowered to an inherited-global surrogate option: install
    // its full authored spec verbatim. The surrogate option carries none of the
    // tail-only fields (`separator`/`verbatim`/`accepts_stdio`/occurrence bounds),
    // so reconstructing from it would silently drop them; the authored spec keeps
    // them (and any resulting invalid state, e.g. `verbatim` without `separator`,
    // is caught later by `validate_tool`). `min`/`max` are already occurrence
    // bounds here, so the inferred-only `has_min_or_max_attr` rejection is skipped.
    if let Some(authored_tail) = authored_tail_surrogate {
        // Defensive against hand-built bodies: confirm the option really is the
        // droppable repeatable-list surrogate before replacing it with the tail.
        verify_promotable_surrogate(&body.options[pos], &name)?;
        body.options.remove(pos);
        body.positionals.tail = Some(*authored_tail);
        return Ok(());
    }

    // A tail is variadic with no absent state and interprets `min`/`max` as
    // occurrence bounds, so these authored shapes cannot move onto a tail.
    if optional_vec {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it is an `Option<Vec<T>>`, which has no tail-positional \
                     representation, but de-projection made it the last positional \
                     so it must become the tail"
                .to_string(),
        });
    }
    if has_min_or_max_attr {
        return Err(ToolBuildError::VecSurfaceConflict {
            name,
            reason: "it has a `min`/`max` bound applied to its items as a \
                     repeatable-list option, but de-projection made it the last \
                     positional so it must become the tail (where `min`/`max` would \
                     instead bound the occurrence count)"
                .to_string(),
        });
    }
    verify_promotable_surrogate(&body.options[pos], &name)?;

    let option = body.options.remove(pos);
    let item_type = match option.shape {
        ExtendedOptionShape::RepeatableList(list) => list.item_type,
        _ => unreachable!("shape checked as RepeatableList above"),
    };
    body.positionals.tail = Some(ExtendedTailPositional {
        name: option.long,
        doc: option.doc,
        value_name: option.value_name,
        item_type,
        min: 0,
        max: None,
        separator: None,
        verbatim: false,
        accepts_stdio: false,
    });
    Ok(())
}

/// Verify that `option` is a droppable repeatable-list surrogate that can take the
/// tail slot: it must carry no option-only surface a tail positional cannot
/// express (`short`/`aliases`/`default`/`required`/`env`), and must be a
/// `RepeatableList` with `Repeated` (non-delimited) repetition.
fn verify_promotable_surrogate(
    option: &ExtendedOptionSpec,
    name: &str,
) -> Result<(), ToolBuildError> {
    if option.short.is_some()
        || !option.aliases.is_empty()
        || option.default.is_some()
        || option.required
        || option.env_var.is_some()
    {
        return Err(ToolBuildError::VecSurfaceConflict {
            name: name.to_string(),
            reason: "it has an option-only attribute (`short`/`aliases`/`default`/\
                     `required`/`env`) that a tail positional cannot express, but \
                     de-projection made it the last positional so it must become the tail"
                .to_string(),
        });
    }
    let ExtendedOptionShape::RepeatableList(list) = &option.shape else {
        return Err(ToolBuildError::VecSurfaceConflict {
            name: name.to_string(),
            reason: "it does not project to a repeatable list, so it has no \
                     tail-positional representation"
                .to_string(),
        });
    };
    if !matches!(list.repetition, wire::Repetition::Repeated) {
        return Err(ToolBuildError::VecSurfaceConflict {
            name: name.to_string(),
            reason: "it uses a delimited repetition that a tail positional cannot \
                     express, but de-projection made it the last positional so it \
                     must become the tail"
                .to_string(),
        });
    }
    Ok(())
}

/// Whether the body still carries a positional-eligible parameter with the given
/// surface name (as a fixed positional, the tail, or a repeatable-list option),
/// i.e. it survived de-projection.
fn body_contains_positional(body: &ExtendedCommandBody, name: &str) -> bool {
    body.positionals.fixed.iter().any(|p| p.name == name)
        || body
            .positionals
            .tail
            .as_ref()
            .is_some_and(|t| t.name == name)
        || body.options.iter().any(|o| o.long == name)
}

/// Decide the fate of one local declaration against the inherited globals in
/// scope. Returns `Ok(true)` when the local is a compatible re-declaration that
/// should be removed (the inherited global covers it), `Ok(false)` when no
/// inherited global shares a surface name (keep the local), and
/// [`ToolBuildError::InheritedGlobalConflict`] when a surface name matches but
/// the input shapes are incompatible.
fn reconcile_local(
    local_names: &[String],
    local_shape: &FieldShape,
    ancestors: &[EffectiveCommandField],
    command: &str,
) -> Result<bool, ToolBuildError> {
    // A local may share a surface name with more than one inherited global (its
    // long name with one, an alias with another). All ancestors are scanned so
    // that an incompatible collision — even one found after a compatible one — is
    // reported immediately as a conflict. The local can be de-projected onto an
    // inherited global only when it collides with exactly one *distinct* inherited
    // global (matching the same global through both its long name and an alias is
    // a single ancestor entry, hence still one global). Colliding compatibly with
    // two or more distinct inherited globals is ambiguous — there is no single
    // global to inherit from, and silently dropping the local would leave the Rust
    // parameter with no canonical body field — so that is also a conflict.
    let mut compatible: Vec<String> = Vec::new();
    for inherited in ancestors {
        let inherited_names = effective_field_surface_names(inherited);
        let Some(colliding) = local_names
            .iter()
            .find(|l| inherited_names.iter().any(|n| n == *l))
        else {
            continue;
        };
        if !field_shapes_compatible(&effective_field_shape(inherited), local_shape) {
            return Err(ToolBuildError::InheritedGlobalConflict {
                name: colliding.clone(),
                inherited: effective_field_primary_name(inherited),
                command: command.to_string(),
            });
        }
        let primary = effective_field_primary_name(inherited);
        if !compatible.iter().any(|p| p == &primary) {
            compatible.push(primary);
        }
    }
    if compatible.len() > 1 {
        return Err(ToolBuildError::InheritedGlobalConflict {
            name: local_names.first().cloned().unwrap_or_default(),
            inherited: compatible.join(", "),
            command: command.to_string(),
        });
    }
    Ok(!compatible.is_empty())
}

/// The primary (long) surface name of an inherited effective global, used to
/// name the colliding global in [`ToolBuildError::InheritedGlobalConflict`].
fn effective_field_primary_name(g: &EffectiveCommandField) -> String {
    match g {
        EffectiveCommandField::Option(o) => o.long.clone(),
        EffectiveCommandField::Flag(f) => f.long.clone(),
    }
}

/// The canonical input "surface family" of a command field, used to decide
/// whether a local re-declaration is compatible with an inherited global. Flags
/// are distinguished by their flag family (a bool flag and a count flag carry
/// different values, and neither is interchangeable with a value-bearing option
/// or positional of the same name); every value-bearing form (scalar/optional
/// option, repeatable list/map option, fixed positional, tail positional) is
/// compared by its canonical input value graph.
#[allow(clippy::large_enum_variant)]
enum FieldShape {
    BoolFlag,
    CountFlag,
    Value(SchemaGraph),
}

fn flag_field_shape(f: &FlagSpec) -> FieldShape {
    match f.shape {
        wire::FlagShape::BoolFlag(_) => FieldShape::BoolFlag,
        wire::FlagShape::CountFlag(_) => FieldShape::CountFlag,
    }
}

fn effective_field_shape(g: &EffectiveCommandField) -> FieldShape {
    match g {
        EffectiveCommandField::Option(o) => FieldShape::Value(option_collected_graph(&o.shape)),
        EffectiveCommandField::Flag(f) => flag_field_shape(f),
    }
}

fn field_shapes_compatible(a: &FieldShape, b: &FieldShape) -> bool {
    match (a, b) {
        (FieldShape::BoolFlag, FieldShape::BoolFlag) => true,
        (FieldShape::CountFlag, FieldShape::CountFlag) => true,
        (FieldShape::Value(ga), FieldShape::Value(gb)) => schema_shapes_match(ga, gb),
        _ => false,
    }
}

fn option_surface_names(o: &ExtendedOptionSpec) -> Vec<String> {
    let mut names = Vec::with_capacity(1 + o.aliases.len());
    names.push(o.long.clone());
    names.extend(o.aliases.iter().cloned());
    names
}

fn flag_surface_names(f: &FlagSpec) -> Vec<String> {
    let mut names = Vec::with_capacity(1 + f.aliases.len());
    names.push(f.long.clone());
    names.extend(f.aliases.iter().cloned());
    names
}

fn effective_field_surface_names(g: &EffectiveCommandField) -> Vec<String> {
    match g {
        EffectiveCommandField::Option(o) => option_surface_names(o),
        EffectiveCommandField::Flag(f) => flag_surface_names(f),
    }
}

/// Maximum recursion depth for structural shape comparison; deeper than this the
/// comparison gives up and reports "not a match". This keeps the comparison
/// terminating on pathological (deeply nested or recursive) types. Reporting
/// "not a match" on exhaustion is the safe direction: a non-match between two
/// same-named declarations surfaces as an explicit
/// [`ToolBuildError::InheritedGlobalConflict`] rather than silently dropping a
/// local parameter that might actually differ. Real CLI input schemas (strings,
/// numbers, bools, lists, maps, small records) are far shallower than this.
const SHAPE_MATCH_MAX_DEPTH: u32 = 32;

/// Whether two canonical input value graphs describe the same value *shape*,
/// ignoring metadata and validation restrictions (docs, numeric/text bounds,
/// etc.) but honoring structure and exact primitive representation. References
/// are resolved against their respective graphs.
///
/// Recursive (cyclic) graphs are compared coinductively: when the same pair of
/// referenced definitions is reached again along a path, the two shapes are
/// assumed to match (the cycle has already been established structurally). This
/// makes an identical recursive type re-declaration compare equal instead of
/// being falsely rejected once the structural recursion bottoms out. The
/// per-pair memo is what guarantees termination; the depth counter is a defensive
/// secondary guard for pathologically deep finite types.
fn schema_shapes_match(a: &SchemaGraph, b: &SchemaGraph) -> bool {
    let mut visiting = std::collections::HashSet::new();
    schema_types_match(a, &a.root, b, &b.root, SHAPE_MATCH_MAX_DEPTH, &mut visiting)
}

fn schema_types_match(
    a_graph: &SchemaGraph,
    a_ty: &SchemaType,
    b_graph: &SchemaGraph,
    b_ty: &SchemaType,
    depth: u32,
    visiting: &mut std::collections::HashSet<(crate::schema::TypeId, crate::schema::TypeId)>,
) -> bool {
    // Break recursion at reference boundaries before resolving: revisiting the
    // same pair of named definitions means we have already entered comparing
    // them, so the recursive shapes coincide along this path.
    if let (SchemaType::Ref { id: a_id, .. }, SchemaType::Ref { id: b_id, .. }) = (a_ty, b_ty)
        && !visiting.insert((a_id.clone(), b_id.clone()))
    {
        return true;
    }
    let (Ok(a_ty), Ok(b_ty)) = (a_graph.resolve_ref(a_ty), b_graph.resolve_ref(b_ty)) else {
        return false;
    };
    if depth == 0 {
        return false;
    }
    let next = depth - 1;
    match (a_ty, b_ty) {
        (SchemaType::List { element: ea, .. }, SchemaType::List { element: eb, .. }) => {
            schema_types_match(a_graph, ea, b_graph, eb, next, visiting)
        }
        (
            SchemaType::FixedList {
                element: ea,
                length: la,
                ..
            },
            SchemaType::FixedList {
                element: eb,
                length: lb,
                ..
            },
        ) => la == lb && schema_types_match(a_graph, ea, b_graph, eb, next, visiting),
        (SchemaType::Option { inner: ia, .. }, SchemaType::Option { inner: ib, .. }) => {
            schema_types_match(a_graph, ia, b_graph, ib, next, visiting)
        }
        (
            SchemaType::Map {
                key: ka, value: va, ..
            },
            SchemaType::Map {
                key: kb, value: vb, ..
            },
        ) => {
            schema_types_match(a_graph, ka, b_graph, kb, next, visiting)
                && schema_types_match(a_graph, va, b_graph, vb, next, visiting)
        }
        (SchemaType::Tuple { elements: ea, .. }, SchemaType::Tuple { elements: eb, .. }) => {
            ea.len() == eb.len()
                && ea
                    .iter()
                    .zip(eb)
                    .all(|(x, y)| schema_types_match(a_graph, x, b_graph, y, next, visiting))
        }
        (SchemaType::Record { fields: fa, .. }, SchemaType::Record { fields: fb, .. }) => {
            fa.len() == fb.len()
                && fa.iter().zip(fb).all(|(x, y)| {
                    x.name == y.name
                        && schema_types_match(a_graph, &x.body, b_graph, &y.body, next, visiting)
                })
        }
        (SchemaType::Variant { cases: ca, .. }, SchemaType::Variant { cases: cb, .. }) => {
            ca.len() == cb.len()
                && ca.iter().zip(cb).all(|(x, y)| {
                    x.name == y.name
                        && match (&x.payload, &y.payload) {
                            (None, None) => true,
                            (Some(px), Some(py)) => {
                                schema_types_match(a_graph, px, b_graph, py, next, visiting)
                            }
                            _ => false,
                        }
                })
        }
        (SchemaType::Union { spec: sa, .. }, SchemaType::Union { spec: sb, .. }) => {
            sa.branches.len() == sb.branches.len()
                && sa.branches.iter().zip(&sb.branches).all(|(x, y)| {
                    x.tag == y.tag
                        && x.discriminator == y.discriminator
                        && schema_types_match(a_graph, &x.body, b_graph, &y.body, next, visiting)
                })
        }
        (SchemaType::Enum { cases: ca, .. }, SchemaType::Enum { cases: cb, .. }) => ca == cb,
        (SchemaType::Flags { flags: fa, .. }, SchemaType::Flags { flags: fb, .. }) => fa == fb,
        // Rich leaf types whose spec carries *type identity* (not just refinable
        // validation restrictions) must compare those identity fields. Otherwise
        // a leaf could de-project an inherited global onto a genuinely different
        // Rust type — e.g. `Quantity<Meters>` vs `Quantity<Seconds>`, which
        // share the `Quantity` discriminant but are not interchangeable values.
        // Identity here means the parts of the spec derived from the Rust type
        // (not overlaid by `#[arg]`): the quantity unit set, the secret payload
        // type and category, the quota-token resource, and the unstructured text
        // languages / binary MIME sets. Pure validation restrictions
        // (numeric/text/url bounds, path direction/kind — all `#[arg]`-refinable)
        // stay ignored per this function's contract.
        (SchemaType::Quantity { spec: sa, .. }, SchemaType::Quantity { spec: sb, .. }) => {
            sa.base_unit == sb.base_unit
                && str_sets_match(
                    &effective_quantity_units(&sa.base_unit, &sa.allowed_suffixes),
                    &effective_quantity_units(&sb.base_unit, &sb.allowed_suffixes),
                )
        }
        (SchemaType::Secret { spec: sa, .. }, SchemaType::Secret { spec: sb, .. }) => {
            sa.category == sb.category
                && schema_types_match(a_graph, &sa.inner, b_graph, &sb.inner, next, visiting)
        }
        (SchemaType::QuotaToken { spec: sa, .. }, SchemaType::QuotaToken { spec: sb, .. }) => {
            sa.resource_name == sb.resource_name
        }
        (
            SchemaType::Text {
                restrictions: ra, ..
            },
            SchemaType::Text {
                restrictions: rb, ..
            },
        ) => opt_str_sets_match(&ra.languages, &rb.languages),
        (
            SchemaType::Binary {
                restrictions: ra, ..
            },
            SchemaType::Binary {
                restrictions: rb, ..
            },
        ) => opt_str_sets_match(&ra.mime_types, &rb.mime_types),
        // A plain `String` and a `Text` differ only by the latter carrying
        // refinable restrictions (regex/min/max), which `#[arg]` overlays via
        // `refine_text` (the only `String`→`Text` promotion). So an inherited
        // refined-`String` global (`Text`) and a leaf redeclaring the same plain
        // `String` describe the same shape and must de-project. The exception is
        // a `languages`-restricted `Text`, which reflects a different Rust type
        // (`UnstructuredText<…>`) and stays incompatible with a plain `String`.
        (SchemaType::String { .. }, SchemaType::Text { restrictions, .. })
        | (SchemaType::Text { restrictions, .. }, SchemaType::String { .. }) => {
            restrictions.languages.is_none()
        }
        (SchemaType::Result { spec: sa, .. }, SchemaType::Result { spec: sb, .. }) => {
            schema_opt_box_match(
                a_graph,
                sa.ok.as_deref(),
                b_graph,
                sb.ok.as_deref(),
                next,
                visiting,
            ) && schema_opt_box_match(
                a_graph,
                sa.err.as_deref(),
                b_graph,
                sb.err.as_deref(),
                next,
                visiting,
            )
        }
        (SchemaType::Future { inner: ia, .. }, SchemaType::Future { inner: ib, .. })
        | (SchemaType::Stream { inner: ia, .. }, SchemaType::Stream { inner: ib, .. }) => {
            schema_opt_box_match(
                a_graph,
                ia.as_deref(),
                b_graph,
                ib.as_deref(),
                next,
                visiting,
            )
        }
        // Primitives and the remaining rich leaf types (incl. distinct numeric
        // widths/signs, `Url`, `Path`, `Datetime`, `Duration`) are compared by
        // kind, which already ignores their refinable restrictions.
        _ => std::mem::discriminant(a_ty) == std::mem::discriminant(b_ty),
    }
}

/// Whether two string collections describe the same *set* of values (order- and
/// duplicate-insensitive). Used to compare rich-type identity fields that are
/// authored as ordered lists but semantically unordered (quantity units, allowed
/// languages, allowed MIME types).
fn str_sets_match(a: &[String], b: &[String]) -> bool {
    let sa: std::collections::HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: std::collections::HashSet<&str> = b.iter().map(String::as_str).collect();
    sa == sb
}

/// Set comparison for an optional restriction (`None` = unrestricted). An
/// unrestricted side never matches a restricted side: they describe different
/// accepted value sets.
fn opt_str_sets_match(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => str_sets_match(a, b),
        _ => false,
    }
}

/// The set of units a [`SchemaType::Quantity`] accepts: its explicit
/// `allowed_suffixes`, or just the canonical `base_unit` when no suffixes are
/// declared. Two quantities with the same base unit but different accepted unit
/// sets are not interchangeable, so de-projection must treat them as distinct.
fn effective_quantity_units(base_unit: &str, allowed_suffixes: &[String]) -> Vec<String> {
    if allowed_suffixes.is_empty() {
        vec![base_unit.to_string()]
    } else {
        allowed_suffixes.to_vec()
    }
}

fn schema_opt_box_match(
    a_graph: &SchemaGraph,
    a_ty: Option<&SchemaType>,
    b_graph: &SchemaGraph,
    b_ty: Option<&SchemaType>,
    depth: u32,
    visiting: &mut std::collections::HashSet<(crate::schema::TypeId, crate::schema::TypeId)>,
) -> bool {
    match (a_ty, b_ty) {
        (None, None) => true,
        (Some(x), Some(y)) => schema_types_match(a_graph, x, b_graph, y, depth, visiting),
        _ => false,
    }
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
    use crate::agentic::tool_refinement::{refine_numeric, refine_path, refine_text, refine_url};
    use crate::schema::schema_type::{NumericBound, NumericRestrictions};
    use crate::schema::{
        BinaryRestrictions, PathDirection, PathKind, PathSpec, QuantitySpec, QuotaTokenSpec,
        SecretSpec, TextRestrictions,
    };
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
                        positional_plan: vec![],
                    }),
                },
            ],
        }
    }

    #[test]
    fn canonical_input_model_rejects_conflicting_type_ids_across_fields() {
        use crate::schema::{SchemaTypeDef, TypeId};

        // Two canonical input fields carry a definition with the same id but
        // different bodies: the record schema construction must fail rather
        // than silently pick either body.
        let mut tool = sample_tool();
        let body = tool.commands[1].body.as_mut().unwrap();
        body.positionals.fixed[0].type_ = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: TypeId::from("conflicting"),
                name: None,
                body: SchemaType::string(),
            }],
            root: SchemaType::ref_to(TypeId::from("conflicting")),
        };
        body.options[0].shape = ExtendedOptionShape::Scalar(SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: TypeId::from("conflicting"),
                name: None,
                body: SchemaType::u32(),
            }],
            root: SchemaType::ref_to(TypeId::from("conflicting")),
        });

        // The per-field graphs are still the author-provided ones.
        let fields = tool.canonical_input_fields(1);
        let input = fields.iter().find(|f| f.name == "input").unwrap();
        assert_eq!(
            input.schema.root,
            SchemaType::ref_to(TypeId::from("conflicting"))
        );
        assert_eq!(input.schema.defs[0].body, SchemaType::string());
        let config = fields.iter().find(|f| f.name == "config").unwrap();
        assert_eq!(config.schema.defs[0].body, SchemaType::u32());

        assert!(tool.canonical_input_model(1).is_err());
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
    fn canonical_input_model_builds_record_schema_in_field_order() {
        let tool = sample_tool();
        let model = tool.canonical_input_model(1).unwrap();

        assert_eq!(
            model
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["verbose", "input", "config", "force"]
        );
        let SchemaType::Record { fields, .. } = &model.record_schema.root else {
            panic!("canonical input schema must be a record")
        };
        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["verbose", "input", "config", "force"]
        );
    }

    #[test]
    fn canonical_input_model_decodes_positional_record_by_index() {
        let tool = sample_tool();
        let model = tool.canonical_input_model(1).unwrap();
        let decoded = model
            .decode_record(SchemaValue::Record {
                fields: vec![
                    SchemaValue::U32(3),
                    SchemaValue::String("in.txt".to_string()),
                    SchemaValue::Map { entries: vec![] },
                    SchemaValue::Bool(true),
                ],
            })
            .unwrap();

        assert_eq!(
            decoded
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["verbose", "input", "config", "force"]
        );
        assert_eq!(decoded[0].value, SchemaValue::U32(3));
        assert_eq!(decoded[1].value, SchemaValue::String("in.txt".to_string()));
        assert_eq!(decoded[3].value, SchemaValue::Bool(true));
    }

    #[test]
    fn canonical_input_model_rejects_non_record_and_wrong_field_count() {
        let tool = sample_tool();
        let model = tool.canonical_input_model(1).unwrap();

        assert_eq!(
            model.decode_record(SchemaValue::String("nope".to_string())),
            Err(CanonicalInputDecodeError::ExpectedRecord)
        );
        assert_eq!(
            model.decode_record(SchemaValue::Record { fields: vec![] }),
            Err(CanonicalInputDecodeError::FieldCountMismatch {
                expected: 4,
                actual: 0,
            })
        );
    }

    #[test]
    fn canonical_input_model_rejects_out_of_bounds_command_index() {
        let tool = sample_tool();

        assert_eq!(
            tool.canonical_input_model(99).unwrap_err(),
            ToolBuildError::CommandIndexOutOfBounds { index: 99, len: 2 }
        );
        assert_eq!(
            tool.canonical_input_record_schema(99).unwrap_err(),
            ToolBuildError::CommandIndexOutOfBounds { index: 99, len: 2 }
        );
        assert_eq!(
            tool.decode_canonical_input_record(99, SchemaValue::Record { fields: vec![] }),
            Err(CanonicalInputDecodeError::Model(
                ToolBuildError::CommandIndexOutOfBounds { index: 99, len: 2 }
            ))
        );
    }

    #[test]
    fn canonical_input_model_validates_synthesized_record_schema() {
        let error = CanonicalInputModel::from_fields(vec![
            CanonicalInputField {
                name: "same".to_string(),
                aliases: Vec::new(),
                schema: str_graph(),
            },
            CanonicalInputField {
                name: "same".to_string(),
                aliases: Vec::new(),
                schema: u32_graph(),
            },
        ])
        .unwrap_err();

        assert!(
            matches!(error, ToolBuildError::IllFormedSchema { .. }),
            "expected synthesized record validation error, got {error:?}"
        );
    }

    #[test]
    fn canonical_input_model_rejects_field_schema_with_duplicate_identical_defs() {
        let id = crate::schema::TypeId::new("dup");
        let graph = SchemaGraph {
            defs: vec![
                crate::schema::SchemaTypeDef {
                    id: id.clone(),
                    name: Some("first".to_string()),
                    body: SchemaType::string(),
                },
                crate::schema::SchemaTypeDef {
                    id: id.clone(),
                    name: Some("second".to_string()),
                    body: SchemaType::string(),
                },
            ],
            root: SchemaType::ref_to(id),
        };

        assert!(crate::schema::validation::validate_graph(&graph).is_err());
        let error = CanonicalInputModel::from_fields(vec![CanonicalInputField {
            name: "field".to_string(),
            aliases: Vec::new(),
            schema: graph,
        }])
        .unwrap_err();

        assert!(
            matches!(error, ToolBuildError::IllFormedSchema { .. }),
            "expected duplicate type id validation error, got {error:?}"
        );
    }

    /// Builds a two-node tool: a root carrying one inherited global option, and a
    /// single `leaf` body whose only option is described by `(long, aliases)`.
    /// Used to exercise alias-aware body-vs-inherited-global shadowing on an
    /// unnormalized descriptor.
    fn tool_with_inherited_global_and_body_option(
        global_long: &str,
        global_aliases: Vec<String>,
        body_long: &str,
        body_aliases: Vec<String>,
    ) -> ExtendedToolType {
        ExtendedToolType {
            version: "0.1.0".to_string(),
            commands: vec![
                ExtendedCommandNode {
                    name: "root".to_string(),
                    aliases: vec![],
                    doc: doc("root"),
                    globals: ExtendedGlobals {
                        options: vec![ExtendedOptionSpec {
                            long: global_long.to_string(),
                            short: None,
                            aliases: global_aliases,
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
                    name: "leaf".to_string(),
                    aliases: vec![],
                    doc: doc("leaf"),
                    globals: ExtendedGlobals::default(),
                    subcommands: vec![],
                    body: Some(ExtendedCommandBody {
                        positionals: ExtendedPositionals::default(),
                        options: vec![ExtendedOptionSpec {
                            long: body_long.to_string(),
                            short: None,
                            aliases: body_aliases,
                            doc: doc("body"),
                            value_name: None,
                            shape: ExtendedOptionShape::Scalar(str_graph()),
                            default: None,
                            required: false,
                            env_var: None,
                        }],
                        flags: vec![],
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        result: None,
                        errors: vec![],
                        annotations: None,
                        positional_plan: vec![],
                    }),
                },
            ],
        }
    }

    #[test]
    fn canonical_input_fields_body_alias_shadows_inherited_global() {
        // A body option whose ALIAS equals an inherited global's long name
        // shadows that global on an unnormalized descriptor.
        let tool = tool_with_inherited_global_and_body_option(
            "verbose",
            vec![],
            "local",
            vec!["verbose".to_string()],
        );
        let names: Vec<_> = tool
            .canonical_input_fields(1)
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(
            names,
            vec!["local"],
            "an inherited global must be shadowed by a body option aliased to its name"
        );
    }

    #[test]
    fn canonical_input_fields_inherited_global_alias_is_shadowed() {
        // The reverse: an inherited global whose ALIAS equals a body option's
        // long name is shadowed too.
        let tool = tool_with_inherited_global_and_body_option(
            "global",
            vec!["v".to_string()],
            "v",
            vec![],
        );
        let names: Vec<_> = tool
            .canonical_input_fields(1)
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(
            names,
            vec!["v"],
            "an inherited global aliased to a body option's name must be shadowed"
        );
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
    fn graft_accepts_root_with_body() {
        // A grafted subtree root may carry its own body (the child trait's
        // implicit-body method). With no parent globals, the body survives
        // unchanged.
        let graft = graft_subtree(
            leaf_tool_with_body(empty_body()),
            "t",
            ExtendedGlobals::default(),
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(graft[0].body.is_some(), "grafted root keeps its body");
    }

    #[test]
    fn graft_deprojects_child_body_against_parent_globals() {
        // The child root body re-declares `verbose` as a string option, which is
        // compatible with the parent global of the same name; it is de-projected
        // and the parent global is the single source of truth.
        let parent_globals = ExtendedGlobals {
            options: vec![scalar_opt("verbose", None)],
            flags: vec![],
        };
        let mut child_body = empty_body();
        child_body.options = vec![scalar_opt("verbose", None)];
        child_body.options[0].doc = doc("local verbose");
        let graft = graft_subtree(
            leaf_tool_with_body(child_body),
            "t",
            parent_globals,
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let body = graft[0].body.as_ref().expect("body preserved");
        assert!(
            !body.options.iter().any(|o| o.long == "verbose"),
            "compatible body option is de-projected onto the parent global"
        );
        assert!(
            graft[0].globals.options.iter().any(|o| o.long == "verbose"),
            "parent global is prepended onto the grafted root globals"
        );
    }

    #[test]
    fn graft_rejects_incompatible_child_body_vs_parent_global() {
        // The child root body declares `verbose` as a string option, but the
        // parent global is a bool flag of the same name: incompatible shapes
        // are an `InheritedGlobalConflict`, reported against the final grafted
        // command name.
        let parent_globals = ExtendedGlobals {
            options: vec![],
            flags: vec![bool_flag("verbose", None)],
        };
        let mut child_body = empty_body();
        child_body.options = vec![scalar_opt("verbose", None)];
        let err = graft_subtree(
            leaf_tool_with_body(child_body),
            "t",
            parent_globals,
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ToolBuildError::InheritedGlobalConflict { name, command, .. }
                if name == "verbose" && command == "t"
        ));
    }

    /// A single-root child whose root carries `globals` (no body). Used to
    /// exercise graft-time reconciliation of the grafted root's own globals
    /// against `parent_globals`.
    fn leaf_tool_with_globals(globals: ExtendedGlobals) -> ExtendedToolType {
        ExtendedToolType {
            version: "0.1.0".to_string(),
            commands: vec![ExtendedCommandNode {
                name: "t".to_string(),
                aliases: vec![],
                doc: doc(""),
                globals,
                subcommands: vec![],
                body: None,
            }],
        }
    }

    #[test]
    fn graft_deprojects_child_root_globals_against_parent_globals() {
        // The child root re-declares `verbose` as its own global (e.g. the
        // standalone child trait propagating it to its own descendants). It is
        // compatible with the parent global of the same name, so it is
        // de-projected and the prepended parent global is the single source.
        let parent_globals = ExtendedGlobals {
            options: vec![],
            flags: vec![bool_flag("verbose", None)],
        };
        let child_globals = ExtendedGlobals {
            options: vec![],
            flags: vec![bool_flag("verbose", None)],
        };
        let graft = graft_subtree(
            leaf_tool_with_globals(child_globals),
            "t",
            parent_globals,
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let verbose_count = graft[0]
            .globals
            .flags
            .iter()
            .filter(|f| f.long == "verbose")
            .count();
        assert_eq!(
            verbose_count, 1,
            "compatible child-root global is de-projected; exactly one verbose global remains"
        );
    }

    #[test]
    fn graft_rejects_incompatible_child_root_global_vs_parent_global() {
        // The child root global `verbose` (string option) is incompatible with
        // the parent global `verbose` (bool flag): `InheritedGlobalConflict`.
        let parent_globals = ExtendedGlobals {
            options: vec![],
            flags: vec![bool_flag("verbose", None)],
        };
        let child_globals = ExtendedGlobals {
            options: vec![scalar_opt("verbose", None)],
            flags: vec![],
        };
        let err = graft_subtree(
            leaf_tool_with_globals(child_globals),
            "t",
            parent_globals,
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ToolBuildError::InheritedGlobalConflict { name, command, .. }
                if name == "verbose" && command == "t"
        ));
    }

    #[test]
    fn graft_preserves_local_indices() {
        // The child root stays at index 0; its graft-local subcommand index (1)
        // is unchanged.
        let graft = graft_subtree(
            dispatcher_child(),
            "child",
            ExtendedGlobals::default(),
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap();
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
        let mismatch = graft_subtree(
            dispatcher_child(),
            "remote",
            ExtendedGlobals::default(),
            &[],
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            mismatch,
            ToolBuildError::SubtreeRootNameMismatch { .. }
        ));

        // An explicit override name bypasses the match rule.
        let ok = graft_subtree(
            dispatcher_child(),
            "remote",
            ExtendedGlobals::default(),
            &[],
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
            ExtendedGlobals::default(),
            &[],
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
        let text = refine_text(SchemaType::string(), Some("x+".into()), Some(1), Some(3)).unwrap();
        assert!(matches!(text, SchemaType::Text { .. }));
        let url = refine_url(
            SchemaType::url(Default::default()),
            Some(vec!["https".into()]),
        )
        .unwrap();
        assert!(matches!(url, SchemaType::Url { .. }));
        let num = refine_numeric(
            SchemaType::u32(),
            Some(NumericBound::Unsigned(1)),
            None,
            Some("ms".into()),
        )
        .unwrap();
        assert_eq!(
            num.numeric_restrictions().unwrap().unit.as_deref(),
            Some("ms")
        );

        // Refinements reject schema kinds that cannot carry their restrictions
        // (the runtime backstop for macro-opaque types).
        assert!(matches!(
            refine_numeric(SchemaType::string(), None, None, Some("ms".into())),
            Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "numeric",
                ..
            })
        ));
        assert!(matches!(
            refine_path(SchemaType::string(), None, None, None),
            Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "path",
                ..
            })
        ));
        assert!(matches!(
            refine_url(SchemaType::string(), Some(vec!["https".into()])),
            Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "url",
                ..
            })
        ));
        assert!(matches!(
            refine_text(SchemaType::u32(), Some("x+".into()), None, None),
            Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "text",
                ..
            })
        ));
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
        )
        .unwrap();
        let refined = refine_numeric(base, None, Some(NumericBound::Unsigned(20)), None).unwrap();
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

        let refined = refine_numeric(base, None, Some(NumericBound::Unsigned(200)), None).unwrap();

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
            positional_plan: vec![],
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
                value: ExtendedValueIsLiteral::Resolved(SchemaValue::U32(1)),
            })],
        )]));
        assert!(ok.try_to_tool().is_ok());

        let bad = leaf_tool_with_body(map_config_option(vec![ExtendedConstraint::RequiresAll(
            vec![ExtendedRef::ValueIs(ExtendedValueIsRef {
                name: "config".to_string(),
                value: ExtendedValueIsLiteral::Resolved(SchemaValue::String("x".to_string())),
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

    /// Builds a parent-dispatcher + child-leaf tool where the child's body carries
    /// `constraints` referencing names that only exist on the parent's globals (a
    /// subtree-style composition). The parent declares the given `parent_globals`.
    fn deferred_value_is_tree(
        parent_globals: ExtendedGlobals,
        constraints: Vec<ExtendedConstraint>,
    ) -> ExtendedToolType {
        let mut child_body = empty_body();
        child_body.constraints = constraints;
        tool_with_nodes(vec![
            ExtendedCommandNode {
                name: "root".into(),
                aliases: vec![],
                doc: doc(""),
                globals: parent_globals,
                subcommands: vec![1],
                body: None,
            },
            ExtendedCommandNode {
                name: "leaf".into(),
                aliases: vec![],
                doc: doc(""),
                globals: ExtendedGlobals::default(),
                subcommands: vec![],
                body: Some(child_body),
            },
        ])
    }

    fn first_value_is(body: &ExtendedCommandBody) -> &ExtendedValueIsRef {
        match &body.constraints[0] {
            ExtendedConstraint::RequiresAll(refs) => match &refs[0] {
                ExtendedRef::ValueIs(v) => v,
                other => panic!("expected a value-is ref, got {other:?}"),
            },
            other => panic!("expected a requires-all constraint, got {other:?}"),
        }
    }

    #[test]
    fn deferred_value_is_resolves_against_ancestor_global() {
        let mut tool = deferred_value_is_tree(
            ExtendedGlobals {
                options: vec![scalar_opt("format", None)],
                flags: vec![],
            },
            vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
                ExtendedValueIsRef {
                    name: "format".into(),
                    value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("json".into())),
                },
            )])],
        );
        normalize_inherited_globals(&mut tool)
            .expect("composition should resolve the deferred literal against the parent global");
        let resolved = first_value_is(tool.commands[1].body.as_ref().unwrap());
        assert!(
            matches!(&resolved.value, ExtendedValueIsLiteral::Resolved(SchemaValue::String(s)) if s == "json"),
            "deferred literal should be resolved to a string, got {:?}",
            resolved.value
        );
        assert!(tool.try_to_tool().is_ok());
    }

    #[test]
    fn deferred_value_is_unknown_name_is_rejected() {
        let mut tool = deferred_value_is_tree(
            ExtendedGlobals::default(),
            vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
                ExtendedValueIsRef {
                    name: "missing".into(),
                    value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("json".into())),
                },
            )])],
        );
        // Normalization leaves a name absent from every scope deferred; the
        // dangling reference is reported by validation.
        normalize_inherited_globals(&mut tool).expect("normalization tolerates unknown names");
        assert!(matches!(
            tool.try_to_tool().unwrap_err(),
            ToolBuildError::UnresolvedConstraintRef(name) if name == "missing"
        ));
    }

    #[test]
    fn deferred_value_is_incompatible_literal_is_rejected() {
        let mut tool = deferred_value_is_tree(
            ExtendedGlobals {
                options: vec![scalar_opt("format", None)],
                flags: vec![],
            },
            vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
                ExtendedValueIsRef {
                    name: "format".into(),
                    value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Bool(true)),
                },
            )])],
        );
        assert!(matches!(
            normalize_inherited_globals(&mut tool).unwrap_err(),
            ToolBuildError::ValueIsTypeMismatch(name) if name == "format"
        ));
    }

    #[test]
    fn deferred_value_is_against_ancestor_flag_is_rejected() {
        let mut tool = deferred_value_is_tree(
            ExtendedGlobals {
                options: vec![],
                flags: vec![bool_flag("force", None)],
            },
            vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
                ExtendedValueIsRef {
                    name: "force".into(),
                    value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Bool(true)),
                },
            )])],
        );
        assert!(matches!(
            normalize_inherited_globals(&mut tool).unwrap_err(),
            ToolBuildError::ValueIsTypeMismatch(name) if name == "force"
        ));
    }

    #[test]
    fn deferred_value_is_unresolved_at_validation_is_rejected() {
        // A deferred literal whose name *is* in scope but was never resolved
        // (composition skipped) is a resolution gap, not a silently accepted ref.
        let mut body = empty_body();
        body.options = vec![scalar_opt("format", None)];
        body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "format".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("json".into())),
            },
        )])];
        assert!(matches!(
            validate_tool(&leaf_tool_with_body(body)),
            Err(ToolBuildError::UnresolvedValueIsLiteral(name)) if name == "format"
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
    fn canonical_input_model_rejects_invalid_subcommand_index_in_reachable_tree() {
        let tool = tool_with_nodes(vec![
            bare_node("root", vec![1, -1]),
            bare_node("child", vec![]),
        ]);

        assert!(matches!(
            validate_tool(&tool),
            Err(ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 })
        ));
        assert!(matches!(
            tool.canonical_input_model(1),
            Err(ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 })
        ));
        assert!(matches!(
            tool.canonical_input_record_schema(1),
            Err(ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 })
        ));
        assert!(matches!(
            tool.decode_canonical_input_record(1, SchemaValue::Record { fields: vec![] }),
            Err(CanonicalInputDecodeError::Model(
                ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 }
            ))
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
    fn canonical_input_model_rejects_cycle_on_target_path() {
        let tool = tool_with_nodes(vec![
            bare_node("root", vec![1]),
            bare_node("child", vec![0]),
        ]);

        assert!(matches!(
            tool.canonical_input_model(1),
            Err(ToolBuildError::CommandTreeCycle(_))
        ));
        assert!(matches!(
            tool.decode_canonical_input_record(1, SchemaValue::Record { fields: vec![] }),
            Err(CanonicalInputDecodeError::Model(
                ToolBuildError::CommandTreeCycle(_)
            ))
        ));
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
    fn canonical_input_model_rejects_duplicate_parent_command_path() {
        let mut a = bare_node("a", vec![3]);
        a.globals.options = vec![scalar_opt("from-a", None)];
        let mut b = bare_node("b", vec![3]);
        b.globals.options = vec![scalar_opt("from-b", None)];
        let tool = tool_with_nodes(vec![
            bare_node("root", vec![1, 2]),
            a,
            b,
            bare_node("leaf", vec![]),
        ]);

        assert!(matches!(
            validate_tool(&tool),
            Err(ToolBuildError::DuplicateCommandParent(3))
        ));
        assert!(matches!(
            tool.canonical_input_model(3),
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
                value: ExtendedValueIsLiteral::Resolved(SchemaValue::Bool(true)),
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

    #[test]
    fn deferred_value_is_does_not_mask_repeatable_map_type_error() {
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
        body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "config".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Int(1)),
            },
        )])];

        let mut tool = leaf_tool_with_body(body);
        normalize_inherited_globals(&mut tool).expect(
            "normalization must not report a value-is mismatch before validation can report the malformed repeatable-map type",
        );
        assert!(matches!(
            validate_tool(&tool),
            Err(ToolBuildError::RepeatableMapTypeNotMap(name)) if name == "config"
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
    fn deferred_value_is_does_not_mask_dangling_option_type_ref() {
        let mut body = empty_body();
        body.options = vec![scalar_opt("name", None)];
        body.options[0].shape = ExtendedOptionShape::Scalar(ref_graph("nope"));
        body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "name".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("x".into())),
            },
        )])];

        let mut tool = leaf_tool_with_body(body);
        normalize_inherited_globals(&mut tool).expect(
            "normalization must not report a value-is mismatch before schema validation can report the dangling type ref",
        );
        assert!(matches!(
            validate_tool(&tool),
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

    fn shapes_match(a: SchemaType, b: SchemaType) -> bool {
        schema_shapes_match(&SchemaGraph::anonymous(a), &SchemaGraph::anonymous(b))
    }

    fn quantity(base_unit: &str, allowed_suffixes: &[&str]) -> SchemaType {
        SchemaType::quantity(QuantitySpec {
            base_unit: base_unit.to_string(),
            allowed_suffixes: allowed_suffixes.iter().map(|s| s.to_string()).collect(),
            min: None,
            max: None,
        })
    }

    #[test]
    fn rich_leaf_quantity_identity_is_compared_but_bounds_ignored() {
        // Different base unit: not interchangeable.
        assert!(!shapes_match(quantity("m", &[]), quantity("s", &[])));
        // Same base unit, different accepted unit sets: not interchangeable.
        assert!(!shapes_match(
            quantity("m", &["m", "km"]),
            quantity("m", &["m"])
        ));
        // Same identity, suffix order irrelevant.
        assert!(shapes_match(
            quantity("m", &["m", "km"]),
            quantity("m", &["km", "m"])
        ));
        // Bounds (min/max) are validation restrictions and stay ignored.
        let lo = SchemaType::quantity(QuantitySpec {
            base_unit: "m".to_string(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        });
        let hi = SchemaType::quantity(QuantitySpec {
            base_unit: "m".to_string(),
            allowed_suffixes: vec![],
            min: Some(crate::schema::QuantityValue {
                mantissa: 0,
                scale: 0,
                unit: "m".to_string(),
            }),
            max: None,
        });
        assert!(shapes_match(lo, hi));
    }

    #[test]
    fn rich_leaf_secret_and_quota_identity_is_compared() {
        assert!(shapes_match(
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("api-key".into()),
            }),
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("api-key".into()),
            }),
        ));
        assert!(!shapes_match(
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("api-key".into()),
            }),
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("oauth-token".into()),
            }),
        ));
        assert!(!shapes_match(
            SchemaType::quota_token(QuotaTokenSpec {
                resource_name: Some("tokens".into())
            }),
            SchemaType::quota_token(QuotaTokenSpec {
                resource_name: Some("requests".into())
            }),
        ));
    }

    #[test]
    fn rich_leaf_secret_inner_identity_is_compared() {
        assert!(!shapes_match(
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("api-key".into()),
            }),
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::u64()),
                category: Some("api-key".into()),
            }),
        ));
    }

    #[test]
    fn rich_leaf_text_and_binary_identity_is_compared_but_other_bounds_ignored() {
        let text = |languages: Option<&[&str]>, regex: Option<&str>| {
            SchemaType::text(TextRestrictions {
                languages: languages.map(|ls| ls.iter().map(|s| s.to_string()).collect()),
                min_length: None,
                max_length: None,
                regex: regex.map(|s| s.to_string()),
            })
        };
        // Language set is type identity.
        assert!(!shapes_match(
            text(Some(&["en"]), None),
            text(Some(&["de"]), None)
        ));
        assert!(!shapes_match(text(None, None), text(Some(&["en"]), None)));
        // Regex is a refinable restriction and stays ignored.
        assert!(shapes_match(
            text(Some(&["en"]), Some("a+")),
            text(Some(&["en"]), Some("b+"))
        ));

        let binary = |mime: Option<&[&str]>, max_bytes: Option<u32>| {
            SchemaType::binary(BinaryRestrictions {
                mime_types: mime.map(|ms| ms.iter().map(|s| s.to_string()).collect()),
                min_bytes: None,
                max_bytes,
            })
        };
        assert!(!shapes_match(
            binary(Some(&["image/png"]), None),
            binary(Some(&["image/jpeg"]), None)
        ));
        // Byte bounds are refinable restrictions and stay ignored.
        assert!(shapes_match(
            binary(Some(&["image/png"]), Some(10)),
            binary(Some(&["image/png"]), Some(20))
        ));
    }

    #[test]
    fn rich_leaf_path_direction_and_kind_are_refinable_and_ignored() {
        let path = |direction: PathDirection, kind: PathKind| {
            SchemaType::path(PathSpec {
                direction,
                kind,
                allowed_mime_types: None,
                allowed_extensions: None,
            })
        };
        // direction/kind are `#[arg]`-refinable, so a leaf may re-specify them;
        // de-projection still treats them as the same inherited global.
        assert!(shapes_match(
            path(PathDirection::Input, PathKind::File),
            path(PathDirection::Output, PathKind::Directory)
        ));
    }

    #[test]
    fn plain_string_matches_unrestricted_text_but_not_language_restricted() {
        let unrestricted = SchemaType::text(TextRestrictions {
            languages: None,
            min_length: None,
            max_length: None,
            regex: Some("^x$".to_string()),
        });
        let language_restricted = SchemaType::text(TextRestrictions {
            languages: Some(vec!["en".to_string()]),
            min_length: None,
            max_length: None,
            regex: None,
        });
        // A leaf `String` de-projects onto an inherited refined-`String` (`Text`)
        // global; the regex is a refinable restriction and is ignored.
        assert!(shapes_match(SchemaType::string(), unrestricted.clone()));
        assert!(shapes_match(unrestricted, SchemaType::string()));
        // A language-restricted `Text` is a different Rust type and must conflict.
        assert!(!shapes_match(SchemaType::string(), language_restricted));
    }
}
