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

//! `#[tool_definition]` entry point.
//!
//! This parses the trait and all tool authoring attributes into a
//! [`ToolDefinitionIr`], strips the helper attributes from the emitted trait,
//! and adds the hidden `tool_implementation_annotation` item that forces every
//! implementation to carry `#[tool_implementation]`. Metadata synthesis and
//! runtime registration are added later from that IR.

use crate::tool::arg::parse_arg;
use crate::tool::command::{CommandAttr, parse_command_into};
use crate::tool::constraint::parse_constraint;
use crate::tool::doc::parse_doc_full;
use crate::tool::helpers::{SeenKeys, to_kebab_case};
use crate::tool::ir::{ArgIr, ArgPlacement, CommandIr, ParamIr, ToolDefinitionIr};
use crate::tool::result::parse_result;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::{
    Attribute, Error, Expr, FnArg, GenericArgument, ItemTrait, Lit, PathArguments, ReturnType,
    TraitItem, Type,
};

const HELPER_ATTRS: [&str; 5] = ["arg", "command", "constraint", "result", "example"];

pub fn tool_definition_impl(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_trait = syn::parse_macro_input!(item as ItemTrait);

    let version = match parse_version(attrs.into()) {
        Ok(v) => v,
        Err(err) => return err.to_compile_error().into(),
    };

    // Building the IR validates every tool authoring attribute and surfaces
    // parse errors at compile time.
    let ir = match build_tool_definition_ir(&item_trait, version) {
        Ok(ir) => ir,
        Err(err) => return err.to_compile_error().into(),
    };

    // Metadata synthesis: the hidden free descriptor function that builds the
    // runtime `ExtendedToolType`. It is emitted as a module-level free function
    // so a parent tool's `#[command(subtree = path::Child)]` can reach it
    // through the child trait's module path without a concrete `Self`.
    let descriptor_fn = match crate::tool::descriptor::synthesize_descriptor_fn(&ir) {
        Ok(tokens) => tokens,
        Err(err) => return err.to_compile_error().into(),
    };
    let client = crate::tool::client::synthesize_client(&ir);
    let descriptor_fn_ident = crate::tool::descriptor::descriptor_fn_ident(&ir.trait_ident);

    strip_helper_attrs(&mut item_trait);

    // A hidden default method delegating to the free descriptor function, so the
    // `#[tool_implementation]` registration ctor can call it through the trait.
    let descriptor_item: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn __tool_descriptor() -> golem_rust::agentic::ExtendedToolType
        where
            Self: Sized,
        {
            #descriptor_fn_ident(&mut golem_rust::agentic::ToolBuildCtx::new())
                .expect("tool descriptor build failed")
        }
    };
    item_trait.items.push(descriptor_item);

    let method_paths = tool_method_paths(&ir);
    let method_paths_item: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn __tool_invoke_method_paths() -> &'static [(&'static str, &'static [&'static str])]
        where
            Self: Sized,
        {
            &[ #(#method_paths),* ]
        }
    };
    item_trait.items.push(method_paths_item);

    let method_param_names = tool_method_param_names(&ir);
    let method_param_names_item: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn __tool_invoke_method_param_names() -> &'static [(&'static str, &'static [&'static str])]
        where
            Self: Sized,
        {
            &[ #(#method_param_names),* ]
        }
    };
    item_trait.items.push(method_param_names_item);

    let subtree_paths = tool_subtree_paths(&ir);
    let subtree_paths_item: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn __tool_invoke_subtree_paths() -> &'static [(&'static [&'static str], &'static str)]
        where
            Self: Sized,
        {
            &[ #(#subtree_paths),* ]
        }
    };
    item_trait.items.push(subtree_paths_item);

    let invoker_item = match syn::parse2::<TraitItem>(synthesize_tool_invoker(&ir)) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    item_trait.items.push(invoker_item);

    let command_kinds = tool_command_kinds(&ir);
    item_trait.items.extend(command_kinds);

    let annotation_item: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn tool_implementation_annotation() where Self: Sized;
    };
    item_trait.items.push(annotation_item);

    quote! {
        #[allow(async_fn_in_trait)]
        #item_trait

        #descriptor_fn

        #client
    }
    .into()
}

fn tool_method_paths(ir: &ToolDefinitionIr) -> Vec<proc_macro2::TokenStream> {
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    ir.commands
        .iter()
        .map(|cmd| {
            let method_name = cmd.method_ident.to_string();
            let command_name = if to_kebab_case(&method_name) == tool_name {
                tool_name.clone()
            } else {
                cmd.name_override
                    .clone()
                    .unwrap_or_else(|| to_kebab_case(&method_name))
            };
            if command_name == tool_name {
                quote! { (#method_name, &[] as &'static [&'static str]) }
            } else {
                quote! { (#method_name, &[#command_name] as &'static [&'static str]) }
            }
        })
        .collect()
}

fn tool_method_param_names(ir: &ToolDefinitionIr) -> Vec<proc_macro2::TokenStream> {
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    ir.commands
        .iter()
        .map(|cmd| {
            let method_name = cmd.method_ident.to_string();
            let param_names = cmd
                .params
                .iter()
                .map(|param| canonical_param_name(ir, cmd, param, &tool_name))
                .collect::<Vec<_>>();
            quote! { (#method_name, &[#(#param_names),*] as &'static [&'static str]) }
        })
        .collect()
}

fn canonical_param_name(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> String {
    let own_name = to_kebab_case(&param.ident.to_string());
    if to_kebab_case(&cmd.method_ident.to_string()) == tool_name {
        return own_name;
    }
    let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    else {
        return own_name;
    };
    for root_param in &root.params {
        if !is_global_param(root, root_param) {
            continue;
        }
        let root_name = to_kebab_case(&root_param.ident.to_string());
        if param_surfaces_intersect(
            &root_name,
            &param_aliases(root, root_param),
            &own_name,
            &param_aliases(cmd, param),
        ) {
            return root_name;
        }
    }
    own_name
}

fn param_surfaces_intersect(
    left_name: &str,
    left_aliases: &[String],
    right_name: &str,
    right_aliases: &[String],
) -> bool {
    left_name == right_name
        || left_aliases.iter().any(|alias| alias == right_name)
        || right_aliases.iter().any(|alias| alias == left_name)
        || left_aliases
            .iter()
            .any(|left| right_aliases.iter().any(|right| right == left))
}

fn is_global_param(cmd: &CommandIr, param: &ParamIr) -> bool {
    cmd.args
        .iter()
        .find(|arg| arg.param == param.ident)
        .and_then(|arg| arg.placement)
        == Some(ArgPlacement::Global)
}

fn param_aliases(cmd: &CommandIr, param: &ParamIr) -> Vec<String> {
    cmd.args
        .iter()
        .find(|arg| arg.param == param.ident)
        .map(|arg| arg.aliases.clone())
        .unwrap_or_default()
}

fn tool_command_kinds(ir: &ToolDefinitionIr) -> Vec<TraitItem> {
    ir.commands
        .iter()
        .map(|cmd| {
            let method_ident = tool_command_kind_method_ident(&cmd.method_ident);
            if cmd.subtree.is_some() {
                syn::parse_quote! {
                    #[doc(hidden)]
                    fn #method_ident() -> golem_rust::agentic::ToolSubtreeCommand
                    where
                        Self: Sized,
                    {
                        golem_rust::agentic::ToolSubtreeCommand
                    }
                }
            } else {
                syn::parse_quote! {
                    #[doc(hidden)]
                    fn #method_ident() -> golem_rust::agentic::ToolLeafCommand
                    where
                        Self: Sized,
                    {
                        golem_rust::agentic::ToolLeafCommand
                    }
                }
            }
        })
        .collect()
}

pub(crate) fn tool_command_kind_method_ident(method_ident: &syn::Ident) -> syn::Ident {
    quote::format_ident!("__tool_invoke_kind_{}", method_ident)
}

fn tool_subtree_paths(ir: &ToolDefinitionIr) -> Vec<proc_macro2::TokenStream> {
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    ir.commands
        .iter()
        .filter_map(|cmd| {
            let subtree = cmd.subtree.as_ref()?;
            let method_name = cmd.method_ident.to_string();
            let command_name = if to_kebab_case(&method_name) == tool_name {
                tool_name.clone()
            } else {
                cmd.name_override
                    .clone()
                    .unwrap_or_else(|| to_kebab_case(&method_name))
            };
            let child_tool_name = subtree
                .path
                .segments
                .last()
                .map(|segment| to_kebab_case(&segment.ident.to_string()))
                .unwrap_or_default();
            let mut paths = Vec::new();
            if command_name == tool_name {
                paths.push(quote! { (&[] as &'static [&'static str], #child_tool_name) });
            } else {
                paths.push(
                    quote! { (&[#command_name] as &'static [&'static str], #child_tool_name) },
                );
                for alias in &cmd.aliases {
                    paths.push(quote! { (&[#alias] as &'static [&'static str], #child_tool_name) });
                }
            }
            Some(quote! { #(#paths),* })
        })
        .collect()
}

fn synthesize_tool_invoker(ir: &ToolDefinitionIr) -> proc_macro2::TokenStream {
    let invoke_arms = synthesize_invoke_arms(ir);
    quote! {
        #[doc(hidden)]
        fn __tool_invoke(
            __command_path: ::std::vec::Vec<::std::string::String>,
            __input: golem_rust::golem_agentic::exports::golem::tool::guest::TypedSchemaValue,
            mut __stdin: ::std::option::Option<golem_rust::wasip2::io::streams::InputStream>,
            __principal: golem_rust::golem_agentic::golem::agent::common::Principal,
        ) -> ::std::result::Result<
            golem_rust::golem_agentic::exports::golem::tool::guest::InvocationResult,
            golem_rust::golem_agentic::exports::golem::tool::guest::ToolError,
        >
        where
            Self: Sized,
        {
            fn __encode_success_value<T: golem_rust::IntoSchema + ?Sized>(
                __value: &T,
                __stdout: ::std::option::Option<golem_rust::wasip2::io::streams::OutputStream>,
            ) -> ::std::result::Result<
                golem_rust::golem_agentic::exports::golem::tool::guest::InvocationResult,
                golem_rust::golem_agentic::exports::golem::tool::guest::ToolError,
            > {
                let __value = golem_rust::IntoTypedSchemaValue::into_typed_schema_value(__value)
                    .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidResult(__err.to_string()))?;
                let __value = golem_rust::encode_typed_schema_value(&__value)
                    .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidResult(__err.to_string()))?;
                ::std::result::Result::Ok(golem_rust::golem_agentic::exports::golem::tool::guest::InvocationResult {
                    result: ::std::option::Option::Some(__value),
                    stdout: __stdout,
                })
            }

            fn __encode_success_unit(
                __stdout: ::std::option::Option<golem_rust::wasip2::io::streams::OutputStream>,
            ) -> ::std::result::Result<
                golem_rust::golem_agentic::exports::golem::tool::guest::InvocationResult,
                golem_rust::golem_agentic::exports::golem::tool::guest::ToolError,
            > {
                ::std::result::Result::Ok(golem_rust::golem_agentic::exports::golem::tool::guest::InvocationResult {
                    result: ::std::option::Option::None,
                    stdout: __stdout,
                })
            }

            fn __encode_custom_error<T: golem_rust::agentic::ToolErrorSchema + ?Sized>(
                __error: &T,
            ) -> ::std::result::Result<
                golem_rust::golem_agentic::exports::golem::tool::guest::ToolError,
                golem_rust::golem_agentic::exports::golem::tool::guest::ToolError,
            > {
                let __value = golem_rust::agentic::ToolErrorSchema::to_error_payload_value(__error)
                    .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidResult(__err.to_string()))?;
                let __value = golem_rust::encode_typed_schema_value(&__value)
                    .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidResult(__err.to_string()))?;
                ::std::result::Result::Ok(
                    golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::CustomError(__value)
                )
            }

            let __tool = Self::__tool_descriptor();
            let __command_index = __tool.command_index_by_path(&__command_path).ok_or_else(|| {
                golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidCommandPath(
                    __command_path.clone()
                )
            })?;
            let __input = golem_rust::decode_typed_schema_value(&__input)
                .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(__err.to_string()))?;
            let __input_fields = __tool.decode_canonical_input_record(__command_index, __input.value().clone())
                .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(__err.to_string()))?;
            if ::std::mem::size_of::<Self>() != 0 {
                return ::std::result::Result::Err(
                    golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(
                        "guest tool invocation requires a zero-sized implementation type".to_string()
                    )
                );
            }
            let __impl: &Self = unsafe {
                &*::std::ptr::NonNull::<Self>::dangling().as_ptr()
            };

            #(#invoke_arms)*

            for (__subtree_path, __subtool_name) in Self::__tool_invoke_subtree_paths() {
                if __command_path.len() >= __subtree_path.len()
                    && __command_path.iter().zip(__subtree_path.iter()).all(|(__actual, __expected)| __actual == __expected)
                {
                    let __subtool_path = __command_path[__subtree_path.len()..].to_vec();
                    let __subtool_invoker = golem_rust::agentic::get_tool_invoker_by_name(__subtool_name)
                        .ok_or_else(|| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidToolName((*__subtool_name).to_string()))?;
                    let __subtool_input = if let ::std::option::Option::Some(__subtool) = golem_rust::agentic::get_extended_tool_by_name(__subtool_name) {
                        let __subtool_command_index = __subtool.command_index_by_path(&__subtool_path).ok_or_else(|| {
                            golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidCommandPath(
                                __command_path.clone()
                            )
                        })?;
                        let __subtool_fields = __subtool.canonical_input_fields(__subtool_command_index);
                        let __subtool_model = golem_rust::agentic::CanonicalInputModel::from_fields(__subtool_fields.clone())
                            .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(__err.to_string()))?;
                        let mut __subtool_record_fields = ::std::vec::Vec::new();
                        for __field in __subtool_fields.into_iter() {
                            let __value = __input_fields.iter()
                                .find(|__input_field| {
                                    __input_field.name == __field.name
                                        || __input_field.aliases.iter().any(|__alias| __alias == &__field.name)
                                        || __field.aliases.iter().any(|__alias| {
                                            __input_field.name == *__alias
                                                || __input_field.aliases.iter().any(|__input_alias| __input_alias == __alias)
                                        })
                                })
                                .ok_or_else(|| {
                                    golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(
                                        format!("missing canonical tool input field `{}`", __field.name)
                                    )
                                })?;
                            if __value.schema != __field.schema {
                                return ::std::result::Result::Err(
                                    golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(
                                        format!("canonical tool input field `{}` has incompatible schema for forwarded field `{}`", __value.name, __field.name)
                                    )
                                );
                            }
                            __subtool_record_fields.push(__value.value.clone());
                        }
                        golem_rust::TypedSchemaValue::new(
                            __subtool_model.record_schema,
                            golem_rust::SchemaValue::Record { fields: __subtool_record_fields },
                        )
                    } else {
                        __input
                    };
                    return __subtool_invoker(
                        __subtool_path,
                        golem_rust::encode_typed_schema_value(&__subtool_input)
                            .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(__err.to_string()))?,
                        __stdin,
                        __principal,
                    );
                }
            }

            ::std::result::Result::Err(
                golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidCommandPath(__command_path)
            )
        }
    }
}

fn synthesize_invoke_arms(ir: &ToolDefinitionIr) -> Vec<proc_macro2::TokenStream> {
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    ir.commands
        .iter()
        .filter(|cmd| cmd.subtree.is_none())
        .map(|cmd| {
            let method_ident = &cmd.method_ident;
            let method_name = method_ident.to_string();
            let has_stdout = cmd
                .params
                .iter()
                .any(|param| type_last_ident(&param.ty).as_deref() == Some("OutputStream"));
            let args = cmd.params.iter().map(|param| {
                let ident = &param.ident;
                let ty = &param.ty;
                let value_name = canonical_param_name(ir, cmd, param, &tool_name);
                if is_auto_injected_principal_type(ty) {
                    quote! {
                        let #ident = __principal.clone();
                    }
                } else if type_last_ident(ty).as_deref() == Some("InputStream") {
                    quote! {
                        let #ident = __stdin.take().ok_or_else(|| {
                            golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(
                                "tool invocation did not contain declared stdin stream".to_string()
                            )
                        })?;
                    }
                } else if type_last_ident(ty).as_deref() == Some("OutputStream") {
                    quote! {
                        let #ident = golem_rust::wasip2::cli::stdout::get_stdout();
                    }
                } else {
                    quote! {
                        let #ident = {
                            let __field = __input_fields.iter()
                                .find(|__field| __field.name == #value_name)
                                .ok_or_else(|| {
                                    golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(
                                        format!("missing canonical tool input field `{}`", #value_name)
                                    )
                                })?;
                            <#ty as golem_rust::FromSchema>::from_value(&__field.value)
                                .map_err(|__err| golem_rust::golem_agentic::exports::golem::tool::guest::ToolError::InvalidInput(__err.to_string()))?
                        };
                    }
                }
            });
            let arg_idents = cmd.params.iter().map(|param| &param.ident);
            let call = if cmd.is_async {
                quote! { golem_rust::wstd::runtime::block_on(__impl.#method_ident(#(#arg_idents),*)) }
            } else {
                quote! { __impl.#method_ident(#(#arg_idents),*) }
            };
            let encode = encode_invocation_result(&cmd.output, call, has_stdout);
            command_match_arm(&method_name, quote! {
                #(#args)*
                #encode
            })
        })
        .collect()
}

fn command_match_arm(
    method_name: &str,
    body: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        let __method_command_index = Self::__tool_invoke_method_paths()
            .iter()
            .find_map(|(__name, __path)| {
                if *__name == #method_name {
                    let __path = __path.iter().map(|__segment| __segment.to_string()).collect::<::std::vec::Vec<_>>();
                    __tool.command_index_by_path(&__path)
                } else {
                    ::std::option::Option::None
                }
            });
        if __method_command_index == ::std::option::Option::Some(__command_index) {
            #body
        }
    }
}

fn encode_invocation_result(
    output: &ReturnType,
    call: proc_macro2::TokenStream,
    has_stdout: bool,
) -> proc_macro2::TokenStream {
    let (ok, err) = split_result(output);
    let stdout_expr = if has_stdout {
        quote! { ::std::option::Option::Some(golem_rust::wasip2::cli::stdout::get_stdout()) }
    } else {
        quote! { ::std::option::Option::None }
    };
    if err.is_some() {
        match ok {
            Some(_) => quote! {
                match #call {
                    ::std::result::Result::Ok(__value) => return __encode_success_value(&__value, #stdout_expr),
                    ::std::result::Result::Err(__error) => return ::std::result::Result::Err(__encode_custom_error(&__error)?),
                }
            },
            None => quote! {
                match #call {
                    ::std::result::Result::Ok(()) => return __encode_success_unit(#stdout_expr),
                    ::std::result::Result::Err(__error) => return ::std::result::Result::Err(__encode_custom_error(&__error)?),
                }
            },
        }
    } else {
        match ok {
            Some(_) => quote! {
                return __encode_success_value(&#call, #stdout_expr);
            },
            None => quote! {
                #call;
                return __encode_success_unit(#stdout_expr);
            },
        }
    }
}

fn type_last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn is_auto_injected_principal_type(ty: &Type) -> bool {
    let Type::Path(tp) = ty else {
        return false;
    };
    let segments = tp
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let segments = segments.iter().map(String::as_str).collect::<Vec<_>>();
    matches!(
        segments.as_slice(),
        ["golem_rust", "agentic", "Principal"]
            | [
                "golem_rust",
                "golem_agentic",
                "golem",
                "agent",
                "common",
                "Principal"
            ]
    )
}

fn split_result(output: &ReturnType) -> (Option<&Type>, Option<&Type>) {
    let ty = match output {
        ReturnType::Default => return (None, None),
        ReturnType::Type(_, t) => t.as_ref(),
    };
    if let Type::Path(tp) = ty
        && let Some(seg) = tp.path.segments.last()
        && seg.ident == "Result"
        && let PathArguments::AngleBracketed(args) = &seg.arguments
    {
        let mut types = args.args.iter().filter_map(|ga| {
            if let GenericArgument::Type(t) = ga {
                Some(t)
            } else {
                None
            }
        });
        let ok = types.next();
        let err = types.next();
        return (ok.filter(|t| !is_unit(t)), err);
    }
    if is_unit(ty) {
        (None, None)
    } else {
        (Some(ty), None)
    }
}

fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

/// Parses a `#[tool_definition]` trait and its authoring attributes into IR.
pub fn build_tool_definition_ir(
    item_trait: &ItemTrait,
    version: Option<String>,
) -> Result<ToolDefinitionIr, Error> {
    reject_misplaced_method_helper_attrs(item_trait)?;

    let mut commands = Vec::new();
    for item in &item_trait.items {
        if let TraitItem::Fn(method) = item {
            commands.push(build_command_ir(method)?);
        }
    }

    Ok(ToolDefinitionIr {
        trait_ident: item_trait.ident.clone(),
        version,
        doc: parse_doc_full(&item_trait.attrs)?,
        commands,
    })
}

fn build_command_ir(method: &syn::TraitItemFn) -> Result<CommandIr, Error> {
    let mut command_attr = CommandAttr::default();
    let mut command_seen = SeenKeys::default();
    let mut args = Vec::new();
    let mut constraints = Vec::new();
    let mut result = None;

    for attr in &method.attrs {
        let path = attr.path();
        if path.is_ident("command") {
            parse_command_into(attr, &mut command_attr, &mut command_seen)?;
        } else if path.is_ident("arg") {
            args.push(parse_arg(attr)?);
        } else if path.is_ident("constraint") {
            constraints.push(parse_constraint(attr)?);
        } else if path.is_ident("result") {
            if result.is_some() {
                return Err(Error::new(
                    attr.span(),
                    "a command may have at most one #[result(...)] attribute",
                ));
            }
            result = Some(parse_result(attr)?);
        }
    }

    let params = collect_params(&method.sig)?;
    validate_arg_bindings(&args, &params)?;

    Ok(CommandIr {
        method_ident: method.sig.ident.clone(),
        doc: parse_doc_full(&method.attrs)?,
        aliases: command_attr.aliases,
        name_override: command_attr.name_override,
        annotations: command_attr.annotations,
        subtree: command_attr.subtree,
        is_async: method.sig.asyncness.is_some(),
        params,
        output: method.sig.output.clone(),
        args,
        constraints,
        result,
    })
}

/// Ensures every `#[arg(...)]` binds to an existing method parameter and that no
/// parameter is described by more than one `#[arg(...)]`.
fn validate_arg_bindings(args: &[ArgIr], params: &[ParamIr]) -> Result<(), Error> {
    let param_names: BTreeSet<String> = params.iter().map(|p| p.ident.to_string()).collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for arg in args {
        let name = arg.param.to_string();
        if !param_names.contains(&name) {
            return Err(Error::new(
                arg.param.span(),
                format!(
                    "#[arg(...)] refers to unknown parameter `{name}`; the method has no such parameter"
                ),
            ));
        }
        if !seen.insert(name.clone()) {
            return Err(Error::new(
                arg.param.span(),
                format!("duplicate #[arg(...)] for parameter `{name}`"),
            ));
        }
    }
    Ok(())
}

/// Collects the typed parameters of a method, excluding the `&self` receiver.
///
/// Every typed parameter must be a simple named identifier so that metadata synthesis can
/// build the command metadata from this IR without re-reading the trait.
fn collect_params(sig: &syn::Signature) -> Result<Vec<ParamIr>, Error> {
    let mut params = Vec::new();
    for input in &sig.inputs {
        match input {
            FnArg::Receiver(_) => {}
            FnArg::Typed(pat_type) => match &*pat_type.pat {
                syn::Pat::Ident(pat_ident) if pat_ident.subpat.is_none() => {
                    params.push(ParamIr {
                        ident: pat_ident.ident.clone(),
                        ty: (*pat_type.ty).clone(),
                    });
                }
                other => {
                    return Err(Error::new(
                        other.span(),
                        "tool method parameters must be named identifiers, e.g. `name: Type`",
                    ));
                }
            },
        }
    }
    Ok(params)
}

/// Rejects helper attributes used in positions the IR builder does not read,
/// where they would otherwise be silently ignored. The method-only helpers
/// (`#[arg]`, `#[command]`, `#[constraint]`, `#[result]`) are valid only on a
/// tool method; `#[example]` is valid only on the trait or a method. Anywhere
/// else — the trait's non-method items or a method's parameters — is rejected.
fn reject_misplaced_method_helper_attrs(item_trait: &ItemTrait) -> Result<(), Error> {
    fn reject_method_only(attrs: &[Attribute]) -> Result<(), Error> {
        const METHOD_ONLY: [&str; 4] = ["arg", "command", "constraint", "result"];
        for attr in attrs {
            if METHOD_ONLY.iter().any(|name| attr.path().is_ident(name)) {
                return Err(Error::new(
                    attr.span(),
                    "#[arg], #[command], #[constraint], and #[result] may only be used on tool methods",
                ));
            }
        }
        Ok(())
    }

    fn reject_example(attrs: &[Attribute]) -> Result<(), Error> {
        for attr in attrs {
            if attr.path().is_ident("example") {
                return Err(Error::new(
                    attr.span(),
                    "#[example] may only be used on the tool trait or its methods",
                ));
            }
        }
        Ok(())
    }

    // `#[example]` is allowed on the trait itself, so only the method-only
    // helpers are rejected at trait level.
    reject_method_only(&item_trait.attrs)?;

    for item in &item_trait.items {
        match item {
            TraitItem::Fn(method) => {
                // A method's own attributes are valid; its parameters' are not.
                for input in &method.sig.inputs {
                    let attrs = match input {
                        FnArg::Receiver(r) => &r.attrs,
                        FnArg::Typed(t) => &t.attrs,
                    };
                    reject_method_only(attrs)?;
                    reject_example(attrs)?;
                }
            }
            TraitItem::Const(c) => {
                reject_method_only(&c.attrs)?;
                reject_example(&c.attrs)?;
            }
            TraitItem::Type(t) => {
                reject_method_only(&t.attrs)?;
                reject_example(&t.attrs)?;
            }
            TraitItem::Macro(m) => {
                reject_method_only(&m.attrs)?;
                reject_example(&m.attrs)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn strip_helper_attrs(item_trait: &mut ItemTrait) {
    item_trait
        .attrs
        .retain(|attr| !attr.path().is_ident("example"));

    for item in &mut item_trait.items {
        if let TraitItem::Fn(method) = item {
            method
                .attrs
                .retain(|attr| !HELPER_ATTRS.iter().any(|h| attr.path().is_ident(h)));
        }
    }
}

/// Parses the optional `#[tool_definition(version = "...")]` attribute argument.
fn parse_version(attrs: proc_macro2::TokenStream) -> Result<Option<String>, Error> {
    if attrs.is_empty() {
        return Ok(None);
    }
    use syn::parse::Parser;
    use syn::punctuated::Punctuated;
    let parser = Punctuated::<Expr, syn::Token![,]>::parse_terminated;
    let exprs = parser.parse2(attrs)?;
    let mut version = None;
    let mut seen = SeenKeys::default();
    for expr in exprs.iter() {
        let Expr::Assign(assign) = expr else {
            return Err(Error::new(
                expr.span(),
                "expected `version = \"...\"` in #[tool_definition(...)]",
            ));
        };
        let key = match &*assign.left {
            Expr::Path(p) if p.path.get_ident().is_some() => p.path.get_ident().unwrap().clone(),
            other => {
                return Err(Error::new(
                    other.span(),
                    "the only supported #[tool_definition] argument is `version`",
                ));
            }
        };
        if key != "version" {
            return Err(Error::new(
                key.span(),
                "the only supported #[tool_definition] argument is `version`",
            ));
        }
        seen.insert(&key)?;
        match &*assign.right {
            Expr::Lit(syn::ExprLit {
                lit: Lit::Str(s), ..
            }) => version = Some(s.value()),
            other => {
                return Err(Error::new(other.span(), "version must be a string literal"));
            }
        }
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ir(src: &str) -> Result<ToolDefinitionIr, Error> {
        let item_trait: ItemTrait = syn::parse_str(src).unwrap();
        build_tool_definition_ir(&item_trait, None)
    }

    fn version(src: &str) -> Result<Option<String>, Error> {
        let attrs: proc_macro2::TokenStream = src.parse().unwrap();
        parse_version(attrs)
    }

    #[test]
    fn version_is_parsed() {
        assert_eq!(version("").unwrap(), None);
        assert_eq!(
            version(r#"version = "1.2.3""#).unwrap(),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn duplicate_tool_definition_version_is_error() {
        let err = version(r#"version = "1.0.0", version = "2.0.0""#).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn unknown_tool_definition_arg_is_error() {
        let err = version(r#"flavor = "fast""#).unwrap_err();
        assert!(err.to_string().contains("only supported"));
    }

    #[test]
    fn non_string_version_is_error() {
        let err = version("version = 3").unwrap_err();
        assert!(err.to_string().contains("string literal"));
    }

    #[test]
    fn grep_like_trait() {
        let def = ir(r##"
            /// Search files for a regex pattern.
            pub trait Grep {
                #[arg(case_sensitive = "global", short = 'i', kind = "flag")]
                #[arg(pattern = "positional", regex = r"^.+$")]
                #[arg(files = "tail", accepts_stdio = true)]
                #[result(formatters = ["human", "json"], default = "human")]
                async fn grep(&self, case_sensitive: bool, pattern: RegexString, files: Vec<Path>)
                    -> Result<Vec<Hit>, GrepError>;
            }
        "##)
        .unwrap();
        assert_eq!(def.trait_ident.to_string(), "Grep");
        assert_eq!(def.doc.summary, "Search files for a regex pattern.");
        assert_eq!(def.commands.len(), 1);
        let cmd = &def.commands[0];
        assert_eq!(cmd.method_ident.to_string(), "grep");
        assert_eq!(cmd.args.len(), 3);
        assert!(cmd.result.is_some());
    }

    #[test]
    fn command_with_subtree_and_constraint() {
        let def = ir(r#"
            pub trait Git {
                #[command(aliases = ["ci"], annotations(destructive = true))]
                #[arg(message = "option", short = 'm', required = true)]
                #[constraint(implies(lhs = "reset-author", rhs = "amend"))]
                async fn commit(&self, message: String) -> Result<(), CommitError>;

                #[command(subtree = path::Remote, aliases = ["rmt"])]
                fn remote(&self) -> Remote;
            }
        "#)
        .unwrap();
        assert_eq!(def.commands.len(), 2);
        let commit = &def.commands[0];
        assert_eq!(commit.aliases, vec!["ci".to_string()]);
        assert!(commit.annotations.is_some());
        assert_eq!(commit.constraints.len(), 1);
        let remote = &def.commands[1];
        assert!(remote.subtree.is_some());
        assert_eq!(remote.aliases, vec!["rmt".to_string()]);
    }

    #[test]
    fn duplicate_result_is_error() {
        let err = ir(r#"
            pub trait T {
                #[result(formatters = ["a"])]
                #[result(formatters = ["b"])]
                async fn f(&self) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("at most one #[result"));
    }

    #[test]
    fn bad_arg_is_error() {
        let err = ir(r#"
            pub trait T {
                #[arg(x = "nope")]
                async fn f(&self) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("invalid arg placement"));
    }

    #[test]
    fn params_and_output_are_captured() {
        let def = ir(r#"
            pub trait T {
                async fn f(&self, name: String, count: u32) -> Result<Vec<Hit>, E>;
            }
        "#)
        .unwrap();
        let cmd = &def.commands[0];
        let params: Vec<String> = cmd.params.iter().map(|p| p.ident.to_string()).collect();
        assert_eq!(params, vec!["name".to_string(), "count".to_string()]);
        assert!(matches!(cmd.output, syn::ReturnType::Type(..)));
    }

    #[test]
    fn trait_level_method_helper_attr_is_error() {
        let err = ir(r#"
            #[arg(name = "positional")]
            pub trait T {
                async fn f(&self, name: String) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("may only be used on tool methods"));
    }

    #[test]
    fn arg_for_unknown_param_is_error() {
        let err = ir(r#"
            pub trait T {
                #[arg(missing = "option")]
                async fn f(&self, name: String) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn duplicate_arg_for_same_param_is_error() {
        let err = ir(r#"
            pub trait T {
                #[arg(name = "option")]
                #[arg(name = "flag")]
                async fn f(&self, name: bool) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn non_ident_param_is_error() {
        let err = ir(r#"
            pub trait T {
                async fn f(&self, (a, b): (u32, u32)) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("named identifiers"));
    }

    #[test]
    fn helper_attr_on_associated_type_is_error() {
        let err = ir(r#"
            pub trait T {
                #[arg(name = "option")]
                type State;

                async fn f(&self) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("may only be used on tool methods"));
    }

    #[test]
    fn helper_attr_on_parameter_is_error() {
        let err = ir(r#"
            pub trait T {
                async fn f(
                    &self,
                    #[arg(name = "option")]
                    name: String,
                ) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("may only be used on tool methods"));
    }

    #[test]
    fn example_on_associated_type_is_error() {
        let err = ir(r#"
            pub trait T {
                #[example(body = "ignored")]
                type State;

                async fn f(&self) -> Result<(), E>;
            }
        "#)
        .unwrap_err();
        assert!(err.to_string().contains("example"));
    }
}
