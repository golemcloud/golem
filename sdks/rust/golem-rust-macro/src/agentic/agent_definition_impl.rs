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

use crate::agentic::helpers::{is_async_trait_attr, is_constructor_method};
use crate::agentic::{
    async_trait_in_agent_definition_error, get_remote_client, multiple_constructor_methods_error,
    no_constructor_method_error,
};

fn parse_agent_mode(attrs: TokenStream) -> proc_macro2::TokenStream {
    if attrs.is_empty() {
        return quote! {
            golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable
        };
    }

    if let Ok(ident) = syn::parse2::<syn::Ident>(attrs.clone().into()) {
        // Shorthand case: just "ephemeral"
        if ident == "ephemeral" {
            return quote! {
                golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral
            };
        }
    }

    // Try parsing the full expression: mode = "..." or mode = ...
    if let Ok(expr) = syn::parse2::<syn::ExprAssign>(attrs.into()) {
        if let syn::Expr::Path(left) = &*expr.left {
            if left.path.is_ident("mode") {
                // Extract the right side
                match &*expr.right {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(lit_str),
                        ..
                    }) => {
                        if lit_str.value() == "ephemeral" {
                            return quote! {
                                golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral
                            };
                        } else if lit_str.value() == "durable" {
                            return quote! {
                                golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable
                            };
                        }
                    }
                    syn::Expr::Path(path) => {
                        if path.path.is_ident("ephemeral") {
                            return quote! {
                                golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral
                            };
                        } else if path.path.is_ident("durable") {
                            return quote! {
                                golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable
                            };
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    panic!("Invalid agent mode - use `mode = ephemeral` or `mode = durable`");
}

pub fn agent_definition_impl(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_trait = syn::parse_macro_input!(item as ItemTrait);
    let agent_mode = parse_agent_mode(attrs);

    let has_async_trait_attribute = item_trait.attrs.iter().any(is_async_trait_attr);

    if has_async_trait_attribute {
        return async_trait_in_agent_definition_error(&item_trait).into();
    }

    match get_agent_type_with_remote_client(&item_trait, agent_mode) {
        Ok(agent_type_with_remote_client) => {
            let AgentTypeWithRemoteClient {
                agent_type,
                remote_client,
            } = agent_type_with_remote_client;

            let register_fn_name = get_register_function_ident(&item_trait);

            // ctor_parse! instead of #[ctor] to avoid dependency on ctor crate at user side
            // This is one level of indirection to ensure the usage of ctor that is re-exported by golem_rust
            let register_fn = quote! {
                ::golem_rust::ctor::__support::ctor_parse!(#[ctor]fn #register_fn_name() {
                    let agent_type = #agent_type;
                    golem_rust::agentic::register_agent_type(
                        golem_rust::agentic::AgentTypeName(agent_type.type_name.to_string()),
                        agent_type
                    );
                });
            };

            let load_snapshot_item = get_load_snapshot_item();
            let save_snapshot_item = get_save_snapshot_item();

            item_trait.items.push(load_snapshot_item);
            item_trait.items.push(save_snapshot_item);

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

fn get_load_snapshot_item() -> syn::TraitItem {
    syn::parse_quote! {
        async fn load_snapshot(&self, _bytes: Vec<u8>) -> Result<(), String> {
            Err("load_snapshot not implemented".to_string())
        }
    }
}

fn get_save_snapshot_item() -> syn::TraitItem {
    syn::parse_quote! {
        async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
            Err("save_snapshot not implemented".to_string())
        }
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
    mode_value: proc_macro2::TokenStream,
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

            let mut input_schema_logic = vec![];
            let mut output_schema_logic = vec![];
            let input_schema_token = quote! {
                let mut multi_modal_inputs = vec![];
                let mut default_inputs = vec![];
            };
            let output_schema_token = quote! {
               let mut multi_modal_outputs = vec![];
               let mut default_outputs = vec![];
            };

            for input in &trait_fn.sig.inputs {
                if let syn::FnArg::Typed(pat_type) = input {
                    let param_name = match &*pat_type.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                        _ => "_".to_string(), // fallback for patterns like destructuring
                    };
                    let ty = &pat_type.ty;
                    input_schema_logic.push(quote! {
                        let schema: golem_rust::agentic::StructuredSchema = <#ty as golem_rust::agentic::Schema>::get_type();
                        match schema {
                            golem_rust::agentic::StructuredSchema::Default(element_schema) => {
                                default_inputs.push((#param_name.to_string(), element_schema));
                            },
                            golem_rust::agentic::StructuredSchema::Multimodal(name_and_types) => {
                                multi_modal_inputs.extend(name_and_types);
                            }
                        }
                    });
                }
            }

            match &trait_fn.sig.output {
                syn::ReturnType::Default => (),
                syn::ReturnType::Type(_, ty) => {
                    let is_unit = matches!(**ty, syn::Type::Tuple(ref t) if t.elems.is_empty());

                    if !is_unit {
                        output_schema_logic.push(quote! {
                            let schema = <#ty as golem_rust::agentic::Schema>::get_type();
                            match schema {
                                golem_rust::agentic::StructuredSchema::Default(element_schema) => {
                                    default_outputs.push(("return-value".to_string(), element_schema));
                                },
                                golem_rust::agentic::StructuredSchema::Multimodal(name_and_types) => {
                                    multi_modal_outputs.extend(name_and_types)
                                }
                            }
                        });
                    }
                }
            };

            let input_schema = quote! {
                {
                    #input_schema_token
                    #(#input_schema_logic)*
                    if !multi_modal_inputs.is_empty() {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(multi_modal_inputs)
                    } else {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(default_inputs)
                    }
                }
            };

            let output_schema = quote! {
                {
                    #output_schema_token
                    #(#output_schema_logic)*
                    if !multi_modal_outputs.is_empty() {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(multi_modal_outputs)
                    } else {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(default_outputs)
                    }
                }
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

    let constructor_schema_init = quote! {
        let mut constructor_multi_modal_inputs = vec![];
        let mut constructor_default_inputs = vec![];
    };

    let mut constructor_parameters_with_schema: Vec<proc_macro2::TokenStream> = vec![];

    let mut constructor_param_defs = vec![];

    let mut constructor_param_names = vec![];

    if constructor_methods.is_empty() {
        return Err(no_constructor_method_error(item_trait).into());
    }

    if constructor_methods.len() > 1 {
        return Err(multiple_constructor_methods_error(item_trait).into());
    }

    if let Some(ctor_fn) = &constructor_methods.first().as_mut() {
        for input in &ctor_fn.sig.inputs {
            if let syn::FnArg::Typed(pat_type) = input {
                let param_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => {
                        let param_ident = &pat_ident.ident;
                        let ty = &pat_type.ty;
                        constructor_param_defs.push(quote! {
                            #param_ident: #ty
                        });

                        constructor_param_names.push(quote! {
                            #param_ident
                        });

                        pat_ident.ident.to_string()
                    }
                    _ => "_".to_string(),
                };

                let ty = &pat_type.ty;
                constructor_parameters_with_schema.push(quote! {

                    let schema: golem_rust::agentic::StructuredSchema = <#ty as golem_rust::agentic::Schema>::get_type();
                    match schema {
                        golem_rust::agentic::StructuredSchema::Default(element_schema) => {
                            constructor_default_inputs.push((#param_name.to_string(), element_schema));
                        },
                        golem_rust::agentic::StructuredSchema::Multimodal(name_and_types) => {
                            constructor_multi_modal_inputs.extend(name_and_types);
                        }
                    }
                });
            }
        }
    }

    let agent_constructor_input_schema = quote! {
        {
            #constructor_schema_init
            #(#constructor_parameters_with_schema)*
            if !constructor_multi_modal_inputs.is_empty() {
                golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(constructor_multi_modal_inputs)
            } else {
                golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(constructor_default_inputs)
            }
        }
    };

    let constructor_data_schema_token = quote! {
        let constructor_data_schema = {
            #agent_constructor_input_schema
        };
    };

    let remote_client =
        get_remote_client(item_trait, constructor_param_defs, constructor_param_names);

    let agent_constructor = quote! {
        {
         #constructor_data_schema_token

         golem_rust::golem_agentic::golem::agent::common::AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: constructor_data_schema,
         }
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
                mode: #mode_value,
            }
        },
        remote_client,
    })
}
