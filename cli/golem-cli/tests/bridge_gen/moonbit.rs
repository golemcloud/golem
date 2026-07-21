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
    agent, def, field, method, multi_agent_wrapper_2_types, named_field, ref_to,
    single_agent_wrapper_types, variant_case,
};
use crate::bridge_gen::type_naming::test_type_naming;
use camino::Utf8Path;
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::moonbit::moonbit::{
    to_moonbit_constructor_ident, to_moonbit_term_ident, unique_idents, unique_idents_with_reserved,
};
use golem_cli::bridge_gen::moonbit::tool::MoonBitToolBridgeGenerator;
use golem_cli::bridge_gen::moonbit::{
    MoonBitBridgeGenerator, MoonBitBridgeMode, MoonBitTypeName, emit_schema_graph_literal,
};
use golem_cli::bridge_gen::type_naming::TypeName;
use golem_cli::model::GuestLanguage;
use golem_cli::sdk_overrides::workspace_root;
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NumericBound, NumericRestrictions,
    PathDirection, PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, SecretSpec,
    TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
use golem_common::schema::tool::{
    CommandBody, CommandIndex, CommandNode, CommandTree, Doc, ErrorCase, ErrorKind, Formatter,
    Globals, Positional, Positionals, ResultSpec as ToolResultSpec, StreamSpec, Tool,
};
use golem_common::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_text_schema_type,
};
use golem_common::schema::{
    AgentTypeSchema, MetadataEnvelope, ResultSpec, Role, SchemaGraph, SchemaType, SchemaTypeDef,
    TypeId,
};
use tempfile::TempDir;
use test_r::{test, test_dep};

/// Builds the structural multimodal schema type `list<variant<…> Role::Multimodal>`
/// from `(case_name, payload)` modalities.
fn multimodal(cases: Vec<(&str, SchemaType)>) -> SchemaType {
    let variant = SchemaType::variant(
        cases
            .into_iter()
            .map(|(name, payload)| VariantCaseType {
                name: name.to_string(),
                payload: Some(payload),
                metadata: Default::default(),
            })
            .collect(),
    );
    let mut ty = SchemaType::list(variant);
    ty.metadata_mut().role = Some(Role::Multimodal);
    ty
}

/// A generated MoonBit bridge module, type-checked with `moon check --target
/// native`. The module root is the temp dir itself (the generator writes
/// `moon.mod.json`, `runtime/`, and `client/` directly under the target path).
struct GeneratedPackage {
    dir: TempDir,
}

impl GeneratedPackage {
    fn new(agent_type: AgentTypeSchema) -> Self {
        let dir = TempDir::new().unwrap();
        let target_dir = Utf8Path::from_path(dir.path()).unwrap();
        let mut generator = MoonBitBridgeGenerator::new(agent_type, target_dir, true).unwrap();
        generator.generate().unwrap();
        let pkg = GeneratedPackage { dir };
        pkg.check();
        pkg
    }

    fn module_dir(&self) -> &Utf8Path {
        Utf8Path::from_path(self.dir.path()).unwrap()
    }

    fn client_source(&self) -> String {
        std::fs::read_to_string(self.module_dir().join("client/client.mbt")).unwrap()
    }

    fn check(&self) {
        let output = std::process::Command::new("moon")
            .arg("check")
            .arg("--target")
            .arg("native")
            .current_dir(self.module_dir())
            .output()
            .expect("failed to run moon; is it installed?");
        assert!(
            output.status.success(),
            "moon check failed in {}:\nstdout:\n{}\nstderr:\n{}",
            self.module_dir(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

fn generate_without_check(agent_type: AgentTypeSchema, mode: MoonBitBridgeMode) -> TempDir {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let mut generator =
        MoonBitBridgeGenerator::new_with_mode(agent_type, target_dir, true, mode).unwrap();
    generator.generate().unwrap();
    dir
}

fn generate_with_default_mode_without_check(agent_type: AgentTypeSchema) -> TempDir {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let mut generator = MoonBitBridgeGenerator::new(agent_type, target_dir, true).unwrap();
    generator.generate().unwrap();
    dir
}

fn generated_files(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn visit(
        root: &std::path::Path,
        current: &std::path::Path,
        files: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
    ) {
        for entry in std::fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                ));
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort_by(|(left, _), (right, _)| left.cmp(right));
    files
}

fn moon_check_wasm(path: &std::path::Path) {
    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(path)
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "moon check failed in {}:\nstdout:\n{}\nstderr:\n{}",
        path.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tool_doc(summary: &str) -> Doc {
    Doc {
        summary: summary.to_string(),
        description: String::new(),
        examples: vec![],
    }
}

fn empty_tool_body() -> CommandBody {
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

fn tool_positional(name: &str, type_: SchemaType) -> Positional {
    Positional {
        name: name.to_string(),
        doc: tool_doc(name),
        value_name: None,
        type_,
        default: None,
        required: true,
        accepts_stdio: false,
    }
}

fn tool_node(name: &str) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        aliases: vec![],
        doc: tool_doc(name),
        globals: Globals::default(),
        subcommands: vec![],
        body: None,
    }
}

fn phase_eight_tool() -> Tool {
    let recursive_id = TypeId::new("phase-eight.Recursive");
    let outer_alias_id = TypeId::new("phase-eight.OuterAlias");
    let inner_alias_id = TypeId::new("phase-eight.InnerAlias");
    let tuple_8 = SchemaType::tuple(vec![SchemaType::string(); 8]);
    let tuple_9 = SchemaType::tuple(vec![SchemaType::string(); 9]);

    let mut root = tool_node("new");
    root.subcommands = vec![
        CommandIndex(1),
        CommandIndex(2),
        CommandIndex(3),
        CommandIndex(4),
    ];
    root.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![
                tool_positional(
                    "text",
                    unstructured_text_schema_type(TextRestrictions {
                        languages: Some(vec!["en".into(), "de".into()]),
                        ..Default::default()
                    }),
                ),
                tool_positional("items", SchemaType::list(SchemaType::string())),
                tool_positional(
                    "fixed-items",
                    SchemaType::fixed_list(SchemaType::string(), 2),
                ),
                tool_positional("empty-tuple", SchemaType::tuple(vec![])),
                tool_positional(
                    "single-tuple",
                    SchemaType::tuple(vec![SchemaType::string()]),
                ),
                tool_positional("tuple-eight", tuple_8),
                tool_positional("tuple-nine", tuple_9),
                tool_positional(
                    "path",
                    SchemaType::path(PathSpec {
                        direction: PathDirection::InOut,
                        kind: PathKind::Any,
                        allowed_mime_types: None,
                        allowed_extensions: None,
                    }),
                ),
                tool_positional("url", SchemaType::url(UrlRestrictions::default())),
                tool_positional("datetime", SchemaType::datetime()),
                tool_positional("duration", SchemaType::duration()),
                tool_positional("recursive", SchemaType::ref_to(recursive_id.clone())),
                tool_positional("aliased", SchemaType::ref_to(outer_alias_id.clone())),
            ],
            tail: None,
        },
        stdin: Some(StreamSpec {
            doc: tool_doc("stdin"),
            mime: vec!["text/plain".into()],
            required: false,
        }),
        stdout: Some(StreamSpec {
            doc: tool_doc("stdout"),
            mime: vec!["text/plain".into()],
            required: false,
        }),
        result: Some(ToolResultSpec {
            type_: SchemaType::string(),
            doc: tool_doc("result"),
            formatters: vec![Formatter {
                name: "text".into(),
                doc: tool_doc("text"),
            }],
            default_formatter: "text".into(),
        }),
        errors: vec![
            ErrorCase {
                name: "error".into(),
                doc: tool_doc("text error"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: Some(SchemaType::string()),
            },
            ErrorCase {
                name: "error".into(),
                doc: tool_doc("restricted text error"),
                kind: ErrorKind::UsageError,
                exit_code: 2,
                payload: Some(unstructured_text_schema_type(TextRestrictions {
                    languages: Some(vec!["fr".into()]),
                    ..Default::default()
                })),
            },
        ],
        ..empty_tool_body()
    });

    let mut required_streams = tool_node("drop");
    required_streams.body = Some(CommandBody {
        stdin: Some(StreamSpec {
            doc: tool_doc("stdin"),
            mime: vec![],
            required: true,
        }),
        stdout: Some(StreamSpec {
            doc: tool_doc("stdout"),
            mime: vec![],
            required: true,
        }),
        ..empty_tool_body()
    });

    let mut result_only = tool_node("client");
    result_only.body = Some(CommandBody {
        result: Some(ToolResultSpec {
            type_: SchemaType::u64(),
            doc: tool_doc("result"),
            formatters: vec![],
            default_formatter: String::new(),
        }),
        ..empty_tool_body()
    });

    let mut optional_stdout = tool_node("new");
    optional_stdout.body = Some(CommandBody {
        stdout: Some(StreamSpec {
            doc: tool_doc("stdout"),
            mime: vec![],
            required: false,
        }),
        ..empty_tool_body()
    });

    let mut multimodal_output = tool_node("render");
    multimodal_output.body = Some(CommandBody {
        result: Some(ToolResultSpec {
            type_: multimodal(vec![("text", SchemaType::string())]),
            doc: tool_doc("rendered modalities"),
            formatters: vec![],
            default_formatter: String::new(),
        }),
        errors: vec![ErrorCase {
            name: "render-failed".into(),
            doc: tool_doc("render failed"),
            kind: ErrorKind::RuntimeError,
            exit_code: 1,
            payload: Some(multimodal(vec![("text", SchemaType::string())])),
        }],
        ..empty_tool_body()
    });

    Tool {
        version: "1".into(),
        commands: CommandTree {
            nodes: vec![
                root,
                required_streams,
                result_only,
                optional_stdout,
                multimodal_output,
            ],
        },
        schema: SchemaGraph {
            defs: vec![
                SchemaTypeDef {
                    id: recursive_id.clone(),
                    name: Some("Recursive".into()),
                    body: SchemaType::record(vec![named_field(
                        "next",
                        SchemaType::option(SchemaType::ref_to(recursive_id)),
                    )]),
                },
                SchemaTypeDef {
                    id: outer_alias_id,
                    name: Some("OuterAlias".into()),
                    body: SchemaType::ref_to(inner_alias_id.clone()),
                },
                SchemaTypeDef {
                    id: inner_alias_id,
                    name: Some("InnerAlias".into()),
                    body: SchemaType::string(),
                },
            ],
            root: SchemaType::record(vec![]),
        },
    }
}

fn counter_agent() -> AgentTypeSchema {
    agent(
        "CounterAgent",
        "moonbit",
        vec![field("name", SchemaType::string())],
        vec![method("increment", vec![], Some(SchemaType::f64()))],
        vec![],
        AgentMode::Durable,
    )
}

#[test]
fn explicit_external_mode_is_byte_identical_to_the_default() {
    let default = generate_with_default_mode_without_check(counter_agent());
    let explicit = generate_without_check(counter_agent(), MoonBitBridgeMode::ExternalRest);

    assert_eq!(
        generated_files(default.path()),
        generated_files(explicit.path())
    );
}

#[test]
fn guest_mode_generates_wasm_rpc_project_layout() {
    let guest = generate_without_check(counter_agent(), MoonBitBridgeMode::GuestWasmRpc);
    let module: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(guest.path().join("moon.mod.json")).unwrap())
            .unwrap();
    let sdk_path = workspace_root().unwrap().join("sdks/moonbit/golem_sdk");

    assert_eq!(module["name"], "counter-agent-guest-client");
    assert_eq!(module["preferred-target"], "wasm");
    assert_eq!(
        module["deps"]["golemcloud/golem_sdk"]["path"],
        sdk_path.to_str().unwrap()
    );
    assert!(module["deps"].get("moonbitlang/async").is_none());
    assert!(!guest.path().join("runtime").exists());

    let moon_pkg = std::fs::read_to_string(guest.path().join("client/moon.pkg")).unwrap();
    assert!(moon_pkg.contains(r#""golemcloud/golem_sdk/agents""#));
    assert!(moon_pkg.contains(r#""golemcloud/golem_sdk/rpc""#));
    assert!(moon_pkg.contains(r#""golemcloud/golem_sdk/schema_model" @model"#));
    assert!(moon_pkg.contains(r#""golemcloud/golem_sdk/interface/golem/agent/common" @common"#));
    assert!(moon_pkg.contains(r#""golemcloud/golem_sdk/interface/golem/core/types" @types"#));
    assert!(!moon_pkg.contains("moonbitlang/async"));
    assert!(!moon_pkg.contains("/runtime"));
    assert!(!moon_pkg.contains("golemcloud/golem_sdk/tool"));
}

#[test]
fn guest_tool_mode_generates_schema_complete_buildable_consumer_module() {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    let mut generator = MoonBitToolBridgeGenerator::new(phase_eight_tool(), target, true).unwrap();
    generator.generate().unwrap();

    let module: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("moon.mod.json")).unwrap())
            .unwrap();
    assert_eq!(module["name"], "new-tool-guest-client");
    assert_eq!(module["preferred-target"], "wasm");
    assert!(!target.join("runtime").exists());

    let pkg = std::fs::read_to_string(target.join("client/moon.pkg")).unwrap();
    let source = std::fs::read_to_string(target.join("client/client.mbt")).unwrap();
    assert!(!pkg.contains("/runtime"));
    assert!(!source.contains("@runtime"));
    for expected in [
        "pub(all) struct NewClient",
        "pub fn NewClient::new_2(",
        "pub fn NewClient::drop_2(",
        "pub fn NewClient::client(",
        "stdin : @streams.InputStream?",
        "stdin : @streams.InputStream",
        "@streams.OutputStream?",
        "@streams.OutputStream",
        "pub(all) enum NewError",
        "Error(String)",
        "Error_2(UnstructuredText)",
        "@tool.CanonicalInputModel::{ fields:",
        "body: @model.FixedList",
        "body: @model.List",
        "body: @model.Path",
        "body: @model.Url",
        "body: @model.Datetime",
        "body: @model.Duration",
        "\"phase-eight.Recursive\"",
        "[\"en\", \"de\"]",
        "[\"fr\"]",
    ] {
        assert!(source.contains(expected), "missing {expected}:\n{source}");
    }

    std::fs::create_dir(target.join("consumer")).unwrap();
    std::fs::write(
        target.join("consumer/moon.pkg"),
        "import {\n  \"new-tool-guest-client/client\" @client,\n}\n",
    )
    .unwrap();
    std::fs::write(
        target.join("consumer/consumer.mbt"),
        r#"pub fn consume() -> Unit {
  let client = @client.NewClient::new()
  ignore(client.client())
  client.drop()
}
"#,
    )
    .unwrap();

    moon_check_wasm(dir.path());
}

#[test]
fn guest_tool_mode_moon_checks_fixed_codec_type_name_collision() {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    let mut tool = phase_eight_tool();
    tool.commands.nodes[0].name = "codec".into();
    let mut generator = MoonBitToolBridgeGenerator::new(tool, target, true).unwrap();
    generator.generate().unwrap();

    let source = std::fs::read_to_string(target.join("client/client.mbt")).unwrap();
    assert!(source.contains("pub(all) enum CodecError2"), "{source}");
    moon_check_wasm(dir.path());
}

#[test]
fn guest_mode_reserves_unstructured_support_type_names() {
    let user_type = def(
        "UnstructuredText",
        SchemaType::record(vec![named_field("value", SchemaType::string())]),
    );
    let agent = agent(
        "CollisionAgent",
        "moonbit",
        vec![field("value", ref_to("UnstructuredText"))],
        vec![],
        vec![user_type],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_emits_standalone_schema_value_codecs_and_moon_checks() {
    let guest = generate_without_check(kitchen_sink_agent(), MoonBitBridgeMode::GuestWasmRpc);
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();

    assert!(source.contains("pub suberror CodecError"));
    assert!(source.contains("-> @model.SchemaValue"));
    assert!(source.contains("@model.SchemaValue::FixedList("));
    assert!(source.contains("@model.SchemaValue::Datetime("));
    assert!(!source.contains("@runtime"));
    assert!(!source.contains("IntoSchema"));
    assert!(!source.contains("FromSchema"));
    assert!(source.contains("pub(all) struct KitchenAgentClient"));

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_emits_native_agent_client_and_exact_config_graph() {
    let mut agent_type = agent(
        "ConfiguredCounter",
        "moonbit",
        vec![field("name", SchemaType::string())],
        vec![
            method(
                "read",
                vec![field("delta", SchemaType::s32())],
                Some(SchemaType::s64()),
            ),
            method("reset", vec![], None),
        ],
        vec![],
        AgentMode::Durable,
    );
    agent_type.config = vec![AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["limits".into(), "retries".into()],
        value_type: SchemaType::S64 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Signed(1)),
                max: Some(NumericBound::Signed(9)),
                unit: Some("attempts".into()),
            }),
            metadata: MetadataEnvelope::default(),
        },
    }];
    let guest = generate_without_check(agent_type, MoonBitBridgeMode::GuestWasmRpc);
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();

    assert!(source.contains("pub(all) struct ConfiguredCounterClient"));
    assert!(source.contains("@rpc.AgentClient::get(\"ConfiguredCounter\", constructor_input)"));
    assert!(source.contains("@rpc.AgentClient::new_phantom("));
    assert!(source.contains("@rpc.AgentClient::get_phantom("));
    assert!(source.contains("pub fn[T] ConfiguredCounterClient::scoped("));
    assert!(source.contains("pub fn ConfiguredCounterClient::get_agent_id("));
    assert!(source.contains("pub fn ConfiguredCounterClient::phantom_id("));
    assert!(source.contains("pub fn ConfiguredCounterClient::drop("));
    assert!(source.contains("self.client.invoke_and_await(\"read\", input).value"));
    assert!(source.contains("let _ = self.client.invoke(\"read\", input)"));
    assert!(
        source.contains("let _ = self.client.schedule_invocation(scheduled_at, \"read\", input)")
    );
    assert!(
        source.contains(
            "self.client.schedule_cancelable_invocation(scheduled_at, \"read\", input).cancellation_token"
        )
    );
    assert!(source.contains("@types.Signed(1L)"));
    assert!(source.contains("@types.Signed(9L)"));
    assert!(source.contains("Some(\"attempts\")"));
    assert!(!source.contains("typed_config_value"));

    std::fs::create_dir(guest.path().join("consumer")).unwrap();
    std::fs::write(
        guest.path().join("consumer/moon.pkg"),
        "import {\n  \"configured-counter-guest-client/client\" @client,\n  \"golemcloud/golem_sdk/interface/golem/agent/common\" @common,\n}\n",
    )
    .unwrap();
    std::fs::write(
        guest.path().join("consumer/consumer.mbt"),
        r#"pub fn call_provider(name : String) -> Int64 raise @common.AgentError {
  @client.ConfiguredCounterClient::scoped(
    name,
    fn(provider) raise @common.AgentError {
      provider.read(1)
    },
  )
}
"#,
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_handles_helper_wrapper_and_local_name_collisions() {
    let agent_type = agent(
        "CollisionAgent",
        "moonbit",
        vec![
            field("client", SchemaType::string()),
            field("constructor-input", SchemaType::string()),
        ],
        vec![
            method(
                "drop",
                vec![
                    field("input", SchemaType::string()),
                    field("scheduled-at", SchemaType::string()),
                ],
                None,
            ),
            method("schedule-cancelable-drop", vec![], None),
            method("phantom-id", vec![], Some(SchemaType::string())),
        ],
        vec![],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent_type, MoonBitBridgeMode::GuestWasmRpc);
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();

    assert!(source.contains("pub fn CollisionAgentClient::drop_2("));
    assert!(source.contains("pub fn CollisionAgentClient::phantom_id_2("));
    assert!(source.contains("pub fn CollisionAgentClient::schedule_cancelable_drop_2("));
    assert!(source.contains("client_2 : String"));
    assert!(source.contains("constructor_input_2 : String"));
    assert!(source.contains("input_2 : String"));
    assert!(source.contains("scheduled_at_2 : String"));

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_scoped_does_not_shadow_constructor_type_t() {
    let agent_type = agent(
        "ScopedTypeAgent",
        "moonbit",
        vec![field("config", ref_to("T"))],
        vec![],
        vec![def(
            "T",
            SchemaType::record(vec![named_field("value", SchemaType::string())]),
        )],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent_type, MoonBitBridgeMode::GuestWasmRpc);

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_accepts_numeric_agent_type_names() {
    let guest = generate_without_check(
        agent(
            "123",
            "typescript",
            vec![],
            vec![],
            vec![],
            AgentMode::Durable,
        ),
        MoonBitBridgeMode::GuestWasmRpc,
    );
    let manifest_path = guest.path().join("moon.mod.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["name"] = "numeric-agent-guest-client".into();
    std::fs::write(
        manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ephemeral_guest_client_omits_get_and_scoped() {
    let guest = generate_without_check(
        agent(
            "EphemeralAgent",
            "moonbit",
            vec![],
            vec![method("ping", vec![], None)],
            vec![],
            AgentMode::Ephemeral,
        ),
        MoonBitBridgeMode::GuestWasmRpc,
    );
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();

    assert!(!source.contains("pub fn EphemeralAgentClient::get("));
    assert!(!source.contains("pub fn[T] EphemeralAgentClient::scoped("));
    assert!(source.contains("pub fn EphemeralAgentClient::new_phantom("));
    assert!(source.contains("pub fn EphemeralAgentClient::get_phantom("));
}

#[test]
fn ephemeral_guest_methods_keep_names_of_absent_lifecycle_helpers() {
    let mut agent_type = agent(
        "EphemeralCollisionAgent",
        "moonbit",
        vec![],
        vec![
            method("get", vec![], None),
            method("get_with_config", vec![], None),
            method("scoped", vec![], None),
        ],
        vec![],
        AgentMode::Ephemeral,
    );
    agent_type.config = vec![AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["region".into()],
        value_type: SchemaType::string(),
    }];
    let guest = generate_without_check(agent_type, MoonBitBridgeMode::GuestWasmRpc);
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();
    let expected_methods = [
        "pub fn EphemeralCollisionAgentClient::get(self : EphemeralCollisionAgentClient)",
        "pub fn EphemeralCollisionAgentClient::get_with_config(self : EphemeralCollisionAgentClient)",
        "pub fn EphemeralCollisionAgentClient::scoped(self : EphemeralCollisionAgentClient)",
    ];
    let missing = expected_methods
        .iter()
        .filter(|method| !source.contains(**method))
        .copied()
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "methods were renamed despite the colliding lifecycle helpers being absent: {missing:?}\n{source}"
    );
}

#[test]
fn schema_graph_literal_moon_checks_as_sdk_model() {
    let graph = SchemaGraph {
        defs: vec![def(
            "original.Type-ID",
            SchemaType::record(vec![named_field(
                "next",
                SchemaType::option(ref_to("original.Type-ID")),
            )]),
        )],
        root: SchemaType::record(vec![
            named_field(
                "numeric",
                SchemaType::S64 {
                    restrictions: Some(NumericRestrictions {
                        min: Some(NumericBound::Signed(-9)),
                        max: Some(NumericBound::Signed(12)),
                        unit: Some("ms".into()),
                    }),
                    metadata: MetadataEnvelope {
                        doc: Some("root docs".into()),
                        aliases: vec!["alias".into()],
                        examples: vec!["42".into()],
                        deprecated: Some("old".into()),
                        role: Some(Role::Other("custom".into())),
                    },
                },
            ),
            named_field(
                "text",
                SchemaType::text(TextRestrictions {
                    languages: Some(vec!["en".into()]),
                    min_length: Some(1),
                    max_length: Some(8),
                    regex: Some("^[a-z]+$".into()),
                }),
            ),
            named_field(
                "binary",
                SchemaType::binary(BinaryRestrictions {
                    mime_types: Some(vec!["image/png".into()]),
                    min_bytes: Some(1),
                    max_bytes: Some(1024),
                }),
            ),
            named_field(
                "path",
                SchemaType::path(PathSpec {
                    direction: PathDirection::InOut,
                    kind: PathKind::File,
                    allowed_mime_types: Some(vec!["text/plain".into()]),
                    allowed_extensions: Some(vec![".txt".into()]),
                }),
            ),
            named_field(
                "url",
                SchemaType::url(UrlRestrictions {
                    allowed_schemes: Some(vec!["https".into()]),
                    allowed_hosts: Some(vec!["example.com".into()]),
                }),
            ),
            named_field(
                "quantity",
                SchemaType::quantity(QuantitySpec {
                    base_unit: "m".into(),
                    allowed_suffixes: vec!["km".into()],
                    min: Some(QuantityValue {
                        mantissa: -2,
                        scale: 1,
                        unit: "m".into(),
                    }),
                    max: Some(QuantityValue {
                        mantissa: 9,
                        scale: 0,
                        unit: "m".into(),
                    }),
                }),
            ),
            named_field(
                "union",
                SchemaType::union(UnionSpec {
                    branches: vec![
                        UnionBranch {
                            tag: "prefix".into(),
                            body: SchemaType::string(),
                            discriminator: DiscriminatorRule::Prefix { prefix: "a".into() },
                            metadata: MetadataEnvelope::default(),
                        },
                        UnionBranch {
                            tag: "suffix".into(),
                            body: SchemaType::string(),
                            discriminator: DiscriminatorRule::Suffix { suffix: "z".into() },
                            metadata: MetadataEnvelope::default(),
                        },
                        UnionBranch {
                            tag: "contains".into(),
                            body: SchemaType::string(),
                            discriminator: DiscriminatorRule::Contains {
                                substring: "x".into(),
                            },
                            metadata: MetadataEnvelope::default(),
                        },
                        UnionBranch {
                            tag: "regex".into(),
                            body: SchemaType::string(),
                            discriminator: DiscriminatorRule::Regex { regex: "^r".into() },
                            metadata: MetadataEnvelope::default(),
                        },
                        UnionBranch {
                            tag: "equals".into(),
                            body: SchemaType::record(vec![named_field(
                                "kind",
                                SchemaType::string(),
                            )]),
                            discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                                field_name: "kind".into(),
                                literal: Some("value".into()),
                            }),
                            metadata: MetadataEnvelope::default(),
                        },
                        UnionBranch {
                            tag: "absent".into(),
                            body: SchemaType::record(vec![named_field(
                                "other",
                                SchemaType::string(),
                            )]),
                            discriminator: DiscriminatorRule::FieldAbsent {
                                field_name: "kind".into(),
                            },
                            metadata: MetadataEnvelope::default(),
                        },
                    ],
                }),
            ),
            named_field(
                "secret",
                SchemaType::secret(SecretSpec {
                    inner: Box::new(SchemaType::string()),
                    category: Some("api-key".into()),
                }),
            ),
            named_field(
                "quota",
                SchemaType::quota_token(QuotaTokenSpec {
                    resource_name: Some("cpu".into()),
                }),
            ),
            named_field("future", SchemaType::future(Some(SchemaType::u32()))),
            named_field("stream", SchemaType::stream(None)),
        ]),
    };
    let dir = TempDir::new().unwrap();
    let sdk_path = workspace_root().unwrap().join("sdks/moonbit/golem_sdk");
    std::fs::create_dir(dir.path().join("client")).unwrap();
    std::fs::write(
        dir.path().join("moon.mod.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "schema-graph-literal-check",
            "version": "0.0.0",
            "preferred-target": "wasm",
            "deps": {
                "golemcloud/golem_sdk": { "path": sdk_path }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("client/moon.pkg"),
        "import {\n  \"golemcloud/golem_sdk/schema_model\" @model,\n  \"golemcloud/golem_sdk/interface/golem/core/types\" @types,\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("client/literal.mbt"),
        format!(
            "pub fn emitted_graph() -> @model.SchemaGraph {{\n  {}\n}}\n",
            emit_schema_graph_literal(&graph)
        ),
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(dir.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "schema graph literal moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_moon_checks_result_nested_in_list() {
    let nested = def(
        "nested",
        SchemaType::record(vec![named_field(
            "results",
            SchemaType::list(SchemaType::result(ResultSpec {
                ok: Some(Box::new(SchemaType::string())),
                err: Some(Box::new(SchemaType::u32())),
            })),
        )]),
    );
    let agent = agent(
        "NestedResultAgent",
        "moonbit",
        vec![field("value", ref_to("nested"))],
        vec![],
        vec![nested],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");

    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_moon_checks_unstructured_text_codec() {
    let media = def(
        "media",
        SchemaType::record(vec![named_field(
            "body",
            unstructured_text_schema_type(TextRestrictions::default()),
        )]),
    );
    let agent = agent(
        "MediaAgent",
        "moonbit",
        vec![field("value", ref_to("media"))],
        vec![],
        vec![media],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");

    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_unstructured_text_encoder_rejects_disallowed_language() {
    let media = def(
        "media",
        SchemaType::record(vec![named_field(
            "body",
            unstructured_text_schema_type(TextRestrictions {
                languages: Some(vec!["en".into()]),
                ..Default::default()
            }),
        )]),
    );
    let agent = agent(
        "MediaAgent",
        "moonbit",
        vec![field("value", ref_to("media"))],
        vec![],
        vec![media],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);
    std::fs::write(
        guest.path().join("client/unstructured_wbtest.mbt"),
        r#"test "unstructured text encoder enforces language restrictions" {
  let result = try? encode_Media({ body: Inline("bonjour", Some("fr")) })
  guard result is Err(CodecError(_)) else {
    fail("expected CodecError for a disallowed language")
  }
}
"#,
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("test")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_preserves_union_tags_and_moon_checks_multimodal_datetime() {
    let tagged = def(
        "tagged",
        SchemaType::union(UnionSpec {
            branches: vec![
                UnionBranch {
                    tag: "@runtime.BridgeError".into(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix {
                        prefix: "error:".into(),
                    },
                    metadata: MetadataEnvelope::default(),
                },
                UnionBranch {
                    tag: "@runtime.SchemaValue".into(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix {
                        prefix: "value:".into(),
                    },
                    metadata: MetadataEnvelope::default(),
                },
            ],
        }),
    );
    let agent = agent(
        "TaggedAgent",
        "moonbit",
        vec![
            field("tagged", ref_to("tagged")),
            field(
                "media",
                multimodal(vec![("captured-at", SchemaType::datetime())]),
            ),
        ],
        vec![],
        vec![tagged],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();
    assert!(source.contains("\"@runtime.BridgeError\""));
    assert!(source.contains("\"@runtime.SchemaValue\""));

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_moon_checks_referenced_unstructured_codecs_with_distinct_restrictions() {
    let text = def(
        "localized-text",
        unstructured_text_schema_type(TextRestrictions {
            languages: Some(vec!["en".into(), "de".into()]),
            ..Default::default()
        }),
    );
    let binary = def(
        "png-image",
        unstructured_binary_schema_type(BinaryRestrictions {
            mime_types: Some(vec!["image/png".into()]),
            ..Default::default()
        }),
    );
    let media = def(
        "media",
        SchemaType::record(vec![
            named_field("caption", ref_to("localized-text")),
            named_field("image", ref_to("png-image")),
        ]),
    );
    let agent = agent(
        "MediaAgent",
        "moonbit",
        vec![field("value", ref_to("media"))],
        vec![],
        vec![text, binary, media],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);
    let source = std::fs::read_to_string(guest.path().join("client/client.mbt")).unwrap();
    assert!(source.contains("guest_decode_unstructured_text(fields[0], [\"en\", \"de\"])"));
    assert!(source.contains("guest_decode_unstructured_binary(fields[1], [\"image/png\"])"));

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("check")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_fixed_list_encoder_rejects_wrong_length() {
    let fixed = def(
        "fixed",
        SchemaType::record(vec![named_field(
            "values",
            SchemaType::fixed_list(SchemaType::u32(), 2),
        )]),
    );
    let agent = agent(
        "FixedAgent",
        "moonbit",
        vec![field("value", ref_to("fixed"))],
        vec![],
        vec![fixed],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);
    std::fs::write(
        guest.path().join("client/fixed_wbtest.mbt"),
        r#"test "fixed-list length is checked" {
  let result = try? encode_Fixed({ values: [1U] })
  guard result is Err(CodecError(_)) else {
    fail("expected CodecError")
  }
}
"#,
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("test")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_narrow_integer_encoder_rejects_out_of_range_value() {
    let narrow = def(
        "narrow",
        SchemaType::record(vec![named_field("value", SchemaType::s8())]),
    );
    let agent = agent(
        "NarrowAgent",
        "moonbit",
        vec![field("value", ref_to("narrow"))],
        vec![],
        vec![narrow],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);
    std::fs::write(
        guest.path().join("client/narrow_wbtest.mbt"),
        r#"test "s8 range is checked" {
  let result = try? encode_Narrow({ value: 128 })
  guard result is Err(CodecError(_)) else {
    fail("expected CodecError for an out-of-range s8")
  }
}
"#,
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("test")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_mode_narrow_integer_decoder_rejects_out_of_range_values() {
    let narrow = def(
        "narrow",
        SchemaType::record(vec![
            named_field("s8_value", SchemaType::s8()),
            named_field("s16_value", SchemaType::s16()),
            named_field("u16_value", SchemaType::u16()),
        ]),
    );
    let agent = agent(
        "NarrowAgent",
        "moonbit",
        vec![field("value", ref_to("narrow"))],
        vec![],
        vec![narrow],
        AgentMode::Durable,
    );
    let guest = generate_without_check(agent, MoonBitBridgeMode::GuestWasmRpc);
    std::fs::write(
        guest.path().join("client/narrow_decode_wbtest.mbt"),
        r#"fn assert_decode_rejected(value : @model.SchemaValue) -> Unit raise {
  let result = try? decode_Narrow(value)
  guard result is Err(CodecError(_)) else {
    fail("expected CodecError for an out-of-range narrow integer")
  }
}

test "s8 decoder enforces schema range" {
  assert_decode_rejected(Record([S8(128), S16(0), U16(0)]))
}

test "s16 decoder enforces schema range" {
  assert_decode_rejected(Record([S8(0), S16(-32769), U16(0)]))
}

test "u16 decoder enforces schema range" {
  assert_decode_rejected(Record([S8(0), S16(0), U16(65536)]))
}
"#,
    )
    .unwrap();

    let output = std::process::Command::new("moon")
        .arg("-C")
        .arg(guest.path())
        .arg("test")
        .arg("--target")
        .arg("wasm")
        .output()
        .expect("failed to run moon; is it installed?");
    assert!(
        output.status.success(),
        "guest moon test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An agent whose constructor and methods exercise every schema type kind the
/// MoonBit bridge supports: all scalars, option/list/fixed-list/map/tuple
/// (including a 1-tuple), result (typed and unit), record, enum, variant,
/// flags, and a recursive type.
fn kitchen_sink_agent() -> AgentTypeSchema {
    let tree = def(
        "tree",
        SchemaType::record(vec![
            named_field("label", SchemaType::string()),
            named_field("children", SchemaType::list(ref_to("tree"))),
        ]),
    );
    let perms = def(
        "perms",
        SchemaType::flags(vec!["read".into(), "write".into(), "exec".into()]),
    );
    let shape = def(
        "shape",
        SchemaType::variant(vec![
            variant_case("circle", Some(SchemaType::f64())),
            variant_case(
                "rect",
                Some(SchemaType::tuple(vec![
                    SchemaType::f64(),
                    SchemaType::f64(),
                ])),
            ),
            variant_case("nothing", None),
        ]),
    );
    let color = def(
        "color",
        SchemaType::r#enum(vec!["red".into(), "green".into(), "blue".into()]),
    );
    let kitchen = def(
        "kitchen",
        SchemaType::record(vec![
            named_field("b", SchemaType::bool()),
            named_field("i8", SchemaType::s8()),
            named_field("i16", SchemaType::s16()),
            named_field("i32", SchemaType::s32()),
            named_field("i64", SchemaType::s64()),
            named_field("u8", SchemaType::u8()),
            named_field("u16", SchemaType::u16()),
            named_field("u32", SchemaType::u32()),
            named_field("u64", SchemaType::u64()),
            named_field("f32", SchemaType::f32()),
            named_field("f64", SchemaType::f64()),
            named_field("ch", SchemaType::char()),
            named_field("s", SchemaType::string()),
            named_field("dt", SchemaType::datetime()),
            named_field("dur", SchemaType::duration()),
            named_field(
                "opt",
                SchemaType::option(SchemaType::option(SchemaType::u32())),
            ),
            named_field("lst", SchemaType::list(SchemaType::string())),
            named_field("fixed", SchemaType::fixed_list(SchemaType::u32(), 3)),
            named_field(
                "m",
                SchemaType::map(SchemaType::string(), SchemaType::u32()),
            ),
            named_field(
                "tup",
                SchemaType::tuple(vec![
                    SchemaType::u32(),
                    SchemaType::string(),
                    SchemaType::bool(),
                ]),
            ),
            named_field("single_tup", SchemaType::tuple(vec![SchemaType::u32()])),
            named_field(
                "res",
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(SchemaType::string())),
                    err: Some(Box::new(SchemaType::u32())),
                }),
            ),
            named_field(
                "res_unit",
                SchemaType::result(ResultSpec {
                    ok: None,
                    err: None,
                }),
            ),
            named_field("col", ref_to("color")),
            named_field("perms", ref_to("perms")),
            named_field("shape", ref_to("shape")),
            named_field("tree", ref_to("tree")),
        ]),
    );
    agent(
        "KitchenAgent",
        "moonbit",
        vec![field("config", ref_to("kitchen"))],
        vec![
            method(
                "roundtrip",
                vec![field("input", ref_to("kitchen"))],
                Some(ref_to("kitchen")),
            ),
            method("get-shape", vec![], Some(ref_to("shape"))),
            method("noop", vec![], None),
        ],
        vec![tree, perms, shape, color, kitchen],
        AgentMode::Durable,
    )
}

// --- Compile tests (one shared generated+checked module per fixture) --------

#[test_dep(scope = PerWorker, tagged_as = "single_agent")]
fn moonbit_single_agent() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_1")]
fn moonbit_multi_agent_1() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_2")]
fn moonbit_multi_agent_2() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[1].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "counter_agent")]
fn moonbit_counter_agent() -> GeneratedPackage {
    GeneratedPackage::new(counter_agent())
}

#[test_dep(scope = PerWorker, tagged_as = "kitchen_sink")]
fn moonbit_kitchen_sink() -> GeneratedPackage {
    GeneratedPackage::new(kitchen_sink_agent())
}

#[test]
fn single_agent_compiles(#[tagged_as("single_agent")] _pkg: &GeneratedPackage) {}

#[test]
fn multi_agent_1_compiles(#[tagged_as("multi_agent_1")] _pkg: &GeneratedPackage) {}

#[test]
fn multi_agent_2_compiles(#[tagged_as("multi_agent_2")] _pkg: &GeneratedPackage) {}

#[test]
fn counter_agent_compiles(#[tagged_as("counter_agent")] _pkg: &GeneratedPackage) {}

#[test]
fn kitchen_sink_compiles(#[tagged_as("kitchen_sink")] _pkg: &GeneratedPackage) {}

// --- Structural assertions on the generated sources -------------------------

#[test]
fn generated_project_layout_is_correct(#[tagged_as("counter_agent")] pkg: &GeneratedPackage) {
    let dir = pkg.module_dir();

    let mod_json = std::fs::read_to_string(dir.join("moon.mod.json")).unwrap();
    assert!(mod_json.contains("\"name\": \"counter-agent-client\""));
    assert!(mod_json.contains("\"moonbitlang/async\": \"0.19.2\""));

    for runtime_file in [
        "schema_value.mbt",
        "json_codec.mbt",
        "config.mbt",
        "errors.mbt",
        "helpers.mbt",
        "ids.mbt",
        "protocol.mbt",
        "bridge.mbt",
        "unstructured.mbt",
        "moon.pkg",
    ] {
        assert!(
            dir.join("runtime").join(runtime_file).exists(),
            "runtime file {runtime_file} is missing"
        );
    }

    let client_pkg = std::fs::read_to_string(dir.join("client/moon.pkg")).unwrap();
    assert!(client_pkg.contains("\"counter-agent-client/runtime\" @runtime"));

    let client = pkg.client_source();
    assert!(client.contains("pub struct CounterAgent {"));
    assert!(client.contains("resolved : @runtime.ResolvedAgent"));
    assert!(client.contains("\"CounterAgent\""));
}

#[test]
fn generates_named_type_definitions(#[tagged_as("multi_agent_1")] pkg: &GeneratedPackage) {
    let client = pkg.client_source();

    // enum Color -> enum with payload-less cases.
    assert!(client.contains("pub(all) enum Color {"));
    assert!(client.contains("Red"));
    assert!(client.contains("Green"));
    assert!(client.contains("Blue"));

    // record Person -> struct with mapped field types (kebab-case -> snake_case,
    // u32 -> UInt, optional -> `?`, nested ref -> generated type name).
    assert!(client.contains("pub(all) struct Person {"));
    assert!(client.contains("first_name : String"));
    assert!(client.contains("age : UInt?"));
    assert!(client.contains("eye_color : Color"));

    // variant Location -> enum with payload-carrying and payload-less cases.
    assert!(client.contains("pub(all) enum Location {"));
    assert!(client.contains("Home(String)"));
    assert!(client.contains("Unknown"));
}

#[test]
fn generates_codecs_for_named_types(#[tagged_as("multi_agent_1")] pkg: &GeneratedPackage) {
    let client = pkg.client_source();

    // enum Color: positional case index encode / decode.
    assert!(client.contains("pub fn encode_Color(value : Color) -> @runtime.SchemaValue {"));
    assert!(client.contains("Color::Red => @runtime.EnumValue(0)"));
    assert!(client.contains("pub fn decode_Color(value : @runtime.SchemaValue) -> Color raise {"));
    assert!(client.contains("@runtime.as_enum(value)"));

    // record Person: positional fields, nested enum delegating to its codec,
    // optional u32 through the runtime accessors.
    assert!(client.contains("let fields = @runtime.as_record(value)"));
    assert!(client.contains("let f3 = encode_Color(value.eye_color)"));
    assert!(client.contains("let f3 = decode_Color(fields[3])"));
    assert!(client.contains("@runtime.U32Value"));
    assert!(client.contains("@runtime.as_u32"));

    // variant Location: case index + payload encode / decode.
    assert!(client.contains("@runtime.VariantValue(0, Some(vp))"));
    assert!(client.contains("@runtime.as_variant(value)"));
    assert!(client.contains("Location::Unknown"));
}

#[test]
fn generates_client_surface(#[tagged_as("multi_agent_1")] pkg: &GeneratedPackage) {
    let client = pkg.client_source();

    // Configuration + identity accessors delegating to the runtime.
    assert!(client.contains(
        "pub fn Agent1::configure(server : @runtime.GolemServer, app_name : String, env_name : String) -> Unit {"
    ));
    assert!(client.contains("@runtime.configure(server, app_name, env_name)"));
    assert!(client.contains("pub fn Agent1::agent_id(self : Agent1) -> @runtime.AgentId {"));

    // Durable constructors.
    assert!(client.contains("pub async fn Agent1::get("));
    assert!(client.contains("pub async fn Agent1::get_phantom("));
    assert!(client.contains("pub async fn Agent1::new_phantom("));
    assert!(client.contains("@runtime.create_agent(configuration, \"agent1\", parameters,"));
    assert!(client.contains("@runtime.random_uuid()"));

    // Per-method await / trigger / schedule wrappers, with the await result
    // decoded through the named-type codec.
    assert!(client.contains("pub async fn Agent1::f1(self : Agent1) -> Location raise {"));
    assert!(
        client
            .contains("@runtime.invoke_agent(self.resolved, \"f1\", parameters, \"await\", None)")
    );
    assert!(client.contains("decode_Location(value)"));
    assert!(client.contains("pub async fn Agent1::trigger_f1(self : Agent1) -> Unit raise {"));
    assert!(
        client.contains(
            "@runtime.invoke_agent(self.resolved, \"f1\", parameters, \"schedule\", None)"
        )
    );
    assert!(client.contains(
        "pub async fn Agent1::schedule_f1(self : Agent1, when : String) -> Unit raise {"
    ));
    assert!(client.contains(
        "@runtime.invoke_agent(self.resolved, \"f1\", parameters, \"schedule\", Some(when))"
    ));
}

#[test]
fn ephemeral_agent_omits_get_constructor() {
    let pkg = GeneratedPackage::new(agent(
        "EphemeralAgent",
        "moonbit",
        vec![field("name", SchemaType::string())],
        vec![method("ping", vec![], None)],
        vec![],
        AgentMode::Ephemeral,
    ));
    let client = pkg.client_source();

    assert!(
        !client.contains("pub async fn EphemeralAgent::get("),
        "ephemeral agent must not expose a parameter-addressable get"
    );
    assert!(!client.contains("pub async fn EphemeralAgent::get_phantom("));
    assert!(client.contains("pub async fn EphemeralAgent::new_phantom("));
    assert!(!client.contains("pub fn EphemeralAgent::agent_id("));
    assert!(!client.contains("@runtime.create_agent("));
    assert!(client.contains("-> @runtime.InvocationResponse[Unit] raise"));
    assert!(client.contains("-> @runtime.InvocationReceipt raise"));
    assert!(client.contains("idempotency_key: result.idempotency_key"));
}

/// Method names that are MoonBit keywords, fields whose names need
/// snake_case/keyword escaping, and parameters that collide with the generated
/// internal locals are all disambiguated so the generated client still compiles.
#[test]
fn client_surface_handles_reserved_and_keyword_names() {
    let pkg = GeneratedPackage::new(agent(
        "HygieneAgent",
        // Same-language so identifiers are only keyword/illegal-escaped, which
        // exercises the reserved-name disambiguation on its own.
        "moonbit",
        vec![
            field("configuration", SchemaType::string()),
            field("parameters", SchemaType::string()),
            field("type", SchemaType::string()),
        ],
        vec![
            // A keyword-named method whose parameters collide with method
            // internals (`when` synthetic param, `self`/`result`/`value`
            // locals, the `f0` record local) and include a keyword.
            method(
                "match",
                vec![
                    field("when", SchemaType::string()),
                    field("self", SchemaType::string()),
                    field("result", SchemaType::string()),
                    field("f0", SchemaType::string()),
                    field("type", SchemaType::string()),
                ],
                None,
            ),
        ],
        vec![],
        AgentMode::Durable,
    ));
    let client = pkg.client_source();

    // Keyword method name is escaped with a trailing underscore.
    assert!(client.contains("pub async fn HygieneAgent::match_(self : HygieneAgent"));
    // Keyword parameters/fields are escaped with a trailing underscore (`self`
    // is a keyword, so the user's `self` parameter becomes `self_`).
    assert!(client.contains("type_ : String"));
    assert!(client.contains("self_ : String"));
    // Parameters colliding with internal locals are renamed; the synthetic
    // `when` parameter of schedule keeps its name while the user's `when`
    // parameter is bumped.
    assert!(client.contains("when : String"));
    assert!(client.contains("when_2 : String"));
    assert!(client.contains("result_2 : String"));
    assert!(client.contains("f0_2 : String"));
    // Constructor parameters colliding with internal locals are renamed too.
    assert!(client.contains("configuration_2 : String"));
    assert!(client.contains("parameters_2 : String"));
}

#[test]
fn method_wrapper_names_are_deconflicted() {
    // Method names that normalize to the same MoonBit identifier, or that
    // collide with another method's generated `trigger_`/`schedule_` wrapper,
    // must still produce a module that compiles (no duplicate definitions).
    let pkg = GeneratedPackage::new(agent(
        "CollidingAgent",
        "moonbit",
        vec![field("name", SchemaType::string())],
        vec![
            // `foo-bar` and `foo_bar` both normalize to `foo_bar`.
            method("foo-bar", vec![], Some(SchemaType::string())),
            method("foo_bar", vec![], Some(SchemaType::string())),
            // `foo` generates `trigger_foo`/`schedule_foo`; `trigger-foo` and
            // `schedule-foo` would otherwise clash with those wrappers.
            method("foo", vec![], None),
            method("trigger-foo", vec![], None),
            method("schedule-foo", vec![], None),
        ],
        vec![],
        AgentMode::Durable,
    ));
    let client = pkg.client_source();

    // The two `foo_bar`-normalizing methods get distinct await wrappers.
    assert!(client.contains("pub async fn CollidingAgent::foo_bar(self : CollidingAgent"));
    assert!(client.contains("pub async fn CollidingAgent::foo_bar_2(self : CollidingAgent"));
    // `foo`'s generated wrappers and the `trigger-foo`/`schedule-foo` await
    // methods coexist without colliding (bumped to a `_2` suffix).
    assert!(client.contains("pub async fn CollidingAgent::trigger_foo(self : CollidingAgent"));
    assert!(client.contains("pub async fn CollidingAgent::trigger_foo_2(self : CollidingAgent"));
    assert!(client.contains("pub async fn CollidingAgent::schedule_foo_2(self : CollidingAgent"));
}

// --- MoonBit identifier escaping -------------------------------------------

#[test]
fn term_idents_are_snake_case_and_keyword_escaped() {
    // Cross-language: kebab/camel WIT names become snake_case.
    assert_eq!(to_moonbit_term_ident("first-name", false), "first_name");
    assert_eq!(to_moonbit_term_ident("firstName", false), "first_name");
    // Same-language: already MoonBit-cased, only escaping is applied.
    assert_eq!(to_moonbit_term_ident("first_name", true), "first_name");
    assert_eq!(to_moonbit_term_ident("type", true), "type_");
    assert_eq!(to_moonbit_term_ident("match", true), "match_");
}

#[test]
fn constructor_idents_are_upper_camel_and_deconflicted() {
    assert_eq!(
        to_moonbit_constructor_ident("shape-circle", false),
        "ShapeCircle"
    );
    assert_eq!(to_moonbit_constructor_ident("some", false), "Some_");
    assert_eq!(to_moonbit_constructor_ident("ok", false), "Ok_");
    assert_eq!(to_moonbit_constructor_ident("none", false), "None_");
}

#[test]
fn unique_idents_disambiguate_against_each_other_and_reserved() {
    assert_eq!(
        unique_idents(vec!["a".into(), "a".into(), "b".into()]),
        vec!["a".to_string(), "a_2".to_string(), "b".to_string()]
    );
    assert_eq!(
        unique_idents_with_reserved(vec!["self".into()], &["self"]),
        vec!["self_2".to_string()]
    );
}

// --- MoonBitTypeName casing -------------------------------------------------

#[test]
fn type_names_are_upper_camel_case() {
    let name = MoonBitTypeName::from_owner_and_name(None::<&str>, "all-primitives", false);
    assert_eq!(name.to_string(), "AllPrimitives");

    let owned = MoonBitTypeName::from_owner_and_name(Some("my-mod"), "the-type", false);
    assert_eq!(owned.to_string(), "MyModTheType");

    let segmented = MoonBitTypeName::from_segments(["foo", "bar-baz"], false);
    assert_eq!(segmented.to_string(), "FooBarBaz");
}

#[test]
fn test_type_naming_moonbit_rust_foo_agent() {
    test_type_naming::<MoonBitTypeName>(GuestLanguage::Rust, "FooAgent");
}

#[test]
fn test_type_naming_moonbit_ts_foo_agent() {
    test_type_naming::<MoonBitTypeName>(GuestLanguage::TypeScript, "FooAgent");
}

// --- Config override constructors -------------------------------------------

#[test]
fn config_constructors_are_generated() {
    let mut agent_type = agent(
        "ConfigAgent",
        "moonbit",
        vec![field("name", SchemaType::string())],
        vec![method("ping", vec![], None)],
        vec![],
        AgentMode::Durable,
    );
    agent_type.config = vec![
        AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["api-key".to_string()],
            value_type: SchemaType::string(),
        },
        AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["max".to_string(), "retries".to_string()],
            value_type: SchemaType::u32(),
        },
        // A secret-sourced config must not be exposed as an override parameter.
        AgentConfigDeclarationSchema {
            source: AgentConfigSource::Secret,
            path: vec!["secret".to_string()],
            value_type: SchemaType::string(),
        },
    ];
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();

    // All three with-config variants (durable -> get_with_config too).
    assert!(client.contains("pub async fn ConfigAgent::get_with_config("));
    assert!(client.contains("pub async fn ConfigAgent::get_phantom_with_config("));
    assert!(client.contains("pub async fn ConfigAgent::new_phantom_with_config("));
    // One Option parameter per *local* config, named config_<path_snake>.
    assert!(client.contains("config_api_key : String?"));
    assert!(client.contains("config_max_retries : UInt?"));
    // The secret-sourced config is not exposed.
    assert!(!client.contains("config_secret"));
    // phantom precedes the config overrides in get_phantom_with_config.
    assert!(client.contains(
        "pub async fn ConfigAgent::get_phantom_with_config(name : String, phantom : String, config_api_key : String?, config_max_retries : UInt?)"
    ));
    // Config entries are built from Some values, preserving the original path.
    assert!(client.contains("let agent_config : Array[@runtime.AgentConfigEntry] = []"));
    assert!(client.contains("agent_config.push(@runtime.AgentConfigEntry::{ path: [\"api-key\"]"));
    assert!(client.contains("path: [\"max\", \"retries\"]"));
    assert!(client.contains("None => ()"));
    // create_agent receives the built config array.
    assert!(client.contains("parameters, None, agent_config)"));
    assert!(client.contains("parameters, Some(phantom), agent_config)"));
    // new_phantom_with_config delegates to get_phantom_with_config with a fresh id.
    assert!(client.contains(
        "ConfigAgent::get_phantom_with_config(name, @runtime.random_uuid(), config_api_key, config_max_retries)"
    ));
}

/// An ephemeral agent must not get a `get_with_config` (no parameter-addressable
/// `get`), but still gets the phantom config variants.
#[test]
fn ephemeral_config_constructors_omit_get_with_config() {
    let mut agent_type = agent(
        "EphemeralConfigAgent",
        "moonbit",
        vec![],
        vec![method("ping", vec![], None)],
        vec![],
        AgentMode::Ephemeral,
    );
    agent_type.config = vec![AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["region".to_string()],
        value_type: SchemaType::string(),
    }];
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();

    assert!(!client.contains("pub async fn EphemeralConfigAgent::get_with_config("));
    assert!(!client.contains("pub async fn EphemeralConfigAgent::get_phantom_with_config("));
    assert!(client.contains("pub async fn EphemeralConfigAgent::new_phantom_with_config("));
    assert!(!client.contains("@runtime.create_agent("));
}

/// A constructor parameter whose normalized name collides with a generated
/// `config_<path>` override parameter must still produce a module that compiles.
#[test]
fn config_param_names_are_deconflicted() {
    let mut agent_type = agent(
        "ConfigCollisionAgent",
        "moonbit",
        // Normalizes to `config_api_key`, colliding with the override param below.
        vec![field("config-api-key", SchemaType::string())],
        vec![method("ping", vec![], None)],
        vec![],
        AgentMode::Durable,
    );
    agent_type.config = vec![AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["api-key".to_string()],
        value_type: SchemaType::string(),
    }];
    // Constructing the package type-checks the generated module with `moon check`.
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();
    // The override param is bumped away from the constructor param.
    assert!(client.contains("config_api_key_2 : String?"));
}

// --- Multimodal input / output ---------------------------------------------

#[test]
fn constructor_multimodal_is_precomputed() {
    let agent_type = agent(
        "VisionSession",
        "moonbit",
        vec![field(
            "input",
            multimodal(vec![
                ("text", SchemaType::string()),
                (
                    "image",
                    unstructured_binary_schema_type(BinaryRestrictions::default()),
                ),
            ]),
        )],
        vec![],
        vec![],
        AgentMode::Durable,
    );
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();

    assert!(client.contains("pub(all) enum Multimodal0 {"));
    assert!(client.contains(
        "pub async fn VisionSession::get(input : Array[Multimodal0]) -> VisionSession raise {"
    ));
    assert!(client.contains("let f0 = encode_Multimodal0(input)"));
}

#[test]
fn multimodal_input_and_output_are_generated() {
    let cases = || {
        vec![
            ("text", SchemaType::string()),
            (
                "image",
                unstructured_binary_schema_type(BinaryRestrictions::default()),
            ),
        ]
    };
    let agent_type = agent(
        "VisionAgent",
        "moonbit",
        vec![],
        vec![method(
            "analyze",
            vec![field("input", multimodal(cases()))],
            Some(multimodal(cases())),
        )],
        vec![],
        AgentMode::Durable,
    );
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();

    // One enum per distinct case list, with a case per modality.
    assert!(client.contains("pub(all) enum Multimodal0 {"));
    assert!(client.contains("Text(String)"));
    assert!(client.contains("Image(@runtime.UnstructuredBinary)"));
    // Codecs.
    assert!(client.contains(
        "pub fn encode_Multimodal0(values : Array[Multimodal0]) -> @runtime.SchemaValue {"
    ));
    assert!(client.contains(
        "pub fn decode_Multimodal0(value : @runtime.SchemaValue) -> Array[Multimodal0] raise {"
    ));
    // Method input is the multimodal array; output is the multimodal array too.
    assert!(client.contains(
        "pub async fn VisionAgent::analyze(self : VisionAgent, input : Array[Multimodal0]) -> Array[Multimodal0] raise {"
    ));
    // Multimodal input is still packed as a one-field record on the wire.
    assert!(client.contains("let f0 = encode_Multimodal0(input)"));
    assert!(client.contains("let parameters = @runtime.RecordValue([f0])"));
    // Output decodes the bare list value.
    assert!(client.contains("decode_Multimodal0(value)"));
    // The shared case list reuses a single enum.
    assert!(!client.contains("Multimodal1"));
}

/// Two methods sharing the same modality case list reuse a single multimodal
/// enum; a different case list gets its own.
#[test]
fn multimodal_enums_are_deduplicated() {
    let text_image = || {
        vec![
            ("text", SchemaType::string()),
            (
                "image",
                unstructured_binary_schema_type(BinaryRestrictions::default()),
            ),
        ]
    };
    let agent_type = agent(
        "MultiAgent",
        "moonbit",
        vec![],
        vec![
            method(
                "a",
                vec![field("in", multimodal(text_image()))],
                Some(multimodal(text_image())),
            ),
            method("b", vec![field("in", multimodal(text_image()))], None),
            method(
                "c",
                vec![field(
                    "in",
                    multimodal(vec![("note", SchemaType::string())]),
                )],
                None,
            ),
        ],
        vec![],
        AgentMode::Durable,
    );
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();

    assert!(client.contains("pub(all) enum Multimodal0 {"));
    assert!(client.contains("pub(all) enum Multimodal1 {"));
    assert!(!client.contains("pub(all) enum Multimodal2 {"));
}

// --- Unstructured text / binary restrictions -------------------------------

#[test]
fn unstructured_text_and_binary_restrictions_are_enforced() {
    let agent_type = agent(
        "MediaAgent",
        "moonbit",
        vec![],
        vec![
            method(
                "get_caption",
                vec![],
                Some(unstructured_text_schema_type(TextRestrictions {
                    languages: Some(vec!["en".to_string(), "de".to_string()]),
                    ..Default::default()
                })),
            ),
            method(
                "get_image",
                vec![],
                Some(unstructured_binary_schema_type(BinaryRestrictions {
                    mime_types: Some(vec!["image/png".to_string()]),
                    ..Default::default()
                })),
            ),
        ],
        vec![],
        AgentMode::Durable,
    );
    let pkg = GeneratedPackage::new(agent_type);
    let client = pkg.client_source();

    // Ergonomic wrapper return types.
    assert!(client.contains("-> @runtime.UnstructuredText raise {"));
    assert!(client.contains("-> @runtime.UnstructuredBinary raise {"));
    // Decode validates against the allowed language / MIME lists.
    assert!(client.contains(
        "@runtime.unstructured_text_from_schema_value(\"output\", value, [\"en\", \"de\"])"
    ));
    assert!(client.contains(
        "@runtime.unstructured_binary_from_schema_value(\"output\", value, [\"image/png\"])"
    ));
}
