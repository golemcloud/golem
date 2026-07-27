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

//! MoonBit bridge SDK generator.
//!
//! In external mode, the MoonBit generator emits a fully self-contained `moon`
//! module. The static runtime lives in the `<agent-client>/runtime` package
//! (embedded from `src/bridge_gen/moonbit/runtime`) and the per-agent generated
//! client lives in the `<agent-client>/client` package. In guest mode, the
//! client is a WASM library module that depends on the Golem MoonBit SDK and
//! uses its RPC runtime instead of bundling the REST runtime.
//!
//! Deriving the module name from the agent type keeps multiple generated bridge
//! modules usable from the same consuming MoonBit project.

pub mod mbt_writer;
#[allow(clippy::module_inception)]
pub mod moonbit;
pub mod tool;
pub mod type_name;

pub use type_name::MoonBitTypeName;

use crate::bridge_gen::moonbit::mbt_writer::MoonBitWriter;
use crate::bridge_gen::moonbit::moonbit::{
    RESERVED_TYPE_NAMES, to_moonbit_constructor_ident, to_moonbit_term_ident, unique_idents,
    unique_idents_with_reserved,
};
use crate::bridge_gen::type_naming::{TypeNaming, user_supplied_fields};
use crate::bridge_gen::{BridgeGenerator, BridgeMode, bridge_client_directory_name};
use crate::fs;
use crate::sdk_overrides::{sdk_overrides, workspace_root};
use crate::versions::moonbit_dep;
use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::Role;
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::graph::reachable_defs;
use golem_common::schema::multimodal::multimodal_variant_cases;
use golem_common::schema::schema_type::{
    DiscriminatorRule, NumericBound, NumericRestrictions, QuantityValue, SchemaType,
    VariantCaseType,
};
use golem_common::schema::unstructured::{
    unstructured_binary_restrictions, unstructured_text_restrictions,
};
use golem_common::schema::{
    AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField, OutputSchema,
};
use golem_common::schema::{MetadataEnvelope, SchemaGraph};
use heck::{ToSnakeCase, ToUpperCamelCase};
use include_dir::{Dir, include_dir};
use indoc::formatdoc;

/// Static runtime sources emitted verbatim into every generated module under
/// `runtime/`. Each file already carries its package-relative path.
static RUNTIME_DIR: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/src/bridge_gen/moonbit/runtime");

/// The two `AgentInvocationMode` wire strings (server OpenAPI enum).
const MODE_AWAIT: &str = "await";
const MODE_SCHEDULE: &str = "schedule";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoonBitBridgeMode {
    ExternalRest,
    GuestWasmRpc,
}

impl MoonBitBridgeMode {
    fn bridge_mode(self) -> BridgeMode {
        match self {
            MoonBitBridgeMode::ExternalRest => BridgeMode::External,
            MoonBitBridgeMode::GuestWasmRpc => BridgeMode::Guest,
        }
    }
}

/// Internal local / parameter names emitted in generated constructor and method
/// bodies. A user-supplied parameter is disambiguated away from these so it can
/// never shadow or collide with generated code. The set is the union of all
/// constructor and method contexts so a given input's parameter names are
/// identical wherever they are emitted (wire encoding is positional, so the
/// chosen names never affect the wire format).
const RESERVED_PARAM_NAMES: &[&str] = &[
    "self",
    "configuration",
    "parameters",
    "response",
    "result",
    "value",
    "v",
    "phantom",
    "when",
];

/// Associated function names on the generated agent struct that a user method
/// name must not collide with.
const RESERVED_METHOD_NAMES: &[&str] = &[
    "configure",
    "agent_id",
    "get",
    "get_phantom",
    "new_phantom",
    "get_with_config",
    "get_phantom_with_config",
    "new_phantom_with_config",
];

/// A generated multimodal enum: its MoonBit type name, the variant cases (one
/// per modality, in schema order — the index is the wire variant tag), and the
/// disambiguated MoonBit constructor identifier per case.
struct MultimodalEnum {
    name: String,
    cases: Vec<(String, SchemaType)>,
    case_idents: Vec<String>,
}

pub struct MoonBitBridgeGenerator {
    target_path: Utf8PathBuf,
    agent_type: AgentTypeSchema,
    testing: bool,
    mode: MoonBitBridgeMode,
    same_language: bool,
    type_naming: TypeNaming<MoonBitTypeName>,
    /// Multimodal enums needed by this agent's constructor and methods,
    /// deduplicated by case list. Precomputed during construction so the
    /// `&self` emitters can look them up.
    multimodals: Vec<MultimodalEnum>,
}

impl BridgeGenerator for MoonBitBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        MoonBitBridgeGenerator::new(agent_type, target_path, testing)
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        if !self.target_path.exists() {
            fs::create_dir_all(&self.target_path)?;
        }
        self.write_module_file()?;
        self.write_runtime()?;
        self.write_client()?;
        Ok(())
    }
}

impl MoonBitBridgeGenerator {
    pub fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode(
            agent_type,
            target_path,
            testing,
            MoonBitBridgeMode::ExternalRest,
        )
    }

    pub fn new_with_mode(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
        mode: MoonBitBridgeMode,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode_and_extra_reserved_names(
            agent_type,
            target_path,
            testing,
            mode,
            std::iter::empty::<String>(),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn new_guest_with_extra_reserved_names(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
        extra_reserved_names: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode_and_extra_reserved_names(
            agent_type,
            target_path,
            testing,
            MoonBitBridgeMode::GuestWasmRpc,
            extra_reserved_names,
        )
    }

    fn new_with_mode_and_extra_reserved_names(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
        mode: MoonBitBridgeMode,
        extra_reserved_names: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<Self> {
        let same_language = agent_type.source_language.eq_ignore_ascii_case("moonbit");

        let mut reserved_names = RESERVED_TYPE_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .chain(std::iter::once(agent_struct_name(&agent_type)))
            .collect::<Vec<_>>();
        if mode == MoonBitBridgeMode::GuestWasmRpc {
            reserved_names.push(guest_client_struct_name(&agent_type));
            reserved_names.extend(
                ["CodecError", "UnstructuredText", "UnstructuredBinary"]
                    .into_iter()
                    .map(str::to_string),
            );
            reserved_names.extend(extra_reserved_names);
        }
        let type_naming = TypeNaming::new_with_reserved_names(
            &agent_type,
            same_language,
            reserved_names.iter().cloned().map(MoonBitTypeName::from),
        )?;

        let multimodals =
            Self::collect_multimodals(&agent_type, &type_naming, same_language, &reserved_names)?;

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            testing,
            mode,
            same_language,
            type_naming,
            multimodals,
        })
    }

    /// Walks the constructor input and every method's input and output,
    /// collecting the distinct
    /// multimodal enums (deduplicated by exact case list) and assigning each a
    /// `Multimodal{N}` type name that does not collide with a generated named
    /// type. Multimodal input is only recognized when an invocation takes a single
    /// user field whose schema is the structural multimodal `list<variant<…>>`;
    /// multimodal output when the single return type is that shape.
    fn collect_multimodals(
        agent_type: &AgentTypeSchema,
        type_naming: &TypeNaming<MoonBitTypeName>,
        same_language: bool,
        reserved_names: &[String],
    ) -> anyhow::Result<Vec<MultimodalEnum>> {
        let mut used_names: std::collections::HashSet<String> =
            reserved_names.iter().cloned().collect();
        for (_, name) in type_naming.types() {
            used_names.insert(name.name.clone());
        }

        let mut multimodals: Vec<MultimodalEnum> = Vec::new();
        let mut next_index = 0;

        let mut consider = |cases: &[VariantCaseType]| -> anyhow::Result<()> {
            let pairs = multimodal_pairs(cases)?;
            if multimodals.iter().any(|m| m.cases == pairs) {
                return Ok(());
            }
            let name = loop {
                let candidate = format!("Multimodal{next_index}");
                next_index += 1;
                if !used_names.contains(&candidate) {
                    break candidate;
                }
            };
            used_names.insert(name.clone());
            let case_idents = unique_idents(
                pairs
                    .iter()
                    .map(|(case_name, _)| to_moonbit_constructor_ident(case_name, same_language))
                    .collect(),
            );
            multimodals.push(MultimodalEnum {
                name,
                cases: pairs,
                case_idents,
            });
            Ok(())
        };

        let constructor_fields = user_supplied_fields(&agent_type.constructor.input_schema);
        if let [field] = constructor_fields.as_slice()
            && let Some(cases) = multimodal_variant_cases(type_naming.graph(), &field.schema)?
        {
            consider(cases)?;
        }

        for method in &agent_type.methods {
            let fields = user_supplied_fields(&method.input_schema);
            if let [field] = fields.as_slice()
                && let Some(cases) = multimodal_variant_cases(type_naming.graph(), &field.schema)?
            {
                consider(cases)?;
            }
            if let OutputSchema::Single(ty) = &method.output_schema
                && let Some(cases) = multimodal_variant_cases(type_naming.graph(), ty)?
            {
                consider(cases)?;
            }
        }

        Ok(multimodals)
    }

    /// Name of the generated agent handle struct, e.g. `foo-agent` -> `FooAgent`.
    fn agent_struct_name(&self) -> String {
        agent_struct_name(&self.agent_type)
    }

    /// The generated `moon` module name. The runtime package is
    /// `<module>/runtime` and the generated client package is `<module>/client`.
    fn module_name(&self) -> String {
        bridge_client_directory_name(&self.agent_type.type_name, self.mode.bridge_mode())
    }

    // --- Project files ------------------------------------------------------

    fn write_module_file(&self) -> anyhow::Result<()> {
        let mod_json = match self.mode {
            MoonBitBridgeMode::ExternalRest => formatdoc! {r#"
                {{
                  "name": "{module}",
                  "version": "0.0.1",
                  "deps": {{
                    "moonbitlang/async": "{async_version}"
                  }}
                }}
                "#,
                module = self.module_name(),
                async_version = moonbit_dep::ASYNC_VERSION,
            },
            MoonBitBridgeMode::GuestWasmRpc => {
                let sdk_dep = if self.testing {
                    let sdk_path = workspace_root()?.join("sdks/moonbit/golem_sdk");
                    serde_json::json!({ "path": sdk_path })
                } else {
                    serde_json::from_str(&sdk_overrides()?.moonbit_sdk_dep())
                        .context("failed to parse MoonBit SDK dependency override")?
                };
                let manifest = serde_json::json!({
                    "name": self.module_name(),
                    "version": "0.0.1",
                    "deps": {
                        "golemcloud/golem_sdk": sdk_dep,
                    },
                    "preferred-target": "wasm",
                });
                serde_json::to_string_pretty(&manifest)
                    .context("failed to serialize generated MoonBit module manifest")?
                    + "\n"
            }
        };
        fs::write_str(self.target_path.join("moon.mod.json"), mod_json)?;
        Ok(())
    }

    fn write_runtime(&self) -> anyhow::Result<()> {
        match self.mode {
            MoonBitBridgeMode::ExternalRest => {
                let runtime_root = self.target_path.join("runtime");
                write_dir(&RUNTIME_DIR, &runtime_root)
            }
            MoonBitBridgeMode::GuestWasmRpc => Ok(()),
        }
    }

    fn write_client(&self) -> anyhow::Result<()> {
        let client_dir = self.target_path.join("client");
        fs::create_dir_all(&client_dir)?;

        let moon_pkg = match self.mode {
            MoonBitBridgeMode::ExternalRest => formatdoc! {r#"
                import {{
                  "{module}/runtime" @runtime,
                }}
                "#,
                module = self.module_name(),
            },
            MoonBitBridgeMode::GuestWasmRpc => formatdoc! {r#"
                import {{
                  "golemcloud/golem_sdk/agents",
                  "golemcloud/golem_sdk/interface/golem/agent/common" @common,
                  "golemcloud/golem_sdk/interface/golem/agent/host" @agentHost,
                  "golemcloud/golem_sdk/interface/golem/core/types" @types,
                  "golemcloud/golem_sdk/interface/wasi/clocks/system-clock" @systemClock,
                  "golemcloud/golem_sdk/rpc",
                  "golemcloud/golem_sdk/schema_model" @model,
                }}
                "#},
        };
        fs::write_str(client_dir.join("moon.pkg"), moon_pkg)?;

        let content = self.generate_client_source()?;
        fs::write_str(client_dir.join("client.mbt"), content)?;
        Ok(())
    }

    /// Renders the full generated client source file: the generated type
    /// definitions, their codecs, and the agent handle struct.
    fn generate_client_source(&self) -> anyhow::Result<String> {
        let mut writer = MoonBitWriter::new();
        writer.line("// Generated by golem-cli. Do not edit.");
        writer.blank();
        let description = match self.mode {
            MoonBitBridgeMode::ExternalRest => format!(
                "Type-safe MoonBit client for the `{}` Golem agent, invoking it over the public REST API.",
                self.agent_type.type_name.as_str()
            ),
            MoonBitBridgeMode::GuestWasmRpc => format!(
                "Type-safe MoonBit guest RPC client for the `{}` Golem agent.",
                self.agent_type.type_name.as_str()
            ),
        };
        writer.doc(&description);
        writer.blank();

        self.write_type_definitions(&mut writer)?;
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            self.write_guest_codec_support(&mut writer);
        }
        self.write_codecs(&mut writer)?;
        self.write_multimodals(&mut writer)?;
        match self.mode {
            MoonBitBridgeMode::ExternalRest => self.write_agent_struct(&mut writer)?,
            MoonBitBridgeMode::GuestWasmRpc => self.write_guest_agent_client(&mut writer)?,
        }

        Ok(writer.finish())
    }

    // --- Type definitions ---------------------------------------------------

    fn write_type_definitions(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        let types: Vec<(SchemaType, MoonBitTypeName)> = self
            .type_naming
            .types()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect();

        for (typ, name) in &types {
            let resolved = self.resolve_ref(typ);
            if self.mode == MoonBitBridgeMode::GuestWasmRpc
                && (unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some()
                    || unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some())
            {
                continue;
            }
            if !is_named_composite(resolved) {
                // Non-composite named defs (aliases to scalars / lists / …) are
                // inlined at their use sites by `type_reference`.
                continue;
            }
            self.write_type_definition(writer, &name.name, resolved)?;
            writer.blank();
        }
        Ok(())
    }

    fn write_type_definition(
        &self,
        writer: &mut MoonBitWriter,
        name: &str,
        resolved: &SchemaType,
    ) -> anyhow::Result<()> {
        match resolved {
            SchemaType::Record { fields, .. } => {
                let field_names = self.record_field_idents(fields);
                if fields.is_empty() {
                    writer.line(format!(
                        "pub(all) struct {name} {{}} {}",
                        self.type_derives()
                    ));
                } else {
                    writer.line(format!("pub(all) struct {name} {{"));
                    writer.indent();
                    for (idx, field) in fields.iter().enumerate() {
                        let field_type = self.type_reference(&field.body)?;
                        writer.line(format!("{} : {field_type}", field_names[idx]));
                    }
                    writer.dedent();
                    writer.line(format!("}} {}", self.type_derives()));
                }
            }
            SchemaType::Variant { cases, .. } => {
                let case_names = self.variant_case_idents(cases.iter().map(|c| c.name.as_str()));
                writer.line(format!("pub(all) enum {name} {{"));
                writer.indent();
                for (idx, case) in cases.iter().enumerate() {
                    match &case.payload {
                        Some(payload) => {
                            let payload_type = self.type_reference(payload)?;
                            writer.line(format!("{}({payload_type})", case_names[idx]));
                        }
                        None => writer.line(case_names[idx].clone()),
                    }
                }
                writer.dedent();
                writer.line(format!("}} {}", self.type_derives()));
            }
            SchemaType::Enum { cases, .. } => {
                let case_names = self.variant_case_idents(cases.iter().map(|c| c.as_str()));
                writer.line(format!("pub(all) enum {name} {{"));
                writer.indent();
                for case_name in &case_names {
                    writer.line(case_name.clone());
                }
                writer.dedent();
                writer.line(format!("}} {}", self.type_derives()));
            }
            SchemaType::Flags { flags, .. } => {
                let flag_names = self.record_field_idents_from(flags.iter().map(|f| f.as_str()));
                if flag_names.is_empty() {
                    writer.line(format!(
                        "pub(all) struct {name} {{}} {}",
                        self.type_derives()
                    ));
                } else {
                    writer.line(format!("pub(all) struct {name} {{"));
                    writer.indent();
                    for flag_name in &flag_names {
                        writer.line(format!("{flag_name} : Bool"));
                    }
                    writer.dedent();
                    writer.line(format!("}} {}", self.type_derives()));
                }
            }
            SchemaType::Union { spec, .. } => {
                let branch_names =
                    self.variant_case_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                writer.line(format!("pub(all) enum {name} {{"));
                writer.indent();
                for (idx, branch) in spec.branches.iter().enumerate() {
                    let payload_type = self.type_reference(&branch.body)?;
                    writer.line(format!("{}({payload_type})", branch_names[idx]));
                }
                writer.dedent();
                writer.line(format!("}} {}", self.type_derives()));
            }
            other => {
                bail!("Unexpected non-composite type reached write_type_definition: {other:?}")
            }
        }
        Ok(())
    }

    fn type_derives(&self) -> &'static str {
        match self.mode {
            MoonBitBridgeMode::ExternalRest => "derive(Debug, Eq)",
            MoonBitBridgeMode::GuestWasmRpc => "derive(Eq)",
        }
    }

    fn codec_raise_clause(&self) -> &'static str {
        match self.mode {
            MoonBitBridgeMode::ExternalRest => "",
            MoonBitBridgeMode::GuestWasmRpc => " raise",
        }
    }

    // --- Codecs -------------------------------------------------------------

    /// Emits an `encode_<Name>` / `decode_<Name>` pair for every generated named
    /// composite type. Guest encoders and all decoders raise on a wire-shape
    /// mismatch; external encoders retain their existing total API.
    fn write_codecs(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            let mut external = MoonBitWriter::new();
            self.write_external_codecs(&mut external)?;
            for line in guest_codec_source(external.finish()).lines() {
                writer.line(line);
            }
            return Ok(());
        }
        self.write_external_codecs(writer)
    }

    fn write_external_codecs(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        let types: Vec<(SchemaType, MoonBitTypeName)> = self
            .type_naming
            .types()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect();

        for (typ, name) in &types {
            let resolved = self.resolve_ref(typ);
            if self.mode == MoonBitBridgeMode::GuestWasmRpc
                && (unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some()
                    || unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some())
            {
                continue;
            }
            if !is_named_composite(resolved) {
                continue;
            }
            self.write_encode_fn(writer, &name.name, resolved)?;
            writer.blank();
            self.write_decode_fn(writer, &name.name, resolved)?;
            writer.blank();
        }
        Ok(())
    }

    /// Emits the generated multimodal enums and their codecs. A multimodal value
    /// is an `Array[Multimodal{N}]` where each modality is a variant case; on
    /// the wire it is a `list<variant<…>>` (one variant per item, the case index
    /// being the modality's schema order).
    fn write_multimodals(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            let mut external = MoonBitWriter::new();
            self.write_external_multimodals(&mut external)?;
            for line in guest_codec_source(external.finish()).lines() {
                writer.line(line);
            }
            return Ok(());
        }
        self.write_external_multimodals(writer)
    }

    fn write_external_multimodals(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        for mm in &self.multimodals {
            writer
                .doc("A multimodal value: a sequence of modality items, each carrying a payload.");
            writer.line(format!("pub(all) enum {} {{", mm.name));
            writer.indent();
            for (idx, (_, payload)) in mm.cases.iter().enumerate() {
                let ty = self.type_reference(payload)?;
                writer.line(format!("{}({ty})", mm.case_idents[idx]));
            }
            writer.dedent();
            writer.line(format!("}} {}", self.type_derives()));
            writer.blank();

            // encode
            writer.line(format!(
                "pub fn encode_{}(values : Array[{}]) -> @runtime.SchemaValue{} {{",
                mm.name,
                mm.name,
                self.codec_raise_clause()
            ));
            writer.indent();
            writer.line("let elements : Array[@runtime.SchemaValue] = []");
            writer.line("values.each((item) => {");
            writer.indent();
            writer.line("let encoded = match item {");
            writer.indent();
            for (idx, (_, payload)) in mm.cases.iter().enumerate() {
                let enc = self.encode_expr("inner", payload, 0)?;
                writer.line(format!("{}::{}(inner) => {{", mm.name, mm.case_idents[idx]));
                writer.indent();
                writer.line(format!("let vp = {enc}"));
                writer.line(format!("@runtime.VariantValue({idx}, Some(vp))"));
                writer.dedent();
                writer.line("}");
            }
            writer.dedent();
            writer.line("}");
            writer.line("elements.push(encoded)");
            writer.dedent();
            writer.line("})");
            writer.line("@runtime.ListValue(elements)");
            writer.dedent();
            writer.line("}");
            writer.blank();

            // decode
            writer.line(format!(
                "pub fn decode_{}(value : @runtime.SchemaValue) -> Array[{}] raise {{",
                mm.name, mm.name
            ));
            writer.indent();
            writer.line("@runtime.as_list(value).map((item) => {");
            writer.indent();
            writer.line("let (case_index, payload) = @runtime.as_variant(item)");
            writer.line("match case_index {");
            writer.indent();
            for (idx, (_, payload)) in mm.cases.iter().enumerate() {
                let dec = self.decode_expr("vp", payload, 0)?;
                writer.line(format!("{idx} => {{"));
                writer.indent();
                writer.line("let vp = @runtime.variant_payload(case_index, payload)");
                writer.line(format!("{}::{}({dec})", mm.name, mm.case_idents[idx]));
                writer.dedent();
                writer.line("}");
            }
            writer.line(format!(
                "other => raise @runtime.BridgeError(\"Invalid multimodal variant case index for {}: \\{{other}}\")",
                mm.name
            ));
            writer.dedent();
            writer.line("}");
            writer.dedent();
            writer.line("})");
            writer.dedent();
            writer.line("}");
            writer.blank();
        }
        Ok(())
    }

    fn write_guest_codec_support(&self, writer: &mut MoonBitWriter) {
        let source = r#"pub suberror CodecError(String) derive(Debug, Eq)

pub(all) enum UnstructuredText {
  Inline(String, String?)
  Url(String)
} derive(Debug, Eq)

pub(all) enum UnstructuredBinary {
  Inline(FixedArray[Byte], String?)
  Url(String)
} derive(Debug, Eq)

fn[T] codec_mismatch(expected : String, value : @model.SchemaValue) -> T raise {
  raise CodecError("Expected " + expected + ", got " + value.to_string())
}

fn guest_as_bool(v : @model.SchemaValue) -> Bool raise { match v { Bool(x) => x; o => codec_mismatch("bool", o) } }
fn guest_as_s8(v : @model.SchemaValue) -> Int raise { match v { S8(x) => { if x < -128 || x > 127 { raise CodecError("s8 value out of range: " + x.to_string()) }; x }; o => codec_mismatch("s8", o) } }
fn guest_as_s16(v : @model.SchemaValue) -> Int raise { match v { S16(x) => { if x < -32768 || x > 32767 { raise CodecError("s16 value out of range: " + x.to_string()) }; x }; o => codec_mismatch("s16", o) } }
fn guest_as_s32(v : @model.SchemaValue) -> Int raise { match v { S32(x) => x; o => codec_mismatch("s32", o) } }
fn guest_as_s64(v : @model.SchemaValue) -> Int64 raise { match v { S64(x) => x; o => codec_mismatch("s64", o) } }
fn guest_as_u8(v : @model.SchemaValue) -> Byte raise { match v { U8(x) => x; o => codec_mismatch("u8", o) } }
fn guest_as_u16(v : @model.SchemaValue) -> Int raise { match v { U16(x) => { if x > 65535 { raise CodecError("u16 value out of range: " + x.to_string()) }; x.to_int() }; o => codec_mismatch("u16", o) } }
fn guest_as_u32(v : @model.SchemaValue) -> UInt raise { match v { U32(x) => x; o => codec_mismatch("u32", o) } }
fn guest_as_u64(v : @model.SchemaValue) -> UInt64 raise { match v { U64(x) => x; o => codec_mismatch("u64", o) } }
fn guest_as_f32(v : @model.SchemaValue) -> Float raise { match v { F32(x) => x; o => codec_mismatch("f32", o) } }
fn guest_as_f64(v : @model.SchemaValue) -> Double raise { match v { F64(x) => x; o => codec_mismatch("f64", o) } }
fn guest_as_char(v : @model.SchemaValue) -> Char raise { match v { Char(x) => x; o => codec_mismatch("char", o) } }
fn guest_as_string(v : @model.SchemaValue) -> String raise { match v { String(x) => x; o => codec_mismatch("string", o) } }
fn guest_as_path(v : @model.SchemaValue) -> String raise { match v { Path(x) => x; o => codec_mismatch("path", o) } }
fn guest_as_url(v : @model.SchemaValue) -> String raise { match v { Url(x) => x; o => codec_mismatch("url", o) } }
fn guest_as_datetime(v : @model.SchemaValue) -> @types.Datetime raise { match v { Datetime(x) => x; o => codec_mismatch("datetime", o) } }
fn guest_as_duration(v : @model.SchemaValue) -> Int64 raise { match v { Duration(x) => x; o => codec_mismatch("duration", o) } }
fn guest_as_record(v : @model.SchemaValue) -> Array[@model.SchemaValue] raise { match v { Record(x) => x; o => codec_mismatch("record", o) } }
fn guest_as_variant(v : @model.SchemaValue) -> (UInt, @model.SchemaValue?) raise { match v { Variant(i, x) => (i, x); o => codec_mismatch("variant", o) } }
fn guest_as_enum(v : @model.SchemaValue) -> UInt raise { match v { Enum(x) => x; o => codec_mismatch("enum", o) } }
fn guest_as_flags(v : @model.SchemaValue) -> Array[Bool] raise { match v { Flags(x) => x; o => codec_mismatch("flags", o) } }
fn guest_as_tuple(v : @model.SchemaValue) -> Array[@model.SchemaValue] raise { match v { Tuple(x) => x; o => codec_mismatch("tuple", o) } }
fn guest_as_list(v : @model.SchemaValue) -> Array[@model.SchemaValue] raise { match v { List(x) => x; o => codec_mismatch("list", o) } }
fn guest_as_fixed_list(v : @model.SchemaValue) -> Array[@model.SchemaValue] raise { match v { FixedList(x) => x; o => codec_mismatch("fixed-list", o) } }
fn guest_as_map(v : @model.SchemaValue) -> Array[@model.SchemaMapEntry] raise { match v { Map(x) => x; o => codec_mismatch("map", o) } }
fn guest_as_option(v : @model.SchemaValue) -> @model.SchemaValue? raise { match v { Option(x) => x; o => codec_mismatch("option", o) } }
fn guest_as_result(v : @model.SchemaValue) -> @model.SchemaValue raise { match v { ResultOk(_) | ResultErr(_) => v; o => codec_mismatch("result", o) } }
fn guest_as_union(v : @model.SchemaValue) -> (String, @model.SchemaValue) raise { match v { Union(t, x) => (t, x); o => codec_mismatch("union", o) } }
fn guest_variant_payload(index : UInt, payload : @model.SchemaValue?) -> @model.SchemaValue raise { match payload { Some(x) => x; None => raise CodecError("Missing payload for variant case " + index.to_string()) } }
fn guest_required_payload(payload : @model.SchemaValue?, context : String) -> @model.SchemaValue raise { match payload { Some(x) => x; None => raise CodecError("Missing payload for " + context) } }
fn guest_allowed(value : String?, allowed : Array[String], kind : String) -> Unit raise { match value { Some(x) => if allowed.length() > 0 && !allowed.contains(x) { raise CodecError("Unsupported " + kind + ": " + x) }; None => () } }
fn guest_encode_unstructured_text(value : UnstructuredText, allowed : Array[String]) -> @model.SchemaValue raise { match value { Inline(text, language) => { guest_allowed(language, allowed, "text language"); Variant(0, Some(Text(text, language))) }; Url(url) => Variant(1, Some(Url(url))) } }
fn guest_decode_unstructured_text(value : @model.SchemaValue, allowed : Array[String]) -> UnstructuredText raise { match value { Variant(0, Some(Text(text, language))) => { guest_allowed(language, allowed, "text language"); Inline(text, language) }; Variant(1, Some(Url(url))) => Url(url); o => codec_mismatch("unstructured-text variant", o) } }
fn guest_encode_unstructured_binary(value : UnstructuredBinary, allowed : Array[String]) -> @model.SchemaValue raise { match value { Inline(bytes, mime_type) => { guest_allowed(mime_type, allowed, "MIME type"); Variant(0, Some(Binary(bytes, mime_type))) }; Url(url) => Variant(1, Some(Url(url))) } }
fn guest_decode_unstructured_binary(value : @model.SchemaValue, allowed : Array[String]) -> UnstructuredBinary raise { match value { Variant(0, Some(Binary(bytes, mime_type))) => { guest_allowed(mime_type, allowed, "MIME type"); Inline(bytes, mime_type) }; Variant(1, Some(Url(url))) => Url(url); o => codec_mismatch("unstructured-binary variant", o) } }
"#;
        for line in source.lines() {
            writer.line(line);
        }
        writer.blank();
    }

    fn write_encode_fn(
        &self,
        writer: &mut MoonBitWriter,
        name: &str,
        resolved: &SchemaType,
    ) -> anyhow::Result<()> {
        writer.line(format!(
            "pub fn encode_{name}(value : {name}) -> @runtime.SchemaValue{} {{",
            self.codec_raise_clause()
        ));
        writer.indent();
        match resolved {
            SchemaType::Record { fields, .. } => {
                let field_names = self.record_field_idents(fields);
                if fields.is_empty() {
                    writer.line("@runtime.RecordValue([])");
                } else {
                    let mut elems = Vec::new();
                    for (idx, field) in fields.iter().enumerate() {
                        let enc = self.encode_expr(
                            &format!("value.{}", field_names[idx]),
                            &field.body,
                            0,
                        )?;
                        writer.line(format!("let f{idx} = {enc}"));
                        elems.push(format!("f{idx}"));
                    }
                    writer.line(format!("@runtime.RecordValue([{}])", elems.join(", ")));
                }
            }
            SchemaType::Variant { cases, .. } => {
                let case_names = self.variant_case_idents(cases.iter().map(|c| c.name.as_str()));
                writer.line("match value {");
                writer.indent();
                for (idx, case) in cases.iter().enumerate() {
                    let case_name = &case_names[idx];
                    match &case.payload {
                        Some(payload) => {
                            let enc = self.encode_expr("inner", payload, 0)?;
                            writer.line(format!("{name}::{case_name}(inner) => {{"));
                            writer.indent();
                            writer.line(format!("let vp = {enc}"));
                            writer.line(format!("@runtime.VariantValue({idx}, Some(vp))"));
                            writer.dedent();
                            writer.line("}");
                        }
                        None => {
                            writer.line(format!(
                                "{name}::{case_name} => @runtime.VariantValue({idx}, None)"
                            ));
                        }
                    }
                }
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Enum { cases, .. } => {
                let case_names = self.variant_case_idents(cases.iter().map(|c| c.as_str()));
                writer.line("match value {");
                writer.indent();
                for (idx, case_name) in case_names.iter().enumerate() {
                    writer.line(format!("{name}::{case_name} => @runtime.EnumValue({idx})"));
                }
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Flags { flags, .. } => {
                let flag_names = self.record_field_idents_from(flags.iter().map(|f| f.as_str()));
                let bits = flag_names
                    .iter()
                    .map(|f| format!("value.{f}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                writer.line(format!("@runtime.FlagsValue([{bits}])"));
            }
            SchemaType::Union { spec, .. } => {
                let branch_names =
                    self.variant_case_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                writer.line("match value {");
                writer.indent();
                for (idx, branch) in spec.branches.iter().enumerate() {
                    let branch_name = &branch_names[idx];
                    let tag = moonbit_string_literal(branch.tag.as_str());
                    let enc = self.encode_expr("inner", &branch.body, 0)?;
                    writer.line(format!("{name}::{branch_name}(inner) => {{"));
                    writer.indent();
                    writer.line(format!("let ub = {enc}"));
                    writer.line(format!("@runtime.UnionValue({tag}, ub)"));
                    writer.dedent();
                    writer.line("}");
                }
                writer.dedent();
                writer.line("}");
            }
            other => bail!("Unexpected non-composite type reached write_encode_fn: {other:?}"),
        }
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    fn write_decode_fn(
        &self,
        writer: &mut MoonBitWriter,
        name: &str,
        resolved: &SchemaType,
    ) -> anyhow::Result<()> {
        writer.line(format!(
            "pub fn decode_{name}(value : @runtime.SchemaValue) -> {name} raise {{"
        ));
        writer.indent();
        match resolved {
            SchemaType::Record { fields, .. } => {
                let n = fields.len();
                let field_names = self.record_field_idents(fields);
                writer.line("let fields = @runtime.as_record(value)");
                writer.line(format!(
                    "if fields.length() != {n} {{ raise @runtime.BridgeError(\"Expected record {name} with {n} fields, got \\{{fields.length()}}\") }}"
                ));
                if fields.is_empty() {
                    writer.line(format!("{name}::{{}}"));
                } else {
                    let mut assigns = Vec::new();
                    for (idx, field) in fields.iter().enumerate() {
                        let dec = self.decode_expr(&format!("fields[{idx}]"), &field.body, 0)?;
                        writer.line(format!("let f{idx} = {dec}"));
                        assigns.push(format!("{}: f{idx}", field_names[idx]));
                    }
                    writer.line(format!("{{ {} }}", assigns.join(", ")));
                }
            }
            SchemaType::Variant { cases, .. } => {
                let case_names = self.variant_case_idents(cases.iter().map(|c| c.name.as_str()));
                writer.line("let (case_index, payload) = @runtime.as_variant(value)");
                writer.line("match case_index {");
                writer.indent();
                for (idx, case) in cases.iter().enumerate() {
                    let case_name = &case_names[idx];
                    match &case.payload {
                        Some(payload) => {
                            let dec = self.decode_expr("vp", payload, 0)?;
                            writer.line(format!("{idx} => {{"));
                            writer.indent();
                            writer.line("let vp = @runtime.variant_payload(case_index, payload)");
                            writer.line(format!("{name}::{case_name}({dec})"));
                            writer.dedent();
                            writer.line("}");
                        }
                        None => {
                            let msg = format!(
                                "Unexpected payload for payload-less variant case {name}.{}",
                                case.name
                            );
                            writer.line(format!("{idx} => {{"));
                            writer.indent();
                            writer.line(format!(
                                "if payload is Some(_) {{ raise @runtime.BridgeError({}) }}",
                                moonbit_string_literal(&msg)
                            ));
                            writer.line(format!("{name}::{case_name}"));
                            writer.dedent();
                            writer.line("}");
                        }
                    }
                }
                writer.line(format!(
                    "other => raise @runtime.BridgeError(\"Invalid variant case index for {name}: \\{{other}}\")"
                ));
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Enum { cases, .. } => {
                let case_names = self.variant_case_idents(cases.iter().map(|c| c.as_str()));
                writer.line("match @runtime.as_enum(value) {");
                writer.indent();
                for (idx, case_name) in case_names.iter().enumerate() {
                    writer.line(format!("{idx} => {name}::{case_name}"));
                }
                writer.line(format!(
                    "other => raise @runtime.BridgeError(\"Invalid enum case index for {name}: \\{{other}}\")"
                ));
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Flags { flags, .. } => {
                let n = flags.len();
                let flag_names = self.record_field_idents_from(flags.iter().map(|f| f.as_str()));
                writer.line("let bits = @runtime.as_flags(value)");
                writer.line(format!(
                    "if bits.length() != {n} {{ raise @runtime.BridgeError(\"Expected flags {name} with {n} bits, got \\{{bits.length()}}\") }}"
                ));
                if flag_names.is_empty() {
                    writer.line(format!("{name}::{{}}"));
                } else {
                    let assigns = flag_names
                        .iter()
                        .enumerate()
                        .map(|(i, f)| format!("{f}: bits[{i}]"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    writer.line(format!("{{ {assigns} }}"));
                }
            }
            SchemaType::Union { spec, .. } => {
                let branch_names =
                    self.variant_case_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                writer.line("let (tag, body) = @runtime.as_union(value)");
                writer.line("match tag {");
                writer.indent();
                for (idx, branch) in spec.branches.iter().enumerate() {
                    let branch_name = &branch_names[idx];
                    let tag = moonbit_string_literal(branch.tag.as_str());
                    let dec = self.decode_expr("body", &branch.body, 0)?;
                    writer.line(format!("{tag} => {name}::{branch_name}({dec})"));
                }
                writer.line(format!(
                    "other => raise @runtime.BridgeError(\"Unknown union branch tag for {name}: \\{{other}}\")"
                ));
                writer.dedent();
                writer.line("}");
            }
            other => bail!("Unexpected non-composite type reached write_decode_fn: {other:?}"),
        }
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    // --- Agent handle -------------------------------------------------------

    /// Emits the agent handle struct: the configuration helper, `agent_id`
    /// accessor, mode-aware constructors, and per-method `await` / `trigger` /
    /// `schedule` wrappers.
    fn write_agent_struct(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        let agent = self.agent_struct_name();
        let agent_type_name = moonbit_string_literal(self.agent_type.type_name.as_str());

        writer.doc("A resolved agent. Construct one with `get` / `get_phantom` / `new_phantom`.");
        writer.line(format!("pub struct {agent} {{"));
        writer.indent();
        writer.line("resolved : @runtime.ResolvedAgent");
        writer.dedent();
        writer.line("}");
        writer.blank();

        // configure delegates to the shared runtime configuration cell.
        writer.doc(
            "Configures every generated bridge client in this module to target the given server.",
        );
        writer.line(format!(
            "pub fn {agent}::configure(server : @runtime.GolemServer, app_name : String, env_name : String) -> Unit {{"
        ));
        writer.indent();
        writer.line("@runtime.configure(server, app_name, env_name)");
        writer.dedent();
        writer.line("}");
        writer.blank();

        if self.agent_type.mode == AgentMode::Durable {
            writer.line(format!(
                "pub fn {agent}::agent_id(self : {agent}) -> @runtime.AgentId {{"
            ));
            writer.indent();
            writer.line("self.resolved.agent_id.unwrap()");
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        self.write_constructors(writer, &agent, &agent_type_name)?;

        let methods = self.agent_type.methods.clone();
        let bases = self.method_base_idents(&methods);
        for (method, base) in methods.iter().zip(bases.iter()) {
            self.write_method(writer, &agent, method, base)?;
        }

        Ok(())
    }

    fn write_guest_agent_client(&self, writer: &mut MoonBitWriter) -> anyhow::Result<()> {
        let client = guest_client_struct_name(&self.agent_type);
        let agent_type_name = moonbit_string_literal(self.agent_type.type_name.as_str());

        writer.doc("A native guest RPC client for this agent type.");
        writer.line(format!("pub(all) struct {client} {{"));
        writer.indent();
        writer.line("client : @rpc.AgentClient");
        writer.dedent();
        writer.line("}");
        writer.blank();

        self.write_guest_constructors(writer, &client, &agent_type_name)?;

        writer.line(format!(
            "pub fn {client}::get_agent_id(self : {client}) -> String raise @common.AgentError {{"
        ));
        writer.indent();
        writer.line("self.client.get_agent_id()");
        writer.dedent();
        writer.line("}");
        writer.blank();

        writer.line(format!(
            "pub fn {client}::phantom_id(self : {client}) -> @types.Uuid? {{"
        ));
        writer.indent();
        writer.line("self.client.phantom_id()");
        writer.dedent();
        writer.line("}");
        writer.blank();

        writer.line(format!("pub fn {client}::drop(self : {client}) -> Unit {{"));
        writer.indent();
        writer.line("self.client.drop()");
        writer.dedent();
        writer.line("}");
        writer.blank();

        if self.agent_type.mode == AgentMode::Durable {
            let input = self.agent_type.constructor.input_schema.clone();
            let param_defs = self.input_param_defs(&input)?;
            let param_decls = render_param_decls(&param_defs);
            let result_type_param = self.guest_scoped_result_type_param();
            let param_names = param_defs
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            let f_decl = append_param(
                &format!("f : async ({client}) -> {result_type_param}"),
                &param_decls,
            );
            writer.line(format!(
                "pub async fn[{result_type_param}] {client}::scoped({f_decl}) -> {result_type_param} {{"
            ));
            writer.indent();
            writer.line(format!(
                "let client = {client}::get({})",
                param_names.join(", ")
            ));
            writer.line("defer client.drop()");
            writer.line("f(client)");
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        let methods = self.agent_type.methods.clone();
        let bases = self.method_base_idents(&methods);
        for (method, base) in methods.iter().zip(bases.iter()) {
            self.write_guest_method(writer, &client, method, base)?;
        }
        Ok(())
    }

    fn guest_scoped_result_type_param(&self) -> String {
        let generated_names = self
            .type_naming
            .types()
            .map(|(_, name)| name.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut candidate = "T".to_string();
        let mut suffix = 2;
        while generated_names.contains(candidate.as_str()) {
            candidate = format!("T{suffix}");
            suffix += 1;
        }
        candidate
    }

    fn write_guest_constructors(
        &self,
        writer: &mut MoonBitWriter,
        client: &str,
        agent_type_name: &str,
    ) -> anyhow::Result<()> {
        let input = self.agent_type.constructor.input_schema.clone();
        let param_defs = self.input_param_defs(&input)?;
        let param_decls = render_param_decls(&param_defs);

        if self.agent_type.mode == AgentMode::Durable {
            self.write_guest_constructor(
                writer,
                client,
                agent_type_name,
                "get",
                &param_decls,
                &input,
                None,
                None,
            )?;
        }
        self.write_guest_constructor(
            writer,
            client,
            agent_type_name,
            "new_phantom",
            &param_decls,
            &input,
            None,
            None,
        )?;
        self.write_guest_constructor(
            writer,
            client,
            agent_type_name,
            "get_phantom",
            &append_param("phantom_id : @types.Uuid", &param_decls),
            &input,
            Some("phantom_id"),
            None,
        )?;

        let configs = self.local_configs();
        if configs.is_empty() {
            return Ok(());
        }
        let param_names = param_defs
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let config_names = self.config_param_idents(&param_names, &configs);
        let config_decls = configs
            .iter()
            .zip(config_names.iter())
            .map(|(config, name)| {
                Ok(format!(
                    "{name} : {}?",
                    self.type_reference(&config.value_type)?
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .join(", ");
        let with_config_decls = append_param(&config_decls, &param_decls);

        if self.agent_type.mode == AgentMode::Durable {
            self.write_guest_constructor(
                writer,
                client,
                agent_type_name,
                "get_with_config",
                &with_config_decls,
                &input,
                None,
                Some((&configs, &config_names)),
            )?;
        }
        self.write_guest_constructor(
            writer,
            client,
            agent_type_name,
            "new_phantom_with_config",
            &with_config_decls,
            &input,
            None,
            Some((&configs, &config_names)),
        )?;
        self.write_guest_constructor(
            writer,
            client,
            agent_type_name,
            "get_phantom_with_config",
            &append_param(
                &config_decls,
                &append_param("phantom_id : @types.Uuid", &param_decls),
            ),
            &input,
            Some("phantom_id"),
            Some((&configs, &config_names)),
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_guest_constructor(
        &self,
        writer: &mut MoonBitWriter,
        client: &str,
        agent_type_name: &str,
        name: &str,
        param_decls: &str,
        input: &InputSchema,
        phantom_id: Option<&str>,
        configs: Option<(&[AgentConfigDeclarationSchema], &[String])>,
    ) -> anyhow::Result<()> {
        writer.line(format!(
            "pub fn {client}::{name}({param_decls}) -> {client} raise @common.AgentError {{"
        ));
        writer.indent();
        self.write_guest_invocation_input(writer, input, "constructor_input", "constructor")?;
        if let Some((configs, names)) = configs {
            self.write_guest_config_array(writer, configs, names)?;
        }
        let mut args = vec![agent_type_name.to_string(), "constructor_input".to_string()];
        if let Some(phantom_id) = phantom_id {
            args.push(phantom_id.to_string());
        }
        if configs.is_some() {
            args.push("agent_config".to_string());
        }
        writer.line(format!(
            "let client = @rpc.AgentClient::{name}({})",
            args.join(", ")
        ));
        writer.line("{ client, }");
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

    fn write_guest_config_array(
        &self,
        writer: &mut MoonBitWriter,
        configs: &[AgentConfigDeclarationSchema],
        config_names: &[String],
    ) -> anyhow::Result<()> {
        writer.line("let agent_config : Array[@common.TypedAgentConfigValue] = []");
        for (config, name) in configs.iter().zip(config_names.iter()) {
            writer.line(format!("match {name} {{"));
            writer.indent();
            writer.line("Some(value) => {");
            writer.indent();
            let encoded = guest_codec_source(self.encode_expr("value", &config.value_type, 0)?);
            writer.line(format!("let config_value = {encoded} catch {{"));
            writer.indent();
            writer.line("error => raise @common.AgentError::InvalidInput(\"failed encoding agent config: \" + repr(error))");
            writer.dedent();
            writer.line("}");
            let graph = SchemaGraph {
                defs: reachable_defs(self.type_naming.graph(), &config.value_type),
                root: config.value_type.clone(),
            };
            writer.line(format!(
                "let wire = @model.typed_schema_value_to_wit(@model.TypedSchemaValue::{{ graph: {}, value: config_value, }}) catch {{",
                emit_schema_graph_literal(&graph)
            ));
            writer.indent();
            writer.line("error => raise @common.AgentError::InvalidInput(\"failed encoding agent config: \" + error.to_string())");
            writer.dedent();
            writer.line("}");
            let path = config
                .path
                .iter()
                .map(|segment| moonbit_string_literal(segment))
                .collect::<Vec<_>>()
                .join(", ");
            writer.line(format!(
                "agent_config.push(@common.TypedAgentConfigValue::{{ path: [{path}], value: wire, }})"
            ));
            writer.dedent();
            writer.line("}");
            writer.line("None => ()");
            writer.dedent();
            writer.line("}");
        }
        Ok(())
    }

    fn write_guest_invocation_input(
        &self,
        writer: &mut MoonBitWriter,
        input: &InputSchema,
        local_name: &str,
        context: &str,
    ) -> anyhow::Result<()> {
        let fields = user_supplied_fields(input);
        let names = self.input_param_idents(&fields);
        let mut values = Vec::new();
        if let Some(mm) = self.multimodal_input(input)? {
            writer.line(format!(
                "let f0 = encode_{}({}) catch {{",
                mm.name, names[0]
            ));
            writer.indent();
            writer.line(format!(
                "error => raise @common.AgentError::InvalidInput({} + repr(error))",
                moonbit_string_literal(&format!(
                    "{}.{context}: ",
                    self.agent_type.type_name.as_str()
                ))
            ));
            writer.dedent();
            writer.line("}");
            values.push("f0".to_string());
        } else {
            for (idx, field) in fields.iter().enumerate() {
                let encoded =
                    guest_codec_source(self.encode_expr(&names[idx], &field.schema, 0)?);
                writer.line(format!("let f{idx} = {encoded} catch {{"));
                writer.indent();
                writer.line(format!(
                    "error => raise @common.AgentError::InvalidInput({} + repr(error))",
                    moonbit_string_literal(&format!(
                        "{}.{context}: ",
                        self.agent_type.type_name.as_str()
                    ))
                ));
                writer.dedent();
                writer.line("}");
                values.push(format!("f{idx}"));
            }
        }
        writer.line(format!(
            "let {local_name} = @agents.encode_invocation_input([{}])",
            values.join(", ")
        ));
        Ok(())
    }

    fn write_guest_method(
        &self,
        writer: &mut MoonBitWriter,
        client: &str,
        method: &AgentMethodSchema,
        base: &str,
    ) -> anyhow::Result<()> {
        let method_name = moonbit_string_literal(&method.name);
        let context = format!("{}.{}", self.agent_type.type_name.as_str(), method.name);
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_decls = render_param_decls(&param_defs);
        let self_decls = prepend_self_decl(client, &param_decls);
        let (ret_ty, decode) = self.output_return(&method.output_schema)?;
        let decode = decode.map(guest_codec_source);

        writer.line(format!(
            "pub async fn {client}::{base}(self : {self_decls}) -> {ret_ty} {{"
        ));
        writer.indent();
        self.write_guest_invocation_input(writer, &method.input_schema, "input", &method.name)?;
        match decode {
            None => writer.line(format!(
                "let _ = self.client.invoke_and_await({method_name}, input)"
            )),
            Some(decode) => {
                writer.line(format!(
                    "match self.client.invoke_and_await({method_name}, input).value {{"
                ));
                writer.indent();
                writer.line("Some(tree) => {");
                writer.indent();
                writer.line("let value = @model.schema_value_from_wit(tree) catch {");
                writer.indent();
                writer.line(format!(
                    "error => raise @common.AgentError::InvalidType({} + error.to_string())",
                    moonbit_string_literal(&format!("{context}: "))
                ));
                writer.dedent();
                writer.line("}");
                writer.line(format!("{decode} catch {{"));
                writer.indent();
                writer.line(format!(
                    "error => raise @common.AgentError::InvalidType({} + repr(error))",
                    moonbit_string_literal(&format!("{context}: "))
                ));
                writer.dedent();
                writer.line("}");
                writer.dedent();
                writer.line("}");
                writer.line(format!(
                    "None => raise @common.AgentError::InvalidType({})",
                    moonbit_string_literal(&format!("{context}: expected a value"))
                ));
                writer.dedent();
                writer.line("}");
            }
        }
        writer.dedent();
        writer.line("}");
        writer.blank();

        writer.line(format!(
            "pub fn {client}::trigger_{base}(self : {self_decls}) -> Unit raise @common.AgentError {{"
        ));
        writer.indent();
        self.write_guest_invocation_input(writer, &method.input_schema, "input", &method.name)?;
        writer.line(format!("let _ = self.client.invoke({method_name}, input)"));
        writer.dedent();
        writer.line("}");
        writer.blank();

        let schedule_decls = prepend_param("scheduled_at : @systemClock.Instant", &param_decls);
        let schedule_self_decls = prepend_self_decl(client, &schedule_decls);
        writer.line(format!(
            "pub fn {client}::schedule_{base}(self : {schedule_self_decls}) -> Unit raise @common.AgentError {{"
        ));
        writer.indent();
        self.write_guest_invocation_input(writer, &method.input_schema, "input", &method.name)?;
        writer.line(format!(
            "let _ = self.client.schedule_invocation(scheduled_at, {method_name}, input)"
        ));
        writer.dedent();
        writer.line("}");
        writer.blank();

        writer.line(format!(
            "pub fn {client}::schedule_cancelable_{base}(self : {schedule_self_decls}) -> @agentHost.CancellationToken raise @common.AgentError {{"
        ));
        writer.indent();
        self.write_guest_invocation_input(writer, &method.input_schema, "input", &method.name)?;
        writer.line(format!(
            "self.client.schedule_cancelable_invocation(scheduled_at, {method_name}, input).cancellation_token"
        ));
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

    fn write_constructors(
        &self,
        writer: &mut MoonBitWriter,
        agent: &str,
        agent_type_name: &str,
    ) -> anyhow::Result<()> {
        let input = self.agent_type.constructor.input_schema.clone();
        let param_defs = self.input_param_defs(&input)?;
        let param_decls = render_param_decls(&param_defs);
        let param_names = param_defs
            .iter()
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>();

        // get (durable only)
        if self.agent_type.mode == AgentMode::Durable {
            writer.doc("Gets (creating if necessary) the durable agent addressed by the given constructor parameters.");
            writer.line(format!(
                "pub async fn {agent}::get({param_decls}) -> {agent} raise {{"
            ));
            writer.indent();
            self.write_create_agent_body(
                writer,
                agent,
                agent_type_name,
                &input,
                "None",
                "None",
                "[]",
            )?;
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        // Explicit phantom identities are supported only by durable agents.
        let phantom_decls = append_param("phantom : String", &param_decls);
        if self.agent_type.mode == AgentMode::Durable {
            writer
                .doc("Gets (creating if necessary) the agent with the given explicit phantom id.");
            writer.line(format!(
                "pub async fn {agent}::get_phantom({phantom_decls}) -> {agent} raise {{"
            ));
            writer.indent();
            self.write_create_agent_body(
                writer,
                agent,
                agent_type_name,
                &input,
                "Some(phantom)",
                "Some(phantom)",
                "[]",
            )?;
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        writer.doc(if self.agent_type.mode == AgentMode::Durable {
            "Creates a new agent instance with a fresh random phantom id."
        } else {
            "Creates a local logical proxy; each invocation receives a fresh final identity."
        });
        writer.line(format!(
            "pub async fn {agent}::new_phantom({param_decls}) -> {agent} raise {{"
        ));
        writer.indent();
        if self.agent_type.mode == AgentMode::Durable {
            let phantom_call_args = append_arg("@runtime.random_uuid()", &param_names);
            writer.line(format!("{agent}::get_phantom({phantom_call_args})"));
        } else {
            writer.line("let configuration = @runtime.get_configuration()");
            self.write_param_record(writer, &input)?;
            writer.line(format!("{agent}::{{"));
            writer.indent();
            writer.line("resolved: @runtime.ResolvedAgent::{");
            writer.indent();
            writer.line("configuration,");
            writer.line(format!("agent_type_name: {agent_type_name},"));
            writer.line("parameters,");
            writer.line("phantom_id: None,");
            writer.line("config: [],");
            writer.line("agent_id: None,");
            writer.dedent();
            writer.line("},");
            writer.dedent();
            writer.line("}");
        }
        writer.dedent();
        writer.line("}");
        writer.blank();

        self.write_with_config_constructors(
            writer,
            agent,
            agent_type_name,
            &input,
            &param_decls,
            &param_names,
        )?;
        Ok(())
    }

    /// Emits the `*_with_config` constructor variants when the agent declares
    /// locally overridable configuration. Each generated function takes the
    /// constructor parameters plus one `Option[..]` parameter per local config
    /// declaration; only the `Some` ones are sent as config overrides.
    fn write_with_config_constructors(
        &self,
        writer: &mut MoonBitWriter,
        agent: &str,
        agent_type_name: &str,
        input: &InputSchema,
        param_decls: &str,
        param_names: &[String],
    ) -> anyhow::Result<()> {
        let configs = self.local_configs();
        if configs.is_empty() {
            return Ok(());
        }
        let config_names = self.config_param_idents(param_names, &configs);
        let config_decls = configs
            .iter()
            .enumerate()
            .map(|(idx, config)| {
                let ty = self.type_reference(&config.value_type)?;
                Ok(format!("{} : {ty}?", config_names[idx]))
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .join(", ");

        // get_with_config (durable only)
        if self.agent_type.mode == AgentMode::Durable {
            let decls = append_param(&config_decls, param_decls);
            writer.doc("Gets (creating if necessary) the durable agent, overriding the given configuration values.");
            writer.line(format!(
                "pub async fn {agent}::get_with_config({decls}) -> {agent} raise {{"
            ));
            writer.indent();
            self.write_config_array(writer, &configs, &config_names)?;
            self.write_create_agent_body(
                writer,
                agent,
                agent_type_name,
                input,
                "None",
                "None",
                "agent_config",
            )?;
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        // get_phantom_with_config (durable only)
        let phantom_config_decls = append_param(
            &config_decls,
            &append_param("phantom : String", param_decls),
        );
        if self.agent_type.mode == AgentMode::Durable {
            writer.doc("Gets (creating if necessary) the agent with an explicit phantom id, overriding the given configuration values.");
            writer.line(format!(
            "pub async fn {agent}::get_phantom_with_config({phantom_config_decls}) -> {agent} raise {{"
            ));
            writer.indent();
            self.write_config_array(writer, &configs, &config_names)?;
            self.write_create_agent_body(
                writer,
                agent,
                agent_type_name,
                input,
                "Some(phantom)",
                "Some(phantom)",
                "agent_config",
            )?;
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        // new_phantom_with_config: a fresh random phantom id, delegating to
        // get_phantom_with_config so config overrides are applied.
        let decls = append_param(&config_decls, param_decls);
        writer.doc("Creates a new agent instance with a fresh random phantom id, overriding the given configuration values.");
        writer.line(format!(
            "pub async fn {agent}::new_phantom_with_config({decls}) -> {agent} raise {{"
        ));
        writer.indent();
        if self.agent_type.mode == AgentMode::Durable {
            let call_args = append_arg_list(
                param_names,
                &std::iter::once("@runtime.random_uuid()".to_string())
                    .chain(config_names.iter().cloned())
                    .collect::<Vec<_>>(),
            );
            writer.line(format!("{agent}::get_phantom_with_config({call_args})"));
        } else {
            writer.line("let configuration = @runtime.get_configuration()");
            self.write_param_record(writer, input)?;
            self.write_config_array(writer, &configs, &config_names)?;
            writer.line(format!("{agent}::{{ resolved: @runtime.ResolvedAgent::{{ configuration, agent_type_name: {agent_type_name}, parameters, phantom_id: None, config: agent_config, agent_id: None }} }}"));
        }
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

    /// The local (`AgentConfigSource::Local`) config declarations, in schema
    /// order. These are the configuration values a caller may override at
    /// construction time.
    fn local_configs(&self) -> Vec<AgentConfigDeclarationSchema> {
        self.agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .cloned()
            .collect()
    }

    /// MoonBit parameter identifiers for the `config_<path>` config-override
    /// parameters, disambiguated away from the constructor parameters and from
    /// the locals emitted by the `*_with_config` bodies.
    fn config_param_idents(
        &self,
        constructor_param_names: &[String],
        configs: &[AgentConfigDeclarationSchema],
    ) -> Vec<String> {
        let mut reserved: Vec<String> =
            RESERVED_PARAM_NAMES.iter().map(|s| s.to_string()).collect();
        reserved.push("agent_config".to_string());
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            reserved.extend(
                ["client", "config_value", "error", "phantom_id", "wire"]
                    .into_iter()
                    .map(str::to_string),
            );
        }
        reserved.extend(constructor_param_names.iter().cloned());
        for i in 0..configs.len() {
            reserved.push(format!("cfg{i}"));
        }
        let ctor_fields = user_supplied_fields(&self.agent_type.constructor.input_schema);
        for i in 0..ctor_fields.len() {
            reserved.push(format!("f{i}"));
        }
        for name in [
            "e0", "k0", "v0", "entries0", "m0", "entry0", "elems0", "tup0", "p0", "vp", "ub",
        ] {
            reserved.push(name.to_string());
        }
        for i in 0..MAX_TUPLE_TEMPS {
            reserved.push(format!("te0_{i}"));
        }
        let reserved_refs: Vec<&str> = reserved.iter().map(|s| s.as_str()).collect();
        unique_idents_with_reserved(
            configs
                .iter()
                .map(|config| {
                    let base = format!(
                        "config_{}",
                        config
                            .path
                            .iter()
                            .map(|s| s.to_snake_case())
                            .collect::<Vec<_>>()
                            .join("_")
                    );
                    to_moonbit_term_ident(&base, self.same_language)
                })
                .collect(),
            &reserved_refs,
        )
    }

    /// Emits the lines that build `agent_config`, an
    /// `Array[@runtime.AgentConfigEntry]` populated from the `Some` config
    /// override parameters. The DTO path uses the original (un-normalized) config
    /// path segments.
    fn write_config_array(
        &self,
        writer: &mut MoonBitWriter,
        configs: &[AgentConfigDeclarationSchema],
        config_names: &[String],
    ) -> anyhow::Result<()> {
        writer.line("let agent_config : Array[@runtime.AgentConfigEntry] = []");
        for (idx, config) in configs.iter().enumerate() {
            let path_lits = config
                .path
                .iter()
                .map(|s| moonbit_string_literal(s))
                .collect::<Vec<_>>()
                .join(", ");
            writer.line(format!("match {} {{", config_names[idx]));
            writer.indent();
            writer.line("Some(value) => {");
            writer.indent();
            let enc = self.encode_expr("value", &config.value_type, 0)?;
            writer.line(format!("let cfg{idx} = {enc}"));
            writer.line(format!(
                "agent_config.push(@runtime.AgentConfigEntry::{{ path: [{path_lits}], value: cfg{idx} }})"
            ));
            writer.dedent();
            writer.line("}");
            writer.line("None => ()");
            writer.dedent();
            writer.line("}");
        }
        Ok(())
    }

    /// Emits the shared body of a constructor: pack the parameters, call
    /// `create_agent`, and build the agent handle. `config_expr` is the
    /// expression passed as the create-agent config array (`"[]"` or
    /// `"agent_config"`).
    fn write_create_agent_body(
        &self,
        writer: &mut MoonBitWriter,
        agent: &str,
        agent_type_name: &str,
        input: &InputSchema,
        create_phantom_expr: &str,
        resolved_phantom_expr: &str,
        config_expr: &str,
    ) -> anyhow::Result<()> {
        writer.line("let configuration = @runtime.get_configuration()");
        self.write_param_record(writer, input)?;
        writer.line(format!(
            "let response = @runtime.create_agent(configuration, {agent_type_name}, parameters, {create_phantom_expr}, {config_expr})"
        ));
        writer.line(format!("{agent}::{{"));
        writer.indent();
        writer.line("resolved: @runtime.ResolvedAgent::{");
        writer.indent();
        writer.line("configuration,");
        writer.line(format!("agent_type_name: {agent_type_name},"));
        writer.line("parameters,");
        writer.line(format!("phantom_id: {resolved_phantom_expr},"));
        writer.line("config: [],");
        writer.line("agent_id: Some(response.agent_id),");
        writer.dedent();
        writer.line("},");
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    fn write_method(
        &self,
        writer: &mut MoonBitWriter,
        agent: &str,
        method: &AgentMethodSchema,
        base: &str,
    ) -> anyhow::Result<()> {
        let method_name_lit = moonbit_string_literal(&method.name);
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_decls = render_param_decls(&param_defs);

        // await
        let (ret_ty, decode) = self.output_return(&method.output_schema)?;
        let await_ret_ty = if self.agent_type.mode == AgentMode::Ephemeral {
            format!("@runtime.InvocationResponse[{ret_ty}]")
        } else {
            ret_ty.clone()
        };
        writer.line(format!(
            "pub async fn {agent}::{base}(self : {}) -> {await_ret_ty} raise {{",
            prepend_self_decl(agent, &param_decls)
        ));
        writer.indent();
        self.write_param_record(writer, &method.input_schema)?;
        match decode {
            Some(decode_block) => {
                writer.line(format!(
                    "let result = @runtime.invoke_agent(self.resolved, {method_name_lit}, parameters, {}, None)",
                    moonbit_string_literal(MODE_AWAIT)
                ));
                writer.line("let value = match result.result {");
                writer.indent();
                writer.line("Some(v) => v");
                writer.line(
                    "None => raise @runtime.BridgeError(\"Missing result value for an await invocation\")",
                );
                writer.dedent();
                writer.line("}");
                if self.agent_type.mode == AgentMode::Ephemeral {
                    writer.line(format!("let decoded = {decode_block}"));
                    writer.line("@runtime.InvocationResponse::{ agent_id: result.agent_id, idempotency_key: result.idempotency_key, value: decoded, component_revision: result.component_revision }");
                } else {
                    writer.line(decode_block);
                }
            }
            None => {
                writer.line(format!(
                    "let result = @runtime.invoke_agent(self.resolved, {method_name_lit}, parameters, {}, None)",
                    moonbit_string_literal(MODE_AWAIT)
                ));
                if self.agent_type.mode == AgentMode::Ephemeral {
                    writer.line("@runtime.InvocationResponse::{ agent_id: result.agent_id, idempotency_key: result.idempotency_key, value: (), component_revision: result.component_revision }");
                }
            }
        }
        writer.dedent();
        writer.line("}");
        writer.blank();

        // trigger (schedule, fire-and-forget)
        writer.line(format!(
            "pub async fn {agent}::trigger_{base}(self : {}) -> {} raise {{",
            prepend_self_decl(agent, &param_decls),
            if self.agent_type.mode == AgentMode::Ephemeral {
                "@runtime.InvocationReceipt"
            } else {
                "Unit"
            }
        ));
        writer.indent();
        self.write_param_record(writer, &method.input_schema)?;
        writer.line(format!(
            "let result = @runtime.invoke_agent(self.resolved, {method_name_lit}, parameters, {}, None)",
            moonbit_string_literal(MODE_SCHEDULE)
        ));
        if self.agent_type.mode == AgentMode::Ephemeral {
            writer.line("@runtime.InvocationReceipt::{ agent_id: result.agent_id, idempotency_key: result.idempotency_key, component_revision: result.component_revision }");
        }
        writer.dedent();
        writer.line("}");
        writer.blank();

        // schedule_at
        let schedule_decls = prepend_param("when : String", &param_decls);
        writer.line(format!(
            "pub async fn {agent}::schedule_{base}(self : {}) -> {} raise {{",
            prepend_self_decl(agent, &schedule_decls),
            if self.agent_type.mode == AgentMode::Ephemeral {
                "@runtime.InvocationReceipt"
            } else {
                "Unit"
            }
        ));
        writer.indent();
        self.write_param_record(writer, &method.input_schema)?;
        writer.line(format!(
            "let result = @runtime.invoke_agent(self.resolved, {method_name_lit}, parameters, {}, Some(when))",
            moonbit_string_literal(MODE_SCHEDULE)
        ));
        if self.agent_type.mode == AgentMode::Ephemeral {
            writer.line("@runtime.InvocationReceipt::{ agent_id: result.agent_id, idempotency_key: result.idempotency_key, component_revision: result.component_revision }");
        }
        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

    /// Emits the parameter-packing lines, ending with
    /// `let parameters = @runtime.RecordValue([...])`.
    fn write_param_record(
        &self,
        writer: &mut MoonBitWriter,
        input: &InputSchema,
    ) -> anyhow::Result<()> {
        // Multimodal input is a single field whose value is the multimodal list;
        // it is still packed as a one-field record on the wire (matching the
        // Rust and TypeScript bridges).
        if let Some(mm) = self.multimodal_input(input)? {
            let fields = user_supplied_fields(input);
            let names = self.input_param_idents(&fields);
            writer.line(format!("let f0 = encode_{}({})", mm.name, names[0]));
            writer.line("let parameters = @runtime.RecordValue([f0])");
            return Ok(());
        }

        let fields = user_supplied_fields(input);
        let names = self.input_param_idents(&fields);
        let mut elems = Vec::new();
        for (idx, field) in fields.iter().enumerate() {
            let enc = self.encode_expr(&names[idx], &field.schema, 0)?;
            writer.line(format!("let f{idx} = {enc}"));
            elems.push(format!("f{idx}"));
        }
        writer.line(format!(
            "let parameters = @runtime.RecordValue([{}])",
            elems.join(", ")
        ));
        Ok(())
    }

    /// The `(returnType, decodeBlock)` for a method's output. `decodeBlock` is a
    /// MoonBit expression decoding the local `value` (the result `SchemaValue`)
    /// into `returnType`; `None` for a unit-returning method.
    fn output_return(&self, output: &OutputSchema) -> anyhow::Result<(String, Option<String>)> {
        if let Some(mm) = self.multimodal_output(output)? {
            return Ok((
                format!("Array[{}]", mm.name),
                Some(format!("decode_{}(value)", mm.name)),
            ));
        }
        match output {
            OutputSchema::Unit => Ok(("Unit".to_string(), None)),
            OutputSchema::Single(ty) => {
                let ret_ty = self.type_reference_with_multimodal(ty)?;
                let decode = self.decode_expr_with_multimodal("value", ty)?;
                Ok((ret_ty, Some(decode)))
            }
        }
    }

    fn input_param_defs(&self, input: &InputSchema) -> anyhow::Result<Vec<(String, String)>> {
        let fields = user_supplied_fields(input);
        let names = self.input_param_idents(&fields);
        if let Some(mm) = self.multimodal_input(input)? {
            return Ok(vec![(names[0].clone(), format!("Array[{}]", mm.name))]);
        }
        let mut defs = Vec::new();
        for (idx, field) in fields.iter().enumerate() {
            defs.push((names[idx].clone(), self.type_reference(&field.schema)?));
        }
        Ok(defs)
    }

    /// If `input` is a single multimodal field, returns the precomputed enum.
    fn multimodal_input(&self, input: &InputSchema) -> anyhow::Result<Option<&MultimodalEnum>> {
        let fields = user_supplied_fields(input);
        if let [field] = fields.as_slice()
            && let Some(cases) = multimodal_variant_cases(self.type_naming.graph(), &field.schema)?
        {
            let pairs = multimodal_pairs(cases)?;
            return Ok(Some(self.multimodal_by_cases(&pairs)?));
        }
        Ok(None)
    }

    /// If `output` is a single multimodal return value, returns the precomputed
    /// enum.
    fn multimodal_output(&self, output: &OutputSchema) -> anyhow::Result<Option<&MultimodalEnum>> {
        if let OutputSchema::Single(ty) = output
            && let Some(cases) = multimodal_variant_cases(self.type_naming.graph(), ty)?
        {
            let pairs = multimodal_pairs(cases)?;
            return Ok(Some(self.multimodal_by_cases(&pairs)?));
        }
        Ok(None)
    }

    fn multimodal_by_cases(
        &self,
        pairs: &[(String, SchemaType)],
    ) -> anyhow::Result<&MultimodalEnum> {
        self.multimodals
            .iter()
            .find(|m| m.cases == pairs)
            .ok_or_else(|| anyhow::anyhow!("Multimodal enum not precomputed for cases: {pairs:?}"))
    }

    fn multimodal_for_type(&self, typ: &SchemaType) -> anyhow::Result<Option<&MultimodalEnum>> {
        if let Some(cases) = multimodal_variant_cases(self.type_naming.graph(), typ)? {
            let pairs = multimodal_pairs(cases)?;
            Ok(Some(self.multimodal_by_cases(&pairs)?))
        } else {
            Ok(None)
        }
    }

    fn type_reference_with_multimodal(&self, typ: &SchemaType) -> anyhow::Result<String> {
        if let Some(multimodal) = self.multimodal_for_type(typ)? {
            Ok(format!("Array[{}]", multimodal.name))
        } else {
            self.type_reference(typ)
        }
    }

    fn encode_expr_with_multimodal(&self, value: &str, typ: &SchemaType) -> anyhow::Result<String> {
        if let Some(multimodal) = self.multimodal_for_type(typ)? {
            Ok(format!("encode_{}({value})", multimodal.name))
        } else {
            self.encode_expr(value, typ, 0)
        }
    }

    fn decode_expr_with_multimodal(&self, value: &str, typ: &SchemaType) -> anyhow::Result<String> {
        if let Some(multimodal) = self.multimodal_for_type(typ)? {
            Ok(format!("decode_{}({value})", multimodal.name))
        } else {
            self.decode_expr(value, typ, 0)
        }
    }

    /// Unique MoonBit parameter identifiers for the given input fields,
    /// disambiguated away from each other and from the internal locals emitted
    /// alongside them.
    fn input_param_idents(&self, fields: &[&NamedField]) -> Vec<String> {
        let mut reserved: Vec<String> =
            RESERVED_PARAM_NAMES.iter().map(|s| s.to_string()).collect();
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            reserved.extend(
                [
                    "agent_config",
                    "client",
                    "config_value",
                    "constructor_input",
                    "error",
                    "f",
                    "input",
                    "phantom_id",
                    "scheduled_at",
                    "tree",
                    "wire",
                ]
                .into_iter()
                .map(str::to_string),
            );
        }
        for i in 0..fields.len() {
            reserved.push(format!("f{i}"));
        }
        // Depth-0 temp-local names used by the structural encoders when packing a
        // top-level parameter, plus the variant payload / union body temps.
        for name in [
            "e0", "k0", "v0", "entries0", "m0", "entry0", "elems0", "tup0", "p0", "vp", "ub",
        ] {
            reserved.push(name.to_string());
        }
        for i in 0..MAX_TUPLE_TEMPS {
            reserved.push(format!("te0_{i}"));
        }
        let reserved_refs: Vec<&str> = reserved.iter().map(|s| s.as_str()).collect();
        unique_idents_with_reserved(
            fields
                .iter()
                .map(|f| to_moonbit_term_ident(&f.name, self.same_language))
                .collect(),
            &reserved_refs,
        )
    }

    /// Computes the base identifier for every method's wrappers in one pass.
    ///
    /// Each method emits three associated functions: `<base>` (await),
    /// `trigger_<base>`, and `schedule_<base>`. A base is chosen so that all
    /// three names are free of the reserved associated-function names and of
    /// every name already claimed by an earlier method (its base and its
    /// `trigger_`/`schedule_` forms), so distinct methods whose names normalize
    /// to the same identifier — or whose names collide with another method's
    /// generated `trigger_`/`schedule_` wrapper — never produce duplicate
    /// definitions. Wire encoding uses the original method name, so renaming a
    /// wrapper never changes the wire format.
    fn method_base_idents(&self, methods: &[AgentMethodSchema]) -> Vec<String> {
        let mut used: std::collections::HashSet<String> = RESERVED_METHOD_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect();
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            used.remove("configure");
            used.remove("agent_id");
            if self.agent_type.mode == AgentMode::Ephemeral {
                used.remove("get");
                used.remove("get_with_config");
            }
            if self.local_configs().is_empty() {
                used.remove("get_with_config");
                used.remove("get_phantom_with_config");
                used.remove("new_phantom_with_config");
            }
            used.extend(
                ["drop", "get_agent_id", "phantom_id"]
                    .into_iter()
                    .map(str::to_string),
            );
            if self.agent_type.mode == AgentMode::Durable {
                used.insert("scoped".to_string());
            }
        }
        let mut bases = Vec::with_capacity(methods.len());
        for method in methods {
            let ident = to_moonbit_term_ident(&method.name, self.same_language);
            let mut candidate = ident.clone();
            let mut n = 2;
            while used.contains(&candidate)
                || used.contains(&format!("trigger_{candidate}"))
                || used.contains(&format!("schedule_{candidate}"))
                || (self.mode == MoonBitBridgeMode::GuestWasmRpc
                    && used.contains(&format!("schedule_cancelable_{candidate}")))
            {
                candidate = format!("{ident}_{n}");
                n += 1;
            }
            used.insert(candidate.clone());
            used.insert(format!("trigger_{candidate}"));
            used.insert(format!("schedule_{candidate}"));
            if self.mode == MoonBitBridgeMode::GuestWasmRpc {
                used.insert(format!("schedule_cancelable_{candidate}"));
            }
            bases.push(candidate);
        }
        bases
    }

    fn record_field_idents(&self, fields: &[golem_common::schema::NamedFieldType]) -> Vec<String> {
        self.record_field_idents_from(fields.iter().map(|f| f.name.as_str()))
    }

    fn record_field_idents_from<'a>(&self, names: impl Iterator<Item = &'a str>) -> Vec<String> {
        unique_idents(
            names
                .map(|n| to_moonbit_term_ident(n, self.same_language))
                .collect(),
        )
    }

    fn variant_case_idents<'a>(&self, names: impl Iterator<Item = &'a str>) -> Vec<String> {
        unique_idents(
            names
                .map(|n| to_moonbit_constructor_ident(n, self.same_language))
                .collect(),
        )
    }

    // --- Codec dispatchers --------------------------------------------------

    fn encode_expr(&self, val: &str, typ: &SchemaType, depth: usize) -> anyhow::Result<String> {
        if self.mode == MoonBitBridgeMode::GuestWasmRpc
            && (unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some()
                || unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some())
        {
            return self.encode_structural(val, typ, depth);
        }
        if let Some(name) = self.type_naming.type_name_for_type(typ)
            && is_named_composite(self.resolve_ref(typ))
        {
            return Ok(format!("encode_{}({val})", name.name));
        }
        self.encode_structural(val, typ, depth)
    }

    fn decode_expr(&self, val: &str, typ: &SchemaType, depth: usize) -> anyhow::Result<String> {
        if self.mode == MoonBitBridgeMode::GuestWasmRpc
            && (unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some()
                || unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some())
        {
            return self.decode_structural(val, typ, depth);
        }
        if let Some(name) = self.type_naming.type_name_for_type(typ)
            && is_named_composite(self.resolve_ref(typ))
        {
            return Ok(format!("decode_{}({val})", name.name));
        }
        self.decode_structural(val, typ, depth)
    }

    fn encode_structural(
        &self,
        val: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if let Some(restrictions) = unstructured_text_restrictions(self.type_naming.graph(), typ)? {
            if self.mode == MoonBitBridgeMode::GuestWasmRpc {
                let allowed = moonbit_string_array(restrictions.languages.as_deref());
                return Ok(format!("guest_encode_unstructured_text({val}, {allowed})"));
            }
            return Ok(format!("@runtime.unstructured_text_to_schema_value({val})"));
        }
        if let Some(restrictions) = unstructured_binary_restrictions(self.type_naming.graph(), typ)?
        {
            if self.mode == MoonBitBridgeMode::GuestWasmRpc {
                let allowed = moonbit_string_array(restrictions.mime_types.as_deref());
                return Ok(format!(
                    "guest_encode_unstructured_binary({val}, {allowed})"
                ));
            }
            return Ok(format!(
                "@runtime.unstructured_binary_to_schema_value({val})"
            ));
        }

        let resolved = self.resolve_ref(typ);
        let e = format!("e{depth}");
        let next = depth + 1;
        let rendered = match resolved {
            SchemaType::Bool { .. } => format!("@runtime.BoolValue({val})"),
            SchemaType::S8 { .. } => match self.mode {
                MoonBitBridgeMode::ExternalRest => format!("@runtime.S8Value({val})"),
                MoonBitBridgeMode::GuestWasmRpc => format!(
                    "{{ if {val} < -128 || {val} > 127 {{ raise @runtime.BridgeError(\"s8 value out of range\") }}; @runtime.S8Value({val}) }}"
                ),
            },
            SchemaType::S16 { .. } => match self.mode {
                MoonBitBridgeMode::ExternalRest => format!("@runtime.S16Value({val})"),
                MoonBitBridgeMode::GuestWasmRpc => format!(
                    "{{ if {val} < -32768 || {val} > 32767 {{ raise @runtime.BridgeError(\"s16 value out of range\") }}; @runtime.S16Value({val}) }}"
                ),
            },
            SchemaType::S32 { .. } => format!("@runtime.S32Value({val})"),
            SchemaType::S64 { .. } => format!("@runtime.S64Value({val})"),
            SchemaType::U8 { .. } => match self.mode {
                MoonBitBridgeMode::ExternalRest => format!("@runtime.U8Value({val}.to_int())"),
                MoonBitBridgeMode::GuestWasmRpc => format!("@runtime.U8Value({val})"),
            },
            SchemaType::U16 { .. } => match self.mode {
                MoonBitBridgeMode::ExternalRest => format!("@runtime.U16Value({val})"),
                MoonBitBridgeMode::GuestWasmRpc => format!(
                    "{{ if {val} < 0 || {val} > 65535 {{ raise @runtime.BridgeError(\"u16 value out of range\") }}; @runtime.U16Value({val}.reinterpret_as_uint()) }}"
                ),
            },
            SchemaType::U32 { .. } => format!("@runtime.U32Value({val})"),
            SchemaType::U64 { .. } => format!("@runtime.U64Value({val})"),
            SchemaType::F32 { .. } => format!("@runtime.F32Value({val})"),
            SchemaType::F64 { .. } => format!("@runtime.F64Value({val})"),
            SchemaType::Char { .. } => match self.mode {
                MoonBitBridgeMode::ExternalRest => format!("@runtime.CharValue({val}.to_int())"),
                MoonBitBridgeMode::GuestWasmRpc => format!("@runtime.CharValue({val})"),
            },
            SchemaType::String { .. } => format!("@runtime.StringValue({val})"),
            SchemaType::Option { inner, .. } => {
                let inner_enc = self.encode_expr(&e, inner, next)?;
                format!("@runtime.OptionValue({val}.map(({e}) => {inner_enc}))")
            }
            SchemaType::List { element, .. } => {
                let inner_enc = self.encode_expr(&e, element, next)?;
                format!("@runtime.ListValue({val}.map(({e}) => {inner_enc}))")
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner_enc = self.encode_expr(&e, element, next)?;
                match self.mode {
                    MoonBitBridgeMode::ExternalRest => {
                        format!("@runtime.FixedListValue({val}.map(({e}) => {inner_enc}))")
                    }
                    MoonBitBridgeMode::GuestWasmRpc => format!(
                        "{{\n  if {val}.length() != {length} {{ raise @runtime.BridgeError(\"Expected fixed-list of length {length}, got \\{{{val}.length()}}\") }}\n  @runtime.FixedListValue({val}.map(({e}) => {inner_enc}))\n}}"
                    ),
                }
            }
            SchemaType::Map { key, value, .. } => {
                let entries = format!("entries{depth}");
                let k = format!("k{depth}");
                let v = format!("v{depth}");
                let key_enc = self.encode_expr(&k, key, next)?;
                let val_enc = self.encode_expr(&v, value, next)?;
                format!(
                    "{{\n  let {entries} : Array[@runtime.SchemaMapEntry] = []\n  {val}.each(({k}, {v}) => {entries}.push(@runtime.SchemaMapEntry::{{ key: {key_enc}, value: {val_enc} }}))\n  @runtime.MapValue({entries})\n}}"
                )
            }
            SchemaType::Tuple { elements, .. } => self.encode_tuple(val, elements, depth)?,
            SchemaType::Result { spec, .. } => {
                let r = format!("r{depth}");
                let l = format!("l{depth}");
                let p = format!("p{depth}");
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let enc = self.encode_expr(&r, ok_type, next)?;
                        match self.mode {
                            MoonBitBridgeMode::ExternalRest => format!(
                                "Ok({r}) => {{ let {p} = {enc}; @runtime.ResultValue(@runtime.ResultOk(Some({p}))) }}"
                            ),
                            MoonBitBridgeMode::GuestWasmRpc => format!(
                                "Ok({r}) => {{ let {p} = {enc}; @runtime.ResultOk(Some({p})) }}"
                            ),
                        }
                    }
                    None => match self.mode {
                        MoonBitBridgeMode::ExternalRest => {
                            "Ok(_) => @runtime.ResultValue(@runtime.ResultOk(None))".to_string()
                        }
                        MoonBitBridgeMode::GuestWasmRpc => {
                            "Ok(_) => @runtime.ResultOk(None)".to_string()
                        }
                    },
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let enc = self.encode_expr(&l, err_type, next)?;
                        match self.mode {
                            MoonBitBridgeMode::ExternalRest => format!(
                                "Err({l}) => {{ let {p} = {enc}; @runtime.ResultValue(@runtime.ResultErr(Some({p}))) }}"
                            ),
                            MoonBitBridgeMode::GuestWasmRpc => format!(
                                "Err({l}) => {{ let {p} = {enc}; @runtime.ResultErr(Some({p})) }}"
                            ),
                        }
                    }
                    None => match self.mode {
                        MoonBitBridgeMode::ExternalRest => {
                            "Err(_) => @runtime.ResultValue(@runtime.ResultErr(None))".to_string()
                        }
                        MoonBitBridgeMode::GuestWasmRpc => {
                            "Err(_) => @runtime.ResultErr(None)".to_string()
                        }
                    },
                };
                format!("match {val} {{\n  {ok_arm}\n  {err_arm}\n}}")
            }
            SchemaType::Path { .. } => format!("@runtime.PathValue({val})"),
            SchemaType::Url { .. } => format!("@runtime.UrlValue({val})"),
            SchemaType::Datetime { .. } => format!("@runtime.DatetimeValue({val})"),
            SchemaType::Duration { .. } => format!("@runtime.DurationValue({val})"),
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => bail!(
                "Composite schema type reached encode_structural without a registered name: {resolved:?}"
            ),
            SchemaType::Ref { .. } => unreachable!("Ref was resolved to its body via resolve_ref"),
            SchemaType::Text { .. } | SchemaType::Binary { .. } => bail!(
                "Bare text/binary rich scalars have no MoonBit bridge encoding; \
                 wrap them in the unstructured text/binary variant ({resolved:?})"
            ),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                bail!(
                    "Cannot encode unsupported schema variant in the MoonBit bridge: {resolved:?}"
                )
            }
        };
        Ok(rendered)
    }

    fn decode_structural(
        &self,
        val: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if let Some(restrictions) = unstructured_text_restrictions(self.type_naming.graph(), typ)? {
            let allowed = restrictions
                .languages
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|code| moonbit_string_literal(code))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(if self.mode == MoonBitBridgeMode::GuestWasmRpc {
                format!("guest_decode_unstructured_text({val}, [{allowed}])")
            } else {
                format!(
                    "@runtime.unstructured_text_from_schema_value(\"output\", {val}, [{allowed}])"
                )
            });
        }
        if let Some(restrictions) = unstructured_binary_restrictions(self.type_naming.graph(), typ)?
        {
            let allowed = restrictions
                .mime_types
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|mime| moonbit_string_literal(mime))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(if self.mode == MoonBitBridgeMode::GuestWasmRpc {
                format!("guest_decode_unstructured_binary({val}, [{allowed}])")
            } else {
                format!(
                    "@runtime.unstructured_binary_from_schema_value(\"output\", {val}, [{allowed}])"
                )
            });
        }

        let resolved = self.resolve_ref(typ);
        let e = format!("e{depth}");
        let next = depth + 1;
        let rendered = match resolved {
            SchemaType::Bool { .. } => format!("@runtime.as_bool({val})"),
            SchemaType::S8 { .. } => format!("@runtime.as_s8({val})"),
            SchemaType::S16 { .. } => format!("@runtime.as_s16({val})"),
            SchemaType::S32 { .. } => format!("@runtime.as_s32({val})"),
            SchemaType::S64 { .. } => format!("@runtime.as_s64({val})"),
            SchemaType::U8 { .. } => format!("@runtime.as_u8({val})"),
            SchemaType::U16 { .. } => format!("@runtime.as_u16({val})"),
            SchemaType::U32 { .. } => format!("@runtime.as_u32({val})"),
            SchemaType::U64 { .. } => format!("@runtime.as_u64({val})"),
            SchemaType::F32 { .. } => format!("@runtime.as_f32({val})"),
            SchemaType::F64 { .. } => format!("@runtime.as_f64({val})"),
            SchemaType::Char { .. } => format!("@runtime.as_char({val})"),
            SchemaType::String { .. } => format!("@runtime.as_string({val})"),
            SchemaType::Option { inner, .. } => {
                let inner_dec = self.decode_expr(&e, inner, next)?;
                format!("@runtime.as_option({val}).map(({e}) => {inner_dec})")
            }
            SchemaType::List { element, .. } => {
                let inner_dec = self.decode_expr(&e, element, next)?;
                format!("@runtime.as_list({val}).map(({e}) => {inner_dec})")
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner_dec = self.decode_expr(&e, element, next)?;
                let len = *length;
                let elems = format!("elems{depth}");
                format!(
                    "{{\n  let {elems} = @runtime.as_fixed_list({val})\n  if {elems}.length() != {len} {{ raise @runtime.BridgeError(\"Expected fixed-list of length {len}, got \\{{{elems}.length()}}\") }}\n  {elems}.map(({e}) => {inner_dec})\n}}"
                )
            }
            SchemaType::Map { key, value, .. } => {
                let k_ty = self.type_reference(key)?;
                let v_ty = self.type_reference(value)?;
                let m = format!("m{depth}");
                let entry = format!("entry{depth}");
                let k = format!("k{depth}");
                let v = format!("v{depth}");
                let key_dec = self.decode_expr(&format!("{entry}.key"), key, next)?;
                let val_dec = self.decode_expr(&format!("{entry}.value"), value, next)?;
                format!(
                    "{{\n  let {m} : Map[{k_ty}, {v_ty}] = {{}}\n  for {entry} in @runtime.as_map({val}) {{\n    let {k} = {key_dec}\n    let {v} = {val_dec}\n    {m}[{k}] = {v}\n  }}\n  {m}\n}}"
                )
            }
            SchemaType::Tuple { elements, .. } => self.decode_tuple(val, elements, depth)?,
            SchemaType::Result { spec, .. } => {
                let payload = format!("payload{depth}");
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let dec = self.decode_expr(&payload, ok_type, next)?;
                        format!(
                            "@runtime.ResultOk({payload}) => Ok({{ let {payload} = @runtime.required_payload({payload}, \"result ok\"); {dec} }})"
                        )
                    }
                    None => format!(
                        "@runtime.ResultOk({payload}) => {{ if {payload} is Some(_) {{ raise @runtime.BridgeError(\"Unexpected payload for unit result ok\") }}\n    Ok(()) }}"
                    ),
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let dec = self.decode_expr(&payload, err_type, next)?;
                        format!(
                            "@runtime.ResultErr({payload}) => Err({{ let {payload} = @runtime.required_payload({payload}, \"result err\"); {dec} }})"
                        )
                    }
                    None => format!(
                        "@runtime.ResultErr({payload}) => {{ if {payload} is Some(_) {{ raise @runtime.BridgeError(\"Unexpected payload for unit result err\") }}\n    Err(()) }}"
                    ),
                };
                let fallback = if self.mode == MoonBitBridgeMode::GuestWasmRpc {
                    "\n  other => codec_mismatch(\"result\", other)"
                } else {
                    ""
                };
                format!("match @runtime.as_result({val}) {{\n  {ok_arm}\n  {err_arm}{fallback}\n}}")
            }
            SchemaType::Path { .. } => format!("@runtime.as_path({val})"),
            SchemaType::Url { .. } => format!("@runtime.as_url({val})"),
            SchemaType::Datetime { .. } => format!("@runtime.as_datetime({val})"),
            SchemaType::Duration { .. } => format!("@runtime.as_duration({val})"),
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => bail!(
                "Composite schema type reached decode_structural without a registered name: {resolved:?}"
            ),
            SchemaType::Ref { .. } => unreachable!("Ref was resolved to its body via resolve_ref"),
            SchemaType::Text { .. } | SchemaType::Binary { .. } => bail!(
                "Bare text/binary rich scalars have no MoonBit bridge decoding; \
                 wrap them in the unstructured text/binary variant ({resolved:?})"
            ),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                bail!(
                    "Cannot decode unsupported schema variant in the MoonBit bridge: {resolved:?}"
                )
            }
        };
        Ok(rendered)
    }

    fn encode_tuple(
        &self,
        val: &str,
        elements: &[SchemaType],
        depth: usize,
    ) -> anyhow::Result<String> {
        if elements.is_empty() {
            return Ok("@runtime.TupleValue([])".to_string());
        }
        let next = depth + 1;
        if elements.len() == 1 {
            let enc = self.encode_expr(val, &elements[0], next)?;
            return Ok(format!(
                "{{\n  let te{depth}_0 = {enc}\n  @runtime.TupleValue([te{depth}_0])\n}}"
            ));
        }
        let t = format!("tup{depth}");
        let mut lines = vec![format!("  let {t} = {val}")];
        let mut names = Vec::new();
        for (idx, element) in elements.iter().enumerate() {
            let enc = self.encode_expr(&format!("{t}.{idx}"), element, next)?;
            lines.push(format!("  let te{depth}_{idx} = {enc}"));
            names.push(format!("te{depth}_{idx}"));
        }
        lines.push(format!("  @runtime.TupleValue([{}])", names.join(", ")));
        Ok(format!("{{\n{}\n}}", lines.join("\n")))
    }

    fn decode_tuple(
        &self,
        val: &str,
        elements: &[SchemaType],
        depth: usize,
    ) -> anyhow::Result<String> {
        let elems = format!("elems{depth}");
        let n = elements.len();
        let next = depth + 1;
        if elements.is_empty() {
            return Ok(format!(
                "{{\n  let {elems} = @runtime.as_tuple({val})\n  if {elems}.length() != 0 {{ raise @runtime.BridgeError(\"Expected empty tuple, got \\{{{elems}.length()}} elements\") }}\n  ()\n}}"
            ));
        }
        if elements.len() == 1 {
            let dec = self.decode_expr(&format!("{elems}[0]"), &elements[0], next)?;
            return Ok(format!(
                "{{\n  let {elems} = @runtime.as_tuple({val})\n  if {elems}.length() != 1 {{ raise @runtime.BridgeError(\"Expected tuple of arity 1, got \\{{{elems}.length()}}\") }}\n  {dec}\n}}"
            ));
        }
        let mut lines = vec![
            format!("  let {elems} = @runtime.as_tuple({val})"),
            format!(
                "  if {elems}.length() != {n} {{ raise @runtime.BridgeError(\"Expected tuple of arity {n}, got \\{{{elems}.length()}}\") }}"
            ),
        ];
        let mut names = Vec::new();
        for (idx, element) in elements.iter().enumerate() {
            let dec = self.decode_expr(&format!("{elems}[{idx}]"), element, next)?;
            lines.push(format!("  let te{depth}_{idx} = {dec}"));
            names.push(format!("te{depth}_{idx}"));
        }
        lines.push(format!("  ({})", names.join(", ")));
        Ok(format!("{{\n{}\n}}", lines.join("\n")))
    }

    // --- Type references ----------------------------------------------------

    fn type_reference(&self, typ: &SchemaType) -> anyhow::Result<String> {
        if self.mode == MoonBitBridgeMode::GuestWasmRpc {
            if unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some() {
                return Ok("UnstructuredText".to_string());
            }
            if unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some() {
                return Ok("UnstructuredBinary".to_string());
            }
        }
        if let Some(name) = self.type_naming.type_name_for_type(typ)
            && is_named_composite(self.resolve_ref(typ))
        {
            return Ok(name.name.clone());
        }

        if unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(match self.mode {
                MoonBitBridgeMode::ExternalRest => "@runtime.UnstructuredText",
                MoonBitBridgeMode::GuestWasmRpc => "UnstructuredText",
            }
            .to_string());
        }
        if unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(match self.mode {
                MoonBitBridgeMode::ExternalRest => "@runtime.UnstructuredBinary",
                MoonBitBridgeMode::GuestWasmRpc => "UnstructuredBinary",
            }
            .to_string());
        }

        if self.type_naming.is_recursive_ref(typ) && !is_named_composite(self.resolve_ref(typ)) {
            bail!(
                "Recursive non-composite type alias cannot be represented in the MoonBit bridge: {typ:?}"
            );
        }

        let resolved = self.resolve_ref(typ);
        match resolved {
            SchemaType::Bool { .. } => Ok("Bool".to_string()),
            SchemaType::S8 { .. } | SchemaType::S16 { .. } | SchemaType::S32 { .. } => {
                Ok("Int".to_string())
            }
            SchemaType::S64 { .. } => Ok("Int64".to_string()),
            SchemaType::U8 { .. } => Ok("Byte".to_string()),
            SchemaType::U16 { .. } => Ok("Int".to_string()),
            SchemaType::U32 { .. } => Ok("UInt".to_string()),
            SchemaType::U64 { .. } => Ok("UInt64".to_string()),
            SchemaType::F32 { .. } => Ok("Float".to_string()),
            SchemaType::F64 { .. } => Ok("Double".to_string()),
            SchemaType::Char { .. } => Ok("Char".to_string()),
            SchemaType::String { .. } => Ok("String".to_string()),
            SchemaType::Option { inner, .. } => Ok(format!("{}?", self.type_reference(inner)?)),
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                Ok(format!("Array[{}]", self.type_reference(element)?))
            }
            SchemaType::Map { key, value, .. } => Ok(format!(
                "Map[{}, {}]",
                self.type_reference(key)?,
                self.type_reference(value)?
            )),
            SchemaType::Tuple { elements, .. } => self.tuple_type(elements),
            SchemaType::Result { spec, .. } => {
                let ok_type = match spec.ok.as_deref() {
                    Some(ty) => self.type_reference(ty)?,
                    None => "Unit".to_string(),
                };
                let err_type = match spec.err.as_deref() {
                    Some(ty) => self.type_reference(ty)?,
                    None => "Unit".to_string(),
                };
                Ok(format!("Result[{ok_type}, {err_type}]"))
            }
            SchemaType::Path { .. } | SchemaType::Url { .. } => Ok("String".to_string()),
            SchemaType::Datetime { .. } => Ok(match self.mode {
                MoonBitBridgeMode::ExternalRest => "String",
                MoonBitBridgeMode::GuestWasmRpc => "@types.Datetime",
            }
            .to_string()),
            SchemaType::Duration { .. } => Ok("Int64".to_string()),
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => bail!(
                "Composite schema type reached type_reference without a registered name: {resolved:?}"
            ),
            SchemaType::Ref { .. } => unreachable!("Ref was resolved to its body via resolve_ref"),
            SchemaType::Text { .. } | SchemaType::Binary { .. } => bail!(
                "Bare text/binary rich scalars have no MoonBit bridge type; \
                 wrap them in the unstructured text/binary variant ({resolved:?})"
            ),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => bail!(
                "Cannot emit MoonBit type reference for unsupported schema variant: {resolved:?}"
            ),
        }
    }

    fn tuple_type(&self, elements: &[SchemaType]) -> anyhow::Result<String> {
        let types = elements
            .iter()
            .map(|item| self.type_reference(item))
            .collect::<anyhow::Result<Vec<_>>>()?;
        match types.as_slice() {
            // An empty tuple is the unit value.
            [] => Ok("Unit".to_string()),
            // MoonBit has no 1-tuple type, so a single-element tuple maps to the
            // bare element type; encode/decode still wrap it in a tuple node.
            [single] => Ok(single.clone()),
            _ => Ok(format!("({})", types.join(", "))),
        }
    }

    fn resolve_ref<'a>(&'a self, typ: &'a SchemaType) -> &'a SchemaType {
        self.type_naming
            .graph()
            .resolve_ref(typ)
            .expect("bridge schemas contain only resolvable references")
    }
}

/// Largest tuple-element temp index reserved against user parameter names.
const MAX_TUPLE_TEMPS: usize = 64;

/// Converts the variant cases of a multimodal `list<variant<…>>` into
/// `(case_name, payload_schema)` pairs, preserving schema order (the index is
/// the wire variant tag). Every modality must carry a payload schema.
fn multimodal_pairs(cases: &[VariantCaseType]) -> anyhow::Result<Vec<(String, SchemaType)>> {
    cases
        .iter()
        .map(|case| {
            let payload = case.payload.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "Multimodal case `{}` has no payload schema; expected a modality body",
                    case.name
                )
            })?;
            Ok((case.name.clone(), payload))
        })
        .collect()
}

/// Renders `(name : type)` parameter declarations joined by `, `.
fn render_param_decls(defs: &[(String, String)]) -> String {
    defs.iter()
        .map(|(name, ty)| format!("{name} : {ty}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Prepends a `self : Agent` declaration to a parameter declaration list.
fn prepend_self_decl(agent: &str, decls: &str) -> String {
    if decls.is_empty() {
        agent.to_string()
    } else {
        format!("{agent}, {decls}")
    }
}

/// Appends an extra parameter declaration after a declaration list. The
/// synthetic `phantom` parameter comes last, after the user's constructor
/// parameters (matching the MoonBit SDK's RPC client, whose `get_phantom`
/// takes `phantom_id` after the constructor arguments).
fn append_param(extra: &str, decls: &str) -> String {
    if decls.is_empty() {
        extra.to_string()
    } else {
        format!("{decls}, {extra}")
    }
}

/// Prepends an extra parameter declaration before a declaration list. The
/// synthetic `when` parameter for scheduled invocations comes first, before the
/// user's method parameters (matching the MoonBit SDK's RPC client, whose
/// `schedule_*` wrappers take `scheduled_at` ahead of the method arguments).
fn prepend_param(extra: &str, decls: &str) -> String {
    if decls.is_empty() {
        extra.to_string()
    } else {
        format!("{extra}, {decls}")
    }
}

/// Appends an extra call argument after an argument-name list.
fn append_arg(extra: &str, names: &[String]) -> String {
    if names.is_empty() {
        extra.to_string()
    } else {
        format!("{}, {extra}", names.join(", "))
    }
}

/// Joins an argument-name list followed by extra call arguments, in order.
fn append_arg_list(names: &[String], extras: &[String]) -> String {
    names
        .iter()
        .chain(extras.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders `value` as a double-quoted MoonBit string literal, escaping the
/// characters that would otherwise break out of, or be misinterpreted inside,
/// the literal. A bare `{` is a literal brace in MoonBit; interpolation is
/// triggered only by `\{`, so escaping `\` is sufficient to prevent accidental
/// interpolation and `{` must be left untouched.
fn moonbit_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

/// Whether a (ref-resolved) schema type becomes a generated MoonBit definition
/// (struct / enum). Other named defs are inlined at their use sites.
fn is_named_composite(resolved: &SchemaType) -> bool {
    matches!(
        resolved,
        SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. }
    )
}

/// Name of the generated agent handle struct, e.g. `foo-agent` -> `FooAgent`.
fn agent_struct_name(agent_type: &AgentTypeSchema) -> String {
    let name = agent_type.type_name.as_str().to_upper_camel_case();
    match name.chars().next() {
        Some(first) if first.is_ascii_uppercase() => name,
        _ => format!("Agent{name}"),
    }
}

fn guest_client_struct_name(agent_type: &AgentTypeSchema) -> String {
    format!("{}Client", agent_struct_name(agent_type))
}

/// Recursively writes every file of an embedded [`Dir`] under `dest`, preserving
/// the embedded relative path of each file.
fn write_dir(dir: &Dir<'_>, dest: &Utf8Path) -> anyhow::Result<()> {
    for file in dir.files() {
        let relative = Utf8Path::from_path(file.path()).with_context(|| {
            format!(
                "Embedded runtime path is not valid UTF-8: {:?}",
                file.path()
            )
        })?;
        let target = dest.join(relative);
        let contents = file.contents_utf8().with_context(|| {
            format!(
                "Embedded runtime file is not valid UTF-8: {:?}",
                file.path()
            )
        })?;
        fs::write_str(target, contents)?;
    }
    for sub in dir.dirs() {
        write_dir(sub, dest)?;
    }
    Ok(())
}

fn moonbit_string_array(values: Option<&[String]>) -> String {
    format!(
        "[{}]",
        values
            .unwrap_or_default()
            .iter()
            .map(|value| moonbit_string_literal(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Emits an exact recursive MoonBit `@model.SchemaGraph` literal. This is
/// intentionally independent of bridge mode and call sites so agent config and
/// tool-input generation can share the same schema source representation.
pub fn emit_schema_graph_literal(graph: &SchemaGraph) -> String {
    let defs = graph
        .defs
        .iter()
        .map(|d| {
            format!(
                "@model.SchemaTypeDef::{{ id: {}, name: {}, body: {} }}",
                moonbit_string_literal(d.id.as_str()),
                mb_opt_str(d.name.as_deref()),
                emit_schema_type(&d.body)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "@model.SchemaGraph::{{ defs: [{defs}], root: {} }}",
        emit_schema_type(&graph.root)
    )
}

fn emit_schema_type(ty: &SchemaType) -> String {
    use SchemaType::*;
    let body = match ty {
        Ref { id, .. } => format!("@model.Ref({})", moonbit_string_literal(id.as_str())),
        Bool { .. } => "@model.Bool".into(),
        Char { .. } => "@model.Char".into(),
        String { .. } => "@model.String".into(),
        S8 { restrictions, .. } => mb_numeric("S8", restrictions.as_ref()),
        S16 { restrictions, .. } => mb_numeric("S16", restrictions.as_ref()),
        S32 { restrictions, .. } => mb_numeric("S32", restrictions.as_ref()),
        S64 { restrictions, .. } => mb_numeric("S64", restrictions.as_ref()),
        U8 { restrictions, .. } => mb_numeric("U8", restrictions.as_ref()),
        U16 { restrictions, .. } => mb_numeric("U16", restrictions.as_ref()),
        U32 { restrictions, .. } => mb_numeric("U32", restrictions.as_ref()),
        U64 { restrictions, .. } => mb_numeric("U64", restrictions.as_ref()),
        F32 { restrictions, .. } => mb_numeric("F32", restrictions.as_ref()),
        F64 { restrictions, .. } => mb_numeric("F64", restrictions.as_ref()),
        Record { fields, .. } => format!(
            "@model.Record([{}])",
            fields
                .iter()
                .map(|f| format!(
                    "@model.NamedFieldType::{{ name: {}, body: {}, metadata: {} }}",
                    moonbit_string_literal(&f.name),
                    emit_schema_type(&f.body),
                    mb_metadata(&f.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Variant { cases, .. } => format!(
            "@model.Variant([{}])",
            cases
                .iter()
                .map(|c| format!(
                    "@model.VariantCaseType::{{ name: {}, payload: {}, metadata: {} }}",
                    moonbit_string_literal(&c.name),
                    mb_opt_type(c.payload.as_ref()),
                    mb_metadata(&c.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Enum { cases, .. } => format!("@model.Enum({})", mb_strings(cases)),
        Flags { flags, .. } => format!("@model.Flags({})", mb_strings(flags)),
        Tuple { elements, .. } => format!(
            "@model.Tuple([{}])",
            elements
                .iter()
                .map(emit_schema_type)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        List { element, .. } => format!("@model.List({})", emit_schema_type(element)),
        FixedList {
            element, length, ..
        } => format!(
            "@model.FixedList({}, {}U)",
            emit_schema_type(element),
            length
        ),
        Map { key, value, .. } => format!(
            "@model.Map({}, {})",
            emit_schema_type(key),
            emit_schema_type(value)
        ),
        Option { inner, .. } => format!("@model.Option({})", emit_schema_type(inner)),
        Result { spec, .. } => format!(
            "@model.Result({}, {})",
            mb_opt_type(spec.ok.as_deref()),
            mb_opt_type(spec.err.as_deref())
        ),
        Text { restrictions, .. } => format!(
            "@model.Text(@types.TextRestrictions::{{ languages: {}, min_length: {}, max_length: {}, regex: {} }})",
            mb_opt_strings(restrictions.languages.as_deref()),
            mb_opt_u32(restrictions.min_length),
            mb_opt_u32(restrictions.max_length),
            mb_opt_str(restrictions.regex.as_deref())
        ),
        Binary { restrictions, .. } => format!(
            "@model.Binary(@types.BinaryRestrictions::{{ mime_types: {}, min_bytes: {}, max_bytes: {} }})",
            mb_opt_strings(restrictions.mime_types.as_deref()),
            mb_opt_u32(restrictions.min_bytes),
            mb_opt_u32(restrictions.max_bytes)
        ),
        Path { spec, .. } => format!(
            "@model.Path(@types.PathSpec::{{ direction: @types.{}, kind: @types.{}, allowed_mime_types: {}, allowed_extensions: {} }})",
            mb_path_direction(spec.direction),
            mb_path_kind(spec.kind),
            mb_opt_strings(spec.allowed_mime_types.as_deref()),
            mb_opt_strings(spec.allowed_extensions.as_deref())
        ),
        Url { restrictions, .. } => format!(
            "@model.Url(@types.UrlRestrictions::{{ allowed_schemes: {}, allowed_hosts: {} }})",
            mb_opt_strings(restrictions.allowed_schemes.as_deref()),
            mb_opt_strings(restrictions.allowed_hosts.as_deref())
        ),
        Datetime { .. } => "@model.Datetime".into(),
        Duration { .. } => "@model.Duration".into(),
        Quantity { spec, .. } => format!(
            "@model.Quantity(@types.QuantitySpec::{{ base_unit: {}, allowed_suffixes: {}, min: {}, max: {} }})",
            moonbit_string_literal(&spec.base_unit),
            mb_strings(&spec.allowed_suffixes),
            mb_quantity(spec.min.as_ref()),
            mb_quantity(spec.max.as_ref())
        ),
        Union { spec, .. } => format!(
            "@model.Union([{}])",
            spec.branches
                .iter()
                .map(|b| format!(
                    "@model.UnionBranch::{{ tag: {}, body: {}, discriminator: {}, metadata: {} }}",
                    moonbit_string_literal(&b.tag),
                    emit_schema_type(&b.body),
                    mb_discriminator(&b.discriminator),
                    mb_metadata(&b.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Secret { spec, .. } => format!(
            "@model.Secret(@model.SecretSpec::{{ inner: {}, category: {} }})",
            emit_schema_type(&spec.inner),
            mb_opt_str(spec.category.as_deref())
        ),
        QuotaToken { spec, .. } => format!(
            "@model.QuotaToken(@types.QuotaTokenSpec::{{ resource_name: {} }})",
            mb_opt_str(spec.resource_name.as_deref())
        ),
        Future { inner, .. } => format!("@model.Future({})", mb_opt_type(inner.as_deref())),
        Stream { inner, .. } => format!("@model.Stream({})", mb_opt_type(inner.as_deref())),
    };
    format!(
        "@model.SchemaType::{{ body: {body}, metadata: {} }}",
        mb_metadata(ty.metadata())
    )
}

fn mb_opt_str(value: Option<&str>) -> String {
    value
        .map(|v| format!("Some({})", moonbit_string_literal(v)))
        .unwrap_or_else(|| "None".into())
}
fn mb_strings(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|v| moonbit_string_literal(v))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn mb_opt_strings(values: Option<&[String]>) -> String {
    values
        .map(|v| format!("Some({})", mb_strings(v)))
        .unwrap_or_else(|| "None".into())
}
fn mb_opt_u32(value: Option<u32>) -> String {
    value
        .map(|v| format!("Some({v}U)"))
        .unwrap_or_else(|| "None".into())
}
fn mb_opt_type(value: Option<&SchemaType>) -> String {
    value
        .map(|v| format!("Some({})", emit_schema_type(v)))
        .unwrap_or_else(|| "None".into())
}
fn mb_metadata(value: &MetadataEnvelope) -> String {
    let role = match value.role.as_ref() {
        None => "None".into(),
        Some(Role::Multimodal) => "Some(@types.Multimodal)".into(),
        Some(Role::UnstructuredText) => "Some(@types.UnstructuredText)".into(),
        Some(Role::UnstructuredBinary) => "Some(@types.UnstructuredBinary)".into(),
        Some(Role::Other(v)) => format!("Some(@types.Other({}))", moonbit_string_literal(v)),
    };
    format!(
        "@types.MetadataEnvelope::{{ doc: {}, aliases: {}, examples: {}, deprecated: {}, role: {role} }}",
        mb_opt_str(value.doc.as_deref()),
        mb_strings(&value.aliases),
        mb_strings(&value.examples),
        mb_opt_str(value.deprecated.as_deref())
    )
}
fn mb_bound(value: NumericBound) -> String {
    match value {
        NumericBound::Signed(v) => format!("@types.Signed({v}L)"),
        NumericBound::Unsigned(v) => format!("@types.Unsigned({v}UL)"),
        NumericBound::FloatBits(v) => format!("@types.FloatBits({v}UL)"),
    }
}
fn mb_numeric(name: &str, value: Option<&NumericRestrictions>) -> String {
    let r = value
        .map(|v| {
            format!(
                "Some(@types.NumericRestrictions::{{ min: {}, max: {}, unit: {} }})",
                v.min
                    .map(|x| format!("Some({})", mb_bound(x)))
                    .unwrap_or_else(|| "None".into()),
                v.max
                    .map(|x| format!("Some({})", mb_bound(x)))
                    .unwrap_or_else(|| "None".into()),
                mb_opt_str(v.unit.as_deref())
            )
        })
        .unwrap_or_else(|| "None".into());
    format!("@model.{name}({r})")
}
fn mb_quantity(value: Option<&QuantityValue>) -> String {
    value
        .map(|v| {
            format!(
                "Some(@types.QuantityValue::{{ mantissa: {}L, scale: {}, unit: {} }})",
                v.mantissa,
                v.scale,
                moonbit_string_literal(&v.unit)
            )
        })
        .unwrap_or_else(|| "None".into())
}
fn mb_discriminator(value: &DiscriminatorRule) -> String {
    match value {
        DiscriminatorRule::Prefix { prefix } => {
            format!("@types.Prefix({})", moonbit_string_literal(prefix))
        }
        DiscriminatorRule::Suffix { suffix } => {
            format!("@types.Suffix({})", moonbit_string_literal(suffix))
        }
        DiscriminatorRule::Contains { substring } => {
            format!("@types.Contains({})", moonbit_string_literal(substring))
        }
        DiscriminatorRule::Regex { regex } => {
            format!("@types.Regex({})", moonbit_string_literal(regex))
        }
        DiscriminatorRule::FieldEquals(v) => format!(
            "@types.FieldEquals(@types.FieldDiscriminator::{{ field_name: {}, literal: {} }})",
            moonbit_string_literal(&v.field_name),
            mb_opt_str(v.literal.as_deref())
        ),
        DiscriminatorRule::FieldAbsent { field_name } => {
            format!("@types.FieldAbsent({})", moonbit_string_literal(field_name))
        }
    }
}

fn mb_path_direction(value: golem_common::schema::PathDirection) -> &'static str {
    use golem_common::schema::PathDirection;
    match value {
        PathDirection::Input => "INPUT",
        PathDirection::Output => "OUTPUT",
        PathDirection::InOut => "IN_OUT",
    }
}

fn mb_path_kind(value: golem_common::schema::PathKind) -> &'static str {
    use golem_common::schema::PathKind;
    match value {
        PathKind::File => "FILE",
        PathKind::Directory => "DIRECTORY",
        PathKind::Any => "ANY",
    }
}

fn guest_codec_source(source: String) -> String {
    let mut result = String::with_capacity(source.len());
    let mut code = String::new();
    let mut in_string = false;
    let mut escaped = false;

    for ch in source.chars() {
        if in_string {
            result.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            result.push_str(&replace_guest_codec_tokens(code));
            code = String::new();
            result.push(ch);
            in_string = true;
        } else {
            code.push(ch);
        }
    }
    result.push_str(&replace_guest_codec_tokens(code));
    result
}

fn replace_guest_codec_tokens(mut source: String) -> String {
    let replacements = [
        ("@runtime.SchemaValue", "@model.SchemaValue"),
        ("@runtime.SchemaMapEntry", "@model.SchemaMapEntry"),
        ("@runtime.BoolValue", "@model.SchemaValue::Bool"),
        ("@runtime.S8Value", "@model.SchemaValue::S8"),
        ("@runtime.S16Value", "@model.SchemaValue::S16"),
        ("@runtime.S32Value", "@model.SchemaValue::S32"),
        ("@runtime.S64Value", "@model.SchemaValue::S64"),
        ("@runtime.U8Value", "@model.SchemaValue::U8"),
        ("@runtime.U16Value", "@model.SchemaValue::U16"),
        ("@runtime.U32Value", "@model.SchemaValue::U32"),
        ("@runtime.U64Value", "@model.SchemaValue::U64"),
        ("@runtime.F32Value", "@model.SchemaValue::F32"),
        ("@runtime.F64Value", "@model.SchemaValue::F64"),
        ("@runtime.CharValue", "@model.SchemaValue::Char"),
        ("@runtime.StringValue", "@model.SchemaValue::String"),
        ("@runtime.RecordValue", "@model.SchemaValue::Record"),
        ("@runtime.VariantValue", "@model.SchemaValue::Variant"),
        ("@runtime.EnumValue", "@model.SchemaValue::Enum"),
        ("@runtime.FlagsValue", "@model.SchemaValue::Flags"),
        ("@runtime.TupleValue", "@model.SchemaValue::Tuple"),
        ("@runtime.ListValue", "@model.SchemaValue::List"),
        ("@runtime.FixedListValue", "@model.SchemaValue::FixedList"),
        ("@runtime.MapValue", "@model.SchemaValue::Map"),
        ("@runtime.OptionValue", "@model.SchemaValue::Option"),
        ("@runtime.PathValue", "@model.SchemaValue::Path"),
        ("@runtime.UrlValue", "@model.SchemaValue::Url"),
        ("@runtime.DatetimeValue", "@model.SchemaValue::Datetime"),
        ("@runtime.DurationValue", "@model.SchemaValue::Duration"),
        ("@runtime.UnionValue", "@model.SchemaValue::Union"),
        (
            "@runtime.ResultValue(@runtime.ResultOk",
            "@model.SchemaValue::ResultOk",
        ),
        (
            "@runtime.ResultValue(@runtime.ResultErr",
            "@model.SchemaValue::ResultErr",
        ),
        ("@runtime.ResultOk", "@model.SchemaValue::ResultOk"),
        ("@runtime.ResultErr", "@model.SchemaValue::ResultErr"),
        ("@runtime.as_", "guest_as_"),
        ("@runtime.variant_payload", "guest_variant_payload"),
        ("@runtime.required_payload", "guest_required_payload"),
        ("@runtime.BridgeError", "CodecError"),
    ];
    for (from, to) in replacements {
        source = source.replace(from, to);
    }
    source
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::{AgentTypeName, Snapshotting};
    use golem_common::schema::graph::SchemaTypeDef;
    use golem_common::schema::schema_type::{PathDirection, PathKind, PathSpec};
    use golem_common::schema::{AgentConstructorSchema, MetadataEnvelope, Role, SchemaGraph};

    #[test]
    fn schema_graph_literal_preserves_ids_refs_metadata_and_numeric_literals() {
        let metadata = MetadataEnvelope {
            doc: Some("root docs".into()),
            aliases: vec!["alias".into()],
            examples: vec!["42".into()],
            deprecated: Some("old".into()),
            role: Some(Role::Other("custom".into())),
        };
        let graph = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: "original.Type-ID".into(),
                name: Some("Display".into()),
                body: SchemaType::record(vec![golem_common::schema::NamedFieldType {
                    name: "next".into(),
                    body: SchemaType::Ref {
                        id: "original.Type-ID".into(),
                        metadata: metadata.clone(),
                    },
                    metadata: MetadataEnvelope::default(),
                }]),
            }],
            root: SchemaType::S64 {
                restrictions: Some(NumericRestrictions {
                    min: Some(NumericBound::Signed(-9)),
                    max: Some(NumericBound::Signed(12)),
                    unit: Some("ms".into()),
                }),
                metadata,
            },
        };
        let source = emit_schema_graph_literal(&graph);
        assert!(source.contains("original.Type-ID"));
        assert!(source.contains("@model.Ref(\"original.Type-ID\")"));
        assert!(source.contains("@types.Signed(-9L)"));
        assert!(source.contains("@types.Signed(12L)"));
        assert!(source.contains("root docs"));
        assert!(source.contains("Some(@types.Other(\"custom\"))"));
    }

    #[test]
    fn schema_graph_literal_preserves_path_restriction_strings() {
        let graph = SchemaGraph {
            defs: vec![],
            root: SchemaType::path(PathSpec {
                direction: PathDirection::Input,
                kind: PathKind::File,
                allowed_mime_types: Some(vec!["application/Output".into()]),
                allowed_extensions: Some(vec![".AnyFile".into()]),
            }),
        };

        let source = emit_schema_graph_literal(&graph);

        assert!(source.contains("\"application/Output\""), "{source}");
        assert!(source.contains("\".AnyFile\""), "{source}");
    }

    use tempfile::TempDir;
    use test_r::test;

    #[test]
    fn guest_extra_reserved_name_is_honored_by_multimodal_generation() {
        let variant = SchemaType::variant(vec![VariantCaseType {
            name: "text".to_string(),
            payload: Some(SchemaType::string()),
            metadata: MetadataEnvelope::default(),
        }]);
        let mut multimodal = SchemaType::list(variant);
        multimodal.metadata_mut().role = Some(Role::Multimodal);
        let agent_type = AgentTypeSchema {
            type_name: AgentTypeName("VisionSession".to_string()),
            description: String::new(),
            source_language: "moonbit".to_string(),
            schema: SchemaGraph {
                defs: vec![],
                root: SchemaType::record(vec![]),
            },
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
                    "input", multimodal,
                )]),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        };
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        let mut generator = MoonBitBridgeGenerator::new_guest_with_extra_reserved_names(
            agent_type,
            target,
            true,
            ["Multimodal0".to_string()],
        )
        .unwrap();

        generator.generate().unwrap();

        let client = std::fs::read_to_string(target.join("client/client.mbt")).unwrap();
        assert!(!client.contains("pub(all) enum Multimodal0 {"));
        assert!(client.contains("pub(all) enum Multimodal1 {"));
    }
}
