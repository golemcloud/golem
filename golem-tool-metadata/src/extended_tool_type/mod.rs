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

use crate::tool_literal::{ToolLiteral, value_is_literal_to_schema_value};
use golem_schema::schema::tool as native;
use golem_schema::schema::tool as wire;
use golem_schema::schema::tool::validation::ToolValidationError;
use golem_schema::schema::validation::{is_equivalent_cross_graph, validate_value};

use golem_schema::schema::{SchemaGraph, SchemaType, SchemaValue, merge_agent_graphs};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

pub use golem_schema::schema::tool::{
    BoolFlagShape, DuplicateKeyPolicy, ErrorKind, FlagShape, Quantifier, Repetition,
};

mod schema_graph;
pub use schema_graph::option_collected_graph;
use schema_graph::{
    canonical_option_graph, canonical_positional_graph, flag_graph, list_wrapper_graph,
    option_input_graph, resolves_to_map, schema_graph_root,
};
mod conversion;
pub use conversion::native_tool_value_schema;
mod canonical_input;
pub use canonical_input::{
    adapt_canonical_input_value, build_canonical_input, build_canonical_input_with_prefix,
};
mod validation;
pub use validation::validate_tool;
use validation::{
    check_command_tree_structure, register_fixed_positional_scope, register_flag_scope,
    register_option_scope, register_tail_scope, validate_tool_with, value_is_compatible,
};
mod help;
pub use help::{render_argument_help, render_help};
mod graft;
pub use graft::{
    append_grafted_subtree, graft_subtree, normalize_inherited_globals,
    reconcile_command_inherited_globals, reconcile_subtree_parent_globals,
};

pub trait ToolSchemaRepr: Clone + std::fmt::Debug {
    type Value: Clone + std::fmt::Debug;

    fn list(&self) -> Self;
    fn map_value(&self) -> Option<Self>;
    fn shape_matches(&self, other: &Self) -> bool;
    fn is_sound(&self) -> bool;
    fn value_is_literal(
        &self,
        literal: &ToolLiteral,
        whole_or_one_peel: bool,
    ) -> Result<Self::Value, ToolBuildError>;
}

#[derive(Clone, Debug)]
pub struct ExtendedToolType<S: ToolSchemaRepr = SchemaGraph> {
    pub version: String,
    pub requires_filesystem: bool,
    pub commands: Vec<ExtendedCommandNode<S>>,
}

/// Immutable, validated metadata. Native conversion is performed once when
/// preparing the descriptor. Wire values are encoded from the cached native
/// form because the WIT model is not cloneable.
///
/// ```compile_fail
/// use golem_tool_metadata::PreparedToolDescriptor;
/// fn mutate(prepared: PreparedToolDescriptor) {
///     prepared.clone().native().version.clear();
/// }
/// ```
///
/// ```compile_fail
/// use golem_tool_metadata::PreparedToolDescriptor;
/// fn mutate(prepared: PreparedToolDescriptor) {
///     prepared.extended().commands.clear();
/// }
/// ```
#[derive(Clone, Debug)]
pub struct PreparedToolDescriptor {
    extended: ExtendedToolType,
    native: native::Tool,
}

/// Schema validation selected by generated descriptor code. The fields are
/// private so callers cannot substitute a validator that accepts invalid data.
#[derive(Clone, Copy)]
pub struct ToolSchemaChecks(ToolGraphCheck);

type ToolGraphCheck = fn(&SchemaGraph, &str) -> Result<(), ToolBuildError>;

/// Autoref dispatch lets generated code select using the resolved Rust type,
/// including aliases, without confusing a user type named `String` with std's.
#[doc(hidden)]
pub struct ToolSchemaProbe<T>(pub std::marker::PhantomData<T>);

#[doc(hidden)]
pub trait SelectToolSchemaChecks {
    fn tool_schema_checks(self) -> Option<ToolSchemaChecks>;
    fn tool_value_schema(
        self,
        fallback: impl FnOnce() -> Result<SchemaGraph, ToolBuildError>,
    ) -> Result<SchemaGraph, ToolBuildError>;
}

#[derive(Clone, Debug)]
pub struct ExtendedCommandNode<S: ToolSchemaRepr = SchemaGraph> {
    pub name: String,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub globals: ExtendedGlobals<S>,
    pub subcommands: Vec<i32>,
    pub body: Option<ExtendedCommandBody<S>>,
}

#[derive(Clone, Debug)]
pub struct ExtendedGlobals<S: ToolSchemaRepr = SchemaGraph> {
    pub options: Vec<ExtendedOptionSpec<S>>,
    pub flags: Vec<FlagSpec>,
}

impl<S: ToolSchemaRepr> Default for ExtendedGlobals<S> {
    fn default() -> Self {
        Self {
            options: Vec::new(),
            flags: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExtendedCommandBody<S: ToolSchemaRepr = SchemaGraph> {
    pub positionals: ExtendedPositionals<S>,
    pub options: Vec<ExtendedOptionSpec<S>>,
    pub flags: Vec<FlagSpec>,
    pub constraints: Vec<ExtendedConstraint<S>>,
    pub stdin: Option<StreamSpec>,
    pub stdout: Option<StreamSpec>,
    pub stderr: Option<StreamSpec>,
    pub result: Option<ExtendedResultSpec<S>>,
    pub errors: Vec<ExtendedErrorCase<S>>,
    pub annotations: Option<CommandAnnotations>,
    /// The body's positional-eligible parameters, in declaration order, used to
    /// finalize the tail positional after inherited-global de-projection (see
    /// [`graft::reinfer_body_tail`]). The final surface of a `Vec<T>` candidate (tail
    /// positional vs repeatable-list option) depends on which following
    /// re-declarations survive composition, so the macro records the authored
    /// facts needed to finalize it; an explicit tail additionally carries its full
    /// authored spec so promotion is lossless. Empty for hand-built bodies and
    /// ignored by canonical conversion.
    pub positional_plan: Vec<PositionalCandidate<S>>,
}

/// One positional-eligible parameter of a command body, recorded by the macro in
/// declaration order so the runtime can finalize the tail positional after
/// inherited-global de-projection. See [`ExtendedCommandBody::positional_plan`]
/// and [`graft::reinfer_body_tail`].
#[derive(Clone, Debug)]
pub enum PositionalCandidate<S: ToolSchemaRepr = SchemaGraph> {
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
        authored_tail_surrogate: Option<Box<ExtendedTailPositional<S>>>,
        /// Long names of the body options declared after this `Vec<T>`, in
        /// declaration order. When this candidate is demoted from the tail to a
        /// repeatable-list option, it is inserted before the first of these that
        /// survived de-projection (preserving declaration order, §D7); otherwise
        /// it is appended.
        later_option_names: Vec<String>,
    },
}

impl<S: ToolSchemaRepr> PositionalCandidate<S> {
    fn name(&self) -> &str {
        match self {
            PositionalCandidate::Plain { name }
            | PositionalCandidate::VecCandidate { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExtendedPositionals<S: ToolSchemaRepr = SchemaGraph> {
    pub fixed: Vec<ExtendedPositional<S>>,
    pub tail: Option<ExtendedTailPositional<S>>,
}

impl<S: ToolSchemaRepr> Default for ExtendedPositionals<S> {
    fn default() -> Self {
        Self {
            fixed: Vec::new(),
            tail: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExtendedPositional<S: ToolSchemaRepr = SchemaGraph> {
    pub name: String,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub type_: S,
    pub default: Option<S::Value>,
    pub required: bool,
    pub accepts_stdio: bool,
}

#[derive(Clone, Debug)]
pub struct ExtendedTailPositional<S: ToolSchemaRepr = SchemaGraph> {
    pub name: String,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub item_type: S,
    pub min: u32,
    pub max: Option<u32>,
    pub separator: Option<String>,
    pub verbatim: bool,
    pub accepts_stdio: bool,
}

#[derive(Clone, Debug)]
pub struct ExtendedOptionSpec<S: ToolSchemaRepr = SchemaGraph> {
    pub long: String,
    pub short: Option<char>,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub shape: ExtendedOptionShape<S>,
    pub default: Option<S::Value>,
    pub required: bool,
    pub env_var: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ExtendedOptionShape<S: ToolSchemaRepr = SchemaGraph> {
    Scalar(S),
    OptionalScalar(S),
    RepeatableList(ExtendedRepeatableListShape<S>),
    RepeatableMap(ExtendedRepeatableMapShape<S>),
}

#[derive(Clone, Debug)]
pub struct ExtendedRepeatableListShape<S: ToolSchemaRepr = SchemaGraph> {
    pub repetition: wire::Repetition,
    pub item_type: S,
}

#[derive(Clone, Debug)]
pub struct ExtendedRepeatableMapShape<S: ToolSchemaRepr = SchemaGraph> {
    pub repetition: wire::Repetition,
    pub map_type: S,
    pub duplicate_key_policy: wire::DuplicateKeyPolicy,
}

pub type FlagSpec = wire::FlagSpec;
pub type CommandAnnotations = wire::CommandAnnotations;
pub type StreamSpec = wire::StreamSpec;
pub type ToolFormatter = wire::Formatter;
pub type Doc = wire::Doc;
pub type Example = wire::Example;

#[derive(Clone, Debug)]
pub struct ExtendedResultSpec<S: ToolSchemaRepr = SchemaGraph> {
    pub type_: S,
    pub doc: Doc,
    pub formatters: Vec<ToolFormatter>,
    pub default_formatter: String,
}

#[derive(Clone, Debug)]
pub struct ExtendedErrorCase<S: ToolSchemaRepr = SchemaGraph> {
    pub name: String,
    pub doc: Doc,
    pub kind: wire::ErrorKind,
    pub exit_code: u8,
    pub payload: Option<S>,
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

    fn to_error_payload_value(&self) -> Result<(String, crate::TypedSchemaValue), String>;

    fn from_error_payload_value(
        name: String,
        value: crate::TypedSchemaValue,
    ) -> Result<Option<Self>, String>
    where
        Self: Sized;
}

#[derive(Clone, Debug)]
pub enum ExtendedRef<S: ToolSchemaRepr = SchemaGraph> {
    Present(String),
    ValueIs(ExtendedValueIsRef<S>),
}

#[derive(Clone, Debug)]
pub struct ExtendedValueIsRef<S: ToolSchemaRepr = SchemaGraph> {
    pub name: String,
    pub value: ExtendedValueIsLiteral<S>,
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
pub enum ExtendedValueIsLiteral<S: ToolSchemaRepr = SchemaGraph> {
    Resolved(S::Value),
    Deferred(ToolLiteral),
}

#[derive(Clone, Debug)]
pub enum ExtendedConstraint<S: ToolSchemaRepr = SchemaGraph> {
    RequiresAll(Vec<ExtendedRef<S>>),
    AllOrNone(Vec<ExtendedRef<S>>),
    RequiresAny(Vec<ExtendedRef<S>>),
    MutexGroups(Vec<ExtendedRefGroup<S>>),
    Implies(ExtendedImpliesC<S>),
    Forbids(ExtendedForbidsC<S>),
}

#[derive(Clone, Debug)]
pub struct ExtendedRefGroup<S: ToolSchemaRepr = SchemaGraph> {
    pub refs: Vec<ExtendedRef<S>>,
}
#[derive(Clone, Debug)]
pub struct ExtendedImpliesC<S: ToolSchemaRepr = SchemaGraph> {
    pub lhs_quant: wire::Quantifier,
    pub lhs: Vec<ExtendedRef<S>>,
    pub rhs_quant: wire::Quantifier,
    pub rhs: Vec<ExtendedRef<S>>,
}
#[derive(Clone, Debug)]
pub struct ExtendedForbidsC<S: ToolSchemaRepr = SchemaGraph> {
    pub lhs_quant: wire::Quantifier,
    pub lhs: Vec<ExtendedRef<S>>,
    pub rhs: Vec<ExtendedRef<S>>,
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

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum EffectiveCommandField<S: ToolSchemaRepr = SchemaGraph> {
    Option(ExtendedOptionSpec<S>),
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
    pub short: Option<char>,
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
    pub short: Option<char>,
    pub schema: SchemaGraph,
    pub value: SchemaValue,
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
struct ValueIsComparand<S: ToolSchemaRepr = SchemaGraph> {
    graph: S,
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
enum ValueComparand<S: ToolSchemaRepr = SchemaGraph> {
    Type(ValueIsComparand<S>),
    BlockedByTypeError,
}

/// Per-name `value-is` comparand for constraint resolution (only value-typed
/// names; a name in `names` but absent from `typed` is a flag).
struct NameScope<S: ToolSchemaRepr = SchemaGraph> {
    names: BTreeSet<String>,
    typed: BTreeMap<String, ValueComparand<S>>,
}

impl<S: ToolSchemaRepr> Default for NameScope<S> {
    fn default() -> Self {
        Self {
            names: BTreeSet::new(),
            typed: BTreeMap::new(),
        }
    }
}

pub struct ToolBuildCtx<S: ToolSchemaRepr = SchemaGraph> {
    stack: Vec<String>,
    command_path: Vec<String>,
    inherited_globals: Vec<EffectiveCommandField<S>>,
    graft_roots: Vec<PendingGraftRoot>,
}

#[derive(Clone)]
struct PendingGraftRoot {
    expected_name: String,
    override_name: Option<String>,
}

pub trait ToolDefinitionDescriptor {
    fn metadata(ctx: &mut ToolBuildCtx) -> Result<ExtendedToolType, ToolBuildError>;
}

#[cfg(test)]
mod tests;
