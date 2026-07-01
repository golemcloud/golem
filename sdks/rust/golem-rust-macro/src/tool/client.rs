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

    quote! {
        pub async fn #method_ident(&self, #(#input_args),*) -> #result_ty {
            #(#value_inserts)*

            let __can_use_static_input_model = self.inherited_prefix.is_empty() && self.schema_path.is_empty();
            let mut __command_path = self.command_path.clone();
            let mut __schema_path = self.schema_path.clone();
            #command_path_part
            let __input = if __can_use_static_input_model {
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
                let __record_fields = if __model.fields.len() == __golem_param_values.len()
                    && __model.fields.iter()
                        .zip(__golem_param_values.iter())
                        .all(|(__field, (__name, _))| __field.name.as_str() == *__name)
                {
                    __golem_param_values.into_iter().map(|(_, __value)| __value).collect()
                } else {
                    let mut __record_fields: ::std::vec::Vec<golem_rust::SchemaValue> =
                        ::std::vec::Vec::with_capacity(__model.fields.len());
                    for __field in __model.fields.iter() {
                        let __value_index = __golem_param_values.iter()
                            .rposition(|(__name, _)| *__name == __field.name.as_str())
                            .ok_or_else(|| {
                                golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                                    format!("missing canonical tool input field `{}`", __field.name)
                                ))
                            })?;
                        let (_, __value) = __golem_param_values.remove(__value_index);
                        __record_fields.push(__value);
                    }
                    __record_fields
                };
                golem_rust::TypedSchemaValue::new(
                    __model.record_schema.clone(),
                    golem_rust::SchemaValue::Record { fields: __record_fields },
                )
            } else {
                let __tool = #descriptor_fn_ident(&mut golem_rust::agentic::ToolBuildCtx::new())
                    .expect("tool descriptor build failed");
                let __command_index = __tool.command_index_by_path(&__schema_path).ok_or_else(|| {
                    golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                        format!("invalid generated tool command path `{}`", __schema_path.join(" "))
                    ))
                })?;
                let mut __canonical_fields: ::std::vec::Vec<golem_rust::agentic::CanonicalInputField> =
                    self.inherited_prefix.iter().map(|__value| golem_rust::agentic::CanonicalInputField {
                        name: __value.name.clone(),
                        aliases: __value.aliases.clone(),
                        schema: __value.schema.clone(),
                    }).collect();
                let __inherited_names: ::std::collections::BTreeSet<&str> = self.inherited_prefix.iter()
                    .flat_map(|__value| ::std::iter::once(__value.name.as_str()).chain(__value.aliases.iter().map(::std::string::String::as_str)))
                    .collect();
                __canonical_fields.extend(
                    __tool.canonical_input_fields(__command_index)
                        .into_iter()
                        .filter(|__field| {
                            !__inherited_names.contains(__field.name.as_str())
                                && !__field.aliases.iter().any(|__alias| __inherited_names.contains(__alias.as_str()))
                        })
                );
                let __model = golem_rust::agentic::CanonicalInputModel::from_fields(__canonical_fields)
                    .map_err(|__err| golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(__err.to_string())))?;
                let mut __record_fields: ::std::vec::Vec<golem_rust::SchemaValue> =
                    self.inherited_prefix.iter().map(|__value| __value.value.clone()).collect();
                for __field in __model.fields.iter().skip(self.inherited_prefix.len()) {
                    let __value_index = __golem_param_values.iter()
                        .rposition(|(__name, _)| *__name == __field.name.as_str())
                        .ok_or_else(|| {
                        golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                            format!("missing canonical tool input field `{}`", __field.name)
                        ))
                    })?;
                    let (_, __value) = __golem_param_values.remove(__value_index);
                    __record_fields.push(__value);
                }
                golem_rust::TypedSchemaValue::new(
                    __model.record_schema,
                    golem_rust::SchemaValue::Record { fields: __record_fields },
                )
            };
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

fn synthesize_subtree_method_with_context(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    _subtree: &crate::tool::ir::SubtreeIr,
    tool_name: &str,
    omitted_names: &[String],
    omitted_tag: TokenStream,
    omitted_ty: TokenStream,
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
    let new_omitted = subtree_new_omitted_surfaces(ir, cmd, tool_name, omitted_names);
    let child_omitted_tag = contextual_omitted_tag(omitted_tag, cmd, &new_omitted);
    let child_omitted_ty = omitted_type(quote! { (#omitted_ty, fn() -> Self) }, &new_omitted);
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
    let local_omitted = local_omitted_surfaces(ir);
    let scan_arms = subtree_client_macro_scan_arms(ir, &macro_ident);
    let emit_arms = subtree_client_macro_emit_arms(ir, tool_name);
    let scan_start = if local_omitted.is_empty() {
        quote! {
            #macro_ident!(@emit $client_ident, $omitted_tag, $omitted_ty, [], [$($omitted)*]);
        }
    } else {
        let state = subtree_client_macro_scan_state_ident(0);
        quote! {
            #macro_ident!(#state $client_ident, $omitted_tag, $omitted_ty, [], [$($omitted)*] ; $($omitted)*);
        }
    };

    quote! {
        #[doc(hidden)]
        macro_rules! #macro_ident {
            ($client_ident:ident) => {
                #macro_ident!($client_ident, 0, (), []);
            };
            ($client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, [$($omitted:ident)*]) => {
                #scan_start
            };
            #(#scan_arms)*
            #(#emit_arms)*
        }

        #[doc(hidden)]
        pub(crate) use #macro_ident;
    }
}

fn subtree_client_macro_scan_arms(ir: &ToolDefinitionIr, macro_ident: &Ident) -> Vec<TokenStream> {
    let surfaces = local_omitted_surfaces(ir);
    let len = surfaces.len();
    surfaces
        .into_iter()
        .enumerate()
        .flat_map(|(idx, surface)| {
            let state = subtree_client_macro_scan_state_ident(idx);
            let marker = omitted_marker_ident(&surface);
            let next_with_marker = subtree_client_macro_next_scan(
                macro_ident,
                idx + 1,
                len,
                quote! { [$($found)* #marker] },
            );
            let next_without_marker = subtree_client_macro_next_scan(
                macro_ident,
                idx + 1,
                len,
                quote! { [$($found)*] },
            );
            vec![
                quote! {
                    (#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, [$($found:ident)*], [$($all:ident)*] ; #marker $($rest:ident)*) => {
                        #next_with_marker
                    };
                },
                quote! {
                    (#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, [$($found:ident)*], [$($all:ident)*] ; $unknown:ident $($rest:ident)*) => {
                        #macro_ident!(#state $client_ident, $omitted_tag, $omitted_ty, [$($found)*], [$($all)*] ; $($rest)*);
                    };
                },
                quote! {
                    (#state $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, [$($found:ident)*], [$($all:ident)*] ; ) => {
                        #next_without_marker
                    };
                },
            ]
        })
        .collect()
}

fn subtree_client_macro_next_scan(
    macro_ident: &Ident,
    next_idx: usize,
    len: usize,
    found: TokenStream,
) -> TokenStream {
    if next_idx == len {
        quote! {
            #macro_ident!(@emit $client_ident, $omitted_tag, $omitted_ty, #found, [$($all)*]);
        }
    } else {
        let next_state = subtree_client_macro_scan_state_ident(next_idx);
        quote! {
            #macro_ident!(#next_state $client_ident, $omitted_tag, $omitted_ty, #found, [$($all)*] ; $($all)*);
        }
    }
}

fn subtree_client_macro_scan_state_ident(idx: usize) -> Ident {
    format_ident!("__golem_scan_{}", idx)
}

fn subtree_client_macro_emit_arms(ir: &ToolDefinitionIr, tool_name: &str) -> Vec<TokenStream> {
    possible_omitted_surface_sequences(ir)
        .into_iter()
        .map(|omitted| {
            let omitted_marker_tokens = omitted_markers(&omitted);
            let methods = ir.commands.iter().map(|cmd| {
                if let Some(subtree) = &cmd.subtree {
                    synthesize_subtree_method_with_context(
                        ir,
                        cmd,
                        subtree,
                        tool_name,
                        &omitted,
                        quote! { $omitted_tag },
                        quote! { $omitted_ty },
                    )
                } else {
                    synthesize_leaf_method(ir, cmd, tool_name, &omitted)
                }
            });
            let nested_contexts = ir.commands.iter().filter_map(|cmd| {
                let subtree = cmd.subtree.as_ref()?;
                let child_macro_path = subtree_client_macro_path(&subtree.path);
                let wrapper_ident = subtree_wrapper_ident(ir, cmd);
                let new_omitted = subtree_new_omitted_surfaces(ir, cmd, tool_name, &omitted);
                let child_omitted_tag = contextual_omitted_tag(quote! { $omitted_tag }, cmd, &new_omitted);
                let child_omitted_ty = omitted_type(
                    quote! { ($omitted_ty, fn() -> $client_ident<{ $omitted_tag }, $omitted_ty>) },
                    &new_omitted,
                );
                let new_markers = omitted_markers(&new_omitted);
                Some(quote! {
                    #child_macro_path!(#wrapper_ident, #child_omitted_tag, #child_omitted_ty, [$($all)* #(#new_markers)*]);
                })
            });
            quote! {
                (@emit $client_ident:ident, $omitted_tag:expr, $omitted_ty:ty, [#(#omitted_marker_tokens)*], [$($all:ident)*]) => {
                    impl $client_ident<{ $omitted_tag }, $omitted_ty> {
                        #(#methods)*
                    }
                    #(#nested_contexts)*
                };
            }
        })
        .collect()
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

fn local_omitted_surfaces(ir: &ToolDefinitionIr) -> Vec<String> {
    let mut surfaces = Vec::new();
    for cmd in &ir.commands {
        for param in cmd
            .params
            .iter()
            .filter(|param| !is_principal_type(&param.ty) && !is_stream_type(&param.ty))
        {
            for surface in param_surfaces(cmd, param) {
                if !surfaces.iter().any(|existing| existing == &surface) {
                    surfaces.push(surface);
                }
            }
        }
    }
    surfaces
}

fn possible_omitted_surface_sequences(ir: &ToolDefinitionIr) -> Vec<Vec<String>> {
    let surfaces = local_omitted_surfaces(ir);
    if surfaces.len() > 16 {
        let mut sequences = vec![Vec::new()];
        let mut width = 1;
        while width <= surfaces.len() {
            let before = sequences.len();
            push_bounded_omitted_surface_combinations(
                &surfaces,
                width,
                0,
                &mut Vec::new(),
                &mut sequences,
                4096,
            );
            if sequences.len() == before || sequences.len() >= 4096 {
                break;
            }
            width += 1;
        }
        return sequences;
    }
    let mut sequences = vec![Vec::new()];
    push_omitted_surface_sequences(&surfaces, 0, &mut Vec::new(), &mut sequences);
    sequences
}

fn push_bounded_omitted_surface_combinations(
    surfaces: &[String],
    width: usize,
    index: usize,
    current: &mut Vec<String>,
    sequences: &mut Vec<Vec<String>>,
    max_sequences: usize,
) {
    if sequences.len() >= max_sequences {
        return;
    }
    if current.len() == width {
        sequences.push(current.clone());
        return;
    }
    if index == surfaces.len() {
        return;
    }
    let remaining = width - current.len();
    if surfaces.len() - index < remaining {
        return;
    }

    current.push(surfaces[index].clone());
    push_bounded_omitted_surface_combinations(
        surfaces,
        width,
        index + 1,
        current,
        sequences,
        max_sequences,
    );
    current.pop();
    push_bounded_omitted_surface_combinations(
        surfaces,
        width,
        index + 1,
        current,
        sequences,
        max_sequences,
    );
}

fn push_omitted_surface_sequences(
    surfaces: &[String],
    index: usize,
    current: &mut Vec<String>,
    sequences: &mut Vec<Vec<String>>,
) {
    if index == surfaces.len() {
        return;
    }

    push_omitted_surface_sequences(surfaces, index + 1, current, sequences);
    current.push(surfaces[index].clone());
    sequences.push(current.clone());
    push_omitted_surface_sequences(surfaces, index + 1, current, sequences);
    current.pop();
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

fn subtree_new_omitted_surfaces(
    ir: &ToolDefinitionIr,
    cmd: &CommandIr,
    tool_name: &str,
    inherited_omitted: &[String],
) -> Vec<String> {
    cmd.params
        .iter()
        .filter(|param| !is_principal_type(&param.ty) && !is_stream_type(&param.ty))
        .filter(|param| {
            !inherited_omitted
                .iter()
                .any(|omitted| omitted_matches_param(ir, cmd, param, tool_name, omitted))
        })
        .flat_map(|param| param_surfaces(cmd, param))
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

fn contextual_omitted_tag(base: TokenStream, cmd: &CommandIr, surfaces: &[String]) -> TokenStream {
    let salt = omitted_surface_id(&format!(
        "subtree:{}",
        cmd.name_override
            .clone()
            .unwrap_or_else(|| to_kebab_case(&cmd.method_ident.to_string()))
    ));
    omitted_tag_append(quote! { (#base ^ #salt) }, surfaces)
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
    let stdout_check = if has_stdout {
        quote! {
            let __stdout = __result.stdout.ok_or_else(|| {
                golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                    "tool result did not contain declared stdout stream".to_string()
                ))
            })?;
        }
    } else {
        quote! {
            if __result.stdout.is_some() {
                return ::std::result::Result::Err(golem_rust::agentic::ToolError::Rpc(
                    golem_rust::agentic::RpcError::Protocol(
                        "tool result unexpectedly contained stdout stream".to_string()
                    )
                ));
            }
        }
    };

    match (ok, has_stdout) {
        (Some(ok), true) => quote! {
            #stdout_check
            let __value = __result.result.ok_or_else(|| {
                golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                    "tool result did not contain a value".to_string()
                ))
            })?;
            let __decoded = <#ok as golem_rust::FromSchema>::from_value(__value.value())
                .map_err(|__err| golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(__err.to_string())))?;
            ::std::result::Result::Ok((__decoded, __stdout))
        },
        (None, true) => quote! {
            #stdout_check
            if __result.result.is_some() {
                return ::std::result::Result::Err(golem_rust::agentic::ToolError::Rpc(
                    golem_rust::agentic::RpcError::Protocol("tool result unexpectedly contained a value".to_string())
                ));
            }
            ::std::result::Result::Ok(__stdout)
        },
        (Some(ok), false) => quote! {
            #stdout_check
            let __value = __result.result.ok_or_else(|| {
                golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(
                    "tool result did not contain a value".to_string()
                ))
            })?;
            let __decoded = <#ok as golem_rust::FromSchema>::from_value(__value.value())
                .map_err(|__err| golem_rust::agentic::ToolError::Rpc(golem_rust::agentic::RpcError::Protocol(__err.to_string())))?;
            ::std::result::Result::Ok(__decoded)
        },
        (None, false) => quote! {
            #stdout_check
            if __result.result.is_some() {
                return ::std::result::Result::Err(golem_rust::agentic::ToolError::Rpc(
                    golem_rust::agentic::RpcError::Protocol("tool result unexpectedly contained a value".to_string())
                ));
            }
            ::std::result::Result::Ok(())
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
