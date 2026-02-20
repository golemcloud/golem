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
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Ident};

pub fn derive_config_schema(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let struct_name = &ast.ident;

    let fields = match &ast.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    struct_name,
                    "ConfigSchema only supports structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                struct_name,
                "ConfigSchema can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut assertions = Vec::with_capacity(fields.len());
    let mut describe_config_entries = Vec::with_capacity(fields.len());
    let mut load_entries = Vec::with_capacity(fields.len());

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let field_name = field_ident.to_string();
        let field_ty = &field.ty;

        {
            let invalid_type_error = format!("Invalid field `{field_name}`: Only component-model compatible types can be used in configuration");
            assertions.push(
                quote! {
                    const _: () = {
                        assert!(
                            <#field_ty as #golem_rust_crate_ident::agentic::ConfigField>::Inner::IS_COMPONENT_MODEL_SCHEMA,
                            #invalid_type_error
                        );
                    };
                }
            );
        }

        describe_config_entries.push(
            quote! {
                #golem_rust_crate_ident::agentic::ConfigEntry {
                    key: #field_name.to_string(),
                    shared: <#field_ty as #golem_rust_crate_ident::agentic::ConfigField>::IS_SHARED,
                    schema:
                        match <<#field_ty as #golem_rust_crate_ident::agentic::ConfigField>::Inner
                            as #golem_rust_crate_ident::agentic::Schema>::get_type()
                            .get_element_schema()
                            // safe because IS_COMPONENT_MODEL_SCHEMA is true
                            .unwrap() {
                                #golem_rust_crate_ident::golem_agentic::golem::agent::common::ElementSchema::ComponentModel(inner) => inner,
                                // safe because IS_COMPONENT_MODEL_SCHEMA is true
                                other => panic!(
                                    "ConfigSchema fields must use ElementSchema::ComponentModel, got {:?}",
                                    other
                                ),
                            }
                }
            }
        );

        load_entries.push(
            quote! {
                #field_ident: <#field_ty as #golem_rust_crate_ident::agentic::ConfigField>::load_from_path(&[#field_name.to_string()])?
            }
        );
    }

    let expanded = quote! {
        impl #golem_rust_crate_ident::agentic::ConfigSchema for #struct_name {
            fn describe_config() -> Vec<#golem_rust_crate_ident::agentic::ConfigEntry> {
                #(#assertions)*
                vec![
                    #(#describe_config_entries),*
                ]
            }
            fn load() -> Result<Self, String> {
                Ok(Self {
                    #(#load_entries),*
                })
            }
        }
    };

    expanded.into()
}
