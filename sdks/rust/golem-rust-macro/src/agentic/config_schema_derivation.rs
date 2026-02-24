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

    let mut append_config_entries = Vec::with_capacity(fields.len());
    let mut load_entries = Vec::with_capacity(fields.len());

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let field_name = field_ident.to_string();
        let field_ty = &field.ty;

        append_config_entries.push(
            quote! {
                {
                    let mut collected_entries = <#field_ty as #golem_rust_crate_ident::agentic::ConfigField>::collect_entries(&[#field_name.to_string()]);
                    config_entries.append(&mut collected_entries);
                }
            }
        );

        load_entries.push(
            quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name.to_string());
                    <#field_ty as #golem_rust_crate_ident::agentic::ConfigField>::load(&field_path)?
                }
            }
        );
    }

    let config_schema_impl = quote! {
        impl #golem_rust_crate_ident::agentic::ConfigSchema for #struct_name {
            fn describe_config() -> Vec<#golem_rust_crate_ident::agentic::ConfigEntry> {
                let mut config_entries = Vec::new();
                #(#append_config_entries)*
                config_entries
            }
            fn load(path: &[String]) -> Result<Self, String> {
                Ok(Self {
                    #(#load_entries),*
                })
            }
        }
    };

    let config_field_impl = derive_nested_config_field(&ast, golem_rust_crate_ident);

    quote! {
        #config_schema_impl
        #config_field_impl
    }
    .into()
}

fn derive_nested_config_field(ast: &DeriveInput, golem_rust_crate_ident: &Ident) -> proc_macro2::TokenStream {
    let ident = &ast.ident;
    let generics = &ast.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics #golem_rust_crate_ident::agentic::ConfigField
            for #ident #ty_generics
            #where_clause
        {
            const IS_SHARED: bool = false;

            fn collect_entries(
                path_prefix: &[String]
            ) -> Vec<#golem_rust_crate_ident::agentic::ConfigEntry> {
                let mut config_entries = <Self as #golem_rust_crate_ident::agentic::ConfigSchema>::describe_config();
                for config_entry in config_entries.iter_mut() {
                    let mut key = path_prefix.to_vec();
                    key.append(&mut config_entry.key);
                    config_entry.key = key;
                };
                config_entries
            }

            fn load(path: &[String]) -> Result<Self, String> {
                <Self as #golem_rust_crate_ident::agentic::ConfigSchema>::load(path)
            }
        }
    }
}
