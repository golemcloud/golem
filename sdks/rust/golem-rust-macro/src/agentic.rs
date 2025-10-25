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
use proc_macro2::Ident;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, ItemTrait};

pub fn agent_definition_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let item_trait = syn::parse_macro_input!(item as syn::ItemTrait);

    let agent_type = get_agent_type(&item_trait);

    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_name = get_register_function_ident(&item_trait);

    let register_fn = quote! {
        #[::ctor::ctor]
        fn #register_fn_name() {
            golem_rust::agentic::register_agent_type(
               #trait_name_str.to_string(),
               #agent_type
            );
        }
    };

    let result = quote! {
        #item_trait
        #register_fn

    };

    result.into()
}

pub fn agent_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    item // TODO: implement agent implementation processing
}

pub fn derive_agent_arg(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let struct_name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(named_fields) => &named_fields.named,
            _ => panic!("AgentArg can only be derived for structs with named fields"),
        },
        _ => panic!("AgentArg can only be derived for structs"),
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
                golem_rust::agentic::AgentArg::to_value(&self.#f)
            }
        })
        .collect();

    let wit_type_fields: Vec<_> = field_idents_vec.iter().zip(field_types.iter()).map(|(ident, ty)| {
        let name = ident.to_string();
        quote! {
            golem_wasm::analysis::NameTypePair {
                name: #name.to_string(),
                typ: golem_wasm::analysis::AnalysedType::from(<#ty as golem_agentic::ToWitType>::get_wit_type()),
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
                let #ident = golem_rust::agentic::FromValue::from_value(values[#idx].clone())
                    .map_err(|_| format!("Failed to parse field '{}'", #field_name))?;
            }
        })
        .collect();

    let field_count = field_idents_vec.len();

    let expanded = quote! {
     impl golem_agentic::ToWitType for #struct_name {
         fn get_wit_type() -> golem_wasm::WitType {
             let analysed_type = golem_wasm::analysis::analysed_type::record(vec![
                 #(#wit_type_fields),*
             ]);
             golem_wasm::WitType::from(analysed_type)
         }
     }

     impl golem_agentic::ToValue for #struct_name {
         fn to_value(&self) -> golem_wasm::Value {
            golem_wasm::Value::Record(vec![
                 #(#to_value_fields),*
             ])
         }
     }

     impl golem_agentic::FromWitValue for #struct_name {
         fn from_wit_value(value: golem_wasm::WitValue) -> Result<Self, String> {
             let value = golem_wasm::Value::from(value);
             match value {
                 golem_wasm::Value::Record(values) => {
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

fn get_register_function_ident(item_trait: &ItemTrait) -> Ident {
    let trait_name = item_trait.ident.clone();

    let trait_name_str = trait_name.to_string();

    let register_fn_suffix = &trait_name_str.to_lowercase();

    format_ident!("register_agent_type_{}", register_fn_suffix)
}

fn get_agent_type(item_trait: &syn::ItemTrait) -> proc_macro2::TokenStream {
    let type_name = item_trait.ident.to_string();

    let methods = item_trait.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(trait_fn) = item {
            let name = &trait_fn.sig.ident;
            let method_name = &name.to_string();

            let mut description = String::new();

            for attr in &trait_fn.attrs {
                if attr.path().is_ident("description") {
                    let mut found = None;
                    attr.parse_nested_meta(|meta| {
                        if meta.path.is_ident("description") {
                            let lit: syn::LitStr = meta.value()?.parse()?;
                            found = Some(lit.value());
                            Ok(())
                        } else {
                            Err(meta.error("expected `description = \"...\"`"))
                        }
                    })
                    .ok();
                    if let Some(val) = found {
                        description = val;
                    }
                }
            }


            let mut parameter_types = vec![]; // This is WIT type for now, but needs to support structured text type
            let mut result_type = vec![];

            if let syn::TraitItem::Fn(trait_fn) = item {
                for input in &trait_fn.sig.inputs {
                    if let syn::FnArg::Typed(pat_type) = input {
                        let ty = &pat_type.ty;
                        parameter_types.push(quote! {
                            ("foo".to_string(), golem_rust::golem_agentic::golem::agent::common::ElementSchema::ComponentModel(<#ty as ::golem_rust::agentic::AgentArg>::get_wit_type()))
                        });
                    }
                }

                // Handle return type
                match &trait_fn.sig.output {
                    syn::ReturnType::Default => (),
                    syn::ReturnType::Type(_, ty) => {
                        result_type.push(quote! {
                            ("return-value".to_string(),   golem_rust::golem_agentic::golem::agent::common::ElementSchema::ComponentModel(<#ty as ::golem_rust::agentic::AgentArg>::get_wit_type()))
                        });
                    }
                };
            }

            let input_parameters = parameter_types;
            let output_parameters = result_type;


            Some(quote! {
                ::golem_rust::golem_agentic::golem::agent::common::AgentMethod {
                    name: #method_name.to_string(),
                    description: #description.to_string(),
                    prompt_hint: None,
                    input_schema: ::golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#input_parameters),*]),
                    output_schema: ::golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(vec![#(#output_parameters),*]),
                }
            })
        } else {
            None
        }
    });

    let agent_constructor = quote! { golem_rust::golem_agentic::golem::agent::common::AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: ::golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(
                vec![]
            ),
        }
    };

    quote! {
        ::golem_rust::golem_agentic::golem::agent::common::AgentType {
            type_name: #type_name.to_string(),
            description: "".to_string(),
            methods: vec![#(#methods),*],
            dependencies: vec![],
            constructor: #agent_constructor,
        }
    }
}
