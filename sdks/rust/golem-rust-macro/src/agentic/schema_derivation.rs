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

use crate::agentic::{generate_from_generic, generate_to_generic, is_recursive};
use crate::value;
use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

pub fn derive_schema(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let self_ident = &ast.ident;

    if is_recursive(&ast) {
        let to_generic_impl = generate_to_generic(&ast, self_ident, golem_rust_crate_ident);
        let from_generic_impl = generate_from_generic(&ast, self_ident, golem_rust_crate_ident);

        let value_impl = quote! {
            impl #golem_rust_crate_ident::value_and_type::IntoValue for #self_ident {
                fn add_to_builder<B: #golem_rust_crate_ident::value_and_type::NodeBuilder>(self, builder: B) -> B::Result {
                    use #golem_rust_crate_ident::agentic::ToGenericData;

                    let mut graph = #golem_rust_crate_ident::agentic::GenericData { nodes: vec![], root: 0 };
                    let result_index = self.to_generic(&mut graph);
                    graph.root = result_index;
                    graph.add_to_builder(builder)
                }

                fn add_to_type_builder<B: #golem_rust_crate_ident::value_and_type::TypeNodeBuilder>(builder: B) -> B::Result {
                    #golem_rust_crate_ident::agentic::GenericData::add_to_type_builder(builder)
                }
            }

            impl #golem_rust_crate_ident::value_and_type::FromValueAndType for #self_ident {
                fn from_extractor<'a, 'b>(
                    extractor: &'a impl #golem_rust_crate_ident::value_and_type::WitValueExtractor<'a, 'b>,
                ) -> Result<Self, String> {
                    use #golem_rust_crate_ident::agentic::FromGenericData;

                    let graph = #golem_rust_crate_ident::agentic::GenericData::from_extractor(extractor)?;
                    Self::from_generic(&graph, graph.root)
                }
            }
        };

        quote! {
            #to_generic_impl
            #from_generic_impl
            #value_impl
        }
        .into()
    } else {
        let into_value_tokens: proc_macro2::TokenStream =
            value::derive_into_value(&ast, golem_rust_crate_ident).into();
        let from_value_tokens: proc_macro2::TokenStream =
            value::derive_from_value_and_type(&ast, golem_rust_crate_ident).into();

        quote! {
            #into_value_tokens
            #from_value_tokens
        }
        .into()
    }
}
