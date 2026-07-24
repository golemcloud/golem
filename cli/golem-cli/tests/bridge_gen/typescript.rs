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
    agent, code_first_snippets_agent_type, def, field, local_config, method,
    multi_agent_wrapper_2_types, ref_to, single_agent_wrapper_types,
};
use crate::bridge_gen::scala::grep_tool;
use crate::bridge_gen::type_naming::test_type_naming;
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::bridge_gen::typescript::tool::TypeScriptToolBridgeGenerator;
use golem_cli::bridge_gen::typescript::{
    TypeScriptBridgeGenerator, TypeScriptBridgeMode, TypeScriptTypeName,
};
use golem_cli::bridge_gen::{
    BridgeGenerator, BridgeMode, bridge_client_directory_name, tool_bridge_client_directory_name,
};
use golem_cli::model::GuestLanguage;
use golem_common::model::agent::AgentMode;
use golem_common::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, PathDirection, PathKind, PathSpec, ResultSpec,
    TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions,
};
use golem_common::schema::schema_type::{NumericBound, NumericRestrictions};
use golem_common::schema::tool::StreamSpec;
use golem_common::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_text_schema_type,
};
use golem_common::schema::{AgentTypeSchema, MetadataEnvelope, SchemaType};
use tempfile::TempDir;
use test_r::{test, test_dep};

struct GeneratedPackage {
    pub dir: TempDir,
}

impl GeneratedPackage {
    pub fn new(agent_type: AgentTypeSchema) -> Self {
        let dir = TempDir::new().unwrap();
        let target_dir = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::remove_dir_all(target_dir).ok();
        generate_and_compile(agent_type, target_dir);
        GeneratedPackage { dir }
    }

    pub fn target_dir(&self) -> &Utf8Path {
        Utf8Path::from_path(self.dir.path()).unwrap()
    }
}

#[test_dep(scope = PerWorker, tagged_as = "single_agent_wrapper_types_1")]
fn ts_single_agent_wrapper_1() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_wrapper_2_types_1")]
fn ts_multi_agent_wrapper_2_types_1() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_wrapper_2_types_2")]
fn ts_multi_agent_wrapper_2_types_2() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[1].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "counter_agent")]
fn ts_counter_agent() -> GeneratedPackage {
    GeneratedPackage::new(agent(
        "CounterAgent",
        "typescript",
        vec![field("name", SchemaType::string())],
        vec![method("increment", vec![], Some(SchemaType::f64()))],
        vec![],
        AgentMode::Durable,
    ))
}

#[test_dep(scope = PerWorker, tagged_as = "ts_code_first_snippets_foo_agent")]
fn ts_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::TypeScript,
        "FooAgent",
    ))
}

fn compile_guest_durable_agent() {
    let mut agent_type = agent(
        "GuestAgent",
        "typescript",
        vec![
            field("name", SchemaType::string()),
            field("labels", SchemaType::list(SchemaType::string())),
            field("nickname", SchemaType::option(SchemaType::string())),
            field(
                "properties",
                SchemaType::map(SchemaType::string(), SchemaType::u32()),
            ),
            field("created-at", SchemaType::datetime()),
            field("large-count", SchemaType::u64()),
        ],
        vec![
            method("read", vec![], Some(SchemaType::u64())),
            method("createdAt", vec![], Some(SchemaType::datetime())),
            method("reset", vec![], None),
        ],
        vec![],
        AgentMode::Durable,
    );
    agent_type.config = vec![local_config(vec!["limits", "maximum"], SchemaType::u64())];
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    std::fs::remove_dir_all(target_dir).ok();
    let package_dir = target_dir.join("guest-agent-guest-client");
    let mut generator = TypeScriptBridgeGenerator::new_with_mode(
        agent_type,
        &package_dir,
        true,
        TypeScriptBridgeMode::GuestWasmRpc,
    )
    .unwrap();
    generator.generate().unwrap();
    std::fs::write(
        package_dir.join("bigint-fixture.ts"),
        r#"import { GuestAgent } from './guest-agent-guest-client';
const client = GuestAgent.newPhantom('example', ['label'], undefined, new Map(), '2026-01-01T00:00:00Z', 18446744073709551615n);
const output: Promise<bigint> = client.read();
void output;
"#,
    )
    .unwrap();
    install_and_build(&package_dir);
}

#[test]
fn single_agent_wrapper_1_compiles(
    #[tagged_as("single_agent_wrapper_types_1")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn multi_agent_wrapper_2_types_1_compiles(
    #[tagged_as("multi_agent_wrapper_2_types_1")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn multi_agent_wrapper_2_types_2_compiles(
    #[tagged_as("multi_agent_wrapper_2_types_2")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn counter_agent_compiles(#[tagged_as("counter_agent")] _pkg: &GeneratedPackage) {}

#[test]
fn code_first_snippets_ts_foo_agent_compiles(
    #[tagged_as("ts_code_first_snippets_foo_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn guest_durable_agent_compiles() {
    compile_guest_durable_agent();
}

#[test]
fn guest_agent_generated_names_do_not_collide_with_schema_names() {
    let dir = TempDir::new().unwrap();
    generate_and_compile_with_mode(
        agent(
            "CollisionAgent",
            "typescript",
            vec![field("phantomId", SchemaType::string())],
            vec![
                method("resolved", vec![], Some(SchemaType::string())),
                method("agentId", vec![], Some(SchemaType::string())),
            ],
            vec![],
            AgentMode::Durable,
        ),
        Utf8Path::from_path(dir.path()).unwrap(),
        TypeScriptBridgeMode::GuestWasmRpc,
    );
}

#[test]
fn static_and_instance_agent_methods_can_share_names() {
    let methods = vec![
        method("get", vec![], Some(SchemaType::string())),
        method("getPhantom", vec![], Some(SchemaType::string())),
        method("newPhantom", vec![], Some(SchemaType::string())),
        method("getWithConfig", vec![], Some(SchemaType::string())),
        method("getPhantomWithConfig", vec![], Some(SchemaType::string())),
        method("newPhantomWithConfig", vec![], Some(SchemaType::string())),
        method("getConfiguration", vec![], Some(SchemaType::string())),
    ];

    for (mode, suffix) in [
        (TypeScriptBridgeMode::ExternalRest, "client"),
        (TypeScriptBridgeMode::GuestWasmRpc, "guest-client"),
    ] {
        let dir = TempDir::new().unwrap();
        let target = Utf8Path::from_path(dir.path()).unwrap();
        let package_dir = target.join(format!("collision-agent-{suffix}"));
        TypeScriptBridgeGenerator::new_with_mode(
            agent(
                "CollisionAgent",
                "typescript",
                vec![],
                methods.clone(),
                vec![],
                AgentMode::Durable,
            ),
            &package_dir,
            true,
            mode,
        )
        .unwrap()
        .generate()
        .unwrap();

        let source =
            std::fs::read_to_string(package_dir.join(format!("collision-agent-{suffix}.ts")))
                .unwrap();
        for method_name in [
            "get",
            "getPhantom",
            "newPhantom",
            "getWithConfig",
            "getPhantomWithConfig",
            "newPhantomWithConfig",
            "getConfiguration",
        ] {
            assert!(
                source.contains(&format!("  readonly {method_name}:")),
                "missing instance method {method_name} in {mode:?}\n{source}"
            );
        }
        install_and_build(&package_dir);
    }
}

#[test]
fn guest_agent_runtime_import_alias_does_not_collide_with_agent_class() {
    let dir = TempDir::new().unwrap();
    generate_and_compile_with_mode(
        agent(
            "GuestUnstructuredText",
            "typescript",
            vec![],
            vec![],
            vec![],
            AgentMode::Durable,
        ),
        Utf8Path::from_path(dir.path()).unwrap(),
        TypeScriptBridgeMode::GuestWasmRpc,
    );
}

#[test]
fn guest_agent_class_name_does_not_collide_with_reachable_schema_type() {
    let dir = TempDir::new().unwrap();
    generate_and_compile_with_mode(
        agent(
            "Payload",
            "typescript",
            vec![field("value", ref_to("payload"))],
            vec![],
            vec![def(
                "payload",
                SchemaType::record(vec![crate::bridge_gen::fixtures::named_field(
                    "text",
                    SchemaType::string(),
                )]),
            )],
            AgentMode::Durable,
        ),
        Utf8Path::from_path(dir.path()).unwrap(),
        TypeScriptBridgeMode::GuestWasmRpc,
    );
}

#[test]
fn guest_agent_constructor_fields_that_normalize_alike_remain_distinct() {
    let dir = TempDir::new().unwrap();
    generate_and_compile_with_mode(
        agent(
            "ProjectedConstructor",
            "typescript",
            vec![
                field("a-b", SchemaType::string()),
                field("a_b", SchemaType::string()),
                field("record", ref_to("normalized-record")),
                field("flags", ref_to("normalized-flags")),
            ],
            vec![
                method("do-work", vec![], None),
                method("do_work", vec![], None),
            ],
            vec![
                def(
                    "normalized-record",
                    SchemaType::record(vec![
                        crate::bridge_gen::fixtures::named_field("a-b", SchemaType::string()),
                        crate::bridge_gen::fixtures::named_field("a_b", SchemaType::string()),
                    ]),
                ),
                def(
                    "normalized-flags",
                    SchemaType::flags(vec!["a-b".into(), "a_b".into()]),
                ),
            ],
            AgentMode::Durable,
        ),
        Utf8Path::from_path(dir.path()).unwrap(),
        TypeScriptBridgeMode::GuestWasmRpc,
    );
}

#[test]
fn guest_inputs_do_not_shadow_runtime_class_or_codec_bindings() {
    let dir = TempDir::new().unwrap();
    generate_and_compile_with_mode(
        agent(
            "BindingCollisionAgent",
            "typescript",
            vec![
                field("base", SchemaType::string()),
                field("BindingCollisionAgent", SchemaType::string()),
                field("encodePayload", ref_to("payload")),
                field("decodePayload", ref_to("payload")),
            ],
            vec![method(
                "run",
                vec![
                    field("base", SchemaType::string()),
                    field("BindingCollisionAgent", SchemaType::string()),
                    field("encodePayload", ref_to("payload")),
                    field("decodePayload", ref_to("payload")),
                ],
                None,
            )],
            vec![def(
                "payload",
                SchemaType::record(vec![crate::bridge_gen::fixtures::named_field(
                    "value",
                    SchemaType::string(),
                )]),
            )],
            AgentMode::Durable,
        ),
        Utf8Path::from_path(dir.path()).unwrap(),
        TypeScriptBridgeMode::GuestWasmRpc,
    );
}

#[test]
fn external_constructor_and_test_helpers_allocate_all_colliding_names() {
    let mut agent_type = agent(
        "ExternalCollisionAgent",
        "typescript",
        vec![
            field("base", SchemaType::string()),
            field("ExternalCollisionAgent", SchemaType::string()),
            field("encodePayload", ref_to("payload")),
            field("parameters", SchemaType::string()),
            field("phantomId", SchemaType::string()),
            field("agentConfig", SchemaType::string()),
            field("configFooBar", SchemaType::string()),
        ],
        vec![
            method(
                "do-work",
                vec![
                    field("base", SchemaType::string()),
                    field("__json", SchemaType::string()),
                    field("__result", SchemaType::string()),
                ],
                None,
            ),
            method(
                "do_work",
                vec![field("encodePayload", ref_to("payload"))],
                None,
            ),
        ],
        vec![def(
            "payload",
            SchemaType::record(vec![crate::bridge_gen::fixtures::named_field(
                "value",
                SchemaType::string(),
            )]),
        )],
        AgentMode::Durable,
    );
    agent_type.config = vec![
        local_config(vec!["foo-bar"], SchemaType::string()),
        local_config(vec!["foo_bar"], SchemaType::string()),
    ];
    let dir = TempDir::new().unwrap();
    generate_and_compile(agent_type, Utf8Path::from_path(dir.path()).unwrap());
}

#[test]
fn ephemeral_agent_is_a_local_metadata_bearing_proxy() {
    let mut agent_type = agent(
        "EphemeralAgent",
        "typescript",
        vec![field("name", SchemaType::string())],
        vec![method("run", vec![], Some(SchemaType::string()))],
        vec![],
        AgentMode::Ephemeral,
    );
    agent_type.config = vec![local_config(vec!["model"], SchemaType::string())];

    let pkg = GeneratedPackage::new(agent_type);
    let package_dir = generated_package_dir(pkg.target_dir(), "ephemeral-agent");
    let client = std::fs::read_to_string(package_dir.join("ephemeral-agent-client.ts")).unwrap();

    assert!(client.contains("static async newPhantom("));
    assert!(client.contains("static async newPhantomWithConfig("));
    assert!(!client.contains("static async getPhantom("));
    assert!(!client.contains("base.createAgent("));
    assert!(client.contains("readonly #agentConfig: base.AgentConfigEntry[];"));
    assert!(client.contains("config: this.#agentConfig,"));
    assert!(client.contains("base.createEphemeralRemoteMethod"));
}

#[test]
fn ephemeral_agent_method_can_be_named_agent_config() {
    let pkg = GeneratedPackage::new(agent(
        "EphemeralAgentConfig",
        "typescript",
        vec![],
        vec![method("agentConfig", vec![], Some(SchemaType::string()))],
        vec![],
        AgentMode::Ephemeral,
    ));
    let package_dir = generated_package_dir(pkg.target_dir(), "ephemeral-agent-config");
    let client =
        std::fs::read_to_string(package_dir.join("ephemeral-agent-config-client.ts")).unwrap();
    assert!(client.contains("agentConfig: base.EphemeralRemoteMethod"));
    assert!(!client.contains("agentConfig1: base.EphemeralRemoteMethod"));
}

#[test]
fn bridge_tests_schema_value_encoding(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let package_dir = generated_package_dir(pkg.target_dir(), "foo-agent");
    let index_ts = std::fs::read_to_string(package_dir.join("foo-agent-client.ts")).unwrap();
    assert!(index_ts.contains("kind"));
    assert!(index_ts.contains("SchemaValue"));
}

#[test]
fn guest_durable_generation_uses_sdk_runtime_surface() {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    let mut agent_type = agent(
        "GuestAgent",
        "typescript",
        vec![
            field("name", SchemaType::string()),
            field("labels", SchemaType::list(SchemaType::string())),
            field("nickname", SchemaType::option(SchemaType::string())),
            field(
                "properties",
                SchemaType::map(SchemaType::string(), SchemaType::u32()),
            ),
            field("created-at", SchemaType::datetime()),
            field("large-count", SchemaType::u64()),
        ],
        vec![
            method("read", vec![], Some(SchemaType::u64())),
            method("createdAt", vec![], Some(SchemaType::datetime())),
            method("reset", vec![], None),
        ],
        vec![
            def(
                "reachable-config",
                SchemaType::U64 {
                    restrictions: Some(NumericRestrictions {
                        min: Some(NumericBound::Unsigned(1)),
                        max: Some(NumericBound::Unsigned(u64::MAX)),
                        unit: Some("requests".to_string()),
                    }),
                    metadata: MetadataEnvelope::default(),
                },
            ),
            def("unreachable-config", SchemaType::string()),
        ],
        AgentMode::Durable,
    );
    agent_type.config = vec![local_config(
        vec!["limits", "maximum"],
        ref_to("reachable-config"),
    )];
    let mut generator = TypeScriptBridgeGenerator::new_with_mode(
        agent_type,
        target,
        true,
        TypeScriptBridgeMode::GuestWasmRpc,
    )
    .unwrap();
    generator.generate().unwrap();

    let source = std::fs::read_to_string(target.join("guest-agent-guest-client.ts")).unwrap();
    let package = std::fs::read_to_string(target.join("package.json")).unwrap();
    assert!(source.contains("import { bridge as base } from '@golemcloud/golem-ts-sdk'"));
    assert!(package.contains("\"name\": \"guest-agent-guest-client\""));
    assert!(package.contains("@golemcloud/golem-ts-sdk"));
    assert!(!package.contains("@golemcloud/golem-ts-bridge"));
    assert!(!source.contains("configure("));
    assert!(!source.contains("base.createAgent"));
    assert!(source.contains("static get("));
    assert!(source.contains("static getPhantom("));
    assert!(source.contains("static newPhantom("));
    assert!(source.contains("static getWithConfig("));
    assert!(source.contains("const phantomId = base.Uuid.generate();"));
    assert!(source.contains("base.resolveRemoteAgent(\"GuestAgent\""));
    assert!(source.contains("return this.resolved.agentId"));
    assert!(source.contains("invokeAndAwait("));
    assert!(source.contains("abortable(signal: AbortSignal"));
    assert!(source.contains("this.resolved.invoke("));
    assert!(source.contains("this.resolved.schedule("));
    assert!(source.contains("this.resolved.scheduleCancelable("));
    assert!(source.contains("base.typedSchemaValueFromJson("));
    assert!(source.contains("reachable-config"));
    assert!(source.contains("18446744073709551615"));
    assert!(source.contains("requests"));
    assert!(!source.contains("\\\"id\\\":\\\"unreachable-config\\\""));
    assert!(source.contains("Invalid result value: missing result value"));
    assert!(!source.contains("LegacySchemaValue"));
    assert!(!source.contains("toGuestSchemaValue"));
    assert!(!source.contains("fromGuestSchemaValue"));
    assert!(source.contains("{ tag: 'record', fields:"));
    assert!(source.contains("{ tag: 'list', elements:"));
    assert!(source.contains("{ tag: 'map', entries:"));
    assert!(source.contains("{ tag: 'option', value:"));
    assert!(source.contains("base.datetimeFromISOString("));
    assert!(source.contains("base.datetimeToISOString("));
    assert!(source.contains(": bigint"));
    assert!(source.contains("{ tag: 'u64', value:"));
    assert!(!source.contains("{ tag: 'u64', value: BigInt("));
    assert!(source.contains("n.value as bigint"));
    assert!(!source.contains("Number(n.value)"));
}

#[test]
fn guest_sdk_native_shapes_generate_direct_codecs_and_compile() {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    let native_shapes = def(
        "native-shapes",
        SchemaType::record(vec![
            crate::bridge_gen::fixtures::named_field("bool", SchemaType::bool()),
            crate::bridge_gen::fixtures::named_field("s8", SchemaType::s8()),
            crate::bridge_gen::fixtures::named_field("s16", SchemaType::s16()),
            crate::bridge_gen::fixtures::named_field("s32", SchemaType::s32()),
            crate::bridge_gen::fixtures::named_field("s64", SchemaType::s64()),
            crate::bridge_gen::fixtures::named_field("u8", SchemaType::u8()),
            crate::bridge_gen::fixtures::named_field("u16", SchemaType::u16()),
            crate::bridge_gen::fixtures::named_field("u32", SchemaType::u32()),
            crate::bridge_gen::fixtures::named_field("u64", SchemaType::u64()),
            crate::bridge_gen::fixtures::named_field("f32", SchemaType::f32()),
            crate::bridge_gen::fixtures::named_field("f64", SchemaType::f64()),
            crate::bridge_gen::fixtures::named_field("char", SchemaType::char()),
            crate::bridge_gen::fixtures::named_field("string", SchemaType::string()),
            crate::bridge_gen::fixtures::named_field(
                "variant",
                SchemaType::variant(vec![
                    crate::bridge_gen::fixtures::variant_case(
                        "payload",
                        Some(SchemaType::string()),
                    ),
                    crate::bridge_gen::fixtures::variant_case("unit", None),
                ]),
            ),
            crate::bridge_gen::fixtures::named_field(
                "enum",
                SchemaType::r#enum(vec!["red".into(), "blue".into()]),
            ),
            crate::bridge_gen::fixtures::named_field(
                "flags",
                SchemaType::flags(vec!["read".into(), "write".into()]),
            ),
            crate::bridge_gen::fixtures::named_field(
                "tuple",
                SchemaType::tuple(vec![SchemaType::string(), SchemaType::u64()]),
            ),
            crate::bridge_gen::fixtures::named_field(
                "list",
                SchemaType::list(SchemaType::string()),
            ),
            crate::bridge_gen::fixtures::named_field("bytes", SchemaType::list(SchemaType::u8())),
            crate::bridge_gen::fixtures::named_field(
                "fixed-bytes",
                SchemaType::fixed_list(SchemaType::u8(), 4),
            ),
            crate::bridge_gen::fixtures::named_field(
                "map",
                SchemaType::map(SchemaType::string(), SchemaType::u64()),
            ),
            crate::bridge_gen::fixtures::named_field(
                "option",
                SchemaType::option(SchemaType::string()),
            ),
            crate::bridge_gen::fixtures::named_field(
                "result",
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(SchemaType::u64())),
                    err: Some(Box::new(SchemaType::string())),
                }),
            ),
            crate::bridge_gen::fixtures::named_field(
                "union",
                SchemaType::union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "text".into(),
                        body: SchemaType::string(),
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: "text:".into(),
                        },
                        metadata: MetadataEnvelope::default(),
                    }],
                }),
            ),
            crate::bridge_gen::fixtures::named_field(
                "path",
                SchemaType::path(PathSpec {
                    direction: PathDirection::InOut,
                    kind: PathKind::Any,
                    allowed_mime_types: None,
                    allowed_extensions: None,
                }),
            ),
            crate::bridge_gen::fixtures::named_field(
                "url",
                SchemaType::url(UrlRestrictions::default()),
            ),
            crate::bridge_gen::fixtures::named_field("datetime", SchemaType::datetime()),
            crate::bridge_gen::fixtures::named_field("duration", SchemaType::duration()),
            crate::bridge_gen::fixtures::named_field(
                "localized-text",
                unstructured_text_schema_type(TextRestrictions {
                    languages: Some(vec!["en".into(), "de".into()]),
                    ..Default::default()
                }),
            ),
            crate::bridge_gen::fixtures::named_field(
                "png-image",
                unstructured_binary_schema_type(BinaryRestrictions {
                    mime_types: Some(vec!["image/png".into()]),
                    ..Default::default()
                }),
            ),
        ]),
    );
    let agent_type = agent(
        "CodecAgent",
        "typescript",
        vec![field("value", ref_to("native-shapes"))],
        vec![method(
            "roundtrip",
            vec![field("value", ref_to("native-shapes"))],
            Some(ref_to("native-shapes")),
        )],
        vec![native_shapes],
        AgentMode::Durable,
    );
    generate_and_compile_with_mode(agent_type, target, TypeScriptBridgeMode::GuestWasmRpc);

    let source = std::fs::read_to_string(
        target.join("codec-agent-guest-client/codec-agent-guest-client.ts"),
    )
    .unwrap();
    assert!(source.contains("function encodeNativeShapes(value: NativeShapes): base.SchemaValue"));
    assert!(source.contains("function decodeNativeShapes(value: base.SchemaValue): NativeShapes"));
    assert!(source.contains("{ tag: 's64', value:"));
    assert!(source.contains("{ tag: 'u64', value:"));
    assert!(source.contains("n.value as bigint"));
    assert!(source.contains("{ tag: 'record', fields:"));
    assert!(source.contains("{ tag: 'variant', caseIndex:"));
    assert!(source.contains("{ tag: 'enum', caseIndex"));
    assert!(source.contains("{ tag: 'flags', flags:"));
    assert!(source.contains("{ tag: 'tuple', elements:"));
    assert!(source.contains("{ tag: 'list', elements:"));
    assert!(source.contains("{ tag: 'fixed-list', elements:"));
    assert!(source.contains("new Uint8Array(n.elements.map"));
    assert!(source.contains("entries()).map((entry: any) => ({ key:"));
    assert!(source.contains("n.entries.map((entry: any) => ["));
    assert!(!source.contains("map((entry: any) => [ {"));
    assert!(source.contains("{ tag: 'option', value:"));
    assert!(source.contains("{ tag: 'result', result:"));
    assert!(source.contains("{ tag: 'union', unionTag:"));
    assert!(source.contains("n.unionTag"));
    assert!(source.contains("{ tag: 'path', value:"));
    assert!(source.contains("{ tag: 'url', value:"));
    assert!(source.contains("base.datetimeFromISOString("));
    assert!(source.contains("base.datetimeToISOString("));
    assert!(source.contains("{ tag: 'duration', nanoseconds:"));
    assert!(source.contains("base.UnstructuredTextType<['en', 'de']>"));
    assert!(source.contains("base.UnstructuredBinaryType<['image/png']>"));
    assert!(source.contains("payload: { tag: 'text', text: v.val, language: v.languageCode }"));
    assert!(source.contains("payload: { tag: 'binary', bytes: v.val, mimeType: v.mimeType }"));
    assert!(
        source.contains("base.UnstructuredText.fromInline(n.payload.text, n.payload.language)")
    );
    assert!(
        source.contains("base.UnstructuredBinary.fromInline(n.payload.bytes, n.payload.mimeType)")
    );
    assert!(source.contains("throw new Error(`Invalid enum case index ${__i}`)"));
    assert!(!source.contains("{ kind:"));
    assert!(!source.contains("LegacySchemaValue"));
    assert!(!source.contains("toGuestSchemaValue"));
    assert!(!source.contains("fromGuestSchemaValue"));
}

#[test]
fn guest_ephemeral_generation_uses_metadata_runtime_calls() {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    let agent_type = agent(
        "EphemeralGuest",
        "typescript",
        vec![],
        vec![method("run", vec![], Some(SchemaType::string()))],
        vec![],
        AgentMode::Ephemeral,
    );
    let mut generator = TypeScriptBridgeGenerator::new_with_mode(
        agent_type,
        target,
        false,
        TypeScriptBridgeMode::GuestWasmRpc,
    )
    .unwrap();
    generator.generate().unwrap();
    let source = std::fs::read_to_string(target.join("ephemeral-guest-guest-client.ts")).unwrap();
    assert!(source.contains("static newPhantom("));
    assert!(!source.contains("static get("));
    assert!(!source.contains("static getPhantom("));
    assert!(source.contains("invokeAndAwaitWithMetadata("));
    assert!(source.contains("invokeWithMetadata("));
    assert!(source.contains("scheduleWithMetadata("));
    assert!(source.contains("scheduleCancelableWithMetadata("));
    assert!(source.contains("const phantomId = undefined;"));
    assert!(!source.contains("base.Uuid.generate()"));
}

#[test]
fn external_generation_keeps_rest_runtime_and_name() {
    let dir = TempDir::new().unwrap();
    let target = Utf8Path::from_path(dir.path()).unwrap();
    let mut generator = TypeScriptBridgeGenerator::new(
        agent(
            "External",
            "typescript",
            vec![
                field("signed", SchemaType::s64()),
                field("unsigned", SchemaType::u64()),
            ],
            vec![
                method(
                    "signed",
                    vec![field("value", SchemaType::s64())],
                    Some(SchemaType::s64()),
                ),
                method(
                    "unsigned",
                    vec![field("value", SchemaType::u64())],
                    Some(SchemaType::u64()),
                ),
            ],
            vec![],
            AgentMode::Durable,
        ),
        target,
        false,
    )
    .unwrap();
    generator.generate().unwrap();
    let source = std::fs::read_to_string(target.join("external-client.ts")).unwrap();
    assert!(source.contains("@golemcloud/golem-ts-bridge"));
    assert!(source.contains("export function configure("));
    assert!(source.contains("signed: number"));
    assert!(source.contains("unsigned: number"));
    assert!(source.contains("{ kind: 's64', value:"));
    assert!(source.contains("{ kind: 'u64', value:"));
    assert!(source.contains("n.value as number"));
    assert!(!source.contains(": bigint"));
}

#[test]
fn test_type_naming_ts_foo_agent() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::TypeScript, "FooAgent");
}

#[test]
fn test_type_naming_rust_foo_agent_for_ts_bridge() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::Rust, "FooAgent");
}

#[test]
fn guest_tool_client_tree_compiles_and_uses_sdk_native_protocol() {
    let mut tool = grep_tool();
    let root_body = tool.commands.nodes[0].body.as_mut().unwrap();
    root_body.stdin = Some(StreamSpec {
        doc: Default::default(),
        mime: vec![],
        required: false,
    });
    root_body.stdout = Some(StreamSpec {
        doc: Default::default(),
        mime: vec![],
        required: false,
    });
    let package_name = tool_bridge_client_directory_name("grep");
    let dir = TempDir::new().unwrap();
    let package_dir = Utf8Path::from_path(dir.path()).unwrap().join(&package_name);
    let mut generator = TypeScriptToolBridgeGenerator::new(tool, &package_dir, true).unwrap();
    generator.generate().unwrap();

    let source = std::fs::read_to_string(package_dir.join(format!("{package_name}.ts"))).unwrap();
    assert!(source.contains("base.createToolClientRuntime(\"grep\")"));
    assert!(source.contains("invokeAndAwait([], typedInput, stdin)"));
    assert!(source.contains("invokeAndAwait([\"replace\"], typedInput, undefined)"));
    assert!(source.contains("[...this.inherited"));
    assert!(source.contains("{ tag: 'record', fields }"));
    assert!(source.contains("base.typedSchemaValueFromJson("));
    assert!(source.contains("base.splitToolRpcError(error, decodeGrepError)"));
    assert!(
        source.contains("base.typedSchemaValueConforms(expectedResultGraph, invocation.result)")
    );
    assert!(source.contains("base.typedSchemaValueConforms(expectedGraph, typed)"));
    assert!(source.contains("base.disposeToolStdout(invocation.stdout)"));
    assert!(source.contains("tool result did not contain a value"));
    assert!(source.contains("tool result unexpectedly contained a value"));
    assert!(source.contains("stdout?: ToolOutputStream"));
    assert!(source.contains("export type ColorMode"));
    assert!(!source.contains("golem-ts-bridge"));
    assert!(!source.contains("kind: 'record'"));
    assert!(!source.contains("legacy"));
    install_and_build(&package_dir);
}

#[test]
fn guest_tool_with_unstructured_result_compiles() {
    let mut tool = grep_tool();
    tool.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .result
        .as_mut()
        .unwrap()
        .type_ = unstructured_text_schema_type(TextRestrictions {
        languages: Some(vec!["en".into()]),
        ..Default::default()
    });
    let package_name = tool_bridge_client_directory_name("grep");
    let dir = TempDir::new().unwrap();
    let package_dir = Utf8Path::from_path(dir.path()).unwrap().join(&package_name);
    TypeScriptToolBridgeGenerator::new(tool, &package_dir, true)
        .unwrap()
        .generate()
        .unwrap();
    install_and_build(&package_dir);
}

#[test]
fn guest_tool_generation_escapes_identifiers_and_normalizes_precision() {
    let mut tool = grep_tool();
    tool.commands.nodes[0].name = "constructor".to_string();
    tool.commands.nodes[0].globals.options[0].long = "encodeColorMode".to_string();
    tool.commands.nodes[0].globals.flags[0].long = "protocol".to_string();
    let body = tool.commands.nodes[0].body.as_mut().unwrap();
    body.positionals.fixed[0].name = "base".to_string();
    body.positionals.fixed[0].type_ = SchemaType::U64 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::Unsigned(1)),
            max: Some(NumericBound::Unsigned(u64::MAX)),
            unit: None,
        }),
        metadata: MetadataEnvelope::default(),
    };
    body.options[0].long = "decodeConstructorError".to_string();
    body.flags[0].long = "await".to_string();
    let package_name = tool_bridge_client_directory_name("constructor");
    let dir = TempDir::new().unwrap();
    let package_dir = Utf8Path::from_path(dir.path()).unwrap().join(&package_name);
    TypeScriptToolBridgeGenerator::new(tool, &package_dir, true)
        .unwrap()
        .generate()
        .unwrap();
    let source = std::fs::read_to_string(package_dir.join(format!("{package_name}.ts"))).unwrap();
    assert!(source.contains("async constructor1("));
    assert!(source.contains("base1: bigint"));
    assert!(source.contains("protocol1: boolean"));
    assert!(source.contains("decodeConstructorError1: number"));
    assert!(source.contains("await_: number"));
    assert!(source.contains("encodeColorMode1: ColorMode"));
    assert!(source.contains("\\\"18446744073709551615\\\""));
    install_and_build(&package_dir);
}

fn generate_and_compile(agent_type: AgentTypeSchema, target_dir: &Utf8Path) {
    generate_and_compile_with_mode(agent_type, target_dir, TypeScriptBridgeMode::ExternalRest);
}

fn generate_and_compile_with_mode(
    agent_type: AgentTypeSchema,
    target_dir: &Utf8Path,
    mode: TypeScriptBridgeMode,
) {
    let bridge_mode = match mode {
        TypeScriptBridgeMode::ExternalRest => BridgeMode::External,
        TypeScriptBridgeMode::GuestWasmRpc => BridgeMode::Guest,
    };
    let package_name = bridge_client_directory_name(&agent_type.type_name, bridge_mode);
    let package_dir = target_dir.join(package_name);
    let mut generator =
        TypeScriptBridgeGenerator::new_with_mode(agent_type, &package_dir, true, mode).unwrap();
    generator.generate().unwrap();
    install_and_build(&package_dir);
}

fn install_and_build(package_dir: &Utf8Path) {
    assert!(
        std::process::Command::new("npm")
            .arg("install")
            .current_dir(package_dir)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("npm")
            .arg("run")
            .arg("build")
            .current_dir(package_dir)
            .status()
            .unwrap()
            .success()
    );
}

fn generated_package_dir(target_dir: &Utf8Path, package_name: &str) -> Utf8PathBuf {
    target_dir.join(format!("{package_name}-client"))
}
