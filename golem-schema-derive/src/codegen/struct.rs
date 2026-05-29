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

use crate::codegen::helpers::{
    add_trait_bounds, body_expr_for_field, default_name_for, display_name_expr, from_value_expr,
    metadata_expr_for_item, metadata_expr_for_type, private, to_value_expr, type_id_expr,
};
use crate::parse::{ItemAttrs, TypeAttrs, parse_item_attrs};
use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{DataStruct, DeriveInput, Field, Fields, Ident, LitStr};

pub fn expand_struct_into_schema(
    input: &DeriveInput,
    type_attrs: &TypeAttrs,
    data: &DataStruct,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;

    if type_attrs.transparent {
        return expand_transparent_into_schema(input, type_attrs, data);
    }

    let with_bounds = add_trait_bounds(&input.generics, false, true);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();
    let type_id = type_id_expr(type_attrs, &input.generics);
    let display = display_name_expr(type_attrs, ident);
    let metadata = metadata_expr_for_type(type_attrs);

    let body_expr = struct_body_expr(&data.fields, type_attrs)?;
    let to_value_body = struct_to_value(&data.fields, type_attrs)?;

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::IntoSchema for #ident #ty_generics #where_clause {
            fn type_id() -> #private::TypeId {
                #type_id
            }

            fn register_in(builder: &mut #private::SchemaBuilder) -> #private::SchemaType {
                let id = <Self as #private::IntoSchema>::type_id();
                if builder.is_registered(&id) {
                    return #private::SchemaType::Ref(id);
                }
                builder.reserve(id.clone());
                let body: #private::SchemaType = #body_expr;
                builder.commit(id.clone(), #display, #metadata, body);
                #private::SchemaType::Ref(id)
            }

            fn to_value(&self) -> #private::SchemaValue {
                #to_value_body
            }
        }
    })
}

pub fn expand_struct_from_schema(
    input: &DeriveInput,
    type_attrs: &TypeAttrs,
    data: &DataStruct,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;

    if type_attrs.transparent {
        return expand_transparent_from_schema(input, type_attrs, data);
    }

    let with_bounds = add_trait_bounds(&input.generics, true, false);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();

    let decoder = struct_decoder(ident, &data.fields, type_attrs)?;

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::FromSchema for #ident #ty_generics #where_clause {
            fn from_value(
                value: &#private::SchemaValue,
            ) -> ::core::result::Result<Self, #private::FromSchemaError> {
                #decoder
            }
        }
    })
}

// ----------------------------------------------------------------------
// IntoSchema body (schema construction)
// ----------------------------------------------------------------------

fn struct_body_expr(fields: &Fields, type_attrs: &TypeAttrs) -> syn::Result<TokenStream> {
    let private = private();
    match fields {
        Fields::Named(named) => {
            let mut field_tokens: Vec<TokenStream> = Vec::new();
            for f in &named.named {
                let attrs = parse_item_attrs(&f.attrs)?;
                if attrs.skip || attrs.default_with.is_some() {
                    continue;
                }
                field_tokens.push(named_field_token(f, type_attrs, &attrs)?);
            }
            Ok(quote! {
                #private::SchemaType::Record {
                    fields: ::std::vec![ #( #field_tokens ),* ],
                }
            })
        }
        Fields::Unnamed(unnamed) => {
            let element_tokens: Vec<TokenStream> = unnamed
                .unnamed
                .iter()
                .map(unnamed_element_body)
                .collect::<syn::Result<_>>()?;
            Ok(quote! {
                #private::SchemaType::Tuple {
                    elements: ::std::vec![ #( #element_tokens ),* ],
                }
            })
        }
        Fields::Unit => Ok(quote! {
            #private::SchemaType::Record {
                fields: ::std::vec::Vec::new(),
            }
        }),
    }
}

fn named_field_token(
    field: &Field,
    type_attrs: &TypeAttrs,
    item_attrs: &ItemAttrs,
) -> syn::Result<TokenStream> {
    let private = private();
    let ident = field
        .ident
        .as_ref()
        .expect("named field must have an ident");
    let name = item_attrs
        .rename
        .clone()
        .unwrap_or_else(|| default_name_for(ident, type_attrs.rename_all));
    let name_lit = LitStr::new(&name, ident.span());
    let body = body_expr_for_field(item_attrs, &field.ty)?;
    let metadata = metadata_expr_for_item(item_attrs);
    Ok(quote! {
        #private::NamedFieldType {
            name: ::std::string::String::from(#name_lit),
            body: #body,
            metadata: #metadata,
        }
    })
}

fn unnamed_element_body(field: &Field) -> syn::Result<TokenStream> {
    let item_attrs = parse_item_attrs(&field.attrs)?;
    body_expr_for_field(&item_attrs, &field.ty)
}

// ----------------------------------------------------------------------
// to_value
// ----------------------------------------------------------------------

fn struct_to_value(fields: &Fields, _type_attrs: &TypeAttrs) -> syn::Result<TokenStream> {
    let private = private();
    match fields {
        Fields::Named(named) => {
            let mut element_tokens: Vec<TokenStream> = Vec::new();
            for f in &named.named {
                let attrs = parse_item_attrs(&f.attrs)?;
                if attrs.skip || attrs.default_with.is_some() {
                    continue;
                }
                let ident = f.ident.as_ref().unwrap();
                let value_expr = quote! { self.#ident };
                element_tokens.push(to_value_expr(&attrs, &f.ty, value_expr));
            }
            Ok(quote! {
                #private::SchemaValue::Record {
                    fields: ::std::vec![ #( #element_tokens ),* ],
                }
            })
        }
        Fields::Unnamed(unnamed) => {
            let mut element_tokens: Vec<TokenStream> = Vec::new();
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let attrs = parse_item_attrs(&f.attrs)?;
                let idx = syn::Index::from(i);
                let value_expr = quote! { self.#idx };
                element_tokens.push(to_value_expr(&attrs, &f.ty, value_expr));
            }
            Ok(quote! {
                #private::SchemaValue::Tuple {
                    elements: ::std::vec![ #( #element_tokens ),* ],
                }
            })
        }
        Fields::Unit => Ok(quote! {
            #private::SchemaValue::Record { fields: ::std::vec::Vec::new() }
        }),
    }
}

// ----------------------------------------------------------------------
// FromSchema decoder
// ----------------------------------------------------------------------

fn struct_decoder(
    ident: &Ident,
    fields: &Fields,
    _type_attrs: &TypeAttrs,
) -> syn::Result<TokenStream> {
    let private = private();
    match fields {
        Fields::Named(named) => {
            // Determine the order of "encoded" fields (non-skipped/non-default)
            // because they correspond positionally to entries in the SchemaValue.
            let mut encoded_indices: Vec<usize> = Vec::new();
            let mut field_attrs: Vec<(usize, ItemAttrs)> = Vec::new();
            for (idx, f) in named.named.iter().enumerate() {
                let attrs = parse_item_attrs(&f.attrs)?;
                if !attrs.skip && attrs.default_with.is_none() {
                    encoded_indices.push(idx);
                }
                field_attrs.push((idx, attrs));
            }

            let mut bindings: Vec<TokenStream> = Vec::new();
            for (pos, &orig_idx) in encoded_indices.iter().enumerate() {
                let field = &named.named[orig_idx];
                let field_ident = field.ident.as_ref().unwrap();
                let ty = &field.ty;
                let pos_idx = syn::Index::from(pos);
                let attrs = &field_attrs[orig_idx].1;
                let read_value = quote! {
                    field_values.get(#pos_idx).ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "record field index {} out of bounds",
                            #pos_idx
                        ))
                    })?
                };
                let decoded = from_value_expr(attrs, ty, quote! { (#read_value) });
                bindings.push(quote! {
                    let #field_ident: #ty = #decoded;
                });
            }
            // Skipped / default fields get default values.
            for (orig_idx, attrs) in field_attrs.iter() {
                if !attrs.skip && attrs.default_with.is_none() {
                    continue;
                }
                let field = &named.named[*orig_idx];
                let field_ident = field.ident.as_ref().unwrap();
                let ty = &field.ty;
                let init = if let Some(path) = attrs.default_with.as_ref() {
                    let path_tokens: TokenStream =
                        path.parse().map_err(|e: proc_macro2::LexError| {
                            syn::Error::new(field.span(), format!("invalid default path: {e}"))
                        })?;
                    quote! { #path_tokens() }
                } else {
                    quote! { <#ty as ::core::default::Default>::default() }
                };
                bindings.push(quote! {
                    let #field_ident: #ty = #init;
                });
            }
            let inits = named.named.iter().map(|f| {
                let id = f.ident.as_ref().unwrap();
                quote! { #id }
            });
            Ok(quote! {
                let field_values: &::std::vec::Vec<#private::SchemaValue> = match value {
                    #private::SchemaValue::Record { fields } => fields,
                    other => {
                        return ::core::result::Result::Err(
                            #private::FromSchemaError::shape_mismatch(
                                "record",
                                #private::value_kind(other),
                                ::std::stringify!(#ident),
                            ),
                        );
                    }
                };
                #( #bindings )*
                ::core::result::Result::Ok(#ident { #( #inits ),* })
            })
        }
        Fields::Unnamed(unnamed) => {
            let len = unnamed.unnamed.len();
            let mut bindings: Vec<TokenStream> = Vec::new();
            for (idx, field) in unnamed.unnamed.iter().enumerate() {
                let attrs = parse_item_attrs(&field.attrs)?;
                let ty = &field.ty;
                let bind = quote::format_ident!("__field_{idx}");
                let idx_lit = syn::Index::from(idx);
                let read_value = quote! {
                    element_values.get(#idx_lit).ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "tuple element index {} out of bounds",
                            #idx_lit
                        ))
                    })?
                };
                let decoded = from_value_expr(&attrs, ty, quote! { (#read_value) });
                bindings.push(quote! {
                    let #bind: #ty = #decoded;
                });
            }
            let init_idents = (0..len).map(|i| quote::format_ident!("__field_{i}"));
            Ok(quote! {
                let element_values: &::std::vec::Vec<#private::SchemaValue> = match value {
                    #private::SchemaValue::Tuple { elements } => elements,
                    other => {
                        return ::core::result::Result::Err(
                            #private::FromSchemaError::shape_mismatch(
                                "tuple",
                                #private::value_kind(other),
                                ::std::stringify!(#ident),
                            ),
                        );
                    }
                };
                if element_values.len() != #len {
                    return ::core::result::Result::Err(
                        #private::FromSchemaError::custom(::std::format!(
                            "tuple struct length mismatch: expected {}, got {}",
                            #len,
                            element_values.len(),
                        ))
                    );
                }
                #( #bindings )*
                ::core::result::Result::Ok(#ident( #( #init_idents ),* ))
            })
        }
        Fields::Unit => Ok(quote! {
            match value {
                #private::SchemaValue::Record { fields } if fields.is_empty() => {
                    ::core::result::Result::Ok(#ident)
                }
                other => ::core::result::Result::Err(
                    #private::FromSchemaError::shape_mismatch(
                        "empty record",
                        #private::value_kind(other),
                        ::std::stringify!(#ident),
                    ),
                ),
            }
        }),
    }
}

// ----------------------------------------------------------------------
// Transparent newtypes
// ----------------------------------------------------------------------

fn expand_transparent_into_schema(
    input: &DeriveInput,
    _type_attrs: &TypeAttrs,
    data: &DataStruct,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let with_bounds = add_trait_bounds(&input.generics, false, true);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();
    let inner_ty = sole_inner_type(input, data)?;

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::IntoSchema for #ident #ty_generics #where_clause {
            fn type_id() -> #private::TypeId {
                <#inner_ty as #private::IntoSchema>::type_id()
            }
            fn register_in(builder: &mut #private::SchemaBuilder) -> #private::SchemaType {
                <#inner_ty as #private::IntoSchema>::register_in(builder)
            }
            fn to_value(&self) -> #private::SchemaValue {
                <#inner_ty as #private::IntoSchema>::to_value(&self.0)
            }
        }
    })
}

fn expand_transparent_from_schema(
    input: &DeriveInput,
    _type_attrs: &TypeAttrs,
    data: &DataStruct,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let with_bounds = add_trait_bounds(&input.generics, true, false);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();
    let inner_ty = sole_inner_type(input, data)?;

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::FromSchema for #ident #ty_generics #where_clause {
            fn from_value(
                value: &#private::SchemaValue,
            ) -> ::core::result::Result<Self, #private::FromSchemaError> {
                let inner = <#inner_ty as #private::FromSchema>::from_value(value)?;
                ::core::result::Result::Ok(#ident(inner))
            }
        }
    })
}

fn sole_inner_type(input: &DeriveInput, data: &DataStruct) -> syn::Result<syn::Type> {
    match &data.fields {
        Fields::Unnamed(u) if u.unnamed.len() == 1 => Ok(u.unnamed.first().unwrap().ty.clone()),
        _ => Err(syn::Error::new_spanned(
            &input.ident,
            "`#[schema(transparent)]` requires a tuple struct with exactly one field",
        )),
    }
}
