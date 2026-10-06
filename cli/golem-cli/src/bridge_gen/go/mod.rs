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
//!   `golem.DefineFullAgentClient`, each method with a typed descriptor, and every
//!   conversion is done by the SDK's own reflective codec — so the generator
//!   emits no codec at all. The sum types are registered with the SDK
//!   (`DefineVariant`, `DefineEnum`, `DefineFlags`, `DefineUnion`), which is also
//!   what a hand-written Go agent would write.
//! - **External** mode calls through the `bridge` runtime over REST. With no
//!   SDK beneath it, the generator writes the value conversions itself; see
//!   [`external`].
//!
//! RPC values are positional, so a generated field or parameter name never has
//! to match the schema's: only order and type travel.

pub mod decl;
pub mod external;
#[allow(clippy::module_inception)]
pub mod go;
pub mod go_writer;
pub mod quantity;
pub mod tool;
pub mod type_name;
pub mod type_ref;

pub use type_name::GoTypeName;

use crate::bridge_gen::go::go::{
    go_string, lower_first, to_exported_ident, to_field_ident, to_param_ident, unique_idents,
    unique_idents_with_reserved,
};
use crate::bridge_gen::go::go_writer::GoWriter;
use crate::bridge_gen::go::type_ref::{VALUES, VALUES_PKG};
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
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::agent::contains_stream_in_graph;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{DiscriminatorRule, SchemaType};
use golem_common::schema::{AgentMethodSchema, AgentTypeSchema, InputSchema, OutputSchema};

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
    quantity_units: quantity::QuantityUnits,
    /// The client directory when it is not derived from the agent type: a tool
    /// client is named after the tool.
    client_dir: Option<String>,
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
    /// The typed configuration struct and its option, when the agent declares
    /// local configuration.
    config: Option<(String, String)>,
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
            config: agent_type
                .config
                .iter()
                .any(|c| c.source == AgentConfigSource::Local)
                .then(|| (format!("{agent}Config"), format!("With{agent}Config"))),
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
        if let Some((config, option)) = &self.config {
            out.push(config.clone());
            out.push(option.clone());
        }
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
            GoBridgeMode::ExternalRest => {
                self.write_file("go.mod", self.external_go_mod()?)?;
                self.write_file("types.go", self.types_file()?)?;
                let (client, codec) = self.external_files()?;
                self.write_file("codec.go", codec)?;
                self.write_file("client.go", client)?;
                Ok(())
            }
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
        if input_uses_streams(&agent_type.schema, &agent_type.constructor.input_schema) {
            bail!(
                "the Go bridge cannot generate a client for {}: its constructor takes a stream",
                agent_type.type_name.as_str()
            );
        }
        let names = AgentNames::new(&agent_type);
        let quantity_units = quantity::QuantityUnits::collect(&agent_type, &names.reserved());
        let same_language = agent_type.source_language.eq_ignore_ascii_case("go");
        let type_naming = TypeNaming::new_with_reserved_names(
            &agent_type,
            same_language,
            names
                .reserved()
                .into_iter()
                .chain(quantity_units.names())
                .map(GoTypeName::from),
        )?;

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            mode,
            type_naming,
            names,
            quantity_units,
            client_dir: None,
        })
    }

    /// A guest generator for the types of a tool client: the agent type is the
    /// tool's synthetic one, the directory is the tool client's, and the names
    /// the tool client declares are reserved before any type is named.
    pub(crate) fn new_tool_guest(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        client_dir: String,
        reserved: Vec<String>,
    ) -> anyhow::Result<Self> {
        let names = AgentNames::new(&agent_type);
        let quantity_units = quantity::QuantityUnits::collect(&agent_type, &reserved);
        let type_naming = TypeNaming::new_with_reserved_names(
            &agent_type,
            false,
            reserved
                .into_iter()
                .chain(quantity_units.names())
                .map(GoTypeName::from),
        )?;
        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            mode: GoBridgeMode::GuestWasmRpc,
            type_naming,
            names,
            quantity_units,
            client_dir: Some(client_dir),
        })
    }

    /// The directory a generated client lives in, which also names its module.
    pub fn client_dir_name(&self) -> String {
        match &self.client_dir {
            Some(dir) => dir.clone(),
            None => {
                bridge_client_directory_name(&self.agent_type.type_name, self.mode.bridge_mode())
            }
        }
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
    ///
    /// A quantity's name is that of its unit marker.
    fn named(&self, typ: &SchemaType) -> Option<String> {
        if let SchemaType::Quantity { spec, .. } = self.resolve(typ) {
            return self.quantity_units.name_for(spec).map(str::to_string);
        }
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
        let streams = match self.mode {
            GoBridgeMode::GuestWasmRpc => type_ref::Streams::Guest,
            GoBridgeMode::ExternalRest => type_ref::Streams::External,
        };
        type_ref::render(
            typ,
            &|t| self.named(t),
            &|t| self.resolve(t),
            streams,
            writer,
        )
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
        self.quantity_units.write(&mut writer);
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

    /// The option overriding configuration, as a doc reference.
    fn config_option_hint(&self) -> String {
        match &self.names.config {
            Some((_, option)) => format!(" ({option})"),
            None => " (golem.WithConfigEntries)".to_string(),
        }
    }

    /// The local configuration declarations a caller may override.
    fn local_configs(&self) -> Vec<&AgentConfigDeclarationSchema> {
        self.agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .collect()
    }

    /// The typed configuration struct's field names, one per local
    /// declaration: the path segments joined.
    fn config_field_idents(&self) -> Vec<String> {
        unique_idents(
            self.local_configs()
                .iter()
                .map(|c| c.path.iter().map(|s| to_field_ident(s)).collect())
                .collect(),
        )
    }

    /// The typed configuration struct, with one optional field per local
    /// declaration. Shared by both modes; only the option applying it differs.
    fn write_config_struct(&self, writer: &mut GoWriter) -> anyhow::Result<()> {
        let Some((config, _)) = &self.names.config else {
            return Ok(());
        };
        writer.doc(&format!(
            "{config} overrides {}'s configuration; unset fields keep the\n\
             provisioned values.",
            self.agent_type.type_name.as_str()
        ));
        writer.line(format!("type {config} struct {{"));
        writer.indent();
        for (decl, ident) in self.local_configs().iter().zip(self.config_field_idents()) {
            let typ = self.render(&decl.value_type, writer)?;
            writer.import(VALUES_PKG);
            writer.line(format!("// {}", decl.path.join(".")));
            writer.line(format!("{ident} {VALUES}.Option[{typ}]"));
        }
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

    fn write_guest_config(&self, writer: &mut GoWriter) -> anyhow::Result<()> {
        let Some((config, option)) = &self.names.config else {
            return Ok(());
        };
        self.write_config_struct(writer)?;
        writer.doc(&format!(
            "{option} applies the set fields of cfg as configuration overrides."
        ));
        writer.line(format!("func {option}(cfg {config}) golem.ClientOpt {{"));
        writer.indent();
        writer.line("var entries []golem.ConfigEntry");
        for (decl, ident) in self.local_configs().iter().zip(self.config_field_idents()) {
            let path = decl
                .path
                .iter()
                .map(|s| go_string(s))
                .collect::<Vec<_>>()
                .join(", ");
            writer.line(format!("if v, ok := cfg.{ident}.Get(); ok {{"));
            writer.indent();
            writer.line(format!(
                "entries = append(entries, golem.ConfigEntryOf([]string{{{path}}}, v))"
            ));
            writer.dedent();
            writer.line("}");
        }
        writer.line("return golem.WithConfigEntries(entries...)");
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

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

        let ephemeral = matches!(self.agent_type.mode, AgentMode::Ephemeral);
        let spec = if ephemeral {
            "golem.AgentClientSpec{Mode: golem.Ephemeral}"
        } else {
            "golem.AgentClientSpec{}"
        };
        writer.line(format!(
            "var {remote} = golem.DefineFullAgentClient[{}]({}, {spec})",
            n.id,
            go_string(agent_name)
        ));
        writer.blank();

        // One input struct and one descriptor per method.
        let mut outputs = Vec::with_capacity(self.agent_type.methods.len());
        for (idx, method) in self.agent_type.methods.iter().enumerate() {
            // A method without parameters takes golem.Unit, as a hand-written
            // Go agent's would, rather than an empty struct of its own.
            let input = if user_supplied_fields(&method.input_schema).is_empty() {
                "golem.Unit".to_string()
            } else {
                let input = n.inputs[idx].clone();
                self.write_input_struct(
                    &input,
                    &format!("{input} holds the arguments of {}.", method.name),
                    &method.input_schema,
                    &mut writer,
                )?;
                input
            };
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

        self.write_guest_config(&mut writer)?;

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

        // An ephemeral agent has no durable identity, so only phantoms
        // address one.
        if !ephemeral {
            writer.doc(&format!(
                "{} returns a client for the {agent_name} instance identified by id,\n\
                 creating it if it does not exist yet. Options address a phantom\n\
                 (golem.WithPhantomID) and override configuration{}.",
                n.get,
                self.config_option_hint()
            ));
            // Always multi-line: gofmt keeps a one-line body only below a size
            // limit, and an agent's name decides which side of it this falls on.
            writer.line(format!(
                "func {}(id {}, opts ...golem.ClientOpt) {} {{",
                n.get, n.id, n.client
            ));
            writer.indent();
            writer.line(format!(
                "return {}{{client: {remote}.Get(id, opts...)}}",
                n.client
            ));
            writer.dedent();
            writer.line("}");
            writer.blank();
        }
        writer.doc(&format!(
            "{} allocates a fresh phantom {agent_name} instance. Options override\n\
             configuration{}.",
            n.new_phantom,
            self.config_option_hint()
        ));
        writer.line(format!(
            "func {}(id {}, opts ...golem.ClientOpt) {} {{",
            n.new_phantom, n.id, n.client
        ));
        writer.indent();
        writer.line(format!(
            "return {}{{client: {remote}.NewPhantom(id, opts...)}}",
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
        let input_type = if fields.is_empty() {
            "golem.Unit"
        } else {
            n.inputs[idx].as_str()
        };
        let input = format!(
            "{input_type}{{{}}}",
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

/// True when a user-supplied field of the input carries a stream.
fn input_uses_streams(graph: &SchemaGraph, input: &InputSchema) -> bool {
    user_supplied_fields(input)
        .iter()
        .any(|f| contains_stream_in_graph(graph, &f.schema))
}

/// True when a method takes or returns a stream anywhere in its schema.
pub(super) fn method_uses_streams(graph: &SchemaGraph, method: &AgentMethodSchema) -> bool {
    input_uses_streams(graph, &method.input_schema)
        || matches!(&method.output_schema, OutputSchema::Single(t) if contains_stream_in_graph(graph, t))
}

/// The per-case type names a variant or union declaration emits, which the
/// registration has to name identically.
pub(crate) fn case_idents<'a>(
    type_name: &str,
    cases: impl Iterator<Item = &'a str>,
) -> Vec<String> {
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
