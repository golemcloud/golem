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
    } = get_remote_agent_methods_info(item_trait, agent_type_parameter_names, agent_is_durable);

    let get_method_ident = agent_is_durable.then(|| {
        if method_names.contains("get") {
            method_names.fresh_ident("get_")
        } else {
            method_names.fresh_ident("get")
        }
    });
    let new_phantom_method_ident = method_names.fresh_ident("new_phantom");
    let get_phantom_method_ident =
        agent_is_durable.then(|| method_names.fresh_ident("get_phantom"));
    let get_with_config_method_ident = (agent_is_durable
        && !constructor_agent_config_param_defs.is_empty())
    .then(|| method_names.fresh_ident("get_with_config"));
    let new_phantom_with_config_method_ident = (!constructor_agent_config_param_defs.is_empty())
        .then(|| method_names.fresh_ident("new_phantom_with_config"));
    let get_phantom_with_config_method_ident = (agent_is_durable
        && !constructor_agent_config_param_defs.is_empty())
    .then(|| method_names.fresh_ident("get_phantom_with_config"));
    let phantom_id_accessor_ident =
        agent_is_durable.then(|| method_names.fresh_ident("phantom_id"));
    let get_agent_id_accessor_ident =
        agent_is_durable.then(|| method_names.fresh_ident("get_agent_id"));
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

    let new_phantom_doc = if agent_is_durable {
        "Creates a new agent instance with a fresh random phantom id."
    } else {
        "Creates a local logical proxy; each invocation receives a fresh final identity."
    };

    let optional_new_phantom_with_config_impl = if !constructor_agent_config_param_defs.is_empty() {
        let new_phantom_with_config_method_ident = new_phantom_with_config_method_ident
            .as_ref()
            .expect("agents with config allocate new_phantom_with_config");
        let body = if agent_is_durable {
            build_constructor_body(
                quote! { let #phantom_uuid_ident = golem_rust::Uuid::new_v4(); },
                quote! { Some(#phantom_uuid_ident.into()) },
                quote! { Some(#phantom_uuid_ident) },
                agent_config_params_as_rpc_param.clone(),
            )
        } else {
            build_ephemeral_constructor_body(
                &remote_client_type_name,
                &type_name,
                &encode_constructor,
                &constructor_value_ident,
                agent_config_params_as_rpc_param.clone(),
            )
        };
        quote! {
            #[doc = #new_phantom_doc]
            pub fn #new_phantom_with_config_method_ident(#(#constructor_data_value_param_defs,)* #(#constructor_agent_config_param_defs,)*) -> #remote_client_type_name {
                #body
            }
        }
    } else {
        quote! {}
    };

    let optional_get_phantom_with_config_impl = if agent_is_durable
        && !constructor_agent_config_param_defs.is_empty()
    {
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

    let new_phantom_body = if agent_is_durable {
        build_constructor_body(
            quote! { let #phantom_uuid_ident = golem_rust::Uuid::new_v4(); },
            quote! { Some(#phantom_uuid_ident.into()) },
            quote! { Some(#phantom_uuid_ident) },
            quote! { Vec::new() },
        )
    } else {
        build_ephemeral_constructor_body(
            &remote_client_type_name,
            &type_name,
            &encode_constructor,
            &constructor_value_ident,
            quote! { Vec::new() },
        )
    };

    let get_phantom_body = build_constructor_body(
        quote! {},
        quote! { Some(#phantom_id_param_ident.into()) },
        quote! { Some(#phantom_id_param_ident) },
        quote! { Vec::new() },
    );

    let durable_fields = agent_is_durable.then(|| {
        quote! {
            agent_id: String,
            phantom_id: Option<golem_rust::Uuid>,
            component_id: golem_rust::schema::wit::wire::ComponentId,
        }
    });
    let get_phantom_impl = agent_is_durable.then(|| quote! {
        pub fn #get_phantom_method_ident(#phantom_id_param_ident: golem_rust::Uuid, #(#constructor_data_value_param_defs,)*) -> #remote_client_type_name { #get_phantom_body }
    });
    let accessors = agent_is_durable.then(|| {
        quote! {
            pub fn #phantom_id_accessor_ident(&self) -> Option<golem_rust::Uuid> { self.phantom_id }
            pub fn #get_agent_id_accessor_ident(&self) -> String { self.agent_id.clone() }
        }
    });
    quote! {
        pub struct #remote_client_type_name {
            #durable_fields
            wasm_rpc: golem_rust::golem_agentic::golem::agent::host::WasmRpc,
        }

        impl #remote_client_type_name {
            #get_impl

            #optional_get_with_config_impl

            #[doc = #new_phantom_doc]
            pub fn #new_phantom_method_ident(#(#constructor_data_value_param_defs,)*) -> #remote_client_type_name {
                #new_phantom_body
            }

            #optional_new_phantom_with_config_impl

            #get_phantom_impl

            #optional_get_phantom_with_config_impl

            #accessors

            #methods_impl
        }
    }
}

fn build_ephemeral_constructor_body(
    client: &syn::Ident,
    type_name: &str,
    encode_constructor: &proc_macro2::TokenStream,
    constructor_value: &syn::Ident,
    config: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        #encode_constructor
        let wasm_rpc = golem_rust::golem_agentic::golem::agent::host::WasmRpc::new(
            #type_name,
            golem_rust::encode_schema_value(&#constructor_value)
                .expect("Failed to encode constructor parameters"),
            None,
            #config,
        );
        #client { wasm_rpc }
    }
}

#[cfg(test)]
mod tests;

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
    agent_is_durable: bool,
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
                agent_is_durable,
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
    agent_is_durable: bool,
) -> proc_macro2::TokenStream {
    let remote_method_name = method_name.to_string();
    let remote_token = quote! { #remote_method_name };
    let fn_output_info = FunctionOutputInfo::from_signature(sig);
    let return_type = match &sig.output {
        syn::ReturnType::Type(_, ty) => quote! { #ty },
        syn::ReturnType::Default => quote! { () },
    };
    let stream_types = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(param) => Some(param.ty.as_ref()),
            syn::FnArg::Receiver(_) => None,
        })
        .chain(match &sig.output {
            syn::ReturnType::Type(_, ty) => Some(ty.as_ref()),
            syn::ReturnType::Default => None,
        })
        .collect::<Vec<_>>();
    let reject_non_awaited_stream_invocation = quote! {
        if false #(|| <#stream_types as golem_rust::agentic::Schema>::contains_stream())* {
            let _ = (#(&#input_idents),*);
            panic!("live streams cannot cross remote or scheduled agent invocation boundaries")
        }
    };
    let process_invoke_result = match &sig.output {
        syn::ReturnType::Type(_, ty) if !fn_output_info.is_unit => decode_result_value(
            ty,
            quote! { rpc_result_ok.expect("remote method returned no value") },
        ),
        _ => quote! {},
    };

    let input_record = positional_record_schema_value(input_idents, "Failed to encode parameter");
    let encoded_input = encode_value_only_carrier(input_record.clone());
    let encode_input = quote! { let input = #encoded_input; };
    let encode_input_async = quote! {
        let input_value = #input_record;
        let input = golem_rust::encode_schema_value_async(&input_value)
            .await
            .expect("Failed to encode parameters");
    };
    let scheduled_time_param = fresh_param_ident(input_idents, "scheduled_time");
    if agent_is_durable {
        return quote! {
        pub async fn #method_name(#(#input_defs),*) -> #return_type {
            #encode_input_async

            let rpc_result_future = self.wasm_rpc.async_invoke_and_await(
                #remote_token,
                input,
                None
            ).future;

            let rpc_result: Result<Option<golem_rust::SchemaValue>, golem_rust::golem_agentic::golem::agent::host::RpcError> =
                golem_rust::agentic::await_invoke_schema_value_result(rpc_result_future).await;

            let rpc_result_ok =
                rpc_result.unwrap_or_else(|e| panic!("rpc call to {} failed: {:?}", #remote_token, e));

            #process_invoke_result
        }

        pub fn #trigger_name(#(#input_defs),*) {
            #reject_non_awaited_stream_invocation
            #encode_input

            let rpc_result: Result<(), golem_rust::golem_agentic::golem::agent::host::RpcError> =
                self.wasm_rpc.invoke(#remote_token, input, None).map(|_| ());

            rpc_result.unwrap_or_else(|e| panic!("rpc call to trigger {} failed: {:?}", #remote_token, e));
        }

        pub fn #schedule_name(#(#input_defs,)* #scheduled_time_param: golem_rust::ScheduledTime) -> Result<(), golem_rust::golem_agentic::golem::agent::host::RpcError> {
            #reject_non_awaited_stream_invocation
            #encode_input

            self.wasm_rpc.schedule_invocation(
                #scheduled_time_param,
                #remote_token,
                input,
                None
            ).map(|_| ())
        }

        pub fn #schedule_cancelable_name(#(#input_defs,)* #scheduled_time_param: golem_rust::ScheduledTime) -> Result<golem_rust::golem_agentic::golem::agent::host::CancellationToken, golem_rust::golem_agentic::golem::agent::host::RpcError> {
            #reject_non_awaited_stream_invocation
            #encode_input

            self.wasm_rpc.schedule_cancelable_invocation(
                #scheduled_time_param,
                #remote_token,
                input,
                None
            ).map(|receipt| receipt.cancellation_token)
        }
        };
    }

    quote! {
        pub async fn #method_name(#(#input_defs),*) -> golem_rust::agentic::EphemeralInvocationResult<#return_type> {
            #encode_input_async
            let invocation = self.wasm_rpc.async_invoke_and_await(#remote_token, input, None);
            let metadata = invocation.metadata;
            let rpc_result: Result<Option<golem_rust::SchemaValue>, golem_rust::golem_agentic::golem::agent::host::RpcError> =
                golem_rust::agentic::await_invoke_schema_value_result(invocation.future).await;
            let rpc_result_ok = rpc_result.unwrap_or_else(|e| panic!("rpc call to {} failed: {:?}", #remote_token, e));
            let value = { #process_invoke_result };
            golem_rust::agentic::EphemeralInvocationResult { metadata, value }
        }

        pub fn #trigger_name(#(#input_defs),*) -> golem_rust::golem_agentic::golem::agent::host::InvocationMetadata {
            #reject_non_awaited_stream_invocation
            #encode_input
            self.wasm_rpc.invoke(#remote_token, input, None)
                .unwrap_or_else(|e| panic!("rpc call to trigger {} failed: {:?}", #remote_token, e))
        }

        pub fn #schedule_name(#(#input_defs,)* #scheduled_time_param: golem_rust::ScheduledTime) -> Result<golem_rust::golem_agentic::golem::agent::host::InvocationMetadata, golem_rust::golem_agentic::golem::agent::host::RpcError> {
            #reject_non_awaited_stream_invocation
            #encode_input
            self.wasm_rpc.schedule_invocation(#scheduled_time_param, #remote_token, input, None)
                .map(|receipt| receipt.metadata)
        }

        pub fn #schedule_cancelable_name(#(#input_defs,)* #scheduled_time_param: golem_rust::ScheduledTime) -> Result<golem_rust::golem_agentic::golem::agent::host::CancelableScheduledInvocationReceipt, golem_rust::golem_agentic::golem::agent::host::RpcError> {
            #reject_non_awaited_stream_invocation
            #encode_input
            self.wasm_rpc.schedule_cancelable_invocation(#scheduled_time_param, #remote_token, input, None)
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
