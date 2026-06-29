// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Round-trip conversion between the recursive in-memory tool metadata model in
//! [`super`] and the flat, index-based `golem:tool/common@0.1.0` wire bindings
//! re-exported here as [`wire`].
//!
//! Mirrors [`crate::schema::agent::wit`]: a tool's `defs` plus every type
//! embedded in its command tree are folded into one shared
//! [`wire::SchemaGraph`](crate::schema::wit::wire::SchemaGraph) via
//! [`GraphEncoder`] / [`GraphDecoder`]. Embedded [`SchemaType`]s become
//! `type-node-index` values into that graph, and embedded [`SchemaValue`]
//! defaults / `value-is` literals become bare
//! [`wire::SchemaValueTree`](crate::schema::wit::wire::SchemaValueTree)s via
//! [`encode_value`] / [`decode_value`].
//!
//! Conversions that need neither the shared graph nor any fallible value
//! decoding are expressed as [`From`] impls (see the "infallible, context-free
//! conversions" section); the conversions that fold embedded types into the
//! shared graph (and so thread a [`GraphEncoder`] / [`GraphDecoder`]) or decode
//! embedded values (which is fallible) stay as functions.

use super::*;
use crate::schema::graph::SchemaGraph;
use golem_schema::schema::wit::{
    DecodeError, EncodeError, GraphDecoder, GraphEncoder, decode_value, encode_value,
};

/// Generated `golem:tool/common@0.1.0` types used as the wire shape. Produced by
/// the single workspace agent bindgen in [`crate::schema::agent::bindings`].
pub use crate::schema::agent::bindings::golem::tool::common as wire;

/// Error raised when converting between the native tool model and its wire form.
#[derive(Debug)]
pub enum ToolWitError {
    Encode(EncodeError),
    Decode(DecodeError),
}

impl std::fmt::Display for ToolWitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolWitError::Encode(e) => write!(f, "tool schema encode error: {e}"),
            ToolWitError::Decode(e) => write!(f, "tool schema decode error: {e}"),
        }
    }
}

impl std::error::Error for ToolWitError {}

impl From<EncodeError> for ToolWitError {
    fn from(e: EncodeError) -> Self {
        ToolWitError::Encode(e)
    }
}

impl From<DecodeError> for ToolWitError {
    fn from(e: DecodeError) -> Self {
        ToolWitError::Decode(e)
    }
}

// ============================================================
// infallible, context-free conversions
//
// These native <-> wire conversions need neither the shared graph
// encoder/decoder nor any fallible value decoding, so they are expressed as
// `From` impls (mirroring `crate::schema::agent::wit`). They are taken by
// reference because their call sites already hold borrows of the surrounding
// records.
// ============================================================

impl From<&Doc> for wire::Doc {
    fn from(d: &Doc) -> Self {
        wire::Doc {
            summary: d.summary.clone(),
            description: d.description.clone(),
            examples: d.examples.iter().map(wire::Example::from).collect(),
        }
    }
}

impl From<&wire::Doc> for Doc {
    fn from(d: &wire::Doc) -> Self {
        Doc {
            summary: d.summary.clone(),
            description: d.description.clone(),
            examples: d.examples.iter().map(Example::from).collect(),
        }
    }
}

impl From<&Example> for wire::Example {
    fn from(e: &Example) -> Self {
        wire::Example {
            title: e.title.clone(),
            body: e.body.clone(),
        }
    }
}

impl From<&wire::Example> for Example {
    fn from(e: &wire::Example) -> Self {
        Example {
            title: e.title.clone(),
            body: e.body.clone(),
        }
    }
}

impl From<&FlagSpec> for wire::FlagSpec {
    fn from(f: &FlagSpec) -> Self {
        wire::FlagSpec {
            long: f.long.clone(),
            short: f.short,
            aliases: f.aliases.clone(),
            doc: wire::Doc::from(&f.doc),
            shape: wire::FlagShape::from(&f.shape),
            env_var: f.env_var.clone(),
        }
    }
}

impl From<&wire::FlagSpec> for FlagSpec {
    fn from(f: &wire::FlagSpec) -> Self {
        FlagSpec {
            long: f.long.clone(),
            short: f.short,
            aliases: f.aliases.clone(),
            doc: Doc::from(&f.doc),
            shape: FlagShape::from(&f.shape),
            env_var: f.env_var.clone(),
        }
    }
}

impl From<&FlagShape> for wire::FlagShape {
    fn from(s: &FlagShape) -> Self {
        match s {
            FlagShape::BoolFlag(b) => wire::FlagShape::BoolFlag(wire::BoolFlagShape::from(b)),
            FlagShape::CountFlag(m) => wire::FlagShape::CountFlag(*m),
        }
    }
}

impl From<&wire::FlagShape> for FlagShape {
    fn from(s: &wire::FlagShape) -> Self {
        match s {
            wire::FlagShape::BoolFlag(b) => FlagShape::BoolFlag(BoolFlagShape::from(b)),
            wire::FlagShape::CountFlag(m) => FlagShape::CountFlag(*m),
        }
    }
}

impl From<&BoolFlagShape> for wire::BoolFlagShape {
    fn from(b: &BoolFlagShape) -> Self {
        wire::BoolFlagShape {
            default: b.default,
            negatable: b.negatable,
        }
    }
}

impl From<&wire::BoolFlagShape> for BoolFlagShape {
    fn from(b: &wire::BoolFlagShape) -> Self {
        BoolFlagShape {
            default: b.default,
            negatable: b.negatable,
        }
    }
}

impl From<&Repetition> for wire::Repetition {
    fn from(r: &Repetition) -> Self {
        match r {
            Repetition::Repeated => wire::Repetition::Repeated,
            Repetition::Delimited(c) => wire::Repetition::Delimited(*c),
            Repetition::Either(c) => wire::Repetition::Either(*c),
        }
    }
}

impl From<&wire::Repetition> for Repetition {
    fn from(r: &wire::Repetition) -> Self {
        match r {
            wire::Repetition::Repeated => Repetition::Repeated,
            wire::Repetition::Delimited(c) => Repetition::Delimited(*c),
            wire::Repetition::Either(c) => Repetition::Either(*c),
        }
    }
}

impl From<&Quantifier> for wire::Quantifier {
    fn from(q: &Quantifier) -> Self {
        match q {
            Quantifier::All => wire::Quantifier::All,
            Quantifier::Any => wire::Quantifier::Any,
        }
    }
}

impl From<&wire::Quantifier> for Quantifier {
    fn from(q: &wire::Quantifier) -> Self {
        match q {
            wire::Quantifier::All => Quantifier::All,
            wire::Quantifier::Any => Quantifier::Any,
        }
    }
}

impl From<&StreamSpec> for wire::StreamSpec {
    fn from(s: &StreamSpec) -> Self {
        wire::StreamSpec {
            doc: wire::Doc::from(&s.doc),
            mime: s.mime.clone(),
            required: s.required,
        }
    }
}

impl From<&wire::StreamSpec> for StreamSpec {
    fn from(s: &wire::StreamSpec) -> Self {
        StreamSpec {
            doc: Doc::from(&s.doc),
            mime: s.mime.clone(),
            required: s.required,
        }
    }
}

impl From<&Formatter> for wire::Formatter {
    fn from(f: &Formatter) -> Self {
        wire::Formatter {
            name: f.name.clone(),
            doc: wire::Doc::from(&f.doc),
        }
    }
}

impl From<&wire::Formatter> for Formatter {
    fn from(f: &wire::Formatter) -> Self {
        Formatter {
            name: f.name.clone(),
            doc: Doc::from(&f.doc),
        }
    }
}

impl From<&ErrorKind> for wire::ErrorKind {
    fn from(k: &ErrorKind) -> Self {
        match k {
            ErrorKind::UsageError => wire::ErrorKind::UsageError,
            ErrorKind::RuntimeError => wire::ErrorKind::RuntimeError,
        }
    }
}

impl From<&wire::ErrorKind> for ErrorKind {
    fn from(k: &wire::ErrorKind) -> Self {
        match k {
            wire::ErrorKind::UsageError => ErrorKind::UsageError,
            wire::ErrorKind::RuntimeError => ErrorKind::RuntimeError,
        }
    }
}

impl From<&CommandAnnotations> for wire::CommandAnnotations {
    fn from(a: &CommandAnnotations) -> Self {
        wire::CommandAnnotations {
            read_only: a.read_only,
            destructive: a.destructive,
            idempotent: a.idempotent,
            open_world: a.open_world,
        }
    }
}

impl From<&wire::CommandAnnotations> for CommandAnnotations {
    fn from(a: &wire::CommandAnnotations) -> Self {
        CommandAnnotations {
            read_only: a.read_only,
            destructive: a.destructive,
            idempotent: a.idempotent,
            open_world: a.open_world,
        }
    }
}

// Constraints, refs, and `value-is` literals are context-free in the native
// -> wire direction, but encoding an embedded `SchemaValue` (a `value-is`
// literal) is fallible: `encode_value` refuses non-transportable capability
// values such as a `QuotaToken`. The encode side therefore threads `Result`
// like the rest of the file; the wire -> native direction is fallible too (see
// `decode_constraint`).

fn encode_constraint(c: &Constraint) -> Result<wire::Constraint, ToolWitError> {
    Ok(match c {
        Constraint::RequiresAll(rs) => wire::Constraint::RequiresAll(encode_refs(rs)?),
        Constraint::AllOrNone(rs) => wire::Constraint::AllOrNone(encode_refs(rs)?),
        Constraint::RequiresAny(rs) => wire::Constraint::RequiresAny(encode_refs(rs)?),
        Constraint::MutexGroups(gs) => wire::Constraint::MutexGroups(
            gs.iter()
                .map(encode_ref_group)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Constraint::Implies(i) => wire::Constraint::Implies(wire::ImpliesC {
            lhs_quant: wire::Quantifier::from(&i.lhs_quant),
            lhs: encode_refs(&i.lhs)?,
            rhs_quant: wire::Quantifier::from(&i.rhs_quant),
            rhs: encode_refs(&i.rhs)?,
        }),
        Constraint::Forbids(f) => wire::Constraint::Forbids(wire::ForbidsC {
            lhs_quant: wire::Quantifier::from(&f.lhs_quant),
            lhs: encode_refs(&f.lhs)?,
            rhs: encode_refs(&f.rhs)?,
        }),
    })
}

fn encode_ref_group(g: &RefGroup) -> Result<wire::RefGroup, ToolWitError> {
    Ok(wire::RefGroup {
        refs: encode_refs(&g.refs)?,
    })
}

fn encode_ref(r: &Ref) -> Result<wire::Ref, ToolWitError> {
    Ok(match r {
        Ref::Present(name) => wire::Ref::Present(name.clone()),
        Ref::ValueIs(v) => wire::Ref::ValueIs(encode_value_is_ref(v)?),
    })
}

fn encode_value_is_ref(v: &ValueIsRef) -> Result<wire::ValueIsRef, ToolWitError> {
    Ok(wire::ValueIsRef {
        name: v.name.clone(),
        value: encode_value(&v.value)?,
    })
}

fn encode_refs(rs: &[Ref]) -> Result<Vec<wire::Ref>, ToolWitError> {
    rs.iter().map(encode_ref).collect()
}

// ============================================================
// native -> wire
// ============================================================

/// Encode a [`Tool`] into the flat `golem:tool/common@0.1.0` wire form. The
/// tool's `defs` plus every type embedded in its command tree are folded into
/// one shared [`wire::SchemaGraph`](crate::schema::wit::wire::SchemaGraph).
pub fn encode_tool(tool: &Tool) -> Result<wire::Tool, ToolWitError> {
    let mut enc = GraphEncoder::new(&tool.schema.defs)?;
    let commands = encode_command_tree(&mut enc, &tool.commands)?;
    let schema = enc.finish();
    Ok(wire::Tool {
        version: tool.version.clone(),
        commands,
        schema,
    })
}

fn encode_command_tree(
    enc: &mut GraphEncoder,
    ct: &CommandTree,
) -> Result<wire::CommandTree, ToolWitError> {
    Ok(wire::CommandTree {
        nodes: ct
            .nodes
            .iter()
            .map(|n| encode_command_node(enc, n))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn encode_command_node(
    enc: &mut GraphEncoder,
    n: &CommandNode,
) -> Result<wire::CommandNode, ToolWitError> {
    Ok(wire::CommandNode {
        name: n.name.clone(),
        aliases: n.aliases.clone(),
        doc: wire::Doc::from(&n.doc),
        globals: encode_globals(enc, &n.globals)?,
        subcommands: n.subcommands.iter().map(|c| c.0).collect(),
        body: n
            .body
            .as_ref()
            .map(|b| encode_command_body(enc, b))
            .transpose()?,
    })
}

fn encode_globals(enc: &mut GraphEncoder, g: &Globals) -> Result<wire::Globals, ToolWitError> {
    Ok(wire::Globals {
        options: g
            .options
            .iter()
            .map(|o| encode_option_spec(enc, o))
            .collect::<Result<Vec<_>, _>>()?,
        flags: g.flags.iter().map(wire::FlagSpec::from).collect(),
    })
}

fn encode_command_body(
    enc: &mut GraphEncoder,
    b: &CommandBody,
) -> Result<wire::CommandBody, ToolWitError> {
    Ok(wire::CommandBody {
        positionals: encode_positionals(enc, &b.positionals)?,
        options: b
            .options
            .iter()
            .map(|o| encode_option_spec(enc, o))
            .collect::<Result<Vec<_>, _>>()?,
        flags: b.flags.iter().map(wire::FlagSpec::from).collect(),
        constraints: b
            .constraints
            .iter()
            .map(encode_constraint)
            .collect::<Result<Vec<_>, _>>()?,
        stdin: b.stdin.as_ref().map(wire::StreamSpec::from),
        stdout: b.stdout.as_ref().map(wire::StreamSpec::from),
        result: b
            .result
            .as_ref()
            .map(|r| encode_result_spec(enc, r))
            .transpose()?,
        errors: b
            .errors
            .iter()
            .map(|e| encode_error_case(enc, e))
            .collect::<Result<Vec<_>, _>>()?,
        annotations: b.annotations.as_ref().map(wire::CommandAnnotations::from),
    })
}

fn encode_positionals(
    enc: &mut GraphEncoder,
    p: &Positionals,
) -> Result<wire::Positionals, ToolWitError> {
    Ok(wire::Positionals {
        fixed: p
            .fixed
            .iter()
            .map(|x| encode_positional(enc, x))
            .collect::<Result<Vec<_>, _>>()?,
        tail: p
            .tail
            .as_ref()
            .map(|t| encode_tail_positional(enc, t))
            .transpose()?,
    })
}

fn encode_positional(
    enc: &mut GraphEncoder,
    p: &Positional,
) -> Result<wire::Positional, ToolWitError> {
    Ok(wire::Positional {
        name: p.name.clone(),
        doc: wire::Doc::from(&p.doc),
        value_name: p.value_name.clone(),
        type_: enc.encode_type(&p.type_)?,
        default: p.default.as_ref().map(encode_value).transpose()?,
        required: p.required,
    })
}

fn encode_tail_positional(
    enc: &mut GraphEncoder,
    t: &TailPositional,
) -> Result<wire::TailPositional, ToolWitError> {
    Ok(wire::TailPositional {
        name: t.name.clone(),
        doc: wire::Doc::from(&t.doc),
        value_name: t.value_name.clone(),
        item_type: enc.encode_type(&t.item_type)?,
        min: t.min,
        max: t.max,
        separator: t.separator.clone(),
        verbatim: t.verbatim,
    })
}

fn encode_option_spec(
    enc: &mut GraphEncoder,
    o: &OptionSpec,
) -> Result<wire::OptionSpec, ToolWitError> {
    Ok(wire::OptionSpec {
        long: o.long.clone(),
        short: o.short,
        aliases: o.aliases.clone(),
        doc: wire::Doc::from(&o.doc),
        value_name: o.value_name.clone(),
        shape: encode_option_shape(enc, &o.shape)?,
        default: o.default.as_ref().map(encode_value).transpose()?,
        required: o.required,
        env_var: o.env_var.clone(),
    })
}

fn encode_option_shape(
    enc: &mut GraphEncoder,
    s: &OptionShape,
) -> Result<wire::OptionShape, ToolWitError> {
    Ok(match s {
        OptionShape::Scalar(ty) => wire::OptionShape::Scalar(enc.encode_type(ty)?),
        OptionShape::OptionalScalar(ty) => wire::OptionShape::OptionalScalar(enc.encode_type(ty)?),
        OptionShape::Repeatable(r) => wire::OptionShape::Repeatable(wire::RepeatableShape {
            repetition: wire::Repetition::from(&r.repetition),
            type_: enc.encode_type(&r.type_)?,
        }),
    })
}

fn encode_result_spec(
    enc: &mut GraphEncoder,
    r: &ResultSpec,
) -> Result<wire::ResultSpec, ToolWitError> {
    Ok(wire::ResultSpec {
        type_: enc.encode_type(&r.type_)?,
        doc: wire::Doc::from(&r.doc),
        formatters: r.formatters.iter().map(wire::Formatter::from).collect(),
        default_formatter: r.default_formatter.clone(),
    })
}

fn encode_error_case(
    enc: &mut GraphEncoder,
    e: &ErrorCase,
) -> Result<wire::ErrorCase, ToolWitError> {
    Ok(wire::ErrorCase {
        name: e.name.clone(),
        doc: wire::Doc::from(&e.doc),
        kind: wire::ErrorKind::from(&e.kind),
        exit_code: e.exit_code,
        payload: e.payload.as_ref().map(|p| enc.encode_type(p)).transpose()?,
    })
}

// ============================================================
// wire -> native
// ============================================================

/// Decode a `golem:tool/common@0.1.0` wire [`wire::Tool`] into the recursive
/// [`Tool`] model.
pub fn decode_tool(w: &wire::Tool) -> Result<Tool, ToolWitError> {
    let dec = GraphDecoder::new(&w.schema)?;
    let mut schema = SchemaGraph::empty();
    schema.defs = dec.decode_defs()?;
    let commands = decode_command_tree(&dec, &w.commands)?;
    Ok(Tool {
        version: w.version.clone(),
        commands,
        schema,
    })
}

fn decode_command_tree(
    dec: &GraphDecoder,
    ct: &wire::CommandTree,
) -> Result<CommandTree, ToolWitError> {
    Ok(CommandTree {
        nodes: ct
            .nodes
            .iter()
            .map(|n| decode_command_node(dec, n))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn decode_command_node(
    dec: &GraphDecoder,
    n: &wire::CommandNode,
) -> Result<CommandNode, ToolWitError> {
    Ok(CommandNode {
        name: n.name.clone(),
        aliases: n.aliases.clone(),
        doc: Doc::from(&n.doc),
        globals: decode_globals(dec, &n.globals)?,
        subcommands: n.subcommands.iter().map(|c| CommandIndex(*c)).collect(),
        body: n
            .body
            .as_ref()
            .map(|b| decode_command_body(dec, b))
            .transpose()?,
    })
}

fn decode_globals(dec: &GraphDecoder, g: &wire::Globals) -> Result<Globals, ToolWitError> {
    Ok(Globals {
        options: g
            .options
            .iter()
            .map(|o| decode_option_spec(dec, o))
            .collect::<Result<Vec<_>, _>>()?,
        flags: g.flags.iter().map(FlagSpec::from).collect(),
    })
}

fn decode_command_body(
    dec: &GraphDecoder,
    b: &wire::CommandBody,
) -> Result<CommandBody, ToolWitError> {
    Ok(CommandBody {
        positionals: decode_positionals(dec, &b.positionals)?,
        options: b
            .options
            .iter()
            .map(|o| decode_option_spec(dec, o))
            .collect::<Result<Vec<_>, _>>()?,
        flags: b.flags.iter().map(FlagSpec::from).collect(),
        constraints: b
            .constraints
            .iter()
            .map(decode_constraint)
            .collect::<Result<Vec<_>, _>>()?,
        stdin: b.stdin.as_ref().map(StreamSpec::from),
        stdout: b.stdout.as_ref().map(StreamSpec::from),
        result: b
            .result
            .as_ref()
            .map(|r| decode_result_spec(dec, r))
            .transpose()?,
        errors: b
            .errors
            .iter()
            .map(|e| decode_error_case(dec, e))
            .collect::<Result<Vec<_>, _>>()?,
        annotations: b.annotations.as_ref().map(CommandAnnotations::from),
    })
}

fn decode_positionals(
    dec: &GraphDecoder,
    p: &wire::Positionals,
) -> Result<Positionals, ToolWitError> {
    Ok(Positionals {
        fixed: p
            .fixed
            .iter()
            .map(|x| decode_positional(dec, x))
            .collect::<Result<Vec<_>, _>>()?,
        tail: p
            .tail
            .as_ref()
            .map(|t| decode_tail_positional(dec, t))
            .transpose()?,
    })
}

fn decode_positional(dec: &GraphDecoder, p: &wire::Positional) -> Result<Positional, ToolWitError> {
    Ok(Positional {
        name: p.name.clone(),
        doc: Doc::from(&p.doc),
        value_name: p.value_name.clone(),
        type_: dec.decode_type_at(p.type_)?,
        default: p.default.as_ref().map(decode_value).transpose()?,
        required: p.required,
    })
}

fn decode_tail_positional(
    dec: &GraphDecoder,
    t: &wire::TailPositional,
) -> Result<TailPositional, ToolWitError> {
    Ok(TailPositional {
        name: t.name.clone(),
        doc: Doc::from(&t.doc),
        value_name: t.value_name.clone(),
        item_type: dec.decode_type_at(t.item_type)?,
        min: t.min,
        max: t.max,
        separator: t.separator.clone(),
        verbatim: t.verbatim,
    })
}

fn decode_option_spec(
    dec: &GraphDecoder,
    o: &wire::OptionSpec,
) -> Result<OptionSpec, ToolWitError> {
    Ok(OptionSpec {
        long: o.long.clone(),
        short: o.short,
        aliases: o.aliases.clone(),
        doc: Doc::from(&o.doc),
        value_name: o.value_name.clone(),
        shape: decode_option_shape(dec, &o.shape)?,
        default: o.default.as_ref().map(decode_value).transpose()?,
        required: o.required,
        env_var: o.env_var.clone(),
    })
}

fn decode_option_shape(
    dec: &GraphDecoder,
    s: &wire::OptionShape,
) -> Result<OptionShape, ToolWitError> {
    Ok(match s {
        wire::OptionShape::Scalar(ty) => OptionShape::Scalar(dec.decode_type_at(*ty)?),
        wire::OptionShape::OptionalScalar(ty) => {
            OptionShape::OptionalScalar(dec.decode_type_at(*ty)?)
        }
        wire::OptionShape::Repeatable(r) => OptionShape::Repeatable(RepeatableShape {
            repetition: Repetition::from(&r.repetition),
            type_: dec.decode_type_at(r.type_)?,
        }),
    })
}

fn decode_constraint(c: &wire::Constraint) -> Result<Constraint, ToolWitError> {
    Ok(match c {
        wire::Constraint::RequiresAll(rs) => Constraint::RequiresAll(decode_refs(rs)?),
        wire::Constraint::AllOrNone(rs) => Constraint::AllOrNone(decode_refs(rs)?),
        wire::Constraint::RequiresAny(rs) => Constraint::RequiresAny(decode_refs(rs)?),
        wire::Constraint::MutexGroups(gs) => Constraint::MutexGroups(
            gs.iter()
                .map(decode_ref_group)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        wire::Constraint::Implies(i) => Constraint::Implies(ImpliesC {
            lhs_quant: Quantifier::from(&i.lhs_quant),
            lhs: decode_refs(&i.lhs)?,
            rhs_quant: Quantifier::from(&i.rhs_quant),
            rhs: decode_refs(&i.rhs)?,
        }),
        wire::Constraint::Forbids(f) => Constraint::Forbids(ForbidsC {
            lhs_quant: Quantifier::from(&f.lhs_quant),
            lhs: decode_refs(&f.lhs)?,
            rhs: decode_refs(&f.rhs)?,
        }),
    })
}

fn decode_refs(rs: &[wire::Ref]) -> Result<Vec<Ref>, ToolWitError> {
    rs.iter().map(decode_ref).collect()
}

fn decode_ref_group(g: &wire::RefGroup) -> Result<RefGroup, ToolWitError> {
    Ok(RefGroup {
        refs: decode_refs(&g.refs)?,
    })
}

fn decode_ref(r: &wire::Ref) -> Result<Ref, ToolWitError> {
    Ok(match r {
        wire::Ref::Present(name) => Ref::Present(name.clone()),
        wire::Ref::ValueIs(v) => Ref::ValueIs(ValueIsRef {
            name: v.name.clone(),
            value: decode_value(&v.value)?,
        }),
    })
}

fn decode_result_spec(
    dec: &GraphDecoder,
    r: &wire::ResultSpec,
) -> Result<ResultSpec, ToolWitError> {
    Ok(ResultSpec {
        type_: dec.decode_type_at(r.type_)?,
        doc: Doc::from(&r.doc),
        formatters: r.formatters.iter().map(Formatter::from).collect(),
        default_formatter: r.default_formatter.clone(),
    })
}

fn decode_error_case(dec: &GraphDecoder, e: &wire::ErrorCase) -> Result<ErrorCase, ToolWitError> {
    Ok(ErrorCase {
        name: e.name.clone(),
        doc: Doc::from(&e.doc),
        kind: ErrorKind::from(&e.kind),
        exit_code: e.exit_code,
        payload: e
            .payload
            .as_ref()
            .map(|p| dec.decode_type_at(*p))
            .transpose()?,
    })
}
