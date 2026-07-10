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

//! Typed same-language tool client generation.

use crate::tool::helpers::to_kebab_case;
use crate::tool::ir::{ArgPlacement, ArgSubKind, CommandIr, ParamIr, ToolDefinitionIr};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, GenericArgument, Ident, Path, PathArguments, ReturnType, Type};

pub fn synthesize_client(ir: &ToolDefinitionIr) -> TokenStream {
    let client_ident = format_ident!("{}Client", ir.trait_ident);
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    let constructor_ident = constructor_ident(ir);
    let methods = ir
        .commands
        .iter()
        .map(|cmd| synthesize_method(ir, cmd, &tool_name));
    let subtree_wrappers = ir
        .commands
        .iter()
        .filter_map(|cmd| synthesize_subtree_wrapper(ir, cmd));
    let subtree_macro = synthesize_subtree_client_macro(ir, &tool_name);

    quote! {
        #(#subtree_wrappers)*

        pub struct #client_ident {
            rpc: golem_rust::golem_agentic::golem::tool::host::ToolRpc,
            root_tool_name: ::std::string::String,
            command_path: ::std::vec::Vec<::std::string::String>,
            schema_path: ::std::vec::Vec<::std::string::String>,
            inherited_prefix: ::std::vec::Vec<golem_rust::agentic::CanonicalInputValue>,
        }

        impl #client_ident {
            pub fn #constructor_ident() -> Self {
                Self {
                    rpc: golem_rust::golem_agentic::golem::tool::host::ToolRpc::new(#tool_name),
                    root_tool_name: #tool_name.to_string(),
                    command_path: ::std::vec::Vec::new(),
                    schema_path: ::std::vec::Vec::new(),
                    inherited_prefix: ::std::vec::Vec::new(),
                }
            }

            #(#methods)*
        }

        impl golem_rust::agentic::ToolClientWithParts for #client_ident {
            fn __golem_tool_client_with_parts(
                root_tool_name: ::std::string::String,
                command_path: ::std::vec::Vec<::std::string::String>,
                schema_path: ::std::vec::Vec<::std::string::String>,
                inherited_prefix: ::std::vec::Vec<golem_rust::agentic::CanonicalInputValue>,
            ) -> Self {
                Self {
                    rpc: golem_rust::golem_agentic::golem::tool::host::ToolRpc::new(&root_tool_name),
                    root_tool_name,
                    command_path,
                    schema_path,
                    inherited_prefix,
                }
            }
        }

        impl ::std::default::Default for #client_ident {
            fn default() -> Self {
                Self::#constructor_ident()
            }
        }

        #subtree_macro
    }
}

/// The generated expression building the invocation's input record. The fast
/// path (no inherited prefix, root schema path) resolves the command's
/// canonical input model once per method through a `OnceLock`; the general
/// path recomputes it per call from the descriptor plus the inherited prefix.
/// Record assembly itself is shared runtime code in `golem_rust::agentic`.
fn input_build_expr(descriptor_fn_ident: &Ident, param_values: TokenStream) -> TokenStream {
    quote! {
        if __can_use_static_input_model {
            static __GOLEM_TOOL_INPUT_MODEL: ::std::sync::OnceLock<
                ::std::result::Result<golem_rust::agentic::CanonicalInputModel, ::std::string::String>
            > =
                ::std::sync::OnceLock::new();
            let __model = __GOLEM_TOOL_INPUT_MODEL.get_or_init(|| {
                let __tool = #descriptor_fn_ident(&mut golem_rust::agentic::ToolBuildCtx::new())
                    .expect("tool descriptor build failed");
                let __command_index = __tool.command_index_by_path(&__schema_path).ok_or_else(|| {
                    format!("invalid generated tool command path `{}`", __schema_path.join(" "))
                })?;
                __tool.canonical_input_model(__command_index)
                    .map_err(|__err| __err.to_string())
            }).as_ref().map_err(|__err| {
                golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(__err.clone()))
            })?;
            golem_rust::agentic::build_canonical_input(__model, #param_values)
                .map_err(|__err| golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(__err)))?
        } else {
            let __tool = #descriptor_fn_ident(&mut golem_rust::agentic::ToolBuildCtx::new())
                .expect("tool descriptor build failed");
            let __command_index = __tool.command_index_by_path(&__schema_path).ok_or_else(|| {
                golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                    format!("invalid generated tool command path `{}`", __schema_path.join(" "))
                ))
            })?;
            golem_rust::agentic::build_canonical_input_with_prefix(
                __tool.canonical_input_fields(__command_index),
                &self.inherited_prefix,
                #param_values,
            )
                .map_err(|__err| golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(__err)))?
        }
    }
}

fn synthesize_method(ir: &ToolDefinitionIr, cmd: &CommandIr, tool_name: &str) -> TokenStream {
    if let Some(subtree) = &cmd.subtree {
        return synthesize_subtree_method(ir, cmd, subtree, tool_name, &[]);
    }

    synthesize_leaf_method(ir, cmd, tool_name, &[])
}

fn synthesize_leaf_method(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    omitted_names: &[String],
) -> TokenStream {
    let method_ident = &cmd.method_ident;
    let descriptor_fn_ident = crate::tool::descriptor::descriptor_fn_ident(&ir.trait_ident);
    let command_name = command_name(cmd, tool_name);
    let command_path_part = if command_name == tool_name {
        quote! {}
    } else {
        quote! {
            __command_path.push(#command_name.to_string());
            __schema_path.push(#command_name.to_string());
        }
    };
    let inherited_params = inherited_root_params(ir, cmd, tool_name);
    let input_args =
        kept_client_args_omitting(ir, cmd, &inherited_params, false, omitted_names, tool_name);
    let (stdin_ident, has_stdout) = stream_idents(cmd);
    let stdin_expr = match stdin_ident {
        Some(ident) => quote! { ::std::option::Option::Some(#ident) },
        None => quote! { ::std::option::Option::None },
    };
    let value_inserts = value_inserts(ir, cmd, &inherited_params, tool_name, omitted_names);
    let result_ty = client_result_type(&cmd.output, has_stdout);
    let decode_result = decode_client_result(&cmd.output, has_stdout);
    let invoke = invoke_call(&cmd.output, stdin_expr);
    let input_expr = input_build_expr(&descriptor_fn_ident, quote! { __golem_param_values });

    quote! {
        pub async fn #method_ident(&self, #(#input_args),*) -> #result_ty {
            #(#value_inserts)*

            let __can_use_static_input_model = self.inherited_prefix.is_empty() && self.schema_path.is_empty();
            let mut __command_path = self.command_path.clone();
            let mut __schema_path = self.schema_path.clone();
            #command_path_part
            let __input = #input_expr;

            let __result = #invoke?;
            #decode_result
        }
    }
}

fn synthesize_subtree_method(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    _subtree: &crate::tool::ir::SubtreeIr,
    tool_name: &str,
    omitted_names: &[String],
) -> TokenStream {
    let method_ident = &cmd.method_ident;
    let command_name = cmd
        .name_override
        .clone()
        .unwrap_or_else(|| to_kebab_case(&cmd.method_ident.to_string()));
    let inherited_params = inherited_root_params(ir, cmd, tool_name);
    let input_args =
        kept_client_args_omitting(ir, cmd, &inherited_params, true, omitted_names, tool_name);
    let value_prefixes =
        prefix_value_builders(ir, cmd, &inherited_params, tool_name, omitted_names);
    let wrapper_ident = subtree_wrapper_ident(ir, cmd);
    let inherited_surfaces = inherited_root_param_surfaces(ir, cmd, tool_name);
    let child_omitted =
        subtree_child_omitted_surfaces(ir, cmd, tool_name, &inherited_surfaces, omitted_names);
    let child_omitted_tag = omitted_tag(quote! { 0 }, &child_omitted);
    let child_omitted_ty = omitted_type(quote! { () }, &child_omitted);
    let _ = tool_name;

    quote! {
        pub fn #method_ident(&self, #(#input_args),*) -> #wrapper_ident<{ #child_omitted_tag }, #child_omitted_ty> {
            let mut __command_path = self.command_path.clone();
            __command_path.push(#command_name.to_string());
            let __schema_path = ::std::vec::Vec::new();
            let mut __inherited_prefix = self.inherited_prefix.clone();
            #(#value_prefixes)*
            #wrapper_ident::<{ #child_omitted_tag }, #child_omitted_ty> {
                rpc: golem_rust::golem_agentic::golem::tool::host::ToolRpc::new(&self.root_tool_name),
                root_tool_name: self.root_tool_name.clone(),
                command_path: __command_path,
                schema_path: __schema_path,
                inherited_prefix: __inherited_prefix,
                _omitted: ::std::marker::PhantomData,
            }
        }
    }
}

fn synthesize_leaf_method_dynamic(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    input_args: TokenStream,
    value_inserts: TokenStream,
    param_values: TokenStream,
) -> TokenStream {
    let method_ident = &cmd.method_ident;
    let descriptor_fn_ident = crate::tool::descriptor::descriptor_fn_ident(&ir.trait_ident);
    let command_name = command_name(cmd, tool_name);
    let command_path_part = if command_name == tool_name {
        quote! {}
    } else {
        quote! {
            __command_path.push(#command_name.to_string());
            __schema_path.push(#command_name.to_string());
        }
    };
    let (_, has_stdout) = stream_idents(cmd);
    let stdin_expr = match cmd
        .params
        .iter()
        .find(|param| type_last_ident(&param.ty).as_deref() == Some("InputStream"))
        .map(|param| &param.ident)
    {
        Some(ident) => quote! { ::std::option::Option::Some(#ident) },
        None => quote! { ::std::option::Option::None },
    };
    let result_ty = client_result_type(&cmd.output, has_stdout);
    let decode_result = decode_client_result(&cmd.output, has_stdout);
    let invoke = invoke_call(&cmd.output, stdin_expr);
    let input_expr = input_build_expr(&descriptor_fn_ident, param_values.clone());

    quote! {
        pub async fn #method_ident(&self #input_args) -> #result_ty {
            let mut #param_values: ::std::vec::Vec<(&'static str, golem_rust::SchemaValue)> =
                ::std::vec::Vec::new();
            #value_inserts

            let __can_use_static_input_model = self.inherited_prefix.is_empty() && self.schema_path.is_empty();
            let mut __command_path = self.command_path.clone();
            let mut __schema_path = self.schema_path.clone();
            #command_path_part
            let __input = #input_expr;

            let __result = #invoke?;
            #decode_result
        }
    }
}

fn synthesize_subtree_method_dynamic(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    _subtree: &crate::tool::ir::SubtreeIr,
    tool_name: &str,
    input_args: TokenStream,
    value_prefixes: TokenStream,
    inherited_prefix: TokenStream,
    child_omitted_tag: TokenStream,
    child_omitted_ty: TokenStream,
) -> TokenStream {
    let method_ident = &cmd.method_ident;
    let command_name = cmd
        .name_override
        .clone()
        .unwrap_or_else(|| to_kebab_case(&cmd.method_ident.to_string()));
    let wrapper_ident = subtree_wrapper_ident(ir, cmd);
    let _ = tool_name;

    quote! {
        pub fn #method_ident(&self #input_args) -> #wrapper_ident<{ #child_omitted_tag }, #child_omitted_ty> {
            let mut __command_path = self.command_path.clone();
            __command_path.push(#command_name.to_string());
            let __schema_path = ::std::vec::Vec::new();
            let mut #inherited_prefix = self.inherited_prefix.clone();
            #value_prefixes
            #wrapper_ident::<{ #child_omitted_tag }, #child_omitted_ty> {
                rpc: golem_rust::golem_agentic::golem::tool::host::ToolRpc::new(&self.root_tool_name),
                root_tool_name: self.root_tool_name.clone(),
                command_path: __command_path,
                schema_path: __schema_path,
                inherited_prefix: #inherited_prefix,
                _omitted: ::std::marker::PhantomData,
            }
        }
    }
}

fn synthesize_subtree_wrapper(ir: &ToolDefinitionIr, cmd: &CommandIr) -> Option<TokenStream> {
    let subtree = cmd.subtree.as_ref()?;
    let wrapper_ident = subtree_wrapper_ident(ir, cmd);
    let child_macro_path = subtree_client_macro_path(&subtree.path);
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    let inherited_surfaces = inherited_root_param_surfaces(ir, cmd, &tool_name);
    let root_child_omitted =
        subtree_child_omitted_surfaces(ir, cmd, &tool_name, &inherited_surfaces, &[]);
    let root_child_omitted_tag = omitted_tag(quote! { 0 }, &root_child_omitted);
    let root_child_omitted_ty = omitted_type(quote! { () }, &root_child_omitted);
    let root_child_omitted_markers = omitted_markers(&root_child_omitted);
    let root_child_macro_invocation = if root_child_omitted.is_empty() {
        quote! { #child_macro_path!(#wrapper_ident); }
    } else {
        quote! { #child_macro_path!(#wrapper_ident, #root_child_omitted_tag, #root_child_omitted_ty, [#(#root_child_omitted_markers)*]); }
    };
    Some(quote! {
        pub struct #wrapper_ident<const __GOLEM_OMITTED_TAG: u64 = 0, __GOLEM_OMITTED = ()> {
            rpc: golem_rust::golem_agentic::golem::tool::host::ToolRpc,
            root_tool_name: ::std::string::String,
            command_path: ::std::vec::Vec<::std::string::String>,
            schema_path: ::std::vec::Vec<::std::string::String>,
            inherited_prefix: ::std::vec::Vec<golem_rust::agentic::CanonicalInputValue>,
            _omitted: ::std::marker::PhantomData<fn() -> __GOLEM_OMITTED>,
        }

        #root_child_macro_invocation
    })
}

fn synthesize_subtree_client_macro(ir: &ToolDefinitionIr, tool_name: &str) -> TokenStream {
    let macro_ident = subtree_client_macro_ident(&ir.trait_ident);
    let command_arms = subtree_client_macro_command_arms(ir, tool_name, &macro_ident);
    let command_starts = ir.commands.iter().enumerate().map(|(idx, cmd)| {
        let state = subtree_client_macro_command_state_ident(idx, 0);
        if cmd.subtree.is_some() {
            let salt = subtree_context_salt(cmd);
            quote! {
                #macro_ident!(@#state $client_ident, $omitted_tag, $omitted_ty, __golem_inherited_prefix, [$($omitted)*], [], [], [], ($omitted_tag ^ #salt), ($omitted_ty, fn() -> $client_ident<{ $omitted_tag }, $omitted_ty>) ; $($omitted)*);
            }
        } else {
            quote! {
                #macro_ident!(@#state $client_ident, $omitted_tag, $omitted_ty, __golem_param_values, [$($omitted)*], [], [] ; $($omitted)*);
            }
        }
    });

    quote! {
        #[doc(hidden)]
        macro_rules! #macro_ident {
            ($client_ident:ident) => {
                #macro_ident!($client_ident, 0, (), []);
            };
            ($client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, [$($omitted:ident)*]) => {
                #(#command_starts)*
            };
            #(#command_arms)*
        }

        #[doc(hidden)]
        pub(crate) use #macro_ident;
    }
}

fn subtree_client_macro_command_arms(
    ir: &ToolDefinitionIr,
    tool_name: &str,
    macro_ident: &Ident,
) -> Vec<TokenStream> {
    ir.commands
        .iter()
        .enumerate()
        .flat_map(|(cmd_idx, cmd)| {
            if let Some(subtree) = &cmd.subtree {
                subtree_client_macro_subtree_command_arms(
                    ir,
                    cmd,
                    subtree,
                    tool_name,
                    macro_ident,
                    cmd_idx,
                )
            } else {
                subtree_client_macro_leaf_command_arms(ir, cmd, tool_name, macro_ident, cmd_idx)
            }
        })
        .collect()
}

fn subtree_client_macro_leaf_command_arms(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    macro_ident: &Ident,
    cmd_idx: usize,
) -> Vec<TokenStream> {
    let inherited_params = inherited_root_params(ir, cmd, tool_name);
    let params: Vec<_> = inherited_params
        .iter()
        .chain(cmd.params.iter())
        .filter(|param| {
            !is_principal_type(&param.ty)
                && type_last_ident(&param.ty).as_deref() != Some("OutputStream")
        })
        .cloned()
        .collect();
    let mut arms =
        subtree_client_macro_param_arms(ir, cmd, tool_name, macro_ident, cmd_idx, &params, false);
    let done_state = subtree_client_macro_command_state_ident(cmd_idx, params.len());
    let method = synthesize_leaf_method_dynamic(
        ir,
        cmd,
        tool_name,
        quote! { $($args)* },
        quote! { $($values)* },
        quote! { $param_values },
    );
    arms.push(quote! {
        (@#done_state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $param_values:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*] ; $($rest:ident)*) => {
            impl $client_ident<{ $omitted_tag }, $omitted_ty> {
                #method
            }
        };
    });
    arms
}

fn subtree_client_macro_subtree_command_arms(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    subtree: &crate::tool::ir::SubtreeIr,
    tool_name: &str,
    macro_ident: &Ident,
    cmd_idx: usize,
) -> Vec<TokenStream> {
    let inherited_params = inherited_root_params(ir, cmd, tool_name);
    let params: Vec<_> = inherited_params
        .iter()
        .chain(cmd.params.iter())
        .filter(|param| !is_principal_type(&param.ty))
        .cloned()
        .collect();
    let mut arms =
        subtree_client_macro_param_arms(ir, cmd, tool_name, macro_ident, cmd_idx, &params, true);
    let done_state = subtree_client_macro_command_state_ident(cmd_idx, params.len());
    let method = synthesize_subtree_method_dynamic(
        ir,
        cmd,
        subtree,
        tool_name,
        quote! { $($args)* },
        quote! { $($values)* },
        quote! { $inherited_prefix },
        quote! { $child_tag },
        quote! { $child_ty },
    );
    let child_macro_path = subtree_client_macro_path(&subtree.path);
    let wrapper_ident = subtree_wrapper_ident(ir, cmd);
    arms.push(quote! {
        (@#done_state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $inherited_prefix:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*], [$($child_markers:ident)*], $child_tag:expr, $child_ty:ty ; $($rest:ident)*) => {
            impl $client_ident<{ $omitted_tag }, $omitted_ty> {
                #method
            }
            #child_macro_path!(#wrapper_ident, $child_tag, $child_ty, [$($all)* $($child_markers)*]);
        };
    });
    arms
}

fn subtree_client_macro_param_arms(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    macro_ident: &Ident,
    cmd_idx: usize,
    params: &[ParamIr],
    is_subtree_command: bool,
) -> Vec<TokenStream> {
    params
        .iter()
        .enumerate()
        .flat_map(|(param_idx, param)| {
            let state = subtree_client_macro_command_state_ident(cmd_idx, param_idx);
            let next_state = subtree_client_macro_command_state_ident(cmd_idx, param_idx + 1);
            let keep = subtree_client_macro_keep_param(
                ir,
                cmd,
                param,
                tool_name,
                macro_ident,
                &next_state,
                is_subtree_command,
            );
            if is_stream_type(&param.ty) {
                return if is_subtree_command {
                    vec![quote! {
                        (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $inherited_prefix:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*], [$($child_markers:ident)*], $child_tag:expr, $child_ty:ty ; $($rest:ident)*) => {
                            #keep
                        };
                    }]
                } else {
                    vec![quote! {
                        (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $param_values:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*] ; $($rest:ident)*) => {
                            #keep
                        };
                    }]
                };
            }

            let markers = omitted_markers(&param_omission_surfaces(ir, cmd, param, tool_name));
            let omit = subtree_client_macro_omit_param(macro_ident, &next_state, is_subtree_command);
            if is_subtree_command {
                let marker_arms = markers.iter().map(|marker| {
                    quote! {
                        (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $inherited_prefix:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*], [$($child_markers:ident)*], $child_tag:expr, $child_ty:ty ; #marker $($rest:ident)*) => {
                            #omit
                        };
                    }
                });
                let unknown = quote! {
                    (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $inherited_prefix:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*], [$($child_markers:ident)*], $child_tag:expr, $child_ty:ty ; $unknown:ident $($rest:ident)*) => {
                        #macro_ident!(@#state $client_ident, $omitted_tag, $omitted_ty, $inherited_prefix, [$($all)*], [$($args)*], [$($values)*], [$($child_markers)*], $child_tag, $child_ty ; $($rest)*);
                    };
                };
                let empty = quote! {
                    (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $inherited_prefix:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*], [$($child_markers:ident)*], $child_tag:expr, $child_ty:ty ; ) => {
                        #keep
                    };
                };
                marker_arms.chain([unknown, empty]).collect::<Vec<_>>()
            } else {
                let marker_arms = markers.iter().map(|marker| {
                    quote! {
                        (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $param_values:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*] ; #marker $($rest:ident)*) => {
                            #omit
                        };
                    }
                });
                let unknown = quote! {
                    (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $param_values:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*] ; $unknown:ident $($rest:ident)*) => {
                        #macro_ident!(@#state $client_ident, $omitted_tag, $omitted_ty, $param_values, [$($all)*], [$($args)*], [$($values)*] ; $($rest)*);
                    };
                };
                let empty = quote! {
                    (@#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, $param_values:ident, [$($all:ident)*], [$($args:tt)*], [$($values:tt)*] ; ) => {
                        #keep
                    };
                };
                marker_arms.chain([unknown, empty]).collect::<Vec<_>>()
            }
        })
        .collect()
}

fn subtree_client_macro_keep_param(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
    macro_ident: &Ident,
    next_state: &Ident,
    is_subtree_command: bool,
) -> TokenStream {
    let ident = &param.ident;
    let ty = &param.ty;
    let arg = quote! { , #ident: #ty };
    let value = if is_stream_type(&param.ty) {
        quote! {}
    } else if is_subtree_command {
        let name = canonical_value_name(ir, cmd, param, tool_name);
        let aliases = canonical_param_aliases(ir, cmd, param, tool_name);
        let aliases = aliases.iter();
        quote! {
            $inherited_prefix.push(golem_rust::agentic::CanonicalInputValue {
                name: #name.to_string(),
                aliases: ::std::vec![#(#aliases.to_string()),*],
                schema: <#ty as golem_rust::agentic::Schema>::get_type()
                    .get_schema_graph()
                    .expect("tool parameter must have a concrete schema graph"),
                value: <#ty as golem_rust::agentic::Schema>::to_schema_value(#ident)
                    .expect("failed to encode tool parameter"),
            });
        }
    } else {
        let name = canonical_value_name(ir, cmd, param, tool_name);
        quote! {
            let __golem_value = <_ as golem_rust::agentic::Schema>::to_schema_value(#ident)
                .expect("failed to encode tool parameter");
            $param_values.push((#name, __golem_value));
        }
    };

    if is_subtree_command {
        let command_param = cmd.params.iter().any(|own| own.ident == param.ident);
        let (new_markers, new_tag, new_ty) = if command_param && !is_stream_type(&param.ty) {
            let surfaces = param_surfaces(cmd, param);
            let markers = omitted_markers(&surfaces);
            let tag = omitted_tag_append(quote! { $child_tag }, &surfaces);
            let ty = omitted_type(quote! { $child_ty }, &surfaces);
            (quote! { #(#markers)* }, tag, ty)
        } else {
            (quote! {}, quote! { $child_tag }, quote! { $child_ty })
        };
        quote! {
            #macro_ident!(@#next_state $client_ident, $omitted_tag, $omitted_ty, $inherited_prefix, [$($all)*], [$($args)* #arg], [$($values)* #value], [$($child_markers)* #new_markers], #new_tag, #new_ty ; $($all)*);
        }
    } else {
        quote! {
            #macro_ident!(@#next_state $client_ident, $omitted_tag, $omitted_ty, $param_values, [$($all)*], [$($args)* #arg], [$($values)* #value] ; $($all)*);
        }
    }
}

fn subtree_client_macro_omit_param(
    macro_ident: &Ident,
    next_state: &Ident,
    is_subtree_command: bool,
) -> TokenStream {
    if is_subtree_command {
        quote! {
            #macro_ident!(@#next_state $client_ident, $omitted_tag, $omitted_ty, $inherited_prefix, [$($all)*], [$($args)*], [$($values)*], [$($child_markers)*], $child_tag, $child_ty ; $($all)*);
        }
    } else {
        quote! {
            #macro_ident!(@#next_state $client_ident, $omitted_tag, $omitted_ty, $param_values, [$($all)*], [$($args)*], [$($values)*] ; $($all)*);
        }
    }
}

fn subtree_client_macro_command_state_ident(cmd_idx: usize, param_idx: usize) -> Ident {
    format_ident!("__golem_cmd_{}_param_{}", cmd_idx, param_idx)
}

fn kept_client_args_omitting(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    inherited_params: &[ParamIr],
    include_stdout: bool,
    omitted_names: &[String],
    tool_name: &str,
) -> Vec<FnArg> {
    inherited_params
        .iter()
        .chain(cmd.params.iter())
        .filter(|param| {
            !omitted_names
                .iter()
                .any(|omitted| omitted_matches_param(ir, cmd, param, tool_name, omitted))
                && !is_principal_type(&param.ty)
                && (include_stdout || type_last_ident(&param.ty).as_deref() != Some("OutputStream"))
        })
        .map(|param| {
            let ident = &param.ident;
            let ty = &param.ty;
            syn::parse_quote! { #ident: #ty }
        })
        .collect()
}

fn value_inserts(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    inherited_params: &[ParamIr],
    tool_name: &str,
    omitted_names: &[String],
) -> Vec<TokenStream> {
    let inserts = inherited_params
        .iter()
        .chain(cmd.params.iter())
        .filter_map(|param| {
            if is_principal_type(&param.ty) || is_stream_type(&param.ty) {
                return None;
            }
            if omitted_names
                .iter()
                .any(|omitted| omitted_matches_param(ir, cmd, param, tool_name, omitted))
            {
                return None;
            }
            let ident = &param.ident;
            let name = canonical_value_name(ir, cmd, param, tool_name);
            Some(quote! {
                let __golem_value = <_ as golem_rust::agentic::Schema>::to_schema_value(#ident)
                    .expect("failed to encode tool parameter");
                __golem_param_values.push((#name, __golem_value));
            })
        });
    let inserts: Vec<_> = inserts.collect();
    let capacity = inserts.len();
    vec![quote! {
            let mut __golem_param_values: ::std::vec::Vec<(&'static str, golem_rust::SchemaValue)> =
                ::std::vec::Vec::with_capacity(#capacity);
            #(#inserts)*
    }]
}

fn subtree_child_omitted_surfaces(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    inherited_surfaces: &[String],
    inherited_omitted: &[String],
) -> Vec<String> {
    let mut surfaces = inherited_omitted.to_vec();
    for surface in inherited_surfaces {
        if !surfaces.iter().any(|existing| existing == surface) {
            surfaces.push(surface.clone());
        }
    }
    for param in cmd
        .params
        .iter()
        .filter(|param| !is_principal_type(&param.ty) && !is_stream_type(&param.ty))
    {
        let canonical_name = canonical_value_name(ir, cmd, param, tool_name);
        if !surfaces.iter().any(|existing| existing == &canonical_name) {
            surfaces.push(canonical_name);
        }
        for surface in param_surfaces(cmd, param) {
            if !surfaces.iter().any(|existing| existing == &surface) {
                surfaces.push(surface);
            }
        }
    }
    surfaces
}

fn inherited_root_param_surfaces(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
) -> Vec<String> {
    let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    else {
        return Vec::new();
    };
    inherited_root_params(ir, cmd, tool_name)
        .iter()
        .flat_map(|param| param_surfaces(root, param))
        .collect()
}

fn omitted_type(base: TokenStream, surfaces: &[String]) -> TokenStream {
    surfaces.iter().fold(base, |ty, surface| {
        let id = omitted_surface_id(surface);
        quote! { (#ty, golem_rust::agentic::OmittedSurface<#id>) }
    })
}

fn omitted_tag(base: TokenStream, surfaces: &[String]) -> TokenStream {
    omitted_tag_append(base, surfaces)
}

fn omitted_tag_append(base: TokenStream, surfaces: &[String]) -> TokenStream {
    surfaces.iter().fold(base, |tag, surface| {
        let id = omitted_surface_id(surface);
        quote! { (#tag | #id) }
    })
}

fn subtree_context_salt(cmd: &CommandIr) -> u64 {
    omitted_surface_id(&format!(
        "subtree:{}",
        cmd.name_override
            .clone()
            .unwrap_or_else(|| to_kebab_case(&cmd.method_ident.to_string()))
    ))
}

fn omitted_markers(surfaces: &[String]) -> Vec<Ident> {
    surfaces
        .iter()
        .map(|surface| omitted_marker_ident(surface))
        .collect()
}

fn omitted_marker_ident(surface: &str) -> Ident {
    format_ident!("__golem_omitted_{}", omitted_surface_id(surface))
}

fn omitted_surface_id(surface: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in surface.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn omitted_matches_param(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
    omitted: &str,
) -> bool {
    if param_surfaces(cmd, param)
        .iter()
        .any(|surface| surface == omitted)
    {
        return true;
    }
    if to_kebab_case(&cmd.method_ident.to_string()) == tool_name {
        return false;
    }
    let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    else {
        return false;
    };
    let own_name = to_kebab_case(&param.ident.to_string());
    let own_aliases = param_aliases(cmd, param);
    root.params.iter().any(|root_param| {
        let root_name = to_kebab_case(&root_param.ident.to_string());
        is_global_param(root, root_param)
            && omitted == root_name
            && param_surfaces_intersect(
                &root_name,
                &param_aliases(root, root_param),
                &own_name,
                &own_aliases,
            )
    })
}

fn param_omission_surfaces(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> Vec<String> {
    let mut surfaces = param_surfaces(cmd, param);
    if to_kebab_case(&cmd.method_ident.to_string()) == tool_name {
        return surfaces;
    }
    let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    else {
        return surfaces;
    };
    let own_name = to_kebab_case(&param.ident.to_string());
    let own_aliases = param_aliases(cmd, param);
    for root_param in &root.params {
        if !is_global_param(root, root_param) {
            continue;
        }
        let root_name = to_kebab_case(&root_param.ident.to_string());
        let root_aliases = param_aliases(root, root_param);
        if param_surfaces_intersect(&root_name, &root_aliases, &own_name, &own_aliases)
            && !surfaces.iter().any(|surface| surface == &root_name)
        {
            surfaces.push(root_name);
        }
    }
    surfaces
}

fn param_surfaces(cmd: &CommandIr, param: &ParamIr) -> Vec<String> {
    let mut surfaces = vec![to_kebab_case(&param.ident.to_string())];
    for alias in param_aliases(cmd, param) {
        if !surfaces.iter().any(|surface| surface == &alias) {
            surfaces.push(alias);
        }
    }
    surfaces
}

fn prefix_value_builders(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    inherited_params: &[ParamIr],
    tool_name: &str,
    omitted_names: &[String],
) -> Vec<TokenStream> {
    let mut inherited: Vec<&ParamIr> = inherited_params.iter().collect();
    inherited.sort_by_key(|param| if is_flag_param(cmd, param) { 1 } else { 0 });
    let mut current: Vec<&ParamIr> = cmd.params.iter().collect();
    current.sort_by_key(|param| if is_flag_param(cmd, param) { 1 } else { 0 });
    inherited
        .into_iter()
        .chain(current)
        .filter_map(|param| {
            if is_principal_type(&param.ty) || is_stream_type(&param.ty) {
                return None;
            }
            if omitted_names
                .iter()
                .any(|omitted| omitted_matches_param(ir, cmd, param, tool_name, omitted))
            {
                return None;
            }
            let ident = &param.ident;
            let ty = &param.ty;
            let name = canonical_value_name(ir, cmd, param, tool_name);
            let aliases = canonical_param_aliases(ir, cmd, param, tool_name);
            let aliases = aliases.iter();
            Some(quote! {
                __inherited_prefix.push(golem_rust::agentic::CanonicalInputValue {
                    name: #name.to_string(),
                    aliases: ::std::vec![#(#aliases.to_string()),*],
                    schema: <#ty as golem_rust::agentic::Schema>::get_type()
                        .get_schema_graph()
                        .expect("tool parameter must have a concrete schema graph"),
                    value: <#ty as golem_rust::agentic::Schema>::to_schema_value(#ident)
                        .expect("failed to encode tool parameter"),
                });
            })
        })
        .collect()
}

fn inherited_root_params(ir: &ToolDefinitionIr, cmd: &CommandIr, tool_name: &str) -> Vec<ParamIr> {
    let is_root = to_kebab_case(&cmd.method_ident.to_string()) == tool_name;
    if is_root {
        return Vec::new();
    }
    let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    else {
        return Vec::new();
    };
    root.params
        .iter()
        .filter(|param| is_global_param(root, param))
        .filter(|param| {
            !cmd.params.iter().any(|own| {
                param_surfaces_intersect(
                    &to_kebab_case(&param.ident.to_string()),
                    &param_aliases(root, param),
                    &to_kebab_case(&own.ident.to_string()),
                    &param_aliases(cmd, own),
                )
            })
        })
        .cloned()
        .collect()
}

fn canonical_value_name(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> String {
    let own_name = to_kebab_case(&param.ident.to_string());
    if to_kebab_case(&cmd.method_ident.to_string()) == tool_name {
        return own_name;
    }
    if let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    {
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
    }
    own_name
}

fn canonical_param_aliases(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> Vec<String> {
    let own_name = to_kebab_case(&param.ident.to_string());
    if let Some(root) = ir
        .commands
        .iter()
        .find(|candidate| to_kebab_case(&candidate.method_ident.to_string()) == tool_name)
    {
        for root_param in &root.params {
            if !is_global_param(root, root_param) {
                continue;
            }
            let root_name = to_kebab_case(&root_param.ident.to_string());
            let aliases = param_aliases(root, root_param);
            if param_surfaces_intersect(&root_name, &aliases, &own_name, &param_aliases(cmd, param))
            {
                return aliases;
            }
        }
    }
    param_aliases(cmd, param)
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

fn param_aliases(cmd: &CommandIr, param: &ParamIr) -> Vec<String> {
    cmd.args
        .iter()
        .find(|arg| arg.param == param.ident)
        .map(|arg| arg.aliases.clone())
        .unwrap_or_default()
}

fn is_global_param(cmd: &CommandIr, param: &ParamIr) -> bool {
    cmd.args
        .iter()
        .find(|arg| arg.param == param.ident)
        .and_then(|arg| arg.placement)
        == Some(ArgPlacement::Global)
}

fn is_flag_param(cmd: &CommandIr, param: &ParamIr) -> bool {
    let arg = cmd.args.iter().find(|arg| arg.param == param.ident);
    arg.and_then(|arg| arg.sub_kind)
        .is_some_and(|kind| matches!(kind, ArgSubKind::Flag | ArgSubKind::CountFlag))
        || type_last_ident(&param.ty).as_deref() == Some("bool")
}

fn stream_idents(cmd: &CommandIr) -> (Option<Ident>, bool) {
    let mut stdin = None;
    let mut stdout = false;
    for param in &cmd.params {
        match type_last_ident(&param.ty).as_deref() {
            Some("InputStream") => stdin = Some(param.ident.clone()),
            Some("OutputStream") => stdout = true,
            _ => {}
        }
    }
    (stdin, stdout)
}

fn client_result_type(output: &ReturnType, has_stdout: bool) -> TokenStream {
    let (ok, err) = split_result(output);
    let err_ty = err
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { ::std::convert::Infallible });
    let ok_ty = match (ok, has_stdout) {
        (Some(ok), true) => quote! { (#ok, golem_rust::wasip2::io::streams::OutputStream) },
        (None, true) => quote! { golem_rust::wasip2::io::streams::OutputStream },
        (Some(ok), false) => quote! { #ok },
        (None, false) => quote! { () },
    };
    quote! { ::std::result::Result<#ok_ty, golem_rust::agentic::ToolError<#err_ty>> }
}

fn invoke_call(output: &ReturnType, stdin_expr: TokenStream) -> TokenStream {
    let (_, err) = split_result(output);
    match err {
        Some(err) => quote! {
            {
                fn __golem_assert_tool_error_decodable<E: golem_rust::agentic::Schema>() {}
                __golem_assert_tool_error_decodable::<#err>();
                golem_rust::agentic::invoke_and_await(
                    &self.rpc,
                    &__command_path,
                    &__input,
                    #stdin_expr,
                    <#err as golem_rust::agentic::ToolErrorSchema>::from_error_payload_value,
                )
            }
        },
        None => quote! {
            golem_rust::agentic::invoke_and_await_infallible(
                &self.rpc,
                &__command_path,
                &__input,
                #stdin_expr,
            )
        },
    }
}

fn decode_client_result(output: &ReturnType, has_stdout: bool) -> TokenStream {
    let (ok, _) = split_result(output);
    match (ok, has_stdout) {
        (Some(ok), true) => quote! {
            golem_rust::agentic::decode_result_with_stdout::<#ok, _>(__result)
        },
        (None, true) => quote! {
            golem_rust::agentic::decode_result_stdout_only(__result)
        },
        (Some(ok), false) => quote! {
            golem_rust::agentic::decode_result_value::<#ok, _>(__result)
        },
        (None, false) => quote! {
            golem_rust::agentic::decode_result_empty(__result)
        },
    }
}

fn command_name(cmd: &CommandIr, tool_name: &str) -> String {
    if to_kebab_case(&cmd.method_ident.to_string()) == tool_name {
        tool_name.to_string()
    } else {
        cmd.name_override
            .clone()
            .unwrap_or_else(|| to_kebab_case(&cmd.method_ident.to_string()))
    }
}

fn subtree_wrapper_ident(ir: &ToolDefinitionIr, cmd: &CommandIr) -> Ident {
    format_ident!(
        "{}{}Client",
        ir.trait_ident,
        pascal_case(&cmd.method_ident.to_string())
    )
}

fn subtree_client_macro_path(path: &Path) -> Path {
    let mut rewritten = path.clone();
    if let Some(last) = rewritten.segments.last_mut() {
        last.ident = subtree_client_macro_ident(&last.ident);
    }
    rewritten
}

fn subtree_client_macro_ident(trait_ident: &Ident) -> Ident {
    format_ident!("__golem_tool_subtree_client_methods_for_{}", trait_ident)
}

fn pascal_case(input: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;
    for ch in input.chars() {
        if ch == '_' || ch == '-' {
            capitalize = true;
        } else if capitalize {
            out.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn is_stream_type(ty: &Type) -> bool {
    matches!(
        type_last_ident(ty).as_deref(),
        Some("InputStream" | "OutputStream")
    )
}

fn is_principal_type(ty: &Type) -> bool {
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

fn constructor_ident(ir: &ToolDefinitionIr) -> Ident {
    let method_names = ir
        .commands
        .iter()
        .map(|cmd| cmd.method_ident.to_string())
        .collect::<::std::collections::BTreeSet<_>>();
    if !method_names.contains("new") {
        return format_ident!("new");
    }
    let mut candidate = "__golem_tool_client_new".to_string();
    while method_names.contains(&candidate) {
        candidate.push('_');
    }
    format_ident!("{}", candidate)
}

fn type_last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
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
