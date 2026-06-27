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

//! Metadata synthesis: turning the parsed [`ToolDefinitionIr`] into the
//! `__golem_tool_descriptor_for_<Trait>` free function that builds the runtime
//! [`ExtendedToolType`].
//!
//! The free function is emitted alongside the `#[tool_definition]` trait so that
//! a parent tool's `#[command(subtree = path::Child)]` can reach it through the
//! child trait's module path even though no concrete `Self` is available. The
//! trait also gets a hidden `__tool_descriptor()` default method that calls the
//! free function; `#[tool_implementation]` emits the `#[ctor]` that registers
//! the descriptor, so only an implemented tool is ever registered.

use crate::tool::helpers::to_kebab_case;
use crate::tool::ir::{
    ArgIr, ArgPlacement, ArgSubKind, CommandAnnotationsIr, CommandIr, ConstraintIr, DocIr,
    PathDirectionIr, PathKindIr, QuantifierIr, RefIr, RepeatableMode, ResultIr, ToolDefinitionIr,
};
use crate::tool::synthesis::doc_tokens;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::{Error, Expr, GenericArgument, Ident, Path, PathArguments, ReturnType, Type};

/// The deterministic free-function name for a tool definition trait.
pub fn descriptor_fn_ident(trait_ident: &Ident) -> Ident {
    format_ident!("__golem_tool_descriptor_for_{}", trait_ident)
}

/// Emits the module-level `__golem_tool_descriptor_for_<Trait>` free function.
pub fn synthesize_descriptor_fn(ir: &ToolDefinitionIr) -> Result<TokenStream, Error> {
    let plan = Plan::analyze(ir)?;
    let fn_ident = descriptor_fn_ident(&ir.trait_ident);
    let trait_name = ir.trait_ident.to_string();

    let version = match &ir.version {
        Some(v) => quote! { #v.to_string() },
        None => quote! { env!("CARGO_PKG_VERSION").to_string() },
    };

    // Index 0 is always the root command.
    let root_node = build_root_node(ir, &plan)?;

    // Every non-root command becomes either a leaf subcommand or a grafted
    // subtree, linked beneath the root (index 0).
    let mut links = Vec::new();
    for (idx, cmd) in ir.commands.iter().enumerate() {
        if Some(idx) == plan.root_idx {
            continue;
        }
        if cmd.subtree.is_some() {
            links.push(build_subtree_link(cmd, &plan)?);
        } else {
            links.push(build_leaf_link(cmd, &plan)?);
        }
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #fn_ident(
            ctx: &mut golem_rust::agentic::ToolBuildCtx,
        ) -> ::std::result::Result<
            golem_rust::agentic::ExtendedToolType,
            golem_rust::agentic::ToolBuildError,
        > {
            ctx.with_descriptor(concat!(module_path!(), "::", #trait_name), |ctx| {
                #[allow(unused_variables, unused_mut)]
                let mut commands: ::std::vec::Vec<golem_rust::agentic::ExtendedCommandNode> =
                    ::std::vec::Vec::new();
                commands.push(#root_node);
                #(#links)*
                let mut __tool = golem_rust::agentic::ExtendedToolType {
                    version: #version,
                    commands,
                };
                // A nested subtree child descriptor returns its raw tree with
                // `value-is` literals still deferred; only the outermost
                // descriptor normalizes the fully grafted tree, once every
                // ancestor subtree global and inherited-global de-projection is in
                // scope. See `ToolBuildCtx::is_outermost_descriptor`.
                if ctx.is_outermost_descriptor() {
                    golem_rust::agentic::normalize_inherited_globals(&mut __tool)?;
                }
                ::std::result::Result::Ok(__tool)
            })
        }
    })
}

/// Macro-time facts derived from the trait, with all divergence rules checked.
struct Plan {
    tool_name: String,
    /// Index of the implicit-body method (`kebab(method) == tool name`), if any.
    root_idx: Option<usize>,
    /// Long names + aliases of the root command's globals; descendant commands
    /// must not re-project these (they are inherited by walking root→node).
    root_global_names: BTreeSet<String>,
}

impl Plan {
    fn analyze(ir: &ToolDefinitionIr) -> Result<Self, Error> {
        let tool_name = to_kebab_case(&ir.trait_ident.to_string());

        let mut implicit: Vec<usize> = Vec::new();
        for (idx, cmd) in ir.commands.iter().enumerate() {
            if to_kebab_case(&cmd.method_ident.to_string()) == tool_name {
                implicit.push(idx);
            }
        }
        if implicit.len() > 1 {
            let second = &ir.commands[implicit[1]];
            return Err(Error::new(
                second.method_ident.span(),
                format!(
                    "multiple methods map to the tool's root command name `{tool_name}`; \
                     only one method may be the implicit-body handler (§5.8.1)"
                ),
            ));
        }
        let root_idx = implicit.first().copied();

        if let Some(i) = root_idx {
            let cmd = &ir.commands[i];
            if cmd.subtree.is_some() {
                return Err(Error::new(
                    cmd.method_ident.span(),
                    "the implicit-body method cannot also be a #[command(subtree = ...)]",
                ));
            }
            if let Some(name) = &cmd.name_override
                && name != &tool_name
            {
                return Err(Error::new(
                    cmd.method_ident.span(),
                    format!(
                        "the implicit-body method's #[command(name = {name:?})] diverges from the \
                         tool name {tool_name:?}; the root command name must equal the tool name (§5.8.1)"
                    ),
                ));
            }
        }

        for (idx, cmd) in ir.commands.iter().enumerate() {
            if Some(idx) == root_idx {
                continue;
            }
            let name = command_name(cmd, &tool_name, false);
            if name == tool_name {
                return Err(Error::new(
                    cmd.method_ident.span(),
                    format!(
                        "command `{name}` collides with the tool's root command name; \
                         rename the method or use #[command(name = ...)]"
                    ),
                ));
            }
            if cmd.subtree.is_some() {
                if cmd.annotations.is_some() {
                    return Err(Error::new(
                        cmd.method_ident.span(),
                        "annotations are not supported on a #[command(subtree = ...)] method \
                         (the model places annotations on a command body)",
                    ));
                }
                if !cmd.constraints.is_empty() || cmd.result.is_some() {
                    return Err(Error::new(
                        cmd.method_ident.span(),
                        "#[constraint] / #[result] are not supported on a #[command(subtree = ...)] method",
                    ));
                }
            }
        }

        let mut root_global_names = BTreeSet::new();
        if let Some(i) = root_idx {
            for param in &ir.commands[i].params {
                let arg = arg_for(&ir.commands[i], &param.ident);
                if arg.map(|a| a.placement) == Some(Some(ArgPlacement::Global)) {
                    root_global_names.insert(to_kebab_case(&param.ident.to_string()));
                    if let Some(a) = arg {
                        for alias in &a.aliases {
                            root_global_names.insert(alias.clone());
                        }
                    }
                }
            }
        }

        Ok(Plan {
            tool_name,
            root_idx,
            root_global_names,
        })
    }
}

fn command_name(cmd: &CommandIr, tool_name: &str, is_root: bool) -> String {
    if is_root {
        tool_name.to_string()
    } else {
        cmd.name_override
            .clone()
            .unwrap_or_else(|| to_kebab_case(&cmd.method_ident.to_string()))
    }
}

fn arg_for<'a>(cmd: &'a CommandIr, ident: &Ident) -> Option<&'a ArgIr> {
    cmd.args.iter().find(|a| &a.param == ident)
}

/// The surface names a parameter would project onto: its kebab long name plus
/// any `#[arg(aliases = [...])]`. Used to decide whether a parameter repeats an
/// inherited global (which is keyed by long name *and* aliases), so a parameter
/// aliased to an ancestor global is de-projected even when its long name differs.
fn param_surface_names(ident: &Ident, arg: Option<&ArgIr>) -> Vec<String> {
    let mut names = vec![to_kebab_case(&ident.to_string())];
    if let Some(a) = arg {
        names.extend(a.aliases.iter().cloned());
    }
    names
}

fn repeats_inherited_global(
    ident: &Ident,
    arg: Option<&ArgIr>,
    inherited: &BTreeSet<String>,
) -> bool {
    param_surface_names(ident, arg)
        .iter()
        .any(|n| inherited.contains(n))
}

/// Builds the index-0 root command node. With an implicit-body method the root
/// is a full command (its globals + body); otherwise it is a pure dispatcher.
fn build_root_node(ir: &ToolDefinitionIr, plan: &Plan) -> Result<TokenStream, Error> {
    if let Some(i) = plan.root_idx {
        let cmd = &ir.commands[i];
        build_command_node(cmd, &plan.tool_name, true, &BTreeSet::new())
    } else {
        let name = &plan.tool_name;
        let doc = doc_tokens(&ir.doc);
        Ok(quote! {
            golem_rust::agentic::ExtendedCommandNode {
                name: #name.to_string(),
                aliases: ::std::vec::Vec::new(),
                doc: #doc,
                globals: golem_rust::agentic::ExtendedGlobals::default(),
                subcommands: ::std::vec::Vec::new(),
                body: ::std::option::Option::None,
            }
        })
    }
}

/// Emits the block that pushes a leaf subcommand node and links it under root.
fn build_leaf_link(cmd: &CommandIr, plan: &Plan) -> Result<TokenStream, Error> {
    let node = build_command_node(cmd, &plan.tool_name, false, &plan.root_global_names)?;
    Ok(quote! {
        {
            let __idx = commands.len() as i32;
            commands.push(#node);
            commands[0].subcommands.push(__idx);
        }
    })
}

/// Emits the block that builds a child descriptor, grafts it as a pure
/// dispatcher, prepends the subtree method's params as placeholder globals, and
/// links it under root.
fn build_subtree_link(cmd: &CommandIr, plan: &Plan) -> Result<TokenStream, Error> {
    let subtree = cmd.subtree.as_ref().expect("subtree present");
    let call_path = subtree_call_path(&subtree.path)?;
    let expected_name = command_name(cmd, &plan.tool_name, false);

    let override_name = match &subtree.name_override {
        Some(n) => quote! { ::std::option::Option::Some(#n.to_string()) },
        None => quote! { ::std::option::Option::None },
    };
    let override_doc = if cmd.doc == DocIr::default() {
        quote! { ::std::option::Option::None }
    } else {
        let doc = doc_tokens(&cmd.doc);
        quote! { ::std::option::Option::Some(#doc) }
    };
    let override_aliases = if cmd.aliases.is_empty() {
        quote! { ::std::option::Option::None }
    } else {
        let aliases = cmd.aliases.iter().map(|a| quote! { #a.to_string() });
        quote! { ::std::option::Option::Some(::std::vec![ #(#aliases),* ]) }
    };

    // The subtree method's params become globals on the graft placeholder so
    // they propagate to every descendant subcommand. A param that repeats a
    // global already inherited from the parent root command is not skipped here:
    // it is emitted as a placeholder global and reconciled (removed when
    // compatible, rejected when conflicting) by `normalize_inherited_globals`.
    let mut opts = Vec::new();
    let mut flags = Vec::new();
    for param in &cmd.params {
        let arg = arg_for(cmd, &param.ident);
        // Subtree-method params only contribute propagating globals, never a tail
        // positional, so tail inference is disabled (`is_last_param = false`).
        match classify(&param.ident, &param.ty, arg, true, false)? {
            Projection::Option(spec) => opts.push(spec),
            Projection::Flag(spec) => flags.push(spec),
            _ => {
                return Err(Error::new(
                    param.ident.span(),
                    "a #[command(subtree = ...)] method parameter must project to a global option or flag",
                ));
            }
        }
    }

    Ok(quote! {
        {
            let __child = #call_path(ctx)?;
            let mut __graft = golem_rust::agentic::graft_subtree(
                __child,
                #expected_name,
                #override_name,
                #override_doc,
                #override_aliases,
                ::std::option::Option::None,
            )?;
            // The subtree method's own params become propagating globals on the
            // graft placeholder (index 0), prepended ahead of any globals the
            // child root already carries. Reconciliation against inherited
            // globals (root + ancestor subtree globals) happens once over the
            // whole tree in `normalize_inherited_globals`.
            {
                let __placeholder = &mut __graft[0];
                let mut __opts: ::std::vec::Vec<golem_rust::agentic::ExtendedOptionSpec> =
                    ::std::vec![ #(#opts),* ];
                let mut __flags: ::std::vec::Vec<golem_rust::agentic::FlagSpec> =
                    ::std::vec![ #(#flags),* ];
                __opts.append(&mut __placeholder.globals.options);
                __placeholder.globals.options = __opts;
                __flags.append(&mut __placeholder.globals.flags);
                __placeholder.globals.flags = __flags;
            }
            let __off = golem_rust::agentic::append_grafted_subtree(&mut commands, __graft);
            commands[0].subcommands.push(__off);
        }
    })
}

/// Rewrites a subtree path `a::b::Child` to `a::b::__golem_tool_descriptor_for_Child`.
fn subtree_call_path(path: &Path) -> Result<Path, Error> {
    let mut rewritten = path.clone();
    let last = rewritten.segments.last_mut().ok_or_else(|| {
        Error::new(
            path.span(),
            "subtree path must name a #[tool_definition] trait",
        )
    })?;
    if !matches!(last.arguments, PathArguments::None) {
        return Err(Error::new(
            path.span(),
            "subtree path must not carry generic arguments",
        ));
    }
    last.ident = descriptor_fn_ident(&last.ident);
    Ok(rewritten)
}

/// Builds an `ExtendedCommandNode` (with empty `subcommands`) for a root
/// implicit-body or leaf command.
fn build_command_node(
    cmd: &CommandIr,
    tool_name: &str,
    is_root: bool,
    inherited_globals: &BTreeSet<String>,
) -> Result<TokenStream, Error> {
    let name = command_name(cmd, tool_name, is_root);
    let doc = doc_tokens(&cmd.doc);
    let aliases = cmd.aliases.iter().map(|a| quote! { #a.to_string() });

    let mut global_options = Vec::new();
    let mut global_flags = Vec::new();
    let mut fixed = Vec::new();
    let mut saw_optional_positional = false;
    let mut tail: Option<TokenStream> = None;
    let mut body_options = Vec::new();
    let mut body_flags = Vec::new();
    let mut stdin: Option<TokenStream> = None;
    let mut stdout: Option<TokenStream> = None;

    // The last *positional-eligible* parameter is the only one eligible to
    // become a tail positional by inference: `Vec<T>` at tail → tail positional,
    // `Vec<T>` elsewhere → repeatable option (§5.8). Globals, options, flags,
    // streams, and de-projected inherited globals are never positionals, so they
    // must not block tail inference for a `Vec<T>` that precedes them.
    let last_value_idx = cmd
        .params
        .iter()
        .enumerate()
        .rev()
        .find_map(|(idx, param)| {
            let arg = arg_for(cmd, &param.ident);
            if is_positional_candidate(param, arg, is_root, inherited_globals) {
                Some(idx)
            } else {
                None
            }
        });

    for (idx, param) in cmd.params.iter().enumerate() {
        let arg = arg_for(cmd, &param.ident);
        // A parameter repeating a global inherited from the root command is not
        // skipped here: it is projected normally and reconciled (removed when
        // compatible, rejected when conflicting) by `normalize_inherited_globals`
        // once the whole tree is assembled. Such a parameter is still excluded
        // from tail-position inference (see `is_positional_candidate`), because a
        // value the runtime will drop must not steal the tail slot from a
        // genuine `Vec<T>` body parameter.
        let is_global = arg.map(|a| a.placement) == Some(Some(ArgPlacement::Global));
        // A parameter repeating a global inherited from the root command is
        // emitted in its natural projected form but will be removed (when
        // compatible) or rejected (when conflicting) by
        // `normalize_inherited_globals`. It therefore does not participate in the
        // body's positional-ordering rules: a value the runtime will drop must
        // not constrain (or be constrained by) the genuine body positionals.
        let is_inherited =
            !is_root && repeats_inherited_global(&param.ident, arg, inherited_globals);
        let is_last_param = Some(idx) == last_value_idx;
        match classify(&param.ident, &param.ty, arg, is_global, is_last_param)? {
            Projection::Stdin(spec) => {
                if stdin.is_some() {
                    return Err(Error::new(
                        param.ident.span(),
                        "duplicate stdin stream parameter",
                    ));
                }
                stdin = Some(spec);
            }
            Projection::Stdout(spec) => {
                if stdout.is_some() {
                    return Err(Error::new(
                        param.ident.span(),
                        "duplicate stdout stream parameter",
                    ));
                }
                stdout = Some(spec);
            }
            Projection::Option(spec) => {
                if is_global {
                    global_options.push(spec);
                } else {
                    body_options.push(spec);
                }
            }
            Projection::Flag(spec) => {
                if is_global {
                    global_flags.push(spec);
                } else {
                    body_flags.push(spec);
                }
            }
            Projection::Positional { tokens, required } => {
                if is_global {
                    return Err(Error::new(
                        param.ident.span(),
                        "a global argument cannot be a positional; use an option or flag",
                    ));
                }
                // Inherited re-declarations are excluded from the ordering rules
                // (they are removed/rejected by normalization); only genuine body
                // positionals constrain ordering.
                if !is_inherited {
                    // Positionals are `fixed*` then an optional `tail`; a fixed
                    // positional cannot follow the variadic tail, or the
                    // descriptor would silently drop it (variadic-only-at-tail is
                    // structural).
                    if tail.is_some() {
                        return Err(Error::new(
                            param.ident.span(),
                            "a fixed positional cannot appear after a tail positional; the tail positional must be the last positional",
                        ));
                    }
                    // Optional positionals must be trailing: once an optional
                    // fixed positional appears, no required one may follow it,
                    // otherwise the boundary between them is ambiguous.
                    if required && saw_optional_positional {
                        return Err(Error::new(
                            param.ident.span(),
                            "a required positional cannot appear after an optional positional; optional positionals must be trailing",
                        ));
                    }
                    if !required {
                        saw_optional_positional = true;
                    }
                }
                fixed.push(tokens);
            }
            Projection::Tail(spec) => {
                if is_global {
                    return Err(Error::new(
                        param.ident.span(),
                        "a global argument cannot be a tail positional",
                    ));
                }
                // An inherited re-declaration explicitly marked as a tail is
                // lowered to a droppable repeatable-list option surrogate (same
                // surface name, same item type, collected value `list<item>`) so
                // the runtime normalization pass can compare its shape against
                // the inherited global and either remove it (compatible) or
                // reject it (`InheritedGlobalConflict`). Storing it as the body's
                // single tail slot would let a re-declaration the runtime will
                // drop steal the slot from a genuine `Vec<T>` body tail.
                if is_inherited {
                    let base = unwrap_generic1(&param.ty, "Option").unwrap_or(&param.ty);
                    let item = unwrap_generic1(base, "Vec").ok_or_else(|| {
                        Error::new(
                            param.ident.span(),
                            "a tail positional must be a `Vec<T>` parameter",
                        )
                    })?;
                    let name = to_kebab_case(&param.ident.to_string());
                    body_options.push(inherited_tail_option_surrogate_tokens(&name, item, arg)?);
                    continue;
                }
                if tail.is_some() {
                    return Err(Error::new(
                        param.ident.span(),
                        "a command may have at most one tail positional",
                    ));
                }
                tail = Some(spec);
            }
        }
    }

    let constraints = build_constraints(cmd)?;
    let (result_spec, errors) = build_result(cmd)?;
    let annotations = build_annotations(cmd.annotations.as_ref());

    let tail_tokens = match tail {
        Some(t) => quote! { ::std::option::Option::Some(#t) },
        None => quote! { ::std::option::Option::None },
    };
    let stdin_tokens = match stdin {
        Some(s) => quote! { ::std::option::Option::Some(#s) },
        None => quote! { ::std::option::Option::None },
    };
    let stdout_tokens = match stdout {
        Some(s) => quote! { ::std::option::Option::Some(#s) },
        None => quote! { ::std::option::Option::None },
    };

    let body = quote! {
        golem_rust::agentic::ExtendedCommandBody {
            positionals: golem_rust::agentic::ExtendedPositionals {
                fixed: ::std::vec![ #(#fixed),* ],
                tail: #tail_tokens,
            },
            options: ::std::vec![ #(#body_options),* ],
            flags: ::std::vec![ #(#body_flags),* ],
            constraints: ::std::vec![ #(#constraints),* ],
            stdin: #stdin_tokens,
            stdout: #stdout_tokens,
            result: #result_spec,
            errors: #errors,
            annotations: #annotations,
        }
    };

    Ok(quote! {
        golem_rust::agentic::ExtendedCommandNode {
            name: #name.to_string(),
            aliases: ::std::vec![ #(#aliases),* ],
            doc: #doc,
            globals: golem_rust::agentic::ExtendedGlobals {
                options: ::std::vec![ #(#global_options),* ],
                flags: ::std::vec![ #(#global_flags),* ],
            },
            subcommands: ::std::vec::Vec::new(),
            body: ::std::option::Option::Some(#body),
        }
    })
}

/// Whether a parameter is eligible to become a positional (fixed or tail), and
/// therefore participates in "last positional → tail" inference. Globals,
/// options, flags, streams, and de-projected inherited globals are excluded
/// because none of them can ever be a positional.
fn is_positional_candidate(
    param: &crate::tool::ir::ParamIr,
    arg: Option<&ArgIr>,
    is_root: bool,
    inherited_globals: &BTreeSet<String>,
) -> bool {
    if is_stream_type(&param.ty) {
        return false;
    }
    if !is_root && repeats_inherited_global(&param.ident, arg, inherited_globals) {
        return false;
    }
    match arg.and_then(|a| a.placement) {
        Some(ArgPlacement::Global | ArgPlacement::Option | ArgPlacement::Flag) => false,
        Some(ArgPlacement::Positional | ArgPlacement::Tail) => true,
        None => {
            // No explicit placement: replicate the type-based inference in
            // `classify`. A flag/count-flag kind, an inferred `bool` flag, or an
            // inferred map option is never a positional; a `Vec<T>` or scalar is.
            if arg.and_then(|a| a.sub_kind).is_some() {
                return false;
            }
            let base = unwrap_generic1(&param.ty, "Option").unwrap_or(&param.ty);
            if type_last_ident(base).as_deref() == Some("bool") {
                return false;
            }
            if is_map_type(base) {
                return false;
            }
            true
        }
    }
}

/// The projected surface form of a parameter.
enum Projection {
    Positional { tokens: TokenStream, required: bool },
    Tail(TokenStream),
    Option(TokenStream),
    Flag(TokenStream),
    Stdin(TokenStream),
    Stdout(TokenStream),
}

/// The concrete command-surface a parameter projects onto, used to validate that
/// every authored placement-structural `#[arg]` field is actually lowered by that
/// surface. Value-schema refinements (text/path/url/numeric) are validated
/// separately against the value type by [`value_graph_tokens`] /
/// `reject_*_refinements`, so they are deliberately not represented here.
#[derive(Clone, Copy)]
enum SurfaceKind {
    Positional,
    Tail,
    OptionScalar,
    OptionList,
    OptionMap,
    BoolFlag,
    CountFlag,
    Stream,
}

/// Which placement-structural `#[arg]` fields a [`SurfaceKind`] lowers. A field
/// set in `#[arg]` but not lowered by the resolved surface is an authoring error
/// (it would be silently dropped), so it is rejected at macro time.
struct AllowedStructuralAttrs {
    short: bool,
    aliases: bool,
    env: bool,
    required: bool,
    negatable: bool,
    optional_scalar: bool,
    repeatable: bool,
    delim: bool,
    default: bool,
    separator: bool,
    verbatim: bool,
    accepts_stdio: bool,
    value_name: bool,
}

impl SurfaceKind {
    /// Human-readable surface name for diagnostics.
    fn describe(self) -> &'static str {
        match self {
            SurfaceKind::Positional => "a positional",
            SurfaceKind::Tail => "a tail positional",
            SurfaceKind::OptionScalar => "a scalar option",
            SurfaceKind::OptionList => "a repeatable list option",
            SurfaceKind::OptionMap => "a map option",
            SurfaceKind::BoolFlag => "a flag",
            SurfaceKind::CountFlag => "a count flag",
            SurfaceKind::Stream => "a stdin/stdout stream",
        }
    }

    fn allowed_structural_attrs(self) -> AllowedStructuralAttrs {
        // Every field defaults to "not lowered"; each surface opts in only to the
        // fields it actually projects (see the corresponding `*_spec_tokens`).
        let none = AllowedStructuralAttrs {
            short: false,
            aliases: false,
            env: false,
            required: false,
            negatable: false,
            optional_scalar: false,
            repeatable: false,
            delim: false,
            default: false,
            separator: false,
            verbatim: false,
            accepts_stdio: false,
            value_name: false,
        };
        match self {
            SurfaceKind::Positional => AllowedStructuralAttrs {
                required: true,
                default: true,
                accepts_stdio: true,
                value_name: true,
                ..none
            },
            SurfaceKind::Tail => AllowedStructuralAttrs {
                separator: true,
                verbatim: true,
                accepts_stdio: true,
                value_name: true,
                ..none
            },
            SurfaceKind::OptionScalar => AllowedStructuralAttrs {
                short: true,
                aliases: true,
                env: true,
                required: true,
                optional_scalar: true,
                default: true,
                value_name: true,
                ..none
            },
            SurfaceKind::OptionList | SurfaceKind::OptionMap => AllowedStructuralAttrs {
                short: true,
                aliases: true,
                env: true,
                required: true,
                repeatable: true,
                delim: true,
                default: true,
                value_name: true,
                ..none
            },
            SurfaceKind::BoolFlag => AllowedStructuralAttrs {
                short: true,
                aliases: true,
                env: true,
                negatable: true,
                default: true,
                ..none
            },
            SurfaceKind::CountFlag => AllowedStructuralAttrs {
                short: true,
                aliases: true,
                env: true,
                ..none
            },
            SurfaceKind::Stream => none,
        }
    }
}

/// Rejects placement-structural `#[arg]` fields the resolved [`SurfaceKind`] does
/// not lower, so an authored field is never silently dropped. Value-schema
/// refinements are validated elsewhere (against the value type) and are not
/// considered here.
fn reject_unconsumed_structural_attrs(arg: &ArgIr, kind: SurfaceKind) -> Result<(), Error> {
    let where_ = kind.describe();
    let span = arg.param.span();
    let allowed = kind.allowed_structural_attrs();
    let check = |is_set: bool, ok: bool, field: &str| -> Result<(), Error> {
        if is_set && !ok {
            Err(Error::new(
                span,
                format!("`{field}` is not valid on {where_}"),
            ))
        } else {
            Ok(())
        }
    };
    check(arg.short.is_some(), allowed.short, "short")?;
    check(!arg.aliases.is_empty(), allowed.aliases, "aliases")?;
    check(arg.env.is_some(), allowed.env, "env")?;
    check(arg.required.is_some(), allowed.required, "required")?;
    check(arg.negatable.is_some(), allowed.negatable, "negatable")?;
    check(
        arg.optional_scalar,
        allowed.optional_scalar,
        "optional_scalar",
    )?;
    check(arg.repeatable.is_some(), allowed.repeatable, "repeatable")?;
    check(arg.delim.is_some(), allowed.delim, "delim")?;
    check(arg.default.is_some(), allowed.default, "default")?;
    check(arg.separator.is_some(), allowed.separator, "separator")?;
    check(arg.verbatim, allowed.verbatim, "verbatim")?;
    check(arg.accepts_stdio, allowed.accepts_stdio, "accepts_stdio")?;
    check(arg.value_name.is_some(), allowed.value_name, "value_name")?;
    Ok(())
}

/// Rejects every `#[arg]` field other than documentation on a stdin/stdout
/// stream parameter. A stream is projected purely from its `InputStream` /
/// `OutputStream` type ([`stream_spec_tokens`] lowers only `doc`), so an explicit
/// placement, `kind`, value-schema refinement, or any structural field would be
/// silently dropped.
fn reject_stream_attrs(arg: &ArgIr) -> Result<(), Error> {
    let span = arg.param.span();
    if arg.placement.is_some() {
        return Err(Error::new(
            span,
            "an explicit placement is not valid on a stdin/stdout stream parameter",
        ));
    }
    if arg.sub_kind.is_some() {
        return Err(Error::new(
            span,
            "`kind = \"flag\"` / `\"count-flag\"` is not valid on a stdin/stdout stream parameter",
        ));
    }
    reject_text_path_url_refinements(arg, "a stdin/stdout stream")?;
    if arg.bounds.is_some() || arg.unit.is_some() || arg.raw_min.is_some() || arg.raw_max.is_some()
    {
        return Err(Error::new(
            span,
            "numeric refinements (`min`/`max`/`bounds`/`unit`) are not valid on a stdin/stdout stream",
        ));
    }
    reject_unconsumed_structural_attrs(arg, SurfaceKind::Stream)
}

/// Projects a single parameter onto its command-surface form, applying explicit
/// `#[arg]` placement and type-based inference.
///
/// A record or enum parameter becomes the value schema of a single CLI surface
/// (an option, positional, or global whose value type is the record/enum). Record
/// fields are never flattened into sibling options, and enum parameters are never
/// expanded into subcommands: the command tree is built only from trait methods
/// and `#[command(subtree = ...)]`, and records/enums flow through the value
/// schema rather than the command grammar.
fn classify(
    ident: &Ident,
    ty: &Type,
    arg: Option<&ArgIr>,
    is_global: bool,
    is_last_param: bool,
) -> Result<Projection, Error> {
    let name = to_kebab_case(&ident.to_string());

    // Streams are not value schemas; detect them by type name.
    if let Some(last) = type_last_ident(ty) {
        if last == "InputStream" {
            if let Some(arg) = arg {
                reject_stream_attrs(arg)?;
            }
            return Ok(Projection::Stdin(stream_spec_tokens(arg)));
        }
        if last == "OutputStream" {
            if let Some(arg) = arg {
                reject_stream_attrs(arg)?;
            }
            return Ok(Projection::Stdout(stream_spec_tokens(arg)));
        }
    }

    // Unwrap a single `Option<T>` layer: it only makes the argument not-required.
    let (base_ty, optional) = match unwrap_generic1(ty, "Option") {
        Some(inner) => (inner, true),
        None => (ty, false),
    };

    let placement = arg.and_then(|a| a.placement);
    let sub_kind = arg.and_then(|a| a.sub_kind);

    // `Global` placement (and the `is_global` subtree-dispatcher path) only marks a
    // parameter as propagating to descendant commands; it does not select a
    // command surface. A global is always an option or a flag — never a positional
    // or tail — so an explicit positional/tail placement on a global is a
    // contradiction (it would be silently turned into an option). Reject it, and
    // otherwise infer the surface (flag vs option) from the type/kind exactly as
    // for a local argument by treating `Global` as "no explicit surface".
    if is_global
        && matches!(
            placement,
            Some(ArgPlacement::Positional | ArgPlacement::Tail)
        )
    {
        return Err(Error::new(
            ident.span(),
            "a global parameter cannot be a positional or tail; globals must be options or flags",
        ));
    }
    let surface = match placement {
        Some(ArgPlacement::Global) => None,
        other => other,
    };

    // An explicit value-carrying placement (option/positional/tail) contradicts a
    // flag kind: a flag has no value schema. Reject rather than letting the kind
    // silently win and discard the authored placement.
    if let (Some(p), Some(k)) = (surface, sub_kind)
        && matches!(
            p,
            ArgPlacement::Option | ArgPlacement::Positional | ArgPlacement::Tail
        )
        && matches!(k, ArgSubKind::Flag | ArgSubKind::CountFlag)
    {
        return Err(Error::new(
            ident.span(),
            "a flag kind (`kind = \"flag\"` / `\"count-flag\"`) cannot be combined with an explicit option/positional/tail placement",
        ));
    }

    let is_bool = type_last_ident(base_ty).as_deref() == Some("bool");
    let vec_item = unwrap_generic1(base_ty, "Vec");
    let map_ty = if is_map_type(base_ty) {
        Some(base_ty)
    } else {
        None
    };

    // Flags (explicit placement, sub-kind, or inferred from `bool`). A global bool
    // with no explicit `kind` still follows the bool→flag rule (`surface` is
    // `None` for a global), so it becomes a global flag rather than a value option.
    let is_flag = matches!(surface, Some(ArgPlacement::Flag))
        || matches!(sub_kind, Some(ArgSubKind::Flag | ArgSubKind::CountFlag))
        || (surface.is_none() && is_bool);
    if is_flag {
        // A flag is always present in the canonical input model (a bool flag is
        // present/absent with a default; a count flag counts occurrences, with
        // absence meaning zero). `FlagShape` carries no optionality, so an
        // `Option<_>` parameter cannot be represented and would silently diverge
        // from the metadata. Reject it rather than dropping the wrapper.
        if optional {
            return Err(Error::new(
                ident.span(),
                "a flag parameter must not be `Option<_>`: flags are always present (a bool flag has a default, a count flag counts occurrences), so optionality has no canonical representation; use the bare type (`bool` / `u32`)",
            ));
        }
        // The projected flag shape must agree with the parameter's Rust type:
        // a count flag carries an integer count (`-vvv` → `u32`), while a bool
        // flag carries a boolean (`--name` / `--no-name`). Anything else would
        // produce metadata that disagrees with the implementation signature.
        if matches!(sub_kind, Some(ArgSubKind::CountFlag)) {
            if type_last_ident(base_ty).as_deref() != Some("u32") {
                return Err(Error::new(
                    ident.span(),
                    "a count flag parameter must be `u32`: count flags are exposed as a `u32` canonical input field, so any other type would make the metadata disagree with the implementation signature",
                ));
            }
        } else if !is_bool {
            return Err(Error::new(
                ident.span(),
                "a flag parameter must be `bool` (or `Option<bool>`); for a count flag use `kind = \"count-flag\"` with an integer type",
            ));
        }
        if let Some(arg) = arg {
            let kind = if matches!(sub_kind, Some(ArgSubKind::CountFlag)) {
                SurfaceKind::CountFlag
            } else {
                SurfaceKind::BoolFlag
            };
            reject_unconsumed_structural_attrs(arg, kind)?;
        }
        return Ok(Projection::Flag(flag_spec_tokens(&name, base_ty, arg)?));
    }

    // Tail positional: explicit `#[arg(... = "tail")]`, or — following the
    // uniform projection rule (§5.8) "`Vec<T>` at tail → tail positional,
    // `Vec<T>` elsewhere → repeatable option" — an unmarked `Vec<T>` that is the
    // last value parameter (streams and inherited globals don't count).
    let infer_tail = surface.is_none() && !is_global && vec_item.is_some() && is_last_param;
    if matches!(surface, Some(ArgPlacement::Tail)) || infer_tail {
        // A tail positional is inherently variadic (zero or more), and
        // `ExtendedTailPositional` (like the WIT `tail-positional`) has no
        // required/optional/default field. An `Option<Vec<T>>` therefore has no
        // way to represent its `None` state and the `Option` wrapper would be
        // silently dropped, making the metadata disagree with the implementation
        // signature. Reject it; a bare `Vec<T>` already encodes "none supplied" as
        // an empty tail. (Same representability rule as `Option<_>` flags.)
        if optional {
            return Err(Error::new(
                ident.span(),
                "a tail positional must not be `Option<_>`: a tail positional is already variadic (zero or more) and has no representation for an additional optional/absent state, so the `Option` wrapper would be silently dropped; use `Vec<T>` (an empty tail already means none supplied)",
            ));
        }
        let item = vec_item.ok_or_else(|| {
            Error::new(
                ident.span(),
                "a tail positional must be a `Vec<T>` parameter",
            )
        })?;
        if let Some(arg) = arg {
            reject_unconsumed_structural_attrs(arg, SurfaceKind::Tail)?;
        }
        return Ok(Projection::Tail(tail_tokens(&name, item, arg)?));
    }

    // Options: explicit placement, any non-flag global (globals can only be
    // options or flags, never positionals), or inferred collection types.
    let is_option = matches!(surface, Some(ArgPlacement::Option))
        || is_global
        || (surface.is_none() && (vec_item.is_some() || map_ty.is_some()));
    if is_option {
        if let Some(arg) = arg {
            let kind = if vec_item.is_some() {
                SurfaceKind::OptionList
            } else if map_ty.is_some() {
                SurfaceKind::OptionMap
            } else {
                SurfaceKind::OptionScalar
            };
            reject_unconsumed_structural_attrs(arg, kind)?;
        }
        let spec = option_spec_tokens(&name, base_ty, vec_item, map_ty, optional, arg)?;
        return Ok(Projection::Option(spec));
    }

    // Otherwise a fixed positional. Required by default; `Option<T>` or
    // `required = false` makes it optional (must match `positional_tokens`).
    if let Some(arg) = arg {
        reject_unconsumed_structural_attrs(arg, SurfaceKind::Positional)?;
    }
    let required = !optional && arg.and_then(|a| a.required).unwrap_or(true);
    Ok(Projection::Positional {
        tokens: positional_tokens(&name, base_ty, optional, arg)?,
        required,
    })
}

fn stream_spec_tokens(arg: Option<&ArgIr>) -> TokenStream {
    let doc = arg_doc_tokens(arg);
    quote! {
        golem_rust::agentic::StreamSpec {
            doc: #doc,
            mime: ::std::vec::Vec::new(),
            required: true,
        }
    }
}

fn flag_spec_tokens(name: &str, base_ty: &Type, arg: Option<&ArgIr>) -> Result<TokenStream, Error> {
    let doc = arg_doc_tokens(arg);
    let short = opt_char(arg.and_then(|a| a.short));
    let aliases = alias_tokens(arg);
    let env_var = opt_str(arg.and_then(|a| a.env.as_ref()));

    let is_count = matches!(arg.and_then(|a| a.sub_kind), Some(ArgSubKind::CountFlag));
    if let Some(arg) = arg {
        reject_flag_value_refinements(arg, is_count)?;
    }

    let shape = if is_count {
        let max = match arg.and_then(|a| a.raw_max.as_ref()) {
            Some(expr) => quote! { ::std::option::Option::Some({ let __m: u32 = #expr; __m }) },
            None => quote! { ::std::option::Option::None },
        };
        quote! { golem_rust::golem_agentic::golem::tool::common::FlagShape::CountFlag(#max) }
    } else {
        let default = match arg.and_then(|a| a.default.as_ref()) {
            Some(expr) => bool_default(expr)?,
            None => quote! { false },
        };
        let negatable = arg.and_then(|a| a.negatable).unwrap_or(false);
        quote! {
            golem_rust::golem_agentic::golem::tool::common::FlagShape::BoolFlag(
                golem_rust::golem_agentic::golem::tool::common::BoolFlagShape {
                    default: #default,
                    negatable: #negatable,
                }
            )
        }
    };

    // `bool`/count types carry no author value schema; nothing to build from base_ty.
    let _ = base_ty;
    Ok(quote! {
        golem_rust::agentic::FlagSpec {
            long: #name.to_string(),
            short: #short,
            aliases: #aliases,
            doc: #doc,
            shape: #shape,
            env_var: #env_var,
        }
    })
}

/// Rejects text/path/url refinement keys, which target a leaf scalar value
/// schema and so cannot apply to `context`. A flag has no author value schema; a
/// map option's value graph is the map container, not its leaf entries. Applying
/// them anyway would silently drop the authored refinement.
fn reject_text_path_url_refinements(arg: &ArgIr, context: &str) -> Result<(), Error> {
    let span = arg.param.span();
    if arg.regex.is_some() || arg.min_length.is_some() || arg.max_length.is_some() {
        return Err(Error::new(
            span,
            format!(
                "text refinements (`regex`/`min_length`/`max_length`) are not valid on {context}"
            ),
        ));
    }
    if arg.path_kind.is_some() || arg.direction.is_some() || arg.mime.is_some() {
        return Err(Error::new(
            span,
            format!("path refinements (`kind`/`direction`/`mime`) are not valid on {context}"),
        ));
    }
    if arg.schemes.is_some() {
        return Err(Error::new(
            span,
            format!("url refinements (`schemes`) are not valid on {context}"),
        ));
    }
    Ok(())
}

/// Rejects `#[arg]` value-schema refinements on a flag. A flag carries no author
/// value schema (a bool flag is present/absent; a count flag is an occurrence
/// count), so text/path/url/numeric refinements would be silently dropped and
/// produce metadata that disagrees with the authored `#[arg]`. A count flag's
/// `max` (the count cap) is the only refinement it accepts; `default` and
/// `negatable` belong to bool flags only.
fn reject_flag_value_refinements(arg: &ArgIr, is_count: bool) -> Result<(), Error> {
    reject_text_path_url_refinements(arg, "a flag")?;
    let span = arg.param.span();
    if arg.bounds.is_some() || arg.unit.is_some() {
        return Err(Error::new(
            span,
            "numeric refinements (`bounds`/`unit`) are not valid on a flag",
        ));
    }
    if arg.raw_min.is_some() {
        return Err(Error::new(span, "`min` is not valid on a flag"));
    }
    if arg.raw_max.is_some() && !is_count {
        return Err(Error::new(
            span,
            "`max` is only valid on a count flag (`kind = \"count-flag\"`)",
        ));
    }
    // `default` / `negatable` placement validity (allowed on bool flags, rejected
    // on count flags) is enforced by `reject_unconsumed_structural_attrs`.
    Ok(())
}

/// Rejects `#[arg]` value-schema refinements on a map option. A map's value
/// graph is the map container; the refinement keys target leaf scalars and have
/// no map-entry syntax, so they would be silently dropped.
fn reject_map_value_refinements(arg: &ArgIr) -> Result<(), Error> {
    reject_text_path_url_refinements(arg, "a map option")?;
    if arg.raw_min.is_some() || arg.raw_max.is_some() || arg.bounds.is_some() || arg.unit.is_some()
    {
        return Err(Error::new(
            arg.param.span(),
            "numeric refinements (`min`/`max`/`bounds`/`unit`) are not valid on a map option",
        ));
    }
    Ok(())
}

fn bool_default(expr: &Expr) -> Result<TokenStream, Error> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Bool(b),
            ..
        }) => {
            let v = b.value;
            Ok(quote! { #v })
        }
        // Peel parentheses/groups so a flag default accepts the same literal forms
        // as the general metadata-literal grammar (`tool_literal_tokens`), e.g.
        // `default = (true)`.
        Expr::Paren(p) => bool_default(&p.expr),
        Expr::Group(g) => bool_default(&g.expr),
        other => Err(Error::new(
            other.span(),
            "a flag default must be a boolean literal (`true` or `false`)",
        )),
    }
}

fn option_spec_tokens(
    name: &str,
    base_ty: &Type,
    vec_item: Option<&Type>,
    map_ty: Option<&Type>,
    optional: bool,
    arg: Option<&ArgIr>,
) -> Result<TokenStream, Error> {
    let doc = arg_doc_tokens(arg);
    let short = opt_char(arg.and_then(|a| a.short));
    let aliases = alias_tokens(arg);
    let value_name = opt_str(arg.and_then(|a| a.value_name.as_ref()));
    let env_var = opt_str(arg.and_then(|a| a.env.as_ref()));

    let position = format!("option --{name}");
    let shape = if let Some(item) = vec_item {
        let graph = value_graph_tokens(item, arg, MinMaxRole::Bound, &position)?;
        let rep = repetition_tokens(arg)?;
        quote! {
            golem_rust::agentic::ExtendedOptionShape::RepeatableList(
                golem_rust::agentic::ExtendedRepeatableListShape {
                    repetition: #rep,
                    item_type: #graph,
                }
            )
        }
    } else if let Some(map) = map_ty {
        if let Some(arg) = arg {
            reject_map_value_refinements(arg)?;
        }
        let graph = value_graph_tokens(map, arg, MinMaxRole::Forbidden, &position)?;
        let rep = repetition_tokens(arg)?;
        quote! {
            golem_rust::agentic::ExtendedOptionShape::RepeatableMap(
                golem_rust::agentic::ExtendedRepeatableMapShape {
                    repetition: #rep,
                    map_type: #graph,
                    duplicate_key_policy:
                        golem_rust::golem_agentic::golem::tool::common::DuplicateKeyPolicy::Reject,
                }
            )
        }
    } else {
        let graph = value_graph_tokens(base_ty, arg, MinMaxRole::Bound, &position)?;
        if arg.map(|a| a.optional_scalar).unwrap_or(false) {
            quote! { golem_rust::agentic::ExtendedOptionShape::OptionalScalar(#graph) }
        } else {
            quote! { golem_rust::agentic::ExtendedOptionShape::Scalar(#graph) }
        }
    };

    // An `Option<T>` option is never required; otherwise honor `required`.
    let required = !optional && arg.and_then(|a| a.required).unwrap_or(false);

    let default = match arg.and_then(|a| a.default.as_ref()) {
        Some(expr) => {
            let lit = tool_literal_tokens(expr)?;
            quote! {
                ::std::option::Option::Some(golem_rust::agentic::literal_to_schema_value(
                    &golem_rust::agentic::option_collected_graph(&__shape),
                    &#lit,
                )?)
            }
        }
        None => quote! { ::std::option::Option::None },
    };

    Ok(quote! {
        {
            let __shape = #shape;
            let __default = #default;
            golem_rust::agentic::ExtendedOptionSpec {
                long: #name.to_string(),
                short: #short,
                aliases: #aliases,
                doc: #doc,
                value_name: #value_name,
                shape: __shape,
                default: __default,
                required: #required,
                env_var: #env_var,
            }
        }
    })
}

fn positional_tokens(
    name: &str,
    base_ty: &Type,
    optional: bool,
    arg: Option<&ArgIr>,
) -> Result<TokenStream, Error> {
    let doc = arg_doc_tokens(arg);
    let value_name = opt_str(arg.and_then(|a| a.value_name.as_ref()));
    let graph = value_graph_tokens(
        base_ty,
        arg,
        MinMaxRole::Bound,
        &format!("positional {name}"),
    )?;
    let accepts_stdio = arg.map(|a| a.accepts_stdio).unwrap_or(false);
    // Positionals are required by default; `Option<T>` or `required = false`
    // makes them optional.
    let required = !optional && arg.and_then(|a| a.required).unwrap_or(true);

    let default = match arg.and_then(|a| a.default.as_ref()) {
        Some(expr) => {
            let lit = tool_literal_tokens(expr)?;
            quote! {
                ::std::option::Option::Some(golem_rust::agentic::literal_to_schema_value(
                    &__type, &#lit,
                )?)
            }
        }
        None => quote! { ::std::option::Option::None },
    };

    Ok(quote! {
        {
            let __type = #graph;
            let __default = #default;
            golem_rust::agentic::ExtendedPositional {
                name: #name.to_string(),
                doc: #doc,
                value_name: #value_name,
                type_: __type,
                default: __default,
                required: #required,
                accepts_stdio: #accepts_stdio,
            }
        }
    })
}

fn tail_tokens(name: &str, item: &Type, arg: Option<&ArgIr>) -> Result<TokenStream, Error> {
    // `ExtendedTailPositional` (and the WIT `tail-positional` record) has no
    // default field: a variadic tail has no single default value. An authored
    // `default` is rejected by `reject_unconsumed_structural_attrs` before this
    // point rather than silently dropped.
    let doc = arg_doc_tokens(arg);
    let value_name = opt_str(arg.and_then(|a| a.value_name.as_ref()));
    // A tail's `min`/`max` bound the *occurrence count* (how many items), not a
    // numeric range on each item, so the item's value graph must not consume them
    // as numeric bounds (`MinMaxRole::Occurrence`). Per-item `bounds`/`unit`
    // refinements still apply to the item type.
    let graph = value_graph_tokens(
        item,
        arg,
        MinMaxRole::Occurrence,
        &format!("tail positional {name}"),
    )?;
    let min = match arg.and_then(|a| a.raw_min.as_ref()) {
        Some(expr) => quote! { { let __m: u32 = #expr; __m } },
        None => quote! { 0u32 },
    };
    let max = match arg.and_then(|a| a.raw_max.as_ref()) {
        Some(expr) => quote! { ::std::option::Option::Some({ let __m: u32 = #expr; __m }) },
        None => quote! { ::std::option::Option::None },
    };
    let separator = opt_str(arg.and_then(|a| a.separator.as_ref()));
    let verbatim = arg.map(|a| a.verbatim).unwrap_or(false);
    let accepts_stdio = arg.map(|a| a.accepts_stdio).unwrap_or(false);

    Ok(quote! {
        golem_rust::agentic::ExtendedTailPositional {
            name: #name.to_string(),
            doc: #doc,
            value_name: #value_name,
            item_type: #graph,
            min: #min,
            max: #max,
            separator: #separator,
            verbatim: #verbatim,
            accepts_stdio: #accepts_stdio,
        }
    })
}

/// Lowers an inherited re-declaration that is explicitly marked as a tail
/// positional to a repeatable-list option surrogate. The surrogate keeps the
/// parameter's surface name and item type (so its collected value is
/// `list<item>`, matching a tail's collected value), letting the runtime
/// normalization pass compare it against the inherited global and either drop it
/// (compatible) or reject it (`InheritedGlobalConflict`). It is never the body's
/// structural tail slot, so a genuine `Vec<T>` body tail is not displaced. A
/// tail's `min`/`max` bound the occurrence count rather than the item, so the
/// item graph is built with `MinMaxRole::Occurrence`.
fn inherited_tail_option_surrogate_tokens(
    name: &str,
    item: &Type,
    arg: Option<&ArgIr>,
) -> Result<TokenStream, Error> {
    let doc = arg_doc_tokens(arg);
    let aliases = alias_tokens(arg);
    let value_name = opt_str(arg.and_then(|a| a.value_name.as_ref()));
    let graph = value_graph_tokens(
        item,
        arg,
        MinMaxRole::Occurrence,
        &format!("inherited tail positional {name}"),
    )?;
    Ok(quote! {
        golem_rust::agentic::ExtendedOptionSpec {
            long: #name.to_string(),
            short: ::std::option::Option::None,
            aliases: #aliases,
            doc: #doc,
            value_name: #value_name,
            shape: golem_rust::agentic::ExtendedOptionShape::RepeatableList(
                golem_rust::agentic::ExtendedRepeatableListShape {
                    repetition:
                        golem_rust::golem_agentic::golem::tool::common::Repetition::Repeated,
                    item_type: #graph,
                }
            ),
            default: ::std::option::Option::None,
            required: false,
            env_var: ::std::option::Option::None,
        }
    })
}

fn repetition_tokens(arg: Option<&ArgIr>) -> Result<TokenStream, Error> {
    let mode = arg
        .and_then(|a| a.repeatable)
        .unwrap_or(RepeatableMode::Repeated);
    let delim = arg.and_then(|a| a.delim);
    match mode {
        RepeatableMode::Repeated => {
            // `Repeated` carries no delimiter; a `delim` set here would be
            // silently dropped, so reject it rather than ignore it.
            if delim.is_some() {
                return Err(Error::new(
                    arg.map(|a| a.param.span())
                        .unwrap_or_else(proc_macro2::Span::call_site),
                    "`delim` requires `repeatable = \"delimited\"` or `repeatable = \"either\"`",
                ));
            }
            Ok(quote! { golem_rust::golem_agentic::golem::tool::common::Repetition::Repeated })
        }
        RepeatableMode::Delimited => {
            let d = delim.ok_or_else(|| {
                Error::new(
                    proc_macro2::Span::call_site(),
                    "repeatable = \"delimited\" requires a `delim = '<char>'`",
                )
            })?;
            Ok(quote! { golem_rust::golem_agentic::golem::tool::common::Repetition::Delimited(#d) })
        }
        RepeatableMode::Either => {
            let d = delim.ok_or_else(|| {
                Error::new(
                    proc_macro2::Span::call_site(),
                    "repeatable = \"either\" requires a `delim = '<char>'`",
                )
            })?;
            Ok(quote! { golem_rust::golem_agentic::golem::tool::common::Repetition::Either(#d) })
        }
    }
}

/// How a value graph interprets the `#[arg]` `min`/`max` keys, which are
/// overloaded: on most slots they are numeric bounds on the value, but on a tail
/// they bound the occurrence count (handled by the caller). `bounds`/`unit`
/// refine the value's numeric schema for `Bound`/`Occurrence`; `Forbidden` skips
/// all numeric refinements.
#[derive(Clone, Copy)]
enum MinMaxRole {
    /// Scalars, fixed positionals, and list/scalar option items: `min`/`max`
    /// (and `bounds`/`unit`) refine the value's numeric schema.
    Bound,
    /// Tail items: `min`/`max` bound the occurrence count (consumed by the
    /// caller); only `bounds`/`unit` refine the item's numeric schema.
    Occurrence,
    /// Contexts where numeric refinements are intentionally not applied: a
    /// `value_is` comparand (numeric bounds don't change the value variant) and a
    /// map option (whose author-facing numeric keys are rejected up front by
    /// `reject_map_value_refinements`).
    Forbidden,
}

/// Builds a `SchemaGraph` expression for `inner_ty`, applying the `#[arg]`
/// refinements (text/path/url/numeric) that match the attribute keys present.
fn value_graph_tokens(
    inner_ty: &Type,
    arg: Option<&ArgIr>,
    min_max: MinMaxRole,
    position: &str,
) -> Result<TokenStream, Error> {
    let base = quote! {
        golem_rust::agentic::tool_value_schema::<#inner_ty>(#position)?
    };
    let Some(arg) = arg else {
        return Ok(base);
    };

    let mut steps = Vec::new();

    if arg.regex.is_some() || arg.min_length.is_some() || arg.max_length.is_some() {
        // Text refinements apply to a string-backed schema. A recognized
        // non-text type would be coerced to a `Text` schema, producing metadata
        // that disagrees with the implementation signature.
        if is_known_non_text(inner_ty) {
            return Err(Error::new(
                arg.param.span(),
                "text refinements (`regex`/`min_length`/`max_length`) require a text-typed parameter",
            ));
        }
        let regex = opt_str(arg.regex.as_ref());
        let min_len = opt_u32(arg.min_length);
        let max_len = opt_u32(arg.max_length);
        steps.push(quote! {
            __g.root = golem_rust::agentic::refine_text(__g.root, #regex, #min_len, #max_len)?;
        });
    }
    if arg.path_kind.is_some() || arg.direction.is_some() || arg.mime.is_some() {
        // Path refinements apply to a path-backed schema. A recognized non-path
        // type would be coerced to a `Path` schema, producing metadata that
        // disagrees with the implementation signature.
        if is_known_non_path(inner_ty) {
            return Err(Error::new(
                arg.param.span(),
                "path refinements (`kind`/`direction`/`mime`) require a path-typed parameter",
            ));
        }
        let direction = opt_direction(arg.direction);
        let kind = opt_path_kind(arg.path_kind);
        let mime = opt_str_vec(arg.mime.as_ref());
        steps.push(quote! {
            __g.root = golem_rust::agentic::refine_path(__g.root, #direction, #kind, #mime)?;
        });
    }
    if arg.schemes.is_some() {
        // Url refinements apply to a url-backed schema. A recognized non-url
        // type would be coerced to a `Url` schema, producing metadata that
        // disagrees with the implementation signature.
        if is_known_non_url(inner_ty) {
            return Err(Error::new(
                arg.param.span(),
                "url refinements (`schemes`) require a url-typed parameter",
            ));
        }
        let schemes = opt_str_vec(arg.schemes.as_ref());
        steps.push(quote! {
            __g.root = golem_rust::agentic::refine_url(__g.root, #schemes)?;
        });
    }
    // `bounds`/`unit` always refine the value's numeric schema. `min`/`max`
    // refine it only when this slot interprets them as numeric bounds; on a tail
    // they bound the occurrence count (consumed by the caller) and on a map value
    // no numeric refinement applies at all.
    let (min_max_are_bounds, numeric_allowed) = match min_max {
        MinMaxRole::Bound => (true, true),
        MinMaxRole::Occurrence => (false, true),
        MinMaxRole::Forbidden => (false, false),
    };
    let slot_min = min_max_are_bounds.then_some(arg.raw_min.as_ref()).flatten();
    let slot_max = min_max_are_bounds.then_some(arg.raw_max.as_ref()).flatten();
    if numeric_allowed
        && (arg.bounds.is_some() || arg.unit.is_some() || slot_min.is_some() || slot_max.is_some())
    {
        // Numeric refinements apply to a numeric schema. A recognized
        // non-numeric type would have its restrictions silently dropped by
        // `refine_numeric`, producing metadata that disagrees with the authored
        // `#[arg]`.
        if is_known_non_numeric(inner_ty) {
            return Err(Error::new(
                arg.param.span(),
                "numeric refinements (`min`/`max`/`bounds`/`unit`) require a numeric parameter",
            ));
        }
        if arg.bounds.is_some() && (slot_min.is_some() || slot_max.is_some()) {
            return Err(Error::new(
                arg.param.span(),
                "use either `bounds = (min, max)` or `min`/`max`, not both",
            ));
        }
        let (min, max) = if let Some((lo, hi)) = &arg.bounds {
            let lo_b = numeric_bound(inner_ty, lo);
            let hi_b = numeric_bound(inner_ty, hi);
            (
                quote! { ::std::option::Option::Some(#lo_b) },
                quote! { ::std::option::Option::Some(#hi_b) },
            )
        } else {
            let min = match slot_min {
                Some(e) => {
                    let b = numeric_bound(inner_ty, e);
                    quote! { ::std::option::Option::Some(#b) }
                }
                None => quote! { ::std::option::Option::None },
            };
            let max = match slot_max {
                Some(e) => {
                    let b = numeric_bound(inner_ty, e);
                    quote! { ::std::option::Option::Some(#b) }
                }
                None => quote! { ::std::option::Option::None },
            };
            (min, max)
        };
        let unit = opt_str(arg.unit.as_ref());
        steps.push(quote! {
            __g.root = golem_rust::agentic::refine_numeric(__g.root, #min, #max, #unit)?;
        });
    }

    if steps.is_empty() {
        Ok(base)
    } else {
        Ok(quote! {
            {
                let mut __g = #base;
                #(#steps)*
                __g
            }
        })
    }
}

fn numeric_bound(inner_ty: &Type, expr: &Expr) -> TokenStream {
    quote! {
        {
            let __v: #inner_ty = #expr;
            golem_rust::agentic::IntoNumericBound::into_numeric_bound(__v)?
        }
    }
}

fn build_constraints(cmd: &CommandIr) -> Result<Vec<TokenStream>, Error> {
    cmd.constraints.iter().map(constraint_tokens).collect()
}

fn constraint_tokens(c: &ConstraintIr) -> Result<TokenStream, Error> {
    Ok(match c {
        ConstraintIr::RequiresAll(refs) => {
            let r = refs_tokens(refs)?;
            quote! { golem_rust::agentic::ExtendedConstraint::RequiresAll(#r) }
        }
        ConstraintIr::AllOrNone(refs) => {
            let r = refs_tokens(refs)?;
            quote! { golem_rust::agentic::ExtendedConstraint::AllOrNone(#r) }
        }
        ConstraintIr::RequiresAny(refs) => {
            let r = refs_tokens(refs)?;
            quote! { golem_rust::agentic::ExtendedConstraint::RequiresAny(#r) }
        }
        ConstraintIr::MutexGroups(groups) => {
            let gs = groups
                .iter()
                .map(|g| {
                    let r = refs_tokens(g)?;
                    Ok(quote! { golem_rust::agentic::ExtendedRefGroup { refs: #r } })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            quote! { golem_rust::agentic::ExtendedConstraint::MutexGroups(::std::vec![ #(#gs),* ]) }
        }
        ConstraintIr::Implies {
            lhs_quant,
            lhs,
            rhs_quant,
            rhs,
        } => {
            let lq = quantifier_tokens(*lhs_quant);
            let l = refs_tokens(lhs)?;
            let rq = quantifier_tokens(*rhs_quant);
            let r = refs_tokens(rhs)?;
            quote! {
                golem_rust::agentic::ExtendedConstraint::Implies(golem_rust::agentic::ExtendedImpliesC {
                    lhs_quant: #lq,
                    lhs: #l,
                    rhs_quant: #rq,
                    rhs: #r,
                })
            }
        }
        ConstraintIr::Forbids {
            lhs_quant,
            lhs,
            rhs,
        } => {
            let lq = quantifier_tokens(*lhs_quant);
            let l = refs_tokens(lhs)?;
            let r = refs_tokens(rhs)?;
            quote! {
                golem_rust::agentic::ExtendedConstraint::Forbids(golem_rust::agentic::ExtendedForbidsC {
                    lhs_quant: #lq,
                    lhs: #l,
                    rhs: #r,
                })
            }
        }
    })
}

fn refs_tokens(refs: &[RefIr]) -> Result<TokenStream, Error> {
    let items = refs
        .iter()
        .map(ref_tokens)
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(quote! { ::std::vec![ #(#items),* ] })
}

fn ref_tokens(r: &RefIr) -> Result<TokenStream, Error> {
    match r {
        RefIr::Present(name) => {
            Ok(quote! { golem_rust::agentic::ExtendedRef::Present(#name.to_string()) })
        }
        // A `value-is` literal is always carried as a raw, un-typed literal and
        // resolved + type-checked at composition time against the effective
        // constraint scope (`resolve_deferred_value_is`), which is the single
        // source of truth shared with validation. The macro never re-derives the
        // comparand graph: doing so duplicated the runtime's option/list/map/tail
        // and refinement-placement rules and drifted from them. Resolution still
        // runs inside the generated descriptor fn (via `normalize_inherited_globals`),
        // so a literal that is incompatible with a *locally* known argument is
        // still reported when the descriptor is built; a name supplied only by an
        // ancestor subtree method is resolved once that global is in scope.
        RefIr::ValueIs { name, value } => {
            let lit = tool_literal_tokens(value)?;
            Ok(quote! {
                golem_rust::agentic::ExtendedRef::ValueIs(golem_rust::agentic::ExtendedValueIsRef {
                    name: #name.to_string(),
                    value: golem_rust::agentic::ExtendedValueIsLiteral::Deferred(#lit),
                })
            })
        }
    }
}

fn quantifier_tokens(q: QuantifierIr) -> TokenStream {
    match q {
        QuantifierIr::All => {
            quote! { golem_rust::golem_agentic::golem::tool::common::Quantifier::All }
        }
        QuantifierIr::Any => {
            quote! { golem_rust::golem_agentic::golem::tool::common::Quantifier::Any }
        }
    }
}

/// Builds the `(result_spec, errors)` tokens from the method return type.
fn build_result(cmd: &CommandIr) -> Result<(TokenStream, TokenStream), Error> {
    let (ok_ty, err_ty) = split_result(&cmd.output);

    let errors = match err_ty {
        Some(e) => quote! { <#e as golem_rust::agentic::ToolErrorSchema>::error_cases()? },
        None => quote! { ::std::vec::Vec::new() },
    };

    let result_spec = match ok_ty {
        Some(t) => {
            let graph = quote! {
                golem_rust::agentic::tool_value_schema::<#t>("result")?
            };
            let (formatters, default_formatter) = build_formatters(cmd.result.as_ref());
            let empty_doc = doc_tokens(&DocIr::default());
            quote! {
                ::std::option::Option::Some(golem_rust::agentic::ExtendedResultSpec {
                    type_: #graph,
                    doc: #empty_doc,
                    formatters: #formatters,
                    default_formatter: #default_formatter,
                })
            }
        }
        None => {
            // A unit `()` success carries no result value, so there is no result
            // slot for formatters to render. An explicit `#[result(...)]` would be
            // silently dropped; reject it instead.
            if cmd.result.is_some() {
                return Err(Error::new(
                    cmd.method_ident.span(),
                    "#[result(...)] is not valid on a method with a unit `()` success type: \
                     there is no result value to format",
                ));
            }
            quote! { ::std::option::Option::None }
        }
    };

    Ok((result_spec, errors))
}

/// Builds `(formatters, default_formatter)`. A result with no `#[result]`
/// formatters gets a synthesized single `default` formatter so it always
/// resolves.
fn build_formatters(result: Option<&ResultIr>) -> (TokenStream, TokenStream) {
    let formatters: Vec<String> = result.map(|r| r.formatters.clone()).unwrap_or_default();
    let explicit_default = result.and_then(|r| r.default_formatter.clone());

    if formatters.is_empty() {
        let empty_doc = doc_tokens(&DocIr::default());
        let f = quote! {
            ::std::vec![ golem_rust::agentic::ToolFormatter {
                name: "default".to_string(),
                doc: #empty_doc,
            } ]
        };
        let d = explicit_default.unwrap_or_else(|| "default".to_string());
        return (f, quote! { #d.to_string() });
    }

    let default = explicit_default.unwrap_or_else(|| formatters[0].clone());
    let items = formatters.iter().map(|name| {
        let empty_doc = doc_tokens(&DocIr::default());
        quote! {
            golem_rust::agentic::ToolFormatter {
                name: #name.to_string(),
                doc: #empty_doc,
            }
        }
    });
    (
        quote! { ::std::vec![ #(#items),* ] },
        quote! { #default.to_string() },
    )
}

fn build_annotations(ann: Option<&CommandAnnotationsIr>) -> TokenStream {
    match ann {
        None => quote! { ::std::option::Option::None },
        Some(a) => {
            // MCP defaults for unspecified fields.
            let read_only = a.read_only.unwrap_or(false);
            let destructive = a.destructive.unwrap_or(true);
            let idempotent = a.idempotent.unwrap_or(false);
            let open_world = a.open_world.unwrap_or(true);
            quote! {
                ::std::option::Option::Some(golem_rust::agentic::CommandAnnotations {
                    read_only: #read_only,
                    destructive: #destructive,
                    idempotent: #idempotent,
                    open_world: #open_world,
                })
            }
        }
    }
}

// --- literal lowering -------------------------------------------------------

/// Lowers a metadata literal expression to a `golem_rust::agentic::ToolLiteral`.
fn tool_literal_tokens(expr: &Expr) -> Result<TokenStream, Error> {
    match expr {
        Expr::Lit(syn::ExprLit { lit, .. }) => lit_tokens(lit, expr.span(), false),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => {
            if let Expr::Lit(syn::ExprLit { lit, .. }) = &*u.expr {
                lit_tokens(lit, expr.span(), true)
            } else {
                Err(Error::new(expr.span(), "unsupported negated literal"))
            }
        }
        Expr::Group(g) => tool_literal_tokens(&g.expr),
        Expr::Paren(p) => tool_literal_tokens(&p.expr),
        Expr::Array(a) => array_tokens(&a.elems, expr.span()),
        other => Err(Error::new(
            other.span(),
            "unsupported metadata literal for a default / value_is",
        )),
    }
}

fn lit_tokens(lit: &syn::Lit, span: proc_macro2::Span, negate: bool) -> Result<TokenStream, Error> {
    match lit {
        syn::Lit::Bool(b) => {
            let v = b.value;
            Ok(quote! { golem_rust::agentic::ToolLiteral::Bool(#v) })
        }
        syn::Lit::Str(s) => {
            let v = s.value();
            Ok(quote! { golem_rust::agentic::ToolLiteral::Str(#v.to_string()) })
        }
        syn::Lit::Char(c) => {
            let v = c.value();
            Ok(quote! { golem_rust::agentic::ToolLiteral::Char(#v) })
        }
        syn::Lit::Int(i) => {
            let value: i128 = i
                .base10_parse::<i128>()
                .map_err(|e| Error::new(span, e.to_string()))?;
            let value = if negate { -value } else { value };
            Ok(quote! { golem_rust::agentic::ToolLiteral::Int(#value) })
        }
        syn::Lit::Float(f) => {
            let value: f64 = f
                .base10_parse::<f64>()
                .map_err(|e| Error::new(span, e.to_string()))?;
            let value = if negate { -value } else { value };
            Ok(quote! { golem_rust::agentic::ToolLiteral::Float(#value) })
        }
        other => Err(Error::new(span, format!("unsupported literal {other:?}"))),
    }
}

fn array_tokens(
    elems: &syn::punctuated::Punctuated<Expr, syn::Token![,]>,
    span: proc_macro2::Span,
) -> Result<TokenStream, Error> {
    // A non-empty array whose elements are all 2-tuples is a map literal;
    // otherwise it is a list literal. This is a syntactic heuristic: a default
    // for a `Vec<(A, B)>` (list-of-pairs) cannot be expressed via an array
    // literal, but tuple/record element schemas are not interpretable as default
    // values anyway (`literal_to_schema_value` rejects them), so the only
    // array-of-pairs target reachable today is a `Map`, which this matches.
    let all_pairs = !elems.is_empty()
        && elems
            .iter()
            .all(|e| matches!(e, Expr::Tuple(t) if t.elems.len() == 2));
    if all_pairs {
        let entries = elems
            .iter()
            .map(|e| {
                let Expr::Tuple(t) = e else { unreachable!() };
                let mut it = t.elems.iter();
                let k = tool_literal_tokens(it.next().unwrap())?;
                let v = tool_literal_tokens(it.next().unwrap())?;
                Ok(quote! { (#k, #v) })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(quote! { golem_rust::agentic::ToolLiteral::Map(::std::vec![ #(#entries),* ]) })
    } else {
        let items = elems
            .iter()
            .map(tool_literal_tokens)
            .collect::<Result<Vec<_>, Error>>()?;
        let _ = span;
        Ok(quote! { golem_rust::agentic::ToolLiteral::List(::std::vec![ #(#items),* ]) })
    }
}

// --- small token helpers ----------------------------------------------------

fn arg_doc_tokens(arg: Option<&ArgIr>) -> TokenStream {
    let summary = arg.and_then(|a| a.doc.clone()).unwrap_or_default();
    let doc = DocIr {
        summary,
        description: String::new(),
        examples: Vec::new(),
    };
    doc_tokens(&doc)
}

fn alias_tokens(arg: Option<&ArgIr>) -> TokenStream {
    let aliases = arg
        .map(|a| a.aliases.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|a| quote! { #a.to_string() });
    quote! { ::std::vec![ #(#aliases),* ] }
}

fn opt_char(c: Option<char>) -> TokenStream {
    match c {
        Some(c) => quote! { ::std::option::Option::Some(#c) },
        None => quote! { ::std::option::Option::None },
    }
}

fn opt_str(s: Option<&String>) -> TokenStream {
    match s {
        Some(s) => quote! { ::std::option::Option::Some(#s.to_string()) },
        None => quote! { ::std::option::Option::None },
    }
}

fn opt_u32(n: Option<u32>) -> TokenStream {
    match n {
        Some(n) => quote! { ::std::option::Option::Some(#n) },
        None => quote! { ::std::option::Option::None },
    }
}

fn opt_str_vec(v: Option<&Vec<String>>) -> TokenStream {
    match v {
        Some(v) => {
            let items = v.iter().map(|s| quote! { #s.to_string() });
            quote! { ::std::option::Option::Some(::std::vec![ #(#items),* ]) }
        }
        None => quote! { ::std::option::Option::None },
    }
}

fn opt_direction(d: Option<PathDirectionIr>) -> TokenStream {
    match d {
        Some(PathDirectionIr::Input) => {
            quote! { ::std::option::Option::Some(golem_rust::schema::PathDirection::Input) }
        }
        Some(PathDirectionIr::Output) => {
            quote! { ::std::option::Option::Some(golem_rust::schema::PathDirection::Output) }
        }
        Some(PathDirectionIr::InOut) => {
            quote! { ::std::option::Option::Some(golem_rust::schema::PathDirection::InOut) }
        }
        None => quote! { ::std::option::Option::None },
    }
}

fn opt_path_kind(k: Option<PathKindIr>) -> TokenStream {
    match k {
        Some(PathKindIr::File) => {
            quote! { ::std::option::Option::Some(golem_rust::schema::PathKind::File) }
        }
        Some(PathKindIr::Directory) => {
            quote! { ::std::option::Option::Some(golem_rust::schema::PathKind::Directory) }
        }
        Some(PathKindIr::Any) => {
            quote! { ::std::option::Option::Some(golem_rust::schema::PathKind::Any) }
        }
        None => quote! { ::std::option::Option::None },
    }
}

// --- type inspection --------------------------------------------------------

/// The built-in/standard type family a parameter's value graph resolves to,
/// when it can be recognized syntactically from the type's last path segment
/// (peeling a single reference so `&str` classifies as text). Returns `None` for
/// custom or otherwise unrecognized types, whose `IntoSchema` could resolve to
/// any schema kind — those are never rejected by the refinement guards.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KnownTypeFamily {
    Numeric,
    Bool,
    Char,
    Text,
    Path,
    Url,
}

fn known_type_family(ty: &Type) -> Option<KnownTypeFamily> {
    let ty = match ty {
        Type::Reference(r) => &*r.elem,
        other => other,
    };
    if is_integer_type(ty) {
        return Some(KnownTypeFamily::Numeric);
    }
    match type_last_ident(ty).as_deref() {
        Some("f32" | "f64") => Some(KnownTypeFamily::Numeric),
        Some("bool") => Some(KnownTypeFamily::Bool),
        Some("char") => Some(KnownTypeFamily::Char),
        Some("String" | "str") => Some(KnownTypeFamily::Text),
        Some("PathBuf" | "Path") => Some(KnownTypeFamily::Path),
        Some("Url") => Some(KnownTypeFamily::Url),
        _ => None,
    }
}

/// Whether the type is a recognized family other than the one a refinement
/// requires. Unrecognized (custom) types are never rejected: a proc macro cannot
/// know what schema kind their `IntoSchema` produces. Refinement coercion in
/// `refine_text`/`refine_path`/`refine_url`/`refine_numeric` would otherwise
/// silently rewrite (or drop, for numeric) a recognized incompatible type,
/// producing descriptor metadata that disagrees with the implementation
/// signature.
fn is_known_non_text(ty: &Type) -> bool {
    matches!(known_type_family(ty), Some(f) if f != KnownTypeFamily::Text)
}

fn is_known_non_path(ty: &Type) -> bool {
    matches!(known_type_family(ty), Some(f) if f != KnownTypeFamily::Path)
}

fn is_known_non_url(ty: &Type) -> bool {
    matches!(known_type_family(ty), Some(f) if f != KnownTypeFamily::Url)
}

fn is_known_non_numeric(ty: &Type) -> bool {
    matches!(known_type_family(ty), Some(f) if f != KnownTypeFamily::Numeric)
}

/// Whether the type's last path segment names a built-in Rust integer type.
fn is_integer_type(ty: &Type) -> bool {
    matches!(
        type_last_ident(ty).as_deref(),
        Some(
            "u8" | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
        )
    )
}

fn type_last_ident(ty: &Type) -> Option<String> {
    if let Type::Path(tp) = ty {
        tp.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

fn is_stream_type(ty: &Type) -> bool {
    matches!(
        type_last_ident(ty).as_deref(),
        Some("InputStream" | "OutputStream")
    )
}

/// If `ty` is `Wrapper<Inner>` (matching the last path segment by name),
/// returns `Inner`.
fn unwrap_generic1<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    if let Type::Path(tp) = ty
        && let Some(seg) = tp.path.segments.last()
        && seg.ident == wrapper
        && let PathArguments::AngleBracketed(args) = &seg.arguments
    {
        for ga in &args.args {
            if let GenericArgument::Type(t) = ga {
                return Some(t);
            }
        }
    }
    None
}

fn is_map_type(ty: &Type) -> bool {
    matches!(type_last_ident(ty).as_deref(), Some("BTreeMap" | "HashMap"))
}

/// Splits a method return type into `(ok_type, err_type)`. A `Result<T, E>`
/// yields both (with `T == ()` collapsed to `None`); a plain `-> T` yields
/// `(Some(T), None)`; `-> ()` / no return yields `(None, None)`.
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
        let ok = ok.filter(|t| !is_unit(t));
        return (ok, err);
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
