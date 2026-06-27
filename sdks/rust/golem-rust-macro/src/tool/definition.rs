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
use crate::tool::helpers::SeenKeys;
use crate::tool::ir::{ArgIr, CommandIr, ParamIr, ToolDefinitionIr};
use crate::tool::result::parse_result;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr, FnArg, ItemTrait, Lit, TraitItem};

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

    let annotation_item: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn tool_implementation_annotation() where Self: Sized;
    };
    item_trait.items.push(annotation_item);

    quote! {
        #[allow(async_fn_in_trait)]
        #item_trait

        #descriptor_fn
    }
    .into()
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
