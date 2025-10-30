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

use crate::agentic::helpers::to_kebab_case;

pub fn agent_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let impl_block = match parse_impl_block(&item) {
        Ok(b) => b,
        Err(e) => return e.to_compile_error().into(),
    };

    let (impl_generics, ty_generics, where_clause) = impl_block.generics.split_for_impl();
    let self_ty = &impl_block.self_ty;
    let (trait_name, trait_name_str_raw, trait_name_str_kebab) = extract_trait_name(&impl_block);

    let (match_arms, constructor_method) = build_match_arms(&impl_block);
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

    let base_agent_impl = generate_base_agent_impl(
        &impl_block,
        &match_arms,
        &trait_name_str_kebab,
        &impl_generics,
        &ty_generics,
        where_clause,
    );
    let constructor_param_extraction = generate_constructor_extraction(&ctor_params);
    let initiator_ident = format_ident!("{}Initiator", trait_name);
    let base_initiator_impl = generate_initiator_impl(
        &initiator_ident,
        self_ty,
        ctor_ident,
        &ctor_params,
        &constructor_param_extraction,
    );
    let register_initiator_fn = generate_register_initiator_fn(
        &trait_name_str_raw,
        &trait_name_str_kebab,
        &initiator_ident,
    );

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

fn extract_trait_name(impl_block: &syn::ItemImpl) -> (syn::Ident, String, String) {
    let trait_name = if let Some((_bang, path, _for_token)) = &impl_block.trait_ {
        path.segments.last().unwrap().ident.clone()
    } else {
        panic!("Expected trait implementation, found none");
    };

    let trait_name_str_raw = trait_name.to_string();
    let trait_name_str_kebab = to_kebab_case(&trait_name_str_raw);
    (trait_name, trait_name_str_raw, trait_name_str_kebab)
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

            if returns_self {
                constructor_method = Some(method);
                continue;
            }

            let param_idents = extract_param_idents(method);
            let method_param_extraction = generate_method_param_extraction(&param_idents);
            let method_name = to_kebab_case(&method.sig.ident.to_string());
            let ident = &method.sig.ident;

            match_arms.push(quote! {
                #method_name => {
                    #(#method_param_extraction)*
                    let result = self.#ident(#(#param_idents),*);
                    let wit_value = <_ as golem_rust::agentic::Schema>::to_wit_value(result);
                    let element_value = golem_rust::golem_agentic::golem::agent::common::ElementValue::ComponentModel(wit_value);
                    Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![element_value]))
                }
            });
        }
    }

    (match_arms, constructor_method)
}

fn generate_method_param_extraction(param_idents: &[syn::Ident]) -> Vec<proc_macro2::TokenStream> {
    param_idents.iter().enumerate().map(|(i, ident)| {
        quote! {
            let element_value_result = match &input {
                golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(values) => {
                    let value = values.get(#i);

                    match value {
                        Some(v) => Ok(v.clone()),
                        None => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                            golem_rust::wasm_rpc::ValueAndType::new(
                                golem_rust::wasm_rpc::Value::String(format!("Missing arguments at pos {}", #i)),
                                golem_rust::wasm_rpc::analysis::analysed_type::str(),
                            ).into(),
                        )),
                    }
                },
                _ => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                    golem_rust::wasm_rpc::ValueAndType::new(
                        golem_rust::wasm_rpc::Value::String("Only component types supported".into()),
                        golem_rust::wasm_rpc::analysis::analysed_type::str(),
                    ).into(),
                ))
            };

            let element_value = element_value_result?;

            let wit_value_result = match element_value {
                golem_rust::golem_agentic::golem::agent::common::ElementValue::ComponentModel(wit_value) => Ok(wit_value),
                _ => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                    golem_rust::wasm_rpc::ValueAndType::new(
                        golem_rust::wasm_rpc::Value::String("Only ComponentModel ElementValue supported".into()),
                        golem_rust::wasm_rpc::analysis::analysed_type::str(),
                    ).into(),
                ))
            };

            let wit_value = wit_value_result?;

            let #ident = golem_rust::agentic::Schema::from_wit_value(wit_value).map_err(|e| {
                golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                    golem_rust::wasm_rpc::ValueAndType::new(
                        golem_rust::wasm_rpc::Value::String(format!("Failed parsing arg {}: {}", #i, e)),
                        golem_rust::wasm_rpc::analysis::analysed_type::str(),
                    ).into(),
                )
            })?;
        }
    }).collect()
}

fn generate_base_agent_impl(
    impl_block: &syn::ItemImpl,
    match_arms: &[proc_macro2::TokenStream],
    trait_name_str_kebab: &str,
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
                    _ => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                        golem_rust::wasm_rpc::ValueAndType::new(
                            golem_rust::wasm_rpc::Value::String(format!("Method not found: {}", method_name)),
                            golem_rust::wasm_rpc::analysis::analysed_type::str(),
                        ).into(),
                    )),
                }
            }

            fn get_definition(&self)
                -> ::golem_rust::golem_agentic::golem::agent::common::AgentType {
                golem_rust::agentic::get_agent_type_by_name(&golem_rust::agentic::AgentTypeName(#trait_name_str_kebab.to_string()))
                    .expect("Agent definition not found")
            }
        }
    }
}

fn generate_constructor_extraction(ctor_params: &[syn::Ident]) -> Vec<proc_macro2::TokenStream> {
    ctor_params.iter().enumerate().map(|(i, ident)| {
        quote! {
            let element_value_result = match &params {
                golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(values) => {
                    match values.get(#i) {
                        Some(v) => Ok(v.clone()),
                        None => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                            golem_rust::wasm_rpc::ValueAndType::new(
                                golem_rust::wasm_rpc::Value::String(format!("Missing arguments at pos {}", #i)),
                                golem_rust::wasm_rpc::analysis::analysed_type::str(),
                            ).into(),
                        )),
                    }
                },
                _ => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                    golem_rust::wasm_rpc::ValueAndType::new(
                        golem_rust::wasm_rpc::Value::String("Only component types supported".into()),
                        golem_rust::wasm_rpc::analysis::analysed_type::str(),
                    ).into(),
                )),
            };

            let element_value = element_value_result?;

            let wit_value_result = match element_value {
                golem_rust::golem_agentic::golem::agent::common::ElementValue::ComponentModel(wit_value) => Ok(wit_value),
                _ => Err(golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                    golem_rust::wasm_rpc::ValueAndType::new(
                        golem_rust::wasm_rpc::Value::String("Only ComponentModel ElementValue supported".into()),
                        golem_rust::wasm_rpc::analysis::analysed_type::str(),
                    ).into(),
                )),
            };

            let wit_value = wit_value_result?;

            let #ident = golem_rust::agentic::Schema::from_wit_value(wit_value).map_err(|e| {
                golem_rust::golem_agentic::golem::agent::common::AgentError::CustomError(
                    golem_rust::wasm_rpc::ValueAndType::new(
                        golem_rust::wasm_rpc::Value::String(format!("Failed parsing ctor arg {}: {}", #i, e)),
                        golem_rust::wasm_rpc::analysis::analysed_type::str(),
                    ).into(),
                )
            })?;
        }
    }).collect()
}

fn generate_initiator_impl(
    initiator_ident: &syn::Ident,
    self_ty: &syn::Type,
    ctor_ident: &syn::Ident,
    ctor_params: &[syn::Ident],
    constructor_param_extraction: &[proc_macro2::TokenStream],
) -> proc_macro2::TokenStream {
    quote! {
        struct #initiator_ident;

        impl golem_rust::agentic::AgentInitiator for #initiator_ident {
            fn initiate(&self, params: golem_rust::golem_agentic::golem::agent::common::DataValue)
                -> Result<(), golem_rust::golem_agentic::golem::agent::common::AgentError> {
                #(#constructor_param_extraction)*
                let instance = Box::new(<#self_ty>::#ctor_ident(#(#ctor_params),*));
                golem_rust::agentic::register_agent_instance(
                    golem_rust::agentic::ResolvedAgent { agent: instance }
                );
                Ok(())
            }
        }
    }
}

fn generate_register_initiator_fn(
    trait_name_str_raw: &str,
    trait_name_str_kebab: &str,
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
                #trait_name_str_kebab.to_string().as_str(),
                Box::new(#initiator_ident)
            );
        }
    }
}
