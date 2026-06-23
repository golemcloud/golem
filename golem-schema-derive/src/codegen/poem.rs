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

//! `#[derive(PoemSchema)]` — a schema-only `poem_openapi::types::Type`
//! implementation whose registered `MetaSchema` mirrors the type's **serde**
//! wire representation, while `ToJSON` / `ParseFromJSON` keep delegating to
//! `serde_json` (so the wire format is unchanged).
//!
//! The source of truth for the emitted schema is the `#[serde(...)]` attribute
//! surface, **not** poem's own `#[derive(Object/Union/Enum)]` and **not** the
//! schema model's `#[schema(...)]` attributes. Any serde attribute that would
//! make the schema diverge from the wire format (and that this derive does not
//! model) is rejected at compile time, so the schema can never silently drift
//! from serde.
//!
//! Field/variant body schemas are obtained by delegating to the field type's
//! own `poem_openapi::types::Type` implementation
//! (`<T as Type>::schema_ref()` / `register()`), which already handles
//! `Option<T>`, `Box<T>`, `Vec<T>`, `BTreeMap<String, V>`, primitives, etc.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Field, Fields, Ident, LitStr, Type,
    punctuated::Punctuated, token::Comma,
};

/// serde container-level attributes relevant to the schema shape.
#[derive(Default)]
struct SerdeTypeAttrs {
    tag: Option<String>,
    content: Option<String>,
    rename_all: Option<RenameRule>,
    transparent: bool,
}

/// serde field-level attributes relevant to the schema shape.
#[derive(Default)]
struct SerdeFieldAttrs {
    rename: Option<String>,
    /// `#[serde(default)]` or `#[serde(default = "path")]` — makes the field
    /// optional for decoding (and therefore not `required` in the schema).
    has_default: bool,
}

/// serde variant-level attributes relevant to the schema shape.
#[derive(Default)]
struct SerdeVariantAttrs {
    rename: Option<String>,
}

pub fn expand_poem_schema(input: &DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "PoemSchema does not support generic types",
        ));
    }

    let ident = &input.ident;
    let serde = parse_serde_type_attrs(&input.attrs)?;
    let name = ident.to_string();

    match &input.data {
        Data::Struct(data) => expand_struct(ident, &name, &serde, data),
        Data::Enum(data) => expand_enum(ident, &name, &serde, data),
        Data::Union(_) => Err(syn::Error::new_spanned(
            ident,
            "PoemSchema does not support `union` types",
        )),
    }
}

// --- structs -------------------------------------------------------------

fn expand_struct(
    ident: &Ident,
    name: &str,
    serde: &SerdeTypeAttrs,
    data: &DataStruct,
) -> syn::Result<TokenStream> {
    match &data.fields {
        Fields::Named(named) => {
            if serde.transparent {
                // serde serializes a `transparent` struct as its single
                // non-skipped field, which would not match the object schema we
                // would emit here. Only single-field tuple newtypes are modelled.
                return Err(syn::Error::new_spanned(
                    ident,
                    "PoemSchema only supports `#[serde(transparent)]` on single-field tuple \
                     structs (e.g. `struct TypeId(String)`)",
                ));
            }
            let schema_body = object_schema_block(&named.named, serde.rename_all)?;
            Ok(emit_registered(ident, name, &schema_body, true))
        }
        Fields::Unnamed(unnamed) => {
            if unnamed.unnamed.len() == 1 {
                // A single-field tuple struct serializes (in serde's data
                // model) as its inner value, so delegate the schema to the
                // inner type. This covers `#[serde(transparent)]` newtypes
                // such as `TypeId(String)`.
                let inner = &unnamed.unnamed[0].ty;
                reject_tuple_type(inner)?;
                if is_option_type(inner) {
                    // An `Option` inner would need `nullable` applied to the
                    // delegated schema; poem's `Option<T>` is not nullable by
                    // itself, so reject rather than emit a non-nullable schema.
                    return Err(syn::Error::new_spanned(
                        inner,
                        "PoemSchema does not support a single-field tuple struct over `Option<_>` \
                         (the delegated schema would not be nullable); wrap it in a named struct",
                    ));
                }
                Ok(emit_transparent(ident, inner))
            } else {
                Err(syn::Error::new_spanned(
                    ident,
                    "PoemSchema only supports single-field tuple structs (transparent newtypes); \
                     multi-field tuple structs serialize as arrays and are not modelled",
                ))
            }
        }
        Fields::Unit => Err(syn::Error::new_spanned(
            ident,
            "PoemSchema does not support unit structs",
        )),
    }
}

// --- enums ---------------------------------------------------------------

fn expand_enum(
    ident: &Ident,
    name: &str,
    serde: &SerdeTypeAttrs,
    data: &DataEnum,
) -> syn::Result<TokenStream> {
    if serde.transparent {
        return Err(syn::Error::new_spanned(
            ident,
            "#[serde(transparent)] is not valid on an enum",
        ));
    }

    match (&serde.tag, &serde.content) {
        (Some(tag), Some(content)) => expand_adjacent_enum(ident, name, serde, data, tag, content),
        (Some(tag), None) => expand_internal_enum(ident, name, serde, data, tag),
        (None, Some(_)) => Err(syn::Error::new_spanned(
            ident,
            "`#[serde(content = ...)]` requires `tag = ...`",
        )),
        (None, None) => {
            // No tag/content: the only serde representation this derive models
            // is an all-unit enum, which serializes as a plain string enum.
            if data
                .variants
                .iter()
                .all(|v| matches!(v.fields, Fields::Unit))
            {
                expand_unit_enum(ident, name, serde, data)
            } else {
                Err(syn::Error::new_spanned(
                    ident,
                    "PoemSchema does not support externally-tagged enums with data variants; \
                     use `#[serde(tag = \"...\", content = \"...\")]` for an adjacently-tagged enum",
                ))
            }
        }
    }
}

fn expand_unit_enum(
    ident: &Ident,
    name: &str,
    serde: &SerdeTypeAttrs,
    data: &DataEnum,
) -> syn::Result<TokenStream> {
    let mut case_lits = Vec::new();
    for variant in &data.variants {
        let vattrs = parse_serde_variant_attrs(&variant.attrs)?;
        let case = resolve_variant_name(&variant.ident, &vattrs, serde.rename_all);
        case_lits.push(LitStr::new(&case, variant.ident.span()));
    }

    let schema_body = quote! {
        ::poem_openapi::registry::MetaSchema {
            enum_items: ::std::vec![
                #( ::serde_json::Value::String(::std::string::String::from(#case_lits)) ),*
            ],
            ..::poem_openapi::registry::MetaSchema::new("string")
        }
    };

    Ok(emit_registered(ident, name, &schema_body, false))
}

fn expand_adjacent_enum(
    ident: &Ident,
    name: &str,
    serde: &SerdeTypeAttrs,
    data: &DataEnum,
    tag: &str,
    content: &str,
) -> syn::Result<TokenStream> {
    let tag_lit = LitStr::new(tag, Span::call_site());
    let content_lit = LitStr::new(content, Span::call_site());

    let mut variant_blocks = Vec::new();
    for variant in &data.variants {
        let vattrs = parse_serde_variant_attrs(&variant.attrs)?;
        let case = resolve_variant_name(&variant.ident, &vattrs, serde.rename_all);
        let case_lit = LitStr::new(&case, variant.ident.span());

        let tag_prop = quote! {
            __vprops.push((
                #tag_lit,
                ::poem_openapi::registry::MetaSchemaRef::Inline(::std::boxed::Box::new(
                    ::poem_openapi::registry::MetaSchema {
                        enum_items: ::std::vec![
                            ::serde_json::Value::String(::std::string::String::from(#case_lit))
                        ],
                        ..::poem_openapi::registry::MetaSchema::new("string")
                    }
                )),
            ));
        };

        let block = match &variant.fields {
            Fields::Unit => quote! {{
                let mut __vprops: ::std::vec::Vec<(
                    &'static str,
                    ::poem_openapi::registry::MetaSchemaRef,
                )> = ::std::vec::Vec::new();
                #tag_prop
                __one_of.push(::poem_openapi::registry::MetaSchemaRef::Inline(
                    ::std::boxed::Box::new(::poem_openapi::registry::MetaSchema {
                        required: ::std::vec![#tag_lit],
                        properties: __vprops,
                        ..::poem_openapi::registry::MetaSchema::new("object")
                    }),
                ));
            }},
            Fields::Unnamed(unnamed) => {
                if unnamed.unnamed.len() != 1 {
                    return Err(syn::Error::new_spanned(
                        &variant.ident,
                        "PoemSchema only supports single-field tuple enum variants; \
                         multi-field tuple variants serialize as arrays and are not modelled",
                    ));
                }
                let inner = &unnamed.unnamed[0].ty;
                let content_register = register_expr(inner);
                let content_ref = schema_ref_expr(inner);
                quote! {{
                    #content_register
                    let mut __vprops: ::std::vec::Vec<(
                        &'static str,
                        ::poem_openapi::registry::MetaSchemaRef,
                    )> = ::std::vec::Vec::new();
                    #tag_prop
                    __vprops.push((
                        #content_lit,
                        #content_ref,
                    ));
                    __one_of.push(::poem_openapi::registry::MetaSchemaRef::Inline(
                        ::std::boxed::Box::new(::poem_openapi::registry::MetaSchema {
                            required: ::std::vec![#tag_lit, #content_lit],
                            properties: __vprops,
                            ..::poem_openapi::registry::MetaSchema::new("object")
                        }),
                    ));
                }}
            }
            Fields::Named(named) => {
                // The enum's `rename_all` applies to variant names only, never
                // to struct-variant field names (serde uses `rename_all_fields`
                // for that, which this derive rejects).
                let content_obj = object_schema_block(&named.named, None)?;
                quote! {{
                    let mut __vprops: ::std::vec::Vec<(
                        &'static str,
                        ::poem_openapi::registry::MetaSchemaRef,
                    )> = ::std::vec::Vec::new();
                    #tag_prop
                    let __content_obj = #content_obj;
                    __vprops.push((
                        #content_lit,
                        ::poem_openapi::registry::MetaSchemaRef::Inline(
                            ::std::boxed::Box::new(__content_obj),
                        ),
                    ));
                    __one_of.push(::poem_openapi::registry::MetaSchemaRef::Inline(
                        ::std::boxed::Box::new(::poem_openapi::registry::MetaSchema {
                            required: ::std::vec![#tag_lit, #content_lit],
                            properties: __vprops,
                            ..::poem_openapi::registry::MetaSchema::new("object")
                        }),
                    ));
                }}
            }
        };
        variant_blocks.push(block);
    }

    let schema_body = quote! {
        let mut __one_of: ::std::vec::Vec<::poem_openapi::registry::MetaSchemaRef> =
            ::std::vec::Vec::new();
        #(#variant_blocks)*
        ::poem_openapi::registry::MetaSchema {
            ty: "object",
            one_of: __one_of,
            ..::poem_openapi::registry::MetaSchema::ANY
        }
    };

    Ok(emit_registered(ident, name, &schema_body, true))
}

/// Internally-tagged enum (`#[serde(tag = "...")]` without `content`): each
/// variant serializes as an object that carries the tag property inline
/// alongside the variant's own fields (`{ "tag": "<case>", ...fields }`).
/// serde only supports struct and unit variants in this representation, so
/// newtype/tuple variants are rejected.
fn expand_internal_enum(
    ident: &Ident,
    name: &str,
    serde: &SerdeTypeAttrs,
    data: &DataEnum,
    tag: &str,
) -> syn::Result<TokenStream> {
    let tag_lit = LitStr::new(tag, Span::call_site());

    let mut variant_blocks = Vec::new();
    for variant in &data.variants {
        let vattrs = parse_serde_variant_attrs(&variant.attrs)?;
        let case = resolve_variant_name(&variant.ident, &vattrs, serde.rename_all);
        let case_lit = LitStr::new(&case, variant.ident.span());

        let tag_prop = quote! {
            __props.push((
                #tag_lit,
                ::poem_openapi::registry::MetaSchemaRef::Inline(::std::boxed::Box::new(
                    ::poem_openapi::registry::MetaSchema {
                        enum_items: ::std::vec![
                            ::serde_json::Value::String(::std::string::String::from(#case_lit))
                        ],
                        ..::poem_openapi::registry::MetaSchema::new("string")
                    }
                )),
            ));
        };

        let block = match &variant.fields {
            Fields::Unit => quote! {{
                let mut __props: ::std::vec::Vec<(
                    &'static str,
                    ::poem_openapi::registry::MetaSchemaRef,
                )> = ::std::vec::Vec::new();
                #tag_prop
                __one_of.push(::poem_openapi::registry::MetaSchemaRef::Inline(
                    ::std::boxed::Box::new(::poem_openapi::registry::MetaSchema {
                        required: ::std::vec![#tag_lit],
                        properties: __props,
                        ..::poem_openapi::registry::MetaSchema::new("object")
                    }),
                ));
            }},
            Fields::Named(named) => {
                // As with adjacently-tagged struct variants, the enum's
                // `rename_all` never applies to struct-variant field names.
                let (registers, prop_pushes, req_pushes) =
                    field_schema_parts(&named.named, None)?;
                quote! {{
                    #(#registers)*
                    let mut __props: ::std::vec::Vec<(
                        &'static str,
                        ::poem_openapi::registry::MetaSchemaRef,
                    )> = ::std::vec::Vec::new();
                    #tag_prop
                    #(#prop_pushes)*
                    let mut __required: ::std::vec::Vec<&'static str> =
                        ::std::vec![#tag_lit];
                    #(#req_pushes)*
                    __one_of.push(::poem_openapi::registry::MetaSchemaRef::Inline(
                        ::std::boxed::Box::new(::poem_openapi::registry::MetaSchema {
                            required: __required,
                            properties: __props,
                            ..::poem_openapi::registry::MetaSchema::new("object")
                        }),
                    ));
                }}
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "PoemSchema does not support newtype/tuple variants in \
                     internally-tagged enums; serde requires struct or unit variants",
                ));
            }
        };
        variant_blocks.push(block);
    }

    let schema_body = quote! {
        let mut __one_of: ::std::vec::Vec<::poem_openapi::registry::MetaSchemaRef> =
            ::std::vec::Vec::new();
        #(#variant_blocks)*
        ::poem_openapi::registry::MetaSchema {
            ty: "object",
            one_of: __one_of,
            ..::poem_openapi::registry::MetaSchema::ANY
        }
    };

    Ok(emit_registered(ident, name, &schema_body, true))
}

// --- shared codegen ------------------------------------------------------

/// Build a block expression that registers every field type and evaluates to
/// an object `MetaSchema` describing the given named fields.
fn object_schema_block(
    fields: &Punctuated<Field, Comma>,
    rename_all: Option<RenameRule>,
) -> syn::Result<TokenStream> {
    let (registers, prop_pushes, req_pushes) = field_schema_parts(fields, rename_all)?;

    Ok(quote! {{
        #(#registers)*
        let mut __props: ::std::vec::Vec<(
            &'static str,
            ::poem_openapi::registry::MetaSchemaRef,
        )> = ::std::vec::Vec::new();
        #(#prop_pushes)*
        let mut __required: ::std::vec::Vec<&'static str> = ::std::vec::Vec::new();
        #(#req_pushes)*
        ::poem_openapi::registry::MetaSchema {
            required: __required,
            properties: __props,
            ..::poem_openapi::registry::MetaSchema::new("object")
        }
    }})
}

/// Build, for a set of named fields, the per-field token fragments shared by
/// struct schemas and tagged struct-variant schemas:
///
/// - `registers`: statements that register each field type's component;
/// - `prop_pushes`: statements that push `(name, schema_ref)` into `__props`;
/// - `req_pushes`: statements that push required field names into `__required`.
///
/// Both `__props` and `__required` must be in scope where these fragments are
/// emitted.
fn field_schema_parts(
    fields: &Punctuated<Field, Comma>,
    rename_all: Option<RenameRule>,
) -> syn::Result<(Vec<TokenStream>, Vec<TokenStream>, Vec<TokenStream>)> {
    let mut registers = Vec::new();
    let mut prop_pushes = Vec::new();
    let mut req_pushes = Vec::new();

    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "expected a named field"))?;
        let ty = &field.ty;

        let fattrs = parse_serde_field_attrs(&field.attrs)?;
        let name = resolve_field_name(ident, &fattrs, rename_all);
        let name_lit = LitStr::new(&name, ident.span());
        let is_opt = is_option_type(ty);

        registers.push(register_expr(ty));

        let prop_ref = schema_ref_expr(ty);
        prop_pushes.push(quote! { __props.push((#name_lit, #prop_ref)); });

        if !is_opt && !fattrs.has_default {
            req_pushes.push(quote! { __required.push(#name_lit); });
        }
    }

    Ok((registers, prop_pushes, req_pushes))
}

/// Emit `Type` (registering a named component built from `schema_body`),
/// `ToJSON`/`ParseFromJSON` (serde-backed), and optionally `IsObjectType`.
fn emit_registered(
    ident: &Ident,
    name: &str,
    schema_body: &TokenStream,
    is_object: bool,
) -> TokenStream {
    let name_lit = LitStr::new(name, ident.span());
    let json = json_codec(ident);
    let is_object_impl = if is_object {
        quote! { impl ::poem_openapi::types::IsObjectType for #ident {} }
    } else {
        quote! {}
    };

    quote! {
        impl ::poem_openapi::types::Type for #ident {
            const IS_REQUIRED: bool = true;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(#name_lit)
            }

            fn schema_ref() -> ::poem_openapi::registry::MetaSchemaRef {
                ::poem_openapi::registry::MetaSchemaRef::Reference(
                    ::std::string::String::from(#name_lit),
                )
            }

            fn register(registry: &mut ::poem_openapi::registry::Registry) {
                registry.create_schema::<Self, _>(
                    ::std::string::String::from(#name_lit),
                    |registry| { #schema_body },
                );
            }

            fn as_raw_value(&self) -> ::core::option::Option<&Self::RawValueType> {
                ::core::option::Option::Some(self)
            }

            fn raw_element_iter<'a>(
                &'a self,
            ) -> ::std::boxed::Box<
                dyn ::core::iter::Iterator<Item = &'a Self::RawElementValueType> + 'a,
            > {
                ::std::boxed::Box::new(::core::iter::IntoIterator::into_iter(self.as_raw_value()))
            }
        }

        #json
        #is_object_impl
    }
}

/// Emit a `Type` that delegates its schema entirely to the inner type (used
/// for transparent single-field tuple structs).
fn emit_transparent(ident: &Ident, inner: &Type) -> TokenStream {
    let json = json_codec(ident);
    quote! {
        impl ::poem_openapi::types::Type for #ident {
            const IS_REQUIRED: bool = <#inner as ::poem_openapi::types::Type>::IS_REQUIRED;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> ::std::borrow::Cow<'static, str> {
                <#inner as ::poem_openapi::types::Type>::name()
            }

            fn schema_ref() -> ::poem_openapi::registry::MetaSchemaRef {
                <#inner as ::poem_openapi::types::Type>::schema_ref()
            }

            fn register(registry: &mut ::poem_openapi::registry::Registry) {
                <#inner as ::poem_openapi::types::Type>::register(registry);
            }

            fn as_raw_value(&self) -> ::core::option::Option<&Self::RawValueType> {
                ::core::option::Option::Some(self)
            }

            fn raw_element_iter<'a>(
                &'a self,
            ) -> ::std::boxed::Box<
                dyn ::core::iter::Iterator<Item = &'a Self::RawElementValueType> + 'a,
            > {
                ::std::boxed::Box::new(::core::iter::IntoIterator::into_iter(self.as_raw_value()))
            }
        }

        #json
    }
}

fn json_codec(ident: &Ident) -> TokenStream {
    quote! {
        impl ::poem_openapi::types::ToJSON for #ident {
            fn to_json(&self) -> ::core::option::Option<::serde_json::Value> {
                ::serde_json::to_value(self).ok()
            }
        }

        impl ::poem_openapi::types::ParseFromJSON for #ident {
            fn parse_from_json(
                value: ::core::option::Option<::serde_json::Value>,
            ) -> ::poem_openapi::types::ParseResult<Self> {
                ::core::result::Result::Ok(::serde_json::from_value(
                    value.unwrap_or_default(),
                )?)
            }
        }
    }
}

// --- type helpers --------------------------------------------------------

/// Build the `MetaSchemaRef` expression for a field/variant-content type,
/// applying serde's `Option<T>` → nullable rule (poem's `Option<T>` is not
/// nullable by itself, so we merge `nullable: true` explicitly).
fn type_schema_ref_expr(ty: &Type) -> TokenStream {
    if is_option_type(ty) {
        quote! {
            ::poem_openapi::registry::MetaSchemaRef::merge(
                <#ty as ::poem_openapi::types::Type>::schema_ref(),
                ::poem_openapi::registry::MetaSchema {
                    nullable: true,
                    ..::poem_openapi::registry::MetaSchema::ANY
                },
            )
        }
    } else {
        quote! { <#ty as ::poem_openapi::types::Type>::schema_ref() }
    }
}

/// Register statement(s) for a field/variant-content type. Types that contain
/// a tuple (which has no `poem_openapi::types::Type` impl) are walked
/// structurally so each leaf component is still registered; all other types
/// delegate to `<T as Type>::register`.
fn register_expr(ty: &Type) -> TokenStream {
    if type_contains_tuple(ty) {
        synth_register(ty)
    } else {
        quote! { <#ty as ::poem_openapi::types::Type>::register(registry); }
    }
}

/// `MetaSchemaRef` expression for a field/variant-content type. Tuple-bearing
/// types are synthesized as array schemas (serde serializes tuples as JSON
/// arrays); all other types delegate to their own `Type::schema_ref` (with the
/// `Option<T>` → nullable rule applied by [`type_schema_ref_expr`]).
fn schema_ref_expr(ty: &Type) -> TokenStream {
    if type_contains_tuple(ty) {
        synth_schema_ref(ty)
    } else {
        type_schema_ref_expr(ty)
    }
}

/// Whether `ty` is, or structurally contains, a (non-empty) tuple — looking
/// through the `Vec<_>` / `Option<_>` / `Box<_>` / slice / array wrappers this
/// derive knows how to synthesize.
fn type_contains_tuple(ty: &Type) -> bool {
    match ty {
        Type::Tuple(t) => !t.elems.is_empty(),
        Type::Slice(s) => type_contains_tuple(&s.elem),
        Type::Array(a) => type_contains_tuple(&a.elem),
        Type::Paren(p) => type_contains_tuple(&p.elem),
        Type::Group(g) => type_contains_tuple(&g.elem),
        _ => container_inner(ty).is_some_and(type_contains_tuple),
    }
}

/// Recursively register the leaf component types reachable through tuples and
/// the known wrapper types.
fn synth_register(ty: &Type) -> TokenStream {
    match ty {
        Type::Tuple(t) => {
            let regs = t.elems.iter().map(synth_register);
            quote! { #(#regs)* }
        }
        Type::Slice(s) => synth_register(&s.elem),
        Type::Array(a) => synth_register(&a.elem),
        Type::Paren(p) => synth_register(&p.elem),
        Type::Group(g) => synth_register(&g.elem),
        _ => match container_inner(ty) {
            Some(inner) => synth_register(inner),
            None => quote! { <#ty as ::poem_openapi::types::Type>::register(registry); },
        },
    }
}

/// Synthesize a `MetaSchemaRef` for a tuple-bearing type. Tuples become array
/// schemas with `min_items`/`max_items` fixed to the arity; `Vec`/slice become
/// arrays; `Option` adds `nullable`; `Box` is transparent; everything else
/// delegates to the type's own `schema_ref`.
fn synth_schema_ref(ty: &Type) -> TokenStream {
    match ty {
        Type::Tuple(t) => {
            let arity = t.elems.len();
            let arity_lit = proc_macro2::Literal::usize_unsuffixed(arity);
            let homogeneous = {
                let mut iter = t.elems.iter();
                match iter.next() {
                    Some(first) => {
                        let first_str = quote! { #first }.to_string();
                        iter.all(|e| quote! { #e }.to_string() == first_str)
                    }
                    None => true,
                }
            };
            let items_ref = if homogeneous {
                synth_schema_ref(&t.elems[0])
            } else {
                let elem_refs = t.elems.iter().map(synth_schema_ref);
                quote! {
                    ::poem_openapi::registry::MetaSchemaRef::Inline(::std::boxed::Box::new(
                        ::poem_openapi::registry::MetaSchema {
                            one_of: ::std::vec![ #(#elem_refs),* ],
                            ..::poem_openapi::registry::MetaSchema::ANY
                        }
                    ))
                }
            };
            quote! {
                ::poem_openapi::registry::MetaSchemaRef::Inline(::std::boxed::Box::new(
                    ::poem_openapi::registry::MetaSchema {
                        items: ::core::option::Option::Some(::std::boxed::Box::new(#items_ref)),
                        min_items: ::core::option::Option::Some(#arity_lit),
                        max_items: ::core::option::Option::Some(#arity_lit),
                        ..::poem_openapi::registry::MetaSchema::new("array")
                    }
                ))
            }
        }
        Type::Slice(s) => array_ref(synth_schema_ref(&s.elem)),
        Type::Array(a) => array_ref(synth_schema_ref(&a.elem)),
        Type::Paren(p) => synth_schema_ref(&p.elem),
        Type::Group(g) => synth_schema_ref(&g.elem),
        _ => match container_kind(ty) {
            Some((Container::Vec, inner)) => array_ref(synth_schema_ref(inner)),
            Some((Container::Box, inner)) => synth_schema_ref(inner),
            Some((Container::Option, inner)) => {
                let inner_ref = synth_schema_ref(inner);
                quote! {
                    ::poem_openapi::registry::MetaSchemaRef::merge(
                        #inner_ref,
                        ::poem_openapi::registry::MetaSchema {
                            nullable: true,
                            ..::poem_openapi::registry::MetaSchema::ANY
                        },
                    )
                }
            }
            None => quote! { <#ty as ::poem_openapi::types::Type>::schema_ref() },
        },
    }
}

/// An inline `array` `MetaSchemaRef` whose items are `inner_ref`.
fn array_ref(inner_ref: TokenStream) -> TokenStream {
    quote! {
        ::poem_openapi::registry::MetaSchemaRef::Inline(::std::boxed::Box::new(
            ::poem_openapi::registry::MetaSchema {
                items: ::core::option::Option::Some(::std::boxed::Box::new(#inner_ref)),
                ..::poem_openapi::registry::MetaSchema::new("array")
            }
        ))
    }
}

/// The wrapper kinds whose schema this derive can synthesize structurally.
#[derive(Clone, Copy)]
enum Container {
    Vec,
    Option,
    Box,
}

/// If `ty` is a `Vec<T>` / `Option<T>` / `Box<T>`, return its kind and `T`.
fn container_kind(ty: &Type) -> Option<(Container, &Type)> {
    let Type::Path(tp) = ty else { return None };
    if tp.qself.is_some() {
        return None;
    }
    let seg = tp.path.segments.last()?;
    let kind = match seg.ident.to_string().as_str() {
        "Vec" => Container::Vec,
        "Option" => Container::Option,
        "Box" => Container::Box,
        _ => return None,
    };
    if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
        for arg in &args.args {
            if let syn::GenericArgument::Type(inner) = arg {
                return Some((kind, inner));
            }
        }
    }
    None
}

/// The single inner type of a `Vec<T>` / `Option<T>` / `Box<T>`, if any.
fn container_inner(ty: &Type) -> Option<&Type> {
    container_kind(ty).map(|(_, inner)| inner)
}

fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(tp) = ty
        && tp.qself.is_none()
        && let Some(seg) = tp.path.segments.last()
    {
        return seg.ident == "Option";
    }
    false
}

fn reject_tuple_type(ty: &Type) -> syn::Result<()> {
    if matches!(ty, Type::Tuple(_)) {
        return Err(syn::Error::new_spanned(
            ty,
            "PoemSchema does not support tuple field types (no `poem_openapi::types::Type` impl); \
             wrap the value in a named struct",
        ));
    }
    Ok(())
}

fn resolve_field_name(
    ident: &Ident,
    attrs: &SerdeFieldAttrs,
    rename_all: Option<RenameRule>,
) -> String {
    if let Some(rename) = &attrs.rename {
        return rename.clone();
    }
    let raw = ident.to_string();
    let raw = raw.strip_prefix("r#").unwrap_or(&raw);
    match rename_all {
        Some(rule) => rule.apply_to_field(raw),
        None => raw.to_string(),
    }
}

fn resolve_variant_name(
    ident: &Ident,
    attrs: &SerdeVariantAttrs,
    rename_all: Option<RenameRule>,
) -> String {
    if let Some(rename) = &attrs.rename {
        return rename.clone();
    }
    let raw = ident.to_string();
    match rename_all {
        Some(rule) => rule.apply_to_variant(&raw),
        None => raw,
    }
}

// --- serde attribute parsing --------------------------------------------

fn parse_serde_type_attrs(attrs: &[Attribute]) -> syn::Result<SerdeTypeAttrs> {
    let mut out = SerdeTypeAttrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            match key.as_str() {
                "tag" => out.tag = Some(meta.value()?.parse::<LitStr>()?.value()),
                "content" => out.content = Some(meta.value()?.parse::<LitStr>()?.value()),
                "rename_all" => {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.rename_all = Some(RenameRule::from_str(&lit.value(), lit.span())?);
                }
                "transparent" => out.transparent = true,
                // Affects only the deserializer error message, not the shape.
                "expecting" => {
                    if meta.input.peek(syn::Token![=]) {
                        let _: LitStr = meta.value()?.parse()?;
                    }
                }
                "deny_unknown_fields" => {
                    // Changes what `ParseFromJSON` accepts (rejects extra
                    // properties) but we do not emit `additionalProperties:
                    // false`, so the schema would be more permissive than the
                    // wire format. Reject rather than drift.
                    return Err(meta.error(
                        "PoemSchema does not support `#[serde(deny_unknown_fields)]` \
                         (it is not reflected in the generated schema)",
                    ));
                }
                "untagged" => {
                    return Err(
                        meta.error("PoemSchema does not support `#[serde(untagged)]` enums")
                    );
                }
                "rename_all_fields" => {
                    return Err(meta
                        .error("PoemSchema does not support `#[serde(rename_all_fields = ...)]`"));
                }
                other => {
                    return Err(meta.error(format!(
                        "PoemSchema does not support `#[serde({other})]` on a type"
                    )));
                }
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn parse_serde_field_attrs(attrs: &[Attribute]) -> syn::Result<SerdeFieldAttrs> {
    let mut out = SerdeFieldAttrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            match key.as_str() {
                "rename" => {
                    if !meta.input.peek(syn::Token![=]) {
                        return Err(meta.error(
                            "PoemSchema does not support split `#[serde(rename(serialize = ..., \
                             deserialize = ...))]`",
                        ));
                    }
                    out.rename = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                "default" => {
                    out.has_default = true;
                    if meta.input.peek(syn::Token![=]) {
                        let _: LitStr = meta.value()?.parse()?;
                    }
                }
                // Parsed but irrelevant to the schema shape this derive models.
                "skip_serializing_if" => {
                    let _: LitStr = meta.value()?.parse()?;
                }
                other => {
                    return Err(meta.error(format!(
                        "PoemSchema does not support `#[serde({other})]` on a field"
                    )));
                }
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn parse_serde_variant_attrs(attrs: &[Attribute]) -> syn::Result<SerdeVariantAttrs> {
    let mut out = SerdeVariantAttrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            match key.as_str() {
                "rename" => {
                    if !meta.input.peek(syn::Token![=]) {
                        return Err(meta.error(
                            "PoemSchema does not support split `#[serde(rename(serialize = ..., \
                             deserialize = ...))]`",
                        ));
                    }
                    out.rename = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                other => {
                    return Err(meta.error(format!(
                        "PoemSchema does not support `#[serde({other})]` on a variant"
                    )));
                }
            }
            Ok(())
        })?;
    }
    Ok(out)
}

// --- serde rename rules (ported from serde_derive) ----------------------

#[derive(Clone, Copy)]
enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn from_str(raw: &str, span: Span) -> syn::Result<Self> {
        match raw {
            "lowercase" => Ok(Self::Lower),
            "UPPERCASE" => Ok(Self::Upper),
            "PascalCase" => Ok(Self::Pascal),
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Ok(Self::ScreamingKebab),
            other => Err(syn::Error::new(
                span,
                format!("unsupported serde rename_all rule `{other}`"),
            )),
        }
    }

    /// Apply to a variant name (input is a PascalCase identifier).
    fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => variant[..1].to_ascii_lowercase() + &variant[1..],
            Self::Snake => {
                let mut snake = String::new();
                for (i, ch) in variant.char_indices() {
                    if i > 0 && ch.is_uppercase() {
                        snake.push('_');
                    }
                    snake.push(ch.to_ascii_lowercase());
                }
                snake
            }
            Self::ScreamingSnake => Self::Snake.apply_to_variant(variant).to_ascii_uppercase(),
            Self::Kebab => Self::Snake.apply_to_variant(variant).replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake
                .apply_to_variant(variant)
                .replace('_', "-"),
        }
    }

    /// Apply to a field name (input is a snake_case identifier).
    fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => {
                let mut pascal = String::new();
                let mut capitalize = true;
                for ch in field.chars() {
                    if ch == '_' {
                        capitalize = true;
                    } else if capitalize {
                        pascal.push(ch.to_ascii_uppercase());
                        capitalize = false;
                    } else {
                        pascal.push(ch);
                    }
                }
                pascal
            }
            Self::Camel => {
                let pascal = Self::Pascal.apply_to_field(field);
                pascal[..1].to_ascii_lowercase() + &pascal[1..]
            }
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake.apply_to_field(field).replace('_', "-"),
        }
    }
}
