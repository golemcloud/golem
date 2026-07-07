// Copyright 2024-2026 Golem Cloud
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

//! `#[derive(ToolError)]` with the `#[tool_error(kind, exit_code)]` helper
//! attribute. Parsing produces the [`ToolErrorIr`], from which an
//! `impl ToolErrorSchema` exposing the per-variant error cases is synthesized.

use crate::tool::doc::parse_doc_full;
use crate::tool::helpers::{SeenKeys, expr_str, expr_u8, to_kebab_case};
use crate::tool::ir::{
    ErrorKindIr, ToolErrorIr, ToolErrorNoPayloadStyleIr, ToolErrorPayloadIr, ToolErrorVariantIr,
};
use crate::tool::synthesis::{doc_tokens, error_kind_tokens};
use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Error, Expr, Fields};

pub fn derive_tool_error_impl(input: TokenStream) -> TokenStream {
    let derive_input = syn::parse_macro_input!(input as DeriveInput);
    match parse_tool_error(&derive_input) {
        Ok(ir) => synthesize_tool_error(&ir),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Builds `impl golem_rust::agentic::ToolErrorSchema for <Enum>` from the IR.
fn synthesize_tool_error(ir: &ToolErrorIr) -> TokenStream {
    let enum_ident = &ir.enum_ident;
    let type_name = enum_ident.to_string();
    let cases = ir.variants.iter().map(|variant| {
        let name = to_kebab_case(&variant.variant_ident.to_string());
        let doc = doc_tokens(&variant.doc);
        let kind = error_kind_tokens(variant.kind);
        let exit_code = variant.exit_code;
        let payload = match &variant.payload {
            ToolErrorPayloadIr::None { .. } => quote! { ::std::option::Option::None },
            ToolErrorPayloadIr::Single { ty, .. } => {
                let position = format!("error {name} payload");
                quote! {
                    ::std::option::Option::Some(
                        golem_rust::agentic::tool_value_schema::<#ty>(#position)?
                    )
                }
            }
        };
        quote! {
            golem_rust::agentic::ExtendedErrorCase {
                name: #name.to_string(),
                doc: #doc,
                kind: #kind,
                exit_code: #exit_code,
                payload: #payload,
            }
        }
    });
    let schema_cases = ir.variants.iter().map(|variant| {
        let name = to_kebab_case(&variant.variant_ident.to_string());
        let payload = match &variant.payload {
            ToolErrorPayloadIr::None { .. } => quote! { ::std::option::Option::None },
            ToolErrorPayloadIr::Single { ty, .. } => {
                quote! { ::std::option::Option::Some(<#ty as golem_rust::IntoSchema>::register_in(__builder)) }
            }
        };
        quote! {
            golem_rust::schema::VariantCaseType {
                name: #name.to_string(),
                payload: #payload,
                metadata: ::std::default::Default::default(),
            }
        }
    });
    let to_value_arms = ir.variants.iter().enumerate().map(|(idx, variant)| {
        let variant_ident = &variant.variant_ident;
        let idx = idx as u32;
        match &variant.payload {
            ToolErrorPayloadIr::None { style } => {
                let pattern = no_payload_pattern(variant_ident, *style);
                quote! {
                #pattern => golem_rust::SchemaValue::Variant(
                    golem_rust::schema::VariantValuePayload {
                        case: #idx,
                        payload: ::std::option::Option::None,
                    }
                )
                }
            }
            ToolErrorPayloadIr::Single {
                field_ident: None, ..
            } => quote! {
                Self::#variant_ident(__payload) => golem_rust::SchemaValue::Variant(
                    golem_rust::schema::VariantValuePayload {
                        case: #idx,
                        payload: ::std::option::Option::Some(::std::boxed::Box::new(
                            golem_rust::IntoSchema::to_value(__payload)
                        )),
                    }
                )
            },
            ToolErrorPayloadIr::Single {
                field_ident: Some(field_ident),
                ..
            } => quote! {
                Self::#variant_ident { #field_ident } => golem_rust::SchemaValue::Variant(
                    golem_rust::schema::VariantValuePayload {
                        case: #idx,
                        payload: ::std::option::Option::Some(::std::boxed::Box::new(
                            golem_rust::IntoSchema::to_value(#field_ident)
                        )),
                    }
                )
            },
        }
    });
    let from_value_arms = ir.variants.iter().enumerate().map(|(idx, variant)| {
        let variant_ident = &variant.variant_ident;
        let idx = idx as u32;
        match &variant.payload {
            ToolErrorPayloadIr::None { style } => {
                let constructor = no_payload_constructor(variant_ident, *style);
                quote! {
                #idx => {
                    if __payload.payload.is_some() {
                        return ::std::result::Result::Err(golem_rust::schema::FromSchemaError::custom(
                            "tool error variant unexpectedly carried a payload"
                        ));
                    }
                    ::std::result::Result::Ok(#constructor)
                }
                }
            }
            ToolErrorPayloadIr::Single { ty, field_ident: None } => quote! {
                #idx => {
                    let __value = __payload.payload.as_deref().ok_or_else(|| {
                        golem_rust::schema::FromSchemaError::custom("tool error variant is missing its payload")
                    })?;
                    ::std::result::Result::Ok(Self::#variant_ident(<#ty as golem_rust::FromSchema>::from_value(__value)?))
                }
            },
            ToolErrorPayloadIr::Single { ty, field_ident: Some(field_ident) } => quote! {
                #idx => {
                    let __value = __payload.payload.as_deref().ok_or_else(|| {
                        golem_rust::schema::FromSchemaError::custom("tool error variant is missing its payload")
                    })?;
                    ::std::result::Result::Ok(Self::#variant_ident {
                        #field_ident: <#ty as golem_rust::FromSchema>::from_value(__value)?,
                    })
                }
            },
        }
    });
    let to_error_payload_arms = ir.variants.iter().map(|variant| {
        let variant_ident = &variant.variant_ident;
        match &variant.payload {
            ToolErrorPayloadIr::None { style } => {
                let pattern = no_payload_pattern(variant_ident, *style);
                quote! {
                #pattern => {
                    golem_rust::IntoTypedSchemaValue::into_typed_schema_value(&())
                        .map_err(|__err| __err.to_string())
                }
                }
            }
            ToolErrorPayloadIr::Single {
                field_ident: None, ..
            } => quote! {
                Self::#variant_ident(__payload) => {
                    golem_rust::IntoTypedSchemaValue::into_typed_schema_value(__payload)
                        .map_err(|__err| __err.to_string())
                }
            },
            ToolErrorPayloadIr::Single {
                field_ident: Some(field_ident),
                ..
            } => quote! {
                Self::#variant_ident { #field_ident } => {
                    golem_rust::IntoTypedSchemaValue::into_typed_schema_value(#field_ident)
                        .map_err(|__err| __err.to_string())
                }
            },
        }
    });
    let from_error_payload_arms = ir.variants.iter().map(|variant| {
        let variant_ident = &variant.variant_ident;
        match &variant.payload {
            ToolErrorPayloadIr::None { style } => {
                let constructor = no_payload_constructor(variant_ident, *style);
                quote! {
                if <() as golem_rust::FromSchema>::from_value(__value.value()).is_ok() {
                    return ::std::result::Result::Ok(#constructor);
                }
                }
            }
            ToolErrorPayloadIr::Single { ty, field_ident: None } => quote! {
                if let ::std::result::Result::Ok(__payload) = <#ty as golem_rust::FromSchema>::from_value(__value.value()) {
                    return ::std::result::Result::Ok(Self::#variant_ident(__payload));
                }
            },
            ToolErrorPayloadIr::Single { ty, field_ident: Some(field_ident) } => quote! {
                if let ::std::result::Result::Ok(__payload) = <#ty as golem_rust::FromSchema>::from_value(__value.value()) {
                    return ::std::result::Result::Ok(Self::#variant_ident { #field_ident: __payload });
                }
            },
        }
    });
    let variant_count = ir.variants.len() as u32;
    quote! {
        impl golem_rust::agentic::ToolErrorSchema for #enum_ident {
            fn error_cases() -> ::std::result::Result<
                ::std::vec::Vec<golem_rust::agentic::ExtendedErrorCase>,
                golem_rust::agentic::ToolBuildError,
            > {
                ::std::result::Result::Ok(::std::vec![ #(#cases),* ])
            }

            fn to_error_payload_value(&self) -> ::std::result::Result<golem_rust::TypedSchemaValue, ::std::string::String> {
                match self {
                    #(#to_error_payload_arms),*
                }
            }

            fn from_error_payload_value(
                __value: golem_rust::TypedSchemaValue,
            ) -> ::std::result::Result<Self, ::std::string::String> {
                #(#from_error_payload_arms)*
                ::std::result::Result::Err("remote tool error payload did not match any declared error case".to_string())
            }
        }

        impl golem_rust::IntoSchema for #enum_ident {
            fn type_id() -> golem_rust::schema::TypeId {
                golem_rust::schema::TypeId::new(
                    golem_rust::schema::conversion::normalize_type_path(::std::any::type_name::<Self>())
                )
            }

            fn register_in(__builder: &mut golem_rust::schema::SchemaBuilder) -> golem_rust::SchemaType {
                let __id = <Self as golem_rust::IntoSchema>::type_id();
                if __builder.is_registered(&__id) {
                    return golem_rust::SchemaType::Ref {
                        id: __id,
                        metadata: ::std::default::Default::default(),
                    };
                }
                __builder.reserve(__id.clone());
                let __body = golem_rust::SchemaType::Variant {
                    cases: ::std::vec![ #(#schema_cases),* ],
                    metadata: ::std::default::Default::default(),
                };
                __builder.commit(
                    __id.clone(),
                    ::std::option::Option::Some(#type_name.to_string()),
                    ::std::default::Default::default(),
                    __body,
                );
                golem_rust::SchemaType::Ref {
                    id: __id,
                    metadata: ::std::default::Default::default(),
                }
            }

            fn to_value(&self) -> golem_rust::SchemaValue {
                match self {
                    #(#to_value_arms),*
                }
            }
        }

        impl golem_rust::FromSchema for #enum_ident {
            fn from_value(__value: &golem_rust::SchemaValue) -> ::std::result::Result<Self, golem_rust::schema::FromSchemaError> {
                match __value {
                    golem_rust::SchemaValue::Variant(__payload) => match __payload.case {
                        #(#from_value_arms),*,
                        __idx => ::std::result::Result::Err(golem_rust::schema::FromSchemaError::out_of_range(
                            __idx,
                            #variant_count,
                            "tool error variant",
                        )),
                    },
                    __other => ::std::result::Result::Err(golem_rust::schema::FromSchemaError::shape_mismatch(
                        "variant",
                        golem_rust::schema::conversion::value_kind(__other),
                        "tool error",
                    )),
                }
            }
        }
    }
    .into()
}

/// Parses a `#[derive(ToolError)]` enum into its IR.
pub fn parse_tool_error(input: &DeriveInput) -> Result<ToolErrorIr, Error> {
    let Data::Enum(data) = &input.data else {
        return Err(Error::new(
            input.span(),
            "#[derive(ToolError)] can only be applied to enums",
        ));
    };

    // `#[tool_error(...)]` describes a single error case, so it belongs on a
    // variant; on the enum itself it would be silently ignored. `#[example]` is
    // collected per variant, so it is likewise misplaced on the enum.
    if let Some(attr) = find_tool_error_attr(&input.attrs)? {
        return Err(Error::new(
            attr.span(),
            "#[tool_error(...)] may only be used on variants of a #[derive(ToolError)] enum",
        ));
    }
    if let Some(attr) = find_example_attr(&input.attrs) {
        return Err(Error::new(
            attr.span(),
            "#[example] may only be used on variants of a #[derive(ToolError)] enum",
        ));
    }

    let mut variants = Vec::new();
    for variant in &data.variants {
        // A `#[tool_error(...)]` or `#[example]` on a variant's field would be
        // silently ignored.
        for field in variant.fields.iter() {
            if let Some(attr) = find_tool_error_attr(&field.attrs)? {
                return Err(Error::new(
                    attr.span(),
                    "#[tool_error(...)] may only be used on variants of a #[derive(ToolError)] enum",
                ));
            }
            if let Some(attr) = find_example_attr(&field.attrs) {
                return Err(Error::new(
                    attr.span(),
                    "#[example] may only be used on variants of a #[derive(ToolError)] enum",
                ));
            }
        }
        let attr = find_tool_error_attr(&variant.attrs)?.ok_or_else(|| {
            Error::new(
                variant.span(),
                format!(
                    "variant `{}` is missing #[tool_error(kind = \"...\", exit_code = ...)]",
                    variant.ident
                ),
            )
        })?;
        let (kind, exit_code) = parse_tool_error_attr(attr)?;
        let payload = parse_payload(&variant.fields)?;
        variants.push(ToolErrorVariantIr {
            variant_ident: variant.ident.clone(),
            doc: parse_doc_full(&variant.attrs)?,
            kind,
            exit_code,
            payload,
        });
    }

    Ok(ToolErrorIr {
        enum_ident: input.ident.clone(),
        variants,
    })
}

/// Maps a variant's fields to its payload: unit / zero fields carry no payload,
/// exactly one field carries that field's type, and two or more fields are a
/// compile error (no synthetic record is generated).
fn parse_payload(fields: &Fields) -> Result<ToolErrorPayloadIr, Error> {
    match fields {
        Fields::Unit => Ok(ToolErrorPayloadIr::None {
            style: ToolErrorNoPayloadStyleIr::Unit,
        }),
        Fields::Unnamed(f) if f.unnamed.is_empty() => Ok(ToolErrorPayloadIr::None {
            style: ToolErrorNoPayloadStyleIr::Tuple,
        }),
        Fields::Named(f) if f.named.is_empty() => Ok(ToolErrorPayloadIr::None {
            style: ToolErrorNoPayloadStyleIr::Struct,
        }),
        Fields::Unnamed(f) if f.unnamed.len() == 1 => Ok(ToolErrorPayloadIr::Single {
            ty: f.unnamed.first().unwrap().ty.clone(),
            field_ident: None,
        }),
        Fields::Named(f) if f.named.len() == 1 => {
            let field = f.named.first().unwrap();
            Ok(ToolErrorPayloadIr::Single {
                ty: field.ty.clone(),
                field_ident: field.ident.clone(),
            })
        }
        other => Err(Error::new(
            other.span(),
            "a ToolError variant may have at most one field; wrap multiple values in a struct or tuple type",
        )),
    }
}

fn no_payload_pattern(
    variant_ident: &syn::Ident,
    style: ToolErrorNoPayloadStyleIr,
) -> proc_macro2::TokenStream {
    match style {
        ToolErrorNoPayloadStyleIr::Unit => quote! { Self::#variant_ident },
        ToolErrorNoPayloadStyleIr::Tuple => quote! { Self::#variant_ident() },
        ToolErrorNoPayloadStyleIr::Struct => quote! { Self::#variant_ident {} },
    }
}

fn no_payload_constructor(
    variant_ident: &syn::Ident,
    style: ToolErrorNoPayloadStyleIr,
) -> proc_macro2::TokenStream {
    match style {
        ToolErrorNoPayloadStyleIr::Unit => quote! { Self::#variant_ident },
        ToolErrorNoPayloadStyleIr::Tuple => quote! { Self::#variant_ident() },
        ToolErrorNoPayloadStyleIr::Struct => quote! { Self::#variant_ident {} },
    }
}

/// Finds an `#[example(...)]` attribute, used to reject it in positions where
/// the derive does not collect examples (the enum itself or a variant's field).
fn find_example_attr(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs.iter().find(|a| a.path().is_ident("example"))
}

/// Finds the single `#[tool_error(...)]` attribute on a variant, rejecting a
/// second occurrence so it cannot be silently ignored.
fn find_tool_error_attr(attrs: &[Attribute]) -> Result<Option<&Attribute>, Error> {
    let mut found: Option<&Attribute> = None;
    for attr in attrs {
        if attr.path().is_ident("tool_error") {
            if found.is_some() {
                return Err(Error::new(
                    attr.span(),
                    "duplicate #[tool_error(...)]; a variant may have at most one",
                ));
            }
            found = Some(attr);
        }
    }
    Ok(found)
}

/// Parses `#[tool_error(kind = "...", exit_code = N)]`.
fn parse_tool_error_attr(attr: &Attribute) -> Result<(ErrorKindIr, u8), Error> {
    use syn::punctuated::Punctuated;
    let parser = Punctuated::<Expr, syn::Token![,]>::parse_terminated;
    let exprs = attr.parse_args_with(parser)?;

    let mut kind = None;
    let mut exit_code = None;
    let mut seen = SeenKeys::default();
    for expr in exprs.iter() {
        let Expr::Assign(assign) = expr else {
            return Err(Error::new(
                expr.span(),
                "expected `kind = \"...\"` and `exit_code = N`",
            ));
        };
        let key = assign_left_ident(&assign.left)?;
        seen.insert(&key)?;
        match key.to_string().as_str() {
            "kind" => kind = Some(parse_kind(&assign.right)?),
            "exit_code" => exit_code = Some(expr_u8(&assign.right, "exit_code")?),
            other => {
                return Err(Error::new(
                    key.span(),
                    format!("unknown #[tool_error] key `{other}`"),
                ));
            }
        }
    }

    let kind = kind.ok_or_else(|| Error::new(attr.span(), "#[tool_error] is missing `kind`"))?;
    let exit_code =
        exit_code.ok_or_else(|| Error::new(attr.span(), "#[tool_error] is missing `exit_code`"))?;
    Ok((kind, exit_code))
}

fn parse_kind(expr: &Expr) -> Result<ErrorKindIr, Error> {
    match expr_str(expr, "kind")?.as_str() {
        "usage" | "usage-error" => Ok(ErrorKindIr::UsageError),
        "runtime" | "runtime-error" => Ok(ErrorKindIr::RuntimeError),
        other => Err(Error::new(
            expr.span(),
            format!("invalid error kind `{other}`; expected `usage-error` or `runtime-error`"),
        )),
    }
}

fn assign_left_ident(left: &Expr) -> Result<syn::Ident, Error> {
    if let Expr::Path(p) = left
        && let Some(ident) = p.path.get_ident()
    {
        return Ok(ident.clone());
    }
    Err(Error::new(
        left.span(),
        "expected an identifier on the left of `=`",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Result<ToolErrorIr, Error> {
        let input: DeriveInput = syn::parse_str(src).unwrap();
        parse_tool_error(&input)
    }

    #[test]
    fn parses_variants() {
        let ir = parse(
            r#"
            enum GrepError {
                #[tool_error(kind = "usage-error", exit_code = 2)]
                BadPattern(String),
                #[tool_error(kind = "runtime-error", exit_code = 1)]
                Io(String),
            }
            "#,
        )
        .unwrap();
        assert_eq!(ir.enum_ident.to_string(), "GrepError");
        assert_eq!(ir.variants.len(), 2);
        assert_eq!(ir.variants[0].variant_ident.to_string(), "BadPattern");
        assert_eq!(ir.variants[0].kind, ErrorKindIr::UsageError);
        assert_eq!(ir.variants[0].exit_code, 2);
        assert_eq!(ir.variants[1].kind, ErrorKindIr::RuntimeError);
        assert_eq!(ir.variants[1].exit_code, 1);
        assert!(matches!(
            ir.variants[0].payload,
            ToolErrorPayloadIr::Single { .. }
        ));
    }

    #[test]
    fn payload_shapes() {
        let ir = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                Unit,
                #[tool_error(kind = "usage-error", exit_code = 1)]
                Tuple(String),
                #[tool_error(kind = "usage-error", exit_code = 1)]
                Named { reason: String },
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            ir.variants[0].payload,
            ToolErrorPayloadIr::None {
                style: ToolErrorNoPayloadStyleIr::Unit,
            }
        );
        assert!(matches!(
            ir.variants[1].payload,
            ToolErrorPayloadIr::Single { .. }
        ));
        assert!(matches!(
            ir.variants[2].payload,
            ToolErrorPayloadIr::Single { .. }
        ));
    }

    #[test]
    fn multi_field_variant_is_error() {
        let err = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                Bad { a: String, b: u32 },
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("at most one field"));
    }

    #[test]
    fn legacy_kind_spellings() {
        let ir = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage", exit_code = 129)]
                A,
                #[tool_error(kind = "runtime", exit_code = 128)]
                B,
            }
            "#,
        )
        .unwrap();
        assert_eq!(ir.variants[0].kind, ErrorKindIr::UsageError);
        assert_eq!(ir.variants[1].kind, ErrorKindIr::RuntimeError);
    }

    #[test]
    fn variant_doc_is_captured() {
        let ir = parse(
            r#"
            enum E {
                /// The pattern was bad.
                #[tool_error(kind = "usage-error", exit_code = 2)]
                BadPattern,
            }
            "#,
        )
        .unwrap();
        assert_eq!(ir.variants[0].doc.summary, "The pattern was bad.");
    }

    #[test]
    fn missing_attr_is_error() {
        let err = parse(
            r#"
            enum E {
                A,
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing #[tool_error"));
    }

    #[test]
    fn duplicate_tool_error_attr_is_error() {
        let err = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                #[tool_error(kind = "runtime-error", exit_code = 2)]
                A,
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate") || err.to_string().contains("at most one"));
    }

    #[test]
    fn duplicate_tool_error_key_is_error() {
        let err = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage-error", kind = "runtime-error", exit_code = 1)]
                A,
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn enum_level_tool_error_attr_is_error() {
        let err = parse(
            r#"
            #[tool_error(kind = "runtime-error", exit_code = 9)]
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                A,
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("may only be used on variants"));
    }

    #[test]
    fn field_level_tool_error_attr_is_error() {
        let err = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                A(
                    #[tool_error(kind = "runtime-error", exit_code = 2)]
                    String,
                ),
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("may only be used on variants"));
    }

    #[test]
    fn misplaced_example_attr_in_tool_error_is_error() {
        let enum_level_example_is_accepted = parse(
            r#"
            #[example(body = "ignored")]
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                A,
            }
            "#,
        )
        .is_ok();
        let field_level_example_is_accepted = parse(
            r#"
            enum E {
                #[tool_error(kind = "usage-error", exit_code = 1)]
                A(
                    #[example(body = "ignored")]
                    String,
                ),
            }
            "#,
        )
        .is_ok();

        assert!(
            !enum_level_example_is_accepted && !field_level_example_is_accepted,
            "#[example] is accepted and silently ignored on ToolError enum/field positions"
        );
    }

    #[test]
    fn non_enum_is_error() {
        let err = parse(r#"struct S { x: u32 }"#).unwrap_err();
        assert!(err.to_string().contains("can only be applied to enums"));
    }

    #[test]
    fn invalid_kind_is_error() {
        let err = parse(
            r#"
            enum E {
                #[tool_error(kind = "bogus", exit_code = 1)]
                A,
            }
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid error kind"));
    }
}
