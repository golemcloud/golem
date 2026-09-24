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

//! Go bridge SDK generator.
//!
//! Each generated client is a Go module of its own, named
//! `golem.local/bridge/<client-dir>`. The `.local` suffix can never resolve on a
//! module proxy, so a consumer that forgets the `replace` pointing at the
//! generated directory fails loudly instead of fetching something else.
//!
//! The generated **types** are the same in both modes, because the value
//! vocabulary they are built from lives in the shared `core/values` package. What
//! differs is how a call is made:
//!
//! - **Guest** mode sits directly on the guest SDK. The target is declared with
//!   `golem.DeclareRemoteAgent`, each method with a typed descriptor, and every
//!   conversion is done by the SDK's own reflective codec — so the generator
//!   emits no codec at all. The sum types are registered with the SDK
//!   (`DefineVariant`, `DefineEnum`, `DefineFlags`, `DefineUnion`), which is also
//!   what a hand-written Go agent would write.
//! - **External** mode calls through the `bridge` runtime over REST.
//!
//! RPC values are positional, so a generated field or parameter name never has
//! to match the schema's: only order and type travel.

pub mod decl;
#[allow(clippy::module_inception)]
pub mod go;
pub mod go_writer;
pub mod type_name;
pub mod type_ref;

pub use type_name::GoTypeName;

use crate::bridge_gen::go::go::{
    go_string, lower_first, to_exported_ident, to_field_ident, to_param_ident, unique_idents,
    unique_idents_with_reserved,
};
use crate::bridge_gen::go::go_writer::GoWriter;
use crate::bridge_gen::type_naming::{TypeNaming, user_supplied_fields};
use crate::bridge_gen::{
    BridgeGenerator, BridgeMode, bridge_client_directory_name,
    validate_host_managed_agent_bridge_policy,
};
use crate::fs;
use crate::sdk_overrides::{GO_CORE_MODULE, GO_SDK_MODULE, sdk_overrides};
use crate::versions;
use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::schema::agent::contains_stream_in_graph;
use golem_common::schema::schema_type::{DiscriminatorRule, SchemaType};
use golem_common::schema::{AgentTypeSchema, InputSchema, OutputSchema};

/// Import path of the guest SDK.
pub const GOLEM_PKG: &str = GO_SDK_MODULE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoBridgeMode {
    ExternalRest,
    GuestWasmRpc,
}

impl GoBridgeMode {
    fn bridge_mode(self) -> BridgeMode {
        match self {
            GoBridgeMode::ExternalRest => BridgeMode::External,
            GoBridgeMode::GuestWasmRpc => BridgeMode::Guest,
        }
    }
}

pub struct GoBridgeGenerator {
    target_path: Utf8PathBuf,
    agent_type: AgentTypeSchema,
    mode: GoBridgeMode,
    type_naming: TypeNaming<GoTypeName>,
    names: AgentNames,
}

/// The package-level names the generator emits for the agent itself. They
/// share Go's single package namespace with the generated types, so they are
/// reserved before any type is named.
struct AgentNames {
    /// The agent's exported stem, e.g. `CounterAgent`.
    agent: String,
    id: String,
    client: String,
    get: String,
    new_phantom: String,
    /// Per method, in schema order: the input struct's name.
    inputs: Vec<String>,
    /// Per method, in schema order: the Go method name on the client.
    methods: Vec<String>,
}

impl AgentNames {
    fn new(agent_type: &AgentTypeSchema) -> Self {
        let agent = to_exported_ident(agent_type.type_name.as_str());
        let methods = unique_idents_with_reserved(
            agent_type
                .methods
                .iter()
                .map(|m| to_exported_ident(&m.name))
                .collect(),
            // Methods on the client struct that a schema method must not
            // shadow.
            &["Id"],
        );
        let inputs = methods.iter().map(|m| format!("{agent}{m}Input")).collect();
        Self {
            id: format!("{agent}Id"),
            client: format!("{agent}Client"),
            get: format!("Get{agent}"),
            new_phantom: format!("NewPhantom{agent}"),
            agent,
            inputs,
            methods,
        }
    }

    fn reserved(&self) -> Vec<String> {
        let mut out = vec![
            self.id.clone(),
            self.client.clone(),
            self.get.clone(),
            self.new_phantom.clone(),
        ];
        out.extend(self.inputs.iter().cloned());
        out
    }
}

impl BridgeGenerator for GoBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        _testing: bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode(agent_type, target_path, GoBridgeMode::ExternalRest)
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        if !self.target_path.exists() {
            fs::create_dir_all(&self.target_path)?;
        }
        match self.mode {
            GoBridgeMode::GuestWasmRpc => {
                self.write_file("go.mod", self.go_mod()?)?;
                self.write_file("types.go", self.types_file()?)?;
                self.write_file("registry.go", self.registry_file()?)?;
                self.write_file("client.go", self.guest_client_file()?)?;
                Ok(())
            }
            GoBridgeMode::ExternalRest => bail!(
                "the Go external bridge is not generated yet; only guest bridges are supported"
            ),
        }
    }
}

impl GoBridgeGenerator {
    pub fn new_with_mode(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        mode: GoBridgeMode,
    ) -> anyhow::Result<Self> {
        validate_host_managed_agent_bridge_policy(&agent_type, mode.bridge_mode())?;
        if mode == GoBridgeMode::GuestWasmRpc && agent_uses_streams(&agent_type) {
            bail!(
                "the Go guest bridge does not generate stream-bearing methods yet ({})",
                agent_type.type_name.as_str()
            );
        }

        let names = AgentNames::new(&agent_type);
        let same_language = agent_type.source_language.eq_ignore_ascii_case("go");
        let type_naming = TypeNaming::new_with_reserved_names(
            &agent_type,
            same_language,
            names.reserved().into_iter().map(GoTypeName::from),
        )?;

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            mode,
            type_naming,
            names,
        })
    }

    /// The directory a generated client lives in, which also names its module.
    pub fn client_dir_name(&self) -> String {
        bridge_client_directory_name(&self.agent_type.type_name, self.mode.bridge_mode())
    }

    /// The Go package name: the client directory with everything but letters
    /// and digits removed, since a package name must be an identifier.
    pub fn package_name(&self) -> String {
        self.client_dir_name()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    }

    pub fn module_path(&self) -> String {
        format!("golem.local/bridge/{}", self.client_dir_name())
    }

    fn write_file(&self, name: &str, content: String) -> anyhow::Result<()> {
        let path = self.target_path.join(name);
        fs::write(&path, content).with_context(|| format!("failed to write {path}"))
    }

    fn resolve<'a>(&'a self, typ: &'a SchemaType) -> &'a SchemaType {
        self.type_naming
            .graph()
            .resolve_ref(typ)
            .expect("bridge schemas contain only resolvable references")
    }

    /// The generated name of a schema type that has a named declaration, if it
    /// does. A reference resolves to its definition's name.
    fn named(&self, typ: &SchemaType) -> Option<String> {
        if let Some(name) = self.type_naming.type_name_for_type(typ) {
            return Some(name.name.clone());
        }
        let resolved = self.resolve(typ);
        if resolved != typ {
            return self
                .type_naming
                .type_name_for_type(resolved)
                .map(|n| n.name.clone());
        }
        None
    }

    fn render(&self, typ: &SchemaType, writer: &mut GoWriter) -> anyhow::Result<String> {
        type_ref::render(typ, &|t| self.named(t), &|t| self.resolve(t), writer)
    }

    // --- go.mod ---------------------------------------------------------

    fn go_mod(&self) -> anyhow::Result<String> {
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
             \t{GO_SDK_MODULE} {version}\n\
             \t{GO_CORE_MODULE} {version}\n\
             )\n",
            module = self.module_path(),
            go = versions::build_tool::GO_MIN,
        );
        // A replace in a dependency's go.mod is ignored, so these only matter
        // when the client is built on its own — which is how it is tested.
        let replace = overrides.go_sdk_replace();
        if !replace.is_empty() {
            out.push_str(&replace);
            out.push('\n');
        }
        Ok(out)
    }

    // --- types.go -------------------------------------------------------

    fn types_file(&self) -> anyhow::Result<String> {
        let mut writer = GoWriter::new();
        let render = |t: &SchemaType, w: &mut GoWriter| self.render(t, w);
        for (typ, name) in self.type_naming.types() {
            let body = self.resolve(typ);
            decl::write(&name.name, body, &render, &mut writer)?;
        }
        Ok(writer.finish(&self.package_name()))
    }

    // --- registry.go ----------------------------------------------------

    /// Registers each generated sum type with the guest SDK, exactly as a
    /// hand-written Go agent would. Without it the SDK would read a variant's
    /// interface as unsupported and a flags struct as a record of booleans.
    fn registry_file(&self) -> anyhow::Result<String> {
        let mut writer = GoWriter::new();
        let mut any = false;
        for (typ, name) in self.type_naming.types() {
            let name = &name.name;
            match self.resolve(typ) {
                SchemaType::Enum { cases, .. } => {
                    writer.import(GOLEM_PKG);
                    let args = cases.iter().map(|c| go_string(c)).collect::<Vec<_>>();
                    writer.line(format!(
                        "var _ = golem.DefineEnum[{name}]({})",
                        args.join(", ")
                    ));
                    writer.blank();
                    any = true;
                }
                SchemaType::Flags { .. } => {
                    writer.import(GOLEM_PKG);
                    writer.line(format!("var _ = golem.DefineFlags[{name}]()"));
                    writer.blank();
                    any = true;
                }
                SchemaType::Variant { cases, .. } => {
                    writer.import(GOLEM_PKG);
                    let idents = case_idents(name, cases.iter().map(|c| c.name.as_str()));
                    writer.line(format!("var _ = golem.DefineVariant[{name}]("));
                    writer.indent();
                    for (case, ident) in cases.iter().zip(&idents) {
                        let ctor = if case.payload.is_some() {
                            "WrappedCase"
                        } else {
                            "Case"
                        };
                        writer.line(format!("golem.{ctor}[{ident}]({}),", go_string(&case.name)));
                    }
                    writer.dedent();
                    writer.line(")");
                    writer.blank();
                    any = true;
                }
                SchemaType::Union { spec, .. } => {
                    writer.import(GOLEM_PKG);
                    let idents = case_idents(name, spec.branches.iter().map(|b| b.tag.as_str()));
                    writer.line(format!("var _ = golem.DefineUnion[{name}]("));
                    writer.indent();
                    for (branch, ident) in spec.branches.iter().zip(&idents) {
                        writer.line(format!(
                            "golem.WrappedBranch[{ident}]({}, {}),",
                            go_string(&branch.tag),
                            discriminator(&branch.discriminator)
                        ));
                    }
                    writer.dedent();
                    writer.line(")");
                    writer.blank();
                    any = true;
                }
                _ => {}
            }
        }
        if !any {
            writer.line("// There is no sum type to register.");
        }
        Ok(writer.finish(&self.package_name()))
    }

    // --- client.go (guest) ----------------------------------------------

    fn guest_client_file(&self) -> anyhow::Result<String> {
        let mut writer = GoWriter::new();
        writer.import(GOLEM_PKG);
        let n = &self.names;
        let agent_name = self.agent_type.type_name.as_str();
        let remote = format!("{}Remote", lower_first(&n.agent));

        // The id: the constructor's arguments, which identify an instance.
        self.write_input_struct(
            &n.id,
            &format!(
                "{} identifies a {agent_name} instance: its constructor arguments.",
                n.id
            ),
            &self.agent_type.constructor.input_schema,
            &mut writer,
        )?;

        writer.line(format!(
            "var {remote} = golem.DeclareRemoteAgent[{}]({})",
            n.id,
            go_string(agent_name)
        ));
        writer.blank();

        // One input struct and one descriptor per method.
        let mut outputs = Vec::with_capacity(self.agent_type.methods.len());
        for (idx, method) in self.agent_type.methods.iter().enumerate() {
            let input = &n.inputs[idx];
            self.write_input_struct(
                input,
                &format!("{input} holds the arguments of {}.", method.name),
                &method.input_schema,
                &mut writer,
            )?;
            let output = match &method.output_schema {
                OutputSchema::Unit => "golem.Unit".to_string(),
                OutputSchema::Single(typ) => self.render(typ, &mut writer)?,
            };
            writer.line(format!(
                "var {} = {remote}.Method[{input}, {output}]({})",
                descriptor_var(&n.agent, &n.methods[idx]),
                go_string(&method.name)
            ));
            writer.blank();
            outputs.push(output);
        }

        // The client.
        writer.doc(&format!(
            "{} calls a {agent_name} agent. A failed call panics with the SDK's own\n\
             error, the same as golem.MethodDef.Call, so its classification survives.",
            n.client
        ));
        writer.line(format!(
            "type {} struct{{ client golem.Client[{}] }}",
            n.client, n.id
        ));
        writer.blank();

        writer.doc(&format!(
            "{} returns a client for the {agent_name} instance identified by id,\n\
             creating it if it does not exist yet.",
            n.get
        ));
        // Always multi-line: gofmt keeps a one-line body only below a size
        // limit, and an agent's name decides which side of it this falls on.
        writer.line(format!("func {}(id {}) {} {{", n.get, n.id, n.client));
        writer.indent();
        writer.line(format!("return {}{{client: {remote}.Get(id)}}", n.client));
        writer.dedent();
        writer.line("}");
        writer.blank();

        writer.doc(&format!(
            "{} allocates a fresh phantom {agent_name} instance.",
            n.new_phantom
        ));
        writer.line(format!(
            "func {}(id {}) {} {{",
            n.new_phantom, n.id, n.client
        ));
        writer.indent();
        writer.line(format!(
            "return {}{{client: {remote}.NewPhantom(id)}}",
            n.client
        ));
        writer.dedent();
        writer.line("}");
        writer.blank();

        for (idx, method) in self.agent_type.methods.iter().enumerate() {
            self.write_guest_method(idx, method, &outputs[idx], &mut writer)?;
        }

        Ok(writer.finish(&self.package_name()))
    }

    /// A struct holding the caller-supplied fields of an input schema, in
    /// schema order. Fields the host injects — the principal — are left out: a
    /// caller neither knows nor can override them.
    fn write_input_struct(
        &self,
        name: &str,
        doc: &str,
        input: &InputSchema,
        writer: &mut GoWriter,
    ) -> anyhow::Result<()> {
        let fields = user_supplied_fields(input);
        let mut rendered = Vec::with_capacity(fields.len());
        for field in &fields {
            rendered.push(self.render(&field.schema, writer)?);
        }
        let idents = unique_idents(fields.iter().map(|f| to_field_ident(&f.name)).collect());
        writer.doc(doc);
        if fields.is_empty() {
            writer.line(format!("type {name} struct{{}}"));
        } else {
            let width = idents.iter().map(|i| i.len()).max().unwrap_or(0);
            writer.line(format!("type {name} struct {{"));
            writer.indent();
            for (ident, typ) in idents.iter().zip(&rendered) {
                writer.line(format!("{ident:<width$} {typ}"));
            }
            writer.dedent();
            writer.line("}");
        }
        writer.blank();
        Ok(())
    }

    fn write_guest_method(
        &self,
        idx: usize,
        method: &golem_common::schema::AgentMethodSchema,
        output: &str,
        writer: &mut GoWriter,
    ) -> anyhow::Result<()> {
        let n = &self.names;
        let fields = user_supplied_fields(&method.input_schema);
        let mut params = Vec::with_capacity(fields.len());
        for field in &fields {
            params.push(self.render(&field.schema, writer)?);
        }
        // Parameter names avoid the receiver and the locals the body uses.
        let param_idents = unique_idents_with_reserved(
            fields.iter().map(|f| to_param_ident(&f.name)).collect(),
            &["c", "golem"],
        );
        let field_idents = unique_idents(fields.iter().map(|f| to_field_ident(&f.name)).collect());

        if !method.description.trim().is_empty() {
            writer.doc(&format!(
                "{} {}",
                n.methods[idx],
                lower_first(method.description.trim())
            ));
        } else {
            writer.doc(&format!("{} calls {}.", n.methods[idx], method.name));
        }

        let signature = param_idents
            .iter()
            .zip(&params)
            .map(|(name, typ)| format!("{name} {typ}"))
            .collect::<Vec<_>>()
            .join(", ");
        let input = format!(
            "{}{{{}}}",
            n.inputs[idx],
            field_idents
                .iter()
                .zip(&param_idents)
                .map(|(f, p)| format!("{f}: {p}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let call = format!(
            "{}.Call(c.client, {input})",
            descriptor_var(&n.agent, &n.methods[idx])
        );

        match &method.output_schema {
            OutputSchema::Unit => {
                writer.line(format!(
                    "func (c {}) {}({signature}) {{",
                    n.client, n.methods[idx]
                ));
                writer.indent();
                writer.line(call);
            }
            OutputSchema::Single(_) => {
                writer.line(format!(
                    "func (c {}) {}({signature}) {output} {{",
                    n.client, n.methods[idx]
                ));
                writer.indent();
                writer.line(format!("return {call}"));
            }
        }
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }
}

/// True when any constructor or method schema of the agent carries a stream.
fn agent_uses_streams(agent_type: &AgentTypeSchema) -> bool {
    let graph = &agent_type.schema;
    let in_input = |input: &InputSchema| {
        user_supplied_fields(input)
            .iter()
            .any(|f| contains_stream_in_graph(graph, &f.schema))
    };
    in_input(&agent_type.constructor.input_schema)
        || agent_type.methods.iter().any(|m| {
            in_input(&m.input_schema)
                || matches!(&m.output_schema, OutputSchema::Single(t) if contains_stream_in_graph(graph, t))
        })
}

/// The per-case type names a variant or union declaration emits, which the
/// registration has to name identically.
fn case_idents<'a>(type_name: &str, cases: impl Iterator<Item = &'a str>) -> Vec<String> {
    unique_idents(
        cases
            .map(|case| format!("{type_name}{}", to_exported_ident(case)))
            .collect(),
    )
}

/// The unexported variable a method's descriptor is bound to.
fn descriptor_var(agent: &str, method: &str) -> String {
    format!("{}{method}Method", lower_first(agent))
}

/// The guest SDK constructor for a union discriminator.
fn discriminator(rule: &DiscriminatorRule) -> String {
    match rule {
        DiscriminatorRule::Prefix { prefix } => format!("golem.Prefix({})", go_string(prefix)),
        DiscriminatorRule::Suffix { suffix } => format!("golem.Suffix({})", go_string(suffix)),
        DiscriminatorRule::Contains { substring } => {
            format!("golem.Contains({})", go_string(substring))
        }
        DiscriminatorRule::Regex { regex } => format!("golem.Matches({})", go_string(regex)),
        DiscriminatorRule::FieldEquals(field) => match &field.literal {
            Some(literal) => format!(
                "golem.FieldEquals({}, {})",
                go_string(&field.field_name),
                go_string(literal)
            ),
            None => format!("golem.FieldPresent({})", go_string(&field.field_name)),
        },
        DiscriminatorRule::FieldAbsent { field_name } => {
            format!("golem.FieldAbsent({})", go_string(field_name))
        }
    }
}
