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

use crate::recursion::is_recursive;
use crate::value;
use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput};

pub fn derive_schema(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let is_recursive = is_recursive(&ast);

    if is_recursive {
        return syn::Error::new_spanned(
            &ast.ident,
            format!("Cannot derive `Schema` for recursive type `{}`\n\
            Recursive types are not supported by `Schema` yet\n\
            Help: Avoid direct recursion in this type (e.g. using index-based node lists) and then derive `Schema`", ast.ident
        )).to_compile_error().into();
    }

    let into_value_tokens: proc_macro2::TokenStream =
        value::derive_into_value(&ast, golem_rust_crate_ident).into();

    let from_value_tokens: proc_macro2::TokenStream =
        value::derive_from_value_and_type(&ast, golem_rust_crate_ident).into();

    let config_field_tokens = derive_component_model_config_leaf(&ast, golem_rust_crate_ident);

    quote! {
        #into_value_tokens
        #from_value_tokens
        #config_field_tokens
    }
    .into()
}

fn derive_component_model_config_leaf(
    ast: &DeriveInput,
    golem_rust_crate_ident: &Ident,
) -> proc_macro2::TokenStream {
    let ident = &ast.ident;
    let generics = &ast.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    match &ast.data {
        // ConfigFields impls for structs are handled by ConfigSchema derivation
        Data::Struct(_) => quote! {},
        _ => quote! {
            impl #impl_generics #golem_rust_crate_ident::agentic::ComponentModelConfigLeaf
                for #ident #ty_generics
                #where_clause
            { }
        },
    }
}
