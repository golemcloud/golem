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
use proc_macro2::Span;
use proc_macro_crate::{crate_name, FoundCrate};
use syn::DeriveInput;

use crate::transaction::golem_operation_impl;

mod agentic;
pub(crate) mod recursion;
mod transaction;
mod value;

#[proc_macro_derive(IntoValue, attributes(flatten_value, unit_case))]
pub fn derive_into_value(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).expect("derive input");
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    value::derive_into_value(&ast, &golem_rust_crate_ident)
}

#[proc_macro_derive(FromValueAndType, attributes(flatten_value, unit_case))]
pub fn derive_from_value_and_type(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).expect("derive input");
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    value::derive_from_value_and_type(&ast, &golem_rust_crate_ident)
}

#[proc_macro_derive(MultimodalSchema)]
pub fn derive_multimodal(input: TokenStream) -> TokenStream {
    agentic::derive_multimodal(input)
}

#[proc_macro_derive(Schema)]
pub fn derive_schema(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_schema(input, &golem_rust_crate_ident)
}

#[proc_macro_derive(ConfigSchema)]
pub fn derive_config_schema(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_config_schema(input, &golem_rust_crate_ident)
}

#[proc_macro_derive(AllowedLanguages, attributes(code))]
pub fn derive_allowed_languages(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_allowed_languages(input, &golem_rust_crate_ident)
}

#[proc_macro_derive(AllowedMimeTypes, attributes(mime_type))]
pub fn derive_allowed_mimetypes(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_allowed_mime_types(input, &golem_rust_crate_ident)
}

#[proc_macro_attribute]
pub fn description(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn prompt(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn endpoint(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Defines a function as an `Operation` that can be used in transactions
#[proc_macro_attribute]
pub fn golem_operation(attr: TokenStream, item: TokenStream) -> TokenStream {
    golem_operation_impl(attr, item)
}

#[proc_macro_attribute]
pub fn agent_definition(attr: TokenStream, item: TokenStream) -> TokenStream {
    agentic::agent_definition_impl(attr, item)
}

#[proc_macro_attribute]
pub fn agent_implementation(attr: TokenStream, item: TokenStream) -> TokenStream {
    agentic::agent_implementation_impl(attr, item)
}

// get the identifier of golem_rust crate to use for referencing the `golem-rust` crate
// within the macros. This handles the case where the crate is renamed in Cargo.toml
// or when the macro is used within the `golem-rust` crate itself.
fn get_golem_rust_crate_ident() -> syn::Ident {
    match crate_name("golem-rust") {
        Ok(FoundCrate::Itself) => syn::Ident::new("crate", Span::call_site()),
        Ok(FoundCrate::Name(name)) => syn::Ident::new(&name, Span::call_site()),
        Err(_) => syn::Ident::new("golem_rust", Span::call_site()),
    }
}
