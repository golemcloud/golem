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

//! `#[tool_implementation]` entry point.
//!
//! This injects the hidden `tool_implementation_annotation` item that satisfies
//! the required trait item emitted by `#[tool_definition]` (so an implementation
//! that forgets the attribute is a compile error), and emits the `#[ctor]` that
//! registers the tool's metadata and hidden trait-defined invoker at startup.

use proc_macro::TokenStream;
use quote::{ToTokens, format_ident, quote};
use std::hash::{Hash, Hasher};
use syn::{ImplItem, ItemImpl};

pub fn tool_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_impl = syn::parse_macro_input!(item as ItemImpl);

    let trait_path = match &item_impl.trait_ {
        Some((_, path, _)) => path.clone(),
        None => {
            return syn::Error::new_spanned(
                &item_impl.self_ty,
                "#[tool_implementation] must be applied to a trait implementation \
                 (`impl Trait for Type`)",
            )
            .to_compile_error()
            .into();
        }
    };
    let self_ty = item_impl.self_ty.clone();
    let trait_ident = &trait_path.segments.last().unwrap().ident;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    self_ty.to_token_stream().to_string().hash(&mut hasher);
    trait_path.to_token_stream().to_string().hash(&mut hasher);
    let register_fn_name = format_ident!(
        "__register_tool_{}_{:016x}",
        trait_ident.to_string().to_lowercase(),
        hasher.finish()
    );

    let annotation: ImplItem = syn::parse_quote! {
        #[doc(hidden)]
        fn tool_implementation_annotation() where Self: Sized {}
    };
    item_impl.items.push(annotation);

    quote! {
        #item_impl

        ::golem_rust::ctor::__support::ctor_parse!(
            #[ctor] fn #register_fn_name() {
                golem_rust::agentic::register_tool_invoker(
                    <#self_ty as #trait_path>::__tool_descriptor(),
                    <#self_ty as #trait_path>::__tool_invoke,
                );
            }
        );
    }
    .into()
}
