// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Derive macros for the Golem schema model.
//!
//! Two derive macros are provided:
//!
//! - [`IntoSchema`] — emits an implementation of
//!   `golem_common::schema::conversion::IntoSchema`. It registers the type into a
//!   [`SchemaBuilder`](golem_common::schema::SchemaBuilder), returns a
//!   [`SchemaType`](golem_common::schema::SchemaType) describing how the
//!   type should appear at the call site (always a
//!   [`SchemaType::Ref`](golem_common::schema::SchemaType::Ref) for nominal
//!   user types), and converts Rust values into matching
//!   [`SchemaValue`](golem_common::schema::SchemaValue)s through `to_value`.
//! - [`FromSchema`] — emits a `from_value` implementation that decodes a
//!   [`SchemaValue`](golem_common::schema::SchemaValue) produced against a
//!   compatible schema back into the original Rust value.
//!
//! The Rust-SDK default `type_id()` derivation normalizes a Rust path FQN
//! into the language-independent dotted form (`a::b::c::Xyz` →
//! `a.b.c.Xyz`). Generic instantiations append resolved type arguments
//! inside `<…>`, recursively normalized:
//! `my_crate::Container<my_crate::user::Profile>` becomes
//! `my_crate.Container<my_crate.user.Profile>`. `#[schema(named = "…")]`
//! overrides the base name; for a generic type the explicit name is also
//! auto-suffixed with the resolved arguments so each instantiation gets
//! its own definition.
//!
//! See the crate-level documentation of `golem_common::schema::conversion`
//! for the full attribute surface and recursion semantics.

extern crate proc_macro;

mod codegen;
mod expand;
mod parse;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(IntoSchema, attributes(schema))]
pub fn derive_into_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand::expand_into_schema(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(FromSchema, attributes(schema))]
pub fn derive_from_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand::expand_from_schema(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
