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

    let remote_agent_methods_info =
        get_remote_agent_methods_info(item_trait, agent_type_parameter_names);

    let method_names = &remote_agent_methods_info.method_names;

    let methods_impl = remote_agent_methods_info.methods_impl;

    let encode_constructor =
        generate_constructor_data_value_params_encoding(constructor_data_value_param_idents);

    let get_method_ident = if method_names.contains_get() {
        format_ident!("get_")
    } else {
        format_ident!("get")
    };

    let agent_config_params_as_rpc_param = {
        let add_rpc_params_entries = constructor_agent_config_param_idents.iter().map(|param_ident|
            quote! { result.append(&mut ::golem_rust::agentic::IntoRpcConfigParam::into_rpc_param(#param_ident, &[])); }
        );

        quote! {
            {
                let mut result = Vec::new();
                #(#add_rpc_params_entries)*
                result
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
                let agent_type =
                    golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name)
                        .expect("Internal Error: Agent type not registered");

                #encode_constructor

                #prelude

                let agent_id = golem_rust::golem_agentic::golem::agent::host::make_agent_id(
                    #type_name,
                    golem_rust::encode_schema_value(&constructor_value)
                        .expect("Failed to encode constructor parameters for agent id"),
                    #phantom_wire,
                )
                .expect("Internal Error: Failed to make agent id");

                let wasm_rpc = golem_rust::golem_agentic::golem::agent::host::WasmRpc::new(
                    #type_name,
                    golem_rust::encode_schema_value(&constructor_value)
                        .expect("Failed to encode constructor parameters"),
                    #phantom_wire,
                    #config,
                );

                #remote_client_type_name {
                    agent_id,
                    phantom_id: #phantom_struct,
                    component_id: agent_type.implemented_by,
                    wasm_rpc,
                }
            }
        };

    let optional_get_with_config_impl = if agent_is_durable
        && !constructor_agent_config_param_defs.is_empty()
    {
        let body = build_constructor_body(
            quote! {},
            quote! { None },
            quote! { None },
            agent_config_params_as_rpc_param.clone(),
        );
        quote! {
            pub fn get_with_config(#(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let optional_new_phantom_with_config_impl = if !constructor_agent_config_param_defs.is_empty() {
        let body = build_constructor_body(
            quote! { let phantom_uuid = golem_rust::Uuid::new_v4(); },
            quote! { Some(phantom_uuid.into()) },
            quote! { Some(phantom_uuid) },
            agent_config_params_as_rpc_param.clone(),
        );
        quote! {
            pub fn new_phantom_with_config(#(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let optional_get_phantom_with_config_impl = if !constructor_agent_config_param_defs.is_empty() {
        let body = build_constructor_body(
            quote! {},
            quote! { Some(phantom_id.into()) },
            quote! { Some(phantom_id) },
            agent_config_params_as_rpc_param.clone(),
        );
        quote! {
            pub fn get_phantom_with_config(phantom_id: golem_rust::Uuid, #(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let get_impl = if agent_is_durable {
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
        quote! { let phantom_uuid = golem_rust::Uuid::new_v4(); },
        quote! { Some(phantom_uuid.into()) },
        quote! { Some(phantom_uuid) },
        quote! { Vec::new() },
    );

    let get_phantom_body = build_constructor_body(
        quote! {},
        quote! { Some(phantom_id.into()) },
        quote! { Some(phantom_id) },
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

            pub fn new_phantom(#(#constructor_data_value_param_defs,)*) -> #remote_client_type_name {
                #new_phantom_body
            }

            #optional_new_phantom_with_config_impl

            pub fn get_phantom(phantom_id: golem_rust::Uuid, #(#constructor_data_value_param_defs,)*) -> #remote_client_type_name {
                #get_phantom_body
            }

            #optional_get_phantom_with_config_impl

            pub fn phantom_id(&self) -> Option<golem_rust::Uuid> {
                self.phantom_id
            }

            pub fn get_agent_id(&self) -> String {
                self.agent_id.clone()
            }

            #methods_impl
        }
    }
}

#[cfg(test)]
mod tests {
    use super::get_remote_client;
    use quote::{format_ident, quote};
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
) -> proc_macro2::TokenStream {
    let constructor_record =
        positional_record_schema_value(param_idents, "Failed to convert constructor parameter");
    quote! {
        let constructor_value = #constructor_record;
        golem_rust::agentic::__reject_quota_tokens_in_agent_constructor(&constructor_value)
            .unwrap_or_else(|err| panic!("Invalid agent constructor parameters: {err}"));
    }
}

fn get_remote_agent_methods_info(
    tr: &ItemTrait,
    type_parameter_names: &[String],
) -> RemoteAgentMethodsInfo {
    let mut agent_method_names = AgentClientMethodNames::new();

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
            let trigger_name = format_ident!("trigger_{}", method_name);
            let schedule_name = format_ident!("schedule_{}", method_name);
            let schedule_cancelable_name = format_ident!("schedule_cancelable_{}", method_name);

            agent_method_names.extend(vec![
                method_name.to_string(),
                trigger_name.to_string(),
                schedule_name.to_string(),
                schedule_cancelable_name.to_string(),
            ]);

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

        pub fn #schedule_name(#(#input_defs),*, scheduled_time: golem_rust::wasip2::clocks::wall_clock::Datetime) {
            #encode_input

            self.wasm_rpc.schedule_invocation(
                scheduled_time,
                #remote_token,
                input
            );
        }

        pub fn #schedule_cancelable_name(#(#input_defs),*, scheduled_time: golem_rust::wasip2::clocks::wall_clock::Datetime) -> golem_rust::golem_agentic::golem::agent::host::CancellationToken {
            #encode_input

            self.wasm_rpc.schedule_cancelable_invocation(
                scheduled_time,
                #remote_token,
                input
            )
        }
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
    method_names: Vec<String>,
}

impl AgentClientMethodNames {
    fn new() -> Self {
        Self {
            method_names: vec![],
        }
    }

    fn extend(&mut self, names: Vec<String>) {
        self.method_names.extend(names);
    }

    fn contains(&self, name: &str) -> bool {
        self.method_names.iter().any(|n| n == name)
    }

    fn contains_get(&self) -> bool {
        self.contains("get")
    }
}
