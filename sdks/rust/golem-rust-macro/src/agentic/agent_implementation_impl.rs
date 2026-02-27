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
use syn::ItemImpl;

use crate::agentic::helpers::{
    get_asyncness, has_async_trait_attribute, has_autoinject_attribute, is_constructor_method,
    is_static_method, trim_type_parameter, Asyncness, AutoInjectAttrRemover, FunctionOutputInfo,
};
use syn::visit_mut::VisitMut;

pub fn agent_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut impl_block = match parse_impl_block(&item) {
        Ok(b) => b,
        Err(e) => return e.to_compile_error().into(),
    };

    let has_async_trait_attribute = has_async_trait_attribute(&impl_block);

    if has_async_trait_attribute {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "#[async_trait] cannot be used along with #[agent_implementation]. #[agent_implementation] automatically handles async methods. Please remove it",
        )
        .to_compile_error()
        .into();
    }

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

    let has_load_snapshot = impl_block.items.iter().any(|item| {
        if let syn::ImplItem::Fn(method) = item {
            method.sig.ident == "load_snapshot"
        } else {
            false
        }
    });

    let has_save_snapshot = impl_block.items.iter().any(|item| {
        if let syn::ImplItem::Fn(method) = item {
            method.sig.ident == "save_snapshot"
        } else {
            false
        }
    });

    if has_load_snapshot != has_save_snapshot {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "Both load_snapshot and save_snapshot must be implemented together, or neither should be implemented",
        )
        .to_compile_error()
        .into();
    }

    let has_custom_snapshot = has_load_snapshot && has_save_snapshot;

    let ctor_ident = &constructor_method.sig.ident;

    let ctor_param_idents_and_types = extract_param_idents(constructor_method);
    let ctor_param_idents: Vec<syn::Ident> = ctor_param_idents_and_types
        .iter()
        .map(|(ident, _)| ident.clone())
        .collect();

    let base_agent_impl = generate_base_agent_impl(
        &impl_block,
        &match_arms,
        &trait_name_str_raw,
        &impl_generics,
        &ty_generics,
        where_clause,
        has_custom_snapshot,
    );

    let constructor_kind = get_asyncness(&constructor_method.sig);

    let constructor_param_extraction_call_back = match constructor_kind {
        Asyncness::Future => {
            quote! {
                let agent_instance_raw = <#self_ty>::#ctor_ident(#(#ctor_param_idents),*).await;
                let agent_instance = Box::new(agent_instance_raw);
                let agent_id = golem_rust::bindings::golem::api::host::get_self_metadata().agent_id;
                golem_rust::agentic::register_agent_instance(
                    golem_rust::agentic::ResolvedAgent::new(agent_instance)
                );
                Ok(())
            }
        }
        Asyncness::Immediate => {
            quote! {
                let agent_instance = Box::new(<#self_ty>::#ctor_ident(#(#ctor_param_idents),*));
                let agent_id = golem_rust::bindings::golem::api::host::get_self_metadata().agent_id;
                golem_rust::agentic::register_agent_instance(
                    golem_rust::agentic::ResolvedAgent::new(agent_instance)
                );
                Ok(())
            }
        }
    };

    let constructor_param_extraction = generate_constructor_extraction(
        &ctor_param_idents_and_types,
        &trait_name_str_raw,
        constructor_param_extraction_call_back,
    );

    let initiator_ident = format_ident!("__{}Initiator", trait_name_ident);

    let base_initiator_impl =
        generate_initiator_impl(&initiator_ident, &constructor_param_extraction);

    let register_initiator_fn =
        generate_register_initiator_fn(&impl_block.self_ty, &trait_name_ident, &initiator_ident);

    AutoInjectAttrRemover.visit_item_impl_mut(&mut impl_block);

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

// This will include all auto injected parameters too
fn extract_param_idents(method: &syn::ImplItemFn) -> Vec<(syn::Ident, syn::PatType)> {
    method
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let syn::FnArg::Typed(pat_ty) = arg {
                if let syn::Pat::Ident(pat_ident) = &*pat_ty.pat {
                    Some((pat_ident.ident.clone(), pat_ty.clone()))
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
    impl_block: &ItemImpl,
    agent_type_name: String,
) -> (Vec<proc_macro2::TokenStream>, Option<&syn::ImplItemFn>) {
    let mut match_arms = Vec::new();
    let mut constructor_method = None;

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            let self_ty = &impl_block.self_ty;

            let agent_impl_type_name = match &**self_ty {
                syn::Type::Path(type_path) => {
                    type_path.path.segments.last().unwrap().ident.to_string()
                }
                _ => String::new(),
            };

            if is_constructor_method(&method.sig, Some(&agent_impl_type_name)) {
                constructor_method = Some(method);
                continue;
            }

            if is_static_method(&method.sig) {
                continue;
            }

            if method.sig.ident == "load_snapshot" || method.sig.ident == "save_snapshot" {
                continue;
            }

            let method_name_str = method.sig.ident.to_string();

            let param_idents: Vec<syn::Ident> = extract_param_idents(method)
                .into_iter()
                .map(|(ident, _)| ident)
                .collect();

            let method_name = &method.sig.ident.to_string();

            let ident = &method.sig.ident;

            let fn_output_info = FunctionOutputInfo::from_signature(&method.sig);

            let post_method_param_extraction_logic = match fn_output_info.async_ness {
                Asyncness::Future if !fn_output_info.is_unit => quote! {
                    let result = self.#ident(#(#param_idents),*).await;
                    <_ as golem_rust::agentic::Schema>::to_structured_value(result).map_err(|e| {
                        golem_rust::agentic::custom_error(format!(
                            "Failed serializing return value for method {}: {}",
                            #method_name, e
                        ))
                    }).and_then(|result_value| {
                        match result_value {
                            golem_rust::agentic::StructuredValue::Default(element_value) => {
                                 Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![element_value]))
                            },
                            golem_rust::agentic::StructuredValue::Multimodal(result) => {
                                Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(result))
                            },
                            golem_rust::agentic::StructuredValue::AutoInjected(_) => {
                                Err(golem_rust::agentic::custom_error(format!(
                                    "Principal value cannot be returned from method {}",
                                    #method_name
                                )))
                            }
                        }
                    })
                },
                Asyncness::Future => quote! {
                    let _ = self.#ident(#(#param_idents),*).await;
                    Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![]))
                },
                Asyncness::Immediate if !fn_output_info.is_unit => quote! {
                    let result = self.#ident(#(#param_idents),*);
                    <_ as golem_rust::agentic::Schema>::to_structured_value(result).map_err(|e| {
                        golem_rust::agentic::custom_error(format!(
                            "Failed serializing return value for method {}: {}",
                            #method_name, e
                        ))
                    }).and_then(|result_val| {
                        match result_val {
                            golem_rust::agentic::StructuredValue::Default(element_value) => {
                                Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![element_value]))
                            },
                            golem_rust::agentic::StructuredValue::Multimodal(result) => {
                                Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(result))
                            },
                            golem_rust::agentic::StructuredValue::AutoInjected(_) => {
                                Err(golem_rust::agentic::custom_error(format!(
                                    "Principal value cannot be returned from method {}",
                                    #method_name
                                )))
                            }
                        }
                    })
                },
                Asyncness::Immediate => quote! {
                    let _ = self.#ident(#(#param_idents),*);
                    Ok(golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(vec![]))
                },
            };

            let method_param_extraction = generate_method_param_extraction(
                &param_idents,
                &agent_type_name,
                method_name_str.as_str(),
                post_method_param_extraction_logic,
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
    post_method_param_extraction_logic: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let input_param_index_init = quote! {
      let mut input_param_index = 0;
    };

    let extraction: Vec<proc_macro2::TokenStream> = param_idents.iter().enumerate().map(|(original_method_param_idx, ident)| {
        let ident_result = format_ident!("{}_result", ident);
        quote! {
           let #ident_result = match &input {
               golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(values) => {
                    let enriched_schema = golem_rust::agentic::get_method_parameter_type(
                        &golem_rust::agentic::AgentTypeName(#agent_type_name.to_string()),
                        #method_name,
                        #original_method_param_idx,
                    ).ok_or_else(|| {
                        golem_rust::agentic::custom_error(format!(
                            "Internal Error: Parameter schema not found for agent: {}, method: {}, parameter index: {}",
                            #agent_type_name, #method_name, #original_method_param_idx
                        ))
                    })?;

                    match enriched_schema {
                        golem_rust::agentic::EnrichedElementSchema::AutoInject(auto_injected_schema) => {
                            match auto_injected_schema {
                                golem_rust::agentic::AutoInjectedParamType::Principal => {
                                    golem_rust::agentic::Schema::from_structured_value(golem_rust::agentic::StructuredValue::AutoInjected(golem_rust::agentic::AutoInjectedValue::Principal(principal.clone())), golem_rust::agentic::StructuredSchema::AutoInject(golem_rust::agentic::AutoInjectedParamType::Principal)).map_err(|e| {
                                        golem_rust::agentic::invalid_input_error(format!("Failed parsing arg {} for method {}: {}", #original_method_param_idx, #method_name, e))
                                    })
                                }
                            }
                        }

                        golem_rust::agentic::EnrichedElementSchema::ElementSchema(element_schema) => {
                            let value = values.get(input_param_index);

                            let element_value_result = match value {
                                Some(v) => Ok(v.clone()),
                                None => Err(golem_rust::agentic::invalid_input_error(format!("Missing arguments in method {}", #method_name))),
                            };

                            let element_value = element_value_result?;

                            // only increment the input_param_index for non auto-injected parameters
                            input_param_index += 1;

                            golem_rust::agentic::Schema::from_structured_value(golem_rust::agentic::StructuredValue::Default(element_value), golem_rust::agentic::StructuredSchema::Default(element_schema)).map_err(|e| {
                                golem_rust::agentic::invalid_input_error(format!("Failed parsing arg {} for method {}: {}", #original_method_param_idx, #method_name, e))
                            })
                        }
                    }
                },
              golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(elements) => {
                   let deserialized_value = golem_rust::agentic::Schema::from_structured_value(golem_rust::agentic::StructuredValue::Multimodal(elements.clone()), golem_rust::agentic::StructuredSchema::Multimodal(vec![])).map_err(|e| {
                   golem_rust::agentic::invalid_input_error(format!("Failed parsing arg {} for method {}: {}", #original_method_param_idx, #method_name, e))
                 })?;
                   Ok(deserialized_value)

              }
          };

          let #ident = #ident_result?;
        }
    }).collect();

    quote! {
        #input_param_index_init
        #(#extraction)*
        #post_method_param_extraction_logic
    }
}

fn generate_base_agent_impl(
    impl_block: &syn::ItemImpl,
    match_arms: &[proc_macro2::TokenStream],
    trait_name_str: &str,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
    has_custom_snapshot: bool,
) -> proc_macro2::TokenStream {
    let self_ty = &impl_block.self_ty;

    let snapshot_impl = if has_custom_snapshot {
        quote! {
            async fn load_snapshot_base(&mut self, bytes: Vec<u8>) -> Result<(), String> {
                self.load_snapshot(bytes).await
            }

            async fn save_snapshot_base(&self) -> Result<golem_rust::agentic::SnapshotData, String> {
                let data = self.save_snapshot().await?;
                Ok(golem_rust::agentic::SnapshotData {
                    data,
                    mime_type: "application/octet-stream".to_string(),
                })
            }
        }
    } else {
        quote! {
            async fn load_snapshot_base(&mut self, bytes: Vec<u8>) -> Result<(), String> {
                use golem_rust::agentic::snapshot_auto::SnapshotLoadFallback;
                let mut helper = golem_rust::agentic::snapshot_auto::LoadHelper(self);
                helper.snapshot_load(&bytes)
            }

            async fn save_snapshot_base(&self) -> Result<golem_rust::agentic::SnapshotData, String> {
                use golem_rust::agentic::snapshot_auto::SnapshotSaveFallback;
                let helper = golem_rust::agentic::snapshot_auto::SaveHelper(self);
                helper.snapshot_save()
            }
        }
    };

    quote! {
        #[golem_rust::async_trait::async_trait(?Send)]
        impl #impl_generics golem_rust::agentic::BaseAgent for #self_ty #ty_generics #where_clause {
            fn get_agent_id(&self) -> String {
                golem_rust::agentic::get_agent_id().agent_id
            }

            async fn invoke(&mut self, method_name: String, input: golem_rust::golem_agentic::golem::agent::common::DataValue, principal: golem_rust::golem_agentic::golem::agent::common::Principal)
                -> Result<golem_rust::golem_agentic::golem::agent::common::DataValue, golem_rust::golem_agentic::golem::agent::common::AgentError> {
                match method_name.as_str() {
                    #(#match_arms,)*
                    _ => Err(golem_rust::agentic::invalid_method_error(method_name)),
                }
            }

            fn get_definition(&self)
                -> golem_rust::golem_agentic::golem::agent::common::AgentType {
                golem_rust::agentic::get_agent_type_by_name(&golem_rust::agentic::AgentTypeName(#trait_name_str.to_string()))
                    .expect("Agent definition not found")
            }

            #snapshot_impl
        }
    }
}

fn generate_constructor_extraction(
    ctor_params: &[(syn::Ident, syn::PatType)],
    agent_type_name: &str,
    call_back: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    // this will be incremented as soon as we index into the `params`
    // tuple to extract each parameter into the `initiate` function
    let input_param_index_init = quote! {
      let mut input_param_index = 0;
    };

    let extraction: Vec<proc_macro2::TokenStream> = ctor_params.iter().enumerate().map(|(constructor_param_index, (ident, pat_type))| {
        if has_autoinject_attribute(pat_type) {
            let ty = &pat_type.ty;
            quote! {
                let #ident: #ty = <#ty as ::golem_rust::agentic::AutoInjectable>::autoinject()
                .map_err(|err| golem_rust::agentic::internal_error(format!("Failed loading config of type {}: {}",  err, stringify!(#ty))))?;
            }
        } else {
            let ident_result = format_ident!("{}_result", ident);
            quote! {
                // params is the input to `initiate` function
                let #ident_result = match &params {
                    golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(values) => {
                        let enriched_schema = golem_rust::agentic::get_constructor_parameter_type(
                            &golem_rust::agentic::AgentTypeName(#agent_type_name.to_string()),
                            #constructor_param_index,
                        ).ok_or_else(|| {
                            golem_rust::agentic::internal_error(format!(
                                "Constructor parameter schema not found for agent: {}, parameter index: {}",
                                #agent_type_name, #constructor_param_index
                            ))
                        })?;

                        match enriched_schema {
                            golem_rust::agentic::EnrichedElementSchema::AutoInject(auto_injected_schema) => {
                                match auto_injected_schema {
                                    golem_rust::agentic::AutoInjectedParamType::Principal => {
                                        golem_rust::agentic::Schema::from_structured_value(golem_rust::agentic::StructuredValue::AutoInjected(golem_rust::agentic::AutoInjectedValue::Principal(principal.clone())), golem_rust::agentic::StructuredSchema::AutoInject(golem_rust::agentic::AutoInjectedParamType::Principal)).map_err(|e| {
                                            golem_rust::agentic::invalid_input_error(format!("Failed parsing constructor arg {}: {}", #constructor_param_index, e))
                                        })
                                    }
                                }
                            }

                            golem_rust::agentic::EnrichedElementSchema::ElementSchema(element_schema) => {
                                let element_value_result = match values.get(input_param_index) {
                                    Some(v) => Ok(v.clone()),
                                    None => Err(golem_rust::agentic::invalid_input_error(format!("Missing constructor arguments for agent {}", #agent_type_name))),
                                };

                                let element_value = element_value_result?;

                                // only increment the input_param_index for non auto injected parameters
                                input_param_index += 1;

                                golem_rust::agentic::Schema::from_structured_value(golem_rust::agentic::StructuredValue::Default(element_value), golem_rust::agentic::StructuredSchema::Default(element_schema)).map_err(|e| {
                                    golem_rust::agentic::invalid_input_error(format!("Failed parsing constructor arg {}: {}", #constructor_param_index, e))
                                })
                            }
                        }
                    },
                    golem_rust::golem_agentic::golem::agent::common::DataValue::Multimodal(_) => {
                        // TODO; support multimodal and add call back logic since the parameter names differ for multimodal
                        Err(golem_rust::agentic::internal_error("Multimodal input not supported currently"))
                    }
                };

                let #ident = #ident_result?;
            }
        }
    }).collect::<Vec<_>>();

    quote! {
        #input_param_index_init
        #(#extraction)*
        #call_back
    }
}

fn generate_initiator_impl(
    initiator_ident: &syn::Ident,
    constructor_param_extraction: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        struct #initiator_ident;

        #[golem_rust::async_trait::async_trait(?Send)]
        impl golem_rust::agentic::AgentInitiator for #initiator_ident {
            async fn initiate(&self, params: golem_rust::golem_agentic::golem::agent::common::DataValue, principal: golem_rust::golem_agentic::golem::agent::common::Principal)
                -> Result<(), golem_rust::golem_agentic::golem::agent::common::AgentError> {
                #constructor_param_extraction
            }
        }
    }
}

fn generate_register_initiator_fn(
    self_ty: &syn::Type,
    agent_trait_ident: &syn::Ident,
    initiator_ident: &syn::Ident,
) -> proc_macro2::TokenStream {
    let agent_impl_type_trimmed = trim_type_parameter(self_ty);
    let agent_impl_type_trimmed_ident = format_ident!("{}", agent_impl_type_trimmed);
    let agent_trait_name = agent_trait_ident.to_string();

    let register_initiator_fn_name = format_ident!(
        "__register_agent_initiator_{}",
        agent_trait_ident.to_string().to_lowercase()
    );

    // ctor_parse! instead of #[ctor] to avoid dependency on ctor crate at user side
    // This is one level of indirection to ensure the usage of ctor that is re-exported by golem_rust
    // When registering the initiator, we also register the agent type via the AgentTypeRegistrar
    quote! {
        ::golem_rust::ctor::__support::ctor_parse!(
            #[ctor] fn #register_initiator_fn_name() {
                #agent_impl_type_trimmed_ident::__register_agent_type();

                golem_rust::agentic::register_agent_initiator(
                    &#agent_trait_name,
                    std::sync::Arc::new(#initiator_ident)
                );
            }
        );
    }
}
