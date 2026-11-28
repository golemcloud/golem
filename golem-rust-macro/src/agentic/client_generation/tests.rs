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

use super::get_remote_client;
use quote::{ToTokens, format_ident, quote};
use syn::parse_quote;
use test_r::test;

/// The rendered client struct and its inherent impl, without the scope-card view.
fn client_part(rendered: &str) -> &str {
    rendered
        .split("pub struct ExampleAgentClientWithScopeCard")
        .next()
        .unwrap()
}

fn render_client(agent_is_durable: bool) -> String {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(name: String, enabled: bool, cfg: Config) -> Self;
            fn ping(&self);
        }
    };

    get_remote_client(
        &item_trait,
        &[quote! { name: String }, quote! { enabled: bool }],
        &[format_ident!("name"), format_ident!("enabled")],
        &[quote! { Config }],
        &[format_ident!("cfg")],
        &[],
        agent_is_durable,
    )
    .to_string()
}

/// The parameter names of the generated inherent function `name`.
fn generated_params(tokens: proc_macro2::TokenStream, name: &str) -> Vec<String> {
    let rendered = tokens.to_string();
    let generated = syn::parse2::<syn::File>(tokens).unwrap();
    generated
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item_impl) => item_impl.items.iter().find_map(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return None;
                };
                (method.sig.ident == name).then(|| {
                    method
                        .sig
                        .inputs
                        .iter()
                        .filter_map(|arg| match arg {
                            syn::FnArg::Receiver(_) => None,
                            syn::FnArg::Typed(pat_type) => match pat_type.pat.as_ref() {
                                syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.to_string()),
                                _ => None,
                            },
                        })
                        .collect::<Vec<_>>()
                })
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing generated {name}:\n{rendered}"))
}

#[test]
fn durable_agents_generate_upsert_create_get_by_id_and_agent_id() {
    let rendered = render_client(true);

    assert!(rendered.contains("pub fn upsert (name : String , enabled : bool ,) -> Self"));
    assert!(
        rendered
            .contains("agentic :: __upsert_agent_or_panic (\"ExampleAgent\" , constructor_value)")
    );
    assert!(rendered.contains(
        "pub async fn create (name : String , enabled : bool , options : golem_rust :: CreateOptions < Config > ,) -> Result < Self , golem_rust :: GolemReflectError >"
    ));
    assert!(rendered.contains("keyed : options . keyed"));
    assert!(rendered.contains("env : options . env"));
    assert!(rendered.contains(
        "config : :: golem_rust :: agentic :: IntoRpcConfigParam :: into_rpc_param (options . config , & [])"
    ));
    assert!(rendered.contains(
        "agentic :: __create_agent (\"ExampleAgent\" , constructor_value , options) . await ?"
    ));
    assert!(rendered.contains(
        "pub fn get_by_id (agent_id : & golem_rust :: AgentId) -> Result < Self , golem_rust :: GolemReflectError >"
    ));
    assert!(rendered.contains("agentic :: __get_agent_by_id (\"ExampleAgent\" , agent_id) ?"));
    assert!(rendered.contains(
        "pub async fn agent_id (& self) -> Result < golem_rust :: AgentId , golem_rust :: GolemReflectError >"
    ));
    assert!(rendered.contains("agentic :: __agent_id (& self . wasm_rpc) . await"));
    assert!(rendered.contains("agentic :: __bind_self (\"ExampleAgent\") ?"));
    for removed in [
        "phantom",
        "get_with_config",
        "fn get (",
        "make_agent_id",
        "ParsedAgentId",
        "Uuid :: new_v4",
        "component_id",
        "get_agent_type",
    ] {
        assert!(!rendered.contains(removed), "{removed}:\n{rendered}");
    }
}

#[test]
fn ephemeral_agents_generate_only_upsert() {
    let rendered = render_client(false);

    assert!(rendered.contains("pub fn upsert (name : String , enabled : bool ,) -> Self"));
    assert!(rendered.contains("agentic :: __upsert_agent_or_panic"));
    for absent in [
        "fn create (",
        "fn get_by_id (",
        "fn agent_id (",
        "fn bind_self (",
        "__create_agent",
        "phantom",
        "fn get (",
    ] {
        assert!(!rendered.contains(absent), "{absent}:\n{rendered}");
    }
    assert!(rendered.contains("permissions :: __invoke_and_await"));
    assert!(rendered.contains("wasm_rpc . trigger ("));
    assert!(rendered.contains("wasm_rpc . schedule ("));
    for removed in [
        "async_invoke_and_await",
        "schedule_invocation",
        "schedule_cancelable",
        "pending_",
        "CancellationToken",
        "_with_metadata",
    ] {
        assert!(!rendered.contains(removed), "{removed}:\n{rendered}");
    }
}

#[test]
fn caller_defined_clients_return_typed_errors_from_every_constructor() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(name: String) -> Self;
            fn ping(&self);
        }
    };
    let rendered = super::get_remote_client_for_type(
        &item_trait,
        "remote:example/agent",
        &[quote! { name: String }],
        &[format_ident!("name")],
        &[],
        &[],
        &[],
        true,
        true,
    )
    .to_string();

    assert!(!rendered.contains("get_agent_type"));
    assert!(!rendered.contains("component_id"));
    assert!(rendered.contains(
        "pub fn upsert (name : String ,) -> Result < Self , golem_rust :: GolemReflectError >"
    ));
    assert!(rendered.contains(
        "agentic :: __upsert_agent (\"remote:example/agent\" , golem_rust :: encode_schema_value"
    ));
    assert!(rendered.contains("options : golem_rust :: CreateOptions < () >"));
    assert!(rendered.contains("config : :: std :: vec :: Vec :: new ()"));
    assert!(rendered.contains(
        "agentic :: __create_agent (\"remote:example/agent\" , golem_rust :: encode_schema_value"
    ));
    assert!(
        rendered.contains("agentic :: __get_agent_by_id (\"remote:example/agent\" , agent_id) ?")
    );
    assert!(!rendered.contains("__upsert_agent_or_panic"));
    assert!(!rendered.contains("WasmRpc :: create"));
}

#[test]
fn create_takes_several_agent_configs_as_a_tuple() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(name: String, #[agent_config] a: Config<A>, #[agent_config] b: Config<B>) -> Self;
            fn ping(&self);
        }
    };
    let rendered = get_remote_client(
        &item_trait,
        &[quote! { name: String }],
        &[format_ident!("name")],
        &[quote! { ARpc }, quote! { BRpc }],
        &[format_ident!("a"), format_ident!("b")],
        &[],
        true,
    )
    .to_string();

    assert!(rendered.contains("options : golem_rust :: CreateOptions < (ARpc , BRpc) >"));
    assert!(rendered.contains("let (__golem_config_0 , __golem_config_1) = options . config ;"));
    assert!(rendered.contains("into_rpc_param (__golem_config_0 , & [])"));
    assert!(rendered.contains("into_rpc_param (__golem_config_1 , & [])"));
}

#[test]
fn awaited_streaming_methods_use_async_value_encoding_only() {
    let item_trait = parse_quote! {
        trait StreamingAgent {
            fn new() -> Self;
            fn forward(
                &self,
                stream: AgentStream<String>,
            ) -> AgentStream<String>;
        }
    };

    for durable in [true, false] {
        let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], durable).to_string();

        assert_eq!(rendered.matches("prepare_parameter").count(), 1);
        assert!(!rendered.contains("encode_schema_value"));
        assert!(!rendered.contains("from_schema_value"));
        assert!(!rendered.contains("get_schema_graph"));
        assert!(!rendered.contains("Schema > :: contains_stream"));
        assert!(rendered.contains("parameter_contains_stream"));
        assert_eq!(
            rendered
                .matches(
                    "live streams cannot cross remote or scheduled agent invocation boundaries"
                )
                .count(),
            2
        );
    }
}

#[test]
fn ephemeral_clients_use_shared_non_colliding_result_type() {
    let first = render_client(false);
    let second_trait = parse_quote! {
        trait OtherAgent {
            fn new() -> Self;
            fn ping(&self) -> String;
        }
    };
    let second = get_remote_client(&second_trait, &[], &[], &[], &[], &[], false).to_string();
    let rendered = format!("{first} {second}");

    for output in [
        "-> golem_rust :: InvocationOutput < () >",
        "-> golem_rust :: InvocationOutput < String >",
    ] {
        assert!(rendered.contains(output), "{output}:\n{rendered}");
    }
    assert!(!rendered.contains("struct ExampleAgentInvocationResult"));
    assert!(!rendered.contains("struct OtherAgentInvocationResult"));
    assert!(!rendered.contains("struct Invocation <"));
    assert!(!rendered.contains("struct CancelableInvocationReceipt"));
}

#[test]
fn schedule_wrapper_does_not_duplicate_user_method_name() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn run(&self);
            fn schedule_run(&self);
        }
    };

    let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true).to_string();
    let rendered = client_part(&rendered);

    assert_eq!(
        rendered.matches("fn schedule_run (").count(),
        1,
        "generated client must not emit duplicate schedule_run methods:\n{rendered}"
    );
    assert!(
        rendered.contains("fn schedule_run1 ("),
        "generated client should deconflict the generated wrapper name:\n{rendered}"
    );
}

#[test]
fn constructor_helper_does_not_duplicate_user_method_name() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn upsert(&self);
            fn create(&self);
            fn get_by_id(&self);
            fn bind_self(&self);
            fn bind_self1(&self);
        }
    };

    let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true).to_string();
    let rendered = client_part(&rendered);

    for name in ["upsert", "create", "get_by_id", "bind_self"] {
        assert!(
            rendered.contains(&format!("pub async fn {name} (& self)")),
            "real user method names should win over generated constructor helpers:\n{rendered}"
        );
        assert_eq!(
            rendered.matches(&format!("fn {name} (")).count(),
            1,
            "generated client must not emit a constructor helper with the same inherent method name as a user method:\n{rendered}"
        );
    }
    assert!(rendered.contains("pub fn upsert1 () -> Self"));
    assert!(
        rendered.contains("pub async fn create1 (options : golem_rust :: CreateOptions < () > ,)")
    );
    assert!(rendered.contains("pub fn get_by_id1 (agent_id : & golem_rust :: AgentId)"));
    assert!(rendered.contains("pub async fn bind_self1 ("));
    assert!(
        rendered
            .contains("pub fn bind_self2 () -> Result < Self , golem_rust :: GolemReflectError >")
    );
}

#[test]
fn accessor_helpers_do_not_duplicate_user_method_names() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn agent_id(&self) -> String;
        }
    };

    let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true).to_string();
    let rendered = client_part(&rendered);

    assert!(
        rendered.contains("pub async fn agent_id (& self) -> String"),
        "real user method names should win over generated accessors:\n{rendered}"
    );
    assert_eq!(
        rendered.matches("fn agent_id (").count(),
        1,
        "generated client must not emit an agent_id accessor with the same inherent method name as a user method:\n{rendered}"
    );
    assert!(rendered.contains(
        "pub async fn agent_id1 (& self) -> Result < golem_rust :: AgentId , golem_rust :: GolemReflectError >"
    ));
}

#[test]
fn schedule_wrappers_deconflict_generated_scheduled_time_parameter() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn run(&self, scheduled_time: u64);
        }
    };

    let tokens = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true);
    let rendered = tokens.to_string();
    let generated = syn::parse2::<syn::File>(tokens).unwrap();

    let wrapper_name = "schedule_run";
    let params = generated
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item_impl) => item_impl.items.iter().find_map(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return None;
                };
                (method.sig.ident == wrapper_name).then(|| {
                    method
                        .sig
                        .inputs
                        .iter()
                        .filter_map(|arg| match arg {
                            syn::FnArg::Receiver(_) => None,
                            syn::FnArg::Typed(pat_type) => match pat_type.pat.as_ref() {
                                syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.to_string()),
                                _ => None,
                            },
                        })
                        .collect::<Vec<_>>()
                })
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing generated wrapper {wrapper_name}:\n{rendered}"));
    let unique_params = params.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        unique_params.len(),
        params.len(),
        "{wrapper_name} should deconflict its generated scheduled_time parameter from user parameters; params were {params:?}:\n{rendered}"
    );
}

#[test]
fn create_deconflicts_generated_options_parameter() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(options: String) -> Self;
            fn run(&self);
        }
    };

    let tokens = get_remote_client(
        &item_trait,
        &[quote! { options: String }],
        &[format_ident!("options")],
        &[],
        &[],
        &[],
        true,
    );
    let rendered = tokens.to_string();
    let params = generated_params(tokens, "create");
    assert_eq!(
        params,
        vec!["options".to_string(), "options1".to_string()],
        "create should deconflict its generated options parameter from constructor parameters:\n{rendered}"
    );
    assert!(rendered.contains("keyed : options1 . keyed"));
}

#[test]
fn constructor_body_does_not_shadow_constructor_parameters_before_encoding() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(agent_type: String) -> Self;
            fn run(&self);
        }
    };

    let tokens = get_remote_client(
        &item_trait,
        &[quote! { agent_type: String }],
        &[format_ident!("agent_type")],
        &[],
        &[],
        &[],
        true,
    );
    let rendered = tokens.to_string();
    let generated = syn::parse2::<syn::File>(tokens).unwrap();
    let bindings_before_constructor_value = generated
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item_impl) => item_impl.items.iter().find_map(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return None;
                };
                (method.sig.ident == "upsert").then(|| {
                    let mut bindings = Vec::new();
                    for stmt in &method.block.stmts {
                        let syn::Stmt::Local(local) = stmt else {
                            continue;
                        };
                        let syn::Pat::Ident(pat_ident) = &local.pat else {
                            continue;
                        };
                        if pat_ident.ident == "constructor_value" {
                            break;
                        }
                        bindings.push(pat_ident.ident.to_string());
                    }
                    bindings
                })
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing generated upsert helper:\n{rendered}"));

    assert!(
        !bindings_before_constructor_value
            .iter()
            .any(|name| name == "agent_type"),
        "generated constructor helper must not bind a local named agent_type before encoding the constructor parameter with the same name; early bindings were {bindings_before_constructor_value:?}:\n{rendered}"
    );
}

/// The wire `schema-value-tree` carries an owned, affine `quota-token` handle, so the
/// generated client must never store it. It rejects quota tokens in the constructor value
/// before encoding, and hands the tree to the host once.
#[test]
fn client_does_not_store_affine_constructor_tree() {
    let rendered = render_client(true);
    let client_struct = rendered.split("impl ExampleAgentClient").next().unwrap();

    assert!(!rendered.contains("constructor_data"));
    assert!(!rendered.contains("agent_type_name"));
    assert!(client_struct.contains(
        "pub struct ExampleAgentClient { wasm_rpc : golem_rust :: golem_agentic :: golem :: agent :: host :: WasmRpc , __golem_default_scope_card : golem_rust :: permissions :: __DefaultScopeCard , }"
    ));
    assert!(!rendered.contains("let constructor_value = ||"));
    assert!(rendered.contains("\"ExampleAgent\" , constructor_value)"));
    assert!(rendered.contains("reject_quota_tokens"));
}

#[test]
fn scope_cards_are_attached_only_to_awaited_calls() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn run(&self, value: u32) -> u32;
            fn with_scope_card(&self);
            fn set_default_scope_card(&self);
            fn clear_default_scope_card(&self);
        }
    };

    for (durable, fallible) in [(true, false), (false, false), (true, true), (false, true)] {
        let rendered = super::get_remote_client_for_type(
            &item_trait,
            "ExampleAgent",
            &[],
            &[],
            &[],
            &[],
            &[],
            durable,
            fallible,
        )
        .to_string();
        let (client, view) = rendered
            .split_once("pub struct ExampleAgentClientWithScopeCard")
            .unwrap();

        // Every awaited call goes through the scope-card helper; no other
        // invocation passes a scope card.
        assert!(!rendered.contains("wasm_rpc . invoke_and_await"));
        assert_eq!(
            client
                .matches("golem_rust :: permissions :: __invoke_and_await")
                .count(),
            4,
            "{rendered}"
        );
        assert!(!rendered.contains("invoke (\"run\" , input , Some"));
        assert!(!rendered.contains("invoke (scheduled_time , \"run\" , input , Some"));

        // Every awaited implementation resolves the scope card before it encodes
        // the input, so a dropped default fails before any argument card moves.
        let impls = client
            .split("async fn __golem_")
            .skip(1)
            .collect::<Vec<_>>();
        assert_eq!(impls.len(), 4, "{rendered}");
        for body in impls {
            let body = body.split("pub async fn").next().unwrap();
            let resolve = body
                .find("self . __golem_default_scope_card . resolve (__golem_scope_card)")
                .unwrap_or_else(|| panic!("missing scope-card resolution:\n{body}"));
            let encode = ["WirePreflight", "WireWriter", "encode_schema_value"]
                .iter()
                .filter_map(|marker| body.find(marker))
                .min()
                .unwrap_or_else(|| panic!("missing input encoding:\n{body}"));
            assert!(
                resolve < encode,
                "scope card resolved after encoding:\n{body}"
            );
        }

        // User methods win over the generated scope-card members, which are renamed.
        for name in [
            "with_scope_card",
            "set_default_scope_card",
            "clear_default_scope_card",
        ] {
            assert!(
                client.contains(&format!("pub async fn {name} (& self)")),
                "{rendered}"
            );
        }
        assert!(
            client.contains("pub fn with_scope_card1 < 'a >"),
            "{rendered}"
        );
        assert!(client.contains("pub fn set_default_scope_card1 (& mut self"));
        assert!(client.contains("pub fn clear_default_scope_card1 (& mut self)"));
        assert!(client.contains("scope_card : & golem_rust :: permissions :: ScopeCard"));
        assert!(!rendered.contains("permissions :: PermissionCard"));
        assert!(client.contains("pub async fn run (& self , value : u32)"));
        assert!(client.contains("self . __golem_run_with_scope_card (value , None) . await"));

        // The view exposes only the awaited methods and lends its card.
        assert!(view.contains(
            "self . client . __golem_run_with_scope_card (value , Some (self . scope_card)) . await"
        ));
        assert_eq!(view.matches("pub async fn").count(), 4, "{view}");
        assert!(!view.contains("trigger_"));
        assert!(!view.contains("schedule_"));
        assert!(!rendered.contains("pending_"), "{rendered}");

        // Trigger and schedule wait for the target to accept the invocation, and return its
        // handle typed with the method's result.
        let handle =
            "Result < golem_rust :: Invocation < u32 > , golem_rust :: GolemReflectError >";
        assert!(
            client.contains(&format!(
                "pub async fn trigger_run (& self , value : u32) -> {handle}"
            )),
            "{rendered}"
        );
        assert!(
            client.contains(&format!(
                "pub async fn schedule_run (& self , value : u32 , scheduled_time : golem_rust :: ScheduledTime ,) -> {handle}"
            )),
            "{rendered}"
        );
    }
}
