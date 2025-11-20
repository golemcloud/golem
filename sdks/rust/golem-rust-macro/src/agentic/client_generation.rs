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

use crate::agentic::helpers::{DefaultOrMultimodal, FunctionOutputInfo};
use heck::ToKebabCase;
use quote::{format_ident, quote};
use syn::ItemTrait;

pub fn get_remote_client(
    item_trait: &ItemTrait,
    constructor_param_defs: Vec<proc_macro2::TokenStream>,
    constructor_param_idents: Vec<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let remote_trait_name = format_ident!("{}Client", item_trait.ident);

    let type_name = item_trait.ident.to_string();
    let method_impls = get_remote_method_impls(item_trait, type_name.to_string());

    quote! {
        pub struct #remote_trait_name {
            agent_id: golem_rust::wasm_rpc::AgentId,
            wasm_rpc: golem_rust::wasm_rpc::WasmRpc,
        }

        impl #remote_trait_name {
            pub fn get(#(#constructor_param_defs), *) -> #remote_trait_name {
                let agent_type =
                   golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name).expect("Internal Error: Agent type not registered");

                 let mut value_types = vec![#(golem_rust::agentic::Schema::to_element_value(#constructor_param_idents).expect("Failed to convert constructor parameter to ElementValue")),*];

                 let data_value = match &value_types[0] {
                    golem_rust::agentic::ValueType::Default(_) => {
                        let element_values = value_types.into_iter().map(|vt| {
                            if let golem_rust::agentic::ValueType::Default(ev) = vt {
                                ev
                            } else {
                                panic!("Constructor parameter type mismatch");
                            }
                        }).collect::<Vec<golem_rust::golem_agentic::golem::agent::common::ElementValue>>();

                        golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(element_values)

                    }

                    golem_rust::agentic::ValueType::Multimodal(_) => {
                        let multimodal_result = value_types.remove(0).get_multimodal_value().expect("Constructor parameter type mismatch");
                        golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(multimodal_result)
                    }
                 };

                 let agent_id_string =
                   golem_rust::golem_agentic::golem::agent::host::make_agent_id(#type_name, &data_value).expect("Internal Error: Failed to make agent id");

                 let agent_id = golem_rust::wasm_rpc::AgentId { agent_id: agent_id_string, component_id: agent_type.implemented_by.clone() };

                 let wasm_rpc = golem_rust::wasm_rpc::WasmRpc::new(&agent_id);

                 #remote_trait_name { agent_id: agent_id, wasm_rpc: wasm_rpc }

            }

            pub fn get_agent_id(&self) -> String {
                self.agent_id.agent_id.clone()
            }

            #method_impls
        }
    }
}

fn get_remote_method_impls(tr: &ItemTrait, agent_type_name: String) -> proc_macro2::TokenStream {
    let method_impls = tr.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(method) = item {
            if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                if let syn::Type::Path(type_path) = &**ty {
                    if type_path.path.segments.last().unwrap().ident == "Self" {
                        return None;
                    }
                }
            }

            let method_name = &method.sig.ident;
            let trigger_method_name = format_ident!("trigger_{}", method_name);
            let schedule_method_name = format_ident!("schedule_{}", method_name);

            let remote_method_name = rpc_invoke_method_name(&agent_type_name, &method_name.to_string());

            let remote_method_name_token = {
                quote! {
                   #remote_method_name
                }
            };


            let inputs: Vec<_> = method.sig.inputs.iter().collect();

            let input_idents: Vec<_> = method.sig
                .inputs
                .iter()
                .filter_map(|arg| {
                    if let syn::FnArg::Typed(pat_type) = arg {
                        if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                            Some(pat_ident.ident.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
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
                syn::ReturnType::Default => quote! {
                    ()
                },
            };

                  Some(quote!{
                        pub async fn #method_name(#(#inputs),*) -> #return_type {
                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> =
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed")),*];

                          let rpc_result_future = self.wasm_rpc.async_invoke_and_await(
                            #remote_method_name_token,
                            &wit_values
                          );

                          let rpc_result: Result<golem_rust::wasm_rpc::WitValue, golem_rust::wasm_rpc::RpcError> = golem_rust::agentic::await_invoke_result(rpc_result_future).await;

                          let rpc_result_ok = rpc_result.expect(format!("rpc call to {} failed", #remote_method_name_token).as_str());

                          let wit_value = golem_rust::agentic::unwrap_wit_tuple(rpc_result_ok);

                          #process_invoke_result
                        }

                        pub fn #trigger_method_name(#(#inputs),*) {
                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> =
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed")),*];

                          let rpc_result: Result<(), golem_rust::wasm_rpc::RpcError> = self.wasm_rpc.invoke(
                            #remote_method_name_token,
                            &wit_values
                          );

                          rpc_result.expect(format!("rpc call to trigger {} failed", #remote_method_name_token).as_str());
                        }

                        pub fn #schedule_method_name(#(#inputs),*, scheduled_time: golem_rust::wasm_rpc::golem_rpc_0_2_x::types::Datetime) {
                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> =
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed")),*];

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

    quote! {
        #(#method_impls)*
    }
}

fn rpc_invoke_method_name(agent_type_name: &str, method_name: &str) -> String {
    let agent_type_name_kebab = agent_type_name.to_kebab_case();
    let method_name_kebab = method_name.to_kebab_case();

    format!("{}.{{{}}}", agent_type_name_kebab, method_name_kebab)
}
