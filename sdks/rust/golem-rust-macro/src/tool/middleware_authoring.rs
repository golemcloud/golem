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

use crate::tool::doc::parse_doc;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote};
use std::hash::{Hash, Hasher};
use syn::ext::IdentExt;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    Error, Expr, ExprLit, FnArg, GenericArgument, Ident, ImplItem, Item, ItemFn, ItemImpl, Lit,
    LitStr, MetaNameValue, Pat, Path, PathArguments, ReturnType, Token, Type,
};

pub fn tool_middleware_impl(
    attrs: TokenStream,
    item: TokenStream,
    golem_rust: &Ident,
) -> TokenStream {
    expand_tool_middleware(attrs.into(), item.into(), golem_rust)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

pub fn universal_tool_middleware_impl(
    attrs: TokenStream,
    item: TokenStream,
    golem_rust: &Ident,
) -> TokenStream {
    expand_universal_tool_middleware(attrs.into(), item.into(), golem_rust)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

struct MonomorphicArgs {
    name: LitStr,
    constructor: Path,
}

struct UniversalArgs {
    name: LitStr,
}

fn parse_monomorphic_args(attrs: TokenStream2) -> syn::Result<MonomorphicArgs> {
    let values = parse_name_values(attrs)?;
    let mut name = None;
    let mut constructor = None;
    for value in values {
        let Some(key) = value.path.get_ident() else {
            return Err(Error::new_spanned(
                value.path,
                "tool middleware attribute keys must be identifiers",
            ));
        };
        match key.to_string().as_str() {
            "name" => set_once(&mut name, parse_string(value.value, "name")?, key)?,
            "constructor" => {
                let Expr::Path(path) = value.value else {
                    return Err(Error::new_spanned(
                        value.value,
                        "`constructor` must be a synchronous zero-argument function path",
                    ));
                };
                if path.qself.is_some() {
                    return Err(Error::new_spanned(
                        path,
                        "`constructor` must be a synchronous zero-argument function path",
                    ));
                }
                set_once(&mut constructor, path.path, key)?;
            }
            other => {
                return Err(Error::new_spanned(
                    key,
                    format!(
                        "unknown #[tool_middleware] key `{other}`; expected `name` or `constructor`"
                    ),
                ));
            }
        }
    }
    let name = name.ok_or_else(|| {
        Error::new(
            proc_macro2::Span::call_site(),
            "#[tool_middleware] is missing `name`",
        )
    })?;
    validate_middleware_name(&name)?;
    let constructor = constructor.ok_or_else(|| {
        Error::new(
            proc_macro2::Span::call_site(),
            "#[tool_middleware] is missing `constructor`",
        )
    })?;
    Ok(MonomorphicArgs { name, constructor })
}

fn parse_universal_args(attrs: TokenStream2) -> syn::Result<UniversalArgs> {
    let values = parse_name_values(attrs)?;
    let mut name = None;
    for value in values {
        let Some(key) = value.path.get_ident() else {
            return Err(Error::new_spanned(
                value.path,
                "universal tool middleware attribute keys must be identifiers",
            ));
        };
        match key.to_string().as_str() {
            "name" => set_once(&mut name, parse_string(value.value, "name")?, key)?,
            other => {
                return Err(Error::new_spanned(
                    key,
                    format!(
                        "unknown #[universal_tool_middleware] key `{other}`; expected only `name`"
                    ),
                ));
            }
        }
    }
    let name = name.ok_or_else(|| {
        Error::new(
            proc_macro2::Span::call_site(),
            "#[universal_tool_middleware] is missing `name`",
        )
    })?;
    validate_middleware_name(&name)?;
    Ok(UniversalArgs { name })
}

fn parse_name_values(attrs: TokenStream2) -> syn::Result<Punctuated<MetaNameValue, Token![,]>> {
    Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(attrs)
}

fn parse_string(expr: Expr, key: &str) -> syn::Result<LitStr> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expr
    else {
        return Err(Error::new_spanned(
            expr,
            format!("`{key}` must be a string literal"),
        ));
    };
    Ok(value)
}

fn set_once<T>(slot: &mut Option<T>, value: T, key: &Ident) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        return Err(Error::new_spanned(
            key,
            format!("duplicate tool middleware attribute key `{key}`"),
        ));
    }
    Ok(())
}

fn validate_middleware_name(name: &LitStr) -> syn::Result<()> {
    let value = name.value();
    let mut previous_dash = false;
    let valid = !value.is_empty()
        && value.chars().enumerate().all(|(index, character)| {
            let character_valid = if index == 0 {
                character.is_ascii_lowercase()
            } else {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            };
            let dash_valid = !(character == '-' && previous_dash);
            previous_dash = character == '-';
            character_valid && dash_valid
        })
        && !value.ends_with('-');
    if !valid {
        return Err(Error::new_spanned(
            name,
            "tool middleware names must match `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`",
        ));
    }
    Ok(())
}

fn expand_tool_middleware(
    attrs: TokenStream2,
    item: TokenStream2,
    golem_rust: &Ident,
) -> syn::Result<TokenStream2> {
    let args = parse_monomorphic_args(attrs)?;
    let Item::Impl(mut item_impl) = syn::parse2::<Item>(item)? else {
        return Err(Error::new(
            proc_macro2::Span::call_site(),
            "#[tool_middleware] must be applied to a trait implementation (`impl ToolMiddleware for Type`)",
        ));
    };
    let trait_path = validate_monomorphic_impl(&item_impl)?;
    let self_ty = &item_impl.self_ty;
    let constructor = &args.constructor;
    let name = &args.name;
    let doc = parse_doc(&item_impl.attrs);
    let summary = doc.summary;
    let description = doc.description;
    let suffix = registration_suffix(self_ty, &trait_path, &name.value());
    let trait_name = trait_path
        .segments
        .last()
        .expect("validated middleware trait path")
        .ident
        .unraw()
        .to_string()
        .to_lowercase();
    let descriptor_ident =
        format_ident!("__golem_tool_middleware_descriptor_{trait_name}_{suffix:016x}");
    let invoker_ident = format_ident!("__golem_tool_middleware_invoke_{trait_name}_{suffix:016x}");
    let register_ident =
        format_ident!("__golem_register_tool_middleware_{trait_name}_{suffix:016x}");

    let annotation: ImplItem = syn::parse_quote! {
        #[doc(hidden)]
        fn __golem_tool_middleware_annotation() where Self: Sized {}
    };
    item_impl.items.push(annotation);

    Ok(quote! {
        #item_impl

        #[doc(hidden)]
        fn #descriptor_ident() -> #golem_rust::tool::ToolMiddleware {
            #golem_rust::tool::ToolMiddleware {
                name: #name.to_string(),
                aliases: ::std::vec::Vec::new(),
                doc: #golem_rust::schema::tool::Doc {
                    summary: #summary.to_string(),
                    description: #description.to_string(),
                    examples: ::std::vec::Vec::new(),
                },
                scope: #golem_rust::tool::ToolMiddlewareScope::Monomorphic(
                    #golem_rust::tool::MonomorphicToolMiddlewareScope {
                        presented: <#self_ty as #trait_path>::__golem_presented_tool_descriptor(),
                        expected: ::std::option::Option::Some(
                            <#self_ty as #trait_path>::__golem_expected_tool_descriptor()
                        ),
                    }
                ),
            }
        }

        #[doc(hidden)]
        fn #invoker_ident(
            _tool_name: ::std::string::String,
            _tool_metadata: #golem_rust::tool::Tool,
            command_path: ::std::vec::Vec<::std::string::String>,
            input: #golem_rust::TypedSchemaValue,
            stdin: ::std::option::Option<#golem_rust::tool::InputStream>,
            principal: #golem_rust::tool::Principal,
            underlying: #golem_rust::tool::UnderlyingTool,
        ) -> #golem_rust::tool::ToolMiddlewareInvokeFuture {
            ::std::boxed::Box::pin(async move {
                let constructor: fn() -> #self_ty = #constructor;
                let middleware = constructor();
                <#self_ty as #trait_path>::__golem_invoke_tool_middleware(
                    &middleware,
                    command_path,
                    input,
                    stdin,
                    principal,
                    underlying,
                )
                .await
            })
        }

        #golem_rust::ctor::__support::ctor_parse!(
            #[ctor] fn #register_ident() {
                #golem_rust::tool::register_tool_middleware(
                    #descriptor_ident(),
                    #invoker_ident,
                );
            }
        );
    })
}

fn validate_monomorphic_impl(item_impl: &ItemImpl) -> syn::Result<Path> {
    if !item_impl.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &item_impl.generics,
            "#[tool_middleware] requires a concrete, non-generic implementation",
        ));
    }
    let Some((negative, trait_path, _)) = &item_impl.trait_ else {
        return Err(Error::new_spanned(
            &item_impl.self_ty,
            "#[tool_middleware] must be applied to a generated middleware trait implementation",
        ));
    };
    if negative.is_some() {
        return Err(Error::new_spanned(
            trait_path,
            "#[tool_middleware] does not support negative trait implementations",
        ));
    }
    let segment = trait_path
        .segments
        .last()
        .ok_or_else(|| Error::new_spanned(trait_path, "middleware trait path must not be empty"))?;
    let trait_name = segment.ident.unraw().to_string();
    if trait_name
        .strip_suffix("Middleware")
        .is_none_or(str::is_empty)
    {
        return Err(Error::new_spanned(
            &segment.ident,
            "#[tool_middleware] requires a generated `<Tool>Middleware` trait",
        ));
    }
    match &segment.arguments {
        PathArguments::None => {}
        PathArguments::AngleBracketed(arguments) => {
            if arguments.args.len() != 1 {
                return Err(Error::new_spanned(
                    arguments,
                    "a generated middleware trait accepts exactly one underlying proxy type",
                ));
            }
            let Some(GenericArgument::Type(underlying)) = arguments.args.first() else {
                return Err(Error::new_spanned(
                    arguments,
                    "the middleware trait argument must be a generated `<Tool>Underlying` type",
                ));
            };
            let Some(underlying_name) = type_last_ident(underlying) else {
                return Err(Error::new_spanned(
                    underlying,
                    "the middleware trait argument must be a generated `<Tool>Underlying` type",
                ));
            };
            if underlying_name
                .strip_suffix("Underlying")
                .is_none_or(str::is_empty)
            {
                return Err(Error::new_spanned(
                    underlying,
                    "the middleware trait argument must be a generated `<Tool>Underlying` type",
                ));
            }
        }
        PathArguments::Parenthesized(arguments) => {
            return Err(Error::new_spanned(
                arguments,
                "generated middleware traits do not use parenthesized arguments",
            ));
        }
    }
    Ok(trait_path.clone())
}

fn registration_suffix(self_ty: &Type, trait_path: &Path, name: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    self_ty.to_token_stream().to_string().hash(&mut hasher);
    trait_path.to_token_stream().to_string().hash(&mut hasher);
    name.hash(&mut hasher);
    hasher.finish()
}

fn expand_universal_tool_middleware(
    attrs: TokenStream2,
    item: TokenStream2,
    golem_rust: &Ident,
) -> syn::Result<TokenStream2> {
    let args = parse_universal_args(attrs)?;
    let Item::Fn(item_fn) = syn::parse2::<Item>(item)? else {
        return Err(Error::new(
            proc_macro2::Span::call_site(),
            "#[universal_tool_middleware] must be applied to an async free function",
        ));
    };
    validate_universal_function(&item_fn)?;
    let function_ident = &item_fn.sig.ident;
    let name = &args.name;
    let doc = parse_doc(&item_fn.attrs);
    let summary = doc.summary;
    let description = doc.description;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    function_ident.unraw().to_string().hash(&mut hasher);
    name.value().hash(&mut hasher);
    let suffix = hasher.finish();
    let function_name = function_ident.unraw().to_string().to_lowercase();
    let descriptor_ident =
        format_ident!("__golem_universal_tool_middleware_descriptor_{function_name}_{suffix:016x}");
    let invoker_ident =
        format_ident!("__golem_universal_tool_middleware_invoke_{function_name}_{suffix:016x}");
    let register_ident =
        format_ident!("__golem_register_universal_tool_middleware_{function_name}_{suffix:016x}");

    Ok(quote! {
        #item_fn

        #[doc(hidden)]
        fn #descriptor_ident() -> #golem_rust::tool::ToolMiddleware {
            #golem_rust::tool::ToolMiddleware {
                name: #name.to_string(),
                aliases: ::std::vec::Vec::new(),
                doc: #golem_rust::schema::tool::Doc {
                    summary: #summary.to_string(),
                    description: #description.to_string(),
                    examples: ::std::vec::Vec::new(),
                },
                scope: #golem_rust::tool::ToolMiddlewareScope::Universal,
            }
        }

        #[doc(hidden)]
        fn #invoker_ident(
            tool_name: ::std::string::String,
            tool_metadata: #golem_rust::tool::Tool,
            command_path: ::std::vec::Vec<::std::string::String>,
            input: #golem_rust::TypedSchemaValue,
            stdin: ::std::option::Option<#golem_rust::tool::InputStream>,
            principal: #golem_rust::tool::Principal,
            underlying: #golem_rust::tool::UnderlyingTool,
        ) -> #golem_rust::tool::ToolMiddlewareInvokeFuture {
            ::std::boxed::Box::pin(#function_ident(
                tool_name,
                tool_metadata,
                command_path,
                input,
                stdin,
                principal,
                underlying,
            ))
        }

        #golem_rust::ctor::__support::ctor_parse!(
            #[ctor] fn #register_ident() {
                #golem_rust::tool::register_tool_middleware(
                    #descriptor_ident(),
                    #invoker_ident,
                );
            }
        );
    })
}

fn validate_universal_function(item_fn: &ItemFn) -> syn::Result<()> {
    let signature = &item_fn.sig;
    if signature.asyncness.is_none() {
        return Err(Error::new_spanned(
            signature,
            "#[universal_tool_middleware] requires an async function",
        ));
    }
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            &signature.generics,
            "#[universal_tool_middleware] does not support generic functions",
        ));
    }
    if signature.constness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
    {
        return Err(Error::new_spanned(
            signature,
            "#[universal_tool_middleware] requires a safe Rust async function",
        ));
    }
    let inputs = signature
        .inputs
        .iter()
        .map(|argument| {
            let FnArg::Typed(argument) = argument else {
                return Err(Error::new_spanned(
                    argument,
                    "universal tool middleware functions cannot have a receiver",
                ));
            };
            if !matches!(argument.pat.as_ref(), Pat::Ident(_)) {
                return Err(Error::new_spanned(
                    &argument.pat,
                    "universal tool middleware parameters must use identifier patterns",
                ));
            }
            Ok(argument.ty.as_ref())
        })
        .collect::<syn::Result<Vec<_>>>()?;
    if inputs.len() != 7 {
        return Err(Error::new_spanned(
            &signature.inputs,
            "universal tool middleware functions require exactly seven parameters",
        ));
    }
    let expected = [
        type_is_ident(inputs[0], "String"),
        type_is_ident(inputs[1], "Tool"),
        type_is_container(inputs[2], "Vec", |inner| type_is_ident(inner, "String")),
        type_is_ident(inputs[3], "TypedSchemaValue"),
        type_is_container(inputs[4], "Option", |inner| {
            type_is_ident(inner, "InputStream")
        }),
        type_is_ident(inputs[5], "Principal"),
        type_is_ident(inputs[6], "UnderlyingTool"),
    ];
    if let Some((index, _)) = expected.iter().enumerate().find(|(_, valid)| !**valid) {
        return Err(Error::new_spanned(
            inputs[index],
            format!(
                "universal tool middleware parameter {} has the wrong SDK-owned type",
                index + 1
            ),
        ));
    }
    let ReturnType::Type(_, output) = &signature.output else {
        return Err(Error::new_spanned(
            &signature.output,
            "universal tool middleware functions must return `Result<InvocationResult, ToolInvokeError<TypedSchemaValue>>`",
        ));
    };
    if !result_type_is_exact(output) {
        return Err(Error::new_spanned(
            output,
            "universal tool middleware functions must return `Result<InvocationResult, ToolInvokeError<TypedSchemaValue>>`",
        ));
    }
    Ok(())
}

fn type_is_ident(ty: &Type, expected: &str) -> bool {
    matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.segments.last().is_some_and(|segment| segment.ident == expected && matches!(segment.arguments, PathArguments::None)))
}

fn type_is_container(
    ty: &Type,
    expected: &str,
    validate_first: impl FnOnce(&Type) -> bool,
) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != expected {
        return false;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    if arguments.args.len() != 1 {
        return false;
    }
    let Some(GenericArgument::Type(first)) = arguments.args.first() else {
        return false;
    };
    validate_first(first)
}

fn result_type_is_exact(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    if segment.ident != "Result" || arguments.args.len() != 2 {
        return false;
    }
    let Some(GenericArgument::Type(result)) = arguments.args.first() else {
        return false;
    };
    let Some(GenericArgument::Type(error)) = arguments.args.iter().nth(1) else {
        return false;
    };
    type_is_ident(result, "InvocationResult")
        && type_is_container(error, "ToolInvokeError", |inner| {
            type_is_ident(inner, "TypedSchemaValue")
        })
}

fn type_last_ident(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.unraw().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sdk() -> Ident {
        Ident::new("golem_rust", proc_macro2::Span::call_site())
    }

    #[test]
    fn transparent_monomorphic_impl_expands_with_constructor_assertion() {
        let expanded = expand_tool_middleware(
            quote! { name = "path-policy", constructor = PathPolicy::new },
            quote! {
                /// Enforces the configured path policy.
                impl FileToolMiddleware for PathPolicy {
                    async fn read(
                        &self,
                        underlying: &mut FileToolUnderlying,
                        path: String,
                    ) -> Result<Vec<u8>, golem_rust::tool::ToolInvokeError<FileError>> {
                        underlying.read(path).await
                    }
                }
            },
            &sdk(),
        )
        .unwrap();

        syn::parse2::<syn::File>(expanded.clone()).unwrap();
        let text = expanded.to_string();
        assert!(text.contains("fn () -> PathPolicy = PathPolicy :: new"));
        assert!(text.contains("__golem_tool_middleware_annotation"));
        assert!(text.contains("ToolMiddlewareScope :: Monomorphic"));
    }

    #[test]
    fn adapter_underlying_type_is_accepted() {
        let expanded = expand_tool_middleware(
            quote! { name = "grep-via-ripgrep", constructor = Adapter::new },
            quote! { impl GrepMiddleware<tools::RipgrepUnderlying> for Adapter {} },
            &sdk(),
        )
        .unwrap();
        syn::parse2::<syn::File>(expanded).unwrap();
    }

    #[test]
    fn monomorphic_attributes_are_required_and_closed() {
        let missing_name = expand_tool_middleware(
            quote! { constructor = Policy::new },
            quote! { impl FileMiddleware for Policy {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(missing_name.to_string().contains("missing `name`"));

        let missing = expand_tool_middleware(
            quote! { name = "policy" },
            quote! { impl FileMiddleware for Policy {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(missing.to_string().contains("missing `constructor`"));

        let unknown = expand_tool_middleware(
            quote! { name = "policy", constructor = Policy::new, aliases = [] },
            quote! { impl FileMiddleware for Policy {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("unknown #[tool_middleware] key `aliases`")
        );
    }

    #[test]
    fn monomorphic_target_must_be_concrete_generated_trait_impl() {
        let inherent = expand_tool_middleware(
            quote! { name = "policy", constructor = Policy::new },
            quote! { impl Policy {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(
            inherent
                .to_string()
                .contains("generated middleware trait implementation")
        );

        let generic = expand_tool_middleware(
            quote! { name = "policy", constructor = Policy::<T>::new },
            quote! { impl<T> FileMiddleware for Policy<T> {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(generic.to_string().contains("concrete, non-generic"));

        let wrong_trait = expand_tool_middleware(
            quote! { name = "policy", constructor = Policy::new },
            quote! { impl FileTool for Policy {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(
            wrong_trait
                .to_string()
                .contains("generated `<Tool>Middleware` trait")
        );
    }

    #[test]
    fn middleware_name_uses_tool_identifier_grammar() {
        let error = expand_tool_middleware(
            quote! { name = "Not Valid", constructor = Policy::new },
            quote! { impl FileMiddleware for Policy {} },
            &sdk(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("must match"));
    }

    #[test]
    fn valid_universal_function_expands() {
        let expanded = expand_universal_tool_middleware(
            quote! { name = "audit" },
            quote! {
                /// Audits every tool invocation.
                async fn audit(
                    tool_name: String,
                    tool_metadata: golem_rust::tool::Tool,
                    command_path: Vec<String>,
                    input: golem_rust::TypedSchemaValue,
                    stdin: Option<golem_rust::tool::InputStream>,
                    principal: golem_rust::tool::Principal,
                    underlying: golem_rust::tool::UnderlyingTool,
                ) -> Result<
                    golem_rust::tool::InvocationResult,
                    golem_rust::tool::ToolInvokeError<golem_rust::TypedSchemaValue>,
                > {
                    underlying.invoke(command_path, input, stdin).await
                }
            },
            &sdk(),
        )
        .unwrap();

        syn::parse2::<syn::File>(expanded.clone()).unwrap();
        assert!(
            expanded
                .to_string()
                .contains("ToolMiddlewareScope :: Universal")
        );
        assert!(
            !expanded
                .to_string()
                .contains("tool :: register_universal_tool_middleware")
        );
    }

    #[test]
    fn authoring_expansions_use_resolved_sdk_name_and_common_invoker_abi() {
        let renamed_sdk = Ident::new("sdk_alias", proc_macro2::Span::call_site());
        let monomorphic = expand_tool_middleware(
            quote! { name = "policy", constructor = Policy::new },
            quote! { impl FileMiddleware for Policy {} },
            &renamed_sdk,
        )
        .unwrap();
        let universal = expand_universal_tool_middleware(
            quote! { name = "audit" },
            universal_function(true, false),
            &renamed_sdk,
        )
        .unwrap();

        for expansion in [monomorphic, universal] {
            let file = syn::parse2::<syn::File>(expansion.clone()).unwrap();
            let invoker = file
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Fn(item) if item.sig.ident.to_string().contains("invoke") => Some(item),
                    _ => None,
                })
                .next()
                .expect("expansion should contain an invoker adapter");
            assert_eq!(invoker.sig.inputs.len(), 7);
            let text = expansion.to_string();
            assert!(text.contains("sdk_alias :: tool :: register_tool_middleware"));
        }
    }

    #[test]
    fn universal_generic_arity_is_exact() {
        let too_many_vec_args: Type = syn::parse_quote! { Vec<String, String> };
        assert!(!type_is_container(&too_many_vec_args, "Vec", |inner| {
            type_is_ident(inner, "String")
        }));

        let too_many_result_args: Type = syn::parse_quote! {
            Result<InvocationResult, ToolInvokeError<TypedSchemaValue>, TypedSchemaValue>
        };
        assert!(!result_type_is_exact(&too_many_result_args));

        let too_many_error_args: Type = syn::parse_quote! {
            Result<InvocationResult, ToolInvokeError<TypedSchemaValue, TypedSchemaValue>>
        };
        assert!(!result_type_is_exact(&too_many_error_args));
    }

    #[test]
    fn universal_target_and_signature_are_validated() {
        let not_function = expand_universal_tool_middleware(
            quote! { name = "audit" },
            quote! { struct Audit; },
            &sdk(),
        )
        .unwrap_err();
        assert!(not_function.to_string().contains("async free function"));

        let not_async = expand_universal_tool_middleware(
            quote! { name = "audit" },
            universal_function(false, false),
            &sdk(),
        )
        .unwrap_err();
        assert!(not_async.to_string().contains("requires an async function"));

        let generic = expand_universal_tool_middleware(
            quote! { name = "audit" },
            universal_function(true, true),
            &sdk(),
        )
        .unwrap_err();
        assert!(
            generic
                .to_string()
                .contains("does not support generic functions")
        );
    }

    #[test]
    fn universal_attributes_are_closed() {
        let error = expand_universal_tool_middleware(
            quote! { name = "audit", constructor = Audit::new },
            universal_function(true, false),
            &sdk(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown #[universal_tool_middleware] key `constructor`")
        );
    }

    fn universal_function(is_async: bool, generic: bool) -> TokenStream2 {
        let asyncness = is_async.then(|| quote! { async });
        let generics = generic.then(|| quote! { <T> });
        quote! {
            #asyncness fn audit #generics(
                tool_name: String,
                tool_metadata: Tool,
                command_path: Vec<String>,
                input: TypedSchemaValue,
                stdin: Option<InputStream>,
                principal: Principal,
                underlying: UnderlyingTool,
            ) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
                unimplemented!()
            }
        }
    }
}
