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
        &[quote! { cfg: Config }],
        &[format_ident!("cfg")],
        &[],
        agent_is_durable,
    )
    .to_string()
}

#[test]
fn durable_agents_generate_getters() {
    let rendered = render_client(true);

    assert!(rendered.contains("pub fn get ("));
    assert!(rendered.contains("get_with_config"));
}

#[test]
fn ephemeral_agents_skip_non_phantom_getters() {
    let rendered = render_client(false);

    assert!(!rendered.contains("pub fn get ("));
    assert!(!rendered.contains("get_with_config"));
    assert!(rendered.contains("new_phantom"));
    assert!(!rendered.contains("get_phantom"));
    assert!(!rendered.contains("get_phantom_with_config"));
    assert!(!rendered.contains("Uuid :: new_v4"));
    assert!(!rendered.contains("make_agent_id"));
    assert!(rendered.contains("async_invoke_and_await"));
    assert!(rendered.contains("wasm_rpc . invoke"));
    assert!(rendered.contains("schedule_invocation"));
    assert!(!rendered.contains("_with_metadata"));
    assert!(!rendered.contains("fn phantom_id ("));
    assert!(!rendered.contains("fn get_agent_id ("));
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

        assert_eq!(rendered.matches("encode_schema_value_async").count(), 1);
        assert_eq!(
            rendered
                .matches(
                    "live streams cannot cross remote or scheduled agent invocation boundaries"
                )
                .count(),
            3
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

    assert_eq!(rendered.matches("EphemeralInvocationResult").count(), 4);
    assert!(!rendered.contains("struct ExampleAgentInvocationResult"));
    assert!(!rendered.contains("struct OtherAgentInvocationResult"));
    assert!(!rendered.contains("struct Invocation <"));
    assert!(!rendered.contains("struct CancelableInvocationReceipt"));
}

#[test]
fn cancelable_schedule_wrapper_does_not_duplicate_user_method_name() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn run(&self);
            fn schedule_cancelable_run(&self);
        }
    };

    let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true).to_string();

    assert_eq!(
        rendered.matches("fn schedule_cancelable_run (").count(),
        1,
        "generated client must not emit duplicate schedule_cancelable_run methods:\n{rendered}"
    );
    assert!(
        rendered.contains("fn schedule_cancelable_run1 ("),
        "generated client should deconflict the generated wrapper name:\n{rendered}"
    );
}

#[test]
fn constructor_helper_does_not_duplicate_user_method_name() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn new_phantom(&self);
        }
    };

    let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true).to_string();

    assert!(
        rendered.contains("pub async fn new_phantom ("),
        "real user method names should win over generated constructor helpers:\n{rendered}"
    );
    assert_eq!(
        rendered.matches("fn new_phantom (").count(),
        1,
        "generated client must not emit a constructor helper with the same inherent method name as a user method:\n{rendered}"
    );
}

#[test]
fn durable_get_constructor_collision_keeps_existing_get_compat_name() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(init: String) -> Self;
            fn get(&self) -> String;
        }
    };

    let rendered = get_remote_client(
        &item_trait,
        &[quote! { init: String }],
        &[format_ident!("init")],
        &[],
        &[],
        &[],
        true,
    )
    .to_string();

    assert!(
        rendered.contains("pub fn get_ ("),
        "existing Rust SDK tests and components use get_ as the durable constructor helper when get is a user method; generated client was:\n{rendered}"
    );
}

#[test]
fn accessor_helpers_do_not_duplicate_user_method_names() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new() -> Self;
            fn get_agent_id(&self) -> String;
            fn phantom_id(&self) -> String;
        }
    };

    let rendered = get_remote_client(&item_trait, &[], &[], &[], &[], &[], true).to_string();

    assert!(
        rendered.contains("pub async fn get_agent_id ("),
        "real user method names should win over generated accessors:\n{rendered}"
    );
    assert!(
        rendered.contains("pub async fn phantom_id ("),
        "real user method names should win over generated accessors:\n{rendered}"
    );
    assert_eq!(
        rendered.matches("fn get_agent_id (").count(),
        1,
        "generated client must not emit a get_agent_id accessor with the same inherent method name as a user method:\n{rendered}"
    );
    assert_eq!(
        rendered.matches("fn phantom_id (").count(),
        1,
        "generated client must not emit a phantom_id accessor with the same inherent method name as a user method:\n{rendered}"
    );
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

    for wrapper_name in ["schedule_run", "schedule_cancelable_run"] {
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
}

#[test]
fn get_phantom_deconflicts_generated_phantom_id_parameter() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(phantom_id: String) -> Self;
            fn run(&self);
        }
    };

    let tokens = get_remote_client(
        &item_trait,
        &[quote! { phantom_id: String }],
        &[format_ident!("phantom_id")],
        &[],
        &[],
        &[],
        true,
    );
    let rendered = tokens.to_string();
    let generated = syn::parse2::<syn::File>(tokens).unwrap();
    let params = generated
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item_impl) => item_impl.items.iter().find_map(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return None;
                };
                (method.sig.ident == "get_phantom").then(|| {
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
        .unwrap_or_else(|| panic!("missing generated get_phantom helper:\n{rendered}"));
    let unique_params = params.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        unique_params.len(),
        params.len(),
        "get_phantom should deconflict its generated phantom_id parameter from constructor parameters; params were {params:?}:\n{rendered}"
    );
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
                (method.sig.ident == "get").then(|| {
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
        .unwrap_or_else(|| panic!("missing generated get helper:\n{rendered}"));

    assert!(
        !bindings_before_constructor_value
            .iter()
            .any(|name| name == "agent_type"),
        "generated constructor helper must not bind a local named agent_type before encoding the constructor parameter with the same name; early bindings were {bindings_before_constructor_value:?}:\n{rendered}"
    );
}

#[test]
fn config_encoding_does_not_shadow_config_parameters() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(#[agent_config] result: Config) -> Self;
            fn run(&self);
        }
    };

    let rendered = get_remote_client(
        &item_trait,
        &[],
        &[],
        &[quote! { result: Config }],
        &[format_ident!("result")],
        &[],
        true,
    )
    .to_string();

    assert!(
        !rendered.contains("let mut result = Vec :: new () ; result . append (& mut :: golem_rust :: agentic :: IntoRpcConfigParam :: into_rpc_param (result ,"),
        "generated config encoding must not shadow a config parameter named result before passing it to IntoRpcConfigParam; generated client was:\n{rendered}"
    );
}

#[test]
fn config_encoding_deconflicts_internal_temp_name_from_config_parameters() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(#[agent_config] __golem_rpc_config_params: Config) -> Self;
            fn run(&self);
        }
    };

    let rendered = get_remote_client(
        &item_trait,
        &[],
        &[],
        &[quote! { __golem_rpc_config_params: Config }],
        &[format_ident!("__golem_rpc_config_params")],
        &[],
        true,
    )
    .to_string();

    assert!(
        !rendered.contains("let mut __golem_rpc_config_params = Vec :: new () ; __golem_rpc_config_params . append (& mut :: golem_rust :: agentic :: IntoRpcConfigParam :: into_rpc_param (__golem_rpc_config_params ,"),
        "generated config encoding must not shadow a config parameter whose legal Rust identifier matches the internal temporary name; generated client was:\n{rendered}"
    );
}

#[test]
fn config_encoding_does_not_shadow_config_parameter_named_agent_id() {
    let item_trait = parse_quote! {
        trait ExampleAgent {
            fn new(#[agent_config] agent_id: Config) -> Self;
            fn run(&self);
        }
    };

    let tokens = get_remote_client(
        &item_trait,
        &[],
        &[],
        &[quote! { agent_id: Config }],
        &[format_ident!("agent_id")],
        &[],
        true,
    );
    let rendered = tokens.to_string();
    let generated = syn::parse2::<syn::File>(tokens).unwrap();
    let shadows_before_config_encoding = generated
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item_impl) => item_impl.items.iter().find_map(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return None;
                };
                (method.sig.ident == "get_with_config").then(|| {
                    let mut agent_id_is_shadowed = false;
                    for stmt in &method.block.stmts {
                        if stmt
                            .to_token_stream()
                            .to_string()
                            .contains("IntoRpcConfigParam :: into_rpc_param (agent_id ,")
                        {
                            return agent_id_is_shadowed;
                        }
                        if let syn::Stmt::Local(local) = stmt
                            && let syn::Pat::Ident(pat_ident) = &local.pat
                            && pat_ident.ident == "agent_id"
                        {
                            agent_id_is_shadowed = true;
                        }
                    }
                    false
                })
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing generated get_with_config helper:\n{rendered}"));

    assert!(
        !shadows_before_config_encoding,
        "generated config encoding must not bind a local named agent_id before passing the config parameter with the same name to IntoRpcConfigParam; generated client was:\n{rendered}"
    );
}

/// The wire `schema-value-tree` now carries an owned, affine `quota-token`
/// handle, so the generated client must never store it. It stores the
/// eagerly-computed `agent_id` string instead, rejects quota tokens in the
/// constructor value before encoding, and computes the agent id via
/// `make_agent_id` during construction.
#[test]
fn client_does_not_store_affine_constructor_tree() {
    let rendered = render_client(true);

    // No affine wire tree (or its old companion field) is retained.
    assert!(!rendered.contains("constructor_data"));
    assert!(!rendered.contains("agent_type_name"));
    // Agent id is computed eagerly during construction and stored.
    assert!(rendered.contains("make_agent_id"));
    assert!(rendered.contains("agent_id"));
    // Quota tokens are rejected in constructor parameters before any encode.
    assert!(rendered.contains("__reject_quota_tokens_in_agent_constructor"));
}
