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
use syn::{parse_macro_input, Data, DeriveInput, Fields};

pub fn derive_multimodal(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let enum_name = &input.ident;

    let data_enum = match input.data {
        Data::Enum(data_enum) => data_enum,
        _ => {
            return syn::Error::new_spanned(input.ident, "Multimodal derive only supports enums")
                .to_compile_error()
                .into()
        }
    };

    let mut get_type_pairs = Vec::new();
    let mut serialize_match_arms = Vec::new();
    let mut get_name_match_arms = Vec::new();
    let mut deserialize_match_arms = Vec::new();
    let mut wit_value_match_arms = Vec::new();

    for variant in data_enum.variants.iter() {
        let variant_ident = &variant.ident;
        let variant_name = variant_ident.to_string();

        // Only support single-field tuple variants
        match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field_type = &fields.unnamed[0].ty;

                get_type_pairs.push(quote! {
                    (#variant_name.to_string(), <#field_type as golem_rust::agentic::Schema>::get_type())
                });

                serialize_match_arms.push(quote! {
                    #enum_name::#variant_ident(inner) => {
                        (#variant_name.to_string(), <#field_type as golem_rust::agentic::Schema>::to_element_value(inner)?)
                    }
                });

                wit_value_match_arms.push(quote! {
                    #enum_name::#variant_ident(inner) => {
                        <#field_type as golem_rust::agentic::Schema>::to_wit_value(inner)
                    }
                });

                (get_name_match_arms).push(quote! {
                    #enum_name::#variant_ident(_) => #variant_name.to_string()
                });

                deserialize_match_arms.push(quote! {
                    #variant_name => {
                        let val = <#field_type as golem_rust::agentic::Schema>::from_element_value(elem.clone(), <#field_type as golem_rust::agentic::Schema>::get_type())?;
                        Ok(#enum_name::#variant_ident(val))
                    }
                });
            }
            _ => {
                return syn::Error::new_spanned(
                    variant,
                    "Multimodal derive only supports single-field tuple variants",
                )
                .to_compile_error()
                .into()
            }
        }
    }

    let expanded = quote! {
        impl golem_rust::agentic::MultimodalSchema for #enum_name {
            fn get_multimodal_schema() -> Vec<(String, golem_rust::golem_agentic::golem::agent::common::ElementSchema)> {
                vec![
                    #(#get_type_pairs),*
                ]
            }

            fn to_element_value(self) -> Result<(String, golem_rust::golem_agentic::golem::agent::common::ElementValue), String> {
                let result = match self {
                    #(#serialize_match_arms),*
                };
                Ok(result)
            }

            fn get_name(&self) -> String {
                match self {
                    #(#get_name_match_arms),*
                }
            }

            fn to_wit_value(self) -> Result<golem_rust::wasm_rpc::WitValue, String> {
                match self {
                    #(#wit_value_match_arms),*
                }
            }

            fn from_element_value(elem: (String, golem_rust::golem_agentic::golem::agent::common::ElementValue)) -> Result<Self, String> {
                let (name, elem) = elem;

                 match name.as_str() {
                    #(#deserialize_match_arms),*,
                    _ => return Err(format!("Unknown modality: {}", name))
                 }
            }
        }
    };

    TokenStream::from(expanded)
}
