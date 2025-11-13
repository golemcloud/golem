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
use syn::{ItemTrait, Type};

use crate::agentic::helpers::{extract_inner_type_if_multimodal, is_constructor_method};
use crate::agentic::{
    get_remote_client,
    helpers::{get_input_param_info, get_output_param_info, ParamType},
    multiple_constructor_methods_error, no_constructor_method_error,
};

pub fn agent_definition_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let item_trait = syn::parse_macro_input!(item as ItemTrait);

    match get_agent_type_with_remote_client(&item_trait) {
        Ok(agent_type_with_remote_client) => {
            let AgentTypeWithRemoteClient {
                agent_type,
                remote_client,
            } = agent_type_with_remote_client;

            let register_fn_name = get_register_function_ident(&item_trait);

            let register_fn = quote! {
                #[::ctor::ctor]
                fn #register_fn_name() {
                    let agent_type = #agent_type;
                    golem_rust::agentic::register_agent_type(
                        golem_rust::agentic::AgentTypeName(agent_type.type_name.to_string()),
                        agent_type
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

        Err(invalid_trait_error) => invalid_trait_error,
    }
}

fn get_register_function_ident(item_trait: &ItemTrait) -> proc_macro2::Ident {
    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_suffix = &trait_name_str.to_lowercase();

    format_ident!("__register_agent_type_{}", register_fn_suffix)
}

struct AgentTypeWithRemoteClient {
    agent_type: proc_macro2::TokenStream,
    remote_client: proc_macro2::TokenStream,
}

fn get_agent_type_with_remote_client(
    item_trait: &syn::ItemTrait,
) -> Result<AgentTypeWithRemoteClient, TokenStream> {
    let trait_ident = &item_trait.ident;
    let type_name = trait_ident.to_string();

    let mut constructor_methods = vec![];

    for item in &item_trait.items {
        if let syn::TraitItem::Fn(trait_fn) = item {
            if is_constructor_method(&trait_fn.sig) {
                constructor_methods.push(trait_fn.clone());
            }
        }
    }

    let methods = item_trait.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(trait_fn) = item {
            let input_param_info = get_input_param_info(&trait_fn.sig);
            let output_param_info = get_output_param_info(&trait_fn.sig);

            if is_constructor_method(&trait_fn.sig) {
                return None;
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

            match input_param_info.param_type {
                ParamType::Tuple =>  {
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
                ParamType::Multimodal => {
                    for input in &trait_fn.sig.inputs {
                        if let syn::FnArg::Typed(pat_type) = input {
                            let ty = &pat_type.ty;

                            let inner_type: &Type = extract_inner_type_if_multimodal(ty).expect(
                                "Expected Multimodal type to have an inner type",
                            );

                            input_parameters.push(quote! {
                                golem_rust::agentic::Multimodal::<#inner_type>::get_schema()
                            });

                        }
                    }
                }
            }

            match output_param_info.param_type {
                ParamType::Tuple => {
                    match &trait_fn.sig.output {
                        syn::ReturnType::Default => (),
                        syn::ReturnType::Type(_, ty) => {
                            let is_unit = matches!(**ty, syn::Type::Tuple(ref t) if t.elems.is_empty());

                            if !is_unit {
                                output_parameters.push(quote! {
                                    ("return-value".to_string(), <#ty as golem_rust::agentic::Schema>::get_type())
                                });
                            }
                        }
                    };
                },
                ParamType::Multimodal => {
                    match &trait_fn.sig.output {
                        syn::ReturnType::Default => (),
                        syn::ReturnType::Type(_, ty) => {
                            let inner_type: &Type = extract_inner_type_if_multimodal(ty).expect(
                                "Expected Multimodal type to have an inner type",
                            );
                            output_parameters.push(quote! {
                                <#inner_type as golem_rust::agentic::MultimodalSchema>::get_multimodal_schema()
                            });
                        }
                    };
                }
            }

            let input_schema = match input_param_info.param_type {
                ParamType::Tuple => quote! {
                    golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#input_parameters),*])
                },
                ParamType::Multimodal => {
                    let multimodal_param = &input_parameters[0];
                    quote! {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(#multimodal_param)
                    }
                },
            };

            let output_schema = match output_param_info.param_type {
                ParamType::Tuple =>
                    quote! {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#output_parameters),*])
                    },
                ParamType::Multimodal => {
                    let multimodal_param = &output_parameters[0];

                    quote! {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(#multimodal_param)
                    }
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

    let mut constructor_param_type = ParamType::Tuple;

    // name and type of the constructor params
    let mut constructor_param_defs = vec![];

    // just the parmaeter identities
    let mut constructor_param_idents = vec![];

    if constructor_methods.is_empty() {
        return Err(no_constructor_method_error(item_trait).into());
    }

    if constructor_methods.len() > 1 {
        return Err(multiple_constructor_methods_error(item_trait).into());
    }

    if let Some(ctor_fn) = &constructor_methods.first().as_mut() {
        constructor_param_type = get_input_param_info(&ctor_fn.sig).param_type;

        match constructor_param_type {
            ParamType::Tuple => {
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
            ParamType::Multimodal => {
                todo!("Multimodal constructor parameters are not yet supported")
            }
        }
    }

    let remote_client = get_remote_client(
        item_trait,
        &constructor_param_type,
        constructor_param_defs,
        constructor_param_idents,
    );

    let agent_constructor_input_schema = match constructor_param_type {
        ParamType::Tuple => quote! {
            golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#constructor_parameters_with_schema),*])
        },
        ParamType::Multimodal => quote! {
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

    Ok(AgentTypeWithRemoteClient {
        agent_type: quote! {
            golem_rust::golem_agentic::golem::agent::common::AgentType {
                type_name: #type_name.to_string(),
                description: "".to_string(),
                methods: vec![#(#methods),*],
                dependencies: vec![],
                constructor: #agent_constructor,
            }
        },
        remote_client,
    })
}
