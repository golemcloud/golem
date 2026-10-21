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

impl PreparedToolDescriptor {
    pub fn extended(&self) -> &ExtendedToolType {
        &self.extended
    }

    pub fn native(&self) -> &native::Tool {
        &self.native
    }

    #[cfg(feature = "guest")]
    pub fn wire(&self) -> golem_schema::schema::tool::wit::wire::Tool {
        golem_schema::schema::tool::wit::encode_tool(&self.native)
            .expect("prepared tool metadata must remain encodable")
    }

    fn build(extended: ExtendedToolType) -> Result<Self, ToolBuildError> {
        let native = extended.build_native_tool()?;
        #[cfg(feature = "guest")]
        golem_schema::schema::tool::wit::encode_tool(&native)
            .map_err(|error| ToolBuildError::EncodeError(error.to_string()))?;
        Ok(Self { extended, native })
    }
}

/// Build the value schema graph for a tool parameter/result Rust type,
/// returning a [`ToolBuildError`] (instead of panicking) when the type resolves
/// to an auto-injected schema, which has no value graph. Used by macro-generated
/// descriptors so an auto-injected param/result is a clean descriptor-build
/// error rather than a panic at registration time.
pub fn native_tool_value_schema<T: golem_schema::schema::IntoSchema>(
    position: &str,
) -> Result<SchemaGraph, ToolBuildError> {
    golem_schema::schema::try_into_schema_graph::<T>().map_err(|error| {
        ToolBuildError::IllFormedSchema {
            position: position.to_string(),
            detail: error.to_string(),
        }
    })
}

fn native_doc(doc: &Doc) -> native::Doc {
    native::Doc {
        summary: doc.summary.clone(),
        description: doc.description.clone(),
        examples: doc
            .examples
            .iter()
            .map(|example| native::Example {
                title: example.title.clone(),
                body: example.body.clone(),
            })
            .collect(),
    }
}

fn native_repetition(repetition: &wire::Repetition) -> native::Repetition {
    match repetition {
        wire::Repetition::Repeated => native::Repetition::Repeated,
        wire::Repetition::Delimited(delimiter) => native::Repetition::Delimited(*delimiter),
        wire::Repetition::Either(delimiter) => native::Repetition::Either(*delimiter),
    }
}

fn native_option(option: &ExtendedOptionSpec) -> native::OptionSpec {
    native::OptionSpec {
        long: option.long.clone(),
        short: option.short,
        aliases: option.aliases.clone(),
        doc: native_doc(&option.doc),
        value_name: option.value_name.clone(),
        shape: match &option.shape {
            ExtendedOptionShape::Scalar(graph) => native::OptionShape::Scalar(graph.root.clone()),
            ExtendedOptionShape::OptionalScalar(graph) => {
                native::OptionShape::OptionalScalar(graph.root.clone())
            }
            ExtendedOptionShape::RepeatableList(shape) => {
                native::OptionShape::RepeatableList(native::RepeatableListShape {
                    repetition: native_repetition(&shape.repetition),
                    item_type: shape.item_type.root.clone(),
                })
            }
            ExtendedOptionShape::RepeatableMap(shape) => {
                native::OptionShape::RepeatableMap(native::RepeatableMapShape {
                    repetition: native_repetition(&shape.repetition),
                    map_type: shape.map_type.root.clone(),
                    duplicate_key_policy: match shape.duplicate_key_policy {
                        wire::DuplicateKeyPolicy::Reject => native::DuplicateKeyPolicy::Reject,
                        wire::DuplicateKeyPolicy::LastWins => native::DuplicateKeyPolicy::LastWins,
                    },
                })
            }
        },
        default: option.default.clone(),
        required: option.required,
        env_var: option.env_var.clone(),
    }
}

fn native_flag(flag: &FlagSpec) -> native::FlagSpec {
    native::FlagSpec {
        long: flag.long.clone(),
        short: flag.short,
        aliases: flag.aliases.clone(),
        doc: native_doc(&flag.doc),
        shape: match flag.shape {
            wire::FlagShape::BoolFlag(ref shape) => {
                native::FlagShape::BoolFlag(native::BoolFlagShape {
                    default: shape.default,
                    negatable: shape.negatable,
                })
            }
            wire::FlagShape::CountFlag(max) => native::FlagShape::CountFlag(max),
        },
        env_var: flag.env_var.clone(),
    }
}

fn native_ref(reference: &ExtendedRef) -> Result<native::Ref, ToolBuildError> {
    match reference {
        ExtendedRef::Present(name) => Ok(native::Ref::Present(name.clone())),
        ExtendedRef::ValueIs(value_is) => {
            let value = match &value_is.value {
                ExtendedValueIsLiteral::Resolved(value) => value.clone(),
                ExtendedValueIsLiteral::Deferred(_) => {
                    return Err(ToolBuildError::UnresolvedValueIsLiteral(
                        value_is.name.clone(),
                    ));
                }
            };
            Ok(native::Ref::ValueIs(native::ValueIsRef {
                name: value_is.name.clone(),
                value,
            }))
        }
    }
}

fn native_refs(references: &[ExtendedRef]) -> Result<Vec<native::Ref>, ToolBuildError> {
    references.iter().map(native_ref).collect()
}

fn native_quantifier(quantifier: wire::Quantifier) -> native::Quantifier {
    match quantifier {
        wire::Quantifier::All => native::Quantifier::All,
        wire::Quantifier::Any => native::Quantifier::Any,
    }
}

fn native_constraint(
    constraint: &ExtendedConstraint,
) -> Result<native::Constraint, ToolBuildError> {
    match constraint {
        ExtendedConstraint::RequiresAll(references) => {
            Ok(native::Constraint::RequiresAll(native_refs(references)?))
        }
        ExtendedConstraint::AllOrNone(references) => {
            Ok(native::Constraint::AllOrNone(native_refs(references)?))
        }
        ExtendedConstraint::RequiresAny(references) => {
            Ok(native::Constraint::RequiresAny(native_refs(references)?))
        }
        ExtendedConstraint::MutexGroups(groups) => Ok(native::Constraint::MutexGroups(
            groups
                .iter()
                .map(|group| {
                    Ok(native::RefGroup {
                        refs: native_refs(&group.refs)?,
                    })
                })
                .collect::<Result<Vec<_>, ToolBuildError>>()?,
        )),
        ExtendedConstraint::Implies(implies) => Ok(native::Constraint::Implies(native::ImpliesC {
            lhs_quant: native_quantifier(implies.lhs_quant),
            lhs: native_refs(&implies.lhs)?,
            rhs_quant: native_quantifier(implies.rhs_quant),
            rhs: native_refs(&implies.rhs)?,
        })),
        ExtendedConstraint::Forbids(forbids) => Ok(native::Constraint::Forbids(native::ForbidsC {
            lhs_quant: native_quantifier(forbids.lhs_quant),
            lhs: native_refs(&forbids.lhs)?,
            rhs: native_refs(&forbids.rhs)?,
        })),
    }
}

fn native_stream(stream: &StreamSpec) -> native::StreamSpec {
    native::StreamSpec {
        doc: native_doc(&stream.doc),
        mime: stream.mime.clone(),
        required: stream.required,
    }
}

fn native_command_body(body: &ExtendedCommandBody) -> Result<native::CommandBody, ToolBuildError> {
    Ok(native::CommandBody {
        positionals: native::Positionals {
            fixed: body
                .positionals
                .fixed
                .iter()
                .map(|positional| native::Positional {
                    name: positional.name.clone(),
                    doc: native_doc(&positional.doc),
                    value_name: positional.value_name.clone(),
                    type_: positional.type_.root.clone(),
                    default: positional.default.clone(),
                    required: positional.required,
                    accepts_stdio: positional.accepts_stdio,
                })
                .collect(),
            tail: body
                .positionals
                .tail
                .as_ref()
                .map(|tail| native::TailPositional {
                    name: tail.name.clone(),
                    doc: native_doc(&tail.doc),
                    value_name: tail.value_name.clone(),
                    item_type: tail.item_type.root.clone(),
                    min: tail.min,
                    max: tail.max,
                    separator: tail.separator.clone(),
                    verbatim: tail.verbatim,
                    accepts_stdio: tail.accepts_stdio,
                }),
        },
        options: body.options.iter().map(native_option).collect(),
        flags: body.flags.iter().map(native_flag).collect(),
        constraints: body
            .constraints
            .iter()
            .map(native_constraint)
            .collect::<Result<Vec<_>, _>>()?,
        stdin: body.stdin.as_ref().map(native_stream),
        stdout: body.stdout.as_ref().map(native_stream),
        stderr: body.stderr.as_ref().map(native_stream),
        result: body.result.as_ref().map(|result| native::ResultSpec {
            type_: result.type_.root.clone(),
            doc: native_doc(&result.doc),
            formatters: result
                .formatters
                .iter()
                .map(|formatter| native::Formatter {
                    name: formatter.name.clone(),
                    doc: native_doc(&formatter.doc),
                })
                .collect(),
            default_formatter: result.default_formatter.clone(),
        }),
        errors: body
            .errors
            .iter()
            .map(|error| native::ErrorCase {
                name: error.name.clone(),
                doc: native_doc(&error.doc),
                kind: match error.kind {
                    wire::ErrorKind::UsageError => native::ErrorKind::UsageError,
                    wire::ErrorKind::RuntimeError => native::ErrorKind::RuntimeError,
                },
                exit_code: error.exit_code,
                payload: error.payload.as_ref().map(|payload| payload.root.clone()),
            })
            .collect(),
        annotations: body
            .annotations
            .as_ref()
            .map(|annotations| native::CommandAnnotations {
                read_only: annotations.read_only,
                destructive: annotations.destructive,
                idempotent: annotations.idempotent,
                open_world: annotations.open_world,
            }),
    })
}

fn native_command_node(
    command: &ExtendedCommandNode,
) -> Result<native::CommandNode, ToolBuildError> {
    Ok(native::CommandNode {
        name: command.name.clone(),
        aliases: command.aliases.clone(),
        doc: native_doc(&command.doc),
        globals: native::Globals {
            options: command.globals.options.iter().map(native_option).collect(),
            flags: command.globals.flags.iter().map(native_flag).collect(),
        },
        subcommands: command
            .subcommands
            .iter()
            .copied()
            .map(native::CommandIndex)
            .collect(),
        body: command.body.as_ref().map(native_command_body).transpose()?,
    })
}

impl ExtendedToolType {
    pub fn tool_name(&self) -> &str {
        self.commands.first().map(|c| c.name.as_str()).unwrap_or("")
    }

    /// Prepare an already normalized, genuinely dynamic descriptor with all
    /// schema and literal checks enabled.
    pub fn prepare(self) -> Result<PreparedToolDescriptor, ToolBuildError> {
        validate_tool(&self)?;
        PreparedToolDescriptor::build(self)
    }

    /// Prepare a generated descriptor with no defaults or `value-is` literals.
    /// Unexpected literals are rejected, not ignored. Schema checks remain
    /// enabled and are selected using macro facts and resolved Rust types.
    pub fn prepare_without_literals(
        self,
        schemas: ToolSchemaChecks,
    ) -> Result<PreparedToolDescriptor, ToolBuildError> {
        validate_tool_with::<false>(&self, schemas.0)?;
        PreparedToolDescriptor::build(self)
    }

    #[cfg(feature = "guest")]
    pub fn to_tool(&self) -> golem_schema::schema::tool::wit::wire::Tool {
        self.try_to_tool().expect("failed to build tool metadata")
    }

    #[cfg(feature = "guest")]
    pub fn try_to_tool(
        &self,
    ) -> Result<golem_schema::schema::tool::wit::wire::Tool, ToolBuildError> {
        let native = self.try_to_native_tool()?;
        golem_schema::schema::tool::wit::encode_tool(&native)
            .map_err(|error| ToolBuildError::EncodeError(error.to_string()))
    }

    /// Builds the SDK-owned recursive metadata model used by middleware APIs.
    pub fn try_to_native_tool(&self) -> Result<native::Tool, ToolBuildError> {
        validate_tool(self)?;
        self.build_native_tool()
    }

    fn build_native_tool(&self) -> Result<native::Tool, ToolBuildError> {
        let schema = merge_agent_graphs(collect_schema_graphs(self))
            .map_err(|error| ToolBuildError::EncodeError(error.to_string()))?;
        let commands = self
            .commands
            .iter()
            .map(native_command_node)
            .collect::<Result<Vec<_>, _>>()?;
        let mut native_schema = SchemaGraph::empty();
        native_schema.defs = schema.defs;
        Ok(native::Tool {
            version: self.version.clone(),
            requires_filesystem: self.requires_filesystem,
            commands: native::CommandTree { nodes: commands },
            schema: native_schema,
        })
    }
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
