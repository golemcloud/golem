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
        stderr: value.stderr,
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
            requires_filesystem: value.requires_filesystem,
            http_mount: value.http_mount,
            commands: value
                .commands
                .nodes
                .into_iter()
                .map(|v| ExtendedCommandNode {
                    name: v.name,
                    aliases: v.aliases,
                    doc: v.doc,
                    http: v.http,
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
    use golem_schema::http::{LiteralSegment, PathSegment, PathVariable};
    use golem_schema::schema::{SchemaTypeDef, TypeId};
    use test_r::test;

    #[cfg(feature = "guest")]
    #[test]
    fn direct_wire_preserves_http_presence_without_defaults() {
        use crate::{WireToolSchema, WireTypeRef};
        use golem_schema::http::*;

        for http_mount in [
            None,
            Some(native::ToolHttpMount {
                path_prefix: vec![],
            }),
        ] {
            for http in [
                None,
                Some(native::CommandHttp::Prefix(native::ToolHttpPrefix {
                    path_suffix: vec![],
                })),
                Some(native::CommandHttp::Endpoint(native::ToolHttpEndpoint {
                    route_id: None,
                    http_method: HttpMethod::Custom(CustomHttpMethod {
                        value: "REPORT".into(),
                    }),
                    path_suffix: vec![PathSegment::RemainingPathVariable(PathVariable {
                        variable_name: "rest".into(),
                    })],
                    query_vars: vec![QueryVariable {
                        query_param_name: "q".into(),
                        variable_name: "query".into(),
                    }],
                    header_vars: vec![HeaderVariable {
                        header_name: "X-Test".into(),
                        variable_name: "test".into(),
                    }],
                    response_headers: vec![FileResponseHeader {
                        name: "X-Response".into(),
                        value: "é".into(),
                    }],
                    mode: None,
                    durable_streams: Some(DurableStreamRouteOptions {
                        slots: vec![DurableStreamSlotOptions {
                            source: DurableStreamSlotSource::Output(
                                DurableStreamOutputSlotSource {
                                    name: "stdout".into(),
                                },
                            ),
                            name: Some(String::new()),
                            content_type: None,
                        }],
                        load: Some(DurableStreamRouteLoadOptions {
                            max_concurrent_readers_per_stream: Some(u32::MAX),
                            max_append_requests_per_second_per_stream: Some(0),
                        }),
                    }),
                })),
            ] {
                let tool = native::Tool {
                    version: "1".into(),
                    requires_filesystem: false,
                    http_mount: http_mount.clone(),
                    schema: SchemaGraph::empty(),
                    commands: native::CommandTree {
                        nodes: vec![native::CommandNode {
                            name: "tool".into(),
                            aliases: vec![],
                            doc: native::Doc::default(),
                            http: http.clone(),
                            globals: native::Globals::default(),
                            subcommands: vec![],
                            body: None,
                        }],
                    },
                };
                let reflected = ExtendedToolType::from(tool.clone());
                assert_eq!(reflected.http_mount, tool.http_mount);
                assert_eq!(reflected.commands[0].http, tool.commands.nodes[0].http);
                if matches!(http, Some(CommandHttp::Endpoint(_))) {
                    assert!(reflected.try_to_native_tool().is_err());
                } else {
                    assert_eq!(reflected.try_to_native_tool().unwrap(), tool);
                    assert_eq!(
                        native::wit::decode_tool(reflected.try_to_tool().unwrap()).unwrap(),
                        tool
                    );
                }
                let direct = ExtendedToolType::<WireTypeRef> {
                    version: "1".into(),
                    requires_filesystem: false,
                    http_mount: http_mount.clone(),
                    commands: vec![ExtendedCommandNode {
                        name: "tool".into(),
                        aliases: vec![],
                        doc: native::Doc::default(),
                        http,
                        globals: ExtendedGlobals::default(),
                        subcommands: vec![],
                        body: None,
                    }],
                };
                assert_eq!(
                    native::wit::decode_tool(direct.into_wire(WireToolSchema::default()).unwrap())
                        .unwrap(),
                    tool
                );
            }
        }
    }

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
            requires_filesystem: false,
            http_mount: Some(native::ToolHttpMount {
                path_prefix: vec![PathSegment::Literal(LiteralSegment {
                    value: "reports".into(),
                })],
            }),
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
                        http: Some(native::CommandHttp::Prefix(native::ToolHttpPrefix {
                            path_suffix: vec![PathSegment::Literal(LiteralSegment {
                                value: "admin".into(),
                            })],
                        })),
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
                        http: Some(native::CommandHttp::Endpoint(native::ToolHttpEndpoint {
                            route_id: Some("custom-route".into()),
                            http_method: golem_schema::http::HttpMethod::Post(
                                golem_schema::Empty {},
                            ),
                            path_suffix: vec![
                                PathSegment::Literal(LiteralSegment {
                                    value: "run".into(),
                                }),
                                PathSegment::PathVariable(PathVariable {
                                    variable_name: "path".into(),
                                }),
                            ],
                            query_vars: vec![golem_schema::http::QueryVariable {
                                query_param_name: "m".into(),
                                variable_name: "mode".into(),
                            }],
                            header_vars: vec![golem_schema::http::HeaderVariable {
                                header_name: "X-Profile".into(),
                                variable_name: "profile".into(),
                            }],
                            response_headers: vec![golem_schema::http::FileResponseHeader {
                                name: "X-Custom".into(),
                                value: "yes".into(),
                            }],
                            mode: Some(golem_schema::http::HttpRouteMode::Plain),
                            durable_streams: Some(golem_schema::http::DurableStreamRouteOptions {
                                slots: vec![golem_schema::http::DurableStreamSlotOptions {
                                    source: golem_schema::http::DurableStreamSlotSource::Output(
                                        golem_schema::http::DurableStreamOutputSlotSource {
                                            name: "stdout".into(),
                                        },
                                    ),
                                    name: Some("download".into()),
                                    content_type: Some("text/plain".into()),
                                }],
                                load: Some(golem_schema::http::DurableStreamRouteLoadOptions {
                                    max_concurrent_readers_per_stream: Some(7),
                                    max_append_requests_per_second_per_stream: Some(13),
                                }),
                            }),
                        })),
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
                            stderr: None,
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
        #[cfg(feature = "guest")]
        assert_eq!(
            native::wit::decode_tool(reflected.try_to_tool().unwrap()).unwrap(),
            tool
        );
    }
}
