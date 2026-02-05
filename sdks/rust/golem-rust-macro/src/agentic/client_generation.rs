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

    let method_impls = tr.items.iter().filter_map(|item| {

        if let syn::TraitItem::Fn(method) = item {
            if is_static_method(&method.sig) {
                return None;
            }

            if let syn::ReturnType::Type(_, ty) = &method.sig.output {

                let type_name = match &**ty {
                    syn::Type::Path(type_path) => {
                        type_path.path.segments.last().unwrap().ident.to_string()
                    },
                    _ => "".to_string(),
                };

                if type_parameter_names.contains(&type_name) {
                    return generic_type_in_agent_return_type_error(method.sig.ident.span(), &type_name).into();
                }

                if let syn::Type::Path(type_path) = &**ty {
                    if type_path.path.segments.last().unwrap().ident == "Self" {
                        return None;
                    }
                }
            }

            let method_name = &method.sig.ident;

            let trigger_method_name = format_ident!("trigger_{}", method_name);

            let schedule_method_name = format_ident!("schedule_{}", method_name);

            agent_method_names.extend(vec![method_name.to_string(), trigger_method_name.to_string(), schedule_method_name.to_string()]);

            let remote_method_name = rpc_invoke_method_name(&agent_type_name, &method_name.to_string());

            let remote_method_name_token = quote! { #remote_method_name };

            let input_defs: Vec<&syn::FnArg> = method
                .sig
                .inputs
                .iter()
                .filter(|arg| {
                    match arg {
                       FnArg::Receiver(_) => true,

                       FnArg::Typed(pat_type) => {
                            let type_name = match &*pat_type.ty {
                               Type::Path(type_path) => {
                                    type_path.path.segments.last()
                                        .map(|s| s.ident == "Principal")
                                        .unwrap_or(false)
                                }
                                _ => false,
                            };

                            !type_name
                        }
                    }
                })
                .collect();


            for fn_arg in method.sig.inputs.iter() {
                if let FnArg::Typed(pat_type) = fn_arg {
                    let pat_type_name = match &*pat_type.ty {
                        Type::Path(type_path) => {
                            type_path.path.segments.last().unwrap().ident.to_string()
                        },
                        _ => "".to_string(),
                    };

                    if type_parameter_names.contains(&pat_type_name) {
                        return generic_type_in_agent_method_error(pat_type.ty.span(), &pat_type_name).into();
                    }
                }
            }

            let input_param_idents: Vec<_> = method
                .sig
                .inputs
                .iter()
                .filter_map(|arg| {
                    let syn::FnArg::Typed(pat_type) = arg else {
                        return None;
                    };

                    let ident = match &*pat_type.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                        _ => return None,
                    };

                    // Only exclude if the type is exactly `Principal`
                    if let syn::Type::Path(type_path) = &*pat_type.ty {
                        if type_path
                            .path
                            .segments
                            .last()
                            .is_some_and(|seg| seg.ident == "Principal")
                        {
                            return None;
                        }
                    }

                    Some(ident)
                })
                .collect();
            
            let fn_output_info = FunctionOutputInfo::from_signature(&method.sig);

            let return_type = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => quote! { #ty },
                syn::ReturnType::Default => quote! { () },
            };

            let process_invoke_result = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => {
                    if fn_output_info.is_unit {
                        quote! {}
                    } else {
                        quote! {
                            let schema_type = <#ty as golem_rust::agentic::Schema>::get_type();
                            <#ty as golem_rust::agentic::Schema>::from_wit_value(wit_value, schema_type).expect("Failed to deserialize rpc result to return type")
                        }
                    }
                },
                syn::ReturnType::Default => quote! {},
            };

            Some(quote!{
              pub async fn #method_name(#(#input_defs),*) -> #return_type {
                let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                  vec![#(golem_rust::agentic::Schema::to_wit_value(#input_param_idents).expect("Failed")),*];

                let rpc_result_future = self.wasm_rpc.async_invoke_and_await(
                  #remote_method_name_token,
                  &wit_values
                );

                let rpc_result: Result<golem_rust::golem_wasm::WitValue, golem_rust::golem_wasm::RpcError> = golem_rust::agentic::await_invoke_result(rpc_result_future).await;

                let rpc_result_ok = rpc_result.expect(format!("rpc call to {} failed", #remote_method_name_token).as_str());

                let wit_value = golem_rust::agentic::unwrap_wit_tuple(rpc_result_ok);

                #process_invoke_result
              }

              pub fn #trigger_method_name(#(#input_defs),*) {
                let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                  vec![#(golem_rust::agentic::Schema::to_wit_value(#input_param_idents).expect("Failed")),*];

                let rpc_result: Result<(), golem_rust::golem_wasm::RpcError> = self.wasm_rpc.invoke(
                  #remote_method_name_token,
                  &wit_values
                );

                rpc_result.expect(format!("rpc call to trigger {} failed", #remote_method_name_token).as_str());
              }

              pub fn #schedule_method_name(#(#input_defs),*, scheduled_time: golem_rust::golem_wasm::golem_rpc_0_2_x::types::Datetime) {
                let wit_values: Vec<golem_rust::golem_wasm::WitValue> =
                  vec![#(golem_rust::agentic::Schema::to_wit_value(#input_param_idents).expect("Failed")),*];

                self.wasm_rpc.schedule_invocation(
                  scheduled_time,
                  #remote_method_name_token,
                  &wit_values
                );
              }
           })
        } else {
            None
        }

    }).collect::<Vec<_>>();

    let code = quote! {
        #(#method_impls)*
    };

    RemoteAgentMethodsInfo::new(code, agent_method_names)
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
