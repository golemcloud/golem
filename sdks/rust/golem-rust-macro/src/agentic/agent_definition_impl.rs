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

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemTrait};

use crate::agentic::helpers::{
    InputParamType, OutputParamType, convert_to_kebab, get_input_param_type, get_output_param_type
};

pub fn agent_definition_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let item_trait = syn::parse_macro_input!(item as syn::ItemTrait);

    let (agent_type, remote_client) = get_agent_type(&item_trait);

    let register_fn_name = get_register_function_ident(&item_trait);

    let register_fn = quote! {
        #[::ctor::ctor]
        fn #register_fn_name() {
            golem_rust::agentic::register_agent_type(
               golem_rust::agentic::AgentTypeName(#agent_type.type_name.to_string()),
               #agent_type
            );
        }
    };

    let result = quote! {
        #item_trait
        #register_fn
        #remote_client

    };

    result.into()
}

fn get_register_function_ident(item_trait: &ItemTrait) -> proc_macro2::Ident {
    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_suffix = &trait_name_str.to_lowercase();

    format_ident!("__register_agent_type_{}", register_fn_suffix)
}

fn get_agent_type(
    item_trait: &syn::ItemTrait,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let trait_ident = &item_trait.ident;
    let type_name = trait_ident.to_string();

    let mut constructor_methods = vec![];

    // Capture constructor methods (returning Self)
    for item in &item_trait.items {
        if let syn::TraitItem::Fn(trait_fn) = item {
            if let syn::ReturnType::Type(_, ty) = &trait_fn.sig.output {
                if let syn::Type::Path(type_path) = &**ty {
                    if type_path.path.segments.last().unwrap().ident == "Self" {
                        constructor_methods.push(trait_fn.clone());
                    }
                }
            }
        }
    }

    let methods = item_trait.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(trait_fn) = item {
            if let syn::ReturnType::Type(_, ty) = &trait_fn.sig.output {
                if let syn::Type::Path(type_path) = &**ty {
                    if type_path.path.segments.last().unwrap().ident == "Self" {
                        return None;
                    }
                }
            }


            let name = &trait_fn.sig.ident;
            let method_name = &name.to_string();

            let mut description = String::new();

            for attr in &trait_fn.attrs {
                if attr.path().is_ident("description") {
                    let mut found = None;
                    attr.parse_nested_meta(|meta| {
                        if meta.path.is_ident("description") {
                            let lit: syn::LitStr = meta.value()?.parse()?;
                            found = Some(lit.value());
                            Ok(())
                        } else {
                            Err(meta.error("expected `description = \"...\"`"))
                        }
                    })
                    .ok();
                    if let Some(val) = found {
                        description = val;
                    }
                }
            }

            let mut input_parameters = vec![];
            let mut output_parameters = vec![];
            

            let input_param_type = get_input_param_type(&trait_fn.sig);
            let output_param_type = get_output_param_type(&trait_fn.sig);

            match input_param_type {
                InputParamType::Tuple =>  {
                    for input in &trait_fn.sig.inputs {
                        if let syn::FnArg::Typed(pat_type) = input {
                            let param_name = match &*pat_type.pat {
                                syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                                _ => "_".to_string(), // fallback for patterns like destructuring
                            };
                            let ty = &pat_type.ty;
                            input_parameters.push(quote! {
                                (#param_name.to_string(), <#ty as golem_rust::agentic::Schema>::get_type())
                            });
                        }
                    }

                },
                InputParamType::Multimodal => {
                    let input = &trait_fn.sig.inputs[0];
                    if let syn::FnArg::Typed(_) = input {
                        // TODO; Once multimodal representation is decided,
                        // we can expand this to retireve each name and type from multimodal;
                    }

                }
            }

            match output_param_type {
                OutputParamType::Tuple => {
                    match &trait_fn.sig.output {
                        syn::ReturnType::Default => (),
                        syn::ReturnType::Type(_, ty) => {
                            output_parameters.push(quote! {
                                ("return-value".to_string(), <#ty as golem_rust::agentic::Schema>::get_type())
                            });
                        }
                    };
                },
                OutputParamType::Multimodal => {
                    // TODO; Once multimodal representation is decided,
                    // we can expand this to retireve each name and type from multimodal;
                }
            }

            let input_schema = match input_param_type {
                InputParamType::Tuple => quote! {
                    golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#input_parameters),*])
                },
                InputParamType::Multimodal => quote! {
                    golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(vec![#(#input_parameters),*])
                },
            };

            let output_schema = match output_param_type {
                OutputParamType::Tuple => quote! {
                    golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#output_parameters),*])
                },
                OutputParamType::Multimodal => quote! {
                    golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(vec![#(#output_parameters),*])
                },
            };

            Some(quote! {
                golem_rust::golem_agentic::golem::agent::common::AgentMethod {
                    name: #method_name.to_string(),
                    description: #description.to_string(),
                    prompt_hint: None,
                    input_schema: #input_schema,
                    output_schema: #output_schema,
                }
            })
        } else {
            None
        }
    });

    // It holds the name and type of the constructor parmeters with schema
    let mut constructor_parameters_with_schema: Vec<proc_macro2::TokenStream> = vec![];

    let mut constructor_param_type = InputParamType::Tuple;

    // name and type of the constructor params
    let mut constructor_param_defs = vec![];

    // just the parmaeter identities
    let mut constructor_param_idents = vec![];

    if let Some(ctor_fn) = &constructor_methods.first().as_mut() {
        constructor_param_type = get_input_param_type(&ctor_fn.sig);

        match constructor_param_type {
            InputParamType::Tuple => {
                for input in &ctor_fn.sig.inputs {
                    if let syn::FnArg::Typed(pat_type) = input {
                        let param_name = match &*pat_type.pat {
                            syn::Pat::Ident(pat_ident) => {
                                let param_ident = &pat_ident.ident;
                                let ty = &pat_type.ty;
                                constructor_param_defs.push(quote! {
                                    #param_ident: #ty
                                });

                                constructor_param_idents.push(quote! {
                                    #param_ident
                                });

                                pat_ident.ident.to_string()
                            }
                            _ => "_".to_string(),
                        };

                        let ty = &pat_type.ty;
                        constructor_parameters_with_schema.push(quote! {
                            (#param_name.to_string(), <#ty as golem_rust::agentic::Schema>::get_type())
                        });
                    }
                }
            }
            InputParamType::Multimodal => {
                let input = &ctor_fn.sig.inputs[0];
                if let syn::FnArg::Typed(_) = input {
                    // TODO; Once multimodal representation is decided,
                    // we can expand this to retireve each name and type from multimodal;
                }
            }
        }
    }

    let remote_trait_name = format_ident!("{}Client", item_trait.ident);

    let method_impls = get_remote_method_impls(item_trait, type_name.clone());

    let remote_client = match constructor_param_type {
        // If constructor parameter is tuple we can generate the remote client
        InputParamType::Tuple => {
            let remote_client = quote! {
                pub struct #remote_trait_name {
                    agent_id: golem_rust::wasm_rpc::AgentId,
                }

                impl #remote_trait_name {
                    pub fn get(#(#constructor_param_defs), *) -> #remote_trait_name {
                        let agent_type =
                           golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name).expect("Internal Error: Agent type not registered");

                         let element_values = vec![#(golem_rust::agentic::Schema::to_element_value(#constructor_param_idents).expect("Failed to convert constructor parameter to ElementValue")),*];

                         let data_value  = golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(element_values);

                         let agent_id_string =
                           golem_rust::golem_agentic::golem::agent::host::make_agent_id(#type_name, &data_value).expect("Internal Error: Failed to make agent id");

                         let agent_id = golem_rust::wasm_rpc::AgentId { agent_id: agent_id_string, component_id: agent_type.implemented_by.clone() };

                        #remote_trait_name { agent_id: agent_id }

                       // #remote_trait_name {}
                    }

                    pub fn get_id(&self) -> String {
                        self.agent_id.agent_id.clone()
                    }

                    #method_impls


                }

            };

            remote_client
        }
        InputParamType::Multimodal => {
            // TODO; Once multimodal representation is decided,
            // We can almost copy paste the above tokens expect for picking only 1 constructor parameter, and enumerate and get DataValue::Multimodal
            quote! {}
        }
    };

    let agent_constructor_input_schema = match constructor_param_type {
        InputParamType::Tuple => quote! {
            golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#constructor_parameters_with_schema),*])
        },
        InputParamType::Multimodal => quote! {
            golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(vec![#(#constructor_parameters_with_schema),*])
        },
    };

    let agent_constructor = quote! { golem_rust::golem_agentic::golem::agent::common::AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: #agent_constructor_input_schema,
        }
    };

    (
        quote! {
            golem_rust::golem_agentic::golem::agent::common::AgentType {
                type_name: #type_name.to_string(),
                description: "".to_string(),
                methods: vec![#(#methods),*],
                dependencies: vec![],
                constructor: #agent_constructor,
            }
        },
        remote_client,
    )
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
            let agent_type_name_kebab = convert_to_kebab(&agent_type_name);
            let method_name_str_kebab = convert_to_kebab(&method_name.to_string());

            // To form remote method name, we convert this back to kebab-case similar to TS
            let remote_method_name = format!(
                "{}.{{{}}}",
                agent_type_name_kebab,
                method_name_str_kebab
            );

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


            let return_type = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => quote! { #ty },
                syn::ReturnType::Default => quote! { () },
            };

            // Depending on the input parameter type and output parameter type generate different implementations
            let input_parameter_type = get_input_param_type(&method.sig);

            let output_parameter_type = get_output_param_type(&method.sig);

            match (input_parameter_type, output_parameter_type) {
                
                (InputParamType::Tuple, OutputParamType::Tuple) =>
                    Some(quote!{
                        pub fn #method_name(#(#inputs),*) -> #return_type {
                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> = 
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed")),*];
          
                          let wasm_rpc = golem_rust::wasm_rpc::WasmRpc::new(&self.agent_id);
          
                          // Change to async later
                          let result: Result<golem_rust::wasm_rpc::WitValue, golem_rust::wasm_rpc::RpcError> = wasm_rpc.invoke_and_await(
                              #remote_method_name_token,
                              &wit_values
                          );

                          let element_value = golem_rust::golem_agentic::golem::agent::common::ElementValue::ComponentModel(result.expect("RPC call failed"));
                          let element_schema = <#return_type as golem_rust::agentic::Schema>::get_type();

                          <#return_type as golem_rust::agentic::Schema>::from_element_value(element_value, element_schema).expect("Failed to convert ElementValue to return type")

                        }
                     }),
                  (InputParamType::Tuple, OutputParamType::Multimodal) => {
                    Some(quote!{
                        pub fn #method_name(#(#inputs),*) -> #return_type {

                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> = 
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed to serialize")),*];
          
                          let wasm_rpc = golem_rust::wasm_rpc::WasmRpc::new(&self.agent_id);
          
                          // Change to async later
                          let result: Result<golem_rust::wasm_rpc::WitValue, golem_rust::wasm_rpc::RpcError> = wasm_rpc.invoke_and_await(
                              #remote_method_name_token,
                              &wit_values
                          );

                          // If its multimodal, we cannot use Schema instance directly, we need to enumerate the values from multimodal
                          // and apply them separately.
                          todo!("Multimodal output parameter handling not implemented yet");
                        }
                     })
                  },
                _ => {
                    None
                }

            }
    
        } else {
            None
        }

    }).collect::<Vec<_>>();

    quote! {
        #(#method_impls)*
    }
}
