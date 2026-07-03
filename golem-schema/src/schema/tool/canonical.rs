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

//! Canonical input model for tool commands.
//!
//! `golem:tool/guest.invoke` takes a single record `typed-schema-value` whose
//! fields are the flattened input surface of the selected command body. The
//! *canonical input model* defines that record's deterministic field order,
//! shared by every tool client (which encodes the record) and every tool
//! implementation (which decodes it):
//!
//! 1. effective inherited globals along the command path, in path order
//!    (options before flags within each node),
//! 2. the body's fixed positionals, in declaration order,
//! 3. the body's tail positional (collected as `list<item>`), if any,
//! 4. the body's options, in declaration order,
//! 5. the body's flags, in declaration order.
//!
//! An inherited global is shadowed (skipped) when any of its surface names
//! (long name or alias) collides with any body-local surface name. A
//! well-formed tool never has such a collision, but this module may be called
//! on a not-yet-validated tool, and surfacing the body-local field is the
//! least misleading fallback.
//!
//! Field types are expressed with the collected value type of each surface: a
//! `repeatable-list` option and a tail positional collect into `list<item>`, a
//! `repeatable-map` option into its map node, a bool flag into `bool`, and a
//! count flag into `u32`.

use super::{CommandNode, FlagShape, FlagSpec, OptionShape, OptionSpec, TailPositional, Tool};
use crate::schema::graph::{SchemaGraph, reachable_defs};
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;
use crate::schema::tool::validation::ToolValidationError;
use crate::schema::validation::well_formedness::{SchemaError, validate_graph};
use std::collections::BTreeSet;

/// One field of a command's canonical input record. `type_` may reference
/// definitions in the owning [`Tool`]'s shared [`SchemaGraph`].
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalInputField {
    pub name: String,
    pub aliases: Vec<String>,
    pub type_: SchemaType,
}

/// The full canonical input record shape of one command: the ordered fields
/// and the self-contained record schema (definitions projected from the tool's
/// shared graph to those reachable from the record).
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalInputModel {
    pub fields: Vec<CanonicalInputField>,
    pub record_schema: SchemaGraph,
}

/// One decoded field of a canonical input record.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalInputValue {
    pub name: String,
    pub aliases: Vec<String>,
    pub type_: SchemaType,
    pub value: SchemaValue,
}

/// Failure while decoding a canonical input record against a
/// [`CanonicalInputModel`].
#[derive(Clone, Debug, PartialEq)]
pub enum CanonicalInputDecodeError {
    ExpectedRecord,
    FieldCountMismatch { expected: usize, actual: usize },
    Model(ToolValidationError),
}

impl std::fmt::Display for CanonicalInputDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanonicalInputDecodeError::ExpectedRecord => {
                write!(f, "canonical tool input must be a record value")
            }
            CanonicalInputDecodeError::FieldCountMismatch { expected, actual } => write!(
                f,
                "canonical tool input record has {actual} fields, expected {expected}"
            ),
            CanonicalInputDecodeError::Model(error) => {
                write!(f, "invalid canonical input model: {error}")
            }
        }
    }
}

impl std::error::Error for CanonicalInputDecodeError {}

/// An inherited option or flag in scope for a command, collected along the
/// path from the root.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum EffectiveCommandField {
    Option(OptionSpec),
    Flag(FlagSpec),
}

/// One canonical input surface of a command, identified by its position in
/// the tool definition rather than by name (see
/// [`Tool::canonical_input_surfaces`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalSurfaceRef {
    /// An inherited global option: `node` indexes the command tree, `index`
    /// the node's `globals.options`.
    GlobalOption { node: usize, index: usize },
    /// An inherited global flag: `node` indexes the command tree, `index` the
    /// node's `globals.flags`.
    GlobalFlag { node: usize, index: usize },
    /// A fixed positional of the command's body.
    BodyPositional { index: usize },
    /// The tail positional of the command's body.
    BodyTail,
    /// An option of the command's body.
    BodyOption { index: usize },
    /// A flag of the command's body.
    BodyFlag { index: usize },
}

impl CanonicalInputModel {
    /// Splits a record value into per-field [`CanonicalInputValue`]s following
    /// this model's field order.
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
                type_: field.type_,
                value,
            })
            .collect())
    }
}

impl Tool {
    /// Resolves a `command-path` (as passed to `golem:tool` `invoke`) to the
    /// index of the command node whose body it selects. An empty path selects
    /// the root command. Each segment matches a subcommand's name or alias.
    /// Returns `None` when a segment does not match or the selected command
    /// has no body.
    pub fn command_index_by_path(&self, command_path: &[String]) -> Option<usize> {
        let mut current = 0usize;
        if self.commands.nodes.is_empty() {
            return None;
        }
        for segment in command_path {
            let next = self.commands.nodes[current]
                .subcommands
                .iter()
                .find_map(|idx| {
                    let idx = idx.as_usize()?;
                    let node = self.commands.nodes.get(idx)?;
                    (node.name == *segment || node.aliases.iter().any(|alias| alias == segment))
                        .then_some(idx)
                })?;
            current = next;
        }
        self.commands.nodes[current].body.as_ref().map(|_| current)
    }

    /// The inherited globals in scope for `command_index`, collected along the
    /// path from the root in path order (options before flags within each
    /// node).
    pub fn effective_globals(&self, command_index: usize) -> Vec<EffectiveCommandField> {
        let path = self.path_to(command_index).unwrap_or_default();
        let mut result = Vec::new();
        for idx in path {
            let Some(node) = self.commands.nodes.get(idx) else {
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

    /// The ordered canonical input surfaces of the command at `command_index`,
    /// identified by their position in the tool definition (see the module
    /// documentation for the ordering and shadowing rules).
    ///
    /// This is the single source of the canonical field *ordering*: callers
    /// with richer per-surface data (such as the Rust SDK's extended
    /// descriptor model) map each returned reference back onto their own
    /// structures by position, which stays unambiguous even for invalid tools
    /// with duplicate surface names.
    pub fn canonical_input_surfaces(&self, command_index: usize) -> Vec<CanonicalSurfaceRef> {
        let body = self
            .commands
            .nodes
            .get(command_index)
            .and_then(|c| c.body.as_ref());
        // Body surface names include option/flag aliases, so an inherited
        // global is shadowed when *any* of its surface names (long or alias)
        // collides with any body-local surface name. A well-formed tool never
        // has such a collision, but this method may be called on a
        // not-yet-validated tool, and surfacing the body-local field is the
        // least misleading fallback.
        let mut body_names: BTreeSet<&str> = BTreeSet::new();
        if let Some(body) = body {
            for p in &body.positionals.fixed {
                body_names.insert(p.name.as_str());
            }
            if let Some(t) = &body.positionals.tail {
                body_names.insert(t.name.as_str());
            }
            for o in &body.options {
                body_names.insert(o.long.as_str());
                body_names.extend(o.aliases.iter().map(String::as_str));
            }
            for f in &body.flags {
                body_names.insert(f.long.as_str());
                body_names.extend(f.aliases.iter().map(String::as_str));
            }
        }

        let shadowed = |long: &str, aliases: &[String]| {
            body_names.contains(long)
                || aliases
                    .iter()
                    .any(|alias| body_names.contains(alias.as_str()))
        };

        let mut surfaces = Vec::new();
        for node_index in self.path_to(command_index).unwrap_or_default() {
            let Some(node) = self.commands.nodes.get(node_index) else {
                continue;
            };
            for (index, option) in node.globals.options.iter().enumerate() {
                if shadowed(&option.long, &option.aliases) {
                    continue;
                }
                surfaces.push(CanonicalSurfaceRef::GlobalOption {
                    node: node_index,
                    index,
                });
            }
            for (index, flag) in node.globals.flags.iter().enumerate() {
                if shadowed(&flag.long, &flag.aliases) {
                    continue;
                }
                surfaces.push(CanonicalSurfaceRef::GlobalFlag {
                    node: node_index,
                    index,
                });
            }
        }
        if let Some(body) = body {
            surfaces.extend(
                (0..body.positionals.fixed.len())
                    .map(|index| CanonicalSurfaceRef::BodyPositional { index }),
            );
            if body.positionals.tail.is_some() {
                surfaces.push(CanonicalSurfaceRef::BodyTail);
            }
            surfaces.extend(
                (0..body.options.len()).map(|index| CanonicalSurfaceRef::BodyOption { index }),
            );
            surfaces
                .extend((0..body.flags.len()).map(|index| CanonicalSurfaceRef::BodyFlag { index }));
        }
        surfaces
    }

    /// The canonical input field for one surface reference of the command at
    /// `command_index`, or `None` if the reference does not resolve.
    pub fn canonical_field_for_surface(
        &self,
        command_index: usize,
        surface: CanonicalSurfaceRef,
    ) -> Option<CanonicalInputField> {
        let body = || {
            self.commands
                .nodes
                .get(command_index)
                .and_then(|c| c.body.as_ref())
        };
        match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => {
                let option = self.commands.nodes.get(node)?.globals.options.get(index)?;
                Some(CanonicalInputField {
                    name: option.long.clone(),
                    aliases: option.aliases.clone(),
                    type_: option_collected_type(&option.shape),
                })
            }
            CanonicalSurfaceRef::GlobalFlag { node, index } => {
                let flag = self.commands.nodes.get(node)?.globals.flags.get(index)?;
                Some(CanonicalInputField {
                    name: flag.long.clone(),
                    aliases: flag.aliases.clone(),
                    type_: flag_type(flag),
                })
            }
            CanonicalSurfaceRef::BodyPositional { index } => {
                let positional = body()?.positionals.fixed.get(index)?;
                Some(CanonicalInputField {
                    name: positional.name.clone(),
                    aliases: Vec::new(),
                    type_: positional.type_.clone(),
                })
            }
            CanonicalSurfaceRef::BodyTail => {
                let tail = body()?.positionals.tail.as_ref()?;
                Some(CanonicalInputField {
                    name: tail.name.clone(),
                    aliases: Vec::new(),
                    type_: tail_collected_type(tail),
                })
            }
            CanonicalSurfaceRef::BodyOption { index } => {
                let option = body()?.options.get(index)?;
                Some(CanonicalInputField {
                    name: option.long.clone(),
                    aliases: option.aliases.clone(),
                    type_: option_collected_type(&option.shape),
                })
            }
            CanonicalSurfaceRef::BodyFlag { index } => {
                let flag = body()?.flags.get(index)?;
                Some(CanonicalInputField {
                    name: flag.long.clone(),
                    aliases: flag.aliases.clone(),
                    type_: flag_type(flag),
                })
            }
        }
    }

    /// The ordered canonical input fields of the command at `command_index`
    /// (see the module documentation for the ordering and shadowing rules).
    pub fn canonical_input_fields(&self, command_index: usize) -> Vec<CanonicalInputField> {
        self.canonical_input_surfaces(command_index)
            .into_iter()
            .map(|surface| {
                self.canonical_field_for_surface(command_index, surface)
                    .expect("canonical_input_surfaces returned an unresolved surface")
            })
            .collect()
    }

    /// The canonical input model of the command at `command_index`: the
    /// ordered fields plus the self-contained record schema.
    pub fn canonical_input_model(
        &self,
        command_index: usize,
    ) -> Result<CanonicalInputModel, ToolValidationError> {
        self.check_canonical_input_command_index(command_index)?;
        let fields = self.canonical_input_fields(command_index);
        let record_schema = self.canonical_input_record_schema_for_fields(&fields)?;
        Ok(CanonicalInputModel {
            fields,
            record_schema,
        })
    }

    /// The self-contained record schema of the command's canonical input: a
    /// record with one field per canonical input field, with the tool's shared
    /// definitions projected to those reachable from the record.
    pub fn canonical_input_record_schema(
        &self,
        command_index: usize,
    ) -> Result<SchemaGraph, ToolValidationError> {
        self.check_canonical_input_command_index(command_index)?;
        let fields = self.canonical_input_fields(command_index);
        self.canonical_input_record_schema_for_fields(&fields)
    }

    /// Decodes a canonical input record value against the command's canonical
    /// input model.
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

    fn canonical_input_record_schema_for_fields(
        &self,
        fields: &[CanonicalInputField],
    ) -> Result<SchemaGraph, ToolValidationError> {
        let command = self
            .commands
            .nodes
            .first()
            .map(|node| node.name.clone())
            .unwrap_or_default();
        let root = SchemaType::record(
            fields
                .iter()
                .map(|field| NamedFieldType {
                    name: field.name.clone(),
                    body: field.type_.clone(),
                    metadata: MetadataEnvelope::default(),
                })
                .collect(),
        );
        let defs = reachable_defs(&self.schema, &root);
        // Projection is first-def-wins, so a duplicate definition id in the
        // shared graph would silently disappear here even though validation
        // rejects it. Keep the defensive behavior of this module aligned with
        // `validate_tool` for the reachable definitions.
        for def in &defs {
            if self
                .schema
                .defs
                .iter()
                .filter(|candidate| candidate.id == def.id)
                .count()
                > 1
            {
                return Err(ToolValidationError::DuplicateTypeId {
                    id: def.id.to_string(),
                });
            }
        }
        let graph = SchemaGraph { defs, root };
        check_graph_closed(&graph, command, "canonical input record")?;
        Ok(graph)
    }

    fn check_canonical_input_command_index(
        &self,
        command_index: usize,
    ) -> Result<(), ToolValidationError> {
        check_command_tree_structure(self)?;
        if command_index >= self.commands.nodes.len() {
            return Err(ToolValidationError::CommandIndexOutOfBounds {
                index: command_index as i32,
                len: self.commands.nodes.len(),
            });
        }
        if self.path_to(command_index).is_none() {
            return Err(ToolValidationError::UnreachableCommandNode {
                index: command_index as i32,
            });
        }
        Ok(())
    }

    fn path_to(&self, command_index: usize) -> Option<Vec<usize>> {
        fn visit(
            nodes: &[CommandNode],
            cur: usize,
            target: usize,
            path: &mut Vec<usize>,
            on_stack: &mut BTreeSet<usize>,
        ) -> bool {
            // Guards against malformed (cyclic) command trees so this helper
            // is safe to call before validation proves the tree acyclic.
            if !on_stack.insert(cur) {
                return false;
            }
            path.push(cur);
            if cur == target {
                return true;
            }
            for child in &nodes[cur].subcommands {
                if let Some(child) = child.as_usize()
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
        if !self.commands.nodes.is_empty()
            && visit(
                &self.commands.nodes,
                0,
                command_index,
                &mut path,
                &mut on_stack,
            )
        {
            Some(path)
        } else {
            None
        }
    }
}

/// Builds the self-contained canonical input record schema from per-field
/// self-contained graphs: each field graph must be closed, the definitions are
/// unioned (rejecting id collisions with conflicting bodies), and the record's
/// field types are the graph roots in field order.
///
/// This is the record construction used when the fields do not all come from
/// one [`Tool`]'s shared graph — for example when a client composes inherited
/// values captured from another tool's metadata with the current command's
/// fields.
pub fn record_schema_from_field_graphs<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a SchemaGraph)> + Clone,
) -> Result<SchemaGraph, ToolValidationError> {
    for (name, schema) in fields.clone() {
        check_graph_closed(
            schema,
            String::new(),
            &format!("canonical input field {name:?}"),
        )?;
    }
    let merged = crate::schema::conversion::merge_agent_graphs(
        fields.clone().into_iter().map(|(_, schema)| schema.clone()),
    )
    .map_err(|error| ToolValidationError::IllFormedSchema {
        command: String::new(),
        position: "canonical input record".to_string(),
        detail: error.to_string(),
    })?;
    let graph = SchemaGraph {
        defs: merged.defs,
        root: SchemaType::record(
            fields
                .into_iter()
                .map(|(name, schema)| NamedFieldType {
                    name: name.to_string(),
                    body: schema.root.clone(),
                    metadata: MetadataEnvelope::default(),
                })
                .collect(),
        ),
    };
    check_graph_closed(&graph, String::new(), "canonical input record")?;
    Ok(graph)
}

/// The whole collected value type of an option: a `repeatable-list` collects
/// into `list<item>`, a `repeatable-map` into its map node; scalar and
/// optional-scalar options use the value type directly.
pub fn option_collected_type(shape: &OptionShape) -> SchemaType {
    match shape {
        OptionShape::Scalar(t) | OptionShape::OptionalScalar(t) => t.clone(),
        OptionShape::RepeatableList(r) => SchemaType::list(r.item_type.clone()),
        OptionShape::RepeatableMap(r) => r.map_type.clone(),
    }
}

/// The collected value type of a tail positional: `list<item>`.
pub fn tail_collected_type(tail: &TailPositional) -> SchemaType {
    SchemaType::list(tail.item_type.clone())
}

/// The derived input-record field type for a flag: `bool` for a bool-flag,
/// `u32` for a count-flag. Flags carry no author-supplied value type.
pub fn flag_type(flag: &FlagSpec) -> SchemaType {
    match flag.shape {
        FlagShape::BoolFlag(_) => SchemaType::bool(),
        FlagShape::CountFlag(_) => SchemaType::u32(),
    }
}

/// Checks that the command tree is non-empty, in bounds, acyclic, and a tree
/// (every node reachable from the root through exactly one parent).
pub fn check_command_tree_structure(tool: &Tool) -> Result<(), ToolValidationError> {
    let len = tool.commands.nodes.len();
    if len == 0 {
        return Err(ToolValidationError::EmptyCommandTree);
    }
    for node in &tool.commands.nodes {
        for sub in &node.subcommands {
            if sub.0 < 0 || (sub.0 as usize) >= len {
                return Err(ToolValidationError::CommandIndexOutOfBounds { index: sub.0, len });
            }
        }
    }
    let mut visited = vec![false; len];
    let mut on_stack = vec![false; len];
    dfs_command_tree(tool, 0, &mut visited, &mut on_stack)?;
    for (i, seen) in visited.iter().enumerate() {
        if !seen {
            return Err(ToolValidationError::UnreachableCommandNode { index: i as i32 });
        }
    }
    Ok(())
}

fn dfs_command_tree(
    tool: &Tool,
    idx: usize,
    visited: &mut [bool],
    on_stack: &mut [bool],
) -> Result<(), ToolValidationError> {
    if on_stack[idx] {
        return Err(ToolValidationError::CommandTreeCycle { index: idx as i32 });
    }
    if visited[idx] {
        return Err(ToolValidationError::DuplicateCommandParent { index: idx as i32 });
    }
    visited[idx] = true;
    on_stack[idx] = true;
    for sub in &tool.commands.nodes[idx].subcommands {
        // Bounds were validated in check_command_tree_structure.
        dfs_command_tree(tool, sub.0 as usize, visited, on_stack)?;
    }
    on_stack[idx] = false;
    Ok(())
}

/// Rejects graphs with dangling refs or ill-formed types, reporting the first
/// error deterministically and preferring the precise dangling-ref variant.
fn check_graph_closed(
    graph: &SchemaGraph,
    command: String,
    position: &str,
) -> Result<(), ToolValidationError> {
    let errors = match validate_graph(graph) {
        Ok(()) => return Ok(()),
        Err(errors) => errors,
    };
    let first = errors
        .iter()
        .find(|e| matches!(e, SchemaError::DanglingRef(_)))
        .or_else(|| errors.first());
    match first {
        Some(SchemaError::DanglingRef(id)) => Err(ToolValidationError::UnresolvedTypeRef {
            command,
            position: position.to_string(),
            id: id.to_string(),
        }),
        Some(error) => Err(ToolValidationError::IllFormedSchema {
            command,
            position: position.to_string(),
            detail: error.to_string(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::graph::SchemaTypeDef;
    use crate::schema::metadata::TypeId;
    use crate::schema::tool::{
        BoolFlagShape, CommandBody, CommandIndex, CommandTree, Doc, Globals, Positional,
        Positionals, RepeatableListShape, RepeatableMapShape, Repetition, TailPositional,
    };
    use test_r::test;

    fn body() -> CommandBody {
        CommandBody {
            positionals: Positionals::default(),
            options: Vec::new(),
            flags: Vec::new(),
            constraints: Vec::new(),
            stdin: None,
            stdout: None,
            result: None,
            errors: Vec::new(),
            annotations: None,
        }
    }

    fn node(name: &str) -> CommandNode {
        CommandNode {
            name: name.to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            globals: Globals::default(),
            subcommands: Vec::new(),
            body: None,
        }
    }

    fn positional(name: &str, type_: SchemaType) -> Positional {
        Positional {
            name: name.to_string(),
            doc: Doc::default(),
            value_name: None,
            type_,
            default: None,
            required: true,
            accepts_stdio: false,
        }
    }

    fn option(long: &str, shape: OptionShape) -> OptionSpec {
        OptionSpec {
            long: long.to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            value_name: None,
            shape,
            default: None,
            required: false,
            env_var: None,
        }
    }

    fn bool_flag(long: &str) -> FlagSpec {
        FlagSpec {
            long: long.to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            shape: FlagShape::BoolFlag(BoolFlagShape {
                default: false,
                negatable: false,
            }),
            env_var: None,
        }
    }

    fn count_flag(long: &str) -> FlagSpec {
        FlagSpec {
            long: long.to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            shape: FlagShape::CountFlag(None),
            env_var: None,
        }
    }

    fn color_def() -> SchemaTypeDef {
        SchemaTypeDef {
            id: TypeId::from("color-mode"),
            name: None,
            body: SchemaType::r#enum(vec![
                "never".to_string(),
                "always".to_string(),
                "auto".to_string(),
            ]),
        }
    }

    fn color_ref() -> SchemaType {
        SchemaType::ref_to(TypeId::from("color-mode"))
    }

    /// A grep-like tool: root command `grep` with a `color` global option (a
    /// ref into the shared defs), a `case-sensitive` global flag, a `pattern`
    /// fixed positional, a `files` tail, an `extra-patterns` repeatable-list
    /// option, a `max-count` option, a `verbosity` count flag, and a `replace`
    /// subcommand.
    fn grep_tool() -> Tool {
        let mut root = node("grep");
        root.globals = Globals {
            options: vec![option("color", OptionShape::Scalar(color_ref()))],
            flags: vec![bool_flag("case-sensitive")],
        };
        root.body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![positional("pattern", SchemaType::string())],
                tail: Some(TailPositional {
                    name: "files".to_string(),
                    doc: Doc::default(),
                    value_name: None,
                    item_type: SchemaType::string(),
                    min: 0,
                    max: None,
                    separator: None,
                    verbatim: false,
                    accepts_stdio: false,
                }),
            },
            options: vec![
                option(
                    "extra-patterns",
                    OptionShape::RepeatableList(RepeatableListShape {
                        repetition: Repetition::Repeated,
                        item_type: SchemaType::string(),
                    }),
                ),
                option("max-count", OptionShape::Scalar(SchemaType::u32())),
            ],
            flags: vec![count_flag("verbosity")],
            ..body()
        });
        root.subcommands = vec![CommandIndex(1)];

        let mut replace = node("replace");
        replace.aliases = vec!["substitute".to_string()];
        replace.body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![
                    positional("pattern", SchemaType::string()),
                    positional("replacement", SchemaType::string()),
                ],
                tail: None,
            },
            ..body()
        });

        Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![root, replace],
            },
            schema: SchemaGraph {
                defs: vec![color_def()],
                root: SchemaType::record(Vec::new()),
            },
        }
    }

    #[test]
    fn canonical_input_field_order_at_root() {
        // Globals (options then flags in path order), then body fixed
        // positionals, tail, options, flags.
        let tool = grep_tool();
        let fields = tool.canonical_input_fields(0);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "color",
                "case-sensitive",
                "pattern",
                "files",
                "extra-patterns",
                "max-count",
                "verbosity",
            ]
        );
    }

    #[test]
    fn globals_are_effective_on_subcommand() {
        let tool = grep_tool();
        let effective = tool.effective_globals(1);
        assert!(
            effective
                .iter()
                .any(|g| matches!(g, EffectiveCommandField::Option(o) if o.long == "color"))
        );
        assert!(
            effective
                .iter()
                .any(|g| matches!(g, EffectiveCommandField::Flag(f) if f.long == "case-sensitive"))
        );

        let fields = tool.canonical_input_fields(1);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["color", "case-sensitive", "pattern", "replacement"]
        );
    }

    #[test]
    fn body_local_name_shadows_inherited_global() {
        let mut tool = grep_tool();
        // Give the subcommand a body-local option whose alias collides with
        // the root's `color` global.
        let mut clashing = option("colour", OptionShape::Scalar(SchemaType::string()));
        clashing.aliases = vec!["color".to_string()];
        tool.commands.nodes[1].body.as_mut().unwrap().options = vec![clashing];

        let fields = tool.canonical_input_fields(1);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["case-sensitive", "pattern", "replacement", "colour",],
            "the inherited `color` global is shadowed by the body-local alias"
        );
    }

    #[test]
    fn collected_field_types() {
        let tool = grep_tool();
        let fields = tool.canonical_input_fields(0);
        let by_name = |name: &str| {
            fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("field {name}"))
        };
        assert_eq!(by_name("color").type_, color_ref());
        assert_eq!(by_name("case-sensitive").type_, SchemaType::bool());
        assert_eq!(by_name("pattern").type_, SchemaType::string());
        assert_eq!(
            by_name("files").type_,
            SchemaType::list(SchemaType::string())
        );
        assert_eq!(
            by_name("extra-patterns").type_,
            SchemaType::list(SchemaType::string())
        );
        assert_eq!(by_name("max-count").type_, SchemaType::u32());
        assert_eq!(by_name("verbosity").type_, SchemaType::u32());
    }

    #[test]
    fn repeatable_map_collects_into_map_node() {
        let map_type = SchemaType::map(SchemaType::string(), SchemaType::string());
        let shape = OptionShape::RepeatableMap(RepeatableMapShape {
            repetition: Repetition::Repeated,
            map_type: map_type.clone(),
            duplicate_key_policy: crate::schema::tool::DuplicateKeyPolicy::Reject,
        });
        assert_eq!(option_collected_type(&shape), map_type);
    }

    #[test]
    fn command_index_by_path_resolves_names_and_aliases() {
        let tool = grep_tool();
        assert_eq!(tool.command_index_by_path(&[]), Some(0));
        assert_eq!(
            tool.command_index_by_path(&["replace".to_string()]),
            Some(1)
        );
        assert_eq!(
            tool.command_index_by_path(&["substitute".to_string()]),
            Some(1)
        );
        assert_eq!(tool.command_index_by_path(&["missing".to_string()]), None);
    }

    #[test]
    fn command_index_by_path_requires_a_body() {
        let mut tool = grep_tool();
        tool.commands.nodes[1].body = None;
        assert_eq!(tool.command_index_by_path(&["replace".to_string()]), None);

        let mut tool = grep_tool();
        tool.commands.nodes[0].body = None;
        assert_eq!(tool.command_index_by_path(&[]), None);
    }

    #[test]
    fn inherited_global_alias_is_shadowed_by_body_local_long_name() {
        let mut tool = grep_tool();
        // Give the root's `color` global an alias that matches a body-local
        // option long name on the subcommand.
        tool.commands.nodes[0].globals.options[0].aliases = vec!["hue".to_string()];
        tool.commands.nodes[1].body.as_mut().unwrap().options =
            vec![option("hue", OptionShape::Scalar(SchemaType::string()))];

        let fields = tool.canonical_input_fields(1);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["case-sensitive", "pattern", "replacement", "hue"],
            "the inherited global is shadowed when its alias matches a body-local long name"
        );
    }

    #[test]
    fn record_schema_rejects_duplicate_reachable_type_ids() {
        let mut tool = grep_tool();
        tool.schema.defs.push(color_def());
        let error = tool.canonical_input_model(0).expect_err("duplicate id");
        assert!(matches!(error, ToolValidationError::DuplicateTypeId { .. }));
    }

    #[test]
    fn record_schema_projects_reachable_defs() {
        let mut tool = grep_tool();
        // Add an unused def: it must not appear in the projected record schema.
        tool.schema.defs.push(SchemaTypeDef {
            id: TypeId::from("unused"),
            name: None,
            body: SchemaType::string(),
        });

        let model = tool.canonical_input_model(0).expect("valid model");
        assert_eq!(model.record_schema.defs.len(), 1);
        assert_eq!(model.record_schema.defs[0].id, TypeId::from("color-mode"));

        let SchemaType::Record { fields, .. } = &model.record_schema.root else {
            panic!("record schema root must be a record");
        };
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec![
                "color",
                "case-sensitive",
                "pattern",
                "files",
                "extra-patterns",
                "max-count",
                "verbosity",
            ]
        );
    }

    #[test]
    fn record_schema_rejects_dangling_refs() {
        let mut tool = grep_tool();
        tool.schema.defs.clear();
        let error = tool.canonical_input_model(0).expect_err("dangling ref");
        assert!(matches!(
            error,
            ToolValidationError::UnresolvedTypeRef { .. }
        ));
    }

    #[test]
    fn canonical_input_model_rejects_malformed_trees() {
        let mut tool = grep_tool();
        tool.commands.nodes[1].subcommands = vec![CommandIndex(0)];
        assert!(matches!(
            tool.canonical_input_model(0),
            Err(ToolValidationError::CommandTreeCycle { .. })
        ));

        let mut tool = grep_tool();
        tool.commands.nodes[0].subcommands.clear();
        assert!(matches!(
            tool.canonical_input_model(1),
            Err(ToolValidationError::UnreachableCommandNode { .. })
        ));

        let mut tool = grep_tool();
        tool.commands.nodes[0].subcommands = vec![CommandIndex(7)];
        assert!(matches!(
            tool.canonical_input_model(0),
            Err(ToolValidationError::CommandIndexOutOfBounds { .. })
        ));

        let mut tool = grep_tool();
        tool.commands.nodes.clear();
        assert!(matches!(
            tool.canonical_input_model(0),
            Err(ToolValidationError::EmptyCommandTree)
        ));
    }

    #[test]
    fn decode_record_splits_fields_in_model_order() {
        let tool = grep_tool();
        let model = tool.canonical_input_model(1).expect("valid model");
        let values = model
            .decode_record(SchemaValue::Record {
                fields: vec![
                    SchemaValue::Enum { case: 2 },
                    SchemaValue::Bool(true),
                    SchemaValue::String("a".to_string()),
                    SchemaValue::String("b".to_string()),
                ],
            })
            .expect("decodes");
        assert_eq!(
            values.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            vec!["color", "case-sensitive", "pattern", "replacement"]
        );
        assert_eq!(values[2].value, SchemaValue::String("a".to_string()));
    }

    #[test]
    fn decode_record_rejects_wrong_shape() {
        let tool = grep_tool();
        let model = tool.canonical_input_model(1).expect("valid model");
        assert_eq!(
            model.decode_record(SchemaValue::Bool(true)),
            Err(CanonicalInputDecodeError::ExpectedRecord)
        );
        assert_eq!(
            model.decode_record(SchemaValue::Record { fields: vec![] }),
            Err(CanonicalInputDecodeError::FieldCountMismatch {
                expected: 4,
                actual: 0
            })
        );
    }
}
