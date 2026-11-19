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

pub(crate) use crate::tool::helpers::is_stream_type;
use crate::tool::helpers::{StreamKind, stream_type, to_kebab_case};
use crate::tool::ir::{ArgSubKind, CommandIr, ParamIr, ToolDefinitionIr};
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
    let input_schemas = ir
        .commands
        .iter()
        .filter(|&_cmd| {
            !crate::tool::descriptor::client_needs_descriptor_input(ir)
                && crate::tool::descriptor::client_has_refined_input(ir)
        })
        .map(|cmd| synthesize_input_schema(ir, cmd, &tool_name));
    let (descriptor_field, descriptor_init) =
        if crate::tool::descriptor::client_needs_descriptor_input(ir) {
            let descriptor_fn =
                crate::tool::descriptor::standalone_wire_descriptor_fn_ident(&ir.trait_ident);
            (
                quote! {
                    root_descriptor: ::std::option::Option<golem_rust::agentic::WireToolDescriptorFn>,
                },
                quote! {
                    root_descriptor: ::std::option::Option::Some(
                        #descriptor_fn as golem_rust::agentic::WireToolDescriptorFn
                    ),
                },
            )
        } else {
            (quote! {}, quote! {})
        };

    quote! {
        #(#input_schemas)*
        #(#subtree_wrappers)*

        pub struct #client_ident {
            rpc: golem_rust::agentic::ambient_tool_rpc::AmbientToolRpc,
            root_tool_name: ::std::string::String,
            #descriptor_field
            command_path: ::std::vec::Vec<::std::string::String>,
            schema_path: ::std::vec::Vec<::std::string::String>,
            inherited_prefix: ::std::vec::Vec<golem_rust::agentic::DirectInputValue>,
        }

        impl #client_ident {
            pub fn #constructor_ident() -> Self {
                Self::new_with_lookup_name(#tool_name)
            }

            /// Constructs this presented-definition client while routing calls to an explicit leaf.
            pub fn new_with_lookup_name(lookup_name: impl Into<::std::string::String>) -> Self {
                let lookup_name = lookup_name.into();
                Self {
                    rpc: golem_rust::agentic::ambient_tool_rpc::AmbientToolRpc::new(&lookup_name),
                    root_tool_name: lookup_name,
                    #descriptor_init
                    command_path: ::std::vec::Vec::new(),
                    schema_path: ::std::vec::Vec::new(),
                    inherited_prefix: ::std::vec::Vec::new(),
                }
            }

            #(#methods)*
        }

        impl ::std::default::Default for #client_ident {
            fn default() -> Self {
                Self::#constructor_ident()
            }
        }

        #subtree_macro
    }
}

fn canonical_input_params<'a>(
    ir: &'a ToolDefinitionIr,
    cmd: &'a CommandIr,
    tool_name: &str,
) -> Result<Vec<(&'a CommandIr, &'a ParamIr)>, syn::Error> {
    let mut ordered = crate::tool::descriptor::root_global_params(ir, tool_name);
    ordered.sort_by_key(|(source, param)| is_flag_param(source, param));
    let mut fields = Vec::new();
    for param in &cmd.params {
        if is_injected_context_type(&param.ty) || is_stream_type(&param.ty) {
            continue;
        }
        let order = crate::tool::descriptor::client_surface_order(ir, cmd, param)?;
        fields.push((order, canonical_param_source(ir, cmd, param, tool_name)));
    }
    fields.sort_by_key(|(order, _)| *order);
    for (_, (source, param)) in fields {
        if !ordered
            .iter()
            .any(|(_, previous)| previous.ident == param.ident)
        {
            ordered.push((source, param));
        }
    }
    Ok(ordered)
}

fn input_schema_fn_ident(ir: &ToolDefinitionIr, cmd: &CommandIr) -> Ident {
    format_ident!(
        "__golem_wire_input_schema_for_{}_{}",
        ir.trait_ident,
        cmd.method_ident
    )
}

fn synthesize_input_schema(ir: &ToolDefinitionIr, cmd: &CommandIr, tool_name: &str) -> TokenStream {
    let params = match canonical_input_params(ir, cmd, tool_name) {
        Ok(params) => params,
        Err(error) => return error.to_compile_error(),
    };
    let mut fields = Vec::new();
    let mut aliases = Vec::new();
    for (source, param) in params {
        let name = to_kebab_case(&param.ident.to_string());
        let schema = match crate::tool::descriptor::client_field_schema(ir, source, param) {
            Ok(schema) => schema,
            Err(error) => return error.to_compile_error(),
        };
        let optional = direct_input_needs_option_carrier(ir, source, param, tool_name);
        let field_aliases = param_aliases(source, param);
        aliases.push(quote! { ::std::vec![#(#field_aliases.to_string()),*] });
        fields.push(quote! {{
            let __type = #schema;
            let __body = __golem_wire_schema.with_builder(|__builder| {
                if #optional && !matches!(
                    __builder.node(__type.index).map(|__node| &__node.body),
                    Some(golem_rust::schema::wit::wire::SchemaTypeBody::OptionType(_))
                ) {
                    __builder.push(golem_rust::schema::wit::wire::SchemaTypeBody::OptionType(__type.index))
                } else {
                    __type.index
                }
            });
            golem_rust::schema::wit::wire::NamedFieldType {
                name: #name.to_string(),
                body: __body,
                metadata: golem_rust::schema::wit::direct::empty_metadata(),
            }
        }});
    }
    let ident = input_schema_fn_ident(ir, cmd);
    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #ident() -> ::std::result::Result<golem_rust::agentic::WireCanonicalInputModel, golem_rust::agentic::ToolBuildError> {
            let __golem_wire_schema = golem_rust::agentic::WireToolSchema::default();
            let __fields = ::std::vec![#(#fields),*];
            let __root = __golem_wire_schema.with_builder(|__builder| {
                __builder.push(golem_rust::schema::wit::wire::SchemaTypeBody::RecordType(__fields))
            });
            ::std::result::Result::Ok(golem_rust::agentic::WireCanonicalInputModel {
                graph: __golem_wire_schema.finish_at(__root),
                aliases: ::std::vec![#(#aliases),*],
            })
        }
    }
}

/// Encodes the compiled canonical field order from concrete captured values.
fn input_build_expr(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    param_values: TokenStream,
    use_descriptor: bool,
) -> TokenStream {
    let ordered = match canonical_input_params(ir, cmd, tool_name) {
        Ok(params) => params
            .into_iter()
            .map(|(_, param)| to_kebab_case(&param.ident.to_string()))
            .collect::<Vec<_>>(),
        Err(error) => return error.to_compile_error(),
    };
    let encode = if use_descriptor {
        quote! {
            golem_rust::agentic::encode_direct_tool_input_with_descriptor(
                &__values,
                &[#(#ordered),*],
                self.root_descriptor,
                &__command_path,
            ).await
        }
    } else if crate::tool::descriptor::client_has_refined_input(ir) {
        let schema = input_schema_fn_ident(ir, cmd);
        quote! {
            golem_rust::agentic::encode_direct_tool_input_with_schema(&__values, #schema).await
        }
    } else {
        quote! {
            golem_rust::agentic::encode_direct_tool_input(&__values, &[#(#ordered),*]).await
        }
    };
    quote! {
        {
            let mut __values = self.inherited_prefix.clone();
            __values.extend(#param_values);
            #encode
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
    let (stdin_ident, has_stdout, has_stderr) = stream_idents(cmd);
    let has_output = has_stdout || has_stderr;
    let stdin_expr = match stdin_ident {
        Some((ident, true)) => quote! { ::std::option::Option::Some(#ident) },
        Some((ident, false)) => quote! { #ident },
        None => quote! { ::std::option::Option::None },
    };
    let value_inserts = value_inserts(ir, cmd, &inherited_params, tool_name, omitted_names);
    let result_ty = client_result_type(&cmd.output, has_output);
    let decode_result = decode_client_result(&cmd.output);
    let invoke = invoke_call(&cmd.output, stdin_expr.clone());
    let input_expr = input_build_expr(
        ir,
        cmd,
        tool_name,
        quote! { __golem_param_values },
        crate::tool::descriptor::client_needs_descriptor_input(ir),
    );

    if has_output {
        let started_ty = started_result_type(&cmd.output);
        let start = start_call(&cmd.output, stdin_expr, has_stdout, has_stderr);
        return quote! {
            pub async fn #method_ident(&self, #(#input_args),*) -> #started_ty {
                #(#value_inserts)*

                let mut __command_path = self.command_path.clone();
                let mut __schema_path = self.schema_path.clone();
                #command_path_part
                let __input = #input_expr;
                #start.await
            }
        };
    }

    quote! {
        pub async fn #method_ident(&self, #(#input_args),*) -> #result_ty {
            #(#value_inserts)*

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
                rpc: golem_rust::agentic::ambient_tool_rpc::AmbientToolRpc::new(&self.root_tool_name),
                root_tool_name: self.root_tool_name.clone(),
                root_descriptor: self.root_descriptor,
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
    let command_name = command_name(cmd, tool_name);
    let command_path_part = if command_name == tool_name {
        quote! {}
    } else {
        quote! {
            __command_path.push(#command_name.to_string());
            __schema_path.push(#command_name.to_string());
        }
    };
    let (stdin_ident, has_stdout, has_stderr) = stream_idents(cmd);
    let has_output = has_stdout || has_stderr;
    let stdin_arg = stdin_ident.as_ref().map(|(ident, _)| {
        let ty = &cmd
            .params
            .iter()
            .find(|param| param.ident == *ident)
            .expect("stream identifier belongs to a command parameter")
            .ty;
        quote! { , #ident: #ty }
    });
    let stdin_expr = match stdin_ident {
        Some((ident, true)) => quote! { ::std::option::Option::Some(#ident) },
        Some((ident, false)) => quote! { #ident },
        None => quote! { ::std::option::Option::None },
    };
    let result_ty = client_result_type(&cmd.output, has_output);
    let decode_result = decode_client_result(&cmd.output);
    let invoke = invoke_call(&cmd.output, stdin_expr.clone());
    let input_expr = input_build_expr(ir, cmd, tool_name, param_values.clone(), true);

    if has_output {
        let result_ty = started_result_type(&cmd.output);
        let start = start_call(&cmd.output, stdin_expr, has_stdout, has_stderr);
        return quote! {
            pub async fn #method_ident(&self #input_args #stdin_arg) -> #result_ty {
                let mut #param_values: ::std::vec::Vec<golem_rust::agentic::DirectInputValue> =
                    ::std::vec![#value_inserts];

                let mut __command_path = self.command_path.clone();
                let mut __schema_path = self.schema_path.clone();
                #command_path_part
                let __input = #input_expr;
                #start.await
            }
        };
    }

    quote! {
        pub async fn #method_ident(&self #input_args #stdin_arg) -> #result_ty {
            let mut #param_values: ::std::vec::Vec<golem_rust::agentic::DirectInputValue> =
                ::std::vec![#value_inserts];

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
                rpc: golem_rust::agentic::ambient_tool_rpc::AmbientToolRpc::new(&self.root_tool_name),
                root_tool_name: self.root_tool_name.clone(),
                root_descriptor: self.root_descriptor,
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
            rpc: golem_rust::agentic::ambient_tool_rpc::AmbientToolRpc,
            root_tool_name: ::std::string::String,
            root_descriptor: ::std::option::Option<golem_rust::agentic::WireToolDescriptorFn>,
            command_path: ::std::vec::Vec<::std::string::String>,
            schema_path: ::std::vec::Vec<::std::string::String>,
            inherited_prefix: ::std::vec::Vec<golem_rust::agentic::DirectInputValue>,
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
        #[allow(unused_imports)]
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
        .filter(|param| !is_injected_context_type(&param.ty) && !is_stream_type(&param.ty))
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
        .filter(|param| !is_injected_context_type(&param.ty))
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
        let short = option_char_tokens(canonical_param_short(ir, cmd, param, tool_name));
        let option_carrier = direct_input_needs_option_carrier(ir, cmd, param, tool_name);
        quote! {
            $inherited_prefix.push(golem_rust::agentic::DirectInputValue::new(
                #name, ::std::vec![#(#aliases.to_string()),*], #short, #ident
            ).with_option_carrier(#option_carrier));
        }
    } else {
        let name = canonical_value_name(ir, cmd, param, tool_name);
        let aliases = canonical_param_aliases(ir, cmd, param, tool_name);
        let aliases = aliases.iter();
        let short = option_char_tokens(canonical_param_short(ir, cmd, param, tool_name));
        let option_carrier = direct_input_needs_option_carrier(ir, cmd, param, tool_name);
        quote! {
            golem_rust::agentic::DirectInputValue::new(
                #name, ::std::vec![#(#aliases.to_string()),*], #short, #ident
            ).with_option_carrier(#option_carrier),
        }
    };

    if is_subtree_command {
        let (new_markers, new_tag, new_ty) = if !is_stream_type(&param.ty) {
            let surfaces = captured_param_surfaces(ir, cmd, param, tool_name);
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
                && !is_injected_context_type(&param.ty)
                && (include_stdout
                    || !matches!(stream_type(&param.ty), Some((StreamKind::Output, _))))
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
            if is_injected_context_type(&param.ty) || is_stream_type(&param.ty) {
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
            let aliases = canonical_param_aliases(ir, cmd, param, tool_name);
            let aliases = aliases.iter();
            let short = option_char_tokens(canonical_param_short(ir, cmd, param, tool_name));
            let option_carrier = direct_input_needs_option_carrier(ir, cmd, param, tool_name);
            Some(quote! {
                golem_rust::agentic::DirectInputValue::new(
                    #name, ::std::vec![#(#aliases.to_string()),*], #short, #ident
                ).with_option_carrier(#option_carrier)
            })
        });
    let inserts: Vec<_> = inserts.collect();
    vec![quote! {
            let mut __golem_param_values: ::std::vec::Vec<golem_rust::agentic::DirectInputValue> =
                ::std::vec![#(#inserts),*];
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
        .filter(|param| !is_injected_context_type(&param.ty) && !is_stream_type(&param.ty))
    {
        for surface in captured_param_surfaces(ir, cmd, param, tool_name) {
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
    inherited_root_params(ir, cmd, tool_name)
        .iter()
        .flat_map(|param| captured_param_surfaces(ir, cmd, param, tool_name))
        .collect()
}

fn captured_param_surfaces(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> Vec<String> {
    // Reconciliation removes a redeclaration's local surfaces; only the
    // effective source's name and aliases remain inherited by descendants.
    let (source, source_param) = canonical_param_source(ir, cmd, param, tool_name);
    param_surfaces(source, source_param)
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

pub(crate) fn omitted_marker_ident(surface: &str) -> Ident {
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
    param_omission_surfaces(ir, cmd, param, tool_name)
        .iter()
        .any(|surface| surface == omitted)
}

pub(crate) fn param_omission_surfaces(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> Vec<String> {
    let mut surfaces = param_surfaces(cmd, param);
    // A graft can de-project the standalone root global through an alias.
    // Its other surfaces do not necessarily remain inherited by this parameter.
    let canonical_name = canonical_value_name(ir, cmd, param, tool_name);
    if !surfaces.iter().any(|surface| surface == &canonical_name) {
        surfaces.push(canonical_name);
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
    let mut inherited: Vec<(&ParamIr, bool)> =
        inherited_params.iter().map(|param| (param, true)).collect();
    inherited.sort_by_key(|(param, _)| if is_flag_param(cmd, param) { 1 } else { 0 });
    let mut current: Vec<(&ParamIr, bool)> =
        cmd.params.iter().map(|param| (param, false)).collect();
    current.sort_by_key(|(param, _)| if is_flag_param(cmd, param) { 1 } else { 0 });
    inherited
        .into_iter()
        .chain(current)
        .filter_map(|(param, _from_root)| {
            if is_injected_context_type(&param.ty) || is_stream_type(&param.ty) {
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
            let aliases = canonical_param_aliases(ir, cmd, param, tool_name);
            let aliases = aliases.iter();
            let short = option_char_tokens(canonical_param_short(ir, cmd, param, tool_name));
            let option_carrier = direct_input_needs_option_carrier(ir, cmd, param, tool_name);
            Some(quote! {
                __inherited_prefix.push(golem_rust::agentic::DirectInputValue::new(
                    #name, ::std::vec![#(#aliases.to_string()),*], #short, #ident
                ).with_option_carrier(#option_carrier));
            })
        })
        .collect()
}

pub(crate) fn inherited_root_params(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
) -> Vec<ParamIr> {
    let is_root = to_kebab_case(&cmd.method_ident.to_string()) == tool_name;
    if is_root {
        return Vec::new();
    }
    crate::tool::descriptor::root_global_params(ir, tool_name)
        .into_iter()
        .filter(|(root, param)| {
            !cmd.params.iter().any(|own| {
                param_surfaces_intersect(
                    &to_kebab_case(&param.ident.to_string()),
                    &param_aliases(root, param),
                    &to_kebab_case(&own.ident.to_string()),
                    &param_aliases(cmd, own),
                )
            })
        })
        .map(|(_, param)| param.clone())
        .collect()
}

pub(crate) fn canonical_value_name(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> String {
    let (_, source) = canonical_param_source(ir, cmd, param, tool_name);
    to_kebab_case(&source.ident.to_string())
}

fn canonical_param_source<'a>(
    ir: &'a ToolDefinitionIr,
    cmd: &'a CommandIr,
    param: &'a ParamIr,
    tool_name: &str,
) -> (&'a CommandIr, &'a ParamIr) {
    let own_name = to_kebab_case(&param.ident.to_string());
    for (root, root_param) in crate::tool::descriptor::root_global_params(ir, tool_name) {
        if param_surfaces_intersect(
            &to_kebab_case(&root_param.ident.to_string()),
            &param_aliases(root, root_param),
            &own_name,
            &param_aliases(cmd, param),
        ) {
            return (root, root_param);
        }
    }
    (cmd, param)
}

fn direct_input_needs_option_carrier(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> bool {
    let (source_cmd, source_param) = canonical_param_source(ir, cmd, param, tool_name);
    crate::tool::descriptor::canonical_field_has_option_carrier(ir, source_cmd, source_param)
        .unwrap_or(false)
}

fn canonical_param_aliases(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> Vec<String> {
    let (source, param) = canonical_param_source(ir, cmd, param, tool_name);
    param_aliases(source, param)
}

fn canonical_param_short(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
) -> Option<char> {
    let (source, param) = canonical_param_source(ir, cmd, param, tool_name);
    param_short(source, param)
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

fn param_short(cmd: &CommandIr, param: &ParamIr) -> Option<char> {
    cmd.args
        .iter()
        .find(|arg| arg.param == param.ident)
        .and_then(|arg| arg.short)
}

fn option_char_tokens(value: Option<char>) -> TokenStream {
    match value {
        Some(value) => quote! { ::std::option::Option::Some(#value) },
        None => quote! { ::std::option::Option::None },
    }
}

fn is_flag_param(cmd: &CommandIr, param: &ParamIr) -> bool {
    let arg = cmd.args.iter().find(|arg| arg.param == param.ident);
    arg.and_then(|arg| arg.sub_kind)
        .is_some_and(|kind| matches!(kind, ArgSubKind::Flag | ArgSubKind::CountFlag))
        || type_last_ident(&param.ty).as_deref() == Some("bool")
}

pub(crate) fn stream_idents(cmd: &CommandIr) -> (Option<(Ident, bool)>, bool, bool) {
    let mut stdin = None;
    let mut stdout = false;
    let mut stderr = false;
    for param in &cmd.params {
        match stream_type(&param.ty) {
            Some((StreamKind::Input, required)) => stdin = Some((param.ident.clone(), required)),
            Some((StreamKind::Output, _)) => {
                let channel = cmd
                    .args
                    .iter()
                    .find(|arg| arg.param == param.ident)
                    .and_then(|arg| arg.output_channel)
                    .unwrap_or(crate::tool::ir::OutputChannelIr::Stdout);
                match channel {
                    crate::tool::ir::OutputChannelIr::Stdout => stdout = true,
                    crate::tool::ir::OutputChannelIr::Stderr => stderr = true,
                }
            }
            _ => {}
        }
    }
    (stdin, stdout, stderr)
}

fn client_result_type(output: &ReturnType, has_stdout: bool) -> TokenStream {
    let (ok, err) = split_result(output);
    let err_ty = err
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { ::std::convert::Infallible });
    let ok_ty = match (ok, has_stdout) {
        (Some(ok), true) => quote! { (#ok, golem_rust::agentic::InputStream) },
        (None, true) => quote! { golem_rust::agentic::InputStream },
        (Some(ok), false) => quote! { #ok },
        (None, false) => quote! { () },
    };
    quote! { ::std::result::Result<#ok_ty, golem_rust::agentic::ToolError<#err_ty>> }
}

fn started_result_type(output: &ReturnType) -> TokenStream {
    let (ok, err) = split_result(output);
    let ok = ok.map(|ty| quote! { #ty }).unwrap_or_else(|| quote! { () });
    let err = err
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { ::std::convert::Infallible });
    quote! {
        ::std::result::Result<
            golem_rust::agentic::ToolInvocation<#ok, #err>,
            golem_rust::agentic::ToolError<#err>
        >
    }
}

fn start_call(
    output: &ReturnType,
    stdin_expr: TokenStream,
    has_stdout: bool,
    has_stderr: bool,
) -> TokenStream {
    let (ok, err) = split_result(output);
    let decode = match ok {
        Some(ok) => {
            quote! { |__result| golem_rust::agentic::decode_direct_result_value::<#ok, _>(__result) }
        }
        None => quote! { |__result| golem_rust::agentic::decode_direct_result_empty(__result) },
    };
    match err {
        Some(err) => quote! {
            {
                fn __golem_assert_tool_error_decodable<E: golem_rust::agentic::DirectToolError>() {}
                __golem_assert_tool_error_decodable::<#err>();
                golem_rust::agentic::start_tool_invocation_direct_input(
                    &self.rpc,
                    &__command_path,
                    __input,
                    #stdin_expr,
                    #has_stdout,
                    #has_stderr,
                    #decode,
                    <#err as golem_rust::agentic::DirectToolError>::recognizes_error_name,
                    <#err as golem_rust::agentic::DirectToolError>::from_direct_error_reader,
                )
            }
        },
        None => quote! {
            golem_rust::agentic::start_tool_invocation_direct_input(
                &self.rpc,
                &__command_path,
                __input,
                #stdin_expr,
                #has_stdout,
                #has_stderr,
                #decode,
                |_| false,
                |_, _, _| ::std::result::Result::Ok(::std::option::Option::None),
            )
        },
    }
}

fn invoke_call(output: &ReturnType, stdin_expr: TokenStream) -> TokenStream {
    let (_, err) = split_result(output);
    match err {
        Some(err) => quote! {
            {
                fn __golem_assert_tool_error_decodable<E: golem_rust::agentic::DirectToolError>() {}
                __golem_assert_tool_error_decodable::<#err>();
                golem_rust::agentic::invoke_and_await_direct::<#err, _>(
                    &self.rpc,
                    &__command_path,
                    __input,
                    (#stdin_expr).map(golem_rust::agentic::pump_tool_stdin),
                    ::std::option::Option::None,
                    ::std::option::Option::None,
                ).await
            }
        },
        None => quote! {
            golem_rust::agentic::invoke_and_await_direct_infallible(
                &self.rpc,
                &__command_path,
                __input,
                (#stdin_expr).map(golem_rust::agentic::pump_tool_stdin),
                ::std::option::Option::None,
                ::std::option::Option::None,
            ).await
        },
    }
}

/// Only reached for commands without stdout; a stdout-bearing command returns a
/// started invocation before this is used.
fn decode_client_result(output: &ReturnType) -> TokenStream {
    let (ok, _) = split_result(output);
    match ok {
        Some(ok) => quote! {
            golem_rust::agentic::decode_direct_result_value::<#ok, _>(__result)
        },
        None => quote! {
            golem_rust::agentic::decode_direct_result_empty(__result)
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

pub(crate) fn is_injected_context_type(ty: &Type) -> bool {
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
        ["golem_rust", "agentic" | "tool", "InvocationContext"]
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

pub(crate) fn split_result(output: &ReturnType) -> (Option<&Type>, Option<&Type>) {
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
