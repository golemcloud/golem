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
use syn::{ItemImpl, ReturnType, Type};

use crate::agentic::helpers::{
    get_input_param_type, get_output_param_type, InputParamType, OutputParamType,
};

pub fn agent_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let impl_block = match parse_impl_block(&item) {
        Ok(b) => b,
        Err(e) => return e.to_compile_error().into(),
    };

    let (impl_generics, ty_generics, where_clause) = impl_block.generics.split_for_impl();
    let self_ty = &impl_block.self_ty;
    let (trait_name_ident, trait_name_str_raw) = extract_trait_name(&impl_block);

    let (match_arms, constructor_method) =
        build_match_arms(&impl_block, trait_name_str_raw.to_string());
    let constructor_method = match constructor_method {
        Some(m) => m,
        None => {
            return syn::Error::new_spanned(
                &impl_block.self_ty,
                "No constructor found (a function returning Self is required)",
            )
            .to_compile_error()
            .into();
        }
    };

    let ctor_ident = &constructor_method.sig.ident;
    let ctor_params = extract_param_idents(constructor_method);
    let input_param_type = get_input_param_type(constructor_method);

    let base_agent_impl = generate_base_agent_impl(
        &impl_block,
        &match_arms,
        &trait_name_str_raw,
        &impl_generics,
        &ty_generics,
        where_clause,
    );

    let post_constructor_param_extraction_logic = quote! {
        let instance = Box::new(<#self_ty>::#ctor_ident(#(#ctor_params),*));

        golem_rust::agentic::register_agent_instance(
            golem_rust::agentic::ResolvedAgent { agent: instance }
        );
        Ok(())
    };

    let constructor_param_extraction = generate_constructor_extraction(
        &ctor_params,
        &trait_name_str_raw,
        &post_constructor_param_extraction_logic,
        &input_param_type,
    );

    let initiator_ident = format_ident!("{}Initiator", trait_name_ident);
    let base_initiator_impl =
        generate_initiator_impl(&initiator_ident, &constructor_param_extraction);
    let register_initiator_fn =
        generate_register_initiator_fn(&trait_name_str_raw, &initiator_ident);

    quote! {
        #impl_block
        #base_agent_impl
        #base_initiator_impl
        #register_initiator_fn
    }
    .into()
}

fn parse_impl_block(item: &TokenStream) -> syn::Result<ItemImpl> {
    syn::parse::<ItemImpl>(item.clone())
}

fn extract_trait_name(impl_block: &syn::ItemImpl) -> (syn::Ident, String) {
    let trait_name = if let Some((_bang, path, _for_token)) = &impl_block.trait_ {
        path.segments.last().unwrap().ident.clone()
    } else {
        panic!("Expected trait implementation, found none");
    };

    let trait_name_str_raw = trait_name.to_string();
    (trait_name, trait_name_str_raw)
}

fn extract_param_idents(method: &syn::ImplItemFn) -> Vec<syn::Ident> {
    method
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let syn::FnArg::Typed(pat_ty) = arg {
                if let syn::Pat::Ident(pat_ident) = &*pat_ty.pat {
                    Some(pat_ident.ident.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

fn build_match_arms(
    impl_block: &syn::ItemImpl,
    agent_type_name: String,
) -> (Vec<proc_macro2::TokenStream>, Option<&syn::ImplItemFn>) {
    let mut match_arms = Vec::new();
    let mut constructor_method = None;

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            let returns_self = match &method.sig.output {
                ReturnType::Type(_, ty) => match &**ty {
                    Type::Path(tp) => tp.path.segments.last().unwrap().ident == "Self",
                    _ => false,
                },
                _ => false,
            };

            let method_name_str = method.sig.ident.to_string();

            if returns_self {
                constructor_method = Some(method);
                continue;
            }

            let param_idents = extract_param_idents(method);

            let method_name = &method.sig.ident.to_string();

            let ident = &method.sig.ident;

            let input_param_type = get_input_param_type(method);

            let output_param_type = get_output_param_type(method);

            let post_method_param_extraction_logic = match output_param_type {
                OutputParamType::Tuple => quote! {
                    let result = self.#ident(#(#param_idents),*);
                    <_ as golem_rust::agentic::Schema>::to_element_value(result).map_err(|e| {
                        golem_rust::agentic::custom_error(format!(
                            "Failed serializing return value for method {}: {}",
                            #method_name, e
                        ))
                    }).map(|element_value| {
                        golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![element_value])
                    })
                },
                OutputParamType::Multimodal => quote! {
                    // multimodal_result is assumed to be of type `Multimodal`. It cannot be wrapped in anything else because
                    // DataValue::Tuple cannot hold multimodal
                    // The actual representation of `Multimodal` is TBD
                    let multimodal_result = self.#ident(#(#param_idents),*);

                    // TODO; once Multimodal representation is decided, convert its elements to Vec<(String, ElementValue)>
                    // using corresponding `Schema` implementations
                    Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(vec![]))
                },
            };

            let method_param_extraction = generate_method_param_extraction(
                &param_idents,
                &agent_type_name,
                method_name_str.as_str(),
                &post_method_param_extraction_logic,
                &input_param_type,
            );

            match_arms.push(quote! {
                #method_name => {
                    #method_param_extraction
                }
            });
        }
    }

    (match_arms, constructor_method)
}

fn generate_method_param_extraction(
    param_idents: &[syn::Ident],
    agent_type_name: &str,
    method_name: &str,
    call_back: &proc_macro2::TokenStream, // A call back is the logic that calls the method after the extraction of paramters and then converting to either DataValue::Tuple or DataValue::Multimodal
    input_param_type: &InputParamType,
) -> proc_macro2::TokenStream {
    let extraction: Vec<proc_macro2::TokenStream> = param_idents.iter().enumerate().map(|(i, ident)| {
        let ident_result = format_ident!("{}_result", ident);
        quote! {
            let #ident_result = match &input {
                golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(values) => {
                    let value = values.get(#i);

                    let element_value_result =  match value {
                        Some(v) => Ok(v.clone()),
                        None => Err(golem_rust::agentic::invalid_input_error(format!("Missing arguments in method {}", #method_name))),
                    };

                    let element_value = element_value_result?;

                    let element_schema = golem_rust::agentic::get_method_parameter_type(
                        &golem_rust::agentic::AgentTypeName(#agent_type_name.to_string()),
                        #method_name,
                        #i,
                    ).ok_or_else(|| {
                        golem_rust::agentic::custom_error(format!(
                            "Internal Error: Parameter schema not found for agent: {}, method: {}, parameter index: {}",
                            #agent_type_name, #method_name, #i
                        ))
                    })?;
                    let deserialized_value = golem_rust::agentic::Schema::from_element_value(element_value, element_schema).map_err(|e| {
                        golem_rust::agentic::invalid_input_error(format!("Failed parsing arg {} for method {}: {}", #i, #method_name, e))
                    })?;
                    Ok(deserialized_value)
                },
                golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(_) => {
                    // TODO; support multimodal and add call back logic here
                    Err(golem_rust::agentic::internal_error("Multimodal input not supported currently"))
                }
            };
            let #ident = #ident_result?;
        }
    }).collect();

    match input_param_type {
        InputParamType::Tuple => quote! {
            #(#extraction)*
            #call_back
        }
        .into(),
        InputParamType::Multimodal => quote! {
           extraction[0] // When it comes to multimodal, there is only 1 set of tokens and that represents all parameters
        }
        .into(),
    }
}

fn generate_base_agent_impl(
    impl_block: &syn::ItemImpl,
    match_arms: &[proc_macro2::TokenStream],
    trait_name_str: &str,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
) -> proc_macro2::TokenStream {
    let self_ty = &impl_block.self_ty;
    quote! {
        impl #impl_generics golem_rust::agentic::Agent for #self_ty #ty_generics #where_clause {
            fn get_id(&self) -> String {
                todo!("Unimplemented get_id method")
            }

            fn invoke(&mut self, method_name: String, input: golem_rust::golem_agentic::golem::agent::common::DataValue)
                -> Result<golem_rust::golem_agentic::golem::agent::common::DataValue, golem_rust::golem_agentic::golem::agent::common::AgentError> {
                match method_name.as_str() {
                    #(#match_arms,)*
                    _ => Err(golem_rust::agentic::invalid_method_error(method_name)),
                }
            }

            fn get_definition(&self)
                -> ::golem_rust::golem_agentic::golem::agent::common::AgentType {
                golem_rust::agentic::get_agent_type_by_name(&golem_rust::agentic::AgentTypeName(#trait_name_str.to_string()))
                    .expect("Agent definition not found")
            }
        }
    }
}

fn generate_constructor_extraction(
    ctor_params: &[syn::Ident],
    agent_type_name: &str,
    call_back: &proc_macro2::TokenStream, // A call back is the logic that instantiates the constructor after the extraction of paramters
    input_param_type: &InputParamType,
) -> proc_macro2::TokenStream {
    let extraction: Vec<proc_macro2::TokenStream> = ctor_params.iter().enumerate().map(|(i, ident)| {
        let ident_result = format_ident!("{}_result", ident);
        quote! {
            let #ident_result = match &params {
                golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(values) => {
                    let element_value_result = match values.get(#i) {
                        Some(v) => Ok(v.clone()),
                        None => Err(golem_rust::agentic::invalid_input_error(format!("Missing constructor arguments for agent {}", #agent_type_name))),
                    };

                    let element_value = element_value_result?;

                    let element_schema = golem_rust::agentic::get_constructor_parameter_type(
                        &golem_rust::agentic::AgentTypeName(#agent_type_name.to_string()),
                        #i,
                    ).ok_or_else(|| {
                        golem_rust::agentic::internal_error(format!(
                            "Constructor parameter schema not found for agent: {}, parameter index: {}",
                            #agent_type_name, #i
                        ))
                    })?;

                    golem_rust::agentic::Schema::from_element_value(element_value, element_schema).map_err(|e| {
                        golem_rust::agentic::invalid_input_error(format!("Failed parsing constructor arg {}: {}", #i, e))
                    })
                },
                golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(_) => {
                    // TODO; support multimodal and add call back logic here
                    Err(golem_rust::agentic::internal_error("Multimodal input not supported currently"))
                }
            };

            let #ident = #ident_result?;
        }
    }).collect::<Vec<_>>();

    match input_param_type {
        InputParamType::Tuple => quote! {
            #(#extraction)*
            #call_back
        }
        .into(),
        InputParamType::Multimodal => quote! {
           extraction[0] // When it comes to multimodal, there is only 1 set of tokens and that represents all parameters
        }
        .into(),
    }
}

fn generate_initiator_impl(
    initiator_ident: &syn::Ident,
    constructor_param_extraction: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        struct #initiator_ident;

        impl golem_rust::agentic::AgentInitiator for #initiator_ident {
            fn initiate(&self, params: golem_rust::golem_agentic::golem::agent::common::DataValue)
                -> Result<(), golem_rust::golem_agentic::golem::agent::common::AgentError> {
                #constructor_param_extraction
            }
        }
    }
}

fn generate_register_initiator_fn(
    trait_name_str_raw: &str,
    initiator_ident: &syn::Ident,
) -> proc_macro2::TokenStream {
    let register_initiator_fn_name = format_ident!(
        "register_agent_initiator_{}",
        trait_name_str_raw.to_lowercase()
    );

    quote! {
        #[::ctor::ctor]
        fn #register_initiator_fn_name() {
            golem_rust::agentic::register_agent_initiator(
                #trait_name_str_raw.to_string().as_str(),
                Box::new(#initiator_ident)
            );
        }
    }
}
