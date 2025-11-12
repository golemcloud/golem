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
    let mut deserialize_match_arms = Vec::new();

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
                        vec![(#variant_name.to_string(), <#field_type as golem_rust::agentic::Schema>::to_element_value(inner.clone())?)]
                    }
                });

                deserialize_match_arms.push(quote! {
                    #variant_name => {
                        let val = <#field_type as golem_rust::agentic::Schema>::from_element_value(elem.clone(), <#field_type as golem_rust::agentic::Schema>::get_type())?;
                        #enum_name::#variant_ident(val)
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

            fn serialize_multimodal(input: golem_rust::agentic::MultiModal<Self>) -> Result<Vec<(String, golem_rust::golem_agentic::golem::agent::common::ElementValue)>, String> {
                let mut items = Vec::new();
                for instance in input.items {
                    let mut serialized = match instance {
                        #(#serialize_match_arms),*
                    };
                    items.append(&mut serialized);
                }
                Ok(items)
            }

            fn deserialize_multimodal(values: Vec<(String, golem_rust::golem_agentic::golem::agent::common::ElementValue)>) -> Result<golem_rust::agentic::MultiModal<Self>, String> {
                let mut items = Vec::new();
                for (name, elem) in values {
                    let variant = match name.as_str() {
                        #(#deserialize_match_arms),*,
                        _ => return Err(format!("Unknown modality: {}", name))
                    };
                    items.push(variant);
                }
                Ok(golem_rust::agentic::MultiModal::new(items))
            }
        }
    };

    TokenStream::from(expanded)
}
