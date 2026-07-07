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

//! Native, serializable data model for `golem:tool/common@0.1.0` tool metadata.
//!
//! A *tool* is a callable unit described by a single metadata record. The model
//! is CLI-native (commands, subcommands, options, flags, positionals).
//!
//! Types and values are not modeled here: every input/output type and every
//! metadata-time value is expressed with the shared `golem:core/types@2.0.0`
//! schema model, exactly as the agent model does. A [`Tool`] owns a single
//! [`SchemaGraph`] — the named-type registry shared by all of its commands —
//! and each typed position embeds a recursive [`SchemaType`]; metadata-time
//! defaults and `value-is` literals embed a recursive [`SchemaValue`]. The
//! flattening into the WIT `schema-graph` / `type-node-index` /
//! `schema-value-tree` wire form happens in `golem_common::schema::tool::wit`.
//!
//! The only tool-specific recursion site is the command tree: a flattened
//! command hierarchy with the root at index 0 and children referenced by
//! [`CommandIndex`].
//!
//! Producer-side construction invariants are checked by [`validation`]. The
//! deterministic per-command input record shape shared by tool clients and
//! tool implementations is defined by [`canonical`].

use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use serde::{Deserialize, Serialize};

pub mod canonical;
pub mod validation;

/// Index into [`CommandTree::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandIndex(pub i32);

impl CommandIndex {
    /// Returns the index as a `usize`, or `None` if it is negative.
    pub fn as_usize(self) -> Option<usize> {
        usize::try_from(self.0).ok()
    }
}

/// Top-level tool metadata record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub version: String,
    pub commands: CommandTree,
    /// Named-type registry shared by this tool's commands. Typed positions in
    /// the command tree may reference these definitions via
    /// [`SchemaType::Ref`]. Use [`SchemaGraph::empty`] when there are no shared
    /// definitions to declare.
    ///
    /// The graph's `root` field is a structurally-required placeholder (empty
    /// record); only `defs` is consumed. The real roots are the per-position
    /// embedded [`SchemaType`]s.
    #[serde(default = "SchemaGraph::empty")]
    pub schema: SchemaGraph,
}

impl Tool {
    /// Returns the name of the tool: the name of the root node of its command tree.
    pub fn name(&self) -> Option<&str> {
        self.commands.nodes.first().map(|node| node.name.as_str())
    }
}

/// Flattened command hierarchy. Always non-empty; the root command is at index 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandTree {
    pub nodes: Vec<CommandNode>,
}

/// A node in the command tree. May dispatch to subcommands, run its own body, or
/// both. Globals declared here apply to this command's own body and to every
/// descendant subcommand body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandNode {
    pub name: String,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub globals: Globals,
    pub subcommands: Vec<CommandIndex>,
    pub body: Option<CommandBody>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Globals {
    pub options: Vec<OptionSpec>,
    pub flags: Vec<FlagSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandBody {
    pub positionals: Positionals,
    pub options: Vec<OptionSpec>,
    pub flags: Vec<FlagSpec>,
    pub constraints: Vec<Constraint>,
    pub stdin: Option<StreamSpec>,
    pub stdout: Option<StreamSpec>,
    pub result: Option<ResultSpec>,
    pub errors: Vec<ErrorCase>,
    pub annotations: Option<CommandAnnotations>,
}

/// Behavioral hints surfaced to MCP and other LLM-facing surfaces, following
/// the MCP convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAnnotations {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Positionals {
    pub fixed: Vec<Positional>,
    pub tail: Option<TailPositional>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Positional {
    pub name: String,
    pub doc: Doc,
    pub value_name: Option<String>,
    /// Schema of the positional's value.
    pub type_: SchemaType,
    /// Default value, interpreted against [`type_`](Self::type_).
    pub default: Option<SchemaValue>,
    pub required: bool,
    /// If true, the positional's value may be read from standard input.
    pub accepts_stdio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailPositional {
    pub name: String,
    pub doc: Doc,
    pub value_name: Option<String>,
    /// Schema of a single tail item.
    pub item_type: SchemaType,
    pub min: u32,
    pub max: Option<u32>,
    /// Token required before tail items (e.g. `--` for `git log -- <paths>`).
    pub separator: Option<String>,
    /// If true, tokens after `separator` are not flag-parsed.
    pub verbatim: bool,
    /// If true, the tail items may be read from standard input.
    pub accepts_stdio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionSpec {
    pub long: String,
    pub short: Option<char>,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub value_name: Option<String>,
    pub shape: OptionShape,
    /// Default value, interpreted against the option's value type.
    pub default: Option<SchemaValue>,
    pub required: bool,
    pub env_var: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OptionShape {
    /// Required value: `--opt VALUE` or `--opt=VALUE`.
    Scalar(SchemaType),
    /// Bare presence collapses to `default`; with value parses normally.
    OptionalScalar(SchemaType),
    /// Repeatable scalar option (`-e a -e b`); the collected value is a list
    /// of the element type.
    RepeatableList(RepeatableListShape),
    /// Repeatable key-value option (`-c a=1 -c b=2`); the collected value is a
    /// [`SchemaType::Map`], never a `list<tuple>`.
    RepeatableMap(RepeatableMapShape),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepeatableListShape {
    pub repetition: Repetition,
    /// Schema of a single collected element.
    pub item_type: SchemaType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepeatableMapShape {
    pub repetition: Repetition,
    /// A [`SchemaType::Map`] node; the collected value is this map.
    pub map_type: SchemaType,
    /// What happens when the same key is supplied more than once.
    pub duplicate_key_policy: DuplicateKeyPolicy,
}

/// Resolution policy for a repeated key in a [`OptionShape::RepeatableMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuplicateKeyPolicy {
    /// A repeated key is a usage error.
    Reject,
    /// A repeated key takes the last supplied value.
    LastWins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repetition {
    /// `--inc a --inc b`
    Repeated,
    /// `--inc=a,b`
    Delimited(char),
    /// Both surface forms accepted.
    Either(char),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagSpec {
    pub long: String,
    pub short: Option<char>,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub shape: FlagShape,
    pub env_var: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlagShape {
    BoolFlag(BoolFlagShape),
    /// Counted flag (`-vvv`); optional max count.
    CountFlag(Option<u32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoolFlagShape {
    pub default: bool,
    /// If true, `--no-<name>` is auto-synthesized.
    pub negatable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Ref {
    Present(String),
    ValueIs(ValueIsRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueIsRef {
    pub name: String,
    /// Literal value, interpreted against the declared type of `name`.
    pub value: SchemaValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Constraint {
    RequiresAll(Vec<Ref>),
    AllOrNone(Vec<Ref>),
    RequiresAny(Vec<Ref>),
    MutexGroups(Vec<RefGroup>),
    Implies(ImpliesC),
    Forbids(ForbidsC),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefGroup {
    pub refs: Vec<Ref>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpliesC {
    pub lhs_quant: Quantifier,
    pub lhs: Vec<Ref>,
    pub rhs_quant: Quantifier,
    pub rhs: Vec<Ref>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForbidsC {
    pub lhs_quant: Quantifier,
    pub lhs: Vec<Ref>,
    pub rhs: Vec<Ref>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantifier {
    All,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSpec {
    pub doc: Doc,
    pub mime: Vec<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultSpec {
    /// Schema of the result value.
    pub type_: SchemaType,
    pub doc: Doc,
    pub formatters: Vec<Formatter>,
    pub default_formatter: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Formatter {
    pub name: String,
    pub doc: Doc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorCase {
    pub name: String,
    pub doc: Doc,
    pub kind: ErrorKind,
    pub exit_code: u8,
    /// Schema of the error payload, if any.
    pub payload: Option<SchemaType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    UsageError,
    RuntimeError,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Doc {
    pub summary: String,
    pub description: String,
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub title: String,
    pub body: String,
}
