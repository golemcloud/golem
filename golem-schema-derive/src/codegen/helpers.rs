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

//! Shared codegen primitives reused across struct / enum / union expansions.

use crate::parse::{DeprecatedMarker, ItemAttrs, PathAttrSpec, RenameAll, RichSpec, TypeAttrs};
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{GenericParam, Generics, Type};

/// Path prefix used for everything the derive references.
pub fn private() -> TokenStream {
    let schema_crate = schema_crate_path();
    quote! { #schema_crate::schema::derive::__private }
}

fn schema_crate_path() -> TokenStream {
    crate_path("golem-schema")
        .or_else(|| crate_path("golem-rust"))
        .unwrap_or_else(|| crate_path("golem-common").unwrap_or_else(|| quote! { ::golem_common }))
}

fn crate_path(name: &str) -> Option<TokenStream> {
    match crate_name(name).ok()? {
        FoundCrate::Itself => Some(quote! { crate }),
        FoundCrate::Name(name) => {
            let ident = syn::Ident::new(&name, Span::call_site());
            Some(quote! { ::#ident })
        }
    }
}

/// Emit the `TypeId` value for a derived type, applying `#[schema(named = ...)]`
/// when present. Generic type instantiations append resolved type arguments
/// inside `<…>` so each instantiation gets its own definition.
pub fn type_id_expr(type_attrs: &TypeAttrs, generics: &Generics) -> TokenStream {
    let private = private();
    let type_params: Vec<&syn::Ident> = generics
        .params
        .iter()
        .filter_map(|p| match p {
            GenericParam::Type(t) => Some(&t.ident),
            _ => None,
        })
        .collect();

    match (&type_attrs.named, type_params.is_empty()) {
        // Explicit name, no generics — use as-is.
        (Some(name), true) => quote! { #private::TypeId::new(#name) },
        // Explicit name with generics — append resolved args.
        (Some(name), false) => {
            quote! {
                #private::type_id_with_args(
                    #name,
                    &[ #( <#type_params as #private::IntoSchema>::type_id() ),* ],
                )
            }
        }
        // No explicit name — normalize the Rust path FQN.
        (None, _) => quote! {
            #private::default_type_id_from(::core::any::type_name::<Self>())
        },
    }
}

/// Compile-time `String` for the type's display name (used as
/// `SchemaTypeDef::name`).
pub fn display_name_expr(type_attrs: &TypeAttrs, ident: &syn::Ident) -> TokenStream {
    match &type_attrs.named {
        Some(name) => {
            let lit = syn::LitStr::new(name, proc_macro2::Span::call_site());
            quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
        }
        None => {
            let lit = syn::LitStr::new(&ident.to_string(), ident.span());
            quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
        }
    }
}

/// Build a `MetadataEnvelope` expression from the parsed type-level attributes.
pub fn metadata_expr_for_type(type_attrs: &TypeAttrs) -> TokenStream {
    metadata_expr_inner(
        type_attrs.doc.as_deref(),
        &type_attrs.alias,
        &type_attrs.example,
        type_attrs.deprecated.as_ref(),
        type_attrs.role.as_deref(),
    )
}

/// Build a `MetadataEnvelope` expression from item-level (field / case)
/// attributes.
pub fn metadata_expr_for_item(item_attrs: &ItemAttrs) -> TokenStream {
    metadata_expr_inner(
        item_attrs.doc.as_deref(),
        &item_attrs.alias,
        &item_attrs.example,
        item_attrs.deprecated.as_ref(),
        None,
    )
}

fn metadata_expr_inner(
    doc: Option<&str>,
    alias: &[String],
    example: &[String],
    deprecated: Option<&DeprecatedMarker>,
    role: Option<&str>,
) -> TokenStream {
    let private = private();
    let doc_tokens = match doc {
        Some(s) => {
            let lit = syn::LitStr::new(s, proc_macro2::Span::call_site());
            quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
        }
        None => quote! { ::core::option::Option::None },
    };
    let alias_tokens = string_vec(alias);
    let example_tokens = string_vec(example);
    let deprecated_tokens = match deprecated {
        Some(marker) => {
            let lit = syn::LitStr::new(&marker.message(), proc_macro2::Span::call_site());
            quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
        }
        None => quote! { ::core::option::Option::None },
    };
    let role_tokens = match role {
        Some(role) => match role {
            "multimodal" => {
                quote! { ::core::option::Option::Some(#private::Role::Multimodal) }
            }
            "unstructured-text" => {
                quote! { ::core::option::Option::Some(#private::Role::UnstructuredText) }
            }
            "unstructured-binary" => {
                quote! { ::core::option::Option::Some(#private::Role::UnstructuredBinary) }
            }
            other => {
                let lit = syn::LitStr::new(other, proc_macro2::Span::call_site());
                quote! { ::core::option::Option::Some(#private::Role::Other(::std::string::String::from(#lit))) }
            }
        },
        None => quote! { ::core::option::Option::None },
    };
    quote! {
        #private::MetadataEnvelope {
            doc: #doc_tokens,
            aliases: #alias_tokens,
            examples: #example_tokens,
            deprecated: #deprecated_tokens,
            role: #role_tokens,
        }
    }
}

fn string_vec(strs: &[String]) -> TokenStream {
    if strs.is_empty() {
        quote! { ::std::vec::Vec::new() }
    } else {
        let lits = strs
            .iter()
            .map(|s| syn::LitStr::new(s, proc_macro2::Span::call_site()));
        quote! { ::std::vec![ #( ::std::string::String::from(#lits) ),* ] }
    }
}

/// Body expression for a field or case payload (the `SchemaType` body used at
/// the parent's use-site). If a rich-scalar attribute is present, the
/// resulting body is the matching rich scalar; otherwise falls through to
/// `<ty as IntoSchema>::register_in(builder)`.
pub fn body_expr_for_field(item_attrs: &ItemAttrs, ty: &Type) -> syn::Result<TokenStream> {
    if let Some(rich) = &item_attrs.rich {
        return Ok(rich_spec_to_body(rich));
    }
    Ok(register_type_expr(ty))
}

/// Body expression for a `register_in` call against an arbitrary type.
pub fn register_type_expr(ty: &Type) -> TokenStream {
    let private = private();
    quote! {
        <#ty as #private::IntoSchema>::register_in(builder)
    }
}

/// Convert an expression yielding a value of `ty` to a `SchemaValue`,
/// honouring rich-scalar attributes when present.
pub fn to_value_expr(item_attrs: &ItemAttrs, ty: &Type, value_expr: TokenStream) -> TokenStream {
    let private = private();
    match item_attrs.rich.as_ref() {
        Some(RichSpec::Text(spec)) => {
            let language = match (spec.language.as_ref(), spec.languages.as_ref()) {
                (Some(l), _) => {
                    let lit = syn::LitStr::new(l, proc_macro2::Span::call_site());
                    quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
                }
                _ => quote! { ::core::option::Option::None },
            };
            quote! {
                #private::text_to_value(::std::clone::Clone::clone(&(#value_expr)), #language)
            }
        }
        Some(RichSpec::Binary(spec)) => {
            let mime_type = match (spec.mime_type.as_ref(), spec.mime_types.as_ref()) {
                (Some(m), _) => {
                    let lit = syn::LitStr::new(m, proc_macro2::Span::call_site());
                    quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
                }
                _ => quote! { ::core::option::Option::None },
            };
            quote! {
                #private::binary_to_value(::std::clone::Clone::clone(&(#value_expr)), #mime_type)
            }
        }
        Some(RichSpec::Path(_)) => {
            quote! {
                #private::path_to_value(::std::clone::Clone::clone(&(#value_expr)))
            }
        }
        Some(RichSpec::Url(_)) => {
            quote! {
                #private::url_to_value(::std::clone::Clone::clone(&(#value_expr)))
            }
        }
        Some(RichSpec::Quantity(_)) => {
            quote! {
                #private::SchemaValue::Quantity(::std::clone::Clone::clone(&(#value_expr)))
            }
        }
        Some(RichSpec::Secret(_)) => {
            quote! {
                #private::secret_to_value(::std::clone::Clone::clone(&(#value_expr)))
            }
        }
        Some(RichSpec::QuotaToken(_)) => {
            quote! {
                #private::SchemaValue::QuotaToken(::std::clone::Clone::clone(&(#value_expr)))
            }
        }
        None => quote! {
            <#ty as #private::IntoSchema>::to_value(&(#value_expr))
        },
    }
}

/// Inverse of [`to_value_expr`]: given a `SchemaValue` reference, produce an
/// expression decoding it back into `ty`, honouring rich-scalar attributes.
pub fn from_value_expr(item_attrs: &ItemAttrs, ty: &Type, value_expr: TokenStream) -> TokenStream {
    let private = private();
    let ctx_lit = syn::LitStr::new(
        &quote::quote!(#ty).to_string(),
        proc_macro2::Span::call_site(),
    );
    match item_attrs.rich.as_ref() {
        Some(RichSpec::Text(_)) => {
            quote! {
                {
                    let __decoded: #ty = #private::text_from_value(#value_expr, #ctx_lit)?;
                    __decoded
                }
            }
        }
        Some(RichSpec::Binary(_)) => {
            quote! {
                {
                    let __decoded: #ty = #private::binary_from_value(#value_expr, #ctx_lit)?;
                    __decoded
                }
            }
        }
        Some(RichSpec::Path(_)) => {
            quote! {
                {
                    let __decoded: #ty = #private::path_from_value(#value_expr, #ctx_lit)?;
                    __decoded
                }
            }
        }
        Some(RichSpec::Url(_)) => {
            quote! {
                {
                    let __decoded: #ty = #private::url_from_value(#value_expr, #ctx_lit)?;
                    __decoded
                }
            }
        }
        Some(RichSpec::Quantity(_)) => {
            quote! {
                match #value_expr {
                    #private::SchemaValue::Quantity(q) => {
                        let __decoded: #ty = ::std::clone::Clone::clone(q);
                        __decoded
                    }
                    other => return ::core::result::Result::Err(
                        #private::FromSchemaError::shape_mismatch(
                            "quantity", #private::value_kind(other), #ctx_lit,
                        ),
                    ),
                }
            }
        }
        Some(RichSpec::Secret(_)) => {
            quote! {
                {
                    let __decoded: #ty = #private::secret_from_value(#value_expr, #ctx_lit)?;
                    __decoded
                }
            }
        }
        Some(RichSpec::QuotaToken(_)) => {
            quote! {
                match #value_expr {
                    #private::SchemaValue::QuotaToken(q) => {
                        let __decoded: #ty = ::std::clone::Clone::clone(q);
                        __decoded
                    }
                    other => return ::core::result::Result::Err(
                        #private::FromSchemaError::shape_mismatch(
                            "quota-token", #private::value_kind(other), #ctx_lit,
                        ),
                    ),
                }
            }
        }
        None => quote! {
            <#ty as #private::FromSchema>::from_value(#value_expr)?
        },
    }
}

fn rich_spec_to_body(rich: &RichSpec) -> TokenStream {
    let private = private();
    match rich {
        RichSpec::Text(spec) => {
            let languages = match (spec.language.as_ref(), spec.languages.as_ref()) {
                (None, None) => quote! { ::core::option::Option::None },
                (Some(language), None) => {
                    let lit = syn::LitStr::new(language, proc_macro2::Span::call_site());
                    quote! { ::core::option::Option::Some(::std::vec![::std::string::String::from(#lit)]) }
                }
                (_, Some(list)) => {
                    let lits = list
                        .iter()
                        .map(|s| syn::LitStr::new(s, proc_macro2::Span::call_site()));
                    quote! { ::core::option::Option::Some(::std::vec![ #( ::std::string::String::from(#lits) ),* ]) }
                }
            };
            let min = option_u32(spec.min);
            let max = option_u32(spec.max);
            let regex = option_string(spec.regex.as_deref());
            quote! {
                #private::SchemaType::text(#private::TextRestrictions {
                    languages: #languages,
                    min_length: #min,
                    max_length: #max,
                    regex: #regex,
                })
            }
        }
        RichSpec::Binary(spec) => {
            let mime_types = match (spec.mime_type.as_ref(), spec.mime_types.as_ref()) {
                (None, None) => quote! { ::core::option::Option::None },
                (Some(mt), None) => {
                    let lit = syn::LitStr::new(mt, proc_macro2::Span::call_site());
                    quote! { ::core::option::Option::Some(::std::vec![::std::string::String::from(#lit)]) }
                }
                (_, Some(list)) => {
                    let lits = list
                        .iter()
                        .map(|s| syn::LitStr::new(s, proc_macro2::Span::call_site()));
                    quote! { ::core::option::Option::Some(::std::vec![ #( ::std::string::String::from(#lits) ),* ]) }
                }
            };
            let min = option_u32(spec.min_bytes);
            let max = option_u32(spec.max_bytes);
            quote! {
                #private::SchemaType::binary(#private::BinaryRestrictions {
                    mime_types: #mime_types,
                    min_bytes: #min,
                    max_bytes: #max,
                })
            }
        }
        RichSpec::Path(spec) => path_spec_body(spec),
        RichSpec::Url(spec) => {
            let schemes = option_string_vec(spec.allowed_schemes.as_deref());
            let hosts = option_string_vec(spec.allowed_hosts.as_deref());
            quote! {
                #private::SchemaType::url(#private::UrlRestrictions {
                    allowed_schemes: #schemes,
                    allowed_hosts: #hosts,
                })
            }
        }
        RichSpec::Quantity(spec) => {
            let base = syn::LitStr::new(&spec.base_unit, proc_macro2::Span::call_site());
            let suffixes = spec.allowed_suffixes.clone().unwrap_or_default();
            let suffix_lits = suffixes
                .iter()
                .map(|s| syn::LitStr::new(s, proc_macro2::Span::call_site()));
            let min = parse_quantity_opt_token(spec.min.as_deref(), &spec.base_unit);
            let max = parse_quantity_opt_token(spec.max.as_deref(), &spec.base_unit);
            quote! {
                #private::SchemaType::quantity(#private::QuantitySpec {
                    base_unit: ::std::string::String::from(#base),
                    allowed_suffixes: ::std::vec![ #( ::std::string::String::from(#suffix_lits) ),* ],
                    min: #min,
                    max: #max,
                })
            }
        }
        RichSpec::Secret(spec) => {
            let category = option_string(spec.category.as_deref());
            quote! {
                #private::SchemaType::secret(#private::SecretSpec {
                    category: #category,
                })
            }
        }
        RichSpec::QuotaToken(spec) => {
            let resource = option_string(spec.resource_name.as_deref());
            quote! {
                #private::SchemaType::quota_token(#private::QuotaTokenSpec {
                    resource_name: #resource,
                })
            }
        }
    }
}

fn path_spec_body(spec: &PathAttrSpec) -> TokenStream {
    let private = private();
    let direction = match spec.direction.as_str() {
        "input" => quote! { #private::PathDirection::Input },
        "output" => quote! { #private::PathDirection::Output },
        "in-out" | "inout" => quote! { #private::PathDirection::InOut },
        other => {
            let msg = format!(
                "unknown path direction `{other}` (expected `input`, `output`, or `in-out`)"
            );
            return syn::Error::new(spec.direction.span(), msg).to_compile_error();
        }
    };
    let kind = match spec.kind.as_str() {
        "file" => quote! { #private::PathKind::File },
        "directory" => quote! { #private::PathKind::Directory },
        "any" => quote! { #private::PathKind::Any },
        other => {
            let msg =
                format!("unknown path kind `{other}` (expected `file`, `directory`, or `any`)");
            return syn::Error::new(spec.kind.span(), msg).to_compile_error();
        }
    };
    let allowed_mime = option_string_vec(spec.allowed_mime_types.as_deref());
    let allowed_ext = option_string_vec(spec.allowed_extensions.as_deref());
    quote! {
        #private::SchemaType::path(#private::PathSpec {
            direction: #direction,
            kind: #kind,
            allowed_mime_types: #allowed_mime,
            allowed_extensions: #allowed_ext,
        })
    }
}

fn parse_quantity_opt_token(raw: Option<&str>, default_unit: &str) -> TokenStream {
    let private = private();
    match raw {
        None => quote! { ::core::option::Option::None },
        Some(text) => {
            let (mantissa, scale, unit) = parse_quantity_literal(text, default_unit);
            let unit_lit = syn::LitStr::new(&unit, proc_macro2::Span::call_site());
            quote! {
                ::core::option::Option::Some(#private::QuantityValue {
                    mantissa: #mantissa,
                    scale: #scale,
                    unit: ::std::string::String::from(#unit_lit),
                })
            }
        }
    }
}

/// Parse a quantity literal of the form `<decimal><unit>` into `(mantissa,
/// scale, unit)`. Permissive compile-time parser; runtime canonical parsing
/// is the source of truth.
fn parse_quantity_literal(text: &str, default_unit: &str) -> (i64, i32, String) {
    let trimmed = text.trim();
    let split_at = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+' || c == '.'))
        .unwrap_or(trimmed.len());
    let (number_part, unit_part) = trimmed.split_at(split_at);
    let unit = unit_part.trim();
    let unit_owned = if unit.is_empty() {
        default_unit.to_string()
    } else {
        unit.to_string()
    };
    let (mantissa, scale) = parse_decimal(number_part).unwrap_or((0, 0));
    (mantissa, scale, unit_owned)
}

fn parse_decimal(input: &str) -> Option<(i64, i32)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (sign, rest) = if let Some(rest) = trimmed.strip_prefix('-') {
        (-1i64, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (1, rest)
    } else {
        (1, trimmed)
    };
    let mut digits = String::new();
    let mut scale = 0i32;
    let mut seen_dot = false;
    for ch in rest.chars() {
        if ch == '.' {
            if seen_dot {
                return None;
            }
            seen_dot = true;
        } else if ch.is_ascii_digit() {
            digits.push(ch);
            if seen_dot {
                scale += 1;
            }
        } else {
            return None;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let mantissa: i64 = digits.parse().ok()?;
    Some((sign * mantissa, scale))
}

fn option_u32(value: Option<u32>) -> TokenStream {
    match value {
        Some(v) => quote! { ::core::option::Option::Some(#v) },
        None => quote! { ::core::option::Option::None },
    }
}

fn option_string(value: Option<&str>) -> TokenStream {
    match value {
        Some(s) => {
            let lit = syn::LitStr::new(s, proc_macro2::Span::call_site());
            quote! { ::core::option::Option::Some(::std::string::String::from(#lit)) }
        }
        None => quote! { ::core::option::Option::None },
    }
}

fn option_string_vec(value: Option<&[String]>) -> TokenStream {
    match value {
        Some(items) => {
            let lits = items
                .iter()
                .map(|s| syn::LitStr::new(s, proc_macro2::Span::call_site()));
            quote! {
                ::core::option::Option::Some(::std::vec![ #( ::std::string::String::from(#lits) ),* ])
            }
        }
        None => quote! { ::core::option::Option::None },
    }
}

/// Default name for a field or variant case based on the `#[schema(rename_all
/// = …)]` strategy on the enclosing type.
pub fn default_name_for(ident: &syn::Ident, strategy: RenameAll) -> String {
    apply_rename_all(&ident.to_string(), strategy)
}

fn apply_rename_all(input: &str, strategy: RenameAll) -> String {
    use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
    match strategy {
        RenameAll::Kebab => input.to_kebab_case(),
        RenameAll::Snake => input.to_snake_case(),
        RenameAll::Camel => input.to_lower_camel_case(),
        RenameAll::Pascal => input.to_upper_camel_case(),
        RenameAll::ScreamingSnake => input.to_shouty_snake_case(),
    }
}

/// Inject `T: IntoSchema + FromSchema` (or `T: IntoSchema` only when
/// `add_from = false`) bounds for every generic type parameter into the
/// existing `where` clause. Lifetime / const params are left untouched.
pub fn add_trait_bounds(generics: &Generics, add_from: bool, add_into: bool) -> Generics {
    let mut g = generics.clone();
    let private = private();
    for p in &mut g.params {
        if let GenericParam::Type(t) = p {
            if add_into {
                t.bounds.push(syn::parse_quote!(#private::IntoSchema));
            }
            if add_from {
                t.bounds.push(syn::parse_quote!(#private::FromSchema));
            }
        }
    }
    g
}
