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
use golem_cli::bridge_gen::moonbit::{MoonBitBridgeGenerator, MoonBitTypeName};
use golem_cli::bridge_gen::type_naming::TypeName;
use golem_cli::model::GuestLanguage;
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::schema_type::{BinaryRestrictions, TextRestrictions, VariantCaseType};
use golem_common::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_text_schema_type,
};
use golem_common::schema::{AgentTypeSchema, ResultSpec, Role, SchemaType};
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
    assert!(mod_json.contains("\"name\": \"golem/bridge\""));
    assert!(mod_json.contains("\"moonbitlang/async\": \"0.18.1\""));

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
    assert!(client_pkg.contains("\"golem/bridge/runtime\" @runtime"));

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
    assert!(client.contains("pub async fn EphemeralAgent::get_phantom("));
    assert!(client.contains("pub async fn EphemeralAgent::new_phantom("));
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
    assert!(client.contains("pub async fn EphemeralConfigAgent::get_phantom_with_config("));
    assert!(client.contains("pub async fn EphemeralConfigAgent::new_phantom_with_config("));
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
