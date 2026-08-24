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

use super::tool_metadata_wire::encode_tool;
use super::{
    InputStream, Principal, Tool, ToolMiddleware, ToolMiddlewareInvokeFuture, ToolMiddlewareScope,
    UnderlyingTool,
};
use crate::TypedSchemaValue;
use crate::schema::tool::validation::validate_tool;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[doc(hidden)]
pub type ToolMiddlewareInvoker = fn(
    String,
    Tool,
    Vec<String>,
    TypedSchemaValue,
    Option<InputStream>,
    Principal,
    UnderlyingTool,
) -> ToolMiddlewareInvokeFuture;

#[derive(Default)]
struct ToolMiddlewares {
    descriptors: BTreeMap<String, ToolMiddleware>,
    invokers: BTreeMap<String, ToolMiddlewareInvoker>,
}

#[derive(Default)]
struct State {
    middlewares: RefCell<ToolMiddlewares>,
}

static mut STATE: Option<State> = None;

#[allow(static_mut_refs)]
fn get_state() -> &'static State {
    unsafe {
        if STATE.is_none() {
            STATE = Some(State::default());
        }
        STATE.as_ref().unwrap()
    }
}

#[doc(hidden)]
pub fn register_tool_middleware(middleware: ToolMiddleware, invoker: ToolMiddlewareInvoker) {
    validate_descriptor(&middleware);
    let name = middleware.name.clone();
    let mut middlewares = get_state().middlewares.borrow_mut();
    if middlewares.descriptors.contains_key(&name) {
        panic!("duplicate tool middleware registration for middleware name: {name}");
    }
    middlewares.invokers.insert(name.clone(), invoker);
    middlewares.descriptors.insert(name, middleware);
}

fn validate_descriptor(middleware: &ToolMiddleware) {
    if let ToolMiddlewareScope::Monomorphic(scope) = &middleware.scope {
        validate_tool(&scope.presented)
            .expect("tool middleware presented descriptor validation failed");
        encode_tool(&scope.presented).expect("tool middleware presented descriptor build failed");
        let expected = scope
            .expected
            .as_ref()
            .expect("monomorphic tool middleware requires an expected descriptor");
        validate_tool(expected).expect("tool middleware expected descriptor validation failed");
        encode_tool(expected).expect("tool middleware expected descriptor build failed");
    }
}

#[doc(hidden)]
pub fn get_all_tool_middlewares() -> Vec<ToolMiddleware> {
    get_state()
        .middlewares
        .borrow()
        .descriptors
        .values()
        .cloned()
        .collect()
}

#[doc(hidden)]
pub fn get_tool_middleware_by_name(name: &str) -> Option<ToolMiddleware> {
    get_state()
        .middlewares
        .borrow()
        .descriptors
        .get(name)
        .cloned()
}

#[doc(hidden)]
pub fn get_tool_middleware_invoker_by_name(name: &str) -> Option<ToolMiddlewareInvoker> {
    get_state().middlewares.borrow().invokers.get(name).copied()
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn clear_tool_middlewares_for_tests() {
    let mut middlewares = get_state().middlewares.borrow_mut();
    middlewares.descriptors.clear();
    middlewares.invokers.clear();
}

#[cfg(test)]
#[test_r::sequential]
mod tests {
    use super::*;
    use crate::schema::SchemaGraph;
    use crate::schema::tool::{CommandNode, CommandTree, Doc, Globals};
    use crate::tool::InvocationResult;
    use test_r::test;

    fn universal(name: &str) -> ToolMiddleware {
        ToolMiddleware {
            name: name.to_string(),
            aliases: vec![],
            doc: Doc {
                summary: String::new(),
                description: String::new(),
                examples: vec![],
            },
            scope: ToolMiddlewareScope::Universal,
        }
    }

    fn tool(name: &str) -> Tool {
        Tool {
            version: "0.1.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: vec![],
                    doc: Doc {
                        summary: String::new(),
                        description: String::new(),
                        examples: vec![],
                    },
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        }
    }

    fn invoker(
        _tool_name: String,
        _tool: Tool,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<InputStream>,
        _principal: Principal,
        _underlying: UnderlyingTool,
    ) -> ToolMiddlewareInvokeFuture {
        Box::pin(async {
            Ok(InvocationResult {
                result: None,
                stdout: None,
            })
        })
    }

    #[cfg(all(feature = "export_golem_agentic", feature = "macro"))]
    mod authored_dispatch {
        use super::*;
        use crate::schema::{FromSchema, IntoTypedSchemaValue, SchemaValue};
        use crate::tool::{ToolInvokeError, ToolUnderlying};
        use std::cell::Cell;
        use std::convert::Infallible;

        thread_local! {
            static CONSTRUCTIONS: Cell<u32> = const { Cell::new(0) };
            static UNDERLYING_CALLS: Cell<u32> = const { Cell::new(0) };
        }

        #[golem_rust_macro::tool_definition]
        #[allow(dead_code)]
        trait RegistryDispatchEcho {
            fn echo(&self, value: String) -> String;
        }

        struct RegistryPolicy;

        impl RegistryPolicy {
            fn new() -> Self {
                CONSTRUCTIONS.with(|count| count.set(count.get() + 1));
                Self
            }
        }

        #[golem_rust_macro::tool_middleware(
            name = "phase-five-authored-dispatch",
            constructor = RegistryPolicy::new
        )]
        impl RegistryDispatchEchoMiddleware for RegistryPolicy {
            async fn echo(
                &self,
                underlying: &mut RegistryDispatchEchoUnderlying,
                value: String,
            ) -> Result<String, ToolInvokeError<Infallible>> {
                if value == "short" {
                    return Ok("short-circuited".to_string());
                }
                let first = underlying.echo(value.clone()).await?;
                if value == "twice" {
                    let second = underlying.echo(value).await?;
                    Ok(format!("{first}+{second}"))
                } else {
                    Ok(first)
                }
            }
        }

        fn input(value: &str) -> TypedSchemaValue {
            let tool =
                <RegistryDispatchEchoUnderlying as ToolUnderlying>::__golem_tool_descriptor();
            let command_index = tool
                .command_index_by_path(&["echo".to_string()])
                .expect("echo command exists");
            let model = tool
                .canonical_input_model(command_index)
                .expect("canonical input model builds");
            TypedSchemaValue::new(
                model.record_schema,
                SchemaValue::Record {
                    fields: vec![SchemaValue::String(value.to_string())],
                },
            )
        }

        fn underlying() -> UnderlyingTool {
            UnderlyingTool::from_fake(Box::new(|_, _, _| {
                UNDERLYING_CALLS.with(|count| count.set(count.get() + 1));
                Box::pin(async {
                    let value = "inner".to_string().into_typed_schema_value().unwrap();
                    Ok(crate::tool::wire::InvocationResult {
                        result: Some(crate::encode_typed_schema_value_owned(value).unwrap()),
                        stdout: None,
                    })
                })
            }))
        }

        async fn invoke(
            value: TypedSchemaValue,
        ) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
            let invoker = get_tool_middleware_invoker_by_name("phase-five-authored-dispatch")
                .expect("authored middleware ctor registered its invoker");
            invoker(
                "registry-dispatch-echo".to_string(),
                <RegistryDispatchEchoUnderlying as ToolUnderlying>::__golem_tool_descriptor(),
                vec!["echo".to_string()],
                value,
                None,
                Principal::Anonymous,
                underlying(),
            )
            .await
        }

        #[test_r::test]
        async fn authored_invoker_constructs_once_and_controls_underlying_calls() {
            CONSTRUCTIONS.with(|count| count.set(0));
            UNDERLYING_CALLS.with(|count| count.set(0));

            assert!(get_tool_middleware_invoker_by_name("unknown-middleware").is_none());
            assert_eq!(CONSTRUCTIONS.with(Cell::get), 0);

            let short = invoke(input("short")).await.unwrap();
            assert_eq!(
                String::from_value(short.result.unwrap().value()).unwrap(),
                "short-circuited"
            );
            assert_eq!(CONSTRUCTIONS.with(Cell::get), 1);
            assert_eq!(UNDERLYING_CALLS.with(Cell::get), 0);

            let twice = invoke(input("twice")).await.unwrap();
            assert_eq!(
                String::from_value(twice.result.unwrap().value()).unwrap(),
                "inner+inner"
            );
            assert_eq!(CONSTRUCTIONS.with(Cell::get), 2);
            assert_eq!(UNDERLYING_CALLS.with(Cell::get), 2);

            let malformed = "not a record"
                .to_string()
                .into_typed_schema_value()
                .unwrap();
            assert!(matches!(
                invoke(malformed).await,
                Err(ToolInvokeError::InvalidInput(_))
            ));
            assert_eq!(CONSTRUCTIONS.with(Cell::get), 3);
            assert_eq!(UNDERLYING_CALLS.with(Cell::get), 2);
        }

        #[test_r::test]
        fn generated_tool_metadata_round_trips_through_the_guest_wire_model() {
            let tool =
                <RegistryDispatchEchoUnderlying as ToolUnderlying>::__golem_tool_descriptor();
            let encoded = super::super::super::tool_metadata_wire::encode_tool(&tool).unwrap();
            let decoded = super::super::super::tool_metadata_wire::decode_tool(encoded).unwrap();

            assert_eq!(decoded, tool);
        }
    }

    #[test]
    fn discovery_is_sorted_and_invokers_are_looked_up_separately() {
        register_tool_middleware(universal("registry-sort-second"), invoker);
        register_tool_middleware(universal("registry-sort-first"), invoker);

        assert_eq!(
            get_all_tool_middlewares()
                .iter()
                .filter(|middleware| middleware.name.starts_with("registry-sort-"))
                .map(|middleware| middleware.name.as_str())
                .collect::<Vec<_>>(),
            vec!["registry-sort-first", "registry-sort-second"]
        );
        assert_eq!(
            get_tool_middleware_by_name("registry-sort-first")
                .unwrap()
                .name,
            "registry-sort-first"
        );
        assert!(get_tool_middleware_invoker_by_name("registry-sort-first").is_some());
        assert!(get_tool_middleware_invoker_by_name("missing").is_none());
    }

    #[test]
    #[should_panic(
        expected = "duplicate tool middleware registration for middleware name: registry-duplicate"
    )]
    fn duplicate_registration_panics() {
        register_tool_middleware(universal("registry-duplicate"), invoker);
        register_tool_middleware(universal("registry-duplicate"), invoker);
    }

    #[test]
    #[should_panic(expected = "tool middleware presented descriptor validation failed")]
    fn monomorphic_registration_rejects_an_empty_presented_command_tree() {
        let mut presented = tool("registry-invalid-presented");
        presented.commands.nodes.clear();

        register_tool_middleware(
            ToolMiddleware {
                name: "registry-invalid-presented".to_string(),
                aliases: vec![],
                doc: Doc {
                    summary: String::new(),
                    description: String::new(),
                    examples: vec![],
                },
                scope: ToolMiddlewareScope::Monomorphic(
                    super::super::MonomorphicToolMiddlewareScope {
                        presented,
                        expected: None,
                    },
                ),
            },
            invoker,
        );
    }

    #[test]
    #[should_panic(expected = "monomorphic tool middleware requires an expected descriptor")]
    fn monomorphic_registration_rejects_a_missing_expected_descriptor() {
        register_tool_middleware(
            ToolMiddleware {
                name: "registry-missing-expected".to_string(),
                aliases: vec![],
                doc: Doc {
                    summary: String::new(),
                    description: String::new(),
                    examples: vec![],
                },
                scope: ToolMiddlewareScope::Monomorphic(
                    super::super::MonomorphicToolMiddlewareScope {
                        presented: tool("registry-missing-expected"),
                        expected: None,
                    },
                ),
            },
            invoker,
        );
    }

    #[test]
    fn middleware_namespace_is_independent_from_tool_names() {
        register_tool_middleware(
            ToolMiddleware {
                name: "registry-shared-name".to_string(),
                aliases: vec![],
                doc: Doc {
                    summary: String::new(),
                    description: String::new(),
                    examples: vec![],
                },
                scope: ToolMiddlewareScope::Monomorphic(
                    super::super::MonomorphicToolMiddlewareScope {
                        presented: tool("registry-shared-name"),
                        expected: Some(tool("registry-shared-name")),
                    },
                ),
            },
            invoker,
        );

        assert!(get_tool_middleware_by_name("registry-shared-name").is_some());
    }

    #[cfg(any(
        feature = "export_golem_tool_middleware",
        feature = "export_golem_agentic_tool_middleware"
    ))]
    #[test]
    fn guest_discovery_and_lookup_encode_complete_scope_metadata() {
        let presented = tool("registry-presented");
        let expected = tool("registry-expected");
        register_tool_middleware(
            ToolMiddleware {
                name: "registry-boundary-monomorphic".to_string(),
                aliases: vec!["registry-boundary-alias".to_string()],
                doc: Doc {
                    summary: "Boundary summary".to_string(),
                    description: "Boundary description".to_string(),
                    examples: vec![],
                },
                scope: ToolMiddlewareScope::Monomorphic(
                    super::super::MonomorphicToolMiddlewareScope {
                        presented: presented.clone(),
                        expected: Some(expected.clone()),
                    },
                ),
            },
            invoker,
        );
        register_tool_middleware(
            ToolMiddleware {
                name: "registry-boundary-universal".to_string(),
                aliases: vec![],
                doc: Doc {
                    summary: "Universal summary".to_string(),
                    description: String::new(),
                    examples: vec![],
                },
                scope: ToolMiddlewareScope::Universal,
            },
            invoker,
        );

        let discovered = super::super::tool_middleware_impl::discover_tool_middlewares().unwrap();
        let names = discovered
            .iter()
            .filter(|middleware| middleware.name.starts_with("registry-boundary-"))
            .map(|middleware| middleware.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "registry-boundary-monomorphic",
                "registry-boundary-universal"
            ]
        );

        let encoded = super::super::tool_middleware_impl::get_tool_middleware(
            "registry-boundary-monomorphic".to_string(),
        )
        .unwrap();
        assert_eq!(encoded.aliases, vec!["registry-boundary-alias"]);
        assert_eq!(encoded.doc.summary, "Boundary summary");
        match encoded.scope {
            crate::tool::wire::ToolMiddlewareScope::Monomorphic(scope) => {
                assert_eq!(
                    super::super::tool_metadata_wire::decode_tool(scope.presented).unwrap(),
                    presented
                );
                assert_eq!(
                    super::super::tool_metadata_wire::decode_tool(scope.expected.unwrap()).unwrap(),
                    expected
                );
            }
            crate::tool::wire::ToolMiddlewareScope::Universal => {
                panic!("monomorphic middleware encoded as universal")
            }
        }

        let universal = super::super::tool_middleware_impl::get_tool_middleware(
            "registry-boundary-universal".to_string(),
        )
        .unwrap();
        assert!(matches!(
            universal.scope,
            crate::tool::wire::ToolMiddlewareScope::Universal
        ));

        assert!(matches!(
            super::super::tool_middleware_impl::get_tool_middleware(
                "registry-boundary-missing".to_string()
            ),
            Err(crate::tool::wire::ToolError::InvalidToolName(name))
                if name == "registry-boundary-missing"
        ));
    }
}
