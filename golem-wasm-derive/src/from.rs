// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::{is_unit_case, parse_wit_field_attribute, parse_wit_type_attrs, WitField};
use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, Type};

pub fn derive_from_value(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).expect("derive input");
    let ident = &ast.ident;
    let wit_transparent = ast
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("wit_transparent"));
    let wit = parse_wit_type_attrs(&ast.attrs);

    let from_value = match ast.data {
        Data::Struct(data) => {
            let newtype_result = if data.fields.len() == 1 {
                let field = data.fields.iter().next().unwrap().clone();
                if field.ident.is_none() || wit_transparent {
                    // single field without an identifier, or explicit transparent flag => we consider this a newtype wrapper
                    let field_type = field.ty;

                    let from_value = match field.ident {
                        None => quote! {
                            let inner = <#field_type as golem_wasm::FromValue>::from_value(value)?;
                            Ok(Self(inner))
                        },
                        Some(field_name) => quote! {
                            let #field_name = <#field_type as golem_wasm::FromValue>::from_value(value)?;
                            Ok(Self { #field_name })
                        },
                    };

                    Some(from_value)
                } else {
                    None
                }
            } else {
                None
            };

            match newtype_result {
                Some(newtype_result) => newtype_result,
                None => record_or_tuple_from_value(&data.fields),
            }
        }
        Data::Enum(data) => {
            let is_simple_enum = data
                .variants
                .iter()
                .all(|variant| variant.fields.is_empty());

            if is_simple_enum && !wit.as_variant {
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;
                        quote! {
                            #idx => Ok(#ident::#case_ident)
                        }
                    })
                    .collect::<Vec<_>>();

                quote! {
                    match value {
                        golem_wasm::Value::Enum(idx) => match idx {
                            #(#case_branches),*,
                            _ => Err(format!("Invalid enum index: {}", idx)),
                        },
                        _ => Err(format!("Expected Enum value, got {:?}", value)),
                    }
                }
            } else if is_simple_enum && wit.as_variant {
                // as_variant: all-unit enum deserialized from WIT variant
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;
                        quote! {
                            #idx => Ok(#ident::#case_ident)
                        }
                    })
                    .collect::<Vec<_>>();

                quote! {
                    match value {
                        golem_wasm::Value::Variant { case_idx, case_value: _ } => match case_idx {
                            #(#case_branches),*,
                            _ => Err(format!("Invalid variant case index: {}", case_idx)),
                        },
                        _ => Err(format!("Expected Variant value, got {:?}", value)),
                    }
                }
            } else {
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;

                        let wit_fields = variant.fields
                            .iter()
                            .map(|field| {
                                field
                                    .attrs
                                    .iter()
                                    .find(|attr| attr.path().is_ident("wit_field"))
                                    .map(parse_wit_field_attribute)
                                    .unwrap_or_default()
                            })
                            .collect::<Vec<_>>();

                        if variant.fields.is_empty() {
                            quote! {
                                #idx => Ok(#ident::#case_ident)
                            }
                        } else if has_single_anonymous_field(&variant.fields) {
                            // separate inner type
                            if is_unit_case(variant) {
                                quote! {
                                    #idx => Ok(#ident::#case_ident(Default::default()))
                                }
                            } else {
                                let single_field = variant.fields.iter().next().unwrap();
                                let typ = &single_field.ty;
                                let wit_field = wit_fields.first().unwrap();
                                let from_value = apply_from_conversions(typ, wit_field, quote! { *inner });
                                quote! {
                                    #idx => {
                                        let inner = case_value.ok_or("Missing case value")?;
                                        let inner = #from_value;
                                        Ok(#ident::#case_ident(inner))
                                    }
                                }
                            }
                        } else if has_only_named_fields(&variant.fields) {
                            // record case

                            let mut wit_idx = 0usize;
                            let field_values = variant.fields.iter().enumerate().map(|(field_idx, field)| {
                                let field_ident = field.ident.as_ref().unwrap();
                                let field_ty = &field.ty;
                                let wit_field = &wit_fields[field_idx];
                                let field_from_value = if wit_field.skip || has_from_value_skip_attribute(&field.attrs) {
                                    quote! { Default::default() }
                                } else {
                                    let current_wit_idx = wit_idx;
                                    wit_idx += 1;
                                    apply_from_conversions(field_ty, wit_field, quote! { fields[#current_wit_idx].clone() })
                                };
                                quote! {
                                    #field_ident: #field_from_value
                                }
                            });

                            if is_unit_case(variant) {
                                quote! {
                                    #idx => Ok(#ident::#case_ident(Default::default()))
                                }
                            } else {
                                let expected_len = variant.fields.iter().enumerate().filter(|(field_idx, field)| {
                                    let wit_field = &wit_fields[*field_idx];
                                    !wit_field.skip && !has_from_value_skip_attribute(&field.attrs)
                                }).count();
                                quote! {
                                    #idx => {
                                        let fields = case_value.ok_or("Missing case value")?;
                                        match *fields {
                                            golem_wasm::Value::Record(fields) if fields.len() == #expected_len => Ok(#ident::#case_ident {
                                                #(#field_values),*
                                            }),
                                            _ => Err(format!("Expected Record with {} fields for variant fields, got {:?}", #expected_len, fields)),
                                        }
                                    }
                                }
                            }
                        } else {
                            // tuple case
                            let field_values = variant.fields.iter().enumerate().map(|(field_idx, field)| {
                                let elem_ty = &field.ty;
                                let wit_field = &wit_fields[field_idx];
                                let field_from_value = if wit_field.skip || has_from_value_skip_attribute(&field.attrs) {
                                    quote! { Default::default() }
                                } else {
                                    apply_from_conversions(elem_ty, wit_field, quote! { elements[#field_idx].clone() })
                                };
                                quote! {
                                    #field_from_value
                                }
                            });

                            if is_unit_case(variant) {
                                quote! {
                                    #idx => Ok(#ident::#case_ident(Default::default()))
                                }
                            } else {
                                let expected_len = variant.fields.iter().enumerate().filter(|(field_idx, field)| {
                                    let wit_field = &wit_fields[*field_idx];
                                    !wit_field.skip && !has_from_value_skip_attribute(&field.attrs)
                                }).count();
                                quote! {
                                    #idx => {
                                        let elements = case_value.ok_or("Missing case value")?;
                                        match *elements {
                                            golem_wasm::Value::Tuple(elements) if elements.len() == #expected_len => Ok(#ident::#case_ident(
                                                #(#field_values),*
                                            )),
                                            _ => Err(format!("Expected Tuple with {} fields for variant fields, got {:?}", #expected_len, elements)),
                                        }
                                    }
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                quote! {
                    match value {
                        golem_wasm::Value::Variant { case_idx, case_value } => match case_idx {
                            #(#case_branches),*,
                            _ => Err(format!("Invalid variant case index: {}", case_idx)),
                        },
                        _ => Err(format!("Expected Variant value, got {:?}", value)),
                    }
                }
            }
        }
        Data::Union(_data) => {
            panic!("Cannot derive FromValue for unions")
        }
    };

    let result = quote! {
        impl golem_wasm::FromValue for #ident {
            fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
                #from_value
            }
        }
    };

    result.into()
}

fn record_or_tuple_from_value(fields: &Fields) -> proc_macro2::TokenStream {
    let all_fields_has_names = fields.iter().all(|field| field.ident.is_some());

    if all_fields_has_names {
        let wit_fields = fields
            .iter()
            .map(|field| {
                field
                    .attrs
                    .iter()
                    .find(|attr| attr.path().is_ident("wit_field"))
                    .map(parse_wit_field_attribute)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();

        let mut wit_idx = 0usize;
        let field_values = fields.iter().enumerate().map(|(idx, field)| {
            let wit_field = &wit_fields[idx];
            let field_name = field.ident.as_ref().unwrap();
            if wit_field.skip || has_from_value_skip_attribute(&field.attrs) {
                quote! {
                    #field_name: Default::default()
                }
            } else {
                let current_wit_idx = wit_idx;
                wit_idx += 1;
                let field_from_value = apply_from_conversions(
                    &field.ty,
                    wit_field,
                    quote! { fields[#current_wit_idx].clone() },
                );
                quote! {
                    #field_name: #field_from_value
                }
            }
        });

        let expected_len = wit_fields.iter().filter(|f| !f.skip).count();
        quote! {
            match value {
                golem_wasm::Value::Record(fields) if fields.len() == #expected_len => Ok(Self {
                    #(#field_values),*
                }),
                _ => Err(format!("Expected Record value with {} fields, got {:?}", #expected_len, value)),
            }
        }
    } else {
        let field_values = fields.iter().enumerate().map(|(idx, field)| {
            let ty = &field.ty;
            let field_from_value =
                quote! { <#ty as golem_wasm::FromValue>::from_value(fields[#idx].clone())? };
            quote! {
                #field_from_value
            }
        });

        let expected_len = fields.len();
        quote! {
            match value {
                golem_wasm::Value::Tuple(fields) if fields.len() == #expected_len => Ok(Self(
                    #(#field_values),*
                )),
                _ => Err(format!("Expected Tuple value with {} fields, got {:?}", #expected_len, value)),
            }
        }
    }
}

fn has_single_anonymous_field(fields: &Fields) -> bool {
    fields.len() == 1 && fields.iter().next().unwrap().ident.is_none()
}

fn has_only_named_fields(fields: &Fields) -> bool {
    fields.iter().all(|field| field.ident.is_some())
}

fn has_from_value_skip_attribute(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("from_value") && {
            if let Ok(nested) = attr.parse_args::<syn::Ident>() {
                nested == "skip"
            } else {
                false
            }
        }
    })
}

fn apply_from_conversions(
    ty: &Type,
    wit_field: &WitField,
    field_access: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    match (
        &wit_field.convert,
        &wit_field.try_convert,
        &wit_field.convert_vec,
        &wit_field.convert_option,
    ) {
        (Some(convert_to), None, None, None) => {
            quote! { Into::<#ty>::into(<#convert_to as golem_wasm::FromValue>::from_value(#field_access)?) }
        }
        (None, Some(convert_to), None, None) => {
            quote! { TryInto::<#ty>::try_into(<#convert_to as golem_wasm::FromValue>::from_value(#field_access)?)? }
        }
        (None, None, Some(convert_to), None) => {
            quote! { Vec::<#convert_to>::from_value(#field_access)?.into_iter().map(|item| Into::into(item)).collect::<Vec<_>>() }
        }
        (None, None, None, Some(convert_to)) => {
            quote! { Option::<#convert_to>::from_value(#field_access)?.into_iter().map(Into::into) }
        }
        _ => quote! { <#ty as golem_wasm::FromValue>::from_value(#field_access)? },
    }
}
