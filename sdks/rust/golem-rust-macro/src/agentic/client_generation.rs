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

use crate::agentic::{generic_type_in_agent_method_error, generic_type_in_agent_return_type_error};
use crate::rpc_client_common::{
    FunctionOutputInfo, collect_kept_args, collect_typed_params, decode_result_value,
    encode_value_only_carrier, find_generic_param_in_inputs, find_generic_param_in_return,
    is_principal_param, is_static_method, positional_record_schema_value,
};
use quote::{format_ident, quote};
use std::collections::{HashMap, HashSet};
use syn::ItemTrait;

pub fn get_remote_client(
    item_trait: &ItemTrait,
    constructor_data_value_param_defs: &[proc_macro2::TokenStream],
    constructor_data_value_param_idents: &[proc_macro2::Ident],
    constructor_agent_config_param_defs: &[proc_macro2::TokenStream],
    constructor_agent_config_param_idents: &[proc_macro2::Ident],
    agent_type_parameter_names: &[String],
    agent_is_durable: bool,
) -> proc_macro2::TokenStream {
    let remote_client_type_name = format_ident!("{}Client", item_trait.ident);

    let type_name = item_trait.ident.to_string();

    let RemoteAgentMethodsInfo {
        methods_impl,
        mut method_names,
    } = get_remote_agent_methods_info(item_trait, agent_type_parameter_names);

    let get_method_ident = agent_is_durable.then(|| {
        if method_names.contains("get") {
            method_names.fresh_ident("get_")
        } else {
            method_names.fresh_ident("get")
        }
    });
    let new_phantom_method_ident = method_names.fresh_ident("new_phantom");
    let get_phantom_method_ident = method_names.fresh_ident("get_phantom");
    let get_with_config_method_ident = (agent_is_durable
        && !constructor_agent_config_param_defs.is_empty())
    .then(|| method_names.fresh_ident("get_with_config"));
    let new_phantom_with_config_method_ident = (!constructor_agent_config_param_defs.is_empty())
        .then(|| method_names.fresh_ident("new_phantom_with_config"));
    let get_phantom_with_config_method_ident = (!constructor_agent_config_param_defs.is_empty())
        .then(|| method_names.fresh_ident("get_phantom_with_config"));
    let phantom_id_accessor_ident = method_names.fresh_ident("phantom_id");
    let get_agent_id_accessor_ident = method_names.fresh_ident("get_agent_id");
    let constructor_param_idents = constructor_data_value_param_idents
        .iter()
        .chain(constructor_agent_config_param_idents)
        .cloned()
        .collect::<Vec<_>>();
    let phantom_id_param_ident = fresh_param_ident(&constructor_param_idents, "phantom_id");
    let rpc_config_params_ident =
        fresh_param_ident(&constructor_param_idents, "__golem_rpc_config_params");
    let remote_agent_type_ident =
        fresh_param_ident(&constructor_param_idents, "__golem_agent_type");
    let phantom_uuid_ident = fresh_param_ident(&constructor_param_idents, "phantom_uuid");
    let constructor_value_ident = fresh_param_ident(&constructor_param_idents, "constructor_value");
    let agent_id_ident = fresh_param_ident(&constructor_param_idents, "agent_id");
    let encode_constructor = generate_constructor_data_value_params_encoding(
        constructor_data_value_param_idents,
        &constructor_value_ident,
    );

    let agent_config_params_as_rpc_param = {
        let add_rpc_params_entries = constructor_agent_config_param_idents.iter().map(|param_ident|
            quote! { #rpc_config_params_ident.append(&mut ::golem_rust::agentic::IntoRpcConfigParam::into_rpc_param(#param_ident, &[])); }
        );

        quote! {
            {
                let mut #rpc_config_params_ident = Vec::new();
                #(#add_rpc_params_entries)*
                #rpc_config_params_ident
            }
        }
    };

    // Builds the shared body of a remote-client constructor. Because the wire
    // `schema-value-tree` now carries an owned `quota-token` handle, it is an
    // affine, one-shot transfer envelope: it cannot be stored and reused.
    // The constructor value is therefore encoded freshly for each host call
    // (`make-agent-id` and the `wasm-rpc` constructor), the agent-id is computed
    // eagerly, and only the resulting `String` is kept in the client struct.
    let build_constructor_body =
        |prelude: proc_macro2::TokenStream,
         phantom_wire: proc_macro2::TokenStream,
         phantom_struct: proc_macro2::TokenStream,
         config: proc_macro2::TokenStream| {
            quote! {
                #encode_constructor

                #prelude

                let #remote_agent_type_ident =
                    golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name)
                        .expect("Internal Error: Agent type not registered");

                let #agent_id_ident = golem_rust::golem_agentic::golem::agent::host::make_agent_id(
                    #type_name,
                    golem_rust::encode_schema_value(&#constructor_value_ident)
                        .expect("Failed to encode constructor parameters for agent id"),
                    #phantom_wire,
                )
                .expect("Internal Error: Failed to make agent id");

                let wasm_rpc = golem_rust::golem_agentic::golem::agent::host::WasmRpc::new(
                    #type_name,
                    golem_rust::encode_schema_value(&#constructor_value_ident)
                        .expect("Failed to encode constructor parameters"),
                    #phantom_wire,
                    #config,
                );

                #remote_client_type_name {
                    agent_id: #agent_id_ident,
                    phantom_id: #phantom_struct,
                    component_id: #remote_agent_type_ident.implemented_by,
                    wasm_rpc,
                }
            }
        };

    let optional_get_with_config_impl = if agent_is_durable
        && !constructor_agent_config_param_defs.is_empty()
    {
        let get_with_config_method_ident = get_with_config_method_ident
            .as_ref()
            .expect("durable agents with config allocate get_with_config");
        let body = build_constructor_body(
            quote! {},
            quote! { None },
            quote! { None },
            agent_config_params_as_rpc_param.clone(),
        );
        quote! {
            pub fn #get_with_config_method_ident(#(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let optional_new_phantom_with_config_impl = if !constructor_agent_config_param_defs.is_empty() {
        let new_phantom_with_config_method_ident = new_phantom_with_config_method_ident
            .as_ref()
            .expect("agents with config allocate new_phantom_with_config");
        let body = build_constructor_body(
            quote! { let #phantom_uuid_ident = golem_rust::Uuid::new_v4(); },
            quote! { Some(#phantom_uuid_ident.into()) },
            quote! { Some(#phantom_uuid_ident) },
            agent_config_params_as_rpc_param.clone(),
        );
        quote! {
            pub fn #new_phantom_with_config_method_ident(#(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let optional_get_phantom_with_config_impl = if !constructor_agent_config_param_defs.is_empty() {
        let get_phantom_with_config_method_ident = get_phantom_with_config_method_ident
            .as_ref()
            .expect("agents with config allocate get_phantom_with_config");
        let body = build_constructor_body(
            quote! {},
            quote! { Some(#phantom_id_param_ident.into()) },
            quote! { Some(#phantom_id_param_ident) },
            agent_config_params_as_rpc_param.clone(),
        );
        quote! {
            pub fn #get_phantom_with_config_method_ident(#phantom_id_param_ident: golem_rust::Uuid, #(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let get_impl = if agent_is_durable {
        let get_method_ident = get_method_ident
            .as_ref()
            .expect("durable agents allocate get");
        let body = build_constructor_body(
            quote! {},
            quote! { None },
            quote! { None },
            quote! { Vec::new() },
        );
        quote! {
            pub fn #get_method_ident(#(#constructor_data_value_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let new_phantom_body = build_constructor_body(
        quote! { let #phantom_uuid_ident = golem_rust::Uuid::new_v4(); },
        quote! { Some(#phantom_uuid_ident.into()) },
        quote! { Some(#phantom_uuid_ident) },
        quote! { Vec::new() },
    );

    let get_phantom_body = build_constructor_body(
        quote! {},
        quote! { Some(#phantom_id_param_ident.into()) },
        quote! { Some(#phantom_id_param_ident) },
        quote! { Vec::new() },
    );

    quote! {
        pub struct #remote_client_type_name {
            agent_id: String,
            phantom_id: Option<golem_rust::Uuid>,
            component_id: golem_rust::schema::wit::wire::ComponentId,
            wasm_rpc: golem_rust::golem_agentic::golem::agent::host::WasmRpc,
        }

        impl #remote_client_type_name {
            #get_impl

            #optional_get_with_config_impl

            pub fn #new_phantom_method_ident(#(#constructor_data_value_param_defs,)*) -> #remote_client_type_name {
                #new_phantom_body
            }

            #optional_new_phantom_with_config_impl

            pub fn #get_phantom_method_ident(#phantom_id_param_ident: golem_rust::Uuid, #(#constructor_data_value_param_defs,)*) -> #remote_client_type_name {
                #get_phantom_body
            }

            #optional_get_phantom_with_config_impl

            pub fn #phantom_id_accessor_ident(&self) -> Option<golem_rust::Uuid> {
                self.phantom_id
            }

            pub fn #get_agent_id_accessor_ident(&self) -> String {
                self.agent_id.clone()
            }

            #methods_impl
        }
    }
}

#[cfg(test)]
mod tests {
    use super::get_remote_client;
    use quote::{ToTokens, format_ident, quote};
    use syn::parse_quote;

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
        assert!(rendered.contains("get_phantom"));
        assert!(rendered.contains("get_phantom_with_config"));
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
                                        syn::Pat::Ident(pat_ident) => {
                                            Some(pat_ident.ident.to_string())
                                        }
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
                            if let syn::Stmt::Local(local) = stmt {
                                if let syn::Pat::Ident(pat_ident) = &local.pat {
                                    if pat_ident.ident == "agent_id" {
                                        agent_id_is_shadowed = true;
                                    }
                                }
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
}

fn generate_constructor_data_value_params_encoding(
    param_idents: &[proc_macro2::Ident],
    constructor_value_ident: &proc_macro2::Ident,
) -> proc_macro2::TokenStream {
    let constructor_record =
        positional_record_schema_value(param_idents, "Failed to convert constructor parameter");
    quote! {
        let #constructor_value_ident = #constructor_record;
        golem_rust::agentic::__reject_quota_tokens_in_agent_constructor(&#constructor_value_ident)
            .unwrap_or_else(|err| panic!("Invalid agent constructor parameters: {err}"));
    }
}

fn get_remote_agent_methods_info(
    tr: &ItemTrait,
    type_parameter_names: &[String],
) -> RemoteAgentMethodsInfo {
    let user_method_names = tr
        .items
        .iter()
        .filter_map(extract_method)
        .filter(|method| !is_static_method(&method.sig))
        .map(|method| method.sig.ident.to_string());
    let mut agent_method_names = AgentClientMethodNames::new(user_method_names);

    let method_impls = tr
        .items
        .iter()
        .filter_map(|trait_item| {
            let method = extract_method(trait_item)?;

            if is_static_method(&method.sig) {
                return None;
            }

            if let Some(violation) = find_generic_param_in_return(&method.sig, type_parameter_names)
            {
                return Some(generic_type_in_agent_return_type_error(
                    violation.span,
                    &violation.type_name,
                ));
            }

            if let Some(violation) = find_generic_param_in_inputs(&method.sig, type_parameter_names)
            {
                return Some(generic_type_in_agent_method_error(
                    violation.span,
                    &violation.type_name,
                ));
            }

            let keep = |pat_type: &syn::PatType| !is_principal_param(pat_type);
            let input_defs = collect_kept_args(&method.sig, keep);
            let input_idents: Vec<syn::Ident> = collect_typed_params(&method.sig, keep)
                .into_iter()
                .map(|param| param.ident)
                .collect();

            let method_name = &method.sig.ident;
            let trigger_name = agent_method_names.fresh_ident(format!("trigger_{method_name}"));
            let schedule_name = agent_method_names.fresh_ident(format!("schedule_{method_name}"));
            let schedule_cancelable_name =
                agent_method_names.fresh_ident(format!("schedule_cancelable_{method_name}"));

            Some(generate_method_code(
                method_name,
                &trigger_name,
                &schedule_name,
                &schedule_cancelable_name,
                &input_defs,
                &input_idents,
                &method.sig,
            ))
        })
        .collect::<Vec<_>>();

    let code = quote! { #(#method_impls)* };
    RemoteAgentMethodsInfo::new(code, agent_method_names)
}

fn extract_method(item: &syn::TraitItem) -> Option<&syn::TraitItemFn> {
    if let syn::TraitItem::Fn(m) = item {
        Some(m)
    } else {
        None
    }
}

fn generate_method_code(
    method_name: &syn::Ident,
    trigger_name: &syn::Ident,
    schedule_name: &syn::Ident,
    schedule_cancelable_name: &syn::Ident,
    input_defs: &[&syn::FnArg],
    input_idents: &[syn::Ident],
    sig: &syn::Signature,
) -> proc_macro2::TokenStream {
    let remote_method_name = method_name.to_string();
    let remote_token = quote! { #remote_method_name };
    let fn_output_info = FunctionOutputInfo::from_signature(sig);
    let return_type = match &sig.output {
        syn::ReturnType::Type(_, ty) => quote! { #ty },
        syn::ReturnType::Default => quote! { () },
    };
    let process_invoke_result = match &sig.output {
        syn::ReturnType::Type(_, ty) if !fn_output_info.is_unit => decode_result_value(
            ty,
            quote! { rpc_result_ok.expect("remote method returned no value") },
        ),
        _ => quote! {},
    };

    let input_record = positional_record_schema_value(input_idents, "Failed to encode parameter");
    let encoded_input = encode_value_only_carrier(input_record);
    let encode_input = quote! { let input = #encoded_input; };
    let scheduled_time_param = fresh_param_ident(input_idents, "scheduled_time");

    quote! {
        pub async fn #method_name(#(#input_defs),*) -> #return_type {
            #encode_input

            let rpc_result_future = self.wasm_rpc.async_invoke_and_await(
                #remote_token,
                input
            );

            let rpc_result: Result<Option<golem_rust::SchemaValue>, golem_rust::golem_agentic::golem::agent::host::RpcError> =
                golem_rust::agentic::await_invoke_schema_value_result(rpc_result_future).await;

            let rpc_result_ok =
                rpc_result.unwrap_or_else(|e| panic!("rpc call to {} failed: {:?}", #remote_token, e));

            #process_invoke_result
        }

        pub fn #trigger_name(#(#input_defs),*) {
            #encode_input

            let rpc_result: Result<(), golem_rust::golem_agentic::golem::agent::host::RpcError> =
                self.wasm_rpc.invoke(#remote_token, input);

            rpc_result.unwrap_or_else(|e| panic!("rpc call to trigger {} failed: {:?}", #remote_token, e));
        }

        pub fn #schedule_name(#(#input_defs,)* #scheduled_time_param: golem_rust::wasip2::clocks::wall_clock::Datetime) {
            #encode_input

            self.wasm_rpc.schedule_invocation(
                #scheduled_time_param,
                #remote_token,
                input
            );
        }

        pub fn #schedule_cancelable_name(#(#input_defs,)* #scheduled_time_param: golem_rust::wasip2::clocks::wall_clock::Datetime) -> golem_rust::golem_agentic::golem::agent::host::CancellationToken {
            #encode_input

            self.wasm_rpc.schedule_cancelable_invocation(
                #scheduled_time_param,
                #remote_token,
                input
            )
        }
    }
}

fn fresh_param_ident(occupied: &[syn::Ident], preferred_name: &str) -> syn::Ident {
    let occupied = occupied
        .iter()
        .map(ToString::to_string)
        .collect::<HashSet<_>>();
    if !occupied.contains(preferred_name) {
        return format_ident!("{}", preferred_name);
    }

    let mut suffix = 1usize;
    loop {
        let candidate = format!("{preferred_name}{suffix}");
        if !occupied.contains(&candidate) {
            return format_ident!("{}", candidate);
        }
        suffix += 1;
    }
}

struct RemoteAgentMethodsInfo {
    methods_impl: proc_macro2::TokenStream,
    method_names: AgentClientMethodNames,
}

impl RemoteAgentMethodsInfo {
    fn new(methods_impl: proc_macro2::TokenStream, method_names: AgentClientMethodNames) -> Self {
        Self {
            methods_impl,
            method_names,
        }
    }
}

#[derive(Debug)]
struct AgentClientMethodNames {
    method_names: HashSet<String>,
    next_suffix_by_name: HashMap<String, usize>,
}

impl AgentClientMethodNames {
    fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            method_names: names.into_iter().collect(),
            next_suffix_by_name: HashMap::new(),
        }
    }

    fn fresh_ident(&mut self, preferred_name: impl Into<String>) -> syn::Ident {
        let preferred_name = preferred_name.into();
        let name = if self.method_names.insert(preferred_name.clone()) {
            self.next_suffix_by_name
                .entry(preferred_name.clone())
                .or_insert(1);
            preferred_name
        } else {
            let next_suffix = self
                .next_suffix_by_name
                .entry(preferred_name.clone())
                .or_insert(1);

            loop {
                let candidate = format!("{preferred_name}{next_suffix}");
                *next_suffix += 1;

                if self.method_names.insert(candidate.clone()) {
                    break candidate;
                }
            }
        };

        format_ident!("{}", name)
    }

    fn contains(&self, name: &str) -> bool {
        self.method_names.contains(name)
    }
}
