// Copyright 2024-2025 Golem Cloud
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

use crate::agentic::helpers::{is_async_trait_attr, is_constructor_method, is_static_method};
use crate::agentic::{
    async_trait_in_agent_definition_error, generic_type_in_agent_method_error,
    generic_type_in_agent_return_type_error, generic_type_in_constructor_error, get_remote_client,
    multiple_constructor_methods_error, no_constructor_method_error,
};

use syn::spanned::Spanned;
use syn::ItemTrait;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::{punctuated::Punctuated, Error, Expr, ExprArray, ExprLit, Lit, Token};

struct ParsedHttpMount {
    mount: Option<syn::LitStr>,
    cors: Vec<syn::LitStr>,
    auth: bool,
    headers: Vec<(String, syn::LitStr)>,
}

fn parse_http_expr(expr: &Expr, out: &mut ParsedHttpMount) -> Result<(), Error> {
    match expr {
        Expr::Assign(assign) => {
            if let Expr::Path(left) = &*assign.left {
                if let Some(ident) = left.path.get_ident() {
                    match ident.to_string().as_str() {
                        "mount" => {
                            if let Expr::Lit(ExprLit {
                                lit: Lit::Str(lit), ..
                            }) = &*assign.right
                            {
                                out.mount = Some(lit.clone());
                                return Ok(());
                            } else {
                                return Err(Error::new_spanned(
                                    &assign.right,
                                    "mount must be a string literal",
                                ));
                            }
                        }
                        "auth" => {
                            if let Expr::Lit(ExprLit {
                                lit: Lit::Bool(b), ..
                            }) = &*assign.right
                            {
                                out.auth = b.value;
                                return Ok(());
                            } else {
                                return Err(Error::new_spanned(
                                    &assign.right,
                                    "auth must be a boolean literal",
                                ));
                            }
                        }
                        "cors" => {
                            if let Expr::Array(ExprArray { elems, .. }) = &*assign.right {
                                for elem in elems {
                                    if let Expr::Lit(ExprLit {
                                        lit: Lit::Str(lit), ..
                                    }) = elem
                                    {
                                        out.cors.push(lit.clone());
                                    } else {
                                        return Err(Error::new_spanned(
                                            elem,
                                            "cors entries must be string literals",
                                        ));
                                    }
                                }
                                return Ok(());
                            } else {
                                return Err(Error::new_spanned(
                                    &assign.right,
                                    "cors must be an array of string literals",
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        Expr::Call(call) => {
            if let Expr::Path(func) = &*call.func {
                if func.path.is_ident("headers") {
                    for arg in &call.args {
                        if let Expr::Assign(assign) = arg {
                            let key = if let Expr::Path(p) = &*assign.left {
                                p.path
                                    .get_ident()
                                    .ok_or_else(|| {
                                        Error::new_spanned(
                                            &assign.left,
                                            "header key must be an identifier",
                                        )
                                    })?
                                    .to_string()
                            } else {
                                return Err(Error::new_spanned(
                                    &assign.left,
                                    "header key must be an identifier",
                                ));
                            };
                            let val = if let Expr::Lit(ExprLit {
                                lit: Lit::Str(lit), ..
                            }) = &*assign.right
                            {
                                lit.clone()
                            } else {
                                return Err(Error::new_spanned(
                                    &assign.right,
                                    "header value must be string literal",
                                ));
                            };
                            out.headers.push((key, val));
                        } else {
                            return Err(Error::new_spanned(
                                arg,
                                "invalid headers syntax, must be key = value",
                            ));
                        }
                    }
                    return Ok(());
                }
            }
        }

        _ => {}
    }

    Err(Error::new_spanned(
        expr,
        "Unknown agent_definition parameter",
    ))
}

pub fn parse_definition_attributes(
    attrs: TokenStream,
) -> Result<(proc_macro2::TokenStream, Option<proc_macro2::TokenStream>), Error> {
    let mut mode = quote! {
        golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable
    };
    let mut http = ParsedHttpMount {
        mount: None,
        cors: vec![],
        auth: false,
        headers: vec![],
    };

    if attrs.is_empty() {
        return Ok((mode, None));
    }

    // Parse comma-separated expressions
    let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
    let exprs = parser.parse(attrs.into())?;

    for expr in exprs.iter() {
        // shorthand mode: ephemeral / durable
        if let Expr::Path(p) = expr {
            if p.path.is_ident("ephemeral") {
                mode = quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral };
                continue;
            } else if p.path.is_ident("durable") {
                mode =
                    quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable };
                continue;
            }
        }

        // mode = "..."
        if let Expr::Assign(assign) = expr {
            if let Expr::Path(left) = &*assign.left {
                if left.path.is_ident("mode") {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(lit), ..
                    }) = &*assign.right
                    {
                        mode = match lit.value().as_str() {
                            "ephemeral" => {
                                quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral }
                            }
                            "durable" => {
                                quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable }
                            }
                            other => {
                                return Err(Error::new_spanned(
                                    lit,
                                    format!("invalid mode `{}`", other),
                                ))
                            }
                        };
                        continue;
                    } else {
                        return Err(Error::new_spanned(
                            &assign.right,
                            "mode must be a string literal",
                        ));
                    }
                }
            }
        }

        // parse HTTP options
        parse_http_expr(expr, &mut http)?;
    }

    let http_tokens = http.mount.map(|mount| {
        let cors = http.cors;
        let auth = http.auth;

        quote! {
            golem_rust::agentic::make_http_mount_details(
                #mount,
                #auth,
                false,
                golem_rust::golem_agentic::golem::agent::common::CorsOptions {
                    allowed_patterns: vec![ #( #cors.to_string() ),* ],
                },
                None
            ).expect("Invalid HTTP mount configuration")
        }
    });

    Ok((mode, http_tokens))
}

pub fn agent_definition_impl(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut agent_definition_trait = syn::parse_macro_input!(item as ItemTrait);

    let (agent_mode, http_options) = match parse_definition_attributes(attrs) {
        Ok(v) => v,
        Err(err) => return err.to_compile_error().into(),
    };

    // if let Some(options) = http_options {
    //     dbg!(&options.to_string());
    // }

    let type_parameters = agent_definition_trait
        .generics
        .type_params()
        .map(|tp| tp.ident.to_string())
        .collect::<Vec<_>>();

    let has_async_trait_attribute = agent_definition_trait.attrs.iter().any(is_async_trait_attr);

    if has_async_trait_attribute {
        return async_trait_in_agent_definition_error(&agent_definition_trait).into();
    }

    match get_agent_type_with_remote_client(
        &agent_definition_trait,
        agent_mode,
        http_options,
        &type_parameters,
    ) {
        Ok(agent_type_with_remote_client) => {
            let AgentTypeWithRemoteClient {
                agent_type,
                remote_client,
            } = agent_type_with_remote_client;

            let registration_function: syn::TraitItem = syn::parse_quote! {
                fn __register_agent_type() {
                    let agent_type = #agent_type;

                    golem_rust::agentic::register_agent_type(
                        golem_rust::agentic::AgentTypeName(agent_type.type_name.to_string()),
                        agent_type
                    );
                }
            };

            let load_snapshot_item = get_load_snapshot_item();
            let save_snapshot_item = get_save_snapshot_item();

            agent_definition_trait.items.push(load_snapshot_item);
            agent_definition_trait.items.push(save_snapshot_item);
            agent_definition_trait.items.push(registration_function);

            let result = quote! {
                #[allow(async_fn_in_trait)]
                #agent_definition_trait
                #remote_client
            };

            result.into()
        }

        Err(invalid_trait_error) => invalid_trait_error,
    }
}

fn get_load_snapshot_item() -> syn::TraitItem {
    syn::parse_quote! {
        async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
            Err("load_snapshot not implemented".to_string())
        }
    }
}

fn get_save_snapshot_item() -> syn::TraitItem {
    syn::parse_quote! {
        async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
            Err("save_snapshot not implemented".to_string())
        }
    }
}

struct AgentTypeWithRemoteClient {
    agent_type: proc_macro2::TokenStream,
    remote_client: proc_macro2::TokenStream,
}

fn get_agent_type_with_remote_client(
    agent_definition_trait: &ItemTrait,
    mode_value: proc_macro2::TokenStream,
    http_options: Option<proc_macro2::TokenStream>,
    type_parameters: &[String],
) -> Result<AgentTypeWithRemoteClient, TokenStream> {
    let agent_def_trait_ident = &agent_definition_trait.ident;
    let agent_trait_name = agent_def_trait_ident.to_string();

    let mut constructor_methods = vec![];

    for item in &agent_definition_trait.items {
        if let syn::TraitItem::Fn(trait_fn) = item {
            if is_constructor_method(&trait_fn.sig, None) {
                constructor_methods.push(trait_fn.clone());
            }
        }
    }

    let http_options = if let Some(options) = http_options {
        quote! {
            Some(#options)
        }
    } else {
        quote! {
            None
        }
    };

    let methods = agent_definition_trait.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(trait_fn) = item {
            if is_constructor_method(&trait_fn.sig, None) {
                return None;
            }

            if is_static_method(&trait_fn.sig) {
                return None;
            }

            let name = &trait_fn.sig.ident;
            let method_name = &name.to_string();

            let method_description = extract_description(&trait_fn.attrs).unwrap_or_default();
            let method_prompt_hint = extract_prompt_hint(&trait_fn.attrs).unwrap_or_default();

            let mut input_schema_logic = vec![];

            let mut output_schema_logic = vec![];

            let input_schema_token = quote! {
                let mut multi_modal_inputs = vec![];
                let mut default_inputs = vec![];
            };

            let output_schema_token = quote! {
               let mut multi_modal_outputs = vec![];
               let mut default_outputs = vec![];
            };

            for input in &trait_fn.sig.inputs {
                if let syn::FnArg::Typed(pat_type) = input {
                    let param_name = match &*pat_type.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                        _ => "_".to_string(), // fallback for patterns like destructuring
                    };

                    let type_name = match &*pat_type.ty {
                        syn::Type::Path(type_path) => {
                            type_path.path.segments.last().unwrap().ident.to_string()
                        },
                        _ => "".to_string(),
                    };

                    if type_parameters.contains(&type_name) {
                        return generic_type_in_agent_method_error(pat_type.pat.span(), &type_name).into();
                    }

                    let ty = &pat_type.ty;
                    input_schema_logic.push(quote! {
                        let schema: golem_rust::agentic::StructuredSchema = <#ty as golem_rust::agentic::Schema>::get_type();
                        match schema {
                            golem_rust::agentic::StructuredSchema::Default(element_schema) => {
                                default_inputs.push((#param_name.to_string(), element_schema));
                            },
                            golem_rust::agentic::StructuredSchema::Multimodal(name_and_types) => {
                                multi_modal_inputs.extend(name_and_types);
                            }
                        }
                    });
                }
            }

            match &trait_fn.sig.output {
                syn::ReturnType::Default => (),
                syn::ReturnType::Type(_, ty) => {

                    let output_type_name = match &**ty {
                        syn::Type::Path(type_path) => {
                            type_path.path.segments.last().unwrap().ident.to_string()
                        },
                        _ => "".to_string(),
                    };

                    if type_parameters.contains(&output_type_name) {
                        return generic_type_in_agent_return_type_error(ty.span(), &output_type_name).into();
                    }

                    let is_unit = matches!(**ty, syn::Type::Tuple(ref t) if t.elems.is_empty());

                    if !is_unit {
                        output_schema_logic.push(quote! {
                            let schema = <#ty as golem_rust::agentic::Schema>::get_type();
                            match schema {
                                golem_rust::agentic::StructuredSchema::Default(element_schema) => {
                                    default_outputs.push(("return-value".to_string(), element_schema));
                                },
                                golem_rust::agentic::StructuredSchema::Multimodal(name_and_types) => {
                                    multi_modal_outputs.extend(name_and_types)
                                }
                            }
                        });
                    }
                }
            };

            let input_schema = quote! {
                {
                    #input_schema_token
                    #(#input_schema_logic)*
                    if !multi_modal_inputs.is_empty() {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(multi_modal_inputs)
                    } else {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(default_inputs)
                    }
                }
            };

            let output_schema = quote! {
                {
                    #output_schema_token
                    #(#output_schema_logic)*
                    if !multi_modal_outputs.is_empty() {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(multi_modal_outputs)
                    } else {
                        golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(default_outputs)
                    }
                }
            };

            Some(quote! {
                golem_rust::golem_agentic::golem::agent::common::AgentMethod {
                    name: #method_name.to_string(),
                    description: #method_description.to_string(),
                    prompt_hint: {
                        if #method_prompt_hint.is_empty() {
                            None
                        } else {
                            Some(#method_prompt_hint.to_string())
                        }
                    },
                    input_schema: #input_schema,
                    output_schema: #output_schema,
                    // TODO
                    http_endpoint: Vec::new(),
                }
            })
        } else {
            None
        }
    });

    let constructor_schema_init = quote! {
        let mut constructor_multi_modal_inputs = vec![];
        let mut constructor_default_inputs = vec![];
    };

    let mut constructor_parameters_with_schema: Vec<proc_macro2::TokenStream> = vec![];

    let mut constructor_param_defs = vec![];

    let mut constructor_param_names = vec![];

    if constructor_methods.is_empty() {
        return Err(no_constructor_method_error(agent_definition_trait).into());
    }

    if constructor_methods.len() > 1 {
        return Err(multiple_constructor_methods_error(agent_definition_trait).into());
    }

    let mut constructor_description = String::new();
    let mut constructor_prompt = String::new();
    let mut constructor_name = String::new();

    let high_level_description =
        extract_description(&agent_definition_trait.attrs).unwrap_or_default();

    if let Some(ctor_fn) = &constructor_methods.first().as_mut() {
        constructor_description = extract_description(&ctor_fn.attrs).unwrap_or_default();
        constructor_prompt = extract_prompt_hint(&ctor_fn.attrs).unwrap_or_default();
        constructor_name = ctor_fn.sig.ident.to_string();

        for input in &ctor_fn.sig.inputs {
            if let syn::FnArg::Typed(pat_type) = input {
                let param_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => {
                        let param_ident = &pat_ident.ident;

                        let ty = &pat_type.ty;

                        let type_name = match &**ty {
                            syn::Type::Path(type_path) => {
                                type_path.path.segments.last().unwrap().ident.to_string()
                            }
                            _ => "".to_string(),
                        };

                        if type_parameters.contains(&type_name) {
                            return Err(generic_type_in_constructor_error(
                                pat_type.span(),
                                &type_name,
                            )
                            .into());
                        }

                        constructor_param_defs.push(quote! {
                            #param_ident: #ty
                        });

                        constructor_param_names.push(quote! {
                            #param_ident
                        });

                        pat_ident.ident.to_string()
                    }
                    _ => "_".to_string(),
                };

                let ty = &pat_type.ty;
                constructor_parameters_with_schema.push(quote! {

                    let schema: golem_rust::agentic::StructuredSchema = <#ty as golem_rust::agentic::Schema>::get_type();
                    match schema {
                        golem_rust::agentic::StructuredSchema::Default(element_schema) => {
                            constructor_default_inputs.push((#param_name.to_string(), element_schema));
                        },
                        golem_rust::agentic::StructuredSchema::Multimodal(name_and_types) => {
                            constructor_multi_modal_inputs.extend(name_and_types);
                        }
                    }
                });
            }
        }
    }

    let agent_constructor_input_schema = quote! {
        {
            #constructor_schema_init
            #(#constructor_parameters_with_schema)*
            if !constructor_multi_modal_inputs.is_empty() {
                golem_rust::golem_agentic::golem::agent::common::DataSchema::Multimodal(constructor_multi_modal_inputs)
            } else {
                golem_rust::golem_agentic::golem::agent::common::DataSchema::Tuple(constructor_default_inputs)
            }
        }
    };

    let constructor_data_schema_token = quote! {
        let constructor_data_schema = {
            #agent_constructor_input_schema
        };
    };

    let remote_client = get_remote_client(
        agent_definition_trait,
        constructor_param_defs,
        constructor_param_names,
        type_parameters,
    );

    let constructor_prompt_hint = if constructor_prompt.is_empty() {
        quote! { None }
    } else {
        quote! { Some(#constructor_prompt.to_string()) }
    };

    let constructor_name = if constructor_name.is_empty() {
        quote! { None }
    } else {
        quote! { Some(#constructor_name.to_string()) }
    };

    let agent_constructor = quote! {
        {
         #constructor_data_schema_token

         golem_rust::golem_agentic::golem::agent::common::AgentConstructor {
            name: #constructor_name,
            description: #constructor_description.to_string(),
            prompt_hint: #constructor_prompt_hint,
            input_schema: constructor_data_schema,
         }
        }
    };

    let high_level_description_ident = if high_level_description.is_empty() {
        quote! { "" }
    } else {
        quote! { #high_level_description }
    };

    Ok(AgentTypeWithRemoteClient {
        agent_type: quote! {
            golem_rust::golem_agentic::golem::agent::common::AgentType {
                type_name: #agent_trait_name.to_string(),
                description: #high_level_description_ident.to_string(),
                methods: vec![#(#methods),*],
                dependencies: vec![],
                constructor: #agent_constructor,
                mode: #mode_value,
                http_mount: #http_options,
            }
        },
        remote_client,
    })
}

fn extract_description(attrs: &[syn::Attribute]) -> Option<String> {
    extract_meta(attrs, "description")
}

fn extract_prompt_hint(attrs: &[syn::Attribute]) -> Option<String> {
    extract_meta(attrs, "prompt")
}

fn extract_meta(attrs: &[syn::Attribute], key: &str) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident(key) {
            if let Ok(syn::Lit::Str(lit_str)) = attr.parse_args::<syn::Lit>() {
                return Some(lit_str.value());
            }
        }
    }
    None
}
