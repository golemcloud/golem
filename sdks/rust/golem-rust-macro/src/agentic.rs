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
use proc_macro2::Ident;
use quote::{format_ident, quote};
use syn::ItemTrait;

pub fn agent_definition_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let item_trait = syn::parse_macro_input!(item as syn::ItemTrait);

    let agent_type = get_agent_type(&item_trait);

    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_name = get_register_function_ident(&item_trait);

    let register_fn = quote! {
        #[::ctor::ctor]
        fn #register_fn_name() {
            golem_rust::agentic::agent_registry::register_generic_agent_type(
               #trait_name_str.to_string(),
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

pub fn agent_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    item // TODO: implement agent implementation processing
}

pub fn derive_agent_arg(input: TokenStream) -> TokenStream {
    input // TODO: implement AgentArg derive macro
}

fn get_register_function_ident(item_trait: &ItemTrait) -> Ident {
    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_suffix = &trait_name_str.to_lowercase();

    format_ident!("register_generic_agent_type_{}", register_fn_suffix)
}

fn get_agent_type(item_trait: &syn::ItemTrait) -> proc_macro2::TokenStream {
    let type_name = item_trait.ident.to_string();

    let methods = item_trait.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(trait_fn) = item {
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


            let mut parameter_types = vec![]; // This is WIT type for now, but needs to support structured text type
            let mut result_type = vec![];

            if let syn::TraitItem::Fn(trait_fn) = item {
                for input in &trait_fn.sig.inputs {
                    if let syn::FnArg::Typed(pat_type) = input {
                        let ty = &pat_type.ty;
                        parameter_types.push(quote! {
                            ::golem_agentic::bindings::golem::agent::common::ParameterType::Wit(
                                <#ty as ::golem_agentic::AgentArg>::get_wit_type()
                            )
                        });
                    }
                }

                // Handle return type
                match &trait_fn.sig.output {
                    syn::ReturnType::Default => (),
                    syn::ReturnType::Type(_, ty) => {
                        result_type.push(quote! {
                            ::golem_agentic::bindings::golem::agent::common::ParameterType::Wit(
                                <#ty as ::golem_agentic::AgentArg>::get_wit_type()
                            )
                        });
                    }
                };
            }

            let input_parameters = parameter_types;
            let output_parameters = result_type;


            Some(quote! {
                golem_agentic::bindings::golem::agent::common::AgentMethod {
                    name: #method_name.to_string(),
                    description: #description.to_string(),
                    prompt_hint: None,
                    input_schema: ::golem_agentic::bindings::golem::agent::common::DataSchema::Structured(::golem_agentic::bindings::golem::agent::common::Structured {
                          parameters: vec![#(#input_parameters),*]
                    }),
                    output_schema: ::golem_agentic::bindings::golem::agent::common::DataSchema::Structured(::golem_agentic::bindings::golem::agent::common::Structured {
                      parameters: vec![#(#output_parameters),*]
                    }),
                }
            })
        } else {
            None
        }
    });

    quote! {
        golem_agentic::agent_registry::GenericAgentType {
            type_name: #type_name.to_string(),
            description: "".to_string(),
            methods: vec![#(#methods),*],
            requires: vec![]
        }
    }
}
