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
use syn::ItemTrait;

use crate::agentic::helpers::{
    get_input_param_type, get_output_param_type, InputParamType, OutputParamType,
};

pub fn agent_definition_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let item_trait = syn::parse_macro_input!(item as syn::ItemTrait);

    let agent_type = get_agent_type(&item_trait);

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

    };

    result.into()
}

fn get_register_function_ident(item_trait: &ItemTrait) -> proc_macro2::Ident {
    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_suffix = &trait_name_str.to_lowercase();

    format_ident!("__register_agent_type_{}", register_fn_suffix)
}

fn get_agent_type(item_trait: &syn::ItemTrait) -> proc_macro2::TokenStream {
    let type_name = item_trait.ident.to_string();

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

    let mut constructor_parameter_types: Vec<proc_macro2::TokenStream> = vec![];

    let mut constructor_param_type = InputParamType::Tuple;

    if let Some(ctor_fn) = &constructor_methods.first().as_mut() {
        constructor_param_type = get_input_param_type(&ctor_fn.sig);

        match constructor_param_type {
            InputParamType::Tuple => {
                for input in &ctor_fn.sig.inputs {
                    if let syn::FnArg::Typed(pat_type) = input {
                        let param_name = match &*pat_type.pat {
                            syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                            _ => "_".to_string(),
                        };

                        let ty = &pat_type.ty;
                        constructor_parameter_types.push(quote! {
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

    let agent_constructor_input_schema = match constructor_param_type {
        InputParamType::Tuple => quote! {
            golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#constructor_parameter_types),*])
        },
        InputParamType::Multimodal => quote! {
            golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(vec![#(#constructor_parameter_types),*])
        },
    };

    let agent_constructor = quote! { golem_rust::golem_agentic::golem::agent::common::AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: #agent_constructor_input_schema,
        }
    };

    quote! {
        golem_rust::golem_agentic::golem::agent::common::AgentType {
            type_name: #type_name.to_string(),
            description: "".to_string(),
            methods: vec![#(#methods),*],
            dependencies: vec![],
            constructor: #agent_constructor,
        }
    }
}
