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

pub fn derive_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let struct_name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(named_fields) => &named_fields.named,
            _ => panic!("Schema can only be derived for structs with named fields"),
        },
        _ => panic!("Schema can only be derived for structs"),
    };

    let field_idents_vec: Vec<proc_macro2::Ident> = fields
        .iter()
        .map(|f| f.ident.as_ref().unwrap().clone())
        .collect();

    let field_names: Vec<String> = field_idents_vec
        .iter()
        .map(|ident| ident.to_string())
        .collect();

    let field_types: Vec<_> = fields.iter().map(|f| &f.ty).collect();

    let to_value_fields: Vec<_> = field_idents_vec
        .iter()
        .map(|f| {
            quote! {
                golem_rust::agentic::Schema::to_value(self.#f)
            }
        })
        .collect();

    let wit_type_fields: Vec<_> = field_idents_vec.iter().zip(field_types.iter()).map(|(ident, ty)| {
        let name = ident.to_string();
        quote! {
            golem_rust::wasm_rpc::analysis::NameTypePair {
                name: #name.to_string(),
                typ: golem_rust::wasm_rpc::analysis::AnalysedType::from(<#ty as golem_rust::agentic::Schema>::get_wit_type()),
            }
        }
    }).collect();

    let from_value_fields: Vec<_> = field_idents_vec
        .iter()
        .enumerate()
        .map(|(i, ident)| {
            let field_name = &field_names[i];
            let idx = syn::Index::from(i);
            quote! {
                let #ident = golem_rust::agentic::Schema::from_value(values[#idx].clone())
                    .map_err(|_| format!("Failed to parse field '{}'", #field_name))?;
            }
        })
        .collect();

    let field_count = field_idents_vec.len();

    let expanded = quote! {
     impl golem_rust::wasm_rpc::IntoValue for #struct_name {
         fn into_value(self) -> golem_rust::wasm_rpc::Value {
            golem_rust::wasm_rpc::Value::Record(vec![
                 #(#to_value_fields),*
             ])
         }

         fn get_type() -> golem_rust::wasm_rpc::analysis::AnalysedType {
            golem_rust::wasm_rpc::analysis::analysed_type::record(vec![
                #(#wit_type_fields),*
            ])
        }
     }

     impl golem_rust::agentic::FromValue for #struct_name {
         fn from_value(value: golem_rust::wasm_rpc::Value) -> Result<Self, String> {
             match value {
                 golem_rust::wasm_rpc::Value::Record(values) => {
                     if values.len() != #field_count {
                         return Err(format!("Expected {} fields", #field_count));
                     }

                     #(#from_value_fields)*

                     Ok(#struct_name {
                         #(#field_idents_vec),*
                     })
                 }
                 _ => Err("Expected a record WitValue".to_string())
             }
         }
       }
    };

    TokenStream::from(expanded)
}
