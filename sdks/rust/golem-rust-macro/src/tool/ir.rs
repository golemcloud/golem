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

//! Intermediate representation produced by tool attribute parsing.
//!
//! This module is a faithful, type-resolution-free representation of the tool
//! authoring attributes (`#[tool_definition]`, `#[arg]`, `#[command]`,
//! `#[constraint]`, `#[result]`, `#[derive(ToolError)]`). It captures *what the
//! author wrote*; turning the IR plus the trait method signatures into the
//! runtime `ExtendedToolType` metadata is done during metadata synthesis.

use syn::{Expr, Ident, Path, ReturnType, Type};

/// A full `#[tool_definition]` trait, lowered to IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinitionIr {
    /// The trait identifier; the tool name is `kebab(ident)` (resolved during metadata synthesis).
    pub trait_ident: Ident,
    /// Optional `version = "..."` from the `#[tool_definition(...)]` attribute.
    pub version: Option<String>,
    /// Doc comment on the trait.
    pub doc: DocIr,
    /// One entry per trait method, in declaration order.
    pub commands: Vec<CommandIr>,
}

/// A single trait method, lowered to IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandIr {
    /// The method identifier; the command name is `kebab(ident)` (resolved during metadata synthesis).
    pub method_ident: Ident,
    /// Doc comment on the method.
    pub doc: DocIr,
    /// `#[command(aliases = [...])]`.
    pub aliases: Vec<String>,
    /// `#[command(name = "...")]` override for the command name.
    pub name_override: Option<String>,
    /// `#[command(annotations(...))]`.
    pub annotations: Option<CommandAnnotationsIr>,
    /// `#[command(subtree = path::To::Trait)]`, if this method grafts a subtree.
    pub subtree: Option<SubtreeIr>,
    /// Whether the trait method is async; generated guest dispatch blocks on async methods.
    pub is_async: bool,
    /// Typed method parameters, in declaration order (the `&self` receiver is
    /// excluded). Metadata synthesis projects these onto positionals/options/flags/streams
    /// using each parameter's Rust type.
    pub params: Vec<ParamIr>,
    /// The method return type, used during metadata synthesis to derive the result and (for
    /// `Result<T, E>`) the error cases.
    pub output: ReturnType,
    /// `#[arg(...)]` entries, in declaration order.
    pub args: Vec<ArgIr>,
    /// `#[constraint(...)]` entries, in declaration order.
    pub constraints: Vec<ConstraintIr>,
    /// `#[result(...)]`, if present.
    pub result: Option<ResultIr>,
}

/// A typed method parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamIr {
    pub ident: Ident,
    pub ty: Type,
}

/// `#[command(subtree = path, name = "...")]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeIr {
    /// Path to the child `#[tool_definition]` trait.
    pub path: Path,
    /// Optional command-name override for the grafted node.
    pub name_override: Option<String>,
}

/// `#[command(annotations(destructive = .., read_only = .., idempotent = .., open_world = ..))]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandAnnotationsIr {
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub idempotent: Option<bool>,
    pub open_world: Option<bool>,
}

/// Where an argument lives in the command surface, as written in the leading
/// `<param> = "<placement>"` form of `#[arg(...)]`. `None` means the placement
/// is inferred from the parameter type during metadata synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgPlacement {
    Global,
    Positional,
    Option,
    Flag,
    Tail,
}

/// `kind = "flag" | "count-flag"` modifier (used mostly on global args to say a
/// global is a flag/count-flag rather than an option).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSubKind {
    Flag,
    CountFlag,
}

/// `repeatable = "repeated" | "delimited" | "either"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatableMode {
    Repeated,
    Delimited,
    Either,
}

/// Path `kind = "file" | "dir" | "any"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKindIr {
    File,
    Directory,
    Any,
}

/// Path `direction = "input" | "output" | "inout"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDirectionIr {
    Input,
    Output,
    InOut,
}

/// A single `#[arg(...)]` entry, fully parsed but not yet projected onto a
/// schema type (that is metadata synthesis, which has the parameter's Rust type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgIr {
    /// Parameter identifier this `#[arg]` binds to.
    pub param: Ident,
    /// Explicit placement override, or `None` to infer from the type.
    pub placement: Option<ArgPlacement>,
    /// `kind = "flag" | "count-flag"`.
    pub sub_kind: Option<ArgSubKind>,

    // --- option/flag surface ---
    pub short: Option<char>,
    pub aliases: Vec<String>,
    pub env: Option<String>,
    pub required: Option<bool>,
    pub negatable: Option<bool>,
    pub optional_scalar: bool,
    pub repeatable: Option<RepeatableMode>,
    pub delim: Option<char>,
    /// Raw `default = <expr>` literal; resolved against the parameter type during metadata synthesis.
    pub default: Option<Expr>,

    // --- tail positional ---
    pub separator: Option<String>,
    pub verbatim: bool,
    pub accepts_stdio: bool,

    // --- text refinement ---
    pub regex: Option<String>,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,

    // --- path refinement ---
    pub path_kind: Option<PathKindIr>,
    pub direction: Option<PathDirectionIr>,
    pub mime: Option<Vec<String>>,

    // --- url refinement ---
    pub schemes: Option<Vec<String>>,

    // --- raw `min` / `max` (raw exprs) ---
    // Their meaning depends on the *final* placement and sub-kind, which may be
    // inferred from the parameter type during metadata synthesis (tail occurrence counts vs.
    // count-flag max vs. numeric bound), so they are not classified here.
    pub raw_min: Option<Expr>,
    pub raw_max: Option<Expr>,

    // --- numeric refinement (raw exprs; numeric repr known during metadata synthesis) ---
    pub bounds: Option<(Expr, Expr)>,
    pub unit: Option<String>,

    // --- documentation ---
    pub doc: Option<String>,
    pub value_name: Option<String>,
}

impl ArgIr {
    pub fn new(param: Ident) -> Self {
        ArgIr {
            param,
            placement: None,
            sub_kind: None,
            short: None,
            aliases: Vec::new(),
            env: None,
            required: None,
            negatable: None,
            optional_scalar: false,
            repeatable: None,
            delim: None,
            default: None,
            separator: None,
            verbatim: false,
            accepts_stdio: false,
            regex: None,
            min_length: None,
            max_length: None,
            path_kind: None,
            direction: None,
            mime: None,
            schemes: None,
            raw_min: None,
            raw_max: None,
            bounds: None,
            unit: None,
            doc: None,
            value_name: None,
        }
    }
}

/// `all`/`any` quantifier in `implies`/`forbids`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantifierIr {
    All,
    Any,
}

/// A reference to an argument in a constraint, by its tool-facing name.
/// `value_is` carries the raw literal expression (resolved during metadata synthesis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefIr {
    Present(String),
    ValueIs { name: String, value: Expr },
}

/// A single `#[constraint(...)]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintIr {
    RequiresAll(Vec<RefIr>),
    AllOrNone(Vec<RefIr>),
    RequiresAny(Vec<RefIr>),
    MutexGroups(Vec<Vec<RefIr>>),
    Implies {
        lhs_quant: QuantifierIr,
        lhs: Vec<RefIr>,
        rhs_quant: QuantifierIr,
        rhs: Vec<RefIr>,
    },
    Forbids {
        lhs_quant: QuantifierIr,
        lhs: Vec<RefIr>,
        rhs: Vec<RefIr>,
    },
}

/// `#[result(formatters = [...], default = "...")]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResultIr {
    pub formatters: Vec<String>,
    pub default_formatter: Option<String>,
}

/// Doc comment, split into a summary and a longer description, plus any
/// `#[example(...)]` entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocIr {
    pub summary: String,
    pub description: String,
    pub examples: Vec<ExampleIr>,
}

/// A `#[example(title = "...", body = "...")]` entry, mirroring the wire
/// `golem:tool` `example` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleIr {
    pub title: String,
    pub body: String,
}

/// `#[derive(ToolError)]` enum, lowered to IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolErrorIr {
    pub enum_ident: Ident,
    pub variants: Vec<ToolErrorVariantIr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKindIr {
    UsageError,
    RuntimeError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolErrorVariantIr {
    pub variant_ident: Ident,
    pub doc: DocIr,
    pub kind: ErrorKindIr,
    pub exit_code: u8,
    /// Payload schema source: unit variants carry no payload, single-field
    /// variants carry the field's type (resolved to a `SchemaType` during metadata synthesis).
    pub payload: ToolErrorPayloadIr,
}

/// The payload of a `#[derive(ToolError)]` variant. Variants with two or more
/// fields are rejected at parse time (no synthetic record is generated).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ToolErrorPayloadIr {
    None,
    Single {
        ty: Type,
        field_ident: Option<Ident>,
    },
}
