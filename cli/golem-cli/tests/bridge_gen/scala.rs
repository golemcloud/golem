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

use crate::bridge_gen::fixtures::{
    agent, def, field, local_config, method, multi_agent_wrapper_2_types, multimodal, named_field,
    ref_to, single_agent_wrapper_types, variant_case,
};
use camino::Utf8Path;
use golem_cli::bridge_gen::scala::scala::{
    escape_scala_ident, is_scala_keyword, is_valid_scala_ident, to_scala_term_ident,
    unique_idents_with_reserved,
};
use golem_cli::bridge_gen::scala::tool::ScalaToolBridgeGenerator;
use golem_cli::bridge_gen::scala::type_name::RemappedType;
use golem_cli::bridge_gen::scala::{ScalaBridgeGenerator, ScalaBridgeMode, ScalaTypeName};
use golem_cli::bridge_gen::type_naming::{TypeName, TypeNaming};
use golem_cli::bridge_gen::{
    BridgeGenerator, BridgeMode, bridge_client_directory_name, tool_bridge_client_directory_name,
};
use golem_common::model::agent::AgentMode;
use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
use golem_common::schema::tool::{
    BoolFlagShape, CommandBody, CommandIndex, CommandTree, Doc, ErrorCase, ErrorKind, FlagShape,
    FlagSpec, Formatter, Globals, OptionShape, OptionSpec, Positional, Positionals,
    ResultSpec as ToolResultSpec, StreamSpec, TailPositional, Tool,
};
use golem_common::schema::{
    AgentTypeSchema, MetadataEnvelope, NamedFieldType, ResultSpec, SchemaType, TypeId,
};
use tempfile::TempDir;
use test_r::{test, test_dep};

struct GeneratedPackage {
    pub dir: TempDir,
    pub package_name: String,
}

impl GeneratedPackage {
    pub fn new(agent_type: AgentTypeSchema) -> Self {
        Self::new_with_mode(agent_type, ScalaBridgeMode::ExternalRest)
    }

    pub fn new_with_mode(agent_type: AgentTypeSchema, mode: ScalaBridgeMode) -> Self {
        let bridge_mode = match mode {
            ScalaBridgeMode::ExternalRest => BridgeMode::External,
            ScalaBridgeMode::GuestWasmRpc => BridgeMode::Guest,
        };
        let package_name = bridge_client_directory_name(&agent_type.type_name, bridge_mode);
        let dir = TempDir::new().unwrap();
        let target_dir = Utf8Path::from_path(dir.path()).unwrap();
        let package_dir = target_dir.join(&package_name);
        let mut generator =
            ScalaBridgeGenerator::new_with_mode(agent_type, &package_dir, true, mode).unwrap();
        generator.generate().unwrap();
        GeneratedPackage { dir, package_name }
    }

    pub fn package_dir(&self) -> camino::Utf8PathBuf {
        Utf8Path::from_path(self.dir.path())
            .unwrap()
            .join(&self.package_name)
    }
}

fn cross_compile(package_dir: &Utf8Path) {
    let output = std::process::Command::new("sbt")
        .arg("--batch")
        .arg("+compile")
        .current_dir(package_dir)
        .output()
        .expect("failed to run sbt; is it installed?");
    assert!(
        output.status.success(),
        "sbt +compile failed in {package_dir}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cross_compile_guest_if_enabled(package_dir: &Utf8Path) {
    if std::env::var_os("GOLEM_SCALA_GUEST_BRIDGE_COMPILE").is_some() {
        cross_compile(package_dir);
    }
}

fn doc(summary: &str) -> Doc {
    Doc {
        summary: summary.to_string(),
        description: String::new(),
        examples: vec![],
    }
}

fn tool_body() -> CommandBody {
    CommandBody {
        positionals: Positionals::default(),
        options: vec![],
        flags: vec![],
        constraints: vec![],
        stdin: None,
        stdout: None,
        result: None,
        errors: vec![],
        annotations: None,
    }
}

fn command_node(name: &str) -> golem_common::schema::tool::CommandNode {
    golem_common::schema::tool::CommandNode {
        name: name.to_string(),
        aliases: vec![],
        doc: doc(name),
        globals: Globals::default(),
        subcommands: vec![],
        body: None,
    }
}

fn positional(name: &str, type_: SchemaType) -> Positional {
    Positional {
        name: name.to_string(),
        doc: doc(name),
        value_name: None,
        type_,
        default: None,
        required: true,
        accepts_stdio: false,
    }
}

fn option(long: &str, shape: OptionShape) -> OptionSpec {
    OptionSpec {
        long: long.to_string(),
        short: None,
        aliases: vec![],
        doc: doc(long),
        value_name: None,
        shape,
        default: None,
        required: false,
        env_var: None,
    }
}

fn flag(long: &str, shape: FlagShape) -> FlagSpec {
    FlagSpec {
        long: long.to_string(),
        short: None,
        aliases: vec![],
        doc: doc(long),
        shape,
        env_var: None,
    }
}

fn grep_tool() -> Tool {
    let color_type = TypeId::from("color-mode");
    let color_ref = SchemaType::ref_to(color_type.clone());
    let mut root = command_node("grep");
    root.globals = Globals {
        options: vec![option("color", OptionShape::Scalar(color_ref))],
        flags: vec![flag(
            "case-sensitive",
            FlagShape::BoolFlag(BoolFlagShape {
                default: false,
                negatable: false,
            }),
        )],
    };
    root.subcommands = vec![CommandIndex(1)];
    root.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![positional("pattern", SchemaType::string())],
            tail: Some(TailPositional {
                name: "files".to_string(),
                doc: doc("files"),
                value_name: None,
                item_type: SchemaType::string(),
                min: 0,
                max: None,
                separator: None,
                verbatim: false,
                accepts_stdio: false,
            }),
        },
        options: vec![option(
            "max-count",
            OptionShape::OptionalScalar(SchemaType::u32()),
        )],
        flags: vec![flag("verbosity", FlagShape::CountFlag(None))],
        result: Some(ToolResultSpec {
            type_: SchemaType::list(SchemaType::string()),
            doc: doc("matches"),
            formatters: vec![Formatter {
                name: "json".to_string(),
                doc: doc("json"),
            }],
            default_formatter: "json".to_string(),
        }),
        errors: vec![
            ErrorCase {
                name: "bad-pattern".to_string(),
                doc: doc("bad pattern"),
                kind: ErrorKind::UsageError,
                exit_code: 2,
                payload: Some(SchemaType::string()),
            },
            ErrorCase {
                name: "io".to_string(),
                doc: doc("io"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: None,
            },
        ],
        ..tool_body()
    });
    let mut replace = command_node("replace");
    replace.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![
                positional("pattern", SchemaType::string()),
                positional("replacement", SchemaType::string()),
            ],
            tail: None,
        },
        stdout: Some(StreamSpec {
            doc: doc("stdout"),
            mime: vec![],
            required: true,
        }),
        ..tool_body()
    });

    Tool {
        version: "1".to_string(),
        commands: CommandTree {
            nodes: vec![root, replace],
        },
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: color_type,
                name: None,
                body: SchemaType::r#enum(vec![
                    "never".to_string(),
                    "always".to_string(),
                    "auto".to_string(),
                ]),
            }],
            root: SchemaType::record(vec![]),
        },
    }
}

struct GeneratedToolPackage {
    pub dir: TempDir,
    pub package_name: String,
}

impl GeneratedToolPackage {
    fn new(tool: Tool) -> Self {
        let package_name = tool_bridge_client_directory_name(&tool.commands.nodes[0].name);
        let dir = TempDir::new().unwrap();
        let package_dir = Utf8Path::from_path(dir.path()).unwrap().join(&package_name);
        let mut generator = ScalaToolBridgeGenerator::new(tool, &package_dir, true).unwrap();
        generator.generate().unwrap();
        Self { dir, package_name }
    }

    fn package_dir(&self) -> camino::Utf8PathBuf {
        Utf8Path::from_path(self.dir.path())
            .unwrap()
            .join(&self.package_name)
    }
}

#[test_dep(scope = PerWorker, tagged_as = "scala_single_agent")]
fn scala_single_agent() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

/// Generates a single agent bridge and cross-compiles it with sbt against
/// Scala 2.13 and Scala 3.
#[test]
fn single_agent_cross_compiles(#[tagged_as("scala_single_agent")] pkg: &GeneratedPackage) {
    cross_compile(pkg.package_dir().as_path());
}

/// The generated project lays out the static runtime and the per-agent client
/// in the expected packages.
#[test]
fn generated_project_layout_is_correct() {
    let pkg = GeneratedPackage::new(agent(
        "CounterAgent",
        "scala",
        vec![field("name", SchemaType::string())],
        vec![method("increment", vec![], Some(SchemaType::f64()))],
        vec![],
        AgentMode::Durable,
    ));
    let dir = pkg.package_dir();

    assert!(dir.join("build.sbt").exists(), "build.sbt is missing");
    assert!(
        dir.join("project/build.properties").exists(),
        "project/build.properties is missing"
    );

    let runtime_root = dir.join("src/main/scala/golem/bridge/runtime");
    for runtime_file in [
        "SchemaValue.scala",
        "SchemaValueCodec.scala",
        "Configuration.scala",
        "GolemServer.scala",
        "BridgeException.scala",
        "AgentId.scala",
        "json/Json.scala",
    ] {
        assert!(
            runtime_root.join(runtime_file).exists(),
            "runtime file {runtime_file} is missing"
        );
    }

    let client_path =
        dir.join("src/main/scala/golem/bridge/client/counter_agent/CounterAgentClient.scala");
    assert!(client_path.exists(), "generated client object is missing");
    let client_source = std::fs::read_to_string(&client_path).unwrap();
    assert!(client_source.contains("package golem.bridge.client.counter_agent"));
    assert!(client_source.contains("object CounterAgentClient"));
    assert!(client_source.contains("\"CounterAgent\""));

    let build_sbt = std::fs::read_to_string(dir.join("build.sbt")).unwrap();
    assert!(build_sbt.contains("crossScalaVersions"));
    assert!(build_sbt.contains("counter-agent-client"));
}

/// Guest Scala bridge projects skip embedding the external REST runtime and
/// depend on the Scala SDK's guest runtime instead, so the generated client must
/// not refer to the external-only `golem.bridge.runtime` package.
#[test]
fn guest_wasm_rpc_does_not_emit_external_rest_runtime_references() {
    let pkg = GeneratedPackage::new_with_mode(
        agent(
            "CounterAgent",
            "scala",
            vec![field("name", SchemaType::string())],
            vec![method("increment", vec![], Some(SchemaType::f64()))],
            vec![],
            AgentMode::Durable,
        ),
        ScalaBridgeMode::GuestWasmRpc,
    );
    let dir = pkg.package_dir();

    assert!(
        !dir.join("src/main/scala/golem/bridge/runtime").exists(),
        "guest bridge must not embed the external REST runtime"
    );

    let client_path =
        dir.join("src/main/scala/golem/bridge/client/counter_agent/CounterAgentClient.scala");
    let client_source = std::fs::read_to_string(&client_path).unwrap();
    assert!(
        !client_source.contains("golem.bridge.runtime"),
        "guest bridge client references the external REST runtime package:\n{client_source}"
    );

    let build_sbt = std::fs::read_to_string(dir.join("build.sbt")).unwrap();
    assert!(build_sbt.contains("golem-scala-core"));
    assert!(build_sbt.contains("golem-scala-model"));
    assert!(build_sbt.contains("ScalaJSPlugin"));
}

/// Guest agent bridges expose the Scala SDK RPC surface: constructors resolve
/// remote agents through `RemoteAgentClient`, methods invoke Wasm RPC via
/// `asyncInvokeAndAwait`, and the generated Scala.js build is ready for a real
/// compile once the in-tree Scala SDK has been published locally.
#[test]
fn guest_agent_client_surface_targets_scala_sdk_rpc() {
    let pkg = GeneratedPackage::new_with_mode(
        agent(
            "CounterAgent",
            "scala",
            vec![field("name", SchemaType::string())],
            vec![method(
                "increment",
                vec![field("amount", SchemaType::u32())],
                Some(SchemaType::u32()),
            )],
            vec![],
            AgentMode::Durable,
        ),
        ScalaBridgeMode::GuestWasmRpc,
    );
    let dir = pkg.package_dir();
    let client_path =
        dir.join("src/main/scala/golem/bridge/client/counter_agent/CounterAgentClient.scala");
    let client_source = std::fs::read_to_string(&client_path).unwrap();

    assert!(client_source.contains("object CounterAgentClient"));
    assert!(client_source.contains("trait CounterAgentRemote"));
    assert!(client_source.contains("def get(name: _root_.scala.Predef.String)"));
    assert!(client_source.contains("_root_.scala.Either"));
    assert!(client_source.contains("CounterAgentRemote"));
    assert!(client_source.contains("_root_.golem.runtime.rpc.RemoteAgentClient.resolve"));
    assert!(client_source.contains("resolved.asyncInvokeAndAwait"));
    assert!(client_source.contains("resolved.cancelableAsyncInvokeAndAwait"));
    assert!(client_source.contains("_root_.golem.runtime.rpc.CancellationToken"));
    assert!(client_source.contains("_root_.golem.runtime.rpc.SchemaRpcCodec.encodeValue"));
    assert!(client_source.contains("_root_.golem.runtime.rpc.SchemaRpcCodec.decodeValue"));
    assert!(!client_source.contains("Bridge.createAgent"));
    assert!(!client_source.contains("golem.bridge.runtime"));

    let build_sbt = std::fs::read_to_string(dir.join("build.sbt")).unwrap();
    assert!(build_sbt.contains("enablePlugins(ScalaJSPlugin)"));
    assert!(build_sbt.contains("scalaJSUseMainModuleInitializer := false"));
    assert!(build_sbt.contains("ModuleKind.ESModule"));
    assert!(dir.join("project/plugins.sbt").exists());

    cross_compile_guest_if_enabled(dir.as_path());
}

/// Ephemeral guest agents omit the parameter-addressable `get` constructor, but
/// still expose phantom constructors. Constructor errors are surfaced as
/// `Either[String, …]`, not thrown futures, because guest RPC resolution is
/// synchronous at the SDK boundary.
#[test]
fn guest_ephemeral_agent_omits_get_and_returns_either_constructors() {
    let pkg = GeneratedPackage::new_with_mode(
        agent(
            "EphemeralAgent",
            "scala",
            vec![field("name", SchemaType::string())],
            vec![method("ping", vec![], None)],
            vec![],
            AgentMode::Ephemeral,
        ),
        ScalaBridgeMode::GuestWasmRpc,
    );
    let dir = pkg.package_dir();
    let client_source = std::fs::read_to_string(
        dir.join("src/main/scala/golem/bridge/client/ephemeral_agent/EphemeralAgentClient.scala"),
    )
    .unwrap();

    assert!(
        !client_source.contains("def get("),
        "ephemeral guest agent must not expose get:\n{client_source}"
    );
    assert!(client_source.contains("def getPhantom(name: _root_.scala.Predef.String"));
    assert!(client_source.contains("phantom: _root_.golem.Uuid"));
    assert!(client_source.contains("def newPhantom(name: _root_.scala.Predef.String)"));
    assert!(client_source.contains("_root_.scala.Either"));
    assert!(client_source.contains("EphemeralAgentRemote"));

    cross_compile_guest_if_enabled(dir.as_path());
}

/// Generates a bridge for an agent with rich named types (record, enum,
/// variant) and checks the emitted Scala definitions.
#[test]
fn generates_named_type_definitions() {
    let pkg = GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone());
    let dir = pkg.package_dir();
    let client = std::fs::read_to_string(
        dir.join("src/main/scala/golem/bridge/client/agent1/Agent1Client.scala"),
    )
    .unwrap();

    // enum Color -> sealed trait + case objects (cases extend the fully
    // qualified trait so a nested case can never shadow it).
    assert!(client.contains(
        "sealed trait Color extends _root_.scala.Product with _root_.scala.Serializable"
    ));
    assert!(client.contains("case object Red extends _root_.golem.bridge.client.agent1.Color"));
    assert!(client.contains("case object Green extends _root_.golem.bridge.client.agent1.Color"));
    assert!(client.contains("case object Blue extends _root_.golem.bridge.client.agent1.Color"));

    // record Person -> case class with mapped, fully qualified field types
    assert!(client.contains("final case class Person("));
    assert!(client.contains("firstName: _root_.scala.Predef.String"));
    assert!(client.contains("age: _root_.scala.Option[_root_.golem.bridge.runtime.UInt]"));
    assert!(client.contains("eyeColor: _root_.golem.bridge.client.agent1.Color"));

    // variant Location -> sealed trait + case classes / objects
    assert!(client.contains("sealed trait Location"));
    assert!(client.contains(
        "final case class Home(value: _root_.scala.Predef.String) extends _root_.golem.bridge.client.agent1.Location"
    ));
    assert!(
        client.contains("case object Unknown extends _root_.golem.bridge.client.agent1.Location")
    );
}

/// Each generated named composite type gets an `encode<Name>` / `decode<Name>`
/// codec in the `Codecs` object, structurally encoding to and decoding from the
/// runtime `SchemaValue` (Step 5).
#[test]
fn generates_codecs_for_named_types() {
    let pkg = GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone());
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/agent1/Agent1Client.scala"),
    )
    .unwrap();

    assert!(client.contains("object Codecs {"));

    // enum Color: positional case index encode / decode.
    assert!(client.contains(
        "def encodeColor(value: _root_.golem.bridge.client.agent1.Color): _root_.golem.bridge.runtime.SchemaValue"
    ));
    assert!(client.contains(
        "case _root_.golem.bridge.client.agent1.Color.Red => _root_.golem.bridge.runtime.SchemaValue.EnumValue(0)"
    ));
    assert!(client.contains(
        "def decodeColor(value: _root_.golem.bridge.runtime.SchemaValue): _root_.golem.bridge.client.agent1.Color"
    ));
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.enumCase(value)"));

    // record Person: positional record fields, with the nested enum field
    // delegating to its fully qualified codec (so a local `Codecs` cannot shadow
    // it) and the optional u32 going through the runtime accessors. Encoding the
    // unsigned wrapper is routed through the range-validating runtime helper.
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.recordFields(value)"));
    assert!(
        client.contains(
            "val f3 = _root_.golem.bridge.client.agent1.Codecs.encodeColor(value.eyeColor)"
        )
    );
    assert!(
        client.contains("val f3 = _root_.golem.bridge.client.agent1.Codecs.decodeColor(fields(3))")
    );
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.encodeUInt(e0)"));
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.asUInt(e0)"));

    // variant Location: case index + payload encode / decode.
    assert!(
        client.contains(
            "_root_.golem.bridge.runtime.SchemaValue.VariantValue(0, _root_.scala.Some(p))"
        )
    );
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.variantCase(value)"));
    assert!(client.contains("_root_.golem.bridge.client.agent1.Location.Unknown"));
}

/// Fixed-list codecs validate the declared length on encode and decode, and
/// decode goes through the strict accessor that rejects an ordinary `list` node
/// (so a length-1 list can never be silently accepted as a fixed-list).
#[test]
fn fixed_list_codec_is_length_checked_and_strict() {
    // A record carrying a fixed-list field becomes a named composite, so its
    // codec is emitted in the `Codecs` object with the fixed-list logic inline.
    let pkg = GeneratedPackage::new(agent(
        "MatrixAgent",
        "scala",
        vec![field(
            "row",
            SchemaType::record(vec![named_field(
                "values",
                SchemaType::fixed_list(SchemaType::u32(), 3),
            )]),
        )],
        vec![method("noop", vec![], None)],
        vec![],
        AgentMode::Durable,
    ));
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/matrix_agent/MatrixAgentClient.scala"),
    )
    .unwrap();

    // Encoding builds a fixed-list node and validates the declared length.
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValue.FixedListValue"));
    assert!(client.contains("Expected fixed-list of length 3"));
    // Decoding uses the strict accessor that only accepts a fixed-list node
    // (never an ordinary list) and re-checks the declared length.
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.fixedListElements"));
}

/// The generated client object exposes the full surface: configuration
/// helpers, per-method `apply`/`trigger`/`scheduleAt` wrappers, a `Remote`
/// trait, and mode-aware constructors returning `Future[<Agent>Remote]`.
#[test]
fn generates_client_surface() {
    let pkg = GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone());
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/agent1/Agent1Client.scala"),
    )
    .unwrap();

    // Configuration helpers delegate to the shared runtime cell.
    assert!(client.contains("object Agent1Client {"));
    assert!(client.contains(
        "def configure(server: _root_.golem.bridge.runtime.GolemServer, appName: _root_.scala.Predef.String, envName: _root_.scala.Predef.String, executionContext: _root_.scala.concurrent.ExecutionContext = _root_.scala.concurrent.ExecutionContext.global)"
    ));
    assert!(client.contains(
        "def getConfiguration: _root_.golem.bridge.runtime.Configuration = _root_.golem.bridge.runtime.Configuration.get"
    ));

    // Per-method wrapper class with apply (await) / trigger / scheduleAt.
    assert!(client
        .contains("final class F1RemoteMethod private[Agent1Client] (resolved: _root_.golem.bridge.runtime.ResolvedAgent)"));
    assert!(client.contains(
        "def apply(): _root_.scala.concurrent.Future[_root_.golem.bridge.client.agent1.Location]"
    ));
    assert!(client.contains("methodParameters(), \"await\", _root_.scala.None"));
    assert!(client.contains("def trigger(): _root_.scala.concurrent.Future[_root_.scala.Unit]"));
    assert!(client.contains("methodParameters(), \"schedule\", _root_.scala.None).map(_ => ())"));
    assert!(client.contains(
        "def scheduleAt(when: _root_.golem.bridge.runtime.Datetime): _root_.scala.concurrent.Future[_root_.scala.Unit]"
    ));
    assert!(client.contains("\"schedule\", _root_.scala.Some(when.toIsoString)"));
    // The await result is decoded through the named-type codec.
    assert!(client.contains("_root_.golem.bridge.client.agent1.Codecs.decodeLocation(__value)"));

    // Remote trait + factory.
    assert!(client.contains("trait Agent1Remote {"));
    assert!(client.contains("def agentId: _root_.golem.bridge.runtime.AgentId"));
    assert!(client.contains("val f1: F1RemoteMethod"));
    assert!(client.contains(
        "private def bindRemote(resolved: _root_.golem.bridge.runtime.ResolvedAgent): Agent1Remote"
    ));

    // Durable agent: get / getPhantom / newPhantom, all returning the remote.
    assert!(client.contains("bindRemote(_root_.golem.bridge.runtime.ResolvedAgent("));
    assert!(client.contains("def get(person: _root_.golem.bridge.client.agent1.Person"));
    assert!(client.contains(
        "phantom: _root_.golem.bridge.runtime.Uuid): _root_.scala.concurrent.Future[Agent1Remote]"
    ));
    assert!(client.contains("def newPhantom("));
    assert!(client.contains("_root_.golem.bridge.runtime.Uuid.random()"));
    assert!(client.contains(
        "_root_.golem.bridge.runtime.Bridge.createAgent(configuration, agentTypeName, parameters, phantomId,"
    ));
    // Constructor parameters are packed through the named-type codec.
    assert!(client.contains("_root_.golem.bridge.client.agent1.Codecs.encodePerson(person)"));
}

/// An ephemeral agent omits the parameter-addressable `get` constructor but
/// still exposes `getPhantom` and `newPhantom`.
#[test]
fn ephemeral_agent_omits_get_constructor() {
    let pkg = GeneratedPackage::new(agent(
        "EphemeralAgent",
        "scala",
        vec![field("name", SchemaType::string())],
        vec![method("ping", vec![], None)],
        vec![],
        AgentMode::Ephemeral,
    ));
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/ephemeral_agent/EphemeralAgentClient.scala"),
    )
    .unwrap();

    assert!(
        !client.contains("def get("),
        "ephemeral agent must not expose get"
    );
    assert!(client.contains("def getPhantom("));
    assert!(client.contains("def newPhantom("));
    // A Unit-returning method's apply yields Future[Unit].
    assert!(client.contains("def apply(): _root_.scala.concurrent.Future[_root_.scala.Unit]"));

    cross_compile(pkg.package_dir().as_path());
}

/// Generated codecs reject malformed wire shapes on the negative path: a
/// payload-less variant case carrying a payload, a unit-`result` arm carrying a
/// payload, and an invalid (surrogate) `char` on encode. (The runtime-only
/// strictness — option object shape, UUID half ranges, strict case-index
/// parsing — lives in the hand-written runtime sources, which the sbt
/// cross-compile only type-checks rather than executes.)
#[test]
fn codecs_reject_malformed_wire_shapes() {
    let pkg = GeneratedPackage::new(agent(
        "GuardAgent",
        "scala",
        vec![field(
            "data",
            SchemaType::record(vec![
                named_field("initial", SchemaType::char()),
                named_field(
                    "status",
                    SchemaType::result(ResultSpec {
                        ok: None,
                        err: Some(Box::new(SchemaType::string())),
                    }),
                ),
                named_field(
                    "place",
                    SchemaType::variant(vec![
                        variant_case("here", Some(SchemaType::string())),
                        variant_case("nowhere", None),
                    ]),
                ),
            ]),
        )],
        vec![method("noop", vec![], None)],
        vec![],
        AgentMode::Durable,
    ));
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/guard_agent/GuardAgentClient.scala"),
    )
    .unwrap();

    // char encode routes through the surrogate-rejecting runtime helper.
    assert!(client.contains("_root_.golem.bridge.runtime.SchemaValueCodec.encodeChar"));
    // unit-`result` ok arm rejects an unexpected payload.
    assert!(client.contains("Unexpected payload for unit result ok"));
    // payload-less variant case rejects an unexpected payload.
    assert!(client.contains("if (payload.nonEmpty) throw"));
    assert!(client.contains("Unexpected payload for payload-less variant case"));

    // The generated strictness paths (char encode, unit-result and variant
    // payload checks) must also compile on both Scala versions.
    cross_compile(pkg.package_dir().as_path());
}

/// The bridge for an agent with rich named types cross-compiles with sbt.
#[test]
fn multi_agent_named_types_cross_compiles() {
    let pkg = GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone());
    cross_compile(pkg.package_dir().as_path());
}

/// Method names that are Scala keywords, and constructor/method parameters that
/// collide with reserved internal identifiers (helper defs, locals, inherited
/// trait members, the synthetic `when` parameter) or are Scala keywords, are
/// disambiguated so the generated client still compiles on both Scala versions.
#[test]
fn client_surface_handles_reserved_and_keyword_names() {
    let pkg = GeneratedPackage::new(agent(
        "HygieneAgent",
        // Same-language ("scala") so identifiers are only keyword-escaped, which
        // exercises the reserved-name disambiguation on its own.
        "scala",
        // Constructor parameters collide with internal constructor locals /
        // inherited members and include a keyword.
        vec![
            field("configuration", SchemaType::string()),
            field("agentTypeName", SchemaType::string()),
            field("phantom", SchemaType::string()),
            field("type", SchemaType::string()),
        ],
        vec![
            // A keyword-named method whose parameters collide with method
            // internals (`when` synthetic param, `ec`/`resolved` locals, `f0`
            // record local) and include a keyword.
            method(
                "match",
                vec![
                    field("when", SchemaType::string()),
                    field("ec", SchemaType::string()),
                    field("resolved", SchemaType::string()),
                    field("f0", SchemaType::string()),
                    field("type", SchemaType::string()),
                ],
                None,
            ),
            // A method whose name collides with a universal `Any`/`AnyRef`
            // member, with parameters that collide with the structural
            // encoders' depth-0 temp-local names (`e0` for a list element, `p`
            // for a result payload, `t0` for a tuple value).
            method(
                "toString",
                vec![
                    field("e0", SchemaType::list(SchemaType::string())),
                    field("p", SchemaType::string()),
                    field("t0", SchemaType::string()),
                ],
                None,
            ),
        ],
        vec![],
        AgentMode::Durable,
    ));
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/hygiene_agent/HygieneAgentClient.scala"),
    )
    .unwrap();

    // Keyword method name yields a valid UpperCamel class name and a
    // backtick-escaped trait member.
    assert!(client.contains("final class MatchRemoteMethod private[HygieneAgentClient]"));
    assert!(client.contains("val `match`: MatchRemoteMethod"));

    // Method parameters colliding with internal names are renamed; the
    // synthetic `when` parameter of scheduleAt keeps its name.
    assert!(client.contains("when_2: _root_.scala.Predef.String"));
    assert!(client.contains("ec_2: _root_.scala.Predef.String"));
    assert!(client.contains("resolved_2: _root_.scala.Predef.String"));
    assert!(client.contains("f0_2: _root_.scala.Predef.String"));
    assert!(client.contains("when: _root_.golem.bridge.runtime.Datetime"));

    // Constructor parameters colliding with internal names are renamed too.
    assert!(client.contains("configuration_2: _root_.scala.Predef.String"));
    assert!(client.contains("agentTypeName_2: _root_.scala.Predef.String"));
    assert!(client.contains("phantom_2: _root_.scala.Predef.String"));

    // Keyword parameters are backtick-escaped in both contexts.
    assert!(client.contains("`type`: _root_.scala.Predef.String"));

    // A method named like a universal member yields a valid class name and a
    // renamed trait member.
    assert!(client.contains("final class ToStringRemoteMethod private[HygieneAgentClient]"));
    assert!(client.contains("val toString_2: ToStringRemoteMethod"));

    // Parameters colliding with structural-encoder depth-0 temps are renamed.
    assert!(client.contains("e0_2: _root_.scala.collection.immutable.List"));
    assert!(client.contains("p_2: _root_.scala.Predef.String"));
    assert!(client.contains("t0_2: _root_.scala.Predef.String"));

    cross_compile(pkg.package_dir().as_path());
}

/// Generated named-type members (record/flag fields, variant/enum/union cases)
/// whose names would clash with a synthesized case-class member or an inherited
/// `Product`/`Object`/`Any` member are disambiguated, while members that are
/// safe (e.g. `copy`) are left untouched, so the generated types compile on
/// both Scala versions.
#[test]
fn named_type_members_avoid_reserved_member_names() {
    let rec = def(
        "rec",
        SchemaType::record(vec![
            named_field("toString", SchemaType::string()),
            named_field("productPrefix", SchemaType::string()),
            // `productArity` clashes only because its inherited result type
            // (`Int`) differs from the field type, so it must be reserved for
            // arbitrary field types.
            named_field("productArity", SchemaType::string()),
            // `##` is a no-arg universal member; a backtick-escaped field of
            // that name must be renamed too.
            named_field("##", SchemaType::string()),
            // `copy` is a valid case-class field name (it suppresses the
            // synthesized copy), so it must NOT be renamed.
            named_field("copy", SchemaType::string()),
            named_field("data", SchemaType::string()),
        ]),
    );
    let choice = def(
        "choice",
        SchemaType::variant(vec![
            variant_case("toString", Some(SchemaType::string())),
            variant_case("other", None),
        ]),
    );
    let shade = def(
        "shade",
        SchemaType::r#enum(vec!["toString".into(), "other".into()]),
    );

    let pkg = GeneratedPackage::new(agent(
        "MembersAgent",
        // Same-language so member names keep their original casing and actually
        // collide with the reserved members.
        "scala",
        vec![field("rec", ref_to("rec"))],
        vec![method(
            "go",
            vec![
                field("choice", ref_to("choice")),
                field("shade", ref_to("shade")),
            ],
            None,
        )],
        vec![rec, choice, shade],
        AgentMode::Durable,
    ));
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/members_agent/MembersAgentClient.scala"),
    )
    .unwrap();

    // Record fields colliding with case-class/Product/Object members are
    // renamed; a safe field name is preserved.
    assert!(client.contains("final case class Rec("));
    assert!(client.contains("toString_2: _root_.scala.Predef.String"));
    assert!(client.contains("productPrefix_2: _root_.scala.Predef.String"));
    assert!(client.contains("productArity_2: _root_.scala.Predef.String"));
    assert!(client.contains("`##_2`: _root_.scala.Predef.String"));
    assert!(client.contains("copy: _root_.scala.Predef.String"));
    assert!(client.contains("data: _root_.scala.Predef.String"));

    // Variant and enum case members colliding with inherited members are
    // renamed too (they become members of the companion object).
    assert!(client
        .contains("final case class toString_2(value: _root_.scala.Predef.String) extends _root_.golem.bridge.client.members_agent.Choice"));
    assert!(
        client.contains(
            "case object toString_2 extends _root_.golem.bridge.client.members_agent.Shade"
        )
    );

    cross_compile(pkg.package_dir().as_path());
}

/// Identifier uniqueness is computed on the semantic Scala symbol (backticks
/// stripped), so a backtick-escaped name collides with the same plain reserved
/// name, and a plain/backticked pair among the inputs collide with each other.
#[test]
fn reserved_idents_collide_modulo_backticks() {
    assert_eq!(
        unique_idents_with_reserved(vec!["`ec`".to_string()], &["ec"]),
        vec!["`ec_2`".to_string()]
    );
    assert_eq!(
        unique_idents_with_reserved(vec!["foo".to_string(), "`foo`".to_string()], &[]),
        vec!["foo".to_string(), "`foo_2`".to_string()]
    );
}

// --- Scala identifier escaping (Step 2) ------------------------------------

#[test]
fn valid_scala_idents_are_recognized() {
    assert!(is_valid_scala_ident("foo"));
    assert!(is_valid_scala_ident("_foo"));
    assert!(is_valid_scala_ident("foo123"));
    assert!(is_valid_scala_ident("fooBar"));

    assert!(!is_valid_scala_ident(""));
    assert!(!is_valid_scala_ident("1foo"));
    assert!(!is_valid_scala_ident("foo-bar"));
    assert!(!is_valid_scala_ident("foo bar"));
}

#[test]
fn plain_idents_are_left_untouched() {
    assert_eq!(escape_scala_ident("foo"), "foo");
    assert_eq!(escape_scala_ident("fooBar"), "fooBar");
    assert_eq!(escape_scala_ident("_private"), "_private");
}

#[test]
fn keywords_are_backtick_escaped() {
    assert!(is_scala_keyword("type"));
    assert!(is_scala_keyword("match"));
    assert!(is_scala_keyword("enum"));
    assert!(is_scala_keyword("given"));
    // Scala 3 soft keywords are escaped defensively.
    assert!(is_scala_keyword("using"));
    assert!(is_scala_keyword("inline"));
    assert!(!is_scala_keyword("foo"));

    assert_eq!(escape_scala_ident("type"), "`type`");
    assert_eq!(escape_scala_ident("match"), "`match`");
    assert_eq!(escape_scala_ident("enum"), "`enum`");
    assert_eq!(escape_scala_ident("using"), "`using`");
}

#[test]
fn illegal_characters_are_backtick_escaped() {
    assert_eq!(escape_scala_ident("foo-bar"), "`foo-bar`");
    assert_eq!(escape_scala_ident("1foo"), "`1foo`");
    assert_eq!(escape_scala_ident(""), "`_`");
}

#[test]
fn bare_underscore_is_not_a_plain_ident_and_is_escaped() {
    assert!(!is_valid_scala_ident("_"));
    assert_eq!(escape_scala_ident("_"), "`_`");
    // A double underscore is a legal plain identifier, though.
    assert!(is_valid_scala_ident("__"));
}

#[test]
fn backticks_in_input_cannot_break_out_of_escaping() {
    assert_eq!(escape_scala_ident("a`b`c"), "`abc`");
}

#[test]
fn term_ident_casing_respects_same_language() {
    // Cross-language: kebab-case WIT names become lowerCamelCase.
    assert_eq!(to_scala_term_ident("first-name", false), "firstName");
    // Same-language: already Scala-cased, only escaping is applied.
    assert_eq!(to_scala_term_ident("firstName", true), "firstName");
    assert_eq!(to_scala_term_ident("type", true), "`type`");
    assert_eq!(to_scala_term_ident("read-only", false), "readOnly");
}

// --- ScalaTypeName casing & remapping (Step 2) -----------------------------

#[test]
fn type_names_are_upper_camel_case() {
    let name = ScalaTypeName::from_owner_and_name(None::<&str>, "all-primitives", false);
    assert_eq!(name.to_string(), "AllPrimitives");

    let owned = ScalaTypeName::from_owner_and_name(Some("my-mod"), "the-type", false);
    assert_eq!(owned.to_string(), "MyModTheType");

    let segmented = ScalaTypeName::from_segments(["foo", "bar-baz"], false);
    assert_eq!(segmented.to_string(), "FooBarBaz");
}

#[test]
fn uuid_ref_is_remapped() {
    let uuid_ref = SchemaType::ref_to(TypeId::new("uuid.Uuid"));
    let mapped = ScalaTypeName::from_schema_type(&uuid_ref);
    assert_eq!(mapped, Some(ScalaTypeName::Remapped(RemappedType::Uuid)));
    assert_eq!(mapped.unwrap().to_string(), "Uuid");

    let other_ref = SchemaType::ref_to(TypeId::new("my.OtherType"));
    assert_eq!(ScalaTypeName::from_schema_type(&other_ref), None);

    assert_eq!(ScalaTypeName::from_schema_type(&SchemaType::string()), None);
}

// --- UUID remap wired into the TypeNaming walker (Step 3) -------------------

/// The `uuid.Uuid` builtin record is remapped onto the runtime `Uuid` type: no
/// structural record definition is generated for it, while normal nominal types
/// in the same agent still get derived names.
#[test]
fn uuid_ref_is_remapped_through_the_walker() {
    let uuid_def = def(
        "uuid.Uuid",
        SchemaType::record(vec![
            named_field("high-bits", SchemaType::u64()),
            named_field("low-bits", SchemaType::u64()),
        ]),
    );

    let agent_type = agent(
        "IdAgent",
        "scala",
        vec![
            field("id", ref_to("uuid.Uuid")),
            field(
                "color",
                SchemaType::r#enum(vec!["red".into(), "green".into(), "blue".into()]),
            ),
        ],
        vec![method("get-id", vec![], Some(ref_to("uuid.Uuid")))],
        vec![uuid_def],
        AgentMode::Durable,
    );

    let naming = TypeNaming::<ScalaTypeName>::new(&agent_type, false).unwrap();
    let names: Vec<ScalaTypeName> = naming.types().map(|(_, n)| n.clone()).collect();

    // The uuid ref (used by both the constructor field and the method result,
    // structurally identical) collapses to a single remapped entry.
    let uuid_count = names
        .iter()
        .filter(|n| matches!(n, ScalaTypeName::Remapped(RemappedType::Uuid)))
        .count();
    assert_eq!(uuid_count, 1, "expected exactly one remapped uuid entry");

    // No structural definition was generated for the uuid record body, and
    // nothing derived collides with the runtime `Uuid` name.
    assert!(
        !names
            .iter()
            .any(|n| n.to_string() == "Uuid" && n.as_remapped().is_none()),
        "no derived type should be named Uuid: {names:?}"
    );

    // Normal nominal naming still works: the inline enum got a derived name.
    assert!(
        names.iter().any(|n| matches!(n, ScalaTypeName::Derived(_))),
        "expected a derived name for the inline enum: {names:?}"
    );
}

/// A multimodal method input/output (`list<variant<… Role::Multimodal>>`) is
/// surfaced as a generated `sealed trait Multimodal<N>` with one case per
/// modality and a `List[Multimodal<N>]` parameter / return type, decoded
/// through the generated list codec. Structurally identical modality sets used
/// by multiple methods collapse to a single generated type, and the result
/// cross-compiles on both Scala versions.
#[test]
fn multimodal_input_and_output_cross_compiles() {
    let parts = || {
        vec![
            variant_case("text", Some(SchemaType::string())),
            variant_case("image-url", Some(SchemaType::string())),
        ]
    };
    let pkg = GeneratedPackage::new(agent(
        "MediaAgent",
        // Non-same-language so modality case names normalize to UpperCamelCase.
        "",
        vec![field("name", SchemaType::string())],
        vec![
            // Multimodal input, scalar output.
            method(
                "describe",
                vec![field("content", multimodal(parts()))],
                Some(SchemaType::string()),
            ),
            // Scalar input, multimodal output reusing the same modality set.
            method(
                "render",
                vec![field("prompt", SchemaType::string())],
                Some(multimodal(parts())),
            ),
        ],
        vec![],
        AgentMode::Durable,
    ));
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/media_agent/MediaAgentClient.scala"),
    )
    .unwrap();

    // One sealed trait with a case per modality (UpperCamelCase, payload typed).
    assert!(client.contains(
        "sealed trait Multimodal0 extends _root_.scala.Product with _root_.scala.Serializable"
    ));
    assert!(client.contains("object Multimodal0 {"));
    assert!(client.contains(
        "final case class Text(value: _root_.scala.Predef.String) extends _root_.golem.bridge.client.media_agent.Multimodal0"
    ));
    assert!(client.contains(
        "final case class ImageUrl(value: _root_.scala.Predef.String) extends _root_.golem.bridge.client.media_agent.Multimodal0"
    ));

    // The structurally identical modality set used by both methods collapses to
    // a single generated type (no `Multimodal1`).
    assert!(
        !client.contains("Multimodal1"),
        "identical modality sets must collapse to one generated type"
    );

    // Input multimodal is surfaced as a single `List[Multimodal0]` parameter,
    // packed through the generated list codec.
    assert!(client.contains(
        "content: _root_.scala.collection.immutable.List[_root_.golem.bridge.client.media_agent.Multimodal0]"
    ));
    assert!(
        client.contains(
            "_root_.golem.bridge.client.media_agent.Codecs.encodeMultimodal0List(content)"
        )
    );

    // Output multimodal is decoded through the generated list codec.
    assert!(client.contains(
        "def apply(prompt: _root_.scala.Predef.String): _root_.scala.concurrent.Future[_root_.scala.collection.immutable.List[_root_.golem.bridge.client.media_agent.Multimodal0]]"
    ));
    assert!(
        client.contains(
            "_root_.golem.bridge.client.media_agent.Codecs.decodeMultimodal0List(__value)"
        )
    );

    // Element codecs encode each modality to a positional variant case.
    assert!(client.contains(
        "def encodeMultimodal0(value: _root_.golem.bridge.client.media_agent.Multimodal0): _root_.golem.bridge.runtime.SchemaValue"
    ));
    assert!(
        client.contains(
            "_root_.golem.bridge.runtime.SchemaValue.VariantValue(1, _root_.scala.Some(p))"
        )
    );

    cross_compile(pkg.package_dir().as_path());
}

/// A durable agent that declares local config overrides gets the
/// `getWithConfig` / `getPhantomWithConfig` / `newPhantomWithConfig`
/// constructor variants (mirroring the Scala SDK's RPC clients), each taking an
/// `Option[T] = None` per declared override and building the
/// `List[AgentConfigEntry]` from the supplied values. The plain constructors
/// pass an empty config list, and the result cross-compiles on both Scala
/// versions.
#[test]
fn local_config_overrides_cross_compiles() {
    let pkg = GeneratedPackage::new({
        let mut at = agent(
            "ConfigAgent",
            "scala",
            vec![field("name", SchemaType::string())],
            vec![method("ping", vec![], None)],
            vec![],
            AgentMode::Durable,
        );
        at.config = vec![
            local_config(vec!["db", "host"], SchemaType::string()),
            local_config(vec!["max-retries"], SchemaType::u32()),
        ];
        at
    });
    let client = std::fs::read_to_string(
        pkg.package_dir()
            .join("src/main/scala/golem/bridge/client/config_agent/ConfigAgentClient.scala"),
    )
    .unwrap();

    // Config override parameters: one `Option[T] = None` per declared override,
    // camelCased from the config path, after the constructor parameters.
    assert!(client.contains(
        "def getWithConfig(name: _root_.scala.Predef.String, dbHost: _root_.scala.Option[_root_.scala.Predef.String] = _root_.scala.None, maxRetries: _root_.scala.Option[_root_.golem.bridge.runtime.UInt] = _root_.scala.None): _root_.scala.concurrent.Future[ConfigAgentRemote]"
    ));
    assert!(client.contains(
        "def getPhantomWithConfig(name: _root_.scala.Predef.String, phantom: _root_.golem.bridge.runtime.Uuid, dbHost: _root_.scala.Option[_root_.scala.Predef.String] = _root_.scala.None, maxRetries: _root_.scala.Option[_root_.golem.bridge.runtime.UInt] = _root_.scala.None): _root_.scala.concurrent.Future[ConfigAgentRemote]"
    ));
    assert!(client.contains(
        "def newPhantomWithConfig(name: _root_.scala.Predef.String, dbHost: _root_.scala.Option[_root_.scala.Predef.String] = _root_.scala.None, maxRetries: _root_.scala.Option[_root_.golem.bridge.runtime.UInt] = _root_.scala.None): _root_.scala.concurrent.Future[ConfigAgentRemote]"
    ));

    // Supplied overrides build `AgentConfigEntry` values keyed by the path.
    assert!(client.contains(
        "_root_.golem.bridge.runtime.AgentConfigEntry(_root_.scala.collection.immutable.List(\"db\", \"host\"), configValue)"
    ));
    assert!(client.contains(
        "_root_.golem.bridge.runtime.AgentConfigEntry(_root_.scala.collection.immutable.List(\"max-retries\"), configValue)"
    ));

    // The plain constructors pass an empty config list.
    assert!(client.contains(
        "_root_.golem.bridge.runtime.Bridge.createAgent(configuration, agentTypeName, parameters, phantomId, _root_.scala.collection.immutable.List()).map"
    ));

    cross_compile(pkg.package_dir().as_path());
}

/// An ephemeral agent with local config overrides omits the parameter-
/// addressable `getWithConfig` (it has no `get`) but still exposes the phantom
/// config variants; and a durable agent without config declarations emits no
/// `…WithConfig` variants at all.
#[test]
fn config_override_constructors_respect_mode_and_absence() {
    let ephemeral = GeneratedPackage::new({
        let mut at = agent(
            "EphemeralConfigAgent",
            "scala",
            vec![field("name", SchemaType::string())],
            vec![method("ping", vec![], None)],
            vec![],
            AgentMode::Ephemeral,
        );
        at.config = vec![local_config(vec!["db", "host"], SchemaType::string())];
        at
    });
    let ephemeral_client = std::fs::read_to_string(ephemeral.package_dir().join(
        "src/main/scala/golem/bridge/client/ephemeral_config_agent/EphemeralConfigAgentClient.scala",
    ))
    .unwrap();
    assert!(
        !ephemeral_client.contains("def getWithConfig("),
        "ephemeral agent must not expose getWithConfig"
    );
    assert!(ephemeral_client.contains("def getPhantomWithConfig("));
    assert!(ephemeral_client.contains("def newPhantomWithConfig("));

    let no_config = GeneratedPackage::new(agent(
        "PlainAgent",
        "scala",
        vec![field("name", SchemaType::string())],
        vec![method("ping", vec![], None)],
        vec![],
        AgentMode::Durable,
    ));
    let no_config_client = std::fs::read_to_string(
        no_config
            .package_dir()
            .join("src/main/scala/golem/bridge/client/plain_agent/PlainAgentClient.scala"),
    )
    .unwrap();
    assert!(
        !no_config_client.contains("WithConfig"),
        "an agent without config declarations must not emit any WithConfig variant"
    );
}

/// Scala guest tool bridges emit a Scala.js guest-client project, a root client
/// with subtree accessors, canonical record construction, typed error enums,
/// and calls through the Scala SDK's `ToolClientRuntime` boundary.
#[test]
fn guest_tool_generation_emits_client_tree_and_runtime_boundary() {
    let pkg = GeneratedToolPackage::new(grep_tool());
    let dir = pkg.package_dir();
    let source = std::fs::read_to_string(
        dir.join("src/main/scala/golem/bridge/client/grep/GrepClient.scala"),
    )
    .unwrap();

    for shape in [
        "final class GrepClient",
        "object GrepClient",
        "def apply(): GrepClient",
        "def grep(",
        "def replace(",
        "sealed trait GrepError",
        "final case class BadPattern",
        "case object Io",
        "_root_.golem.runtime.tool.client.ToolRpcClient.transport(\"grep\")",
        "_root_.golem.schema.TypedSchemaValue(__schema, _root_.golem.schema.SchemaValue.RecordValue(__fields))",
        "_root_.golem.tool.ToolClientRuntime.invokeAndAwait[_root_.golem.bridge.client.grep.GrepError]",
        "_root_.golem.tool.ToolClientRuntime.invokeAndAwaitInfallible",
        "object Codecs",
        "sealed trait ColorMode",
    ] {
        assert!(source.contains(shape), "missing {shape}:\n{source}");
    }

    assert!(
        !source.contains("golem.bridge.runtime"),
        "guest tool bridge must not reference the external REST runtime:\n{source}"
    );

    let build_sbt = std::fs::read_to_string(dir.join("build.sbt")).unwrap();
    assert!(build_sbt.contains("\"grep-tool-guest-client\""));
    assert!(build_sbt.contains("enablePlugins(ScalaJSPlugin)"));
    assert!(build_sbt.contains("scalaJSUseMainModuleInitializer := false"));
    assert!(dir.join("project/plugins.sbt").exists());

    cross_compile_guest_if_enabled(dir.as_path());
}

/// Tool guest clients reserve Scala keywords, universal members, helper names,
/// and generated error names before emitting clients so awkward tool metadata
/// still produces valid, collision-free Scala.
#[test]
fn guest_tool_generation_handles_keywords_and_collisions() {
    let mut tool = grep_tool();
    tool.commands.nodes[0].name = "type".to_string();
    tool.commands.nodes[0].globals = Globals::default();
    tool.commands.nodes[0].subcommands = vec![];
    tool.commands.nodes[0].body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![
                positional("decode-result-value", SchemaType::string()),
                positional("elems0", SchemaType::fixed_list(SchemaType::string(), 1)),
            ],
            tail: None,
        },
        errors: vec![
            ErrorCase {
                name: "type".to_string(),
                doc: doc("type"),
                kind: ErrorKind::UsageError,
                exit_code: 2,
                payload: Some(SchemaType::string()),
            },
            ErrorCase {
                name: "type".to_string(),
                doc: doc("type duplicate"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: None,
            },
        ],
        ..tool_body()
    });
    tool.commands.nodes.truncate(1);
    tool.schema = SchemaGraph::empty();

    let pkg = GeneratedToolPackage::new(tool);
    let dir = pkg.package_dir();
    let source = std::fs::read_to_string(
        dir.join("src/main/scala/golem/bridge/client/type_/TypeClient.scala"),
    )
    .unwrap();

    assert!(source.contains("package golem.bridge.client.type_"));
    assert!(source.contains("final class TypeClient"));
    assert!(source.contains("def `type`("));
    assert!(
        !source.contains("def type("),
        "keyword method name must not be emitted bare:\n{source}"
    );
    assert!(
        !source.contains("decodeResultValue: _root_.scala.Predef.String"),
        "parameter must not shadow the generated decodeResultValue helper:\n{source}"
    );
    assert!(source.contains("decodeResultValue_2: _root_.scala.Predef.String"));
    assert!(
        !source.contains("val elems0 = elems0.map"),
        "parameter must not shadow fixed-list encoder temporaries:\n{source}"
    );
    assert!(source.contains("elems0_2: _root_.scala.collection.immutable.List"));
    assert!(source.contains("val elems0 = elems0_2.map"));
    assert!(source.contains("final case class Type"));
    assert!(source.contains("case object Type_2"));

    cross_compile_guest_if_enabled(dir.as_path());
}

#[test]
fn guest_tool_error_decoder_return_type_uses_error_trait_when_variant_shadows_name() {
    let mut tool = grep_tool();
    tool.commands.nodes[0].globals = Globals::default();
    tool.commands.nodes[0].subcommands = vec![];
    tool.commands.nodes[0].body = Some(CommandBody {
        errors: vec![
            ErrorCase {
                name: "grep-error".to_string(),
                doc: doc("grep error"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: Some(SchemaType::string()),
            },
            ErrorCase {
                name: "io".to_string(),
                doc: doc("io"),
                kind: ErrorKind::RuntimeError,
                exit_code: 2,
                payload: None,
            },
        ],
        ..tool_body()
    });
    tool.commands.nodes.truncate(1);
    tool.schema = SchemaGraph::empty();

    let pkg = GeneratedToolPackage::new(tool);
    let dir = pkg.package_dir();
    let source = std::fs::read_to_string(
        dir.join("src/main/scala/golem/bridge/client/grep/GrepClient.scala"),
    )
    .unwrap();

    assert!(source.contains(
        "final case class GrepError(value: _root_.scala.Predef.String) extends _root_.golem.bridge.client.grep.GrepError"
    ));
    assert!(source.contains(
        "_root_.scala.Either[_root_.scala.Predef.String, _root_.golem.bridge.client.grep.GrepError]"
    ));
    assert!(source.contains("_root_.golem.bridge.client.grep.GrepError.GrepError"));
    assert!(source.contains("_root_.golem.bridge.client.grep.GrepError.Io"));

    cross_compile(dir.as_path());
}

#[test]
fn guest_tool_error_trait_parents_do_not_resolve_to_generated_user_types() {
    let mut tool = grep_tool();
    let product_type = TypeId::from("product");
    tool.commands.nodes[0].globals = Globals::default();
    tool.commands.nodes[0].subcommands = vec![];
    tool.commands.nodes[0].body = Some(CommandBody {
        errors: vec![ErrorCase {
            name: "bad-product".to_string(),
            doc: doc("bad product"),
            kind: ErrorKind::RuntimeError,
            exit_code: 1,
            payload: Some(SchemaType::ref_to(product_type.clone())),
        }],
        ..tool_body()
    });
    tool.commands.nodes.truncate(1);
    tool.schema = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: product_type,
            name: Some("product".to_string()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "name".to_string(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            }]),
        }],
        root: SchemaType::record(vec![]),
    };

    let pkg = GeneratedToolPackage::new(tool);
    let dir = pkg.package_dir();

    cross_compile(dir.as_path());
}
