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

//! The external (REST) client.
//!
//! An external client has no SDK beneath it to reflect over its types, so the
//! generator writes the conversions itself: one `encodeX`/`decodeX` pair per
//! named type, and one numbered pair per distinct composite spelled inline — a
//! `[]values.Option[int64]`, say. Every pair is built from the `bridge`
//! package's leaf conversions and checked accessors, so the wire encoding
//! itself lives in exactly one place.
//!
//! Composites get a function each, rather than a nested function literal,
//! because a fixed list's decoder needs statements and because a named function
//! can be shared by every place the same Go type occurs.

use crate::bridge_gen::go::go::{
    go_string, lower_first, to_field_ident, to_param_ident, unique_idents,
    unique_idents_with_reserved,
};
use crate::bridge_gen::go::go_writer::GoWriter;
use crate::bridge_gen::go::{GoBridgeGenerator, case_idents};
use crate::bridge_gen::type_naming::user_supplied_fields;
use crate::sdk_overrides::{GO_BRIDGE_MODULE, GO_CORE_MODULE, sdk_overrides};
use crate::versions;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::{AgentMethodSchema, OutputSchema};
use std::collections::HashMap;

pub const BRIDGE_PKG: &str = GO_BRIDGE_MODULE;
pub const SCHEMA_PKG: &str = "github.com/golemcloud/golem/sdks/go/core/schema";

/// Which half of a conversion pair.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Dir {
    Encode,
    Decode,
}

impl Dir {
    fn prefix(self) -> &'static str {
        match self {
            Dir::Encode => "encode",
            Dir::Decode => "decode",
        }
    }
}

/// Collects the conversion functions a file needs. The body of each helper is
/// assembled as lines and emitted after the walk, because a helper's
/// dependencies are discovered while its own body is being built.
struct Codecs<'g> {
    generator: &'g GoBridgeGenerator,
    /// The codec file. Imports a helper's Go type needs are recorded here,
    /// wherever the helper was first asked for.
    w: GoWriter,
    helpers: Vec<String>,
    /// Per direction and Go type: the helper already written for it.
    cache: HashMap<(Dir, String), String>,
    /// Numbered per direction, so a type's encoder and decoder usually share
    /// a number.
    next: HashMap<Dir, usize>,
}

impl<'g> Codecs<'g> {
    fn new(generator: &'g GoBridgeGenerator) -> Self {
        Self {
            generator,
            w: GoWriter::new(),
            helpers: Vec::new(),
            cache: HashMap::new(),
            next: HashMap::new(),
        }
    }

    /// The name of a Go function converting a value of `typ` in direction
    /// `dir`: a `bridge` leaf, a named type's pair, or a helper written here.
    fn func(&mut self, dir: Dir, typ: &SchemaType) -> anyhow::Result<String> {
        let g = self.generator;
        if let Some(name) = g.named(typ) {
            return Ok(format!("{}{name}", dir.prefix()));
        }
        let leaf = |name: &str| {
            Ok(match dir {
                Dir::Encode => format!("bridge.Encode{name}"),
                Dir::Decode => format!("bridge.Decode{name}"),
            })
        };
        let resolved = g.resolve(typ);
        match resolved {
            SchemaType::Bool { .. } => leaf("Bool"),
            SchemaType::S8 { .. } => leaf("S8"),
            SchemaType::S16 { .. } => leaf("S16"),
            SchemaType::S32 { .. } => leaf("S32"),
            SchemaType::S64 { .. } => leaf("S64"),
            SchemaType::U8 { .. } => leaf("U8"),
            SchemaType::U16 { .. } => leaf("U16"),
            SchemaType::U32 { .. } => leaf("U32"),
            SchemaType::U64 { .. } => leaf("U64"),
            SchemaType::F32 { .. } => leaf("F32"),
            SchemaType::F64 { .. } => leaf("F64"),
            SchemaType::String { .. } => leaf("String"),
            SchemaType::Char { .. } => leaf("Char"),
            SchemaType::Text { .. } => leaf("Text"),
            SchemaType::Binary { .. } => leaf("Binary"),
            SchemaType::Path { .. } => leaf("Path"),
            SchemaType::Url { .. } => leaf("URL"),
            SchemaType::Datetime { .. } => leaf("Datetime"),
            SchemaType::Duration { .. } => leaf("Duration"),
            SchemaType::Tuple { elements, .. } if elements.len() == 1 => {
                self.func(dir, &elements[0])
            }
            _ => self.helper(dir, resolved),
        }
    }

    /// A numbered helper for a composite spelled inline.
    fn helper(&mut self, dir: Dir, typ: &SchemaType) -> anyhow::Result<String> {
        let go_type = self.generator.render(typ, &mut self.w)?;
        if let Some(name) = self.cache.get(&(dir, go_type.clone())) {
            return Ok(name.clone());
        }

        let body = match (dir, typ) {
            (Dir::Encode, SchemaType::Option { inner, .. }) => {
                format!("return bridge.EncodeOption(v, {})", self.func(dir, inner)?)
            }
            (Dir::Decode, SchemaType::Option { inner, .. }) => {
                format!("return bridge.DecodeOption(sv, {})", self.func(dir, inner)?)
            }
            (Dir::Encode, SchemaType::List { element, .. }) => {
                format!("return bridge.EncodeList(v, {})", self.func(dir, element)?)
            }
            (Dir::Decode, SchemaType::List { element, .. }) => {
                format!("return bridge.DecodeList(sv, {})", self.func(dir, element)?)
            }
            (Dir::Encode, SchemaType::FixedList { element, .. }) => format!(
                "return bridge.EncodeFixedList(v[:], {})",
                self.func(dir, element)?
            ),
            // A Go array cannot be the result of a generic function over its
            // length, so the decoded slice is copied into one.
            (
                Dir::Decode,
                SchemaType::FixedList {
                    element, length, ..
                },
            ) => format!(
                "var out {go_type}\n\
                 items, err := bridge.DecodeFixedList(sv, {length}, {})\n\
                 copy(out[:], items)\n\
                 return out, err",
                self.func(dir, element)?
            ),
            (Dir::Encode, SchemaType::Map { key, value, .. }) => format!(
                "return bridge.EncodeMap(v, {}, {})",
                self.func(dir, key)?,
                self.func(dir, value)?
            ),
            (Dir::Decode, SchemaType::Map { key, value, .. }) => format!(
                "return bridge.DecodeMap(sv, {}, {})",
                self.func(dir, key)?,
                self.func(dir, value)?
            ),
            (_, SchemaType::Tuple { elements, .. }) => {
                let mut funcs = Vec::with_capacity(elements.len());
                for element in elements {
                    funcs.push(self.func(dir, element)?);
                }
                let n = elements.len();
                match dir {
                    Dir::Encode => format!("return bridge.EncodeTuple{n}(v, {})", funcs.join(", ")),
                    Dir::Decode => {
                        format!("return bridge.DecodeTuple{n}(sv, {})", funcs.join(", "))
                    }
                }
            }
            // A unit arm has no conversion; nil tells the bridge it travels
            // without a value. The type arguments are spelled out because nil
            // gives inference nothing to go on.
            (_, SchemaType::Result { spec, .. }) => {
                let (ok_type, ok) = self.result_arm(dir, spec.ok.as_deref())?;
                let (err_type, err) = self.result_arm(dir, spec.err.as_deref())?;
                match dir {
                    Dir::Encode => {
                        format!("return bridge.EncodeResult[{ok_type}, {err_type}](v, {ok}, {err})")
                    }
                    Dir::Decode => format!(
                        "return bridge.DecodeResult[{ok_type}, {err_type}](sv, {ok}, {err})"
                    ),
                }
            }
            _ => anyhow::bail!("the Go external bridge cannot convert this schema type: {typ:?}"),
        };

        let next = self.next.entry(dir).or_insert(1);
        let name = format!("{}{next}", dir.prefix());
        *next += 1;
        self.cache.insert((dir, go_type.clone()), name.clone());
        let signature = match dir {
            Dir::Encode => format!("func {name}(v {go_type}) schema.SchemaValue {{"),
            Dir::Decode => format!("func {name}(sv schema.SchemaValue) ({go_type}, error) {{"),
        };
        let mut lines = vec![signature];
        lines.extend(body.lines().map(|l| format!("\t{l}")));
        lines.push("}".to_string());
        self.helpers.push(lines.join("\n"));
        Ok(name)
    }

    /// A result arm's Go type and conversion; a unit arm has none.
    fn result_arm(
        &mut self,
        dir: Dir,
        typ: Option<&SchemaType>,
    ) -> anyhow::Result<(String, String)> {
        match typ {
            Some(typ) => Ok((
                self.generator.render(typ, &mut self.w)?,
                self.func(dir, typ)?,
            )),
            None => Ok(("struct{}".to_string(), "nil".to_string())),
        }
    }

    /// The conversion pair of a named type.
    fn write_named(&mut self, name: &str, typ: &SchemaType) -> anyhow::Result<()> {
        match typ {
            SchemaType::Record { fields, .. } => {
                let idents =
                    unique_idents(fields.iter().map(|f| to_field_ident(&f.name)).collect());
                let parts = fields
                    .iter()
                    .zip(&idents)
                    .map(|(f, ident)| (f.name.as_str(), ident.as_str(), &f.body))
                    .collect::<Vec<_>>();
                self.write_record(name, &parts, "v.")
            }
            SchemaType::Enum { cases, .. } => {
                self.w
                    .line(format!("func encode{name}(v {name}) schema.SchemaValue {{"));
                self.w.indent();
                self.w.line("return schema.EnumValue{Case: uint32(v)}");
                self.w.dedent();
                self.w.line("}");
                self.w.blank();
                self.w.line(format!(
                    "func decode{name}(sv schema.SchemaValue) ({name}, error) {{"
                ));
                self.w.indent();
                self.w.line(format!(
                    "c, err := bridge.EnumCase(sv, {}, {})",
                    cases.len(),
                    go_string(name)
                ));
                self.w.line(format!("return {name}(c), err"));
                self.w.dedent();
                self.w.line("}");
                self.w.blank();
                Ok(())
            }
            SchemaType::Flags { flags, .. } => {
                let idents = unique_idents(flags.iter().map(|f| to_field_ident(f)).collect());
                let set = idents
                    .iter()
                    .map(|i| format!("v.{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.w
                    .line(format!("func encode{name}(v {name}) schema.SchemaValue {{"));
                self.w.indent();
                self.w
                    .line(format!("return schema.FlagsValue{{Set: []bool{{{set}}}}}"));
                self.w.dedent();
                self.w.line("}");
                self.w.blank();
                self.w.line(format!(
                    "func decode{name}(sv schema.SchemaValue) ({name}, error) {{"
                ));
                self.w.indent();
                let call = format!("bridge.FlagBits(sv, {}, {})", flags.len(), go_string(name));
                if idents.is_empty() {
                    self.w.line(format!("_, err := {call}"));
                    self.w.line(format!("return {name}{{}}, err"));
                } else {
                    self.w.line(format!("set, err := {call}"));
                    self.w.line("if err != nil {");
                    self.w.indent();
                    self.w.line(format!("return {name}{{}}, err"));
                    self.w.dedent();
                    self.w.line("}");
                    let fields = idents
                        .iter()
                        .enumerate()
                        .map(|(idx, i)| format!("{i}: set[{idx}]"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.w.line(format!("return {name}{{{fields}}}, nil"));
                }
                self.w.dedent();
                self.w.line("}");
                self.w.blank();
                Ok(())
            }
            SchemaType::Variant { cases, .. } => {
                let idents = case_idents(name, cases.iter().map(|c| c.name.as_str()));
                let quoted = go_string(name);

                // Encode: a type switch over the sealed cases.
                let any_payload = cases.iter().any(|c| c.payload.is_some());
                let mut encode = Vec::with_capacity(cases.len());
                for (idx, (case, ident)) in cases.iter().zip(&idents).enumerate() {
                    encode.push(match &case.payload {
                        Some(payload) => format!(
                            "case {ident}:\n\treturn bridge.VariantCase({idx}, {}(v.Value))",
                            self.func(Dir::Encode, payload)?
                        ),
                        None => format!("case {ident}:\n\treturn bridge.VariantUnit({idx})"),
                    });
                }
                self.write_sum_encoder(name, any_payload, &encode);

                // Decode: by case index.
                let mut decode = Vec::with_capacity(cases.len());
                for (idx, (case, ident)) in cases.iter().zip(&idents).enumerate() {
                    let case_name = go_string(&case.name);
                    decode.push(match &case.payload {
                        Some(payload) => format!(
                            "case {idx}:\n\
                             \tbody, err := bridge.CasePayload(payload, {quoted}, {case_name})\n\
                             \tif err != nil {{\n\
                             \t\treturn nil, err\n\
                             \t}}\n\
                             \tvalue, err := {}(body)\n\
                             \tif err != nil {{\n\
                             \t\treturn nil, bridge.FieldError({quoted}, {case_name}, err)\n\
                             \t}}\n\
                             \treturn {ident}{{Value: value}}, nil",
                            self.func(Dir::Decode, payload)?
                        ),
                        None => format!(
                            "case {idx}:\n\
                             \tif err := bridge.CaseNoPayload(payload, {quoted}, {case_name}); err != nil {{\n\
                             \t\treturn nil, err\n\
                             \t}}\n\
                             \treturn {ident}{{}}, nil"
                        ),
                    });
                }
                self.w.line(format!(
                    "func decode{name}(sv schema.SchemaValue) ({name}, error) {{"
                ));
                self.w.indent();
                self.w.line(format!(
                    "c, payload, err := bridge.VariantParts(sv, {quoted})"
                ));
                self.w.line("if err != nil {");
                self.w.indent();
                self.w.line("return nil, err");
                self.w.dedent();
                self.w.line("}");
                if !decode.is_empty() {
                    self.w.line("switch c {");
                    for case in &decode {
                        self.w.line(case);
                    }
                    self.w.line("}");
                }
                if cases.iter().all(|c| c.payload.is_some()) {
                    self.w.line("_ = payload");
                }
                self.w
                    .line(format!("return nil, bridge.UnknownCase({quoted}, c)"));
                self.w.dedent();
                self.w.line("}");
                self.w.blank();
                Ok(())
            }
            SchemaType::Union { spec, .. } => {
                let idents = case_idents(name, spec.branches.iter().map(|b| b.tag.as_str()));
                let quoted = go_string(name);

                let mut encode = Vec::with_capacity(idents.len());
                for (branch, ident) in spec.branches.iter().zip(&idents) {
                    encode.push(format!(
                        "case {ident}:\n\treturn bridge.UnionBranch({}, {}(v.Value))",
                        go_string(&branch.tag),
                        self.func(Dir::Encode, &branch.body)?
                    ));
                }
                self.write_sum_encoder(name, !idents.is_empty(), &encode);

                let mut decode = Vec::with_capacity(idents.len());
                for (branch, ident) in spec.branches.iter().zip(&idents) {
                    let tag = go_string(&branch.tag);
                    decode.push(format!(
                        "case {tag}:\n\
                         \tvalue, err := {}(body)\n\
                         \tif err != nil {{\n\
                         \t\treturn nil, bridge.FieldError({quoted}, {tag}, err)\n\
                         \t}}\n\
                         \treturn {ident}{{Value: value}}, nil",
                        self.func(Dir::Decode, &branch.body)?
                    ));
                }
                self.w.line(format!(
                    "func decode{name}(sv schema.SchemaValue) ({name}, error) {{"
                ));
                self.w.indent();
                self.w
                    .line(format!("tag, body, err := bridge.UnionParts(sv, {quoted})"));
                self.w.line("if err != nil {");
                self.w.indent();
                self.w.line("return nil, err");
                self.w.dedent();
                self.w.line("}");
                if decode.is_empty() {
                    self.w.line("_ = body");
                } else {
                    self.w.line("switch tag {");
                    for case in &decode {
                        self.w.line(case);
                    }
                    self.w.line("}");
                }
                self.w
                    .line(format!("return nil, bridge.UnknownBranch({quoted}, tag)"));
                self.w.dedent();
                self.w.line("}");
                self.w.blank();
                Ok(())
            }
            other => anyhow::bail!("not a named Go declaration: {other:?}"),
        }
    }

    fn write_sum_encoder(&mut self, name: &str, binds: bool, cases: &[String]) {
        self.w
            .line(format!("func encode{name}(v {name}) schema.SchemaValue {{"));
        self.w.indent();
        if !cases.is_empty() {
            // Binding the case value is an error when no case reads it.
            if binds {
                self.w.line("switch v := v.(type) {");
            } else {
                self.w.line("switch v.(type) {");
            }
            for case in cases {
                self.w.line(case);
            }
            self.w.line("}");
        }
        self.w
            .line(format!("bridge.NotACase({}, v)", go_string(name)));
        self.w.line("return nil");
        self.w.dedent();
        self.w.line("}");
        self.w.blank();
    }

    /// A record's pair. `access` prefixes each field on the encoding side, so
    /// the same code serves a struct (`v.`) and a list of parameters (``).
    fn write_record(
        &mut self,
        name: &str,
        fields: &[(&str, &str, &SchemaType)],
        access: &str,
    ) -> anyhow::Result<()> {
        let mut encoders = Vec::with_capacity(fields.len());
        let mut decoders = Vec::with_capacity(fields.len());
        for (_, _, typ) in fields {
            encoders.push(self.func(Dir::Encode, typ)?);
            decoders.push(self.func(Dir::Decode, typ)?);
        }
        let quoted = go_string(name);

        self.w
            .line(format!("func encode{name}(v {name}) schema.SchemaValue {{"));
        self.w.indent();
        write_record_value(
            fields
                .iter()
                .zip(&encoders)
                .map(|((_, ident, _), enc)| format!("{enc}({access}{ident})")),
            &mut self.w,
        );
        self.w.dedent();
        self.w.line("}");
        self.w.blank();

        self.w.line(format!(
            "func decode{name}(sv schema.SchemaValue) ({name}, error) {{"
        ));
        self.w.indent();
        self.w.line(format!("var out {name}"));
        let call = format!("bridge.RecordFields(sv, {}, {quoted})", fields.len());
        if fields.is_empty() {
            self.w.line(format!("_, err := {call}"));
            self.w.line("return out, err");
        } else {
            self.w.line(format!("fields, err := {call}"));
            self.w.line("if err != nil {");
            self.w.indent();
            self.w.line("return out, err");
            self.w.dedent();
            self.w.line("}");
            for (idx, ((field, ident, _), dec)) in fields.iter().zip(&decoders).enumerate() {
                self.w.line(format!(
                    "if out.{ident}, err = {dec}(fields[{idx}]); err != nil {{"
                ));
                self.w.indent();
                self.w.line(format!(
                    "return out, bridge.FieldError({quoted}, {}, err)",
                    go_string(field)
                ));
                self.w.dedent();
                self.w.line("}");
            }
            self.w.line("return out, nil");
        }
        self.w.dedent();
        self.w.line("}");
        self.w.blank();
        Ok(())
    }

    /// Writes every helper collected so far.
    fn flush(&mut self) {
        for helper in self.helpers.drain(..) {
            self.w.line(helper);
            self.w.blank();
        }
    }
}

/// `return schema.RecordValue{...}` over already-encoded field expressions.
fn write_record_value(fields: impl Iterator<Item = String>, w: &mut GoWriter) {
    let fields = fields.collect::<Vec<_>>();
    if fields.is_empty() {
        w.line("return schema.RecordValue{Fields: []schema.SchemaValue{}}");
        return;
    }
    w.line("return schema.RecordValue{Fields: []schema.SchemaValue{");
    w.indent();
    for field in fields {
        w.line(format!("{field},"));
    }
    w.dedent();
    w.line("}}");
}

impl GoBridgeGenerator {
    pub(super) fn external_go_mod(&self) -> anyhow::Result<String> {
        let overrides = sdk_overrides()?;
        let version = overrides.go_sdk_dep();
        let mut out = format!(
            "// Code generated by golem-cli. DO NOT EDIT.\n\
             //\n\
             // A generated bridge client. A consumer requires this module and\n\
             // points a replace at the directory it was generated into.\n\
             module {module}\n\n\
             go {go}\n\n\
             require (\n\
             \t{GO_BRIDGE_MODULE} {version}\n\
             \t{GO_CORE_MODULE} {version}\n\
             )\n",
            module = self.module_path(),
            go = versions::build_tool::GO_MIN,
        );
        let replace = overrides.go_bridge_replace();
        if !replace.is_empty() {
            out.push_str(&replace);
            out.push('\n');
        }
        Ok(out)
    }

    /// The client and its conversions: `client.go` and `codec.go`. They are
    /// made together because the conversions are shared, and a helper first
    /// needed by the client lands in the codec file beside the rest.
    pub(super) fn external_files(&self) -> anyhow::Result<(String, String)> {
        let mut w = GoWriter::new();
        let mut codecs = Codecs::new(self);
        w.import(BRIDGE_PKG);
        let n = &self.names;
        let agent_name = self.agent_type.type_name.as_str();

        // The id: the constructor's arguments, which identify an instance.
        let id_fields = user_supplied_fields(&self.agent_type.constructor.input_schema);
        self.write_input_struct(
            &n.id,
            &format!(
                "{} identifies a {agent_name} instance: its constructor arguments.",
                n.id
            ),
            &self.agent_type.constructor.input_schema,
            &mut w,
        )?;
        let id_idents = unique_idents(id_fields.iter().map(|f| to_field_ident(&f.name)).collect());
        let parts = id_fields
            .iter()
            .zip(&id_idents)
            .map(|(f, ident)| (f.name.as_str(), ident.as_str(), &f.schema))
            .collect::<Vec<_>>();
        // Only the encoder is used; the decoder is written for symmetry with
        // the named types and costs nothing once compiled away.
        codecs.write_record(&n.id, &parts, "v.")?;

        w.doc(&format!(
            "{} calls a {agent_name} agent through the Golem REST API.\n\
             Every call returns an error rather than panicking: a transport failure,\n\
             an error status and a result that does not decode are all reported.",
            n.client
        ));
        w.line(format!("type {} struct{{ agent *bridge.Agent }}", n.client));
        w.blank();

        w.doc(&format!(
            "{} returns a client for the {agent_name} instance identified by id. The\n\
             instance is created on first use. Options select the server\n\
             (bridge.WithConfiguration, otherwise the one set with bridge.Configure),\n\
             a phantom instance (bridge.WithPhantomID, bridge.WithNewPhantomID) and\n\
             configuration overrides (bridge.WithConfig).",
            n.get
        ));
        w.line(format!(
            "func {}(id {}, opts ...bridge.AgentOption) ({}, error) {{",
            n.get, n.id, n.client
        ));
        w.indent();
        w.line(format!(
            "agent, err := bridge.NewAgent({}, encode{}(id), opts...)",
            go_string(agent_name),
            n.id
        ));
        w.line(format!("return {}{{agent: agent}}, err", n.client));
        w.dedent();
        w.line("}");
        w.blank();

        w.doc("Agent is the underlying bridge agent: its type, id and resolved server.");
        w.line(format!("func (c {}) Agent() *bridge.Agent {{", n.client));
        w.indent();
        w.line("return c.agent");
        w.dedent();
        w.line("}");
        w.blank();

        let names = ExternalMethodNames::new(&n.methods);
        for (idx, method) in self.agent_type.methods.iter().enumerate() {
            self.write_external_method(idx, method, &names, &mut codecs, &mut w)?;
        }

        for (typ, name) in self.type_naming.types() {
            codecs.write_named(&name.name, self.resolve(typ))?;
        }
        codecs.flush();
        let mut codec = codecs.w;
        codec.import(BRIDGE_PKG);
        codec.import(SCHEMA_PKG);
        Ok((
            w.finish(&self.package_name()),
            codec.finish(&self.package_name()),
        ))
    }

    fn write_external_method(
        &self,
        idx: usize,
        method: &AgentMethodSchema,
        names: &ExternalMethodNames,
        codecs: &mut Codecs<'_>,
        w: &mut GoWriter,
    ) -> anyhow::Result<()> {
        let n = &self.names;
        let fields = user_supplied_fields(&method.input_schema);
        let mut params = Vec::with_capacity(fields.len());
        let mut encoders = Vec::with_capacity(fields.len());
        for field in &fields {
            params.push(self.render(&field.schema, w)?);
            encoders.push(codecs.func(Dir::Encode, &field.schema)?);
        }
        // Parameter names avoid the receiver, the other locals and the
        // packages the body refers to.
        let param_idents = unique_idents_with_reserved(
            fields.iter().map(|f| to_param_ident(&f.name)).collect(),
            &["c", "ctx", "when", "bridge", "schema", "context", "time"],
        );
        let mut signature = vec!["ctx context.Context".to_string()];
        signature.extend(
            param_idents
                .iter()
                .zip(&params)
                .map(|(name, typ)| format!("{name} {typ}")),
        );
        w.import("context");
        w.import(SCHEMA_PKG);

        let method_name = go_string(&method.name);
        let doc_tail = if method.description.trim().is_empty() {
            format!("calls {}", method.name)
        } else {
            lower_first(method.description.trim().trim_end_matches('.'))
        };

        let encoded = param_idents
            .iter()
            .zip(&encoders)
            .map(|(p, enc)| format!("{enc}({p})"))
            .collect::<Vec<_>>();
        let params_value = if encoded.is_empty() {
            "schema.RecordValue{}".to_string()
        } else {
            format!(
                "schema.RecordValue{{Fields: []schema.SchemaValue{{{}}}}}",
                encoded.join(", ")
            )
        };

        // Await.
        w.doc(&format!("{} {doc_tail}.", names.call[idx]));
        match &method.output_schema {
            OutputSchema::Unit => {
                w.line(format!(
                    "func (c {}) {}({}) error {{",
                    n.client,
                    names.call[idx],
                    signature.join(", ")
                ));
                w.indent();
                w.line(format!(
                    "_, err := c.agent.Invoke(ctx, {method_name}, {params_value})"
                ));
                w.line("return err");
            }
            OutputSchema::Single(typ) => {
                let output = self.render(typ, w)?;
                let decode = codecs.func(Dir::Decode, typ)?;
                w.line(format!(
                    "func (c {}) {}({}) ({output}, error) {{",
                    n.client,
                    names.call[idx],
                    signature.join(", ")
                ));
                w.indent();
                w.line(format!(
                    "return bridge.Call(ctx, c.agent, {method_name}, {params_value}, {decode})"
                ));
            }
        }
        w.dedent();
        w.line("}");
        w.blank();

        // Trigger: enqueue and return.
        w.doc(&format!(
            "{} enqueues {} without waiting for it to run.",
            names.trigger[idx], method.name
        ));
        w.line(format!(
            "func (c {}) {}({}) (bridge.Receipt, error) {{",
            n.client,
            names.trigger[idx],
            signature.join(", ")
        ));
        w.indent();
        w.line(format!(
            "return c.agent.Trigger(ctx, {method_name}, {params_value})"
        ));
        w.dedent();
        w.line("}");
        w.blank();

        // Schedule: enqueue for a time.
        w.import("time");
        let mut schedule_signature = signature.clone();
        schedule_signature.insert(1, "when time.Time".to_string());
        w.doc(&format!(
            "{} enqueues {} to run at when.",
            names.schedule[idx], method.name
        ));
        w.line(format!(
            "func (c {}) {}({}) (bridge.Receipt, error) {{",
            n.client,
            names.schedule[idx],
            schedule_signature.join(", ")
        ));
        w.indent();
        w.line(format!(
            "return c.agent.ScheduleAt(ctx, {method_name}, {params_value}, when)"
        ));
        w.dedent();
        w.line("}");
        w.blank();
        Ok(())
    }
}

/// The three client methods each agent method produces. They share the
/// client's method set with each other and with `Agent`, so a schema method
/// called `trigger-x` must not take the name of `x`'s trigger.
struct ExternalMethodNames {
    call: Vec<String>,
    trigger: Vec<String>,
    schedule: Vec<String>,
}

impl ExternalMethodNames {
    fn new(methods: &[String]) -> Self {
        let count = methods.len();
        let mut all = methods.to_vec();
        all.extend(methods.iter().map(|m| format!("Trigger{m}")));
        all.extend(methods.iter().map(|m| format!("Schedule{m}")));
        let mut unique = unique_idents_with_reserved(all, &["Agent"]);
        let schedule = unique.split_off(2 * count);
        let trigger = unique.split_off(count);
        Self {
            call: unique,
            trigger,
            schedule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn a_method_named_like_a_trigger_keeps_its_name() {
        let names = ExternalMethodNames::new(&["Poll".to_string(), "TriggerPoll".to_string()]);
        assert_eq!(names.call, ["Poll", "TriggerPoll"]);
        assert_eq!(names.trigger, ["TriggerPoll2", "TriggerTriggerPoll"]);
        assert_eq!(names.schedule, ["SchedulePoll", "ScheduleTriggerPoll"]);
    }

    #[test]
    fn a_method_named_agent_does_not_shadow_the_accessor() {
        let names = ExternalMethodNames::new(&["Agent".to_string()]);
        assert_eq!(names.call, ["Agent2"]);
    }
}
