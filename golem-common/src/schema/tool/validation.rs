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
use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use crate::schema::validation::value::validate_value;
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
    /// An embedded type contains a [`SchemaType::Ref`] whose id does not
    /// resolve to a definition in the tool's [`SchemaGraph`].
    UnresolvedTypeRef {
        command: String,
        position: String,
        id: String,
    },
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
}

/// The set of names and declared types reachable from a command body, used for
/// uniqueness checks and constraint-ref resolution.
#[derive(Default)]
struct NameScope<'a> {
    /// All referenceable names (option/flag long names, aliases, positional
    /// names).
    names: HashSet<String>,
    /// Names that carry a declared value type, mapped to the type a `value-is`
    /// literal must be valid for (already unwrapped to the element type for
    /// repeatable options and tail positionals).
    typed: std::collections::HashMap<String, &'a SchemaType>,
}

impl<'a> Validator<'a> {
    fn new(tool: &'a Tool) -> Self {
        Self {
            tool,
            errors: Vec::new(),
            input_roots: Vec::new(),
        }
    }

    fn run(&mut self) {
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
            self.check_option_default(command, opt);
            self.check_type_refs(command, &opt.long, option_value_type(opt));
            // A global option is an input position; its value type must not
            // reach a variant.
            self.input_roots.push(InputRoot {
                command: command.to_string(),
                position: opt.long.clone(),
                ty: option_value_type(opt),
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

    /// Validate an option's `default` literal (if present) against its declared
    /// value type. A `repeatable` option's default is the whole repeated value,
    /// so it is validated against `list<element>`; scalar and optional-scalar
    /// defaults are validated against the scalar type directly.
    fn check_option_default(&mut self, command: &str, opt: &OptionSpec) {
        let Some(default) = &opt.default else {
            return;
        };
        let valid = match &opt.shape {
            OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => {
                validate_value(&self.tool.schema, ty, default).is_ok()
            }
            OptionShape::Repeatable(shape) => {
                let list_ty = SchemaType::list(shape.type_.clone());
                validate_value(&self.tool.schema, &list_ty, default).is_ok()
            }
        };
        if !valid {
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
            && validate_value(&self.tool.schema, &positional.type_, default).is_err()
        {
            self.errors.push(ToolValidationError::DefaultTypeMismatch {
                command: command.to_string(),
                name: positional.name.clone(),
            });
        }
    }

    /// Check that every [`SchemaType::Ref`] embedded (directly or through inline
    /// composites) in `ty` resolves to a definition in the tool's schema graph.
    /// References inside a resolved definition's body are not followed here;
    /// definition bodies are validated once by [`Self::check_def_refs`].
    fn check_type_refs(&mut self, command: &str, position: &str, ty: &SchemaType) {
        let mut unresolved = Vec::new();
        collect_dangling_refs(&self.tool.schema, ty, &mut unresolved);
        for id in unresolved {
            self.errors.push(ToolValidationError::UnresolvedTypeRef {
                command: command.to_string(),
                position: position.to_string(),
                id,
            });
        }
    }

    /// Check that every named definition body references only definitions that
    /// exist in the graph. [`GraphEncoder`](super::wit) encodes all definition
    /// bodies eagerly, so an unresolved reference in any definition — even an
    /// unreferenced one — would fail wire encoding.
    fn check_def_refs(&mut self) {
        let mut unresolved: Vec<(String, String)> = Vec::new();
        for def in &self.tool.schema.defs {
            let mut out = Vec::new();
            collect_dangling_refs(&self.tool.schema, &def.body, &mut out);
            for id in out {
                unresolved.push((def.id.to_string(), id));
            }
        }
        for (def_id, id) in unresolved {
            self.errors.push(ToolValidationError::UnresolvedTypeRef {
                command: format!("<def {def_id}>"),
                position: def_id,
                id,
            });
        }
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
                register_option(&mut scope, opt);
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
            register_option(&mut scope, opt);
            self.check_option_default(command, opt);
            self.check_type_refs(command, &opt.long, option_value_type(opt));
            // A body option is an input position; its value type must not reach
            // a variant.
            self.input_roots.push(InputRoot {
                command: command.to_string(),
                position: opt.long.clone(),
                ty: option_value_type(opt),
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
            scope
                .typed
                .insert(positional.name.clone(), &positional.type_);
            self.check_positional_default(command, positional);
            self.check_type_refs(command, &positional.name, &positional.type_);
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
            // A tail positional is list-like; a value-is literal matches an item.
            scope.typed.insert(tail.name.clone(), &tail.item_type);
            self.check_type_refs(command, &tail.name, &tail.item_type);
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
            self.check_type_refs(command, "result", &result.type_);
        }

        for error_case in &body.errors {
            self.check_identifier("error-case name", &error_case.name);
            if let Some(payload) = &error_case.payload {
                self.check_type_refs(command, &error_case.name, payload);
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
                        Some(declared) => {
                            if !self.value_is_compatible(declared, &value_is.value) {
                                self.errors.push(ToolValidationError::ValueIsTypeMismatch {
                                    command: command.to_string(),
                                    name: value_is.name.clone(),
                                });
                            }
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

    /// A `value-is` literal is compatible if it is a valid value for the
    /// declared type, or — for the "any element/occurrence" relaxation — for
    /// the element type of a list-shaped (optionally `option`-wrapped) declared
    /// type. Repeatable options already store their element type.
    fn value_is_compatible(&self, declared: &SchemaType, value: &SchemaValue) -> bool {
        let graph = &self.tool.schema;
        if validate_value(graph, declared, value).is_ok() {
            return true;
        }
        let Ok(mut peeled) = graph.resolve_ref(declared) else {
            return false;
        };
        // Peel `option` wrappers (resolving refs along the way).
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

/// The type a `value-is` literal for this option is compared against, and the
/// option's input value type. For a repeatable option this is the element type,
/// matching the "any occurrence equals this literal" semantics.
fn option_value_type(opt: &OptionSpec) -> &SchemaType {
    match &opt.shape {
        OptionShape::Scalar(t) | OptionShape::OptionalScalar(t) => t,
        OptionShape::Repeatable(shape) => &shape.type_,
    }
}

fn register_option<'a>(scope: &mut NameScope<'a>, opt: &'a OptionSpec) {
    scope.names.insert(opt.long.clone());
    scope.names.extend(opt.aliases.iter().cloned());
    // Repeatable options accept the element type per occurrence/element.
    let comparand = option_value_type(opt);
    scope.typed.insert(opt.long.clone(), comparand);
    for alias in &opt.aliases {
        scope.typed.insert(alias.clone(), comparand);
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

/// Collect the ids of every [`SchemaType::Ref`] reachable from `ty` through
/// inline composite types whose id is not present in `graph`. References inside
/// a resolved definition body are not followed (definition bodies are checked
/// separately), so no cycle guard is needed: inline composites form a finite
/// tree.
fn collect_dangling_refs(graph: &SchemaGraph, ty: &SchemaType, out: &mut Vec<String>) {
    match ty {
        SchemaType::Ref { id, .. } if graph.lookup(id).is_none() => {
            out.push(id.to_string());
        }
        SchemaType::Record { fields, .. } => {
            for f in fields {
                collect_dangling_refs(graph, &f.body, out);
            }
        }
        SchemaType::Variant { cases, .. } => {
            for c in cases {
                if let Some(payload) = &c.payload {
                    collect_dangling_refs(graph, payload, out);
                }
            }
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            collect_dangling_refs(graph, element, out);
        }
        SchemaType::Option { inner, .. } => collect_dangling_refs(graph, inner, out),
        SchemaType::Map { key, value, .. } => {
            collect_dangling_refs(graph, key, out);
            collect_dangling_refs(graph, value, out);
        }
        SchemaType::Tuple { elements, .. } => {
            for e in elements {
                collect_dangling_refs(graph, e, out);
            }
        }
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = &spec.ok {
                collect_dangling_refs(graph, ok, out);
            }
            if let Some(err) = &spec.err {
                collect_dangling_refs(graph, err, out);
            }
        }
        SchemaType::Union { spec, .. } => {
            for b in &spec.branches {
                collect_dangling_refs(graph, &b.body, out);
            }
        }
        SchemaType::Future { inner: Some(t), .. } | SchemaType::Stream { inner: Some(t), .. } => {
            collect_dangling_refs(graph, t, out);
        }
        _ => {}
    }
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
