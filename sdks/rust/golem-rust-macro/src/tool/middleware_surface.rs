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

use crate::tool::client::{
    canonical_value_name, inherited_root_params, is_principal_type, is_stream_type,
    omitted_marker_ident, param_omission_surfaces, split_result, stream_idents,
};
use crate::tool::descriptor::descriptor_fn_ident;
use crate::tool::helpers::to_kebab_case;
use crate::tool::ir::{CommandIr, ParamIr, ToolDefinitionIr};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitBool, LitStr, Path, ReturnType, Token, Type, bracketed, parenthesized};

pub fn synthesize_middleware_surface(ir: &ToolDefinitionIr) -> TokenStream {
    let visibility = &ir.visibility;
    let trait_ident = &ir.trait_ident;
    let underlying_ident = format_ident!("{}Underlying", trait_ident);
    let middleware_ident = format_ident!("{}Middleware", trait_ident);
    let descriptor_ident = descriptor_fn_ident(trait_ident);
    let projection_macro = projection_macro_ident(trait_ident);
    let projection_macro_definition = synthesize_projection_macro(ir);
    let direct_leaf_names = ir
        .commands
        .iter()
        .filter(|command| command.subtree.is_none())
        .map(|command| &command.method_ident);
    let direct_leaf_names_for_trait = direct_leaf_names.clone();

    quote! {
        #projection_macro_definition

        #visibility struct #underlying_ident {
            underlying: golem_rust::tool::UnderlyingTool,
        }

        impl golem_rust::tool::ToolUnderlying for #underlying_ident {
            fn __golem_from_underlying(underlying: golem_rust::tool::UnderlyingTool) -> Self {
                Self { underlying }
            }

            fn __golem_tool_descriptor() -> golem_rust::tool::Tool {
                #descriptor_ident(&mut golem_rust::agentic::ToolBuildCtx::new())
                    .and_then(|descriptor| descriptor.try_to_native_tool())
                    .expect("tool descriptor build failed")
            }
        }

        impl #underlying_ident {
            #projection_macro!(
                underlying,
                #underlying_ident,
                #descriptor_ident,
                [],
                [],
                [],
                [],
                [#(#direct_leaf_names)*]
            );
        }

        #[allow(async_fn_in_trait)]
        #visibility trait #middleware_ident<U = #underlying_ident>
        where
            U: golem_rust::tool::ToolUnderlying,
        {
            #projection_macro!(
                middleware,
                #underlying_ident,
                #descriptor_ident,
                [],
                [],
                [],
                [],
                [#(#direct_leaf_names_for_trait)*]
            );

            #[doc(hidden)]
            fn __golem_tool_middleware_annotation()
            where
                Self: Sized;

            #[doc(hidden)]
            fn __golem_presented_tool_descriptor() -> golem_rust::tool::Tool
            where
                Self: Sized,
            {
                #descriptor_ident(&mut golem_rust::agentic::ToolBuildCtx::new())
                    .and_then(|descriptor| descriptor.try_to_native_tool())
                    .expect("tool descriptor build failed")
            }
        }
    }
}

fn projection_macro_ident(trait_ident: &Ident) -> Ident {
    format_ident!("__golem_tool_middleware_projection_for_{}", trait_ident)
}

fn projection_macro_path(path: &Path) -> Path {
    let mut rewritten = path.clone();
    if let Some(last) = rewritten.segments.last_mut() {
        last.ident = projection_macro_ident(&last.ident);
    }
    rewritten
}

fn synthesize_projection_macro(ir: &ToolDefinitionIr) -> TokenStream {
    let macro_ident = projection_macro_ident(&ir.trait_ident);
    let tool_name = to_kebab_case(&ir.trait_ident.to_string());
    let starts = ir.commands.iter().enumerate().map(|(command_index, _)| {
        let state = projection_state_ident(command_index, 0);
        quote! {
            #macro_ident!(
                @#state
                $mode,
                $target,
                $descriptor,
                [$($rust_path)*],
                [$($command_path)*],
                [$($ancestor)*],
                [$($omitted)*],
                [$($direct)*],
                [],
                [];
                $($omitted)*
            );
        }
    });
    let arms = ir
        .commands
        .iter()
        .enumerate()
        .flat_map(|(command_index, command)| {
            projection_command_arms(ir, command, &tool_name, &macro_ident, command_index)
        });

    quote! {
        #[doc(hidden)]
        macro_rules! #macro_ident {
            (
                $mode:ident,
                $target:ident,
                $descriptor:path,
                [$($rust_path:ident)*],
                [$($command_path:expr)*],
                [$($ancestor:tt)*],
                [$($omitted:ident)*],
                [$($direct:ident)*]
            ) => {
                #(#starts)*
            };
            #(#arms)*
        }

        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) use #macro_ident;
    }
}

fn projection_command_arms(
    ir: &ToolDefinitionIr,
    command: &CommandIr,
    tool_name: &str,
    macro_ident: &Ident,
    command_index: usize,
) -> Vec<TokenStream> {
    let params = projected_command_params(ir, command, tool_name);
    let mut arms = params
        .iter()
        .enumerate()
        .flat_map(|(param_index, param)| {
            projection_param_arms(
                ir,
                command,
                param,
                tool_name,
                macro_ident,
                command_index,
                param_index,
            )
        })
        .collect::<Vec<_>>();
    arms.push(projection_done_arm(
        command,
        tool_name,
        command_index,
        params.len(),
    ));
    arms
}

fn projected_command_params(
    ir: &ToolDefinitionIr,
    command: &CommandIr,
    tool_name: &str,
) -> Vec<ParamIr> {
    inherited_root_params(ir, command, tool_name)
        .into_iter()
        .chain(command.params.iter().cloned())
        .filter(|param| type_last_ident(&param.ty).as_deref() != Some("OutputStream"))
        .collect()
}

fn projection_param_arms(
    ir: &ToolDefinitionIr,
    command: &CommandIr,
    param: &ParamIr,
    tool_name: &str,
    macro_ident: &Ident,
    command_index: usize,
    param_index: usize,
) -> Vec<TokenStream> {
    let state = projection_state_ident(command_index, param_index);
    let next_state = projection_state_ident(command_index, param_index + 1);
    let ident = &param.ident;
    let ty = &param.ty;
    let canonical_name = canonical_value_name(ir, command, param, tool_name);
    let surfaces = param_omission_surfaces(ir, command, param, tool_name);
    let markers = surfaces.iter().map(|surface| omitted_marker_ident(surface));
    let markers_for_keep = markers.clone();
    let keep = quote! {
        #macro_ident!(
            @#next_state
            $mode,
            $target,
            $descriptor,
            [$($rust_path)*],
            [$($command_path)*],
            [$($ancestor)*],
            [$($omitted)*],
            [$($direct)*],
            [$($args)* (#ident: #ty => #canonical_name)],
            [$($new_omitted)* #(#markers_for_keep)*];
            $($omitted)*
        );
    };
    let omit = quote! {
        #macro_ident!(
            @#next_state
            $mode,
            $target,
            $descriptor,
            [$($rust_path)*],
            [$($command_path)*],
            [$($ancestor)*],
            [$($omitted)*],
            [$($direct)*],
            [$($args)*],
            [$($new_omitted)*];
            $($omitted)*
        );
    };

    let base_pattern = quote! {
        $mode:ident,
        $target:ident,
        $descriptor:path,
        [$($rust_path:ident)*],
        [$($command_path:expr)*],
        [$($ancestor:tt)*],
        [$($omitted:ident)*],
        [$($direct:ident)*],
        [$($args:tt)*],
        [$($new_omitted:ident)*]
    };

    if is_stream_type(&param.ty) {
        return vec![quote! {
            (@#state #base_pattern; $($all:ident)*) => {
                #keep
            };
        }];
    }

    let marker_arms = markers.map(|marker| {
        quote! {
            (@#state #base_pattern; #marker $($all:ident)*) => {
                #omit
            };
        }
    });
    let unknown = quote! {
        (@#state #base_pattern; $unknown:ident $($all:ident)*) => {
            #macro_ident!(
                @#state
                $mode,
                $target,
                $descriptor,
                [$($rust_path)*],
                [$($command_path)*],
                [$($ancestor)*],
                [$($omitted)*],
                [$($direct)*],
                [$($args)*],
                [$($new_omitted)*];
                $($all)*
            );
        };
    };
    let empty = quote! {
        (@#state #base_pattern; ) => {
            #keep
        };
    };
    marker_arms.chain([unknown, empty]).collect()
}

fn projection_done_arm(
    command: &CommandIr,
    tool_name: &str,
    command_index: usize,
    param_count: usize,
) -> TokenStream {
    let state = projection_state_ident(command_index, param_count);
    let method_ident = &command.method_ident;
    let command_name = command
        .name_override
        .clone()
        .unwrap_or_else(|| to_kebab_case(&method_ident.to_string()));
    let command_path_append = if command_name == tool_name {
        quote! {}
    } else {
        quote! { #command_name }
    };
    let base_pattern = quote! {
        $mode:ident,
        $target:ident,
        $descriptor:path,
        [$($rust_path:ident)*],
        [$($command_path:expr)*],
        [$($ancestor:tt)*],
        [$($omitted:ident)*],
        [$($direct:ident)*],
        [$($args:tt)*],
        [$($new_omitted:ident)*]
    };

    if let Some(subtree) = &command.subtree {
        let child_macro = projection_macro_path(&subtree.path);
        quote! {
            (@#state #base_pattern; $($rest:ident)*) => {
                #child_macro!(
                    $mode,
                    $target,
                    $descriptor,
                    [$($rust_path)* #method_ident],
                    [$($command_path)* #command_path_append],
                    [$($ancestor)* $($args)*],
                    [$($omitted)* $($new_omitted)*],
                    [$($direct)*]
                );
            };
        }
    } else {
        let output = &command.output;
        let (_, has_stdout) = stream_idents(command);
        quote! {
            (@#state #base_pattern; $($rest:ident)*) => {
                golem_rust::__golem_emit_tool_middleware_leaf! {
                    mode: $mode,
                    target: $target,
                    descriptor: $descriptor,
                    method: [$($rust_path)* #method_ident],
                    command_path: [$($command_path)* #command_path_append],
                    params: [$($ancestor)* $($args)*],
                    output: (#output),
                    stdout: #has_stdout,
                    direct: [$($direct)*]
                }
            };
        }
    }
}

fn projection_state_ident(command_index: usize, param_index: usize) -> Ident {
    format_ident!("__golem_middleware_cmd_{command_index}_param_{param_index}")
}

struct ProjectedParam {
    ident: Ident,
    ty: Type,
    canonical_name: LitStr,
}

impl Parse for ProjectedParam {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let content;
        parenthesized!(content in input);
        let ident = content.parse()?;
        content.parse::<Token![:]>()?;
        let ty = content.parse()?;
        content.parse::<Token![=>]>()?;
        let canonical_name = content.parse()?;
        Ok(Self {
            ident,
            ty,
            canonical_name,
        })
    }
}

struct LeafInput {
    mode: Ident,
    descriptor: Path,
    method: Vec<Ident>,
    command_path: Vec<LitStr>,
    params: Vec<ProjectedParam>,
    output: ReturnType,
    stdout: bool,
    direct: Vec<Ident>,
}

impl Parse for LeafInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        parse_key(input, "mode")?;
        let mode = input.parse()?;
        input.parse::<Token![,]>()?;
        parse_key(input, "target")?;
        input.parse::<Ident>()?;
        input.parse::<Token![,]>()?;
        parse_key(input, "descriptor")?;
        let descriptor = input.parse()?;
        input.parse::<Token![,]>()?;
        parse_key(input, "method")?;
        let method = parse_bracketed(input)?;
        input.parse::<Token![,]>()?;
        parse_key(input, "command_path")?;
        let command_path = parse_bracketed(input)?;
        input.parse::<Token![,]>()?;
        parse_key(input, "params")?;
        let params = parse_bracketed(input)?;
        input.parse::<Token![,]>()?;
        parse_key(input, "output")?;
        let output_content;
        parenthesized!(output_content in input);
        let output = output_content.parse()?;
        input.parse::<Token![,]>()?;
        parse_key(input, "stdout")?;
        let stdout = input.parse::<LitBool>()?.value;
        input.parse::<Token![,]>()?;
        parse_key(input, "direct")?;
        let direct = parse_bracketed(input)?;
        let _ = input.parse::<Token![,]>();
        Ok(Self {
            mode,
            descriptor,
            method,
            command_path,
            params,
            output,
            stdout,
            direct,
        })
    }
}

fn parse_key(input: ParseStream<'_>, expected: &str) -> syn::Result<()> {
    let key = input.parse::<Ident>()?;
    if key != expected {
        return Err(syn::Error::new(
            key.span(),
            format!("expected `{expected}`"),
        ));
    }
    input.parse::<Token![:]>()?;
    Ok(())
}

fn parse_bracketed<T: Parse>(input: ParseStream<'_>) -> syn::Result<Vec<T>> {
    let content;
    bracketed!(content in input);
    let mut values = Vec::new();
    while !content.is_empty() {
        values.push(content.parse()?);
        let _ = content.parse::<Token![,]>();
    }
    Ok(values)
}

pub fn emit_tool_middleware_leaf(input: TokenStream) -> syn::Result<TokenStream> {
    let input = syn::parse2::<LeafInput>(input)?;
    let method_name = input
        .method
        .iter()
        .map(|ident| ident.unraw().to_string())
        .collect::<Vec<_>>()
        .join("__");
    if input.method.len() > 1
        && input
            .direct
            .iter()
            .any(|direct| direct.unraw() == method_name.as_str())
    {
        return Err(syn::Error::new(
            Span::call_site(),
            format!(
                "flattened middleware method-name collision: `{method_name}` is both a descendant path and a directly authored command"
            ),
        ));
    }
    let method_ident = if input.method.len() == 1 {
        input.method[0].clone()
    } else {
        Ident::new(&method_name, Span::call_site())
    };
    let underlying_ident = fresh_projected_ident(&input.params, "underlying");
    let args = input.params.iter().map(|param| {
        let ident = &param.ident;
        let ty = &param.ty;
        quote! { #ident: #ty }
    });
    let result_ty = middleware_result_type(&input.output, input.stdout);

    match input.mode.to_string().as_str() {
        "middleware" => Ok(quote! {
            async fn #method_ident(
                &self,
                #underlying_ident: &mut U
                #(, #args)*
            ) -> #result_ty;
        }),
        "underlying" => emit_underlying_method(input, method_ident, result_ty),
        _ => Err(syn::Error::new(
            input.mode.span(),
            "unknown generated middleware projection mode",
        )),
    }
}

fn emit_underlying_method(
    input: LeafInput,
    method_ident: Ident,
    result_ty: TokenStream,
) -> syn::Result<TokenStream> {
    let descriptor = input.descriptor;
    let command_path = input.command_path;
    let param_values_ident = fresh_projected_ident(&input.params, "__param_values");
    let value_ident = fresh_projected_ident(&input.params, "__value");
    let command_path_ident = fresh_projected_ident(&input.params, "__command_path");
    let descriptor_ident = fresh_projected_ident(&input.params, "__descriptor");
    let command_index_ident = fresh_projected_ident(&input.params, "__command_index");
    let model_ident = fresh_projected_ident(&input.params, "__model");
    let input_ident = fresh_projected_ident(&input.params, "__input");
    let result_ident = fresh_projected_ident(&input.params, "__result");
    let args = input
        .params
        .iter()
        .filter(|param| !is_principal_type(&param.ty))
        .map(|param| {
            let ident = &param.ident;
            let ty = &param.ty;
            quote! { #ident: #ty }
        });
    let values = input
        .params
        .iter()
        .filter(|param| !is_stream_type(&param.ty) && !is_principal_type(&param.ty))
        .map(|param| {
            let ident = &param.ident;
            let canonical_name = &param.canonical_name;
            quote! {
                let #value_ident = <_ as golem_rust::agentic::Schema>::to_schema_value(#ident)
                    .map_err(|error| golem_rust::tool::ToolInvokeError::InvalidInput(error.to_string()))?;
                #param_values_ident.push((#canonical_name, #value_ident));
            }
        });
    let stdin = input
        .params
        .iter()
        .find(|param| type_last_ident(&param.ty).as_deref() == Some("InputStream"))
        .map(|param| {
            let ident = &param.ident;
            quote! { ::std::option::Option::Some(#ident) }
        })
        .unwrap_or_else(|| quote! { ::std::option::Option::None });
    let invoke = underlying_invoke(&input.output, stdin, &command_path_ident, &input_ident);
    let decode = decode_underlying_result(&input.output, input.stdout, &result_ident);

    Ok(quote! {
        pub async fn #method_ident(&mut self #(, #args)*) -> #result_ty {
            let mut #param_values_ident: ::std::vec::Vec<(&'static str, golem_rust::SchemaValue)> =
                ::std::vec::Vec::new();
            #(#values)*
            let #command_path_ident = ::std::vec![#(#command_path.to_string()),*];
            let #descriptor_ident = #descriptor(&mut golem_rust::agentic::ToolBuildCtx::new())
                .map_err(|error| golem_rust::tool::ToolInvokeError::InvalidInput(error.to_string()))?;
            let #command_index_ident = #descriptor_ident.command_index_by_path(&#command_path_ident)
                .ok_or_else(|| golem_rust::tool::ToolInvokeError::InvalidCommandPath(#command_path_ident.clone()))?;
            let #model_ident = #descriptor_ident.canonical_input_model(#command_index_ident)
                .map_err(|error| golem_rust::tool::ToolInvokeError::InvalidInput(error.to_string()))?;
            let #input_ident = golem_rust::agentic::build_canonical_input(&#model_ident, #param_values_ident)
                .map_err(golem_rust::tool::ToolInvokeError::InvalidInput)?;
            let #result_ident = #invoke?;
            #decode
        }
    })
}

fn middleware_result_type(output: &ReturnType, has_stdout: bool) -> TokenStream {
    let (ok, error) = split_result(output);
    let error = error
        .map(|error| quote! { #error })
        .unwrap_or_else(|| quote! { ::std::convert::Infallible });
    let ok = match (ok, has_stdout) {
        (Some(ok), true) => quote! { (#ok, golem_rust::tool::InputStream) },
        (None, true) => quote! { golem_rust::tool::InputStream },
        (Some(ok), false) => quote! { #ok },
        (None, false) => quote! { () },
    };
    quote! { ::std::result::Result<#ok, golem_rust::tool::ToolInvokeError<#error>> }
}

fn underlying_invoke(
    output: &ReturnType,
    stdin: TokenStream,
    command_path_ident: &Ident,
    input_ident: &Ident,
) -> TokenStream {
    let (_, error) = split_result(output);
    match error {
        Some(error) => quote! {
            self.underlying.invoke_with(
                #command_path_ident,
                #input_ident,
                #stdin,
                <#error as golem_rust::agentic::ToolErrorSchema>::from_error_payload_value,
            ).await
        },
        None => quote! {
            self.underlying.invoke_with(
                #command_path_ident,
                #input_ident,
                #stdin,
                |_| ::std::result::Result::Err(
                    "underlying returned a custom error for an infallible command".to_string()
                ),
            ).await
        },
    }
}

fn decode_underlying_result(
    output: &ReturnType,
    has_stdout: bool,
    result_ident: &Ident,
) -> TokenStream {
    let (ok, _) = split_result(output);
    match (ok, has_stdout) {
        (Some(ok), true) => quote! {
            golem_rust::tool::decode_result_with_stdout::<#ok, _>(#result_ident)
        },
        (None, true) => quote! {
            golem_rust::tool::decode_result_stdout_only(#result_ident)
        },
        (Some(ok), false) => quote! {
            golem_rust::tool::decode_result_value::<#ok, _>(#result_ident)
        },
        (None, false) => quote! {
            golem_rust::tool::decode_result_empty(#result_ident)
        },
    }
}

fn fresh_projected_ident(params: &[ProjectedParam], preferred: &str) -> Ident {
    let mut candidate = preferred.to_string();
    while params
        .iter()
        .any(|param| param.ident.unraw() == candidate.as_str())
    {
        candidate.push('_');
    }
    Ident::new(&candidate, Span::call_site())
}

fn type_last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_projection_macro_is_well_formed() {
        let item: syn::ItemTrait = syn::parse_quote! {
            pub trait Echo {
                fn echo(&self, value: String) -> String;
            }
        };
        let ir = crate::tool::definition::build_tool_definition_ir(&item, None).unwrap();
        let tokens = synthesize_middleware_surface(&ir);
        syn::parse2::<syn::File>(tokens).unwrap();
    }

    #[test]
    fn flattened_leaf_collision_has_targeted_diagnostic() {
        let error = emit_tool_middleware_leaf(quote! {
            mode: middleware,
            target: Parent,
            descriptor: descriptor,
            method: [remote add],
            command_path: ["remote" "add"],
            params: [],
            output: (),
            stdout: false,
            direct: [remote__add]
        })
        .expect_err("a descendant method must not shadow a directly authored command");

        assert!(error.to_string().contains(
            "flattened middleware method-name collision: `remote__add` is both a descendant path and a directly authored command"
        ));
    }

    #[test]
    fn raw_direct_leaf_collision_has_targeted_diagnostic() {
        let error = emit_tool_middleware_leaf(quote! {
            mode: middleware,
            target: Parent,
            descriptor: descriptor,
            method: [remote add],
            command_path: ["remote" "add"],
            params: [],
            output: (),
            stdout: false,
            direct: [r#remote__add]
        })
        .expect_err("a raw direct method must not shadow a flattened descendant method");

        assert!(error.to_string().contains(
            "flattened middleware method-name collision: `remote__add` is both a descendant path and a directly authored command"
        ));
    }
}
