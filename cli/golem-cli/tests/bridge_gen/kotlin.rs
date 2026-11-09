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
    agent, def, field, local_config, method, multi_agent_wrapper_2_types, named_field, ref_to,
    single_agent_wrapper_types, variant_case,
};
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::bridge_gen::kotlin::{KotlinBridgeGenerator, KotlinBridgeMode};
use golem_cli::bridge_gen::{BridgeGenerator, BridgeMode, bridge_client_directory_name};
use golem_cli::sdk_overrides::workspace_root;
use golem_common::model::agent::AgentMode;
use golem_common::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_text_schema_type,
};
use golem_common::schema::{
    AgentTypeSchema, BinaryRestrictions, ResultSpec, SchemaType, TextRestrictions,
};
use tempfile::TempDir;
use test_r::test;

struct GeneratedPackage {
    dir: TempDir,
    package_name: String,
}

impl GeneratedPackage {
    fn new(agent_type: AgentTypeSchema, mode: KotlinBridgeMode) -> Self {
        let bridge_mode = match mode {
            KotlinBridgeMode::ExternalRest => BridgeMode::External,
            KotlinBridgeMode::GuestWasmRpc => BridgeMode::Guest,
        };
        let package_name = bridge_client_directory_name(&agent_type.type_name, bridge_mode);
        let dir = TempDir::new().unwrap();
        let package_dir = Utf8Path::from_path(dir.path()).unwrap().join(&package_name);
        let mut generator =
            KotlinBridgeGenerator::new_with_mode(agent_type, &package_dir, true, mode).unwrap();
        generator.generate().unwrap();
        GeneratedPackage { dir, package_name }
    }

    fn package_dir(&self) -> Utf8PathBuf {
        Utf8Path::from_path(self.dir.path())
            .unwrap()
            .join(&self.package_name)
    }

    fn source(&self, relative: &str) -> String {
        std::fs::read_to_string(self.package_dir().join(relative)).unwrap()
    }
}

/// Runs Gradle in `package_dir`. The Kotlin compile checks need the Golem
/// Kotlin SDK in `mavenLocal`, so they run only when
/// `GOLEM_KOTLIN_BRIDGE_COMPILE` is set. `GOLEM_KOTLIN_GRADLE` names the
/// Gradle executable (default: `gradle`).
fn gradle_if_enabled(package_dir: &Utf8Path, task: &str) {
    if std::env::var_os("GOLEM_KOTLIN_BRIDGE_COMPILE").is_none() {
        return;
    }
    let gradle = std::env::var("GOLEM_KOTLIN_GRADLE").unwrap_or_else(|_| "gradle".to_string());
    let output = std::process::Command::new(gradle)
        .args(["--console=plain", "--no-daemon", task])
        .current_dir(package_dir)
        .output()
        .expect("failed to run gradle; is it installed?");
    assert!(
        output.status.success(),
        "gradle {task} failed in {package_dir}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs the Kotlin test sources of `kotlin_runtime_tests` against the embedded runtime of an external project. Like the compile
/// checks, it runs only when `GOLEM_KOTLIN_BRIDGE_COMPILE` is set.
fn gradle_runtime_tests_if_enabled(package_dir: &Utf8Path) {
    if std::env::var_os("GOLEM_KOTLIN_BRIDGE_COMPILE").is_none() {
        return;
    }
    let tests = package_dir.join("src/test/kotlin");
    std::fs::create_dir_all(&tests).unwrap();
    let source =
        Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bridge_gen/kotlin_runtime_tests");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(
            &path,
            tests.join(path.file_name().unwrap().to_str().unwrap()),
        )
        .unwrap();
    }
    let fixtures = workspace_root()
        .unwrap()
        .join("golem-client/tests/fixtures/stream-session-v1");
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(
            &path,
            package_dir.join(path.file_name().unwrap().to_str().unwrap()),
        )
        .unwrap();
    }
    let build_path = package_dir.join("build.gradle.kts");
    let mut build = std::fs::read_to_string(&build_path).unwrap();
    build.push_str("\ndependencies {\n    testImplementation(kotlin(\"test\"))\n}\n\ntasks.test {\n    useJUnitPlatform()\n}\n");
    std::fs::write(build_path, build).unwrap();
    gradle_if_enabled(package_dir, "test");
}

/// An agent whose inputs and outputs reach every shape the Kotlin generators
/// support.
fn rich_agent(mode: AgentMode) -> AgentTypeSchema {
    let person = def(
        "person",
        SchemaType::record(vec![
            named_field("first-name", SchemaType::string()),
            named_field("class", SchemaType::u8()),
            named_field("age", SchemaType::option(SchemaType::u32())),
            named_field("favourite", ref_to("color")),
            named_field("tags", SchemaType::list(SchemaType::string())),
            named_field(
                "pair",
                SchemaType::tuple(vec![SchemaType::s32(), SchemaType::s64()]),
            ),
            named_field("solo", SchemaType::tuple(vec![SchemaType::string()])),
            named_field(
                "quad",
                SchemaType::tuple(vec![
                    SchemaType::bool(),
                    SchemaType::f32(),
                    SchemaType::f64(),
                    SchemaType::char(),
                ]),
            ),
            named_field(
                "attrs",
                SchemaType::map(SchemaType::string(), SchemaType::u64()),
            ),
            named_field(
                "outcome",
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(SchemaType::u16())),
                    err: Some(Box::new(SchemaType::string())),
                }),
            ),
            named_field(
                "unit-outcome",
                SchemaType::result(ResultSpec {
                    ok: None,
                    err: None,
                }),
            ),
            named_field("id", SchemaType::uuid()),
            named_field("at", SchemaType::datetime()),
            named_field("ttl", SchemaType::duration()),
            named_field("grid", SchemaType::fixed_list(SchemaType::s8(), 3)),
            named_field("flags", ref_to("perms")),
            named_field("where", ref_to("location")),
            named_field("nothing", ref_to("empty")),
        ]),
    );
    let color = def(
        "color",
        SchemaType::r#enum(vec!["red".into(), "name".into(), "ordinal".into()]),
    );
    let location = def(
        "location",
        SchemaType::variant(vec![
            variant_case("home", Some(SchemaType::string())),
            variant_case("in", Some(SchemaType::list(SchemaType::string()))),
            variant_case("unknown", None),
        ]),
    );
    let perms = def(
        "perms",
        SchemaType::flags(vec!["read".into(), "write".into()]),
    );
    let empty = def("empty", SchemaType::record(vec![]));
    agent(
        "rich-agent",
        "rust",
        vec![field("who", ref_to("person"))],
        vec![
            method(
                "go",
                vec![field("to", ref_to("location"))],
                Some(SchemaType::string()),
            ),
            method("ping", vec![], None),
            method(
                "describe",
                vec![field("p", ref_to("person")), field("n", SchemaType::s32())],
                Some(ref_to("person")),
            ),
            method("class", vec![field("in", SchemaType::string())], None),
            method(
                "attach",
                vec![
                    field(
                        "notes",
                        unstructured_text_schema_type(TextRestrictions {
                            languages: Some(vec!["en".to_string(), "de".to_string()]),
                            ..Default::default()
                        }),
                    ),
                    field(
                        "blob",
                        unstructured_binary_schema_type(BinaryRestrictions {
                            mime_types: Some(vec!["image/png".to_string()]),
                            ..Default::default()
                        }),
                    ),
                ],
                Some(unstructured_text_schema_type(TextRestrictions::default())),
            ),
        ],
        vec![person, color, location, perms, empty],
        mode,
    )
}

/// An agent whose method carries the three rich scalars whose spec rides in the lowered grammar:
/// a `path` with its direction and kind, a `url` with its schemes, and a `quantity` with its base
/// unit and suffixes. The generated codecs compile the unit check, which only a compile sees.
fn rich_scalar_agent(mode: AgentMode) -> AgentTypeSchema {
    use golem_common::schema::schema_type::{
        PathDirection, PathKind, PathSpec, QuantitySpec, UrlRestrictions,
    };
    let distance = || {
        SchemaType::quantity(QuantitySpec {
            base_unit: "m".to_string(),
            allowed_suffixes: vec!["km".to_string(), "mi".to_string()],
            min: None,
            max: None,
        })
    };
    agent(
        "rich-scalar-agent",
        "rust",
        vec![field(
            "source",
            SchemaType::url(UrlRestrictions {
                allowed_schemes: Some(vec!["https".to_string()]),
                allowed_hosts: None,
            }),
        )],
        vec![method(
            "measure",
            vec![
                field(
                    "file",
                    SchemaType::path(PathSpec {
                        direction: PathDirection::Input,
                        kind: PathKind::File,
                        allowed_mime_types: Some(vec!["image/png".to_string()]),
                        allowed_extensions: None,
                    }),
                ),
                field("travelled", distance()),
            ],
            Some(distance()),
        )],
        vec![],
        mode,
    )
}

/// An agent whose methods reach every stream shape the external bridge accepts.
/// `stream<string>` travels as JSON schema values and `stream<u8>` travels as
/// packed bytes, so both lanes are compiled. The remaining methods wrap a stream
/// in an option, a list, a record and another stream, and carry an element whose
/// value needs a generated codec inside the encode lambda.
fn streaming_agent(mode: AgentMode) -> AgentTypeSchema {
    let stream = |item| SchemaType::stream(Some(item));
    let item = def(
        "item",
        SchemaType::record(vec![
            named_field("label", SchemaType::string()),
            named_field("size", SchemaType::u32()),
        ]),
    );
    let bundle = def(
        "bundle",
        SchemaType::record(vec![
            named_field("label", SchemaType::string()),
            named_field("items", stream(ref_to("item"))),
        ]),
    );
    let tree = def(
        "tree",
        SchemaType::record(vec![
            named_field("label", SchemaType::string()),
            named_field("children", stream(ref_to("tree"))),
        ]),
    );
    agent(
        "streaming-agent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![
            method(
                "echo",
                vec![field("input", stream(SchemaType::string()))],
                Some(stream(SchemaType::string())),
            ),
            method(
                "pack",
                vec![field("input", stream(SchemaType::u8()))],
                Some(stream(SchemaType::u8())),
            ),
            method(
                "nested",
                vec![field("input", stream(stream(SchemaType::string())))],
                Some(stream(stream(SchemaType::string()))),
            ),
            method(
                "maybe",
                vec![field(
                    "input",
                    SchemaType::option(stream(SchemaType::string())),
                )],
                Some(SchemaType::option(stream(SchemaType::string()))),
            ),
            method(
                "many",
                vec![field(
                    "input",
                    SchemaType::list(stream(SchemaType::string())),
                )],
                Some(SchemaType::list(stream(SchemaType::string()))),
            ),
            method(
                "exchange",
                vec![field("input", ref_to("bundle"))],
                Some(ref_to("bundle")),
            ),
            method(
                "ship",
                vec![field("input", stream(ref_to("bundle")))],
                Some(stream(ref_to("bundle"))),
            ),
            method(
                "grow",
                vec![field("input", ref_to("tree"))],
                Some(ref_to("tree")),
            ),
            method(
                "records",
                vec![field("input", stream(ref_to("item")))],
                Some(stream(ref_to("item"))),
            ),
            method(
                "entries",
                vec![field(
                    "input",
                    stream(SchemaType::map(SchemaType::string(), SchemaType::u32())),
                )],
                Some(stream(SchemaType::map(
                    SchemaType::string(),
                    SchemaType::u32(),
                ))),
            ),
            method("status", vec![], Some(SchemaType::string())),
        ],
        vec![item, bundle, tree],
        mode,
    )
}

#[test]
fn guest_client_project_has_the_expected_layout() {
    let pkg = GeneratedPackage::new(
        rich_agent(AgentMode::Durable),
        KotlinBridgeMode::GuestWasmRpc,
    );
    let source =
        pkg.source("src/wasmWasiMain/kotlin/golem/bridge/client/rich_agent/RichAgentClient.kt");
    assert!(source.contains("object RichAgentClient"));
    assert!(source.contains("class RichAgentRemote"));
    assert!(pkg.package_dir().join("build.gradle.kts").exists());
    assert!(pkg.package_dir().join("settings.gradle.kts").exists());
}

#[test]
fn guest_rich_agent_compiles() {
    let pkg = GeneratedPackage::new(
        rich_agent(AgentMode::Durable),
        KotlinBridgeMode::GuestWasmRpc,
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlinWasmWasi");
}

#[test]
fn guest_ephemeral_agent_compiles() {
    let pkg = GeneratedPackage::new(
        rich_agent(AgentMode::Ephemeral),
        KotlinBridgeMode::GuestWasmRpc,
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlinWasmWasi");
}

#[test]
fn guest_single_agent_compiles() {
    let pkg = GeneratedPackage::new(
        single_agent_wrapper_types()[0].clone(),
        KotlinBridgeMode::GuestWasmRpc,
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlinWasmWasi");
}

#[test]
fn guest_multi_agent_types_compile() {
    for agent_type in multi_agent_wrapper_2_types() {
        let pkg = GeneratedPackage::new(agent_type, KotlinBridgeMode::GuestWasmRpc);
        gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlinWasmWasi");
    }
}

#[test]
fn external_client_project_has_the_expected_layout() {
    let pkg = GeneratedPackage::new(
        rich_agent(AgentMode::Durable),
        KotlinBridgeMode::ExternalRest,
    );
    let source = pkg.source("src/main/kotlin/golem/bridge/client/rich_agent/RichAgentClient.kt");
    assert!(source.contains("object RichAgentClient"));
    assert!(source.contains("golem.bridge.runtime.Bridge.createAgent"));
    let build = pkg.source("build.gradle.kts");
    assert!(build.contains("kotlin(\"jvm\")"));
    assert!(
        !build.contains("implementation("),
        "the external project must depend on the JDK only:\n{build}"
    );
    for runtime in [
        "golem/bridge/runtime/Bridge.kt",
        "golem/bridge/runtime/AgentStream.kt",
        "golem/bridge/runtime/StreamSessionProtocol.kt",
        "golem/bridge/runtime/StreamSessionState.kt",
        "golem/bridge/runtime/StreamSession.kt",
        "golem/bridge/runtime/json/Json.kt",
    ] {
        let path = pkg.package_dir().join("src/main/kotlin").join(runtime);
        assert!(path.exists(), "missing runtime source {path}");
    }
}

#[test]
fn external_rich_agent_compiles() {
    let pkg = GeneratedPackage::new(
        rich_agent(AgentMode::Durable),
        KotlinBridgeMode::ExternalRest,
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlin");
}

#[test]
fn guest_rich_scalar_agent_compiles() {
    let pkg = GeneratedPackage::new(
        rich_scalar_agent(AgentMode::Durable),
        KotlinBridgeMode::GuestWasmRpc,
    );
    let source = pkg.source(
        "src/wasmWasiMain/kotlin/golem/bridge/client/rich_scalar_agent/RichScalarAgentClient.kt",
    );
    assert!(
        source.contains("travelled: cloud.golem.schema.Quantity"),
        "{source}"
    );
    assert!(
        source.contains("quantityValue(travelled, \"m\", listOf(\"km\", \"mi\"))"),
        "{source}"
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlinWasmWasi");
}

#[test]
fn external_rich_scalar_agent_compiles() {
    let pkg = GeneratedPackage::new(
        rich_scalar_agent(AgentMode::Durable),
        KotlinBridgeMode::ExternalRest,
    );
    let source = pkg
        .source("src/main/kotlin/golem/bridge/client/rich_scalar_agent/RichScalarAgentClient.kt");
    assert!(
        source.contains("travelled: golem.bridge.runtime.Quantity"),
        "{source}"
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlin");
}

#[test]
fn external_ephemeral_agent_compiles() {
    let pkg = GeneratedPackage::new(
        rich_agent(AgentMode::Ephemeral),
        KotlinBridgeMode::ExternalRest,
    );
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlin");
}

#[test]
fn external_streaming_agent_compiles() {
    let pkg = GeneratedPackage::new(
        streaming_agent(AgentMode::Durable),
        KotlinBridgeMode::ExternalRest,
    );
    let source =
        pkg.source("src/main/kotlin/golem/bridge/client/streaming_agent/StreamingAgentClient.kt");
    assert!(
        source.contains("golem.bridge.runtime.Bridge.invokeStreamingAgent("),
        "{source}"
    );
    for signature in [
        "fun echo(input: golem.bridge.runtime.AgentStream<kotlin.String>): golem.bridge.runtime.AgentStream<kotlin.String>",
        "fun pack(input: golem.bridge.runtime.AgentStream<kotlin.UByte>): golem.bridge.runtime.AgentStream<kotlin.UByte>",
        "fun nested(input: golem.bridge.runtime.AgentStream<golem.bridge.runtime.AgentStream<kotlin.String>>)",
        "fun maybe(input: golem.bridge.runtime.AgentStream<kotlin.String>?): golem.bridge.runtime.AgentStream<kotlin.String>?",
        "fun many(input: kotlin.collections.List<golem.bridge.runtime.AgentStream<kotlin.String>>)",
        "fun ship(input: golem.bridge.runtime.AgentStream<golem.bridge.client.streaming_agent.Bundle>)",
        "fun records(input: golem.bridge.runtime.AgentStream<golem.bridge.client.streaming_agent.Item>)",
        "fun entries(input: golem.bridge.runtime.AgentStream<kotlin.collections.Map<kotlin.String, kotlin.UInt>>)",
    ] {
        assert!(
            source.contains(signature),
            "missing {signature} in:\n{source}"
        );
    }
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlin");
}

#[test]
fn external_multi_agent_types_compile() {
    for agent_type in multi_agent_wrapper_2_types() {
        let pkg = GeneratedPackage::new(agent_type, KotlinBridgeMode::ExternalRest);
        gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlin");
    }
}

#[test]
fn the_embedded_external_runtime_passes_its_tests() {
    let pkg = GeneratedPackage::new(
        single_agent_wrapper_types()[0].clone(),
        KotlinBridgeMode::ExternalRest,
    );
    gradle_runtime_tests_if_enabled(pkg.package_dir().as_path());
}

#[test]
fn guest_agent_with_local_config_compiles() {
    let mut agent_type = rich_agent(AgentMode::Durable);
    agent_type.config = vec![
        local_config(vec!["retry", "max-attempts"], SchemaType::u32()),
        local_config(vec!["labels"], SchemaType::list(SchemaType::string())),
    ];
    let pkg = GeneratedPackage::new(agent_type, KotlinBridgeMode::GuestWasmRpc);
    let source =
        pkg.source("src/wasmWasiMain/kotlin/golem/bridge/client/rich_agent/RichAgentClient.kt");
    assert!(source.contains("fun getWithConfig("));
    gradle_if_enabled(pkg.package_dir().as_path(), "compileKotlinWasmWasi");
}
