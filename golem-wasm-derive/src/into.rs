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

use crate::{
    apply_naming, is_unit_case, parse_wit_field_attribute, parse_wit_type_attrs, variant_case_name,
    WitField, WitTypeAttrs,
};
use heck::*;
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{Data, DeriveInput, Fields, Index, LitStr, Type};

pub fn derive_into_value(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).expect("derive input");
    let ident = &ast.ident;
    let wit_transparent = ast
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("wit_transparent"));
    let wit = parse_wit_type_attrs(&ast.attrs);
    let default_name = LitStr::new(&ident.to_string(), Span::call_site());

    let (into_value, get_type) = match ast.data {
        Data::Struct(data) => {
            let newtype_result = if data.fields.len() == 1 {
                let field = data.fields.iter().next().unwrap().clone();
                if field.ident.is_none() || wit_transparent {
                    // single field without an identifier, we consider this a newtype
                    let field_type = field.ty;

                    let into_value = match field.ident {
                        None => quote! {
                            self.0.into_value()
                        },
                        Some(field_name) => quote! {
                            self.#field_name.into_value()
                        },
                    };
                    let get_type = quote! {
                        <#field_type as golem_wasm::IntoValue>::get_type()
                    };

                    Some((into_value, get_type))
                } else {
                    None
                }
            } else {
                None
            };

            match newtype_result {
                Some(newtype_result) => newtype_result,
                None => record_or_tuple(&wit, &default_name, &data.fields),
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
                            #ident::#case_ident => golem_wasm::Value::Enum(#idx)
                        }
                    })
                    .collect::<Vec<_>>();
                let case_labels = data
                    .variants
                    .iter()
                    .map(|variant| variant_case_name(variant))
                    .collect::<Vec<_>>();

                let naming = apply_naming(&wit, &default_name);
                let into_value = quote! {
                    match self {
                        #(#case_branches),*
                    }
                };

                let get_type = quote! {
                    golem_wasm::analysis::analysed_type::r#enum(
                        &[#(#case_labels),*]
                    )#naming
                };

                (into_value, get_type)
            } else if is_simple_enum && wit.as_variant {
                // as_variant: all-unit enum rendered as WIT variant with unit cases
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;
                        quote! {
                            #ident::#case_ident => golem_wasm::Value::Variant {
                                case_idx: #idx,
                                case_value: None
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                let case_defs = data
                    .variants
                    .iter()
                    .map(|variant| {
                        let case_name = variant_case_name(variant);
                        quote! {
                            golem_wasm::analysis::analysed_type::unit_case(#case_name)
                        }
                    })
                    .collect::<Vec<_>>();

                let naming = apply_naming(&wit, &default_name);
                let into_value = quote! {
                    match self {
                        #(#case_branches),*
                    }
                };
                let get_type = quote! {
                    golem_wasm::analysis::analysed_type::variant(
                        vec![#(#case_defs),*]
                    )#naming
                };

                (into_value, get_type)
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
                                #ident::#case_ident => golem_wasm::Value::Variant {
                                    case_idx: #idx,
                                    case_value: None
                                }
                            }
                        } else if has_single_anonymous_field(&variant.fields) {
                            // separate inner type
                            if is_unit_case(variant) {
                                quote! {
                                    #ident::#case_ident(inner) => golem_wasm::Value::Variant {
                                        case_idx: #idx,
                                        case_value: None
                                    }
                                }
                            } else {
                                let wit_field = wit_fields.first().expect("Expected one item in wit_fields");
                                let into_value = apply_conversions(wit_field, quote! { inner });
                                quote! {
                                    #ident::#case_ident(inner) => golem_wasm::Value::Variant {
                                        case_idx: #idx,
                                        case_value: Some(Box::new(#into_value))
                                    }
                                }
                            }
                        } else if has_only_named_fields(&variant.fields) {
                            // record case
                            let field_names = variant
                                .fields
                                .iter()
                                .zip(&wit_fields)
                                .map(|(field, wit_field)| {
                                    let field_ident = field.ident.as_ref().expect("Expected field to have an identifier");
                                    if wit_field.skip {
                                        let prefixed = Ident::new(&format!("_{}", field_ident), field_ident.span());
                                        quote! { #field_ident: #prefixed }
                                    } else {
                                        quote! { #field_ident }
                                    }
                                })
                                .collect::<Vec<_>>();

                            let field_values = variant.fields.iter().zip(&wit_fields).filter_map(|(field, wit_field)| {
                                if wit_field.skip {
                                    None
                                } else {
                                    let field_name = field.ident.as_ref().expect("Expected field to have an identifier");
                                    Some(apply_conversions(wit_field, quote! { #field_name }))
                                }
                            }).collect::<Vec<_>>();

                            if is_unit_case(variant) {
                                quote! {
                                    #ident::#case_ident { #(#field_names),* } => golem_wasm::Value::Variant {
                                        case_idx: #idx,
                                        case_value: None
                                    }
                                }
                            } else {
                                quote! {
                                    #ident::#case_ident { #(#field_names),* } =>
                                        golem_wasm::Value::Variant {
                                            case_idx: #idx,
                                            case_value: Some(Box::new(golem_wasm::Value::Record(
                                                vec![#(#field_values),*]
                                            )))
                                        }
                                }
                            }
                        } else {
                            // tuple case
                            let field_names = variant
                                .fields
                                .iter()
                                .enumerate()
                                .map(|(idx, _field)| {
                                    Ident::new(&format!("f{idx}"), Span::call_site())
                                })
                                .collect::<Vec<_>>();

                            let field_values = field_names.iter().map(|field| {
                                quote! {
                                    #field.into_value()
                                }
                            });

                            if is_unit_case(variant) {
                                quote! {
                                    #ident::#case_ident(#(#field_names),*) => golem_wasm::Value::Variant {
                                        case_idx: #idx,
                                        case_value: None
                                    }
                                }
                            } else {
                                quote! {
                                    #ident::#case_ident(#(#field_names),*) =>
                                        golem_wasm::Value::Variant {
                                            case_idx: #idx,
                                            case_value: Some(Box::new(golem_wasm::Value::Tuple(
                                                vec![#(#field_values),*]
                                            )))
                                        }
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                let case_defs = data.variants.iter()
                    .map(|variant| {
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

                        let case_name = variant_case_name(variant);
                        if is_unit_case(variant) {
                            quote! {
                                golem_wasm::analysis::analysed_type::unit_case(#case_name)
                            }
                        } else if has_single_anonymous_field(&variant.fields) {
                            let single_field = variant.fields.iter().next().expect("Expected variant.fields to have at least one item");
                            let typ = &single_field.ty;
                            let wit_field = wit_fields.first().expect("Expected wit_fields to have at least one item");
                            let typ = get_field_type(typ, wit_field);

                            quote! {
                                golem_wasm::analysis::analysed_type::case(#case_name, <#typ as golem_wasm::IntoValue>::get_type())
                            }
                        } else {
                            let no_wit = WitTypeAttrs::default();
                            let case_lit = LitStr::new(&case_name, Span::call_site());
                            let (_, inner_get_type) = record_or_tuple(&no_wit, &case_lit, &variant.fields);

                            quote! {
                                golem_wasm::analysis::analysed_type::case(#case_name, #inner_get_type)
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                let naming = apply_naming(&wit, &default_name);
                let into_value = quote! {
                    match self {
                        #(#case_branches),*
                    }
                };
                let get_type = quote! {
                    golem_wasm::analysis::analysed_type::variant(
                        vec![#(#case_defs),*]
                    )#naming
                };

                (into_value, get_type)
            }
        }
        Data::Union(_data) => {
            panic!("Cannot derive IntoValue for unions")
        }
    };

    let result = quote! {
        impl golem_wasm::IntoValue for #ident {
            fn into_value(self) -> golem_wasm::Value {
                #into_value
            }

            fn get_type() -> golem_wasm::analysis::AnalysedType {
                #get_type
            }
        }
    };

    result.into()
}

fn record_or_tuple(
    wit: &WitTypeAttrs,
    default_name: &LitStr,
    fields: &Fields,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
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

        let field_values = fields
            .iter()
            .zip(&wit_fields)
            .filter_map(|(field, wit_field)| {
                if wit_field.skip {
                    None
                } else {
                    let field_name = field
                        .ident
                        .as_ref()
                        .expect("Expected field to have an identifier");
                    Some(apply_conversions(wit_field, quote! { self.#field_name }))
                }
            })
            .collect::<Vec<_>>();

        let field_defs = fields
            .iter()
            .zip(wit_fields)
            .filter_map(|(field, wit_field)| {
                if wit_field.skip {
                    None
                } else {
                    let field_name = wit_field
                        .rename
                        .as_ref()
                        .map(|lit| lit.value())
                        .unwrap_or_else(|| {
                            field
                                .ident
                                .as_ref()
                                .expect("Expected field to have an identifier")
                                .to_string()
                                .to_kebab_case()
                        });
                    let field_type = get_field_type(&field.ty, &wit_field);
                    Some(quote! {
                        golem_wasm::analysis::analysed_type::field(
                            #field_name,
                            <#field_type as golem_wasm::IntoValue>::get_type()
                        )
                    })
                }
            })
            .collect::<Vec<_>>();

        let naming = apply_naming(wit, default_name);
        let into_value = quote! {
            golem_wasm::Value::Record(vec![
                #(#field_values),*
            ])
        };
        let get_type = quote! {
            golem_wasm::analysis::analysed_type::record(vec![
                #(#field_defs),*
            ])#naming
        };

        (into_value, get_type)
    } else {
        let tuple_field_values = fields
            .iter()
            .enumerate()
            .map(|(idx, _field)| {
                let idx = Index::from(idx);
                quote! { self.#idx.into_value() }
            })
            .collect::<Vec<_>>();

        let tuple_field_types = fields
            .iter()
            .map(|field| {
                let field_type = &field.ty;
                quote! {
                    <#field_type as golem_wasm::IntoValue>::get_type()
                }
            })
            .collect::<Vec<_>>();

        let naming = apply_naming(wit, default_name);
        let into_value = quote! {
            golem_wasm::Value::Tuple(vec![
                #(#tuple_field_values),*
            ])
        };
        let get_type = quote! {
            golem_wasm::analysis::analysed_type::tuple(vec![
                #(#tuple_field_types),*
            ])#naming
        };

        (into_value, get_type)
    }
}

fn get_field_type(ty: &Type, wit_field: &WitField) -> proc_macro2::TokenStream {
    match (
        &wit_field.convert,
        &wit_field.convert_vec,
        &wit_field.convert_option,
    ) {
        (Some(convert_to), None, None) => quote! { #convert_to },
        (None, Some(convert_to), None) => quote! { Vec<#convert_to> },
        (None, None, Some(convert_to)) => quote! { Option<#convert_to> },
        _ => {
            quote! { #ty }
        }
    }
}

fn has_single_anonymous_field(fields: &Fields) -> bool {
    fields.len() == 1 && fields.iter().next().unwrap().ident.is_none()
}

fn has_only_named_fields(fields: &Fields) -> bool {
    fields.iter().all(|field| field.ident.is_some())
}

fn apply_conversions(
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
            quote! { Into::<#convert_to>::into(#field_access).into_value() }
        }
        (None, Some(convert_to), None, None) => {
            quote! { Into::<#convert_to>::into(#field_access).into_value() }
        }
        (None, None, Some(convert_to), None) => {
            quote! { #field_access.into_iter().map(Into::<#convert_to>::into).collect::<Vec<_>>().into_value() }
        }
        (None, None, None, Some(convert_to)) => {
            quote! { #field_access.map(Into::<#convert_to>::into).into_value() }
        }
        _ => quote! { #field_access.into_value() },
    }
}
