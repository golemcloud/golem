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

//! Producer-side construction invariants for the tool metadata model.
//!
//! The WIT shape alone does not enforce the construction invariants listed in
//! the `golem:tool/common@0.1.0` doc-comment; they are checked here by
//! [`validate_tool`]. These checks are intended to run when a tool component's
//! metadata is produced or first ingested, so that every downstream consumer
//! (signature projection, help rendering, the runtime) can assume a
//! well-formed [`Tool`].
//!
//! Type and value well-formedness (recursion, named-definition resolution,
//! value-against-type validity) is delegated to the shared `golem:core/types`
//! model in [`golem_schema`]: typed positions embed a recursive
//! [`SchemaType`] resolving against the tool's [`SchemaGraph`], and
//! [`validate_value`] checks `value-is` literals against the declared type.
//!
//! The CLI-structural checks implemented here are:
//!
//!   * The command tree is non-empty (the root command is at index 0).
//!   * Every `command-index` subcommand reference is in bounds.
//!   * All identifier-like strings match [`is_valid_identifier`].
//!   * Subcommand names and aliases are pairwise unique among siblings.
//!   * Within a `command-body`, option/flag long names, their aliases,
//!     positional names, and short forms are pairwise unique, and unique
//!     against globals inherited from the command itself and any ancestor.
//!   * Every constraint `ref` name resolves to a body-declared or inherited
//!     option/flag/positional.
//!   * Each `value-is` literal is a valid value for the declared type of the
//!     referenced name (with the "any element/occurrence" relaxation for
//!     list-typed and repeatable names).
//!   * A body's `default-formatter` resolves to one of its `formatters`.
//!   * A `tail-positional` with `verbatim = true` declares a `separator`.
//!   * `variant` types never appear in (or are reachable from) an input
//!     position; input-side discrimination uses `union` instead.
//!   * The command tree is an acyclic tree: every node is reachable from the
//!     root and has exactly one parent (no cycles, no shared subcommands).
//!
//! One WIT-listed invariant is deliberately not checked here: the root
//! command-node's name equals the tool's metadata name. A tool is identified by
//! its root command name, so there is no second value to compare against at
//! this layer; the check belongs to whatever registers a tool under an
//! externally supplied name.

use super::{
    CommandBody, CommandIndex, CommandNode, Constraint, Globals, OptionShape, OptionSpec,
    Positional, Ref, Tool,
};
use crate::schema::graph::{RefResolutionError, SchemaGraph};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use crate::schema::validation::value::{ValueError, validate_value};
use crate::schema::validation::well_formedness::{SchemaError, validate_root_type};
use regex::Regex;
use std::collections::HashSet;
use std::fmt::{self, Display, Formatter};
use std::sync::LazyLock;

/// The identifier grammar shared by every identifier-like string in the tool
/// model: lowercase kebab-case, starting with a letter.
static IDENTIFIER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").expect("invalid tool identifier regex")
});

/// Returns `true` if `s` is a valid tool identifier (`^[a-z][a-z0-9]*(-[a-z0-9]+)*$`).
pub fn is_valid_identifier(s: &str) -> bool {
    IDENTIFIER_REGEX.is_match(s)
}

/// A single producer-side construction-invariant violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolValidationError {
    /// The command tree has no nodes (it must contain at least the root).
    EmptyCommandTree,
    /// A `command-index` references a node outside the command tree.
    CommandIndexOutOfBounds { index: i32, len: usize },
    /// An identifier-like string does not match [`is_valid_identifier`].
    InvalidIdentifier { kind: &'static str, value: String },
    /// Two sibling subcommands share a name or alias.
    DuplicateSubcommandName { parent: String, name: String },
    /// Two names (option/flag long names, aliases, positional names) collide
    /// within a command body's scope (including inherited globals).
    DuplicateName { command: String, name: String },
    /// Two short forms collide within a command body's scope.
    DuplicateShort { command: String, short: char },
    /// A constraint `ref` names something not in scope.
    UnresolvedConstraintRef { command: String, name: String },
    /// A `value-is` literal is not a valid value for the referenced name's
    /// declared type.
    ValueIsTypeMismatch { command: String, name: String },
    /// A `default` literal is not a valid value for its option/positional's
    /// declared type (a `repeatable` option's default must be a list of its
    /// element type).
    DefaultTypeMismatch { command: String, name: String },
    /// A `repeatable-map` option's collected `map_type` does not resolve to a
    /// [`SchemaType::Map`] (the collected key-value value must be a map node).
    RepeatableMapTypeNotMap { command: String, name: String },
    /// An embedded type contains a [`SchemaType::Ref`] whose id does not
    /// resolve to a definition in the tool's [`SchemaGraph`].
    UnresolvedTypeRef {
        command: String,
        position: String,
        id: String,
    },
    /// An embedded type is not structurally well-formed (for example an
    /// inverted numeric `min > max`, an empty variant, or a non-primitive map
    /// key), as reported by schema well-formedness validation.
    IllFormedSchema {
        command: String,
        position: String,
        detail: String,
    },
    /// Two definitions in the tool's [`SchemaGraph`] share the same type id; the
    /// wire encoder requires definition ids to be unique.
    DuplicateTypeId { id: String },
    /// A body's `default-formatter` does not resolve to one of its formatters.
    UnresolvedDefaultFormatter { command: String, formatter: String },
    /// A `tail-positional` declares `verbatim = true` without a `separator`.
    VerbatimWithoutSeparator { command: String, positional: String },
    /// A `variant` type is reachable from an input position; input-side
    /// discrimination must use `union` instead.
    VariantInInputPosition { command: String, position: String },
    /// A command-tree node is not reachable from the root.
    UnreachableCommandNode { index: i32 },
    /// The command tree contains a cycle.
    CommandTreeCycle { index: i32 },
    /// A command-tree node is referenced as a subcommand by more than one
    /// parent (the command graph is a DAG, not a tree).
    DuplicateCommandParent { index: i32 },
}

impl Display for ToolValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ToolValidationError::EmptyCommandTree => {
                write!(f, "the command tree is empty")
            }
            ToolValidationError::CommandIndexOutOfBounds { index, len } => {
                write!(
                    f,
                    "command index {index} is out of bounds (tree has {len} nodes)"
                )
            }
            ToolValidationError::InvalidIdentifier { kind, value } => {
                write!(f, "invalid {kind}: {value:?}")
            }
            ToolValidationError::DuplicateSubcommandName { parent, name } => {
                write!(
                    f,
                    "duplicate subcommand name {name:?} under command {parent:?}"
                )
            }
            ToolValidationError::DuplicateName { command, name } => {
                write!(f, "duplicate name {name:?} in command {command:?}")
            }
            ToolValidationError::DuplicateShort { command, short } => {
                write!(f, "duplicate short form {short:?} in command {command:?}")
            }
            ToolValidationError::UnresolvedConstraintRef { command, name } => {
                write!(
                    f,
                    "constraint references unknown name {name:?} in command {command:?}"
                )
            }
            ToolValidationError::ValueIsTypeMismatch { command, name } => {
                write!(
                    f,
                    "value-is literal for {name:?} is not valid for its type in command {command:?}"
                )
            }
            ToolValidationError::DefaultTypeMismatch { command, name } => {
                write!(
                    f,
                    "default literal for {name:?} is not valid for its type in command {command:?}"
                )
            }
            ToolValidationError::RepeatableMapTypeNotMap { command, name } => {
                write!(
                    f,
                    "repeatable-map option {name:?} in command {command:?} does not collect into a map type"
                )
            }
            ToolValidationError::UnresolvedTypeRef {
                command,
                position,
                id,
            } => {
                write!(
                    f,
                    "type reference {id:?} at position {position:?} in command {command:?} does not resolve to a definition in the tool schema"
                )
            }
            ToolValidationError::IllFormedSchema {
                command,
                position,
                detail,
            } => {
                write!(
                    f,
                    "type at position {position:?} in command {command:?} is not well-formed: {detail}"
                )
            }
            ToolValidationError::DuplicateTypeId { id } => {
                write!(f, "duplicate schema definition id {id:?}")
            }
            ToolValidationError::UnresolvedDefaultFormatter { command, formatter } => {
                write!(
                    f,
                    "default-formatter {formatter:?} is not declared in command {command:?}"
                )
            }
            ToolValidationError::VerbatimWithoutSeparator {
                command,
                positional,
            } => {
                write!(
                    f,
                    "tail positional {positional:?} in command {command:?} is verbatim but has no separator"
                )
            }
            ToolValidationError::VariantInInputPosition { command, position } => {
                write!(
                    f,
                    "a variant type is reachable from input position {position:?} in command {command:?}"
                )
            }
            ToolValidationError::UnreachableCommandNode { index } => {
                write!(f, "command node {index} is not reachable from the root")
            }
            ToolValidationError::CommandTreeCycle { index } => {
                write!(f, "the command tree contains a cycle at node {index}")
            }
            ToolValidationError::DuplicateCommandParent { index } => {
                write!(f, "command node {index} has more than one parent")
            }
        }
    }
}

impl std::error::Error for ToolValidationError {}

/// Validate a [`Tool`] against the producer-side construction invariants.
pub fn validate_tool(tool: &Tool) -> Result<(), Vec<ToolValidationError>> {
    let mut ctx = Validator::new(tool);
    ctx.run();
    if ctx.errors.is_empty() {
        Ok(())
    } else {
        Err(ctx.errors)
    }
}

/// An input position whose declared type must not reach a `variant`.
struct InputRoot<'a> {
    command: String,
    position: String,
    ty: &'a SchemaType,
}

struct Validator<'a> {
    tool: &'a Tool,
    errors: Vec<ToolValidationError>,
    /// Types reachable from an input position; checked for `variant` after the
    /// command traversal.
    input_roots: Vec<InputRoot<'a>>,
    /// Every [`SchemaType::Ref`] id written structurally in a command position
    /// type (descending through inline constructors, but not following refs into
    /// definition bodies). These are exactly the refs whose alias chains the
    /// use-site well-formedness check ([`Self::check_type_well_formed`]) walks
    /// and reports on, so they seed the suppression of redundant def-body copies
    /// in [`Self::check_def_refs`].
    used_ref_ids: HashSet<TypeId>,
}

/// How a `value-is` literal is matched against its comparand type. Mirrors the
/// SDK runtime's `ValueIsMode` so host and guest agree on what a `value-is`
/// against a collecting vs. non-collecting surface means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueIsMode {
    /// A non-collecting value surface (a scalar option, a fixed positional): the
    /// literal matches the whole declared value, or — under the one-level
    /// relaxation — a single element of a list/fixed-list or value of a map
    /// (optionally `option`-wrapped).
    WholeOrOnePeel,
    /// A collecting surface (a repeatable option or tail positional, whose
    /// comparand is already the per-occurrence type): the literal matches one
    /// occurrence exactly, with no element/value relaxation (matching one more
    /// level would descend past a single occurrence).
    Exact,
}

/// The `value-is` comparand recorded for a name that carries a declared value
/// type. A name absent from the comparand map carries no value type at all (a
/// flag), which is a genuine `value-is` mismatch.
#[derive(Clone, Copy)]
enum ValueComparand<'a> {
    /// The resolved type a `value-is` literal must be valid for, plus the mode
    /// controlling whether the one-level relaxation applies.
    Type(&'a SchemaType, ValueIsMode),
    /// The name is typed, but its declared type could not be resolved to a
    /// comparable value type (a repeatable-map whose `map_type` does not resolve
    /// to a map). The underlying type error is reported separately, so
    /// `value-is` checking is suppressed to avoid a misleading cascade.
    BlockedByTypeError,
}

/// The classification of a `value-is` literal against a declared value type.
enum ValueIsOutcome {
    /// The literal is a valid value for the declared type (or, under the
    /// element relaxation, for its element type).
    Compatible,
    /// The literal is not valid for the declared type and the mismatch is
    /// genuine (not merely an artifact of an unresolved reference).
    Mismatch,
    /// The comparison failed only because the literal descended into an
    /// unresolved reference, reported separately as `UnresolvedTypeRef`; the
    /// mismatch is suppressed to avoid misleading cascade noise.
    BlockedByDanglingRef,
}

/// The set of names and declared types reachable from a command body, used for
/// uniqueness checks and constraint-ref resolution.
#[derive(Default)]
struct NameScope<'a> {
    /// All referenceable names (option/flag long names, aliases, positional
    /// names).
    names: HashSet<String>,
    /// Names that carry a declared value type, mapped to the comparand a
    /// `value-is` literal must be valid for (already unwrapped to the element
    /// type for repeatable options and tail positionals). A name present in
    /// [`Self::names`] but absent here is a flag (no value type).
    typed: std::collections::HashMap<String, ValueComparand<'a>>,
}

impl<'a> Validator<'a> {
    fn new(tool: &'a Tool) -> Self {
        Self {
            tool,
            errors: Vec::new(),
            input_roots: Vec::new(),
            used_ref_ids: HashSet::new(),
        }
    }

    fn run(&mut self) {
        self.check_duplicate_type_ids();

        if self.tool.commands.nodes.is_empty() {
            self.errors.push(ToolValidationError::EmptyCommandTree);
        }

        if !self.tool.commands.nodes.is_empty() {
            let mut visited = HashSet::new();
            let mut on_stack = HashSet::new();
            self.visit_command(CommandIndex(0), &[], &mut visited, &mut on_stack);
            for i in 0..self.tool.commands.nodes.len() {
                if !visited.contains(&(i as i32)) {
                    self.errors
                        .push(ToolValidationError::UnreachableCommandNode { index: i as i32 });
                }
            }
        }

        self.check_no_variant_in_input();
        self.check_def_refs();
    }

    fn command_node(&self, index: CommandIndex) -> Option<&'a CommandNode> {
        index
            .as_usize()
            .and_then(|i| self.tool.commands.nodes.get(i))
    }

    fn check_identifier(&mut self, kind: &'static str, value: &str) {
        if !is_valid_identifier(value) {
            self.errors.push(ToolValidationError::InvalidIdentifier {
                kind,
                value: value.to_string(),
            });
        }
    }

    // ---- command tree ----

    fn visit_command(
        &mut self,
        index: CommandIndex,
        ancestor_globals: &[&'a Globals],
        visited: &mut HashSet<i32>,
        on_stack: &mut HashSet<i32>,
    ) {
        if on_stack.contains(&index.0) {
            self.errors
                .push(ToolValidationError::CommandTreeCycle { index: index.0 });
            return;
        }
        if !visited.insert(index.0) {
            // Reached again through a different parent path: the command graph is
            // a DAG, not a tree.
            self.errors
                .push(ToolValidationError::DuplicateCommandParent { index: index.0 });
            return;
        }
        let Some(node) = self.command_node(index) else {
            return;
        };
        on_stack.insert(index.0);

        self.check_identifier("command name", &node.name);
        for alias in &node.aliases {
            self.check_identifier("command alias", alias);
        }
        self.check_globals(&node.name, &node.globals);
        self.check_global_scope_uniqueness(&node.name, ancestor_globals, &node.globals);

        // In-scope globals for this command's own body: ancestors plus this
        // command's own globals.
        let mut in_scope: Vec<&'a Globals> = ancestor_globals.to_vec();
        in_scope.push(&node.globals);

        if let Some(body) = &node.body {
            self.check_body(&node.name, body, &in_scope);
        }

        self.check_subcommand_uniqueness(node);

        for sub in &node.subcommands {
            let len = self.tool.commands.nodes.len();
            if sub.as_usize().filter(|i| *i < len).is_none() {
                self.errors
                    .push(ToolValidationError::CommandIndexOutOfBounds { index: sub.0, len });
                continue;
            }
            self.visit_command(*sub, &in_scope, visited, on_stack);
        }

        on_stack.remove(&index.0);
    }

    fn check_globals(&mut self, command: &str, globals: &'a Globals) {
        for opt in &globals.options {
            self.check_option_identifiers(opt);
            self.check_repeatable_map(command, opt);
            self.check_option_default(command, opt);
            self.check_type_well_formed(command, &opt.long, option_input_type(opt));
            // A global option is an input position; its full collected input
            // type (both map key and value for a repeatable-map) must not reach
            // a variant.
            self.input_roots.push(InputRoot {
                command: command.to_string(),
                position: opt.long.clone(),
                ty: option_input_type(opt),
            });
        }
        for flag in &globals.flags {
            self.check_identifier("flag long name", &flag.long);
            for alias in &flag.aliases {
                self.check_identifier("flag alias", alias);
            }
        }
    }

    /// Report collisions among the globals visible at a command: the command's
    /// own globals must be unique among themselves and against every ancestor
    /// global. Reported once, at the command that introduces the colliding
    /// token, so a collision is not re-reported at every descendant body.
    fn check_global_scope_uniqueness(
        &mut self,
        command: &str,
        ancestors: &[&'a Globals],
        own: &Globals,
    ) {
        let mut names: HashSet<String> = HashSet::new();
        let mut shorts: HashSet<char> = HashSet::new();
        for globals in ancestors {
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

        let mut add_name = |this: &mut Self, name: &str| {
            if !names.insert(name.to_string()) {
                this.errors.push(ToolValidationError::DuplicateName {
                    command: command.to_string(),
                    name: name.to_string(),
                });
            }
        };
        for opt in &own.options {
            add_name(self, &opt.long);
            for alias in &opt.aliases {
                add_name(self, alias);
            }
            if let Some(short) = opt.short
                && !shorts.insert(short)
            {
                self.errors.push(ToolValidationError::DuplicateShort {
                    command: command.to_string(),
                    short,
                });
            }
        }
        for flag in &own.flags {
            add_name(self, &flag.long);
            for alias in &flag.aliases {
                add_name(self, alias);
            }
            if let Some(short) = flag.short
                && !shorts.insert(short)
            {
                self.errors.push(ToolValidationError::DuplicateShort {
                    command: command.to_string(),
                    short,
                });
            }
        }
    }

    fn check_option_identifiers(&mut self, opt: &OptionSpec) {
        self.check_identifier("option long name", &opt.long);
        for alias in &opt.aliases {
            self.check_identifier("option alias", alias);
        }
    }

    /// A `repeatable-map` option collects its occurrences into a single map
    /// value, so its stored `map_type` must resolve to a [`SchemaType::Map`].
    fn check_repeatable_map(&mut self, command: &str, opt: &OptionSpec) {
        if let OptionShape::RepeatableMap(shape) = &opt.shape {
            // Whether the stored type is a map is decided by its *top-level*
            // resolution only. A `map_type` that resolves to a concrete non-map
            // type — or to a reference cycle, which can never resolve to a map —
            // is a genuine "not a map" error, even if some nested field is itself
            // a dangling reference. A `map_type` that is itself a *dangling*
            // reference cannot be classified and is reported separately as
            // `UnresolvedTypeRef`, so reporting a "not a map" error on top of it
            // would be misleading.
            let not_a_map = match self.tool.schema.resolve_ref(&shape.map_type) {
                Ok(SchemaType::Map { .. }) => false,
                Ok(_) | Err(RefResolutionError::RecursiveRef(_)) => true,
                Err(RefResolutionError::DanglingRef(_)) => false,
            };
            if not_a_map {
                self.errors
                    .push(ToolValidationError::RepeatableMapTypeNotMap {
                        command: command.to_string(),
                        name: opt.long.clone(),
                    });
            }
        }
    }

    /// Validate an option's `default` literal (if present) against its declared
    /// value type. A repeatable option's default is the whole collected value:
    /// a `repeatable-list` default is validated against `list<element>`, a
    /// `repeatable-map` default against the map node directly; scalar and
    /// optional-scalar defaults are validated against the scalar type directly.
    fn check_option_default(&mut self, command: &str, opt: &OptionSpec) {
        let Some(default) = &opt.default else {
            return;
        };
        let graph = &self.tool.schema;
        // A mismatch is suppressed only when it is *purely* an artifact of an
        // unresolved reference the value descended into (reported separately as
        // `UnresolvedTypeRef`); an independent shape mismatch is still reported.
        let result = match &opt.shape {
            OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => {
                validate_value(graph, ty, default)
            }
            OptionShape::RepeatableList(shape) => {
                let list_ty = SchemaType::list(shape.item_type.clone());
                validate_value(graph, &list_ty, default)
            }
            OptionShape::RepeatableMap(shape) => validate_value(graph, &shape.map_type, default),
        };
        if let Err(errors) = result
            && !value_mismatch_is_only_dangling(&errors)
        {
            self.errors.push(ToolValidationError::DefaultTypeMismatch {
                command: command.to_string(),
                name: opt.long.clone(),
            });
        }
    }

    /// Validate a fixed positional's `default` literal (if present) against its
    /// declared type.
    fn check_positional_default(&mut self, command: &str, positional: &Positional) {
        if let Some(default) = &positional.default
            && let Err(errors) = validate_value(&self.tool.schema, &positional.type_, default)
            // Suppress only when the mismatch is purely an artifact of an
            // unresolved reference (reported separately as `UnresolvedTypeRef`).
            && !value_mismatch_is_only_dangling(&errors)
        {
            self.errors.push(ToolValidationError::DefaultTypeMismatch {
                command: command.to_string(),
                name: positional.name.clone(),
            });
        }
    }

    /// Validate an embedded type at an input/output position for structural
    /// well-formedness against the tool's schema graph: every
    /// [`SchemaType::Ref`] must resolve to a definition, and every inline
    /// restriction (numeric bounds, text/binary ranges, union discriminators,
    /// ...) must be valid. References inside a resolved definition's body are
    /// not followed here; definition bodies are validated once by
    /// [`Self::check_def_refs`].
    ///
    /// A dangling reference is reported as
    /// [`ToolValidationError::UnresolvedTypeRef`]; any other well-formedness
    /// failure is reported as [`ToolValidationError::IllFormedSchema`]. When a
    /// type has dangling references, only those are reported (structural errors
    /// caused by the missing definition would be misleading noise).
    fn check_type_well_formed(&mut self, command: &str, position: &str, ty: &SchemaType) {
        collect_structural_ref_ids(ty, &mut self.used_ref_ids);
        if let Err(errors) = validate_root_type(&self.tool.schema, ty) {
            self.errors.extend(map_schema_errors(
                command.to_string(),
                position.to_string(),
                errors,
            ));
        }
    }

    /// Report definitions that share a type id. The wire [`GraphEncoder`] and
    /// schema well-formedness both require ids to be unique, so a duplicate is a
    /// producer-side invariant violation even though each individual body may be
    /// well-formed.
    fn check_duplicate_type_ids(&mut self) {
        let mut seen: HashSet<String> = HashSet::new();
        for def in &self.tool.schema.defs {
            if !seen.insert(def.id.to_string()) {
                self.errors.push(ToolValidationError::DuplicateTypeId {
                    id: def.id.to_string(),
                });
            }
        }
    }

    /// Check that every named definition body is well-formed and references only
    /// definitions that exist in the graph. `GraphEncoder` (in the WIT conversion layer) encodes
    /// all definition bodies eagerly, so an unresolved reference in any
    /// definition — even an unreferenced one — would fail wire encoding.
    fn check_def_refs(&mut self) {
        // A pure-alias definition (a bare `Ref` body) reports only an alias-chain
        // failure (dangling or recursive), and that exact failure is already
        // reported by every position whose `resolve_ref` walks into it: a command
        // position, or a constructor definition body (which is always validated
        // and whose nested refs are walked). Validating such a pure alias again
        // would double-report. So seed the suppression set from command-position
        // refs and from the refs structurally written in constructor definition
        // bodies, then follow pure-alias edges. Pure aliases nothing references
        // (including an unreferenced alias cycle) are *not* seeded, so they are
        // still validated and reported at their own `<def ...>` position.
        let mut seeds = self.used_ref_ids.clone();
        for def in &self.tool.schema.defs {
            if !matches!(def.body, SchemaType::Ref { .. }) {
                collect_structural_ref_ids(&def.body, &mut seeds);
            }
        }
        let covered = pure_alias_closure(&self.tool.schema, &seeds);
        // Ids carried by more than one definition. Pure-alias coverage is keyed
        // by `TypeId`, but a duplicate-id graph is already malformed (reported as
        // `DuplicateTypeId`) and id resolution is ambiguous, so suppression
        // cannot safely identify which same-id body is the covered alias.
        // Validate every duplicate-id body so none of their failures are lost.
        let mut seen_ids: HashSet<&TypeId> = HashSet::new();
        let mut duplicate_ids: HashSet<&TypeId> = HashSet::new();
        for def in &self.tool.schema.defs {
            if !seen_ids.insert(&def.id) {
                duplicate_ids.insert(&def.id);
            }
        }
        let mut reported: Vec<ToolValidationError> = Vec::new();
        for def in &self.tool.schema.defs {
            // Skip only a uniquely-named pure-alias definition that the closure
            // marked as covered: its single possible failure is already reported
            // at the position whose chain walk reaches it.
            if covered.contains(&def.id)
                && matches!(def.body, SchemaType::Ref { .. })
                && !duplicate_ids.contains(&def.id)
            {
                continue;
            }
            if let Err(errors) = validate_root_type(&self.tool.schema, &def.body) {
                let def_id = def.id.to_string();
                reported.extend(map_schema_errors(format!("<def {def_id}>"), def_id, errors));
            }
        }
        self.errors.extend(reported);
    }

    fn check_subcommand_uniqueness(&mut self, node: &'a CommandNode) {
        let mut seen: HashSet<&str> = HashSet::new();
        for sub in &node.subcommands {
            let Some(child) = self.command_node(*sub) else {
                continue;
            };
            for token in std::iter::once(&child.name).chain(child.aliases.iter()) {
                if !seen.insert(token.as_str()) {
                    self.errors
                        .push(ToolValidationError::DuplicateSubcommandName {
                            parent: node.name.clone(),
                            name: token.clone(),
                        });
                }
            }
        }
    }

    fn check_body(&mut self, command: &str, body: &'a CommandBody, in_scope: &[&'a Globals]) {
        // The tool's shared schema graph, used to resolve `value-is` comparands
        // through any `repeatable-map` `Ref` map types. Bound to the tool's `'a`
        // lifetime (independent of the `&mut self` borrow) so resolved types can
        // live in the name scope.
        let graph: &'a SchemaGraph = &self.tool.schema;
        // Build the in-scope global token sets first so body-local tokens can be
        // checked against them.
        let mut global_names: HashSet<String> = HashSet::new();
        let mut global_shorts: HashSet<char> = HashSet::new();
        for globals in in_scope {
            for opt in &globals.options {
                global_names.insert(opt.long.clone());
                global_names.extend(opt.aliases.iter().cloned());
                if let Some(short) = opt.short {
                    global_shorts.insert(short);
                }
            }
            for flag in &globals.flags {
                global_names.insert(flag.long.clone());
                global_names.extend(flag.aliases.iter().cloned());
                if let Some(short) = flag.short {
                    global_shorts.insert(short);
                }
            }
        }

        // Body-local uniqueness, accumulated incrementally so collisions both
        // within the body and against globals are reported.
        let mut names = global_names.clone();
        let mut shorts = global_shorts.clone();
        let mut add_name = |this: &mut Self, name: &str| {
            if !names.insert(name.to_string()) {
                this.errors.push(ToolValidationError::DuplicateName {
                    command: command.to_string(),
                    name: name.to_string(),
                });
            }
        };

        // The resolution scope (for constraint refs / value-is) includes both
        // globals and body-declared names.
        let mut scope = NameScope::default();
        for globals in in_scope {
            for opt in &globals.options {
                register_option(graph, &mut scope, opt);
            }
            for flag in &globals.flags {
                scope.names.insert(flag.long.clone());
                scope.names.extend(flag.aliases.iter().cloned());
            }
        }

        for opt in &body.options {
            self.check_option_identifiers(opt);
            add_name(self, &opt.long);
            for alias in &opt.aliases {
                add_name(self, alias);
            }
            if let Some(short) = opt.short
                && !shorts.insert(short)
            {
                self.errors.push(ToolValidationError::DuplicateShort {
                    command: command.to_string(),
                    short,
                });
            }
            register_option(graph, &mut scope, opt);
            self.check_repeatable_map(command, opt);
            self.check_option_default(command, opt);
            self.check_type_well_formed(command, &opt.long, option_input_type(opt));
            // A body option is an input position; its value type must not reach
            // a variant.
            self.input_roots.push(InputRoot {
                command: command.to_string(),
                position: opt.long.clone(),
                ty: option_input_type(opt),
            });
        }

        for flag in &body.flags {
            self.check_identifier("flag long name", &flag.long);
            for alias in &flag.aliases {
                self.check_identifier("flag alias", alias);
            }
            add_name(self, &flag.long);
            for alias in &flag.aliases {
                add_name(self, alias);
            }
            if let Some(short) = flag.short
                && !shorts.insert(short)
            {
                self.errors.push(ToolValidationError::DuplicateShort {
                    command: command.to_string(),
                    short,
                });
            }
            scope.names.insert(flag.long.clone());
            scope.names.extend(flag.aliases.iter().cloned());
        }

        for positional in &body.positionals.fixed {
            self.check_identifier("positional name", &positional.name);
            add_name(self, &positional.name);
            scope.names.insert(positional.name.clone());
            scope.typed.insert(
                positional.name.clone(),
                ValueComparand::Type(&positional.type_, ValueIsMode::WholeOrOnePeel),
            );
            self.check_positional_default(command, positional);
            self.check_type_well_formed(command, &positional.name, &positional.type_);
            self.input_roots.push(InputRoot {
                command: command.to_string(),
                position: positional.name.clone(),
                ty: &positional.type_,
            });
        }

        if let Some(tail) = &body.positionals.tail {
            self.check_identifier("positional name", &tail.name);
            add_name(self, &tail.name);
            scope.names.insert(tail.name.clone());
            // A tail positional collects occurrences into a list; a value-is
            // literal matches one item exactly (the per-occurrence item type),
            // never the whole collected list.
            scope.typed.insert(
                tail.name.clone(),
                ValueComparand::Type(&tail.item_type, ValueIsMode::Exact),
            );
            self.check_type_well_formed(command, &tail.name, &tail.item_type);
            self.input_roots.push(InputRoot {
                command: command.to_string(),
                position: tail.name.clone(),
                ty: &tail.item_type,
            });
            if tail.verbatim && tail.separator.is_none() {
                self.errors
                    .push(ToolValidationError::VerbatimWithoutSeparator {
                        command: command.to_string(),
                        positional: tail.name.clone(),
                    });
            }
        }

        for constraint in &body.constraints {
            self.check_constraint(command, constraint, &scope);
        }

        if let Some(result) = &body.result {
            for formatter in &result.formatters {
                self.check_identifier("formatter name", &formatter.name);
            }
            let resolved = result
                .formatters
                .iter()
                .any(|f| f.name == result.default_formatter);
            if !resolved {
                self.errors
                    .push(ToolValidationError::UnresolvedDefaultFormatter {
                        command: command.to_string(),
                        formatter: result.default_formatter.clone(),
                    });
            }
            self.check_type_well_formed(command, "result", &result.type_);
        }

        for error_case in &body.errors {
            self.check_identifier("error-case name", &error_case.name);
            if let Some(payload) = &error_case.payload {
                self.check_type_well_formed(command, &error_case.name, payload);
            }
        }
    }

    fn check_constraint(&mut self, command: &str, constraint: &Constraint, scope: &NameScope) {
        let refs = collect_refs(constraint);
        for r in refs {
            match r {
                Ref::Present(name) => {
                    if !scope.names.contains(name) {
                        self.errors
                            .push(ToolValidationError::UnresolvedConstraintRef {
                                command: command.to_string(),
                                name: name.clone(),
                            });
                    }
                }
                Ref::ValueIs(value_is) => {
                    if !scope.names.contains(&value_is.name) {
                        self.errors
                            .push(ToolValidationError::UnresolvedConstraintRef {
                                command: command.to_string(),
                                name: value_is.name.clone(),
                            });
                        continue;
                    }
                    match scope.typed.get(&value_is.name) {
                        Some(ValueComparand::Type(declared, mode)) => {
                            match self.value_is_outcome(declared, *mode, &value_is.value) {
                                ValueIsOutcome::Compatible => {}
                                ValueIsOutcome::Mismatch => {
                                    self.errors.push(ToolValidationError::ValueIsTypeMismatch {
                                        command: command.to_string(),
                                        name: value_is.name.clone(),
                                    });
                                }
                                ValueIsOutcome::BlockedByDanglingRef => {
                                    // The literal descended into an unresolved
                                    // reference, reported separately as
                                    // `UnresolvedTypeRef`; a value-is mismatch
                                    // here would be misleading cascade noise.
                                }
                            }
                        }
                        Some(ValueComparand::BlockedByTypeError) => {
                            // The name is typed but its declared type could not
                            // be resolved to a comparable value type (a
                            // repeatable-map whose `map_type` is not a map); the
                            // underlying type error is reported separately, so a
                            // value-is mismatch here would be misleading noise.
                        }
                        None => {
                            // Name resolves to a flag (no value type), so a
                            // value-is literal cannot be type-compatible.
                            self.errors.push(ToolValidationError::ValueIsTypeMismatch {
                                command: command.to_string(),
                                name: value_is.name.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    /// Classify a `value-is` literal against the declared type. The literal is
    /// [`ValueIsOutcome::Compatible`] if it is a valid value for the declared
    /// type. For a [`ValueIsMode::WholeOrOnePeel`] comparand (a non-collecting
    /// value surface) it is *also* compatible — under the "any element / entry
    /// equals this literal" relaxation — for the element type of a list/fixed-list
    /// or the value type of a map (optionally `option`-wrapped) declared type. A
    /// [`ValueIsMode::Exact`] comparand (a collecting surface, whose type is
    /// already the per-occurrence type) gets no relaxation: matching one more
    /// level would descend past a single occurrence.
    ///
    /// When neither comparison holds, the result is [`ValueIsOutcome::Mismatch`]
    /// unless the relevant comparison failed *purely* because the value
    /// descended into an unresolved reference, in which case it is
    /// [`ValueIsOutcome::BlockedByDanglingRef`] and the mismatch is suppressed
    /// (the unresolved reference is reported separately as `UnresolvedTypeRef`).
    fn value_is_outcome(
        &self,
        declared: &SchemaType,
        mode: ValueIsMode,
        value: &SchemaValue,
    ) -> ValueIsOutcome {
        let graph = &self.tool.schema;
        let direct = validate_value(graph, declared, value);
        if direct.is_ok() {
            return ValueIsOutcome::Compatible;
        }

        // A collecting surface's comparand is already the per-occurrence type; no
        // element/value relaxation applies. Classify on the direct comparison.
        if mode == ValueIsMode::Exact {
            return match direct {
                Err(errors) if value_mismatch_is_only_dangling(&errors) => {
                    ValueIsOutcome::BlockedByDanglingRef
                }
                _ => ValueIsOutcome::Mismatch,
            };
        }

        // "any element/entry" relaxation: peel `option` wrappers (resolving refs
        // along the way) and, for a list/fixed-list-shaped declared type, compare
        // the literal against the element type, or for a map-shaped type against
        // the value type. When this relaxation applies, its comparison is the
        // relevant one for classification.
        if let Ok(mut peeled) = graph.resolve_ref(declared) {
            while let SchemaType::Option { inner, .. } = peeled {
                match graph.resolve_ref(inner) {
                    Ok(next) => peeled = next,
                    // The wrapped type is a *dangling* reference, so neither the
                    // option-inner nor the element relaxation can be decided; the
                    // missing reference is reported separately as
                    // `UnresolvedTypeRef`, so the mismatch is suppressed.
                    Err(RefResolutionError::DanglingRef(_)) => {
                        return ValueIsOutcome::BlockedByDanglingRef;
                    }
                    // A reference cycle never bottoms out in a concrete type, so
                    // no literal can ever be valid for it: a genuine mismatch.
                    Err(RefResolutionError::RecursiveRef(_)) => {
                        return ValueIsOutcome::Mismatch;
                    }
                }
            }
            let relaxed = match peeled {
                SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                    Some(element.as_ref())
                }
                SchemaType::Map { value: v, .. } => Some(v.as_ref()),
                _ => None,
            };
            if let Some(inner_ty) = relaxed {
                return match validate_value(graph, inner_ty, value) {
                    Ok(()) => ValueIsOutcome::Compatible,
                    Err(errors) if value_mismatch_is_only_dangling(&errors) => {
                        ValueIsOutcome::BlockedByDanglingRef
                    }
                    Err(_) => ValueIsOutcome::Mismatch,
                };
            }
        }

        // No relaxation applied: classify on the direct comparison.
        match direct {
            Err(errors) if value_mismatch_is_only_dangling(&errors) => {
                ValueIsOutcome::BlockedByDanglingRef
            }
            _ => ValueIsOutcome::Mismatch,
        }
    }

    fn check_no_variant_in_input(&mut self) {
        let graph = &self.tool.schema;
        let mut errors = Vec::new();
        for root in &self.input_roots {
            let mut visited: HashSet<String> = HashSet::new();
            if type_reaches_variant(graph, root.ty, &mut visited) {
                errors.push(ToolValidationError::VariantInInputPosition {
                    command: root.command.clone(),
                    position: root.position.clone(),
                });
            }
        }
        self.errors.extend(errors);
    }
}

/// The type a `value-is` literal for this option is compared against. For a
/// `repeatable-list` option this is the element type and for a `repeatable-map`
/// option the map's value type, matching the "any occurrence / entry equals this
/// literal" semantics.
///
/// A `repeatable-map`'s `map_type` may itself be a [`SchemaType::Ref`], so it is
/// resolved through `graph` before its value type is extracted. Returns `None`
/// when the map type is dangling or does not resolve to a [`SchemaType::Map`];
/// those shapes are reported separately (`UnresolvedTypeRef` /
/// `RepeatableMapTypeNotMap`), so the `value-is` comparand is simply skipped to
/// avoid a misleading cascading `ValueIsTypeMismatch`.
fn option_value_type<'a>(graph: &'a SchemaGraph, opt: &'a OptionSpec) -> Option<&'a SchemaType> {
    match &opt.shape {
        OptionShape::Scalar(t) | OptionShape::OptionalScalar(t) => Some(t),
        OptionShape::RepeatableList(shape) => Some(&shape.item_type),
        OptionShape::RepeatableMap(shape) => match graph.resolve_ref(&shape.map_type).ok()? {
            SchemaType::Map { value, .. } => Some(value),
            _ => None,
        },
    }
}

/// The full collected input type of an option, used for type-reference
/// resolution and the "no variant in input position" check. A `repeatable-list`
/// stores its element type (the `list` wrapper carries no extra node); a
/// `repeatable-map` stores the whole `map` node so both key and value types are
/// reached.
fn option_input_type(opt: &OptionSpec) -> &SchemaType {
    match &opt.shape {
        OptionShape::Scalar(t) | OptionShape::OptionalScalar(t) => t,
        OptionShape::RepeatableList(shape) => &shape.item_type,
        OptionShape::RepeatableMap(shape) => &shape.map_type,
    }
}

fn register_option<'a>(graph: &'a SchemaGraph, scope: &mut NameScope<'a>, opt: &'a OptionSpec) {
    scope.names.insert(opt.long.clone());
    scope.names.extend(opt.aliases.iter().cloned());
    // An option always carries a value type, so it is always registered as a
    // comparand. When that type does not resolve to a comparable value type (a
    // repeatable-map whose `map_type` is not a map) the comparand is
    // `BlockedByTypeError` so `value-is` checking is suppressed; the underlying
    // type error is reported elsewhere. A dangling reference inside an otherwise
    // comparable type is handled at check time by [`Validator::value_is_outcome`].
    let comparand = match option_value_type(graph, opt) {
        Some(ty) => ValueComparand::Type(ty, option_value_is_mode(opt)),
        None => ValueComparand::BlockedByTypeError,
    };
    scope.typed.insert(opt.long.clone(), comparand);
    for alias in &opt.aliases {
        scope.typed.insert(alias.clone(), comparand);
    }
}

/// The `value-is` matching mode for an option: a scalar / optional-scalar option
/// is a non-collecting value surface (its declared value is matched with the
/// one-level relaxation); a repeatable-list or repeatable-map option collects
/// occurrences, so its per-occurrence comparand (element / map value type) is
/// matched exactly.
fn option_value_is_mode(opt: &OptionSpec) -> ValueIsMode {
    match &opt.shape {
        OptionShape::Scalar(_) | OptionShape::OptionalScalar(_) => ValueIsMode::WholeOrOnePeel,
        OptionShape::RepeatableList(_) | OptionShape::RepeatableMap(_) => ValueIsMode::Exact,
    }
}

/// Returns `true` if `ty` (resolving named references against `graph`) reaches a
/// [`SchemaType::Variant`]. `visited` records the ref ids currently being
/// resolved so recursive graphs terminate.
fn type_reaches_variant(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visited: &mut HashSet<String>,
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

/// Returns `true` when a value/type comparison failed *only* because the value
/// descended into an unresolved (dangling) reference — every reported
/// [`ValueError`] is a [`ValueError::DanglingRef`]. In that case the dependent
/// mismatch (default / `value-is`) is an artifact of the missing reference,
/// which is reported separately as [`ToolValidationError::UnresolvedTypeRef`],
/// so it is suppressed. An empty error slice never counts as "only dangling".
fn value_mismatch_is_only_dangling(errors: &[ValueError]) -> bool {
    !errors.is_empty()
        && errors
            .iter()
            .all(|e| matches!(e, ValueError::DanglingRef { .. }))
}

/// Map [`SchemaError`]s from well-formedness validation of a single embedded
/// type into position-aware [`ToolValidationError`]s. A dangling reference
/// becomes [`ToolValidationError::UnresolvedTypeRef`]; any other structural
/// failure becomes [`ToolValidationError::IllFormedSchema`].
///
/// Each error is mapped independently: a dangling reference never hides a
/// genuine, unrelated schema error elsewhere in the same type (for example an
/// invalid numeric restriction or a non-primitive map key on a sibling field).
/// Well-formedness already avoids emitting cascade errors that are merely an
/// artifact of a missing reference (for example it does not report
/// `MapKeyNotPrimitive` for a key that is itself a dangling reference), so no
/// suppression is needed here.
fn map_schema_errors(
    command: String,
    position: String,
    errors: Vec<SchemaError>,
) -> Vec<ToolValidationError> {
    // A single position can reference the same broken target id from several
    // spots (for example two record fields aliasing the same dangling type, or
    // two members of one record naming the same recursive alias). Those are a
    // single fact about that target id at this position, so collapse repeated
    // alias-chain failures (`DanglingRef` / `RecursiveAlias`) that share a
    // target id. Structural errors carry no shared target and are never merged:
    // two distinct ill-formed spots remain two facts.
    let mut seen_chain_targets: HashSet<TypeId> = HashSet::new();
    let mut mapped = Vec::with_capacity(errors.len());
    for error in errors {
        match error {
            SchemaError::DanglingRef(id) => {
                if seen_chain_targets.insert(id.clone()) {
                    mapped.push(ToolValidationError::UnresolvedTypeRef {
                        command: command.clone(),
                        position: position.clone(),
                        id: id.to_string(),
                    });
                }
            }
            SchemaError::RecursiveAlias(id) => {
                if seen_chain_targets.insert(id.clone()) {
                    mapped.push(ToolValidationError::IllFormedSchema {
                        command: command.clone(),
                        position: position.clone(),
                        detail: SchemaError::RecursiveAlias(id).to_string(),
                    });
                }
            }
            other => mapped.push(ToolValidationError::IllFormedSchema {
                command: command.clone(),
                position: position.clone(),
                detail: other.to_string(),
            }),
        }
    }
    mapped
}

/// Push every [`SchemaType::Ref`] id written structurally in `ty` (descending
/// through all inline constructor children, but not following the refs into
/// definition bodies) into `out`. This mirrors the set of refs the use-site
/// well-formedness check walks, so it can seed pure-alias suppression.
fn collect_structural_ref_ids(ty: &SchemaType, out: &mut HashSet<TypeId>) {
    match ty {
        SchemaType::Ref { id, .. } => {
            out.insert(id.clone());
        }
        SchemaType::Record { fields, .. } => {
            for field in fields {
                collect_structural_ref_ids(&field.body, out);
            }
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let Some(payload) = &case.payload {
                    collect_structural_ref_ids(payload, out);
                }
            }
        }
        SchemaType::Tuple { elements, .. } => {
            for element in elements {
                collect_structural_ref_ids(element, out);
            }
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            collect_structural_ref_ids(element, out);
        }
        SchemaType::Map { key, value, .. } => {
            collect_structural_ref_ids(key, out);
            collect_structural_ref_ids(value, out);
        }
        SchemaType::Option { inner, .. } => collect_structural_ref_ids(inner, out),
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = &spec.ok {
                collect_structural_ref_ids(ok, out);
            }
            if let Some(err) = &spec.err {
                collect_structural_ref_ids(err, out);
            }
        }
        SchemaType::Union { spec, .. } => {
            for branch in &spec.branches {
                collect_structural_ref_ids(&branch.body, out);
            }
        }
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(inner) = inner {
                collect_structural_ref_ids(inner, out);
            }
        }
        _ => {}
    }
}

/// The set of pure-alias definitions (a bare [`SchemaType::Ref`] body) reachable
/// from `seeds` by following only pure-alias edges. A use-site `resolve_ref`
/// walks exactly these chains, so any dangling/recursive-alias failure they
/// carry is already reported at the use site and must not be reported again
/// while validating these definitions' own bodies. The walk marks each
/// pure-alias definition it visits, stops at constructor bodies and missing
/// definitions (which are not marked, so they are still validated), and guards
/// against cycles.
fn pure_alias_closure(graph: &SchemaGraph, seeds: &HashSet<TypeId>) -> HashSet<TypeId> {
    let mut covered = HashSet::new();
    for seed in seeds {
        let mut current = seed.clone();
        while let Some(def) = graph.lookup(&current) {
            let SchemaType::Ref { id, .. } = &def.body else {
                break;
            };
            if !covered.insert(current.clone()) {
                break;
            }
            current = id.clone();
        }
    }
    covered
}

/// Flatten every [`Ref`] referenced by a constraint, regardless of nesting.
fn collect_refs(constraint: &Constraint) -> Vec<&Ref> {
    match constraint {
        Constraint::RequiresAll(refs)
        | Constraint::AllOrNone(refs)
        | Constraint::RequiresAny(refs) => refs.iter().collect(),
        Constraint::MutexGroups(groups) => groups.iter().flat_map(|g| g.refs.iter()).collect(),
        Constraint::Implies(implies) => implies.lhs.iter().chain(implies.rhs.iter()).collect(),
        Constraint::Forbids(forbids) => forbids.lhs.iter().chain(forbids.rhs.iter()).collect(),
    }
}
