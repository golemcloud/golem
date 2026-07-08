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

//! Scala bridge SDK generator.
//!
//! Unlike the Rust and TypeScript generators, which depend on an external
//! runtime library (`golem-client` / `@golemcloud/golem-ts-bridge`), the Scala
//! generator emits a fully self-contained sbt project. The static runtime code
//! lives under the `golem.bridge.runtime` package (embedded from
//! `src/bridge_gen/scala/runtime`) and the per-agent generated client lives
//! under a per-agent package `golem.bridge.client.<segment>` (the segment is
//! derived from the agent type name; see
//! [`ScalaBridgeGenerator::client_package_segment`]). Namespacing the client
//! per agent keeps the generated `Codecs` object and top-level generated types
//! from colliding when two generated SDKs end up on the same classpath; the
//! static runtime is identical across SDKs and stays under the fixed
//! `golem.bridge.runtime` package, making it straightforward to extract into a
//! published runtime library later.
//!
//! The generated project depends only on the JDK (`java.net.http`) and a
//! hand-rolled JSON model, has no third-party dependencies, and cross-compiles
//! against Scala 2.13 and Scala 3.

#[allow(clippy::module_inception)]
pub mod scala;
pub mod scala_writer;
pub mod type_name;

pub use type_name::{RemappedType, ScalaTypeName};

use crate::bridge_gen::scala::scala::{
    escape_scala_ident, is_scala_keyword, to_scala_term_ident, to_scala_type_ident, unique_idents,
    unique_idents_with_reserved,
};
use crate::bridge_gen::scala::scala_writer::ScalaWriter;
use crate::bridge_gen::type_naming::{TypeNaming, user_supplied_fields};
use crate::bridge_gen::{BridgeGenerator, BridgeMode, bridge_client_directory_name};
use crate::fs;
use crate::versions::scala_dep;
use anyhow::{Context, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
use golem_common::schema::multimodal::multimodal_variant_cases;
use golem_common::schema::schema_type::{SchemaType, VariantCaseType};
use golem_common::schema::unstructured::{
    unstructured_binary_restrictions, unstructured_text_restrictions,
};
use golem_common::schema::{
    AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField, OutputSchema,
};
use heck::{ToLowerCamelCase, ToSnakeCase, ToUpperCamelCase};
use include_dir::{Dir, include_dir};
use indoc::formatdoc;

/// Static runtime sources emitted verbatim into every generated project under
/// `src/main/scala`. Each file already carries its `golem/bridge/runtime/...`
/// relative path.
static RUNTIME_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/src/bridge_gen/scala/runtime");

const SCALA_SOURCE_ROOT: &str = "src/main/scala";

/// Fully-qualified package prefix of the static runtime types.
const RUNTIME_PKG: &str = "_root_.golem.bridge.runtime";

/// Root (un-rooted) package of the generated client types. The per-agent
/// package segment (see [`ScalaBridgeGenerator::client_package_segment`]) is
/// appended so that two generated SDKs placed on the same classpath cannot
/// collide on a shared `golem.bridge.client` namespace (the generated `Codecs`
/// object and named types differ per agent).
const CLIENT_PKG_BASE: &str = "golem.bridge.client";

/// Fully-qualified runtime `SchemaValue` companion (its case classes are
/// referenced as `{SV}.BoolValue`, …). Always fully qualified so a generated
/// member name can never shadow it.
const SV: &str = "_root_.golem.bridge.runtime.SchemaValue";

/// Fully-qualified runtime `SchemaValueCodec` object, holding the typed
/// accessors and the UUID builtin codec the generated codecs lean on.
const CODEC: &str = "_root_.golem.bridge.runtime.SchemaValueCodec";

/// Fully-qualified runtime `SchemaResult` companion.
const SCHEMA_RESULT: &str = "_root_.golem.bridge.runtime.SchemaResult";

/// Fully-qualified runtime `SchemaMapEntry` case class.
const SCHEMA_MAP_ENTRY: &str = "_root_.golem.bridge.runtime.SchemaMapEntry";

/// Fully-qualified runtime `BridgeException`, thrown by generated codecs on a
/// wire-shape mismatch.
const BRIDGE_EXCEPTION: &str = "_root_.golem.bridge.runtime.BridgeException";

/// Fully-qualified runtime `SchemaValue` type, used in codec signatures.
const SCHEMA_VALUE_TYPE: &str = "_root_.golem.bridge.runtime.SchemaValue";

/// Generated object holding the per-named-type encode/decode codecs. Reserved
/// in [`RESERVED_RUNTIME_TYPE_NAMES`] so no generated type can take its name.
/// Always referenced fully qualified as `{client_pkg}.{CODECS_OBJECT}` at call
/// sites so a local term named `Codecs` can never shadow it.
const CODECS_OBJECT: &str = "Codecs";

/// Fully-qualified runtime `AgentConfigEntry` case class, used to build the
/// per-agent local config overrides passed to `Bridge.createAgent`.
const AGENT_CONFIG_ENTRY: &str = "_root_.golem.bridge.runtime.AgentConfigEntry";

/// Immutable `List` constructor used throughout the generated codecs.
const LIST: &str = "_root_.scala.collection.immutable.List";

/// The empty immutable `List` literal, passed as the config argument by the
/// plain constructors that do not take config overrides.
const LIST_EMPTY: &str = "_root_.scala.collection.immutable.List()";

/// Fully-qualified runtime REST transport object.
const BRIDGE: &str = "_root_.golem.bridge.runtime.Bridge";

/// Fully-qualified runtime `Configuration` object/type.
const CONFIGURATION: &str = "_root_.golem.bridge.runtime.Configuration";

/// Fully-qualified runtime `ResolvedAgent` case class.
const RESOLVED_AGENT: &str = "_root_.golem.bridge.runtime.ResolvedAgent";

/// Fully-qualified runtime `AgentId` case class.
const AGENT_ID: &str = "_root_.golem.bridge.runtime.AgentId";

/// Fully-qualified runtime `GolemServer` type.
const GOLEM_SERVER: &str = "_root_.golem.bridge.runtime.GolemServer";

/// Fully-qualified runtime `Datetime` type (scheduling instant).
const DATETIME: &str = "_root_.golem.bridge.runtime.Datetime";

/// Fully-qualified runtime `Uuid` type (phantom ids).
const UUID: &str = "_root_.golem.bridge.runtime.Uuid";

/// Scala `Future`, `ExecutionContext`, `String`, `Unit`.
const FUTURE: &str = "_root_.scala.concurrent.Future";
const EXECUTION_CONTEXT: &str = "_root_.scala.concurrent.ExecutionContext";
const STRING: &str = "_root_.scala.Predef.String";
const UNIT: &str = "_root_.scala.Unit";

/// The two `AgentInvocationMode` wire strings (server OpenAPI enum).
const MODE_AWAIT: &str = "await";
const MODE_SCHEDULE: &str = "schedule";

/// Scala's tuple types (`TupleN`) are only defined up to arity 22.
const MAX_TUPLE_ARITY: usize = 22;

/// Internal identifiers emitted alongside user-supplied parameters in the
/// generated client (constructor/method helper defs, locals, inherited trait
/// members referenced unqualified, and the synthetic `when`/`phantom`
/// parameters). User parameter names are disambiguated away from these so a
/// parameter can never shadow or collide with generated code. The set is the
/// union of all constructor and method contexts so the names chosen for an
/// input are identical wherever that input's parameters are emitted.
const RESERVED_PARAM_NAMES: &[&str] = &[
    "resolved",
    "methodParameters",
    "constructorParameters",
    "ec",
    "__result",
    "__value",
    "__response",
    "when",
    "configuration",
    "parameters",
    "phantomId",
    "phantom",
    "agentId",
    "agentTypeName",
    "bindRemote",
    "get",
    "getPhantom",
    "newPhantom",
    "getWithConfig",
    "getPhantomWithConfig",
    "newPhantomWithConfig",
    "agentConfig",
    "configValue",
];

/// Member names a generated named type cannot use for a case-class field or a
/// companion case object / case class, because they would clash with (or fail
/// to override) a synthesized case-class member or an inherited
/// `Product`/`Object`/`Any` member. The set is the union of the names rejected
/// by the Scala 2.13 and Scala 3 compilers (verified against both for arbitrary
/// field types): the no-arg `Object`/`Any` members `toString`/`hashCode`/`##`/
/// `getClass`/`notify`/`notifyAll`/`wait`/`clone`/`finalize` and the no-arg
/// `Product` members `productArity`/`productPrefix`/`productIterator`/
/// `productElementNames`. (Arg-taking members such as `equals`, `canEqual`,
/// `productElement`, `==`, `eq`, and `synchronized` are not reserved: a no-arg
/// field/accessor does not override them. The polymorphic universal members
/// `asInstanceOf`/`isInstanceOf` are likewise not overridden by a monomorphic
/// member and need no reservation.) A user field/case name colliding with one
/// of these is
/// disambiguated; encoding uses schema field/case order or schema tag strings,
/// never the generated Scala member identifier, so the rename never affects the
/// wire format.
const RESERVED_MEMBER_NAMES: &[&str] = &[
    "toString",
    "hashCode",
    "##",
    "getClass",
    "notify",
    "notifyAll",
    "wait",
    "clone",
    "finalize",
    "productArity",
    "productPrefix",
    "productIterator",
    "productElementNames",
];

/// Type names that already exist in the wildcard-imported
/// `golem.bridge.runtime` package (or are reserved by the protocol layer). A
/// generated type must not take any of these names, otherwise it would shadow
/// the runtime definition. They are reserved in the [`TypeNaming`] walker so
/// colliding generated types are disambiguated by location instead.
const RESERVED_RUNTIME_TYPE_NAMES: &[&str] = &[
    "SchemaValue",
    "SchemaMapEntry",
    "SchemaResult",
    "SchemaValueCodec",
    "Json",
    "BridgeException",
    "GolemServer",
    "Configuration",
    "AgentId",
    "Uuid",
    "Datetime",
    "UByte",
    "UShort",
    "UInt",
    "ULong",
    "UnstructuredText",
    "UnstructuredBinary",
    "Bridge",
    "BridgeProtocol",
    "AgentConfigEntry",
    "CreateAgentRequest",
    "CreateAgentResponse",
    "AgentInvocationRequest",
    "AgentInvocationResult",
    "ResolvedAgent",
];

/// The `(case_name, payload_schema)` modality pairs of one multimodal set.
type MultimodalModalities = Vec<(String, SchemaType)>;

/// A discovered multimodal modality set paired with its generated
/// `Multimodal<N>` sealed-trait name.
type NamedMultimodal = (MultimodalModalities, String);

pub struct ScalaBridgeGenerator {
    target_path: Utf8PathBuf,
    agent_type: AgentTypeSchema,
    #[allow(dead_code)]
    testing: bool,
    same_language: bool,
    type_naming: TypeNaming<ScalaTypeName>,
    /// Distinct multimodal modality sets discovered up front (constructor input,
    /// then each method's input and output, in declaration order), each mapped
    /// to its generated `Multimodal<N>` sealed-trait name. Precomputed so the
    /// generated client can reference `Multimodal<N>` while emission stays
    /// `&self`, and so the names can be reserved in [`TypeNaming`] before user
    /// types are assigned (a user type cannot then take a `Multimodal<N>` name).
    /// The wire format is independent of these names (it is the structural
    /// `list<variant<…>>`), so the discovery order only affects the generated
    /// Scala API surface, never the bytes on the wire.
    multimodals: Vec<NamedMultimodal>,
}

impl BridgeGenerator for ScalaBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        ScalaBridgeGenerator::new(agent_type, target_path, testing)
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        if !self.target_path.exists() {
            fs::create_dir_all(&self.target_path)?;
        }
        self.write_build_files()?;
        self.write_runtime()?;
        self.write_client()?;
        Ok(())
    }
}

impl ScalaBridgeGenerator {
    pub fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        let same_language = agent_type.source_language.eq_ignore_ascii_case("scala");

        // Discover the multimodal modality sets first so their generated
        // `Multimodal<N>` names can be reserved in the walker below.
        let multimodals = collect_multimodals(&agent_type)?;

        let reserved = RESERVED_RUNTIME_TYPE_NAMES
            .iter()
            .map(|name| ScalaTypeName::Derived((*name).to_string()))
            .chain(std::iter::once(ScalaTypeName::Derived(client_object_name(
                &agent_type,
            ))))
            .chain(std::iter::once(ScalaTypeName::Derived(remote_trait_name(
                &agent_type,
            ))))
            // The generated codecs object lives in the client package alongside
            // the generated types, so its name must not be taken by one of them.
            .chain(std::iter::once(ScalaTypeName::Derived(
                CODECS_OBJECT.to_string(),
            )))
            // The generated multimodal sealed traits live in the client package
            // too, so a user type must not be assigned one of their names.
            .chain(
                multimodals
                    .iter()
                    .map(|(_, name)| ScalaTypeName::Derived(name.clone())),
            );
        let type_naming =
            TypeNaming::new_with_reserved_names(&agent_type, same_language, reserved)?;

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            testing,
            same_language,
            type_naming,
            multimodals,
        })
    }

    fn library_name(&self) -> String {
        bridge_client_directory_name(&self.agent_type.type_name, BridgeMode::External)
    }

    /// The per-agent client package segment appended to `golem.bridge.client`,
    /// derived from the agent type name (e.g. `CounterAgent` -> `counter_agent`,
    /// `foo-agent` -> `foo_agent`).
    ///
    /// The result is always a plain (non-backticked) lowercase Scala package
    /// identifier: a backticked segment would be poor generated-DX and awkward
    /// as a directory name. Any character outside `[a-z0-9_]` is replaced with
    /// `_`, a leading non-letter is prefixed (`123-agent` -> `agent_123_agent`),
    /// and a segment that is a Scala keyword is suffixed (`type` -> `type_`).
    fn client_package_segment(&self) -> String {
        let mut seg: String = self
            .agent_type
            .type_name
            .as_str()
            .to_snake_case()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        if seg.is_empty() {
            seg = "agent".to_string();
        }
        if !seg.starts_with(|c: char| c.is_ascii_alphabetic()) {
            seg = format!("agent_{seg}");
        }
        if is_scala_keyword(&seg) {
            seg = format!("{seg}_");
        }
        seg
    }

    /// The dotted client package, e.g. `golem.bridge.client.counter_agent`. Used
    /// in the generated `package …` declaration.
    fn client_package(&self) -> String {
        format!("{CLIENT_PKG_BASE}.{}", self.client_package_segment())
    }

    /// The fully-qualified (`_root_`-rooted) client package prefix, used at every
    /// generated reference to a client-package type or the `Codecs` object so a
    /// nested/local name can never shadow it.
    fn client_pkg(&self) -> String {
        format!("_root_.{}", self.client_package())
    }

    /// Name of the generated client object, e.g. `foo-agent` -> `FooAgentClient`.
    fn client_object_name(&self) -> String {
        client_object_name(&self.agent_type)
    }

    /// Name of the generated remote trait, e.g. `foo-agent` -> `FooAgentRemote`.
    fn remote_trait_name(&self) -> String {
        remote_trait_name(&self.agent_type)
    }

    fn write_build_files(&self) -> anyhow::Result<()> {
        let build_sbt = formatdoc! {r#"
            ThisBuild / organization := "golem.bridge"
            ThisBuild / version      := "0.0.1"

            lazy val root = (project in file("."))
              .settings(
                name               := "{name}",
                scalaVersion       := "{scala3}",
                crossScalaVersions := Seq("{scala2}", "{scala3}"),
                libraryDependencies += "dev.zio" %% "zio-blocks-schema" % "{zio_blocks}"
              )
            "#,
            name = self.library_name(),
            scala2 = scala_dep::SCALA_2_VERSION,
            scala3 = scala_dep::SCALA_VERSION,
            zio_blocks = scala_dep::ZIO_BLOCKS_VERSION,
        };
        fs::write_str(self.target_path.join("build.sbt"), build_sbt)?;

        let build_properties = format!("sbt.version={}\n", scala_dep::SBT_VERSION);
        fs::write_str(
            self.target_path.join("project").join("build.properties"),
            build_properties,
        )?;

        Ok(())
    }

    fn write_runtime(&self) -> anyhow::Result<()> {
        let source_root = self.target_path.join(SCALA_SOURCE_ROOT);
        write_dir(&RUNTIME_DIR, &source_root)
    }

    fn write_client(&self) -> anyhow::Result<()> {
        let content = self.generate_client_source()?;

        let mut client_path = self
            .target_path
            .join(SCALA_SOURCE_ROOT)
            .join("golem")
            .join("bridge")
            .join("client")
            .join(self.client_package_segment());
        client_path.push(format!("{}.scala", self.client_object_name()));
        fs::write_str(client_path, content)?;
        Ok(())
    }

    /// Renders the full generated client source file: package declaration,
    /// imports, the generated type definitions, and the client object.
    fn generate_client_source(&self) -> anyhow::Result<String> {
        let mut writer = ScalaWriter::new();
        writer.line(format!("package {}", self.client_package()));
        writer.blank();
        writer.line("import golem.bridge.runtime._");
        writer.blank();
        writer.line("// Generated by golem-cli. Do not edit.");
        writer.blank();

        self.write_type_definitions(&mut writer)?;
        self.write_multimodal_definitions(&mut writer)?;

        // Encode/decode codecs for every generated named composite type. The
        // structural (non-named) encode/decode is emitted inline at the use
        // sites by `encode_expr` / `decode_expr`; only the named composites get
        // a reusable `encode<Name>` / `decode<Name>` here.
        self.write_codecs(&mut writer)?;

        // The generated client object: configuration helpers, per-method remote
        // wrapper classes, the Remote trait, and the mode-aware constructors.
        self.write_client_object(&mut writer)?;

        Ok(writer.finish())
    }

    // --- Client object ------------------------------------------------------

    /// Emits the `<Agent>Client` object: configuration helpers, the per-method
    /// remote wrapper classes, the `<Agent>Remote` trait + `bindRemote`, and the
    /// mode-aware constructors returning `Future[<Agent>Remote]`.
    fn write_client_object(&self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        let object_name = self.client_object_name();
        let remote_name = self.remote_trait_name();
        let agent_type_name = self.agent_type.type_name.as_str();

        writer.line(format!("object {object_name} {{"));
        writer.indent();

        writer.line(format!(
            "val agentTypeName: {STRING} = {}",
            scala_string_literal(agent_type_name)
        ));
        writer.blank();

        // Configuration convenience, delegating to the shared runtime cell.
        writer.line(format!(
            "def configure(server: {GOLEM_SERVER}, appName: {STRING}, envName: {STRING}, executionContext: {EXECUTION_CONTEXT} = {EXECUTION_CONTEXT}.global): {UNIT} ="
        ));
        writer.indent();
        writer.line(format!(
            "{CONFIGURATION}.configure(server, appName, envName, executionContext)"
        ));
        writer.dedent();
        writer.blank();
        writer.line(format!(
            "def getConfiguration: {CONFIGURATION} = {CONFIGURATION}.get"
        ));
        writer.blank();

        // Per-method remote wrapper classes. Class names are always UpperCamel
        // (independent of the method's source casing) and escaped once, so an
        // escaped/keyword method name can never produce an invalid class name.
        let methods = self.agent_type.methods.clone();
        let method_class_names = unique_idents(
            methods
                .iter()
                .map(|m| remote_method_class_name(&m.name))
                .collect(),
        );
        // Method val names become members of the generated `Remote` trait, so
        // they must not collide with the trait's own `agentId` / `agentTypeName`
        // members nor with the final/universal members inherited from
        // `Any`/`AnyRef` (a `val` of the same name would fail to override them).
        let method_val_names = unique_idents_with_reserved(
            methods
                .iter()
                .map(|m| to_scala_term_ident(&m.name, self.same_language))
                .collect(),
            &[
                "agentId",
                "agentTypeName",
                "toString",
                "hashCode",
                "equals",
                "getClass",
                "isInstanceOf",
                "asInstanceOf",
                "notify",
                "notifyAll",
                "wait",
                "clone",
                "finalize",
                "synchronized",
                "##",
                "==",
                "!=",
                "eq",
                "ne",
            ],
        );
        for (method, class_name) in methods.iter().zip(&method_class_names) {
            self.write_remote_method_class(writer, class_name, method)?;
            writer.blank();
        }

        // The Remote trait and its private factory.
        self.write_remote_trait(writer, &remote_name, &method_class_names, &method_val_names);
        writer.blank();

        // Mode-aware constructors.
        self.write_constructors(writer, &remote_name)?;

        writer.dedent();
        writer.line("}");
        Ok(())
    }

    /// Emits a single `<Method>RemoteMethod` wrapper class with `apply`
    /// (await), `trigger` (fire-and-forget) and `scheduleAt` (scheduled).
    fn write_remote_method_class(
        &self,
        writer: &mut ScalaWriter,
        class_name: &str,
        method: &AgentMethodSchema,
    ) -> anyhow::Result<()> {
        let object_name = self.client_object_name();
        let method_name_lit = scala_string_literal(&method.name);
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_decls = param_defs
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let param_names = param_defs
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        writer.line(format!(
            "final class {class_name} private[{object_name}] (resolved: {RESOLVED_AGENT}) {{"
        ));
        writer.indent();

        // Parameter packing into a positional record SchemaValue.
        writer.line(format!(
            "private def methodParameters({param_decls}): {SCHEMA_VALUE_TYPE} = {{"
        ));
        writer.indent();
        self.write_param_record(writer, &method.input_schema)?;
        writer.dedent();
        writer.line("}");
        writer.blank();

        let invoke_args = param_names.join(", ");

        // apply (await) returns the decoded result, or Unit.
        let (ret_ty, decode_block) = self.output_return(&method.output_schema)?;
        writer.line(format!("def apply({param_decls}): {FUTURE}[{ret_ty}] = {{"));
        writer.indent();
        writer.line(format!(
            "implicit val ec: {EXECUTION_CONTEXT} = resolved.configuration.executionContext"
        ));
        writer.line(format!(
            "{BRIDGE}.invokeAgent(resolved, {method_name_lit}, methodParameters({invoke_args}), {}, _root_.scala.None).map {{ __result =>",
            scala_string_literal(MODE_AWAIT)
        ));
        writer.indent();
        writer.line(decode_block);
        writer.dedent();
        writer.line("}");
        writer.dedent();
        writer.line("}");
        writer.blank();

        // trigger (schedule, no time) — fire-and-forget.
        writer.line(format!("def trigger({param_decls}): {FUTURE}[{UNIT}] = {{"));
        writer.indent();
        writer.line(format!(
            "implicit val ec: {EXECUTION_CONTEXT} = resolved.configuration.executionContext"
        ));
        writer.line(format!(
            "{BRIDGE}.invokeAgent(resolved, {method_name_lit}, methodParameters({invoke_args}), {}, _root_.scala.None).map(_ => ())",
            scala_string_literal(MODE_SCHEDULE)
        ));
        writer.dedent();
        writer.line("}");
        writer.blank();

        // scheduleAt (schedule, at a time).
        let schedule_decls = if param_decls.is_empty() {
            format!("when: {DATETIME}")
        } else {
            format!("{param_decls}, when: {DATETIME}")
        };
        writer.line(format!(
            "def scheduleAt({schedule_decls}): {FUTURE}[{UNIT}] = {{"
        ));
        writer.indent();
        writer.line(format!(
            "implicit val ec: {EXECUTION_CONTEXT} = resolved.configuration.executionContext"
        ));
        writer.line(format!(
            "{BRIDGE}.invokeAgent(resolved, {method_name_lit}, methodParameters({invoke_args}), {}, _root_.scala.Some(when.toIsoString)).map(_ => ())",
            scala_string_literal(MODE_SCHEDULE)
        ));
        writer.dedent();
        writer.line("}");

        writer.dedent();
        writer.line("}");
        Ok(())
    }

    /// Emits the `<Agent>Remote` trait and the private `bindRemote` factory.
    fn write_remote_trait(
        &self,
        writer: &mut ScalaWriter,
        remote_name: &str,
        method_class_names: &[String],
        method_val_names: &[String],
    ) {
        writer.line(format!("trait {remote_name} {{"));
        writer.indent();
        writer.line(format!("def agentId: {AGENT_ID}"));
        writer.line(format!("def agentTypeName: {STRING}"));
        for (val_name, class_name) in method_val_names.iter().zip(method_class_names) {
            writer.line(format!("val {val_name}: {class_name}"));
        }
        writer.dedent();
        writer.line("}");
        writer.blank();

        writer.line(format!(
            "private def bindRemote(resolved: {RESOLVED_AGENT}): {remote_name} = new {remote_name} {{"
        ));
        writer.indent();
        writer.line(format!("def agentId: {AGENT_ID} = resolved.agentId"));
        writer.line(format!(
            "def agentTypeName: {STRING} = resolved.agentTypeName"
        ));
        for (val_name, class_name) in method_val_names.iter().zip(method_class_names) {
            writer.line(format!(
                "val {val_name}: {class_name} = new {class_name}(resolved)"
            ));
        }
        writer.dedent();
        writer.line("}");
    }

    /// Emits the mode-aware constructors. Every agent gets `getPhantom` and
    /// `newPhantom`; durable agents additionally get `get`. When the agent
    /// declares local config overrides, the matching `getWithConfig` /
    /// `getPhantomWithConfig` / `newPhantomWithConfig` variants are emitted too
    /// (mirroring the Scala SDK's RPC clients). Each constructor returns a
    /// `Future[<Agent>Remote]`.
    fn write_constructors(
        &self,
        writer: &mut ScalaWriter,
        remote_name: &str,
    ) -> anyhow::Result<()> {
        let input = self.agent_type.constructor.input_schema.clone();
        let param_defs = self.input_param_defs(&input)?;
        let param_decls = param_defs
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let param_names = param_defs
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let invoke_args = param_names.join(", ");

        // Local config overrides this agent declares (if any). The matching
        // `…WithConfig` constructors are only emitted when this is non-empty.
        let local_configs = self.local_configs();
        let config_param_defs = self.config_param_defs(&param_names, &local_configs)?;
        let config_decls = config_param_defs
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let config_param_names = config_param_defs
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        // Shared parameter-packing helper.
        writer.line(format!(
            "private def constructorParameters({param_decls}): {SCHEMA_VALUE_TYPE} = {{"
        ));
        writer.indent();
        self.write_param_record(writer, &input)?;
        writer.dedent();
        writer.line("}");
        writer.blank();

        // `get` (durable only): resolve by constructor parameters, no phantom.
        if self.agent_type.mode == AgentMode::Durable {
            writer.line(format!(
                "def get({param_decls}): {FUTURE}[{remote_name}] = {{"
            ));
            writer.indent();
            self.write_create_agent_body(writer, &invoke_args, "_root_.scala.None", LIST_EMPTY);
            writer.dedent();
            writer.line("}");
            writer.blank();
        }

        // `getPhantom`: resolve with an explicit phantom UUID.
        let phantom_decls = if param_decls.is_empty() {
            format!("phantom: {UUID}")
        } else {
            format!("{param_decls}, phantom: {UUID}")
        };
        let phantom_some = format!("_root_.scala.Some({UUID}.toStandardString(phantom))");
        writer.line(format!(
            "def getPhantom({phantom_decls}): {FUTURE}[{remote_name}] = {{"
        ));
        writer.indent();
        self.write_create_agent_body(writer, &invoke_args, &phantom_some, LIST_EMPTY);
        writer.dedent();
        writer.line("}");
        writer.blank();

        // `newPhantom`: like `getPhantom` but with a fresh random phantom id.
        let new_phantom_args = if invoke_args.is_empty() {
            format!("{UUID}.random()")
        } else {
            format!("{invoke_args}, {UUID}.random()")
        };
        writer.line(format!(
            "def newPhantom({param_decls}): {FUTURE}[{remote_name}] = getPhantom({new_phantom_args})"
        ));

        // Config-override constructors, emitted only when the agent declares
        // local config. Each builds the `List[AgentConfigEntry]` inline from the
        // non-`None` config parameters and creates the agent with it.
        if !local_configs.is_empty() {
            let with_config_decls = |extra: &str| {
                [param_decls.as_str(), extra, config_decls.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            // `getWithConfig` (durable only).
            if self.agent_type.mode == AgentMode::Durable {
                writer.blank();
                writer.line(format!(
                    "def getWithConfig({}): {FUTURE}[{remote_name}] = {{",
                    with_config_decls("")
                ));
                writer.indent();
                self.write_config_list(writer, &config_param_names, &local_configs)?;
                self.write_create_agent_body(
                    writer,
                    &invoke_args,
                    "_root_.scala.None",
                    "agentConfig",
                );
                writer.dedent();
                writer.line("}");
            }

            // `getPhantomWithConfig`.
            writer.blank();
            writer.line(format!(
                "def getPhantomWithConfig({}): {FUTURE}[{remote_name}] = {{",
                with_config_decls(&format!("phantom: {UUID}"))
            ));
            writer.indent();
            self.write_config_list(writer, &config_param_names, &local_configs)?;
            self.write_create_agent_body(writer, &invoke_args, &phantom_some, "agentConfig");
            writer.dedent();
            writer.line("}");

            // `newPhantomWithConfig`: like `getPhantomWithConfig` but with a
            // fresh random phantom id.
            writer.blank();
            writer.line(format!(
                "def newPhantomWithConfig({}): {FUTURE}[{remote_name}] = {{",
                with_config_decls("")
            ));
            writer.indent();
            self.write_config_list(writer, &config_param_names, &local_configs)?;
            self.write_create_agent_body(
                writer,
                &invoke_args,
                &format!("_root_.scala.Some({UUID}.toStandardString({UUID}.random()))"),
                "agentConfig",
            );
            writer.dedent();
            writer.line("}");
        }

        Ok(())
    }

    /// The agent's local (caller-overridable) config declarations, in order.
    fn local_configs(&self) -> Vec<&AgentConfigDeclarationSchema> {
        self.agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .collect()
    }

    /// The `(name, type)` parameter declarations for the local config overrides,
    /// each an `Option[T] = None`. Names are camelCase of the config path,
    /// disambiguated away from each other, the constructor parameters, and the
    /// reserved internal identifiers. Returns an empty list when there are no
    /// local config declarations.
    fn config_param_defs(
        &self,
        constructor_param_names: &[String],
        local_configs: &[&AgentConfigDeclarationSchema],
    ) -> anyhow::Result<Vec<(String, String)>> {
        if local_configs.is_empty() {
            return Ok(Vec::new());
        }
        let base_names: Vec<String> = local_configs
            .iter()
            .map(|config| {
                let joined = config.path.join("-").to_lower_camel_case();
                if joined.is_empty() {
                    escape_scala_ident("config")
                } else {
                    escape_scala_ident(&joined)
                }
            })
            .collect();
        let mut reserved: Vec<&str> = RESERVED_PARAM_NAMES.to_vec();
        reserved.extend(constructor_param_names.iter().map(|s| s.as_str()));
        let names = unique_idents_with_reserved(base_names, &reserved);

        let mut defs = Vec::new();
        for (idx, config) in local_configs.iter().enumerate() {
            let ty = self.type_reference(&config.value_type)?;
            defs.push((
                names[idx].clone(),
                format!("_root_.scala.Option[{ty}] = _root_.scala.None"),
            ));
        }
        Ok(defs)
    }

    /// Emits `val agentConfig = List(<entry>, …).flatten`, where each `<entry>`
    /// is the matching config parameter mapped (when present) to an
    /// `AgentConfigEntry(path, encodedValue)`.
    fn write_config_list(
        &self,
        writer: &mut ScalaWriter,
        config_param_names: &[String],
        local_configs: &[&AgentConfigDeclarationSchema],
    ) -> anyhow::Result<()> {
        writer.line(format!("val agentConfig = {LIST}("));
        writer.indent();
        for (idx, config) in local_configs.iter().enumerate() {
            let name = &config_param_names[idx];
            let path_lit = config
                .path
                .iter()
                .map(|segment| scala_string_literal(segment))
                .collect::<Vec<_>>()
                .join(", ");
            let enc = self.encode_expr("value", &config.value_type, 0)?;
            let comma = if idx + 1 < local_configs.len() {
                ","
            } else {
                ""
            };
            writer.line(format!("{name}.map {{ value =>"));
            writer.indent();
            writer.line(format!("val configValue = {enc}"));
            writer.line(format!(
                "{AGENT_CONFIG_ENTRY}({LIST}({path_lit}), configValue)"
            ));
            writer.dedent();
            writer.line(format!("}}{comma}"));
        }
        writer.dedent();
        writer.line(").flatten");
        Ok(())
    }

    /// Emits the shared body of a constructor: build the parameter record, call
    /// `Bridge.createAgent`, and bind the resolved agent into a remote. The
    /// `phantom_expr` is the `Option[String]` phantom id expression and
    /// `config_expr` is the `List[AgentConfigEntry]` of local config overrides
    /// (`List()` for the plain constructors that do not take config overrides).
    fn write_create_agent_body(
        &self,
        writer: &mut ScalaWriter,
        invoke_args: &str,
        phantom_expr: &str,
        config_expr: &str,
    ) {
        writer.line(format!("val configuration = {CONFIGURATION}.get"));
        writer.line(format!(
            "implicit val ec: {EXECUTION_CONTEXT} = configuration.executionContext"
        ));
        writer.line(format!(
            "val parameters = constructorParameters({invoke_args})"
        ));
        writer.line(format!("val phantomId = {phantom_expr}"));
        writer.line(format!(
            "{BRIDGE}.createAgent(configuration, agentTypeName, parameters, phantomId, {config_expr}).map {{ __response =>"
        ));
        writer.indent();
        writer.line(format!(
            "bindRemote({RESOLVED_AGENT}(configuration, agentTypeName, parameters, phantomId, __response.agentId))"
        ));
        writer.dedent();
        writer.line("}");
    }

    /// Emits the body of a parameter-packing helper: encodes each user-supplied
    /// field positionally and wraps them in a record `SchemaValue`. A multimodal
    /// input is still packed as a single-field record whose only field is the
    /// encoded `list<variant<…>>` modality list (mirroring the Rust/TS bridges).
    fn write_param_record(
        &self,
        writer: &mut ScalaWriter,
        input: &InputSchema,
    ) -> anyhow::Result<()> {
        if let Some((name, param)) = self.input_multimodal(input)? {
            writer.line(format!(
                "val f0 = {}.{CODECS_OBJECT}.encode{name}List({param})",
                self.client_pkg()
            ));
            writer.line(format!("{SV}.RecordValue({LIST}(f0))"));
            return Ok(());
        }
        let fields = user_supplied_fields(input);
        let names = self.input_param_field_idents(&fields);
        let mut elems = Vec::new();
        for (idx, field) in fields.iter().enumerate() {
            let enc = self.encode_expr(&names[idx], &field.schema, 0)?;
            writer.line(format!("val f{idx} = {enc}"));
            elems.push(format!("f{idx}"));
        }
        writer.line(format!("{SV}.RecordValue({LIST}({}))", elems.join(", ")));
        Ok(())
    }

    /// The `(name, type)` parameter declarations for a constructor or method's
    /// user-supplied input fields, in declaration order. A multimodal input is
    /// surfaced as a single `List[Multimodal<N>]` parameter named after the
    /// single user field.
    fn input_param_defs(&self, input: &InputSchema) -> anyhow::Result<Vec<(String, String)>> {
        if let Some((name, param)) = self.input_multimodal(input)? {
            return Ok(vec![(param, self.multimodal_list_type(&name))]);
        }
        let fields = user_supplied_fields(input);
        let names = self.input_param_field_idents(&fields);
        let mut defs = Vec::new();
        for (idx, field) in fields.iter().enumerate() {
            defs.push((names[idx].clone(), self.type_reference(&field.schema)?));
        }
        Ok(defs)
    }

    // --- Multimodal ---------------------------------------------------------

    /// `Some((Multimodal<N> name, single-field param ident))` if `input` is the
    /// structural multimodal form (a single user field of
    /// `list<variant<… Role::Multimodal>>`), else `None`.
    fn input_multimodal(&self, input: &InputSchema) -> anyhow::Result<Option<(String, String)>> {
        match input_multimodal_cases(self.type_naming.graph(), input)? {
            Some(cases) => {
                let name = self.multimodal_name(&cases)?;
                let fields = user_supplied_fields(input);
                let param = self.input_param_field_idents(&fields)[0].clone();
                Ok(Some((name, param)))
            }
            None => Ok(None),
        }
    }

    /// The generated `Multimodal<N>` name for a precomputed modality set.
    fn multimodal_name(&self, cases: &[(String, SchemaType)]) -> anyhow::Result<String> {
        self.multimodals
            .iter()
            .find(|(existing, _)| existing.as_slice() == cases)
            .map(|(_, name)| name.clone())
            .ok_or_else(|| {
                anyhow!("multimodal modality set was not precomputed; this is a generator bug")
            })
    }

    /// Fully-qualified reference to a generated `Multimodal<N>` sealed trait.
    fn multimodal_type_ref(&self, name: &str) -> String {
        format!("{}.{name}", self.client_pkg())
    }

    /// `List[<Multimodal<N>>]`, the Scala surface type of a multimodal value.
    fn multimodal_list_type(&self, name: &str) -> String {
        format!(
            "_root_.scala.collection.immutable.List[{}]",
            self.multimodal_type_ref(name)
        )
    }

    /// Emits a `sealed trait Multimodal<N>` plus a companion object with one
    /// `final case class <Modality>(value: <PayloadType>)` per modality, for
    /// every distinct multimodal modality set discovered in the agent.
    fn write_multimodal_definitions(&self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        for (cases, name) in &self.multimodals {
            let case_names = self.type_member_idents(cases.iter().map(|(case, _)| case.as_str()));
            let self_type = self.multimodal_type_ref(name);
            self.write_sealed_trait(writer, name, |w, this| {
                for (idx, (_, payload)) in cases.iter().enumerate() {
                    let case_name = &case_names[idx];
                    let payload_type = this.type_reference(payload)?;
                    w.line(format!(
                        "final case class {case_name}(value: {payload_type}) extends {self_type}"
                    ));
                }
                Ok(())
            })?;
            writer.blank();
        }
        Ok(())
    }

    /// Emits the `encode<Multimodal<N>>` / `decode<Multimodal<N>>` element
    /// codecs and the `encode<Multimodal<N>>List` / `decode<Multimodal<N>>List`
    /// list codecs for every multimodal modality set, inside the `Codecs`
    /// object. A multimodal value encodes to a `list<variant<…>>` where the
    /// modality index is the variant case index.
    fn write_multimodal_codecs(&self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        for (cases, name) in &self.multimodals {
            let case_names = self.type_member_idents(cases.iter().map(|(case, _)| case.as_str()));
            let self_type = self.multimodal_type_ref(name);

            // encode<Name>: modality -> variant SchemaValue
            writer.line(format!(
                "def encode{name}(value: {self_type}): {SCHEMA_VALUE_TYPE} = {{"
            ));
            writer.indent();
            writer.line("value match {");
            writer.indent();
            for (idx, (_, payload)) in cases.iter().enumerate() {
                let case_name = &case_names[idx];
                let enc = self.encode_expr("inner", payload, 0)?;
                writer.line(format!("case {self_type}.{case_name}(inner) => {{"));
                writer.indent();
                writer.line(format!("val p = {enc}"));
                writer.line(format!("{SV}.VariantValue({idx}, _root_.scala.Some(p))"));
                writer.dedent();
                writer.line("}");
            }
            writer.dedent();
            writer.line("}");
            writer.dedent();
            writer.line("}");
            writer.blank();

            // decode<Name>: variant SchemaValue -> modality
            writer.line(format!(
                "def decode{name}(value: {SCHEMA_VALUE_TYPE}): {self_type} = {{"
            ));
            writer.indent();
            writer.line(format!(
                "val (caseIndex, payload) = {CODEC}.variantCase(value)"
            ));
            writer.line("caseIndex match {");
            writer.indent();
            for (idx, (case_label, payload)) in cases.iter().enumerate() {
                let case_name = &case_names[idx];
                let context = scala_string_literal(&format!("{name}.{case_label}"));
                let dec = self.decode_expr("p", payload, 0)?;
                writer.line(format!("case {idx} => {{"));
                writer.indent();
                writer.line(format!(
                    "val p = {CODEC}.requiredPayload(payload, {context})"
                ));
                writer.line(format!("{self_type}.{case_name}({dec})"));
                writer.dedent();
                writer.line("}");
            }
            writer.line(format!(
                "case other => throw {BRIDGE_EXCEPTION}(s\"Invalid multimodal variant case index for {name}: $other\")"
            ));
            writer.dedent();
            writer.line("}");
            writer.dedent();
            writer.line("}");
            writer.blank();

            // List codecs.
            let list_ty = self.multimodal_list_type(name);
            writer.line(format!(
                "def encode{name}List(values: {list_ty}): {SCHEMA_VALUE_TYPE} ="
            ));
            writer.indent();
            writer.line(format!("{SV}.ListValue(values.map(encode{name}))"));
            writer.dedent();
            writer.blank();
            writer.line(format!(
                "def decode{name}List(value: {SCHEMA_VALUE_TYPE}): {list_ty} ="
            ));
            writer.indent();
            writer.line(format!("{CODEC}.listElements(value).map(decode{name})"));
            writer.dedent();
            writer.blank();
        }
        Ok(())
    }

    /// Unique Scala term identifiers for the given input fields.
    ///
    /// Names are disambiguated away from each other and from the internal
    /// identifiers emitted alongside them in constructor/method bodies (helper
    /// defs, locals, inherited members, and the synthetic `when`/`phantom`
    /// parameters), so a user parameter can never shadow or collide with
    /// generated code. A single combined reserved set is used for both
    /// constructors and methods, keeping the names produced here identical at
    /// every call site for a given input (wire encoding is positional, so the
    /// chosen names never affect the wire format).
    fn input_param_field_idents(&self, fields: &[&NamedField]) -> Vec<String> {
        let mut reserved: Vec<String> =
            RESERVED_PARAM_NAMES.iter().map(|s| s.to_string()).collect();
        for i in 0..fields.len() {
            reserved.push(format!("f{i}"));
        }
        // A user parameter is the top-level (`depth == 0`) `val_expr` passed to
        // `encode_expr` in `write_param_record`, so it must also avoid the
        // temp-local names the structural encoders emit at depth 0 (otherwise a
        // generated `val e0 = e0...` would shadow or forward-reference the
        // parameter). These mirror the depth-0 names produced by
        // `encode_structural` / `encode_tuple`.
        for name in ["e0", "elems0", "k0", "v0", "r0", "l0", "p", "t0"] {
            reserved.push(name.to_string());
        }
        for i in 0..MAX_TUPLE_ARITY {
            reserved.push(format!("te{i}"));
        }
        let reserved_refs: Vec<&str> = reserved.iter().map(|s| s.as_str()).collect();
        unique_idents_with_reserved(
            fields
                .iter()
                .map(|f| to_scala_term_ident(&f.name, self.same_language))
                .collect(),
            &reserved_refs,
        )
    }

    /// Unique Scala *term* identifiers for generated named-type members in
    /// term position (record fields, flag fields), disambiguated away from each
    /// other and from [`RESERVED_MEMBER_NAMES`]. Used at every site that emits
    /// these names (type definition and codecs) so they never drift apart.
    fn term_member_idents<'a>(&self, names: impl Iterator<Item = &'a str>) -> Vec<String> {
        unique_idents_with_reserved(
            names
                .map(|n| to_scala_term_ident(n, self.same_language))
                .collect(),
            RESERVED_MEMBER_NAMES,
        )
    }

    /// Unique Scala *type* identifiers for generated named-type members in type
    /// position (variant/enum/union cases), disambiguated away from each other
    /// and from [`RESERVED_MEMBER_NAMES`]. Used at every site that emits these
    /// names (type definition and codecs) so they never drift apart.
    fn type_member_idents<'a>(&self, names: impl Iterator<Item = &'a str>) -> Vec<String> {
        unique_idents_with_reserved(
            names
                .map(|n| to_scala_type_ident(n, self.same_language))
                .collect(),
            RESERVED_MEMBER_NAMES,
        )
    }

    /// The `(returnType, decodeBlock)` for a method's output. The decode block
    /// is a Scala expression operating on `__result` (the
    /// `AgentInvocationResult`) producing a value of `returnType`.
    fn output_return(&self, output: &OutputSchema) -> anyhow::Result<(String, String)> {
        // A multimodal output (`list<variant<… Role::Multimodal>>`) is surfaced
        // as `List[Multimodal<N>]`, decoded through the generated list codec.
        if let Some(cases) = output_multimodal_cases(self.type_naming.graph(), output)? {
            let name = self.multimodal_name(&cases)?;
            let ret_ty = self.multimodal_list_type(&name);
            let block = format!(
                "val __value = __result.result.getOrElse(throw {BRIDGE_EXCEPTION}(\"Missing result value for an await invocation\"))\n{}.{CODECS_OBJECT}.decode{name}List(__value)",
                self.client_pkg()
            );
            return Ok((ret_ty, block));
        }
        match output {
            OutputSchema::Unit => Ok((UNIT.to_string(), "()".to_string())),
            OutputSchema::Single(ty) => {
                let ret_ty = self.type_reference(ty)?;
                let decode = self.decode_expr("__value", ty, 0)?;
                let block = format!(
                    "val __value = __result.result.getOrElse(throw {BRIDGE_EXCEPTION}(\"Missing result value for an await invocation\"))\n{decode}"
                );
                Ok((ret_ty, block))
            }
        }
    }

    // --- Type definitions ---------------------------------------------------

    /// Emits a Scala definition for every generated named type.
    fn write_type_definitions(&self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        let types: Vec<(SchemaType, ScalaTypeName)> = self
            .type_naming
            .types()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect();

        for (typ, name) in &types {
            let ScalaTypeName::Derived(name_str) = name else {
                // Remapped names (e.g. the `uuid.Uuid` builtin) reference an
                // existing runtime type and emit no definition.
                continue;
            };
            let resolved = self.resolve_ref(typ);
            if !is_named_composite(resolved) {
                // Non-composite named defs (aliases to scalars / lists / …) are
                // inlined at their use sites by `type_reference`, so no Scala
                // definition is emitted. Scala 2.13 has no top-level `type`
                // aliases, so inlining keeps the cross-build simple.
                continue;
            }
            self.write_type_definition(writer, name_str, resolved)?;
            writer.blank();
        }
        Ok(())
    }

    /// Emits one named type definition given its already ref-resolved body.
    fn write_type_definition(
        &self,
        writer: &mut ScalaWriter,
        name: &str,
        resolved: &SchemaType,
    ) -> anyhow::Result<()> {
        let self_type = format!("{}.{name}", self.client_pkg());
        match resolved {
            SchemaType::Record { fields, .. } => {
                let field_names = self.term_member_idents(fields.iter().map(|f| f.name.as_str()));
                if fields.is_empty() {
                    writer.line(format!("final case class {name}()"));
                } else {
                    writer.line(format!("final case class {name}("));
                    writer.indent();
                    for (idx, field) in fields.iter().enumerate() {
                        let field_type = self.type_reference(&field.body)?;
                        let comma = if idx + 1 < fields.len() { "," } else { "" };
                        writer.line(format!("{}: {field_type}{comma}", field_names[idx]));
                    }
                    writer.dedent();
                    writer.line(")");
                }
            }
            SchemaType::Variant { cases, .. } => {
                let case_names = self.type_member_idents(cases.iter().map(|c| c.name.as_str()));
                self.write_sealed_trait(writer, name, |w, this| {
                    for (idx, case) in cases.iter().enumerate() {
                        let case_name = &case_names[idx];
                        match &case.payload {
                            Some(payload) => {
                                let payload_type = this.type_reference(payload)?;
                                w.line(format!(
                                    "final case class {case_name}(value: {payload_type}) extends {self_type}"
                                ));
                            }
                            None => {
                                w.line(format!("case object {case_name} extends {self_type}"));
                            }
                        }
                    }
                    Ok(())
                })?;
            }
            SchemaType::Enum { cases, .. } => {
                let case_names = self.type_member_idents(cases.iter().map(|c| c.as_str()));
                self.write_sealed_trait(writer, name, |w, _this| {
                    for case_name in &case_names {
                        w.line(format!("case object {case_name} extends {self_type}"));
                    }
                    Ok(())
                })?;
            }
            SchemaType::Flags { flags, .. } => {
                let flag_names = self.term_member_idents(flags.iter().map(|f| f.as_str()));
                if flag_names.is_empty() {
                    writer.line(format!("final case class {name}()"));
                } else {
                    writer.line(format!("final case class {name}("));
                    writer.indent();
                    for (idx, flag_name) in flag_names.iter().enumerate() {
                        let comma = if idx + 1 < flag_names.len() { "," } else { "" };
                        writer.line(format!("{flag_name}: _root_.scala.Boolean{comma}"));
                    }
                    writer.dedent();
                    writer.line(")");
                }
            }
            SchemaType::Union { spec, .. } => {
                let branch_names =
                    self.type_member_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                self.write_sealed_trait(writer, name, |w, this| {
                    for (idx, branch) in spec.branches.iter().enumerate() {
                        let branch_name = &branch_names[idx];
                        let payload_type = this.type_reference(&branch.body)?;
                        w.line(format!(
                            "final case class {branch_name}(value: {payload_type}) extends {self_type}"
                        ));
                    }
                    Ok(())
                })?;
            }
            other => {
                bail!("Unexpected non-composite type reached write_type_definition: {other:?}")
            }
        }
        Ok(())
    }

    /// Emits `sealed trait <name> ...` plus a companion `object <name>`
    /// containing the subtype definitions produced by `body`.
    fn write_sealed_trait(
        &self,
        writer: &mut ScalaWriter,
        name: &str,
        body: impl FnOnce(&mut ScalaWriter, &Self) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        writer.line(format!(
            "sealed trait {name} extends Product with Serializable"
        ));
        writer.line(format!("object {name} {{"));
        writer.indent();
        body(writer, self)?;
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    // --- Codecs -------------------------------------------------------------

    /// Emits `object Codecs { … }` containing an `encode<Name>` /
    /// `decode<Name>` pair for every generated named composite type.
    ///
    /// Encode functions map the generated Scala type to a runtime
    /// [`SchemaValue`](golem.bridge.runtime.SchemaValue); decode functions do
    /// the reverse, throwing [`BridgeException`](golem.bridge.runtime.BridgeException)
    /// on a wire-shape mismatch. Remapped types (the `uuid.Uuid` builtin) and
    /// non-composite aliases are encoded inline by `encode_expr` / `decode_expr`
    /// and have no entry here.
    fn write_codecs(&self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        let types: Vec<(SchemaType, ScalaTypeName)> = self
            .type_naming
            .types()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect();

        writer.line(format!("object {CODECS_OBJECT} {{"));
        writer.indent();

        for (typ, name) in &types {
            let ScalaTypeName::Derived(name_str) = name else {
                continue;
            };
            let resolved = self.resolve_ref(typ);
            if !is_named_composite(resolved) {
                continue;
            }
            self.write_encode_fn(writer, name_str, resolved)?;
            writer.blank();
            self.write_decode_fn(writer, name_str, resolved)?;
            writer.blank();
        }

        // Codecs for the generated multimodal sealed traits.
        self.write_multimodal_codecs(writer)?;

        writer.dedent();
        writer.line("}");
        writer.blank();
        Ok(())
    }

    /// Emits `def encode<Name>(value: <ClientType>): SchemaValue = …`.
    fn write_encode_fn(
        &self,
        writer: &mut ScalaWriter,
        name: &str,
        resolved: &SchemaType,
    ) -> anyhow::Result<()> {
        let self_type = format!("{}.{name}", self.client_pkg());
        writer.line(format!(
            "def encode{name}(value: {self_type}): {SCHEMA_VALUE_TYPE} = {{"
        ));
        writer.indent();
        match resolved {
            SchemaType::Record { fields, .. } => {
                let field_names = self.term_member_idents(fields.iter().map(|f| f.name.as_str()));
                let mut elems = Vec::new();
                for (idx, field) in fields.iter().enumerate() {
                    let enc =
                        self.encode_expr(&format!("value.{}", field_names[idx]), &field.body, 0)?;
                    writer.line(format!("val f{idx} = {enc}"));
                    elems.push(format!("f{idx}"));
                }
                writer.line(format!("{SV}.RecordValue({LIST}({}))", elems.join(", ")));
            }
            SchemaType::Variant { cases, .. } => {
                let case_names = self.type_member_idents(cases.iter().map(|c| c.name.as_str()));
                writer.line("value match {");
                writer.indent();
                for (idx, case) in cases.iter().enumerate() {
                    let case_name = &case_names[idx];
                    match &case.payload {
                        Some(payload) => {
                            let enc = self.encode_expr("inner", payload, 0)?;
                            writer.line(format!("case {self_type}.{case_name}(inner) => {{"));
                            writer.indent();
                            writer.line(format!("val p = {enc}"));
                            writer.line(format!("{SV}.VariantValue({idx}, _root_.scala.Some(p))"));
                            writer.dedent();
                            writer.line("}");
                        }
                        None => {
                            writer.line(format!(
                                "case {self_type}.{case_name} => {SV}.VariantValue({idx}, _root_.scala.None)"
                            ));
                        }
                    }
                }
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Enum { cases, .. } => {
                let case_names = self.type_member_idents(cases.iter().map(|c| c.as_str()));
                writer.line("value match {");
                writer.indent();
                for (idx, case_name) in case_names.iter().enumerate() {
                    writer.line(format!(
                        "case {self_type}.{case_name} => {SV}.EnumValue({idx})"
                    ));
                }
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Flags { flags, .. } => {
                let flag_names = self.term_member_idents(flags.iter().map(|f| f.as_str()));
                let bits = flag_names
                    .iter()
                    .map(|f| format!("value.{f}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                writer.line(format!("{SV}.FlagsValue({LIST}({bits}))"));
            }
            SchemaType::Union { spec, .. } => {
                let branch_names =
                    self.type_member_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                writer.line("value match {");
                writer.indent();
                for (idx, branch) in spec.branches.iter().enumerate() {
                    let branch_name = &branch_names[idx];
                    let tag = branch.tag.as_str();
                    let enc = self.encode_expr("inner", &branch.body, 0)?;
                    writer.line(format!("case {self_type}.{branch_name}(inner) => {{"));
                    writer.indent();
                    writer.line(format!("val b = {enc}"));
                    writer.line(format!("{SV}.UnionValue({}, b)", scala_string_literal(tag)));
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

    /// Emits `def decode<Name>(value: SchemaValue): <ClientType> = …`.
    fn write_decode_fn(
        &self,
        writer: &mut ScalaWriter,
        name: &str,
        resolved: &SchemaType,
    ) -> anyhow::Result<()> {
        let self_type = format!("{}.{name}", self.client_pkg());
        writer.line(format!(
            "def decode{name}(value: {SCHEMA_VALUE_TYPE}): {self_type} = {{"
        ));
        writer.indent();
        match resolved {
            SchemaType::Record { fields, .. } => {
                let n = fields.len();
                writer.line(format!("val fields = {CODEC}.recordFields(value)"));
                writer.line(format!(
                    "if (fields.length != {n}) throw {BRIDGE_EXCEPTION}(s\"Expected record {name} with {n} fields, got ${{fields.length}}\")"
                ));
                let mut args = Vec::new();
                for (idx, field) in fields.iter().enumerate() {
                    let dec = self.decode_expr(&format!("fields({idx})"), &field.body, 0)?;
                    writer.line(format!("val f{idx} = {dec}"));
                    args.push(format!("f{idx}"));
                }
                writer.line(format!("{self_type}({})", args.join(", ")));
            }
            SchemaType::Variant { cases, .. } => {
                let case_names = self.type_member_idents(cases.iter().map(|c| c.name.as_str()));
                writer.line(format!(
                    "val (caseIndex, payload) = {CODEC}.variantCase(value)"
                ));
                writer.line("caseIndex match {");
                writer.indent();
                for (idx, case) in cases.iter().enumerate() {
                    let case_name = &case_names[idx];
                    match &case.payload {
                        Some(payload) => {
                            let context = scala_string_literal(&format!("{name}.{}", case.name));
                            let dec = self.decode_expr("p", payload, 0)?;
                            writer.line(format!("case {idx} => {{"));
                            writer.indent();
                            writer.line(format!(
                                "val p = {CODEC}.requiredPayload(payload, {context})"
                            ));
                            writer.line(format!("{self_type}.{case_name}({dec})"));
                            writer.dedent();
                            writer.line("}");
                        }
                        None => {
                            let msg = scala_string_literal(&format!(
                                "Unexpected payload for payload-less variant case {name}.{}",
                                case.name
                            ));
                            writer.line(format!("case {idx} => {{"));
                            writer.indent();
                            writer.line(format!(
                                "if (payload.nonEmpty) throw {BRIDGE_EXCEPTION}({msg})"
                            ));
                            writer.line(format!("{self_type}.{case_name}"));
                            writer.dedent();
                            writer.line("}");
                        }
                    }
                }
                writer.line(format!(
                    "case other => throw {BRIDGE_EXCEPTION}(s\"Invalid variant case index for {name}: $other\")"
                ));
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Enum { cases, .. } => {
                let case_names = self.type_member_idents(cases.iter().map(|c| c.as_str()));
                writer.line(format!("{CODEC}.enumCase(value) match {{"));
                writer.indent();
                for (idx, case_name) in case_names.iter().enumerate() {
                    writer.line(format!("case {idx} => {self_type}.{case_name}"));
                }
                writer.line(format!(
                    "case other => throw {BRIDGE_EXCEPTION}(s\"Invalid enum case index for {name}: $other\")"
                ));
                writer.dedent();
                writer.line("}");
            }
            SchemaType::Flags { flags, .. } => {
                let n = flags.len();
                writer.line(format!("val bits = {CODEC}.flagBits(value)"));
                writer.line(format!(
                    "if (bits.length != {n}) throw {BRIDGE_EXCEPTION}(s\"Expected flags {name} with {n} bits, got ${{bits.length}}\")"
                ));
                let args = (0..n).map(|i| format!("bits({i})")).collect::<Vec<_>>();
                writer.line(format!("{self_type}({})", args.join(", ")));
            }
            SchemaType::Union { spec, .. } => {
                let branch_names =
                    self.type_member_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                writer.line(format!("val (tag, body) = {CODEC}.unionBody(value)"));
                writer.line("tag match {");
                writer.indent();
                for (idx, branch) in spec.branches.iter().enumerate() {
                    let branch_name = &branch_names[idx];
                    let tag = scala_string_literal(branch.tag.as_str());
                    let dec = self.decode_expr("body", &branch.body, 0)?;
                    writer.line(format!("case {tag} => {self_type}.{branch_name}({dec})"));
                }
                writer.line(format!(
                    "case other => throw {BRIDGE_EXCEPTION}(s\"Unknown union branch tag for {name}: $other\")"
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

    /// A `SchemaValue` expression encoding `val_expr` (a value of the Scala type
    /// for `typ`). Named composites delegate to their `Codecs.encode<Name>`;
    /// the `uuid.Uuid` remap delegates to the runtime UUID codec; everything
    /// else is encoded structurally inline. Mirrors `type_reference`'s dispatch
    /// so the encoded value always matches the rendered type.
    fn encode_expr(
        &self,
        val_expr: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if let Some(name) = self.type_naming.type_name_for_type(typ) {
            match name {
                ScalaTypeName::Remapped(RemappedType::Uuid) => {
                    return Ok(format!("{CODEC}.encodeUuid({val_expr})"));
                }
                ScalaTypeName::Derived(name_str) => {
                    if is_named_composite(self.resolve_ref(typ)) {
                        return Ok(format!(
                            "{}.{CODECS_OBJECT}.encode{name_str}({val_expr})",
                            self.client_pkg()
                        ));
                    }
                }
            }
        }
        self.encode_structural(val_expr, typ, depth)
    }

    /// A Scala expression (of the mapped type for `typ`) decoding the
    /// `SchemaValue` expression `val_expr`. Counterpart of [`Self::encode_expr`].
    fn decode_expr(
        &self,
        val_expr: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if let Some(name) = self.type_naming.type_name_for_type(typ) {
            match name {
                ScalaTypeName::Remapped(RemappedType::Uuid) => {
                    return Ok(format!("{CODEC}.decodeUuidOrThrow({val_expr})"));
                }
                ScalaTypeName::Derived(name_str) => {
                    if is_named_composite(self.resolve_ref(typ)) {
                        return Ok(format!(
                            "{}.{CODECS_OBJECT}.decode{name_str}({val_expr})",
                            self.client_pkg()
                        ));
                    }
                }
            }
        }
        self.decode_structural(val_expr, typ, depth)
    }

    /// Structural (non-named) encoding for `typ`. Composite types must have a
    /// generated name and are handled by [`Self::encode_expr`]; reaching them
    /// here is a generator bug.
    fn encode_structural(
        &self,
        val_expr: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(format!(
                "{RUNTIME_PKG}.UnstructuredText.toSchemaValue({val_expr})"
            ));
        }
        if unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(format!(
                "{RUNTIME_PKG}.UnstructuredBinary.toSchemaValue({val_expr})"
            ));
        }

        let resolved = self.resolve_ref(typ);
        let e = format!("e{depth}");
        let next = depth + 1;
        let rendered = match resolved {
            SchemaType::Bool { .. } => format!("{SV}.BoolValue({val_expr})"),
            SchemaType::S8 { .. } => format!("{SV}.S8Value({val_expr})"),
            SchemaType::S16 { .. } => format!("{SV}.S16Value({val_expr})"),
            SchemaType::S32 { .. } => format!("{SV}.S32Value({val_expr})"),
            SchemaType::S64 { .. } => format!("{SV}.S64Value({val_expr})"),
            // The unsigned wrappers store the value in a wider signed type, so
            // route encoding through the runtime helpers that validate the
            // unsigned range before building the wire node.
            SchemaType::U8 { .. } => format!("{CODEC}.encodeUByte({val_expr})"),
            SchemaType::U16 { .. } => format!("{CODEC}.encodeUShort({val_expr})"),
            SchemaType::U32 { .. } => format!("{CODEC}.encodeUInt({val_expr})"),
            SchemaType::U64 { .. } => format!("{CODEC}.encodeULong({val_expr})"),
            SchemaType::F32 { .. } => format!("{SV}.F32Value({val_expr})"),
            SchemaType::F64 { .. } => format!("{SV}.F64Value({val_expr})"),
            // A scala.Char can hold a lone surrogate; route through the runtime
            // helper so an invalid Unicode scalar value is rejected on encode.
            SchemaType::Char { .. } => format!("{CODEC}.encodeChar({val_expr})"),
            SchemaType::String { .. } => format!("{SV}.StringValue({val_expr})"),
            SchemaType::Option { inner, .. } => {
                let inner_enc = self.encode_expr(&e, inner, next)?;
                format!("{SV}.OptionValue({val_expr}.map({e} => {inner_enc}))")
            }
            SchemaType::List { element, .. } => {
                let inner_enc = self.encode_expr(&e, element, next)?;
                format!("{SV}.ListValue({val_expr}.map({e} => {inner_enc}))")
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner_enc = self.encode_expr(&e, element, next)?;
                let len = *length;
                let elems = format!("elems{depth}");
                format!(
                    "{{\n  val {elems} = {val_expr}.map({e} => {inner_enc})\n  if ({elems}.length != {len}) throw {BRIDGE_EXCEPTION}(s\"Expected fixed-list of length {len}, got ${{{elems}.length}}\")\n  {SV}.FixedListValue({elems})\n}}"
                )
            }
            SchemaType::Map { key, value, .. } => {
                let k = format!("k{depth}");
                let v = format!("v{depth}");
                let key_enc = self.encode_expr(&k, key, next)?;
                let val_enc = self.encode_expr(&v, value, next)?;
                format!(
                    "{SV}.MapValue({val_expr}.toList.map {{ case ({k}, {v}) => {SCHEMA_MAP_ENTRY}({key_enc}, {val_enc}) }})"
                )
            }
            SchemaType::Tuple { elements, .. } => self.encode_tuple(val_expr, elements, depth)?,
            SchemaType::Result { spec, .. } => {
                let r = format!("r{depth}");
                let l = format!("l{depth}");
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let enc = self.encode_expr(&r, ok_type, next)?;
                        format!(
                            "{{ val p = {enc}; {SV}.ResultValue({SCHEMA_RESULT}.Ok(_root_.scala.Some(p))) }}"
                        )
                    }
                    None => format!("{SV}.ResultValue({SCHEMA_RESULT}.Ok(_root_.scala.None))"),
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let enc = self.encode_expr(&l, err_type, next)?;
                        format!(
                            "{{ val p = {enc}; {SV}.ResultValue({SCHEMA_RESULT}.Err(_root_.scala.Some(p))) }}"
                        )
                    }
                    None => format!("{SV}.ResultValue({SCHEMA_RESULT}.Err(_root_.scala.None))"),
                };
                format!(
                    "{val_expr} match {{\n  case _root_.scala.Right({r}) => {ok_arm}\n  case _root_.scala.Left({l}) => {err_arm}\n}}"
                )
            }
            SchemaType::Path { .. } => format!("{SV}.PathValue({val_expr})"),
            SchemaType::Url { .. } => format!("{SV}.UrlValue({val_expr})"),
            SchemaType::Datetime { .. } => format!("{SV}.DatetimeValue({val_expr}.toString)"),
            SchemaType::Duration { .. } => format!("{SV}.DurationValue({val_expr})"),
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => bail!(
                "Composite schema type reached encode_structural without a registered name: {resolved:?}"
            ),
            SchemaType::Ref { .. } => unreachable!("Ref was resolved to its body via resolve_ref"),
            SchemaType::Text { .. } | SchemaType::Binary { .. } => bail!(
                "Bare text/binary rich scalars have no Scala bridge encoding; \
                 wrap them in the unstructured text/binary variant ({resolved:?})"
            ),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                bail!("Cannot encode unsupported schema variant in the Scala bridge: {resolved:?}")
            }
        };
        Ok(rendered)
    }

    /// Structural (non-named) decoding for `typ`. Counterpart of
    /// [`Self::encode_structural`].
    fn decode_structural(
        &self,
        val_expr: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if let Some(restrictions) = unstructured_text_restrictions(self.type_naming.graph(), typ)? {
            let allowed = restrictions
                .languages
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|code| scala_string_literal(code))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(format!(
                "{RUNTIME_PKG}.UnstructuredText.fromSchemaValue(\"output\", {val_expr}, {LIST}({allowed})).fold(__err => throw {BRIDGE_EXCEPTION}(__err), _root_.scala.Predef.identity)"
            ));
        }
        if unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(format!(
                "{RUNTIME_PKG}.UnstructuredBinary.fromSchemaValue(\"output\", {val_expr}).fold(__err => throw {BRIDGE_EXCEPTION}(__err), _root_.scala.Predef.identity)"
            ));
        }

        let resolved = self.resolve_ref(typ);
        let e = format!("e{depth}");
        let next = depth + 1;
        let rendered = match resolved {
            SchemaType::Bool { .. } => format!("{CODEC}.asBool({val_expr})"),
            SchemaType::S8 { .. } => format!("{CODEC}.asByte({val_expr})"),
            SchemaType::S16 { .. } => format!("{CODEC}.asShort({val_expr})"),
            SchemaType::S32 { .. } => format!("{CODEC}.asInt({val_expr})"),
            SchemaType::S64 { .. } => format!("{CODEC}.asLong({val_expr})"),
            SchemaType::U8 { .. } => format!("{CODEC}.asUByte({val_expr})"),
            SchemaType::U16 { .. } => format!("{CODEC}.asUShort({val_expr})"),
            SchemaType::U32 { .. } => format!("{CODEC}.asUInt({val_expr})"),
            SchemaType::U64 { .. } => format!("{CODEC}.asULong({val_expr})"),
            SchemaType::F32 { .. } => format!("{CODEC}.asFloat({val_expr})"),
            SchemaType::F64 { .. } => format!("{CODEC}.asDouble({val_expr})"),
            SchemaType::Char { .. } => format!("{CODEC}.asChar({val_expr})"),
            SchemaType::String { .. } => format!("{CODEC}.asString({val_expr})"),
            SchemaType::Option { inner, .. } => {
                let inner_dec = self.decode_expr(&e, inner, next)?;
                format!("{CODEC}.optionValue({val_expr}).map({e} => {inner_dec})")
            }
            SchemaType::List { element, .. } => {
                let inner_dec = self.decode_expr(&e, element, next)?;
                format!("{CODEC}.listElements({val_expr}).map({e} => {inner_dec})")
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner_dec = self.decode_expr(&e, element, next)?;
                let len = *length;
                let elems = format!("elems{depth}");
                format!(
                    "{{\n  val {elems} = {CODEC}.fixedListElements({val_expr})\n  if ({elems}.length != {len}) throw {BRIDGE_EXCEPTION}(s\"Expected fixed-list of length {len}, got ${{{elems}.length}}\")\n  {elems}.map({e} => {inner_dec})\n}}"
                )
            }
            SchemaType::Map { key, value, .. } => {
                let me = format!("me{depth}");
                let key_dec = self.decode_expr(&format!("{me}.key"), key, next)?;
                let val_dec = self.decode_expr(&format!("{me}.value"), value, next)?;
                format!(
                    "{CODEC}.mapEntries({val_expr}).map {{ {me} =>\n  val k = {key_dec}\n  val v = {val_dec}\n  (k, v)\n}}.toMap"
                )
            }
            SchemaType::Tuple { elements, .. } => self.decode_tuple(val_expr, elements, depth)?,
            SchemaType::Result { spec, .. } => {
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let dec = self.decode_expr("p", ok_type, next)?;
                        format!(
                            "{{ val p = {CODEC}.requiredPayload(payload{depth}, \"result ok\"); _root_.scala.Right({dec}) }}"
                        )
                    }
                    None => format!(
                        "{{ if (payload{depth}.nonEmpty) throw {BRIDGE_EXCEPTION}(\"Unexpected payload for unit result ok\"); _root_.scala.Right(()) }}"
                    ),
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let dec = self.decode_expr("p", err_type, next)?;
                        format!(
                            "{{ val p = {CODEC}.requiredPayload(payload{depth}, \"result err\"); _root_.scala.Left({dec}) }}"
                        )
                    }
                    None => format!(
                        "{{ if (payload{depth}.nonEmpty) throw {BRIDGE_EXCEPTION}(\"Unexpected payload for unit result err\"); _root_.scala.Left(()) }}"
                    ),
                };
                format!(
                    "{CODEC}.resultValue({val_expr}) match {{\n  case {SCHEMA_RESULT}.Ok(payload{depth}) => {ok_arm}\n  case {SCHEMA_RESULT}.Err(payload{depth}) => {err_arm}\n}}"
                )
            }
            SchemaType::Path { .. } => format!("{CODEC}.asPath({val_expr})"),
            SchemaType::Url { .. } => format!("{CODEC}.asUrl({val_expr})"),
            SchemaType::Datetime { .. } => format!("{CODEC}.asDatetime({val_expr})"),
            SchemaType::Duration { .. } => format!("{CODEC}.asDuration({val_expr})"),
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => bail!(
                "Composite schema type reached decode_structural without a registered name: {resolved:?}"
            ),
            SchemaType::Ref { .. } => unreachable!("Ref was resolved to its body via resolve_ref"),
            SchemaType::Text { .. } | SchemaType::Binary { .. } => bail!(
                "Bare text/binary rich scalars have no Scala bridge decoding; \
                 wrap them in the unstructured text/binary variant ({resolved:?})"
            ),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                bail!("Cannot decode unsupported schema variant in the Scala bridge: {resolved:?}")
            }
        };
        Ok(rendered)
    }

    /// Encodes a tuple value into a `tuple` `SchemaValue`, honoring the same
    /// Unit / `Tuple1` / arity rules as [`Self::tuple_type`].
    fn encode_tuple(
        &self,
        val_expr: &str,
        elements: &[SchemaType],
        depth: usize,
    ) -> anyhow::Result<String> {
        if elements.len() > MAX_TUPLE_ARITY {
            bail!(
                "Tuple has arity {} but Scala only supports tuples up to arity {MAX_TUPLE_ARITY}",
                elements.len()
            );
        }
        if elements.is_empty() {
            return Ok(format!("{SV}.TupleValue({LIST}())"));
        }
        let t = format!("t{depth}");
        let next = depth + 1;
        let mut lines = vec![format!("  val {t} = {val_expr}")];
        let mut names = Vec::new();
        for (idx, element) in elements.iter().enumerate() {
            let enc = self.encode_expr(&format!("{t}._{}", idx + 1), element, next)?;
            lines.push(format!("  val te{idx} = {enc}"));
            names.push(format!("te{idx}"));
        }
        lines.push(format!("  {SV}.TupleValue({LIST}({}))", names.join(", ")));
        Ok(format!("{{\n{}\n}}", lines.join("\n")))
    }

    /// Decodes a `tuple` `SchemaValue` into a tuple value (or `Unit`/`Tuple1`),
    /// validating the element count.
    fn decode_tuple(
        &self,
        val_expr: &str,
        elements: &[SchemaType],
        depth: usize,
    ) -> anyhow::Result<String> {
        if elements.len() > MAX_TUPLE_ARITY {
            bail!(
                "Tuple has arity {} but Scala only supports tuples up to arity {MAX_TUPLE_ARITY}",
                elements.len()
            );
        }
        let elems = format!("elems{depth}");
        if elements.is_empty() {
            return Ok(format!(
                "{{\n  val {elems} = {CODEC}.tupleElements({val_expr})\n  if ({elems}.nonEmpty) throw {BRIDGE_EXCEPTION}(s\"Expected empty tuple, got ${{{elems}.length}} elements\")\n  ()\n}}"
            ));
        }
        let n = elements.len();
        let next = depth + 1;
        let mut lines = vec![
            format!("  val {elems} = {CODEC}.tupleElements({val_expr})"),
            format!(
                "  if ({elems}.length != {n}) throw {BRIDGE_EXCEPTION}(s\"Expected tuple of arity {n}, got ${{{elems}.length}}\")"
            ),
        ];
        let mut names = Vec::new();
        for (idx, element) in elements.iter().enumerate() {
            let dec = self.decode_expr(&format!("{elems}({idx})"), element, next)?;
            lines.push(format!("  val te{idx} = {dec}"));
            names.push(format!("te{idx}"));
        }
        let ctor = if n == 1 {
            format!("_root_.scala.Tuple1({})", names.join(", "))
        } else {
            format!("({})", names.join(", "))
        };
        lines.push(format!("  {ctor}"));
        Ok(format!("{{\n{}\n}}", lines.join("\n")))
    }

    // --- Type references ----------------------------------------------------

    /// Renders the Scala type expression for `typ`, mirroring the Scala SDK's
    /// WIT type mapping.
    ///
    /// Every emitted type reference is fully qualified (`_root_.…`) so that a
    /// generated variant/union case name (which lives in a nested companion
    /// scope) can never shadow a runtime type, a generated type, or a Scala
    /// standard-library type referenced in the same scope. This mirrors the
    /// Scala SDK's codegen style.
    fn type_reference(&self, typ: &SchemaType) -> anyhow::Result<String> {
        // A registered composite name (record / variant / enum / flags / union)
        // or a remapped runtime type (the `uuid.Uuid` builtin) is referenced by
        // its name. Non-composite named defs are inlined below.
        if let Some(name) = self.type_naming.type_name_for_type(typ) {
            match name {
                ScalaTypeName::Remapped(remapped) => {
                    return Ok(format!("{RUNTIME_PKG}.{}", remapped.rendered()));
                }
                ScalaTypeName::Derived(name_str) => {
                    if is_named_composite(self.resolve_ref(typ)) {
                        return Ok(format!("{}.{name_str}", self.client_pkg()));
                    }
                }
            }
        }

        // Role-marked unstructured-text/binary variant → ergonomic runtime
        // wrapper type.
        if unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(format!("{RUNTIME_PKG}.UnstructuredText"));
        }
        if unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some() {
            return Ok(format!("{RUNTIME_PKG}.UnstructuredBinary"));
        }

        // A recursive ref that resolves to a non-composite body (e.g. a
        // recursive alias `type a = list<a>`) cannot be inlined without looping
        // forever, and Scala has no top-level alias to break the cycle.
        if self.type_naming.is_recursive_ref(typ) && !is_named_composite(self.resolve_ref(typ)) {
            bail!(
                "Recursive non-composite type alias cannot be represented in the Scala bridge: {typ:?}"
            );
        }

        let resolved = self.resolve_ref(typ);
        match resolved {
            SchemaType::Bool { .. } => Ok("_root_.scala.Boolean".to_string()),
            SchemaType::S8 { .. } => Ok("_root_.scala.Byte".to_string()),
            SchemaType::S16 { .. } => Ok("_root_.scala.Short".to_string()),
            SchemaType::S32 { .. } => Ok("_root_.scala.Int".to_string()),
            SchemaType::S64 { .. } => Ok("_root_.scala.Long".to_string()),
            SchemaType::U8 { .. } => Ok(format!("{RUNTIME_PKG}.UByte")),
            SchemaType::U16 { .. } => Ok(format!("{RUNTIME_PKG}.UShort")),
            SchemaType::U32 { .. } => Ok(format!("{RUNTIME_PKG}.UInt")),
            SchemaType::U64 { .. } => Ok(format!("{RUNTIME_PKG}.ULong")),
            SchemaType::F32 { .. } => Ok("_root_.scala.Float".to_string()),
            SchemaType::F64 { .. } => Ok("_root_.scala.Double".to_string()),
            SchemaType::Char { .. } => Ok("_root_.scala.Char".to_string()),
            SchemaType::String { .. } => Ok("_root_.scala.Predef.String".to_string()),
            SchemaType::Option { inner, .. } => Ok(format!(
                "_root_.scala.Option[{}]",
                self.type_reference(inner)?
            )),
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                Ok(format!(
                    "_root_.scala.collection.immutable.List[{}]",
                    self.type_reference(element)?
                ))
            }
            SchemaType::Tuple { elements, .. } => self.tuple_type(elements),
            SchemaType::Map { key, value, .. } => Ok(format!(
                "_root_.scala.collection.immutable.Map[{}, {}]",
                self.type_reference(key)?,
                self.type_reference(value)?
            )),
            SchemaType::Result { spec, .. } => {
                let ok_type = match spec.ok.as_deref() {
                    Some(ty) => self.type_reference(ty)?,
                    None => "_root_.scala.Unit".to_string(),
                };
                let err_type = match spec.err.as_deref() {
                    Some(ty) => self.type_reference(ty)?,
                    None => "_root_.scala.Unit".to_string(),
                };
                Ok(format!("_root_.scala.Either[{err_type}, {ok_type}]"))
            }
            SchemaType::Path { .. } | SchemaType::Url { .. } => {
                Ok("_root_.scala.Predef.String".to_string())
            }
            SchemaType::Datetime { .. } => Ok("_root_.java.time.Instant".to_string()),
            SchemaType::Duration { .. } => Ok("_root_.scala.Long".to_string()),
            // Named composites should already have been resolved to their name
            // above; reaching here means the walker did not register one.
            SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => bail!(
                "Composite schema type reached type_reference without a registered name: {resolved:?}"
            ),
            SchemaType::Ref { .. } => {
                unreachable!("Ref was resolved to its body via resolve_ref")
            }
            SchemaType::Text { .. } | SchemaType::Binary { .. } => bail!(
                "Bare text/binary rich scalars have no Scala bridge type; \
                 wrap them in the unstructured text/binary variant ({resolved:?})"
            ),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => bail!(
                "Cannot emit Scala type reference for unsupported schema variant: {resolved:?}"
            ),
        }
    }

    fn tuple_type(&self, elements: &[SchemaType]) -> anyhow::Result<String> {
        if elements.len() > MAX_TUPLE_ARITY {
            bail!(
                "Tuple has arity {} but Scala only supports tuples up to arity {MAX_TUPLE_ARITY}",
                elements.len()
            );
        }
        let types = elements
            .iter()
            .map(|item| self.type_reference(item))
            .collect::<anyhow::Result<Vec<_>>>()?;
        match types.as_slice() {
            // An empty tuple is the unit value; `()` is not a valid type
            // expression, so map it to `Unit`.
            [] => Ok("_root_.scala.Unit".to_string()),
            // `(T)` is just `T` in Scala, so a one-element tuple needs the
            // explicit `Tuple1` to stay distinct from the bare element on the
            // wire (a `tuple` node, not the element itself).
            [single] => Ok(format!("_root_.scala.Tuple1[{single}]")),
            _ => Ok(format!("({})", types.join(", "))),
        }
    }

    /// Resolve a [`SchemaType::Ref`] against the agent's schema graph and return
    /// the def body; inline types are returned unchanged.
    fn resolve_ref<'a>(&'a self, typ: &'a SchemaType) -> &'a SchemaType {
        match typ {
            SchemaType::Ref { id, .. } => {
                let def: &SchemaTypeDef = self
                    .type_naming
                    .graph()
                    .lookup(id)
                    .expect("Ref points to a def in the shared graph");
                &def.body
            }
            other => other,
        }
    }
}

/// Renders `value` as a plain double-quoted Scala string literal, escaping the
/// characters that would otherwise break out of, or be misinterpreted inside,
/// the literal. The result is a normal `"..."` literal (not an `s"..."`
/// interpolator), so `$` is emitted verbatim.
fn scala_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            // Any other control character would otherwise be emitted verbatim
            // into the source literal (Scala rejects a raw control char such as
            // a NUL, backspace, or form-feed inside a `"..."` literal), so
            // escape every remaining char below U+0020 as a unicode escape.
            other if (other as u32) < 0x20 => escaped.push_str(&format!("\\u{:04x}", other as u32)),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

/// The multimodal modality `(case_name, payload_schema)` pairs of a
/// `list<variant<… Role::Multimodal>>` variant, erroring if a modality case has
/// no payload (every modality must carry a body).
fn multimodal_pairs(cases: &[VariantCaseType]) -> anyhow::Result<Vec<(String, SchemaType)>> {
    cases
        .iter()
        .map(|case| {
            let payload = case.payload.clone().ok_or_else(|| {
                anyhow!(
                    "Multimodal case `{}` has no payload schema; expected a modality body",
                    case.name
                )
            })?;
            Ok((case.name.clone(), payload))
        })
        .collect()
}

/// The modality pairs of `input` if it is the structural multimodal form (a
/// single user field whose schema is `list<variant<… Role::Multimodal>>`).
fn input_multimodal_cases(
    graph: &SchemaGraph,
    input: &InputSchema,
) -> anyhow::Result<Option<Vec<(String, SchemaType)>>> {
    let fields = user_supplied_fields(input);
    if let [field] = fields.as_slice()
        && let Some(cases) = multimodal_variant_cases(graph, &field.schema)?
    {
        return Ok(Some(multimodal_pairs(cases)?));
    }
    Ok(None)
}

/// The modality pairs of `output` if it is a single multimodal return value.
fn output_multimodal_cases(
    graph: &SchemaGraph,
    output: &OutputSchema,
) -> anyhow::Result<Option<Vec<(String, SchemaType)>>> {
    if let OutputSchema::Single(ty) = output
        && let Some(cases) = multimodal_variant_cases(graph, ty)?
    {
        return Ok(Some(multimodal_pairs(cases)?));
    }
    Ok(None)
}

/// Discovers the distinct multimodal modality sets used by the agent, mapping
/// each to a generated `Multimodal<N>` name. Scans the constructor input first,
/// then each method's input and output in declaration order; structurally
/// identical sets (same modality names and payload schemas) collapse to one
/// generated type. The wire format never depends on these names (it is the
/// structural `list<variant<…>>`), so the discovery order only affects the
/// generated Scala API surface.
fn collect_multimodals(agent_type: &AgentTypeSchema) -> anyhow::Result<Vec<NamedMultimodal>> {
    let graph = &agent_type.schema;
    let mut candidates: Vec<Option<MultimodalModalities>> = Vec::new();
    candidates.push(input_multimodal_cases(
        graph,
        &agent_type.constructor.input_schema,
    )?);
    for method in &agent_type.methods {
        candidates.push(input_multimodal_cases(graph, &method.input_schema)?);
        candidates.push(output_multimodal_cases(graph, &method.output_schema)?);
    }

    let mut known: Vec<MultimodalModalities> = Vec::new();
    for cases in candidates.into_iter().flatten() {
        if !known.contains(&cases) {
            known.push(cases);
        }
    }
    Ok(known
        .into_iter()
        .enumerate()
        .map(|(idx, cases)| (cases, format!("Multimodal{idx}")))
        .collect())
}

/// Whether a (ref-resolved) schema type becomes a generated Scala
/// definition (case class / sealed trait). Other named defs are inlined.
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

/// Name of the generated client object, e.g. `foo-agent` -> `FooAgentClient`.
fn client_object_name(agent_type: &AgentTypeSchema) -> String {
    format!(
        "{}Client",
        agent_type.type_name.as_str().to_upper_camel_case()
    )
}

/// Name of the generated remote trait, e.g. `foo-agent` -> `FooAgentRemote`.
fn remote_trait_name(agent_type: &AgentTypeSchema) -> String {
    format!(
        "{}Remote",
        agent_type.type_name.as_str().to_upper_camel_case()
    )
}

/// Name of the per-method wrapper class, e.g. method `do-it` -> `DoItRemoteMethod`.
///
/// The class name is always built from the UpperCamelCase of the method name
/// (independent of the method's source casing) plus the `RemoteMethod` suffix,
/// then escaped once. The suffix guarantees the result is a non-empty, valid
/// class identifier even for method names that are Scala keywords or normalize
/// awkwardly.
fn remote_method_class_name(method_name: &str) -> String {
    escape_scala_ident(&format!(
        "{}RemoteMethod",
        method_name.to_upper_camel_case()
    ))
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
