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

// The proc-macro entry points must be excluded from the test build because
// test-r generates a non-macro entry point. Their absence makes production
// implementation paths appear unused only while compiling the test harness.
#![cfg_attr(test, allow(dead_code, unused_imports))]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;

use crate::transaction::golem_operation_impl;

mod agentic;
mod rpc_client_common;
mod tool;
mod transaction;

#[cfg(test)]
test_r::enable!();

#[cfg(not(test))]
#[proc_macro_derive(MultimodalSchema)]
pub fn derive_multimodal(input: TokenStream) -> TokenStream {
    agentic::derive_multimodal(input)
}

#[cfg(not(test))]
#[proc_macro_derive(ConfigSchema, attributes(config_schema))]
pub fn derive_config_schema(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_config_schema(input, &golem_rust_crate_ident)
}

#[cfg(not(test))]
#[proc_macro_derive(AllowedLanguages, attributes(code))]
pub fn derive_allowed_languages(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_allowed_languages(input, &golem_rust_crate_ident)
}

#[cfg(not(test))]
#[proc_macro_derive(AllowedMimeTypes, attributes(mime_type))]
pub fn derive_allowed_mimetypes(input: TokenStream) -> TokenStream {
    let golem_rust_crate_ident = get_golem_rust_crate_ident();

    agentic::derive_allowed_mime_types(input, &golem_rust_crate_ident)
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn description(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn prompt(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn endpoint(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn read_only(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Defines a function as an `Operation` that can be used in transactions
#[cfg(not(test))]
#[proc_macro_attribute]
pub fn golem_operation(attr: TokenStream, item: TokenStream) -> TokenStream {
    golem_operation_impl(attr, item)
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn agent_definition(attr: TokenStream, item: TokenStream) -> TokenStream {
    agentic::agent_definition_impl(attr, item)
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn agent_implementation(attr: TokenStream, item: TokenStream) -> TokenStream {
    agentic::agent_implementation_impl(attr, item)
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn tool_definition(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::tool_definition_impl(attr, item, &get_golem_rust_crate_ident())
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn tool_implementation(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::tool_implementation_impl(attr, item, &get_golem_rust_crate_ident())
}

#[doc(hidden)]
#[cfg(not(test))]
#[proc_macro_attribute]
pub fn native_tool_definition(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::native::native_tool_definition_impl(attr, item, &get_native_tool_crate_ident())
}

#[doc(hidden)]
#[cfg(not(test))]
#[proc_macro_attribute]
pub fn native_tool_implementation(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::native::native_tool_implementation_impl(attr, item, &get_native_tool_crate_ident())
}

fn get_native_tool_crate_ident() -> syn::Ident {
    match crate_name("golem-native-tool") {
        Ok(FoundCrate::Itself) => syn::Ident::new("crate", Span::call_site()),
        Ok(FoundCrate::Name(name)) => syn::Ident::new(&name, Span::call_site()),
        Err(_) => syn::Ident::new("golem_native_tool", Span::call_site()),
    }
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn tool_middleware(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::tool_middleware_impl(attr, item, &get_golem_rust_crate_ident())
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn universal_tool_middleware(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::universal_tool_middleware_impl(attr, item, &get_golem_rust_crate_ident())
}

#[cfg(not(test))]
#[proc_macro_derive(ToolError, attributes(tool_error, example))]
pub fn derive_tool_error(input: TokenStream) -> TokenStream {
    tool::derive_tool_error_impl(input, &get_tool_schema_crate_ident())
}

fn get_tool_schema_crate_ident() -> syn::Ident {
    if crate_name("golem-rust").is_ok() {
        get_golem_rust_crate_ident()
    } else {
        match crate_name("golem-native-tool") {
            Ok(FoundCrate::Itself) => syn::Ident::new("crate", Span::call_site()),
            Ok(FoundCrate::Name(name)) => syn::Ident::new(&name, Span::call_site()),
            Err(_) => syn::Ident::new("golem_rust", Span::call_site()),
        }
    }
}

#[doc(hidden)]
#[cfg(not(test))]
#[proc_macro]
pub fn __golem_emit_tool_middleware_leaf(input: TokenStream) -> TokenStream {
    match tool::middleware_surface::emit_tool_middleware_leaf(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

// Helper attributes consumed by `#[tool_definition]`. A correct use sits on a
// method inside a `#[tool_definition]` trait, where the outer macro strips it
// before it ever reaches the compiler. Registering them as real proc-macro
// attributes keeps them recognized in any position; if one is ever actually
// expanded it means it was misplaced (outside a tool trait, or ordered before
// `#[tool_definition]`), so it fails loudly instead of being silently ignored.
fn misplaced_tool_helper_attr(name: &str) -> TokenStream {
    syn::Error::new(
        Span::call_site(),
        format!("#[{name}] may only be used on methods inside a #[tool_definition] trait"),
    )
    .to_compile_error()
    .into()
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn arg(_attr: TokenStream, _item: TokenStream) -> TokenStream {
    misplaced_tool_helper_attr("arg")
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn command(_attr: TokenStream, _item: TokenStream) -> TokenStream {
    misplaced_tool_helper_attr("command")
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn constraint(_attr: TokenStream, _item: TokenStream) -> TokenStream {
    misplaced_tool_helper_attr("constraint")
}

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn result(_attr: TokenStream, _item: TokenStream) -> TokenStream {
    misplaced_tool_helper_attr("result")
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
