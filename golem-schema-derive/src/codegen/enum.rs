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
use syn::{DataEnum, DeriveInput, Fields, Ident, LitStr, Variant};

struct VariantInfo {
    variant_ident: Ident,
    case_name_lit: LitStr,
    item_attrs: ItemAttrs,
    payload: PayloadShape,
}

#[allow(clippy::large_enum_variant)]
enum PayloadShape {
    Unit,
    Single {
        ty: syn::Type,
        attrs: Box<ItemAttrs>,
    },
    Tuple {
        fields: Vec<(syn::Type, ItemAttrs)>,
    },
    Record {
        fields: Vec<RecordFieldInfo>,
    },
}

struct RecordFieldInfo {
    field_ident: Ident,
    field_name_lit: LitStr,
    ty: syn::Type,
    attrs: ItemAttrs,
}

pub fn expand_enum_into_schema(
    input: &DeriveInput,
    type_attrs: &TypeAttrs,
    data: &DataEnum,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let with_bounds = add_trait_bounds(&input.generics, false, true);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();
    let type_id = type_id_expr(type_attrs, &input.generics);
    let display = display_name_expr(type_attrs, ident);
    let metadata = metadata_expr_for_type(type_attrs);

    let variants: Vec<VariantInfo> = data
        .variants
        .iter()
        .map(|v| parse_variant_info(v, type_attrs))
        .collect::<syn::Result<_>>()?;

    // Reject discriminator attrs on plain (non-union) enums.
    for info in &variants {
        if info.item_attrs.discriminator.is_some() {
            return Err(syn::Error::new(
                info.variant_ident.span(),
                "discriminator attributes (`prefix`, `suffix`, `contains`, `regex`, `field_equals`, `field_absent`) require `#[schema(union)]` on the enum",
            ));
        }
    }

    // A Rust enum whose variants are all unit cases maps to a schema `enum`
    // (matching WIT `enum`) instead of a `variant`.
    let all_unit = !variants.is_empty()
        && variants
            .iter()
            .all(|v| matches!(v.payload, PayloadShape::Unit));

    let body_expr: TokenStream = if all_unit {
        let case_names: Vec<&LitStr> = variants.iter().map(|v| &v.case_name_lit).collect();
        quote! {
            #private::SchemaType::r#enum(
                ::std::vec![ #( ::std::string::String::from(#case_names) ),* ],
            )
        }
    } else {
        let case_tokens: Vec<TokenStream> = variants.iter().map(variant_case_token).collect();
        quote! {
            #private::SchemaType::variant(
                ::std::vec![ #( #case_tokens ),* ],
            )
        }
    };

    let to_value_arms: Vec<TokenStream> = if all_unit {
        variants
            .iter()
            .enumerate()
            .map(|(idx, v)| variant_to_enum_value_arm(ident, idx as u32, v))
            .collect()
    } else {
        variants
            .iter()
            .enumerate()
            .map(|(idx, v)| variant_to_value_arm(ident, idx as u32, v))
            .collect()
    };

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::IntoSchema for #ident #ty_generics #where_clause {
            fn type_id() -> #private::TypeId {
                #type_id
            }

            fn register_in(builder: &mut #private::SchemaBuilder) -> #private::SchemaType {
                let id = <Self as #private::IntoSchema>::type_id();
                if builder.is_registered(&id) {
                    return #private::SchemaType::ref_to(id);
                }
                builder.reserve(id.clone());
                let body: #private::SchemaType = #body_expr;
                builder.commit(id.clone(), #display, #metadata, body);
                #private::SchemaType::ref_to(id)
            }

            fn to_value(&self) -> #private::SchemaValue {
                match self {
                    #( #to_value_arms )*
                }
            }
        }
    })
}

pub fn expand_enum_from_schema(
    input: &DeriveInput,
    type_attrs: &TypeAttrs,
    data: &DataEnum,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let with_bounds = add_trait_bounds(&input.generics, true, false);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();

    let variants: Vec<VariantInfo> = data
        .variants
        .iter()
        .map(|v| parse_variant_info(v, type_attrs))
        .collect::<syn::Result<_>>()?;

    // Same compile-time check on the `FromSchema` path so deriving only
    // `FromSchema` on a non-union enum with discriminator attrs is also
    // rejected.
    for info in &variants {
        if info.item_attrs.discriminator.is_some() {
            return Err(syn::Error::new(
                info.variant_ident.span(),
                "discriminator attributes (`prefix`, `suffix`, `contains`, `regex`, `field_equals`, `field_absent`) require `#[schema(union)]` on the enum",
            ));
        }
    }

    let case_count = variants.len() as u32;

    // A Rust enum whose variants are all unit cases decodes from a schema
    // `enum` value (matching the `IntoSchema` side).
    let all_unit = !variants.is_empty()
        && variants
            .iter()
            .all(|v| matches!(v.payload, PayloadShape::Unit));

    if all_unit {
        let arms: Vec<TokenStream> = variants
            .iter()
            .enumerate()
            .map(|(idx, info)| {
                let idx = idx as u32;
                let variant_ident = &info.variant_ident;
                quote! {
                    #idx => ::core::result::Result::Ok(#ident::#variant_ident),
                }
            })
            .collect();

        return Ok(quote! {
            #[automatically_derived]
            impl #impl_generics #private::FromSchema for #ident #ty_generics #where_clause {
                fn from_value(
                    value: &#private::SchemaValue,
                ) -> ::core::result::Result<Self, #private::FromSchemaError> {
                    let case = match value {
                        #private::SchemaValue::Enum { case } => *case,
                        other => {
                            return ::core::result::Result::Err(
                                #private::FromSchemaError::shape_mismatch(
                                    "enum",
                                    #private::value_kind(other),
                                    ::std::stringify!(#ident),
                                ),
                            );
                        }
                    };
                    if case >= #case_count {
                        return ::core::result::Result::Err(
                            #private::FromSchemaError::out_of_range(
                                case,
                                #case_count,
                                ::std::stringify!(#ident),
                            ),
                        );
                    }
                    match case {
                        #( #arms )*
                        other => ::core::result::Result::Err(
                            #private::FromSchemaError::custom(::std::format!(
                                "enum case index {} unhandled",
                                other,
                            )),
                        ),
                    }
                }
            }
        });
    }

    let arms: Vec<TokenStream> = variants
        .iter()
        .enumerate()
        .map(|(idx, info)| variant_decode_arm(ident, idx as u32, info))
        .collect();

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::FromSchema for #ident #ty_generics #where_clause {
            fn from_value(
                value: &#private::SchemaValue,
            ) -> ::core::result::Result<Self, #private::FromSchemaError> {
                let payload = match value {
                    #private::SchemaValue::Variant(payload) => payload,
                    other => {
                        return ::core::result::Result::Err(
                            #private::FromSchemaError::shape_mismatch(
                                "variant",
                                #private::value_kind(other),
                                ::std::stringify!(#ident),
                            ),
                        );
                    }
                };
                if payload.case >= #case_count {
                    return ::core::result::Result::Err(
                        #private::FromSchemaError::out_of_range(
                            payload.case,
                            #case_count,
                            ::std::stringify!(#ident),
                        ),
                    );
                }
                match payload.case {
                    #( #arms )*
                    other => ::core::result::Result::Err(
                        #private::FromSchemaError::custom(::std::format!(
                            "variant case index {} unhandled",
                            other,
                        )),
                    ),
                }
            }
        }
    })
}

fn parse_variant_info(variant: &Variant, type_attrs: &TypeAttrs) -> syn::Result<VariantInfo> {
    let variant_ident = variant.ident.clone();
    let item_attrs = parse_item_attrs(&variant.attrs)?;
    let case_name = item_attrs
        .rename
        .clone()
        .unwrap_or_else(|| default_name_for(&variant_ident, type_attrs.rename_all));
    let case_name_lit = LitStr::new(&case_name, variant_ident.span());
    let payload = match &variant.fields {
        Fields::Unit => PayloadShape::Unit,
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
            let field = unnamed.unnamed.first().unwrap();
            let attrs = parse_item_attrs(&field.attrs)?;
            PayloadShape::Single {
                ty: field.ty.clone(),
                attrs: Box::new(attrs),
            }
        }
        Fields::Unnamed(unnamed) => {
            let mut fields = Vec::new();
            for field in &unnamed.unnamed {
                let attrs = parse_item_attrs(&field.attrs)?;
                fields.push((field.ty.clone(), attrs));
            }
            PayloadShape::Tuple { fields }
        }
        Fields::Named(named) => {
            let mut fields = Vec::new();
            for field in &named.named {
                let field_ident = field
                    .ident
                    .as_ref()
                    .expect("named field has an ident")
                    .clone();
                let attrs = parse_item_attrs(&field.attrs)?;
                let field_name = attrs
                    .rename
                    .clone()
                    .unwrap_or_else(|| default_name_for(&field_ident, type_attrs.rename_all));
                let field_name_lit = LitStr::new(&field_name, field_ident.span());
                fields.push(RecordFieldInfo {
                    field_ident,
                    field_name_lit,
                    ty: field.ty.clone(),
                    attrs,
                });
            }
            PayloadShape::Record { fields }
        }
    };
    Ok(VariantInfo {
        variant_ident,
        case_name_lit,
        item_attrs,
        payload,
    })
}

fn variant_case_token(info: &VariantInfo) -> TokenStream {
    let private = private();
    let case_name = &info.case_name_lit;
    let metadata = metadata_expr_for_item(&info.item_attrs);
    let payload_expr = match &info.payload {
        PayloadShape::Unit => quote! { ::core::option::Option::None },
        PayloadShape::Single { ty, attrs } => {
            let body = body_expr_for_field(attrs, ty).unwrap_or_else(|e| e.to_compile_error());
            quote! { ::core::option::Option::Some(#body) }
        }
        PayloadShape::Tuple { fields } => {
            let elements: Vec<TokenStream> = fields
                .iter()
                .map(|(ty, attrs)| {
                    body_expr_for_field(attrs, ty).unwrap_or_else(|e| e.to_compile_error())
                })
                .collect();
            quote! {
                ::core::option::Option::Some(#private::SchemaType::tuple(
                    ::std::vec![ #( #elements ),* ],
                ))
            }
        }
        PayloadShape::Record { fields } => {
            let field_tokens: Vec<TokenStream> = fields
                .iter()
                .map(|field| {
                    let name_lit = &field.field_name_lit;
                    let body = body_expr_for_field(&field.attrs, &field.ty)
                        .unwrap_or_else(|e| e.to_compile_error());
                    let field_metadata = metadata_expr_for_item(&field.attrs);
                    quote! {
                        #private::NamedFieldType {
                            name: ::std::string::String::from(#name_lit),
                            body: #body,
                            metadata: #field_metadata,
                        }
                    }
                })
                .collect();
            quote! {
                ::core::option::Option::Some(#private::SchemaType::record(
                    ::std::vec![ #( #field_tokens ),* ],
                ))
            }
        }
    };
    quote! {
        #private::VariantCaseType {
            name: ::std::string::String::from(#case_name),
            payload: #payload_expr,
            metadata: #metadata,
        }
    }
}

fn variant_to_enum_value_arm(parent: &Ident, idx: u32, info: &VariantInfo) -> TokenStream {
    let private = private();
    let variant_ident = &info.variant_ident;
    quote! {
        #parent::#variant_ident => #private::SchemaValue::Enum { case: #idx },
    }
}

fn variant_to_value_arm(parent: &Ident, idx: u32, info: &VariantInfo) -> TokenStream {
    let private = private();
    let variant_ident = &info.variant_ident;
    match &info.payload {
        PayloadShape::Unit => quote! {
            #parent::#variant_ident => #private::SchemaValue::Variant(#private::VariantValuePayload {
                case: #idx,
                payload: ::core::option::Option::None,
            }),
        },
        PayloadShape::Single { ty, attrs } => {
            let value_expr = to_value_expr(attrs, ty, quote! { (*__v) });
            quote! {
                #parent::#variant_ident(__v) => #private::SchemaValue::Variant(#private::VariantValuePayload {
                    case: #idx,
                    payload: ::core::option::Option::Some(::std::boxed::Box::new(#value_expr)),
                }),
            }
        }
        PayloadShape::Tuple { fields } => {
            let binds: Vec<Ident> = (0..fields.len())
                .map(|i| quote::format_ident!("__e{i}"))
                .collect();
            let elem_values: Vec<TokenStream> = fields
                .iter()
                .zip(binds.iter())
                .map(|((ty, attrs), bind)| to_value_expr(attrs, ty, quote! { (*#bind) }))
                .collect();
            quote! {
                #parent::#variant_ident( #( #binds ),* ) => {
                    let __tuple = #private::SchemaValue::Tuple {
                        elements: ::std::vec![ #( #elem_values ),* ],
                    };
                    #private::SchemaValue::Variant(#private::VariantValuePayload {
                        case: #idx,
                        payload: ::core::option::Option::Some(::std::boxed::Box::new(__tuple)),
                    })
                }
            }
        }
        PayloadShape::Record { fields } => {
            let binds: Vec<&Ident> = fields.iter().map(|f| &f.field_ident).collect();
            let elem_values: Vec<TokenStream> = fields
                .iter()
                .map(|f| {
                    let bind = &f.field_ident;
                    to_value_expr(&f.attrs, &f.ty, quote! { (*#bind) })
                })
                .collect();
            quote! {
                #parent::#variant_ident { #( #binds ),* } => {
                    let __record = #private::SchemaValue::Record {
                        fields: ::std::vec![ #( #elem_values ),* ],
                    };
                    #private::SchemaValue::Variant(#private::VariantValuePayload {
                        case: #idx,
                        payload: ::core::option::Option::Some(::std::boxed::Box::new(__record)),
                    })
                }
            }
        }
    }
}

fn variant_decode_arm(parent: &Ident, idx: u32, info: &VariantInfo) -> TokenStream {
    let private = private();
    let variant_ident = &info.variant_ident;
    match &info.payload {
        PayloadShape::Unit => quote! {
            #idx => {
                if payload.payload.is_some() {
                    return ::core::result::Result::Err(
                        #private::FromSchemaError::custom(::std::format!(
                            "variant case `{}` expects no payload",
                            ::std::stringify!(#variant_ident),
                        )),
                    );
                }
                ::core::result::Result::Ok(#parent::#variant_ident)
            }
        },
        PayloadShape::Single { ty, attrs } => {
            let inner_decode = from_value_expr(attrs, ty, quote! { (&**inner) });
            quote! {
                #idx => {
                    let inner = payload.payload.as_ref().ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "variant case `{}` expects a payload",
                            ::std::stringify!(#variant_ident),
                        ))
                    })?;
                    let inner: #ty = #inner_decode;
                    ::core::result::Result::Ok(#parent::#variant_ident(inner))
                }
            }
        }
        PayloadShape::Tuple { fields } => {
            let len = fields.len();
            let mut bindings: Vec<TokenStream> = Vec::new();
            for (i, (ty, attrs)) in fields.iter().enumerate() {
                let bind = quote::format_ident!("__elem_{i}");
                let idx_lit = syn::Index::from(i);
                let read = quote! {
                    elements.get(#idx_lit).ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "tuple variant element index {} out of bounds",
                            #idx_lit,
                        ))
                    })?
                };
                let decoded = from_value_expr(attrs, ty, quote! { (#read) });
                bindings.push(quote! {
                    let #bind: #ty = #decoded;
                });
            }
            let init_idents = (0..len).map(|i| quote::format_ident!("__elem_{i}"));
            quote! {
                #idx => {
                    let inner = payload.payload.as_ref().ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "variant case `{}` expects a tuple payload",
                            ::std::stringify!(#variant_ident),
                        ))
                    })?;
                    let elements: &::std::vec::Vec<#private::SchemaValue> = match &**inner {
                        #private::SchemaValue::Tuple { elements } => elements,
                        other => {
                            return ::core::result::Result::Err(
                                #private::FromSchemaError::shape_mismatch(
                                    "tuple",
                                    #private::value_kind(other),
                                    ::std::stringify!(#variant_ident),
                                ),
                            );
                        }
                    };
                    if elements.len() != #len {
                        return ::core::result::Result::Err(
                            #private::FromSchemaError::custom(::std::format!(
                                "tuple variant length mismatch: expected {}, got {}",
                                #len,
                                elements.len(),
                            ))
                        );
                    }
                    #( #bindings )*
                    ::core::result::Result::Ok(#parent::#variant_ident( #( #init_idents ),* ))
                }
            }
        }
        PayloadShape::Record { fields } => {
            let mut bindings: Vec<TokenStream> = Vec::new();
            for (i, field) in fields.iter().enumerate() {
                let field_ident = &field.field_ident;
                let ty = &field.ty;
                let idx_lit = syn::Index::from(i);
                let read = quote! {
                    field_values.get(#idx_lit).ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "struct variant field index {} out of bounds",
                            #idx_lit,
                        ))
                    })?
                };
                let decoded = from_value_expr(&field.attrs, ty, quote! { (#read) });
                bindings.push(quote! {
                    let #field_ident: #ty = #decoded;
                });
            }
            let init_idents = fields.iter().map(|f| &f.field_ident);
            quote! {
                #idx => {
                    let inner = payload.payload.as_ref().ok_or_else(|| {
                        #private::FromSchemaError::custom(::std::format!(
                            "variant case `{}` expects a record payload",
                            ::std::stringify!(#variant_ident),
                        ))
                    })?;
                    let field_values: &::std::vec::Vec<#private::SchemaValue> = match &**inner {
                        #private::SchemaValue::Record { fields } => fields,
                        other => {
                            return ::core::result::Result::Err(
                                #private::FromSchemaError::shape_mismatch(
                                    "record",
                                    #private::value_kind(other),
                                    ::std::stringify!(#variant_ident),
                                ),
                            );
                        }
                    };
                    #( #bindings )*
                    ::core::result::Result::Ok(#parent::#variant_ident { #( #init_idents ),* })
                }
            }
        }
    }
}
