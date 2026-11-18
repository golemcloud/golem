// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::canonical::check_command_tree_structure;
use super::validation::ToolValidationError;
use super::{CommandBody, Tool};
use crate::http::{
    DurableStreamRouteOptions, FileResponseHeader, HeaderVariable, HttpMethod, HttpRouteMode,
    PathSegment, QueryVariable,
};
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};

mod validation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ToolHttpMount {
    pub path_prefix: Vec<PathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum CommandHttp {
    Endpoint(ToolHttpEndpoint),
    Prefix(ToolHttpPrefix),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ToolHttpPrefix {
    pub path_suffix: Vec<PathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ToolHttpEndpoint {
    /// When absent, the route ID is the canonical command path joined with dots.
    pub route_id: Option<String>,
    pub http_method: HttpMethod,
    pub path_suffix: Vec<PathSegment>,
    pub query_vars: Vec<QueryVariable>,
    pub header_vars: Vec<HeaderVariable>,
    pub response_headers: Vec<FileResponseHeader>,
    pub mode: Option<HttpRouteMode>,
    pub durable_streams: Option<DurableStreamRouteOptions>,
}

impl ToolHttpEndpoint {
    /// Resolves this endpoint's mode from its own command, without inheriting
    /// annotations or changing the optional declaration.
    pub fn effective_mode(&self, body: &CommandBody) -> HttpRouteMode {
        self.mode.unwrap_or_else(|| {
            let idempotent = body.annotations.as_ref().is_some_and(|a| a.idempotent);
            if !idempotent
                && (body.stdin.is_some() || body.stdout.is_some() || body.stderr.is_some())
            {
                HttpRouteMode::DurableStreams
            } else {
                HttpRouteMode::Plain
            }
        })
    }
}

/// A typed endpoint composed from the tool mount and canonical command path.
/// Prefix declarations never produce one of these routes.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposedToolHttpEndpoint<'a> {
    pub command_index: usize,
    pub command_path: Vec<String>,
    pub route_id: String,
    pub path: Vec<PathSegment>,
    pub endpoint: &'a ToolHttpEndpoint,
    pub mode: HttpRouteMode,
}

impl Tool {
    /// Composes typed routes without publishing aliases or deriving suffixes
    /// from command names. The root's mount supplies its HTTP ancestry.
    pub fn http_endpoints(&self) -> Result<Vec<ComposedToolHttpEndpoint<'_>>, ToolValidationError> {
        check_command_tree_structure(self)?;
        let mut routes = Vec::new();
        self.compose_http_endpoints(0, &[], &[], None, &mut routes)?;
        Ok(routes)
    }

    fn compose_http_endpoints<'a>(
        &'a self,
        index: usize,
        command_path: &[String],
        parent_path: &[PathSegment],
        undeclared_ancestor: Option<&str>,
        routes: &mut Vec<ComposedToolHttpEndpoint<'a>>,
    ) -> Result<(), ToolValidationError> {
        let node = &self.commands.nodes[index];
        let mut path = if index == 0 {
            self.http_mount
                .as_ref()
                .map(|mount| mount.path_prefix.clone())
                .unwrap_or_default()
        } else {
            parent_path.to_vec()
        };
        match &node.http {
            Some(CommandHttp::Prefix(prefix)) => path.extend_from_slice(&prefix.path_suffix),
            Some(CommandHttp::Endpoint(endpoint)) => {
                let error = |detail: &str| ToolValidationError::InvalidHttpDeclaration {
                    command: command_path.join("."),
                    input: None,
                    detail: detail.into(),
                };
                if self.http_mount.is_none() {
                    return Err(error(
                        "an HTTP endpoint requires a tool mount on the enclosing tool",
                    ));
                }
                if let Some(ancestor) = undeclared_ancestor {
                    return Err(error(&format!(
                        "ancestor command `{ancestor}` requires an HTTP prefix or endpoint declaration",
                    )));
                }
                let body = node
                    .body
                    .as_ref()
                    .ok_or_else(|| error("an HTTP endpoint requires a command body"))?;
                path.extend_from_slice(&endpoint.path_suffix);
                routes.push(ComposedToolHttpEndpoint {
                    command_index: index,
                    command_path: command_path.to_vec(),
                    route_id: endpoint
                        .route_id
                        .clone()
                        .unwrap_or_else(|| command_path.join(".")),
                    path: path.clone(),
                    endpoint,
                    mode: endpoint.effective_mode(body),
                });
            }
            None => {}
        }
        let command = command_path.join(".");
        let undeclared_ancestor = undeclared_ancestor
            .or_else(|| (index != 0 && node.http.is_none()).then_some(command.as_str()));
        for child in &node.subcommands {
            let child = child.0 as usize;
            let mut command_path = command_path.to_vec();
            command_path.push(self.commands.nodes[child].name.clone());
            self.compose_http_endpoints(child, &command_path, &path, undeclared_ancestor, routes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Empty;
    use crate::http::{
        DurableStreamInputSlotSource, DurableStreamOutputSlotSource, DurableStreamRouteLoadOptions,
        DurableStreamSlotOptions, DurableStreamSlotSource, LiteralSegment, PathVariable,
    };
    use crate::schema::SchemaGraph;
    use crate::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use test_r::test;

    #[test]
    fn http_presence_and_boundary_values_survive_all_codecs() {
        use crate::http::CustomHttpMethod;
        use crate::{FromSchema, IntoSchema};

        for route_id in [None, Some(String::new()), Some("日本語.route".into())] {
            for mode in [
                None,
                Some(HttpRouteMode::Plain),
                Some(HttpRouteMode::DurableStreams),
                Some(HttpRouteMode::Sse),
                Some(HttpRouteMode::WebSocket),
            ] {
                for load in [
                    None,
                    Some(DurableStreamRouteLoadOptions {
                        max_concurrent_readers_per_stream: None,
                        max_append_requests_per_second_per_stream: None,
                    }),
                    Some(DurableStreamRouteLoadOptions {
                        max_concurrent_readers_per_stream: Some(0),
                        max_append_requests_per_second_per_stream: Some(u32::MAX),
                    }),
                ] {
                    for durable_streams in [
                        None,
                        Some(DurableStreamRouteOptions {
                            slots: vec![],
                            load: load.clone(),
                        }),
                        Some(DurableStreamRouteOptions {
                            slots: vec![DurableStreamSlotOptions {
                                source: DurableStreamSlotSource::Input(
                                    DurableStreamInputSlotSource {
                                        name: String::new(),
                                    },
                                ),
                                name: Some(String::new()),
                                content_type: Some(String::new()),
                            }],
                            load: load.clone(),
                        }),
                    ] {
                        let paths = vec![PathSegment::RemainingPathVariable(PathVariable {
                            variable_name: "rest".into(),
                        })];
                        let endpoint = ToolHttpEndpoint {
                            route_id: route_id.clone(),
                            http_method: HttpMethod::Custom(CustomHttpMethod {
                                value: "PROPFIND".into(),
                            }),
                            path_suffix: paths.clone(),
                            query_vars: vec![],
                            header_vars: vec![],
                            response_headers: vec![],
                            mode,
                            durable_streams,
                        };
                        let tool = Tool {
                            version: "1".into(),
                            requires_filesystem: false,
                            http_mount: Some(ToolHttpMount {
                                path_prefix: paths.clone(),
                            }),
                            commands: CommandTree {
                                nodes: vec![CommandNode {
                                    name: "tool".into(),
                                    aliases: vec![],
                                    doc: Doc::default(),
                                    globals: Globals::default(),
                                    subcommands: vec![],
                                    body: None,
                                    http: Some(CommandHttp::Endpoint(endpoint)),
                                }],
                            },
                            schema: SchemaGraph::empty(),
                        };
                        assert_eq!(Tool::from_value(&tool.to_value()).unwrap(), tool);
                        assert_eq!(
                            serde_json::from_str::<Tool>(&serde_json::to_string(&tool).unwrap())
                                .unwrap(),
                            tool
                        );
                        #[cfg(feature = "full")]
                        {
                            use prost::Message;
                            let proto: crate::proto::golem::tool::Tool = tool.clone().into();
                            let decoded = crate::proto::golem::tool::Tool::decode(
                                proto.encode_to_vec().as_slice(),
                            )
                            .unwrap();
                            assert_eq!(Tool::try_from(decoded).unwrap(), tool);
                        }
                        #[cfg(all(
                            any(feature = "host", feature = "guest"),
                            not(all(feature = "host", feature = "guest"))
                        ))]
                        {
                            let wire = crate::schema::tool::wit::encode_tool(&tool).unwrap();
                            assert_eq!(crate::schema::tool::wit::decode_tool(wire).unwrap(), tool);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn http_declarations_roundtrip() {
        for mode in [
            None,
            Some(HttpRouteMode::Plain),
            Some(HttpRouteMode::DurableStreams),
            Some(HttpRouteMode::Sse),
            Some(HttpRouteMode::WebSocket),
        ] {
            let endpoint = ToolHttpEndpoint {
                route_id: Some("download".into()),
                http_method: HttpMethod::Post(Empty {}),
                path_suffix: vec![PathSegment::PathVariable(PathVariable {
                    variable_name: "id".into(),
                })],
                query_vars: vec![QueryVariable {
                    query_param_name: "q".into(),
                    variable_name: "query".into(),
                }],
                header_vars: vec![HeaderVariable {
                    header_name: "X-Mode".into(),
                    variable_name: "mode".into(),
                }],
                response_headers: vec![FileResponseHeader {
                    name: "X-Custom".into(),
                    value: "yes".into(),
                }],
                mode,
                durable_streams: Some(DurableStreamRouteOptions {
                    slots: vec![
                        DurableStreamSlotOptions {
                            source: DurableStreamSlotSource::Input(DurableStreamInputSlotSource {
                                name: "stdin".into(),
                            }),
                            name: Some("upload".into()),
                            content_type: Some("text/plain".into()),
                        },
                        DurableStreamSlotOptions {
                            source: DurableStreamSlotSource::Output(
                                DurableStreamOutputSlotSource {
                                    name: "stdout".into(),
                                },
                            ),
                            name: Some("download".into()),
                            content_type: None,
                        },
                    ],
                    load: Some(DurableStreamRouteLoadOptions {
                        max_concurrent_readers_per_stream: Some(7),
                        max_append_requests_per_second_per_stream: Some(13),
                    }),
                }),
            };
            let mut tool = Tool {
                version: "1".into(),
                requires_filesystem: false,
                http_mount: Some(ToolHttpMount {
                    path_prefix: vec![PathSegment::Literal(LiteralSegment {
                        value: "reports".into(),
                    })],
                }),
                commands: CommandTree {
                    nodes: vec![CommandNode {
                        name: "report".into(),
                        aliases: vec![],
                        doc: Doc::default(),
                        globals: Globals::default(),
                        subcommands: vec![],
                        body: None,
                        http: Some(CommandHttp::Endpoint(endpoint)),
                    }],
                },
                schema: SchemaGraph::empty(),
            };
            for http in [
                tool.commands.nodes[0].http.clone(),
                Some(CommandHttp::Prefix(ToolHttpPrefix {
                    path_suffix: vec![PathSegment::Literal(LiteralSegment {
                        value: "admin".into(),
                    })],
                })),
                None,
            ] {
                tool.commands.nodes[0].http = http;
                let json = serde_json::to_string(&tool).unwrap();
                assert_eq!(serde_json::from_str::<Tool>(&json).unwrap(), tool);
                #[cfg(feature = "full")]
                {
                    let proto: crate::proto::golem::tool::Tool = tool.clone().into();
                    assert_eq!(Tool::try_from(proto).unwrap(), tool);
                }
                #[cfg(all(
                    any(feature = "host", feature = "guest"),
                    not(all(feature = "host", feature = "guest"))
                ))]
                {
                    let wire = crate::schema::tool::wit::encode_tool(&tool).unwrap();
                    assert_eq!(crate::schema::tool::wit::decode_tool(wire).unwrap(), tool);
                }
            }
        }
    }
}
