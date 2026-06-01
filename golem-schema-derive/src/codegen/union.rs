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
use crate::parse::{DiscriminatorAttr, ItemAttrs, TypeAttrs, parse_item_attrs};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DataEnum, DeriveInput, Fields, Ident, LitStr, Variant};

struct BranchInfo {
    variant_ident: Ident,
    tag_lit: LitStr,
    tag_value: String,
    item_attrs: ItemAttrs,
    payload_ty: syn::Type,
    payload_attrs: ItemAttrs,
}

pub fn expand_union_into_schema(
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

    let branches: Vec<BranchInfo> = data
        .variants
        .iter()
        .map(|v| parse_branch_info(v, type_attrs))
        .collect::<syn::Result<_>>()?;

    let branch_tokens: Vec<TokenStream> = branches
        .iter()
        .map(branch_to_token)
        .collect::<syn::Result<_>>()?;

    let to_value_arms: Vec<TokenStream> = branches.iter().map(branch_to_value_arm).collect();

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
                let body: #private::SchemaType = #private::SchemaType::union(
                    #private::UnionSpec {
                        branches: ::std::vec![ #( #branch_tokens ),* ],
                    },
                );
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

pub fn expand_union_from_schema(
    input: &DeriveInput,
    type_attrs: &TypeAttrs,
    data: &DataEnum,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let with_bounds = add_trait_bounds(&input.generics, true, false);
    let (impl_generics, _, where_clause) = with_bounds.split_for_impl();
    let (_, ty_generics, _) = input.generics.split_for_impl();
    let private = private();

    let branches: Vec<BranchInfo> = data
        .variants
        .iter()
        .map(|v| parse_branch_info(v, type_attrs))
        .collect::<syn::Result<_>>()?;

    let arms: Vec<TokenStream> = branches
        .iter()
        .map(|info| {
            let tag = &info.tag_lit;
            let ty = &info.payload_ty;
            let variant_ident = &info.variant_ident;
            let decoded = from_value_expr(&info.payload_attrs, ty, quote! { (&payload.body) });
            quote! {
                #tag => {
                    let inner: #ty = #decoded;
                    ::core::result::Result::Ok(#ident::#variant_ident(inner))
                }
            }
        })
        .collect();

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #private::FromSchema for #ident #ty_generics #where_clause {
            fn from_value(
                value: &#private::SchemaValue,
            ) -> ::core::result::Result<Self, #private::FromSchemaError> {
                let payload = match value {
                    #private::SchemaValue::Union(payload) => payload,
                    other => {
                        return ::core::result::Result::Err(
                            #private::FromSchemaError::shape_mismatch(
                                "union",
                                #private::value_kind(other),
                                ::std::stringify!(#ident),
                            ),
                        );
                    }
                };
                match payload.tag.as_str() {
                    #( #arms )*
                    other => ::core::result::Result::Err(
                        #private::FromSchemaError::UnknownUnionTag(::std::string::String::from(other)),
                    ),
                }
            }
        }
    })
}

fn parse_branch_info(variant: &Variant, type_attrs: &TypeAttrs) -> syn::Result<BranchInfo> {
    let variant_ident = variant.ident.clone();
    let item_attrs = parse_item_attrs(&variant.attrs)?;
    if item_attrs.discriminator.is_none() {
        return Err(syn::Error::new(
            variant_ident.span(),
            "each branch of a `#[schema(union)]` enum must declare a discriminator attribute (`prefix`, `suffix`, `contains`, `regex`, `field_equals`, or `field_absent`)",
        ));
    }
    let tag_value = item_attrs
        .rename
        .clone()
        .unwrap_or_else(|| default_name_for(&variant_ident, type_attrs.rename_all));
    let tag_lit = LitStr::new(&tag_value, variant_ident.span());

    let (payload_ty, payload_attrs) = match &variant.fields {
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
            let field = unnamed.unnamed.first().unwrap();
            (field.ty.clone(), parse_item_attrs(&field.attrs)?)
        }
        _ => {
            return Err(syn::Error::new(
                variant_ident.span(),
                "`#[schema(union)]` branches must have exactly one positional field",
            ));
        }
    };

    Ok(BranchInfo {
        variant_ident,
        tag_lit,
        tag_value,
        item_attrs,
        payload_ty,
        payload_attrs,
    })
}

fn branch_to_token(info: &BranchInfo) -> syn::Result<TokenStream> {
    let private = private();
    let tag_lit = &info.tag_lit;
    let body = body_expr_for_field(&info.payload_attrs, &info.payload_ty)?;
    let metadata = metadata_expr_for_item(&info.item_attrs);
    let discriminator = match info.item_attrs.discriminator.as_ref().expect("checked") {
        DiscriminatorAttr::Prefix(p) => {
            let lit = LitStr::new(p, proc_macro2::Span::call_site());
            quote! {
                #private::DiscriminatorRule::Prefix {
                    prefix: ::std::string::String::from(#lit),
                }
            }
        }
        DiscriminatorAttr::Suffix(s) => {
            let lit = LitStr::new(s, proc_macro2::Span::call_site());
            quote! {
                #private::DiscriminatorRule::Suffix {
                    suffix: ::std::string::String::from(#lit),
                }
            }
        }
        DiscriminatorAttr::Contains(c) => {
            let lit = LitStr::new(c, proc_macro2::Span::call_site());
            quote! {
                #private::DiscriminatorRule::Contains {
                    substring: ::std::string::String::from(#lit),
                }
            }
        }
        DiscriminatorAttr::Regex(r) => {
            let lit = LitStr::new(r, proc_macro2::Span::call_site());
            quote! {
                #private::DiscriminatorRule::Regex {
                    regex: ::std::string::String::from(#lit),
                }
            }
        }
        DiscriminatorAttr::FieldEquals { field, literal } => {
            let field_lit = LitStr::new(field, proc_macro2::Span::call_site());
            let literal_tokens = match literal {
                Some(s) => {
                    let lit = LitStr::new(s, proc_macro2::Span::call_site());
                    quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
                }
                None => quote! { ::core::option::Option::None },
            };
            quote! {
                #private::DiscriminatorRule::FieldEquals(#private::FieldDiscriminator {
                    field_name: ::std::string::String::from(#field_lit),
                    literal: #literal_tokens,
                })
            }
        }
        DiscriminatorAttr::FieldAbsent(field) => {
            let lit = LitStr::new(field, proc_macro2::Span::call_site());
            quote! {
                #private::DiscriminatorRule::FieldAbsent {
                    field_name: ::std::string::String::from(#lit),
                }
            }
        }
    };
    Ok(quote! {
        #private::UnionBranch {
            tag: ::std::string::String::from(#tag_lit),
            body: #body,
            discriminator: #discriminator,
            metadata: #metadata,
        }
    })
}

fn branch_to_value_arm(info: &BranchInfo) -> TokenStream {
    let private = private();
    let tag = &info.tag_lit;
    let variant_ident = &info.variant_ident;
    let ty = &info.payload_ty;
    let body_value = to_value_expr(&info.payload_attrs, ty, quote! { (*__inner) });
    let _ = &info.tag_value;
    quote! {
        Self::#variant_ident(__inner) => {
            #private::SchemaValue::Union(#private::UnionValuePayload {
                tag: ::std::string::String::from(#tag),
                body: ::std::boxed::Box::new(#body_value),
            })
        }
    }
}
