// Copyright 2024-2025 Golem Cloud
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

use crate::agentic::helpers::{is_static_method, FunctionOutputInfo};
use crate::agentic::{generic_type_in_agent_method_error, generic_type_in_agent_return_type_error};
use heck::ToKebabCase;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{FnArg, ItemTrait, Type};

pub fn get_remote_client(
    item_trait: &ItemTrait,
    constructor_param_defs: Vec<proc_macro2::TokenStream>,
    constructor_param_idents: Vec<proc_macro2::TokenStream>,
    agent_type_parameter_names: &[String],
) -> proc_macro2::TokenStream {
    let remote_client_type_name = format_ident!("{}Client", item_trait.ident);

    let type_name = item_trait.ident.to_string();

    let remote_agent_methods_info = get_remote_agent_methods_info(
        item_trait,
        type_name.to_string(),
        agent_type_parameter_names,
    );

    let method_names = &remote_agent_methods_info.method_names;

    let methods_impl = remote_agent_methods_info.methods_impl;

    let constructor_params_data_value = quote! {
        let data_value = if structured_values.is_empty() {
            golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![])
        } else {
            match &structured_values[0] {
                golem_rust::agentic::StructuredValue::Default(_) => {
                    let element_values = structured_values.into_iter().map(|vt| {
                        if let golem_rust::agentic::StructuredValue::Default(ev) = vt {
                            ev
                        } else {
                            panic!("constructor parameter type mismatch. Expected default, found multimodal");
                        }
                    }).collect::<Vec<golem_rust::golem_agentic::golem::agent::common::ElementValue>>();

                    golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(element_values)
                }

                golem_rust::agentic::StructuredValue::Multimodal(_) => {
                    let multimodal_result = structured_values.remove(0).get_multimodal_value().expect("Constructor parameter type mismatch. Expected multimodal, found default");
                    golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(multimodal_result)
                }

                golem_rust::agentic::StructuredValue::AutoInjected(_) => {
                    panic!("Internal Error: Trying to convert principal parameter to data value in RPC call");
                }
            }
        };
    };

    let get_method_ident = if method_names.contains_get() {
        format_ident!("get_")
    } else {
        format_ident!("get")
    };

    quote! {
        pub struct #remote_client_type_name {
            agent_id: golem_rust::golem_wasm::AgentId,
            wasm_rpc: golem_rust::golem_wasm::WasmRpc,
        }

        impl #remote_client_type_name {
            pub fn #get_method_ident(#(#constructor_param_defs), *) -> #remote_client_type_name {
                let agent_type =
                   golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name).expect("Internal Error: Agent type not registered");

                 let mut structured_values = vec![#(golem_rust::agentic::Schema::to_structured_value(#constructor_param_idents).expect("Failed to convert constructor parameter to ElementValue")),*];

                 #constructor_params_data_value

                 let agent_id_string =
                   golem_rust::golem_agentic::golem::agent::host::make_agent_id(
                      #type_name,
                      &data_value,
                      None
                   ).expect("Internal Error: Failed to make agent id");

                 let agent_id = golem_rust::golem_wasm::AgentId { agent_id: agent_id_string, component_id: agent_type.implemented_by.clone() };

                 let wasm_rpc = golem_rust::golem_wasm::WasmRpc::new(&agent_id);

                 #remote_client_type_name { agent_id: agent_id, wasm_rpc: wasm_rpc }

            }

            pub fn new_phantom(#(#constructor_param_defs), *) -> #remote_client_type_name {
                let agent_type =
                   golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name).expect("Internal Error: Agent type not registered");

                let mut structured_values = vec![#(golem_rust::agentic::Schema::to_structured_value(#constructor_param_idents).expect("Failed to convert constructor parameter to ElementValue")),*];

                #constructor_params_data_value

                let agent_id_string =
                   golem_rust::golem_agentic::golem::agent::host::make_agent_id(
                        #type_name,
                        &data_value,
                        Some(golem_rust::Uuid::new_v4().into())
                   ).expect("Internal Error: Failed to make agent id");

                 let agent_id = golem_rust::golem_wasm::AgentId { agent_id: agent_id_string, component_id: agent_type.implemented_by.clone() };

                 let wasm_rpc = golem_rust::golem_wasm::WasmRpc::new(&agent_id);

                 #remote_client_type_name { agent_id: agent_id, wasm_rpc: wasm_rpc }

            }

            pub fn get_phantom(phantom_id: golem_rust::Uuid, #(#constructor_param_defs), *) -> #remote_client_type_name {
                let agent_type =
                   golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name).expect("Internal Error: Agent type not registered");

                let mut structured_values = vec![#(golem_rust::agentic::Schema::to_structured_value(#constructor_param_idents).expect("Failed to convert constructor parameter to ElementValue")),*];

                #constructor_params_data_value

                let agent_id_string =
                   golem_rust::golem_agentic::golem::agent::host::make_agent_id(
                        #type_name,
                        &data_value,
                        Some(phantom_id.into())
                   ).expect("Internal Error: Failed to make agent id");

                let agent_id = golem_rust::golem_wasm::AgentId { agent_id: agent_id_string, component_id: agent_type.implemented_by.clone() };

                let wasm_rpc = golem_rust::golem_wasm::WasmRpc::new(&agent_id);

                #remote_client_type_name { agent_id: agent_id, wasm_rpc: wasm_rpc }
            }


            pub fn phantom_id(&self) -> Option<golem_rust::Uuid> {
                let (_, _, phantom_id) = golem_rust::golem_agentic::golem::agent::host::parse_agent_id(&self.agent_id.agent_id).unwrap();
                phantom_id.map(|id| id.into())
            }

            pub fn get_agent_id(&self) -> String {
                self.agent_id.agent_id.clone()
            }

            #methods_impl
        }
    }
}

fn get_remote_agent_methods_info(
    tr: &ItemTrait,
    agent_type_name: String,
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

            if let Err(ts) = validate_return_type(&method.sig, type_parameter_names) {
                return Some(ts);
            }

            if let Err(ts) = validate_input_types(&method.sig, type_parameter_names) {
                return Some(ts);
            }

            let input_defs = collect_input_defs_without_principal(&method.sig);
            let input_idents = collect_input_idents_without_principal(&method.sig);

            let method_name = &method.sig.ident;
            let trigger_name = format_ident!("trigger_{}", method_name);
            let schedule_name = format_ident!("schedule_{}", method_name);
            let schedule_cancelable_name =
                format_ident!("schedule_cancelable_{}", method_name);

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
                &agent_type_name,
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

fn validate_return_type(
    sig: &syn::Signature,
    type_params: &[String],
) -> Result<(), proc_macro2::TokenStream> {
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        if let syn::Type::Path(path) = &**ty {
            let ident = &path.path.segments.last().unwrap().ident;

            if ident == "Self" {
                return Ok(()); // skip Self, still valid
            }

            if type_params.contains(&ident.to_string()) {
                return Err(generic_type_in_agent_return_type_error(
                    sig.ident.span(),
                    &ident.to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_input_types(
    sig: &syn::Signature,
    type_params: &[String],
) -> Result<(), proc_macro2::TokenStream> {
    for fn_arg in &sig.inputs {
        if let FnArg::Typed(pat_type) = fn_arg {
            if let Type::Path(type_path) = &*pat_type.ty {
                let type_name = type_path.path.segments.last().unwrap().ident.to_string();
                if type_params.contains(&type_name) {
                    return Err(generic_type_in_agent_method_error(
                        pat_type.ty.span(),
                        &type_name,
                    ));
                }
            }
        }
    }
    Ok(())
}

fn collect_input_defs_without_principal(sig: &syn::Signature) -> Vec<&syn::FnArg> {
    sig.inputs.iter().filter(|arg| match arg {
        FnArg::Receiver(_) => true,
        FnArg::Typed(pat_type) => !matches!(
            &*pat_type.ty,
            Type::Path(type_path) if type_path.path.segments.last().map(|s| s.ident == "Principal").unwrap_or(false)
        ),
    }).collect()
}

fn collect_input_idents_without_principal(sig: &syn::Signature) -> Vec<syn::Ident> {
    sig.inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                    if let Type::Path(type_path) = &*pat_type.ty {
                        if type_path
                            .path
                            .segments
                            .last()
                            .is_some_and(|seg| seg.ident == "Principal")
                        {
                            return None;
                        }
                    }
                    return Some(pat_ident.ident.clone());
                }
            }
            None
        })
        .collect()
}

fn generate_method_code(
    method_name: &syn::Ident,
    trigger_name: &syn::Ident,
    schedule_name: &syn::Ident,
    schedule_cancelable_name: &syn::Ident,
    agent_type_name: &str,
    input_defs: &[&syn::FnArg],
    input_idents: &[syn::Ident],
    sig: &syn::Signature,
) -> proc_macro2::TokenStream {
    let remote_method_name = rpc_invoke_method_name(agent_type_name, &method_name.to_string());
    let remote_token = quote! { #remote_method_name };
    let fn_output_info = FunctionOutputInfo::from_signature(sig);
    let return_type = match &sig.output {
        syn::ReturnType::Type(_, ty) => quote! { #ty },
        syn::ReturnType::Default => quote! { () },
    };
    let process_invoke_result = match &sig.output {
        syn::ReturnType::Type(_, ty) if !fn_output_info.is_unit => {
            quote! {
                let schema_type = <#ty as golem_rust::agentic::Schema>::get_type();
                <#ty as golem_rust::agentic::Schema>::from_wit_value(wit_value, schema_type)
                    .expect("Failed to deserialize rpc result to return type")
            }
        }
        _ => quote! {},
    };

    quote! {
        pub async fn #method_name(#(#input_defs),*) -> #return_type {
            let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents)
                    .expect("Failed")),*];

            let rpc_result_future = self.wasm_rpc.async_invoke_and_await(
                #remote_token,
                &wit_values
            );

            let rpc_result: Result<golem_rust::golem_wasm::WitValue, golem_rust::golem_wasm::RpcError> =
                golem_rust::agentic::await_invoke_result(rpc_result_future).await;

            let rpc_result_ok =
                rpc_result.expect(format!("rpc call to {} failed", #remote_token).as_str());

            let wit_value = golem_rust::agentic::unwrap_wit_tuple(rpc_result_ok);

            #process_invoke_result
        }

        pub fn #trigger_name(#(#input_defs),*) {
            let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents)
                    .expect("Failed")),*];

            let rpc_result: Result<(), golem_rust::golem_wasm::RpcError> =
                self.wasm_rpc.invoke(#remote_token, &wit_values);

            rpc_result.expect(format!("rpc call to trigger {} failed", #remote_token).as_str());
        }

        pub fn #schedule_name(#(#input_defs),*, scheduled_time: golem_rust::golem_wasm::golem_rpc_0_2_x::types::Datetime) {
            let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents)
                    .expect("Failed")),*];

            self.wasm_rpc.schedule_invocation(
                scheduled_time,
                #remote_token,
                &wit_values
            );
        }

        pub fn #schedule_cancelable_name(#(#input_defs),*, scheduled_time: golem_rust::golem_wasm::golem_rpc_0_2_x::types::Datetime) -> golem_rust::golem_wasm::golem_rpc_0_2_x::types::CancellationToken {
            let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents)
                    .expect("Failed")),*];

            self.wasm_rpc.schedule_cancelable_invocation(
                scheduled_time,
                #remote_token,
                &wit_values
            )
        }
    }
}

fn rpc_invoke_method_name(agent_type_name: &str, method_name: &str) -> String {
    let agent_type_name_kebab = agent_type_name.to_kebab_case();
    let method_name_kebab = method_name.to_kebab_case();

    format!("{}.{{{}}}", agent_type_name_kebab, method_name_kebab)
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
