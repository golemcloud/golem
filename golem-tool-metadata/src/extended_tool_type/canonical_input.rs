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

use super::*;

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
                short: field.short,
                schema: field.schema,
                value,
            })
            .collect())
    }
}

/// Adapts one decoded canonical input field to the schema expected by a
/// generated implementation parameter or forwarded child command field.
///
/// Canonical records use an option carrier for scalar inputs that may be
/// absent. Generated Rust methods keep their authored parameter type, so a
/// present carrier is unwrapped when the destination is required and a bare
/// value is wrapped when the destination retains the optional carrier.
pub fn adapt_canonical_input_value(
    source: CanonicalInputValue,
    target_name: &str,
    target_schema: &SchemaGraph,
) -> Result<SchemaValue, String> {
    if is_equivalent_cross_graph(
        &source.schema,
        &source.schema.root,
        target_schema,
        &target_schema.root,
    ) {
        return Ok(source.value);
    }

    if let SchemaType::Option { inner, .. } = &source.schema.root
        && is_equivalent_cross_graph(&source.schema, inner, target_schema, &target_schema.root)
    {
        return match source.value {
            SchemaValue::Option { inner: Some(value) } => Ok(*value),
            SchemaValue::Option { inner: None } => Err(format!(
                "canonical tool input field `{}` is absent but forwarded field `{target_name}` is required",
                source.name
            )),
            _ => Err(format!(
                "canonical tool input field `{}` has an invalid optional carrier",
                source.name
            )),
        };
    }

    if let SchemaType::Option { inner, .. } = &target_schema.root
        && is_equivalent_cross_graph(&source.schema, &source.schema.root, target_schema, inner)
    {
        return Ok(SchemaValue::Option {
            inner: Some(Box::new(source.value)),
        });
    }

    Err(format!(
        "canonical tool input field `{}` has incompatible schema for forwarded field `{target_name}`",
        source.name
    ))
}

impl ExtendedToolType {
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
        let current = self.node_index_by_path(command_path)?;
        self.commands[current].body.as_ref().map(|_| current)
    }

    /// Resolves a command or namespace node without requiring an invokable body.
    pub fn node_index_by_path(&self, command_path: &[String]) -> Option<usize> {
        let mut current = 0usize;
        if self.commands.is_empty() {
            return None;
        }
        if command_path.is_empty() {
            return Some(current);
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
        Some(current)
    }

    /// Projects the canonical-relevant subset of this descriptor onto the
    /// shared native tool model: command tree topology, surface names/aliases,
    /// and the input surface shapes (positionals, tail, options, flags).
    /// Docs, constraints, results, errors, annotations, and the shared
    /// definition registry are not part of the projection: it is used only to
    /// resolve the shared canonical *ordering* (surface references), which
    /// never inspects definitions. The per-field schema graphs stay the
    /// original embedded graphs of this descriptor.
    pub(super) fn canonical_projection(&self) -> native::Tool {
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
                    stderr: None,
                    result: None,
                    errors: Vec::new(),
                    annotations: None,
                }),
            })
            .collect();

        native::Tool {
            version: self.version.clone(),
            requires_filesystem: self.requires_filesystem,
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
    /// name/aliases from the descriptor plus the self-contained canonical
    /// carrier graph for that surface (collected form for repeatable options
    /// and the tail, optional form for absent scalar inputs).
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
                    short: option.short,
                    schema: canonical_option_graph(option),
                })
            }
            CanonicalSurfaceRef::GlobalFlag { node, index } => {
                let flag = self.commands.get(node)?.globals.flags.get(index)?;
                Some(CanonicalInputField {
                    name: flag.long.clone(),
                    aliases: flag.aliases.clone(),
                    short: flag.short,
                    schema: flag_graph(flag),
                })
            }
            CanonicalSurfaceRef::BodyPositional { index } => {
                let positional = body()?.positionals.fixed.get(index)?;
                Some(CanonicalInputField {
                    name: positional.name.clone(),
                    aliases: Vec::new(),
                    short: None,
                    schema: canonical_positional_graph(positional),
                })
            }
            CanonicalSurfaceRef::BodyTail => {
                let tail = body()?.positionals.tail.as_ref()?;
                Some(CanonicalInputField {
                    name: tail.name.clone(),
                    aliases: Vec::new(),
                    short: None,
                    schema: list_wrapper_graph(&tail.item_type),
                })
            }
            CanonicalSurfaceRef::BodyOption { index } => {
                let option = body()?.options.get(index)?;
                Some(CanonicalInputField {
                    name: option.long.clone(),
                    aliases: option.aliases.clone(),
                    short: option.short,
                    schema: canonical_option_graph(option),
                })
            }
            CanonicalSurfaceRef::BodyFlag { index } => {
                let flag = body()?.flags.get(index)?;
                Some(CanonicalInputField {
                    name: flag.long.clone(),
                    aliases: flag.aliases.clone(),
                    short: flag.short,
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
        model
            .fields
            .iter()
            .zip(params)
            .map(|(field, (_, value))| canonical_input_carrier(field, value))
            .collect()
    } else {
        let mut fields: Vec<SchemaValue> = Vec::with_capacity(model.fields.len());
        for field in &model.fields {
            let index = params
                .iter()
                .rposition(|(name, _)| *name == field.name.as_str())
                .ok_or_else(|| format!("missing canonical tool input field `{}`", field.name))?;
            fields.push(canonical_input_carrier(field, params.remove(index).1));
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
            short: value.short,
            schema: value.schema.clone(),
        })
        .collect();
    let inherited_names: BTreeSet<&str> = inherited_prefix
        .iter()
        .flat_map(|value| {
            std::iter::once(value.name.as_str()).chain(value.aliases.iter().map(String::as_str))
        })
        .collect();
    canonical_fields.extend(
        command_fields
            .iter()
            .filter(|field| {
                !inherited_names.contains(field.name.as_str())
                    && !field
                        .aliases
                        .iter()
                        .any(|alias| inherited_names.contains(alias.as_str()))
            })
            .cloned(),
    );
    let model =
        CanonicalInputModel::from_fields(canonical_fields).map_err(|error| error.to_string())?;

    let mut fields: Vec<SchemaValue> = inherited_prefix
        .iter()
        .zip(model.fields.iter())
        .map(|(value, field)| canonical_input_carrier(field, value.value.clone()))
        .collect();
    for field in model.fields.iter().skip(inherited_prefix.len()) {
        let index = params
            .iter()
            .rposition(|(name, _)| *name == field.name.as_str())
            .ok_or_else(|| format!("missing canonical tool input field `{}`", field.name))?;
        fields.push(canonical_input_carrier(field, params.remove(index).1));
    }
    Ok(crate::TypedSchemaValue::new(
        model.record_schema,
        SchemaValue::Record { fields },
    ))
}

fn canonical_input_carrier(field: &CanonicalInputField, value: SchemaValue) -> SchemaValue {
    if matches!(field.schema.root, SchemaType::Option { .. })
        && !matches!(value, SchemaValue::Option { .. })
    {
        SchemaValue::Option {
            inner: Some(Box::new(value)),
        }
    } else {
        value
    }
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
