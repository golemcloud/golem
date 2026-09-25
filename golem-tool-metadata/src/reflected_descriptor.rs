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

use crate::*;
use golem_schema::schema::tool as native;

fn graph(root: SchemaType, defs: &[golem_schema::schema::SchemaTypeDef]) -> SchemaGraph {
    SchemaGraph {
        root,
        defs: defs.to_vec(),
    }
}

fn option(
    value: native::OptionSpec,
    defs: &[golem_schema::schema::SchemaTypeDef],
) -> ExtendedOptionSpec {
    ExtendedOptionSpec {
        long: value.long,
        short: value.short,
        aliases: value.aliases,
        doc: value.doc,
        value_name: value.value_name,
        shape: match value.shape {
            native::OptionShape::Scalar(root) => ExtendedOptionShape::Scalar(graph(root, defs)),
            native::OptionShape::OptionalScalar(root) => {
                ExtendedOptionShape::OptionalScalar(graph(root, defs))
            }
            native::OptionShape::RepeatableList(shape) => {
                ExtendedOptionShape::RepeatableList(ExtendedRepeatableListShape {
                    repetition: shape.repetition,
                    item_type: graph(shape.item_type, defs),
                })
            }
            native::OptionShape::RepeatableMap(shape) => {
                ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
                    repetition: shape.repetition,
                    map_type: graph(shape.map_type, defs),
                    duplicate_key_policy: shape.duplicate_key_policy,
                })
            }
        },
        default: value.default,
        required: value.required,
        env_var: value.env_var,
    }
}

fn reference(value: native::Ref) -> ExtendedRef {
    match value {
        native::Ref::Present(name) => ExtendedRef::Present(name),
        native::Ref::ValueIs(value) => ExtendedRef::ValueIs(ExtendedValueIsRef {
            name: value.name,
            value: ExtendedValueIsLiteral::Resolved(value.value),
        }),
    }
}

fn references(values: Vec<native::Ref>) -> Vec<ExtendedRef> {
    values.into_iter().map(reference).collect()
}

fn constraint(value: native::Constraint) -> ExtendedConstraint {
    match value {
        native::Constraint::RequiresAll(v) => ExtendedConstraint::RequiresAll(references(v)),
        native::Constraint::AllOrNone(v) => ExtendedConstraint::AllOrNone(references(v)),
        native::Constraint::RequiresAny(v) => ExtendedConstraint::RequiresAny(references(v)),
        native::Constraint::MutexGroups(v) => ExtendedConstraint::MutexGroups(
            v.into_iter()
                .map(|g| ExtendedRefGroup {
                    refs: references(g.refs),
                })
                .collect(),
        ),
        native::Constraint::Implies(v) => ExtendedConstraint::Implies(ExtendedImpliesC {
            lhs_quant: v.lhs_quant,
            lhs: references(v.lhs),
            rhs_quant: v.rhs_quant,
            rhs: references(v.rhs),
        }),
        native::Constraint::Forbids(v) => ExtendedConstraint::Forbids(ExtendedForbidsC {
            lhs_quant: v.lhs_quant,
            lhs: references(v.lhs),
            rhs: references(v.rhs),
        }),
    }
}

fn body(
    value: native::CommandBody,
    defs: &[golem_schema::schema::SchemaTypeDef],
) -> ExtendedCommandBody {
    ExtendedCommandBody {
        positionals: ExtendedPositionals {
            fixed: value
                .positionals
                .fixed
                .into_iter()
                .map(|v| ExtendedPositional {
                    name: v.name,
                    doc: v.doc,
                    value_name: v.value_name,
                    type_: graph(v.type_, defs),
                    default: v.default,
                    required: v.required,
                    accepts_stdio: v.accepts_stdio,
                })
                .collect(),
            tail: value.positionals.tail.map(|v| ExtendedTailPositional {
                name: v.name,
                doc: v.doc,
                value_name: v.value_name,
                item_type: graph(v.item_type, defs),
                min: v.min,
                max: v.max,
                separator: v.separator,
                verbatim: v.verbatim,
                accepts_stdio: v.accepts_stdio,
            }),
        },
        options: value.options.into_iter().map(|v| option(v, defs)).collect(),
        flags: value.flags,
        constraints: value.constraints.into_iter().map(constraint).collect(),
        stdin: value.stdin,
        stdout: value.stdout,
        result: value.result.map(|v| ExtendedResultSpec {
            type_: graph(v.type_, defs),
            doc: v.doc,
            formatters: v.formatters,
            default_formatter: v.default_formatter,
        }),
        errors: value
            .errors
            .into_iter()
            .map(|v| ExtendedErrorCase {
                name: v.name,
                doc: v.doc,
                kind: v.kind,
                exit_code: v.exit_code,
                payload: v.payload.map(|root| graph(root, defs)),
            })
            .collect(),
        annotations: value.annotations,
        positional_plan: Vec::new(),
    }
}

impl From<native::Tool> for ExtendedToolType {
    fn from(value: native::Tool) -> Self {
        let defs = value.schema.defs;
        Self {
            version: value.version,
            commands: value
                .commands
                .nodes
                .into_iter()
                .map(|v| ExtendedCommandNode {
                    name: v.name,
                    aliases: v.aliases,
                    doc: v.doc,
                    globals: ExtendedGlobals {
                        options: v
                            .globals
                            .options
                            .into_iter()
                            .map(|v| option(v, &defs))
                            .collect(),
                        flags: v.globals.flags,
                    },
                    subcommands: v.subcommands.into_iter().map(|v| v.0).collect(),
                    body: v.body.map(|v| body(v, &defs)),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_schema::schema::{SchemaTypeDef, TypeId};
    use test_r::test;

    #[test]
    fn native_descriptor_round_trips_through_reflection() {
        let named = TypeId("example/path".into());
        let doc = native::Doc {
            summary: "summary".into(),
            description: "description".into(),
            examples: vec![native::Example {
                title: "example".into(),
                body: "tool run".into(),
            }],
        };
        let mode = native::OptionSpec {
            long: "mode".into(),
            short: Some('m'),
            aliases: vec!["style".into()],
            doc: doc.clone(),
            value_name: Some("MODE".into()),
            shape: native::OptionShape::Scalar(SchemaType::string()),
            default: Some(SchemaValue::String("safe".into())),
            required: false,
            env_var: Some("MODE".into()),
        };
        let mut profile = mode.clone();
        profile.long = "profile".into();
        profile.short = Some('p');
        profile.aliases.clear();
        let tool = native::Tool {
            version: "1.2.3".into(),
            schema: SchemaGraph {
                root: SchemaType::Record {
                    fields: vec![],
                    metadata: Default::default(),
                },
                defs: vec![SchemaTypeDef {
                    id: named.clone(),
                    name: Some("Path".into()),
                    body: SchemaType::string(),
                }],
            },
            commands: native::CommandTree {
                nodes: vec![
                    native::CommandNode {
                        name: "tool".into(),
                        aliases: vec!["t".into()],
                        doc: doc.clone(),
                        globals: native::Globals {
                            options: vec![profile],
                            flags: vec![],
                        },
                        subcommands: vec![native::CommandIndex(1)],
                        body: None,
                    },
                    native::CommandNode {
                        name: "run".into(),
                        aliases: vec!["r".into()],
                        doc: doc.clone(),
                        globals: native::Globals::default(),
                        subcommands: vec![],
                        body: Some(native::CommandBody {
                            positionals: native::Positionals {
                                fixed: vec![native::Positional {
                                    name: "path".into(),
                                    doc: doc.clone(),
                                    value_name: Some("PATH".into()),
                                    type_: SchemaType::ref_to(named.clone()),
                                    default: None,
                                    required: true,
                                    accepts_stdio: true,
                                }],
                                tail: None,
                            },
                            options: vec![mode],
                            flags: vec![native::FlagSpec {
                                long: "force".into(),
                                short: Some('f'),
                                aliases: vec!["overwrite".into()],
                                doc: doc.clone(),
                                shape: native::FlagShape::BoolFlag(native::BoolFlagShape {
                                    default: false,
                                    negatable: true,
                                }),
                                env_var: None,
                            }],
                            constraints: vec![native::Constraint::RequiresAll(vec![
                                native::Ref::ValueIs(native::ValueIsRef {
                                    name: "mode".into(),
                                    value: SchemaValue::String("safe".into()),
                                }),
                            ])],
                            stdin: Some(native::StreamSpec {
                                doc: doc.clone(),
                                mime: vec!["text/plain".into()],
                                required: false,
                            }),
                            stdout: Some(native::StreamSpec {
                                doc: doc.clone(),
                                mime: vec!["application/json".into()],
                                required: true,
                            }),
                            result: Some(native::ResultSpec {
                                type_: SchemaType::ref_to(named.clone()),
                                doc: doc.clone(),
                                formatters: vec![native::Formatter {
                                    name: "json".into(),
                                    doc: doc.clone(),
                                }],
                                default_formatter: "json".into(),
                            }),
                            errors: vec![native::ErrorCase {
                                name: "failed".into(),
                                doc,
                                kind: native::ErrorKind::RuntimeError,
                                exit_code: 7,
                                payload: Some(SchemaType::ref_to(named)),
                            }],
                            annotations: Some(native::CommandAnnotations {
                                read_only: false,
                                destructive: true,
                                idempotent: true,
                                open_world: false,
                            }),
                        }),
                    },
                ],
            },
        };

        let reflected = ExtendedToolType::from(tool.clone());
        assert!(
            reflected
                .commands
                .iter()
                .filter_map(|command| command.body.as_ref())
                .all(|body| body.positional_plan.is_empty())
        );
        assert_eq!(reflected.try_to_native_tool().unwrap(), tool);
    }
}
