// Copyright 2024-2026 Golem Cloud
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
use syn::{DeriveInput, Fields, Ident, parse_macro_input};

pub fn derive_config_schema(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    // Only structs with named fields are supported
    let fields = match &ast.data {
        syn::Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &ast.ident,
                    "ConfigSchema only supports structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &ast.ident,
                "ConfigSchema can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let struct_name = &ast.ident;
    let rpc_struct_name = Ident::new(&format!("{struct_name}Rpc"), struct_name.span());

    let config_schema_impl = generate_config_schema_impl(
        struct_name,
        &rpc_struct_name,
        golem_rust_crate_ident,
        fields,
    );
    let rpc_struct = generate_rpc_struct(&rpc_struct_name, golem_rust_crate_ident, fields);
    let as_rpc_impl =
        generate_into_rpc_config_param_impl(&rpc_struct_name, golem_rust_crate_ident, fields);

    let expanded = quote::quote! {
        #rpc_struct
        #as_rpc_impl
        #config_schema_impl
    };

    expanded.into()
}

fn generate_config_schema_impl(
    struct_name: &Ident,
    rpc_struct_name: &Ident,
    golem_rust_crate_ident: &Ident,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> proc_macro2::TokenStream {
    let mut append_config_entries = Vec::new();
    let mut load_entries = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let field_name_str = field_ident.to_string();
        let field_ty = &field.ty;

        if has_nested_attr(field) {
            append_config_entries.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let mut nested_entries = <#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::describe_config(&field_path);
                    config_entries.append(&mut nested_entries);
                }
            });
            load_entries.push(quote::quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    <#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::load(&field_path)
                }
            });
        } else if has_secret_attr(field) {
            append_config_entries.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let config_entry = #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigDeclaration {
                        source: #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigSource::Secret,
                        path: field_path,
                        value_type: <<#field_ty as #golem_rust_crate_ident::agentic::InnerTypeHelper>::Type as #golem_rust_crate_ident::value_and_type::IntoValue>::get_type(),
                    };
                    config_entries.push(config_entry);
                }
            });
            load_entries.push(quote::quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    #golem_rust_crate_ident::agentic::Secret::new(field_path)
                }
            });
        } else {
            append_config_entries.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let config_entry = #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigDeclaration {
                        source: #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigSource::Local,
                        path: field_path,
                        value_type: <#field_ty as #golem_rust_crate_ident::value_and_type::IntoValue>::get_type(),
                    };
                    config_entries.push(config_entry);
                }
            });
            load_entries.push(quote::quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let typ = <#field_ty as #golem_rust_crate_ident::value_and_type::IntoValue>::get_type();
                    let value = #golem_rust_crate_ident::golem_agentic::golem::agent::host::get_config_value(&field_path, &typ);
                    let value_and_type = golem_rust::golem_wasm::golem_core_1_5_x::types::ValueAndType { value, typ };
                    #golem_rust_crate_ident::value_and_type::FromValueAndType::from_value_and_type(value_and_type)
                        .expect("failed deserializing config value")
                }
            });
        }
    }

    quote::quote! {
        impl #golem_rust_crate_ident::agentic::ConfigSchema for #struct_name {
            type RpcType = #rpc_struct_name;

            fn describe_config(path: &[String]) -> Vec<#golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigDeclaration> {
                let mut config_entries = Vec::new();
                #(#append_config_entries)*
                config_entries
            }

            fn load(path: &[String]) -> Self {
                Self {
                    #(#load_entries),*
                }
            }
        }
    }
}

fn generate_rpc_struct(
    rpc_struct_name: &Ident,
    golem_rust_crate_ident: &Ident,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> proc_macro2::TokenStream {
    let mut rpc_fields = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let field_ty = &field.ty;

        if has_secret_attr(field) {
            continue; // secrets are omitted
        } else if has_nested_attr(field) {
            rpc_fields.push(quote::quote! {
                pub #field_ident: <#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::RpcType
            });
        } else {
            rpc_fields.push(quote::quote! {
                pub #field_ident: ::std::option::Option<#field_ty>
            });
        }
    }

    quote::quote! {
        #[derive(Default)]
        pub struct #rpc_struct_name {
            #(#rpc_fields),*
        }
    }
}

fn generate_into_rpc_config_param_impl(
    rpc_struct_name: &Ident,
    golem_rust_crate_ident: &Ident,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> proc_macro2::TokenStream {
    let mut field_processes = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let field_name_str = field_ident.to_string();
        let field_ty = &field.ty;

        if has_secret_attr(field) {
            continue; // secrets omitted
        } else if has_nested_attr(field) {
            field_processes.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());

                    let mut nested_fields_as_params = #golem_rust_crate_ident::agentic::IntoRpcConfigParam::into_rpc_param(self.#field_ident, &field_path);
                    result.append(&mut nested_fields_as_params);
                }
            });
        } else {
            field_processes.push(quote::quote! {
                {
                    if let Some(value) = self.#field_ident.clone() {
                        let mut field_path = path.to_vec();
                        field_path.push(#field_name_str.to_string());

                        let value = #golem_rust_crate_ident::value_and_type::IntoValue::into_value(value);
                        let typ = <#field_ty as #golem_rust_crate_ident::value_and_type::IntoValue>::get_type();
                        let value_and_type = golem_rust::golem_wasm::golem_core_1_5_x::types::ValueAndType { value, typ };
                        result.push(#golem_rust_crate_ident::golem_agentic::golem::agent::common::TypedAgentConfigValue {
                            path: field_path,
                            value: value_and_type,
                        });
                    }
                }
            });
        }
    }

    quote::quote! {
        impl #golem_rust_crate_ident::agentic::IntoRpcConfigParam for #rpc_struct_name {
            fn into_rpc_param(self, path: &[String]) -> Vec<#golem_rust_crate_ident::golem_agentic::golem::agent::common::TypedAgentConfigValue> {
                let mut result = Vec::new();
                #(#field_processes)*
                result
            }
        }
    }
}

fn has_secret_attr(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| {
        attr.path().is_ident("config_schema")
            && attr
                .parse_args::<Ident>()
                .map(|i| i == "secret")
                .unwrap_or(false)
    })
}

fn has_nested_attr(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| {
        attr.path().is_ident("config_schema")
            && attr
                .parse_args::<Ident>()
                .map(|i| i == "nested")
                .unwrap_or(false)
    })
}
