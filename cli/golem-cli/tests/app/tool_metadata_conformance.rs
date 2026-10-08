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

use super::{TestContext, cmd, flag};
use crate::workspace_path;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::schema::tool::{
    Constraint, FlagShape, FlagSpec, OptionShape, OptionSpec, Ref, Repetition, Tool,
};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use test_r::{test, timeout};

const APP_NAME: &str = "rich-tool-metadata";

fn schema_type_json(ty: &SchemaType, names: &BTreeMap<String, String>) -> Value {
    match ty {
        SchemaType::Ref { id, .. } => json!({ "kind": "ref", "id": names[&id.to_string()] }),
        SchemaType::Bool { .. } => json!({ "kind": "bool" }),
        SchemaType::S64 { .. } => json!({ "kind": "s64" }),
        SchemaType::U32 { .. } => json!({ "kind": "u32" }),
        SchemaType::U64 { .. } => json!({ "kind": "u64" }),
        SchemaType::String { .. } => json!({ "kind": "string" }),
        SchemaType::Record { fields, .. } => json!({
            "kind": "record",
            "fields": fields.iter().map(|field| json!({
                "name": field.name,
                "type": schema_type_json(&field.body, names),
            })).collect::<Vec<_>>(),
        }),
        SchemaType::Enum { cases, .. } => json!({ "kind": "enum", "cases": cases }),
        SchemaType::List { element, .. } => {
            json!({ "kind": "list", "item": schema_type_json(element, names) })
        }
        SchemaType::Map { key, value, .. } => json!({
            "kind": "map",
            "key": schema_type_json(key, names),
            "value": schema_type_json(value, names),
        }),
        SchemaType::Text { restrictions, .. } => json!({
            "kind": "text",
            "restrictions": {
                "minLength": restrictions.min_length,
                "maxLength": restrictions.max_length,
                "regex": restrictions.regex,
            },
        }),
        SchemaType::Path { spec, .. } => json!({
            "kind": "path",
            "direction": serde_json::to_value(spec.direction).unwrap(),
            "pathKind": serde_json::to_value(spec.kind).unwrap(),
            "extensions": spec.allowed_extensions,
        }),
        other => panic!("schema kind not used by the rich fixture: {other:?}"),
    }
}

fn schema_value_json(value: &SchemaValue, ty: &SchemaType, graph: &SchemaGraph) -> Value {
    let ty = match ty {
        SchemaType::Ref { id, .. } => &graph.lookup(id).unwrap().body,
        ty => ty,
    };

    match (value, ty) {
        (SchemaValue::Bool(value), _) => json!(value),
        (SchemaValue::String(value), _) => json!(value),
        (SchemaValue::S64(value), _) => json!(value),
        (SchemaValue::Enum { case }, SchemaType::Enum { cases, .. }) => {
            json!(cases[*case as usize])
        }
        (SchemaValue::List { elements }, SchemaType::List { element, .. }) => Value::Array(
            elements
                .iter()
                .map(|value| schema_value_json(value, element, graph))
                .collect(),
        ),
        (SchemaValue::Map { entries }, SchemaType::Map { key, value, .. }) => Value::Object(
            entries
                .iter()
                .map(|(entry_key, entry_value)| {
                    let Value::String(entry_key) = schema_value_json(entry_key, key, graph) else {
                        panic!("rich fixture map key is not a string")
                    };
                    (entry_key, schema_value_json(entry_value, value, graph))
                })
                .collect(),
        ),
        _ => panic!("value does not match rich fixture schema: {value:?} / {ty:?}"),
    }
}

fn repetition_json(repetition: &Repetition) -> Value {
    match repetition {
        Repetition::Repeated => json!({ "kind": "repeated" }),
        Repetition::Delimited(delimiter) => {
            json!({ "kind": "delimited", "delimiter": delimiter.to_string() })
        }
        Repetition::Either(delimiter) => {
            json!({ "kind": "either", "delimiter": delimiter.to_string() })
        }
    }
}

fn option_shape_json(shape: &OptionShape, names: &BTreeMap<String, String>) -> Value {
    match shape {
        OptionShape::Scalar(ty) => {
            json!({ "kind": "scalar", "type": schema_type_json(ty, names) })
        }
        OptionShape::OptionalScalar(ty) => {
            json!({ "kind": "optional-scalar", "type": schema_type_json(ty, names) })
        }
        OptionShape::RepeatableList(shape) => json!({
            "kind": "repeatable-list",
            "repetition": repetition_json(&shape.repetition),
            "itemType": schema_type_json(&shape.item_type, names),
        }),
        OptionShape::RepeatableMap(shape) => json!({
            "kind": "repeatable-map",
            "repetition": repetition_json(&shape.repetition),
            "mapType": schema_type_json(&shape.map_type, names),
            "duplicateKeyPolicy": serde_json::to_value(shape.duplicate_key_policy).unwrap(),
        }),
    }
}

fn option_json(
    option: &OptionSpec,
    graph: &SchemaGraph,
    names: &BTreeMap<String, String>,
) -> Value {
    let value_type = match &option.shape {
        OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => ty,
        OptionShape::RepeatableList(shape) => {
            return json!({
                "long": option.long,
                "short": option.short.map(|value| value.to_string()),
                "aliases": option.aliases,
                "valueName": option.value_name,
                "shape": option_shape_json(&option.shape, names),
                "default": option.default.as_ref().map(|value| schema_value_json(
                    value,
                    &SchemaType::List {
                        element: Box::new(shape.item_type.clone()),
                        metadata: Default::default(),
                    },
                    graph,
                )),
                "required": option.required,
                "envVar": option.env_var,
                "doc": option.doc,
            });
        }
        OptionShape::RepeatableMap(shape) => &shape.map_type,
    };

    json!({
        "long": option.long,
        "short": option.short.map(|value| value.to_string()),
        "aliases": option.aliases,
        "valueName": option.value_name,
        "shape": option_shape_json(&option.shape, names),
        "default": option.default.as_ref().map(|value| schema_value_json(value, value_type, graph)),
        "required": option.required,
        "envVar": option.env_var,
        "doc": option.doc,
    })
}

fn flag_json(flag: &FlagSpec) -> Value {
    let shape = match flag.shape {
        FlagShape::BoolFlag(shape) => json!({
            "kind": "bool-flag",
            "default": shape.default,
            "negatable": shape.negatable,
        }),
        FlagShape::CountFlag(max) => json!({ "kind": "count-flag", "max": max }),
    };
    json!({
        "long": flag.long,
        "short": flag.short.map(|value| value.to_string()),
        "aliases": flag.aliases,
        "shape": shape,
        "envVar": flag.env_var,
        "doc": flag.doc,
    })
}

fn reference_json(reference: &Ref, graph: &SchemaGraph) -> Value {
    match reference {
        Ref::Present(name) => json!({ "kind": "present", "name": name }),
        Ref::ValueIs(value) => {
            let ty = match value.name.as_str() {
                "profile" => SchemaType::r#enum(vec!["debug".into(), "release".into()]),
                "format" => SchemaType::r#enum(vec!["json".into(), "text".into()]),
                name => panic!("value-is type not used by rich fixture: {name}"),
            };
            json!({
                "kind": "value-is",
                "name": value.name,
                "value": schema_value_json(&value.value, &ty, graph),
            })
        }
    }
}

fn constraint_json(constraint: &Constraint, graph: &SchemaGraph) -> Value {
    match constraint {
        Constraint::RequiresAll(refs) => json!({
            "kind": "requires-all",
            "refs": refs.iter().map(|value| reference_json(value, graph)).collect::<Vec<_>>(),
        }),
        Constraint::Implies(value) => json!({
            "kind": "implies",
            "lhsQuantifier": serde_json::to_value(value.lhs_quant).unwrap(),
            "lhs": value.lhs.iter().map(|item| reference_json(item, graph)).collect::<Vec<_>>(),
            "rhsQuantifier": serde_json::to_value(value.rhs_quant).unwrap(),
            "rhs": value.rhs.iter().map(|item| reference_json(item, graph)).collect::<Vec<_>>(),
        }),
        Constraint::Forbids(value) => json!({
            "kind": "forbids",
            "lhsQuantifier": serde_json::to_value(value.lhs_quant).unwrap(),
            "lhs": value.lhs.iter().map(|item| reference_json(item, graph)).collect::<Vec<_>>(),
            "rhs": value.rhs.iter().map(|item| reference_json(item, graph)).collect::<Vec<_>>(),
        }),
        other => panic!("constraint not used by the rich fixture: {other:?}"),
    }
}

fn command_json(index: usize, tool: &Tool, names: &BTreeMap<String, String>) -> Value {
    let node = &tool.commands.nodes[index];
    let globals = json!({
        "options": node.globals.options.iter().map(|value| option_json(value, &tool.schema, names)).collect::<Vec<_>>(),
        "flags": node.globals.flags.iter().map(flag_json).collect::<Vec<_>>(),
    });
    let body = node.body.as_ref().map(|body| json!({
        "positionals": {
            "fixed": body.positionals.fixed.iter().map(|value| json!({
                "name": value.name,
                "valueName": value.value_name,
                "type": schema_type_json(&value.type_, names),
                "default": value.default.as_ref().map(|default| schema_value_json(default, &value.type_, &tool.schema)),
                "required": value.required,
                "acceptsStdio": value.accepts_stdio,
                "doc": value.doc,
            })).collect::<Vec<_>>(),
            "tail": body.positionals.tail.as_ref().map(|value| json!({
                "name": value.name,
                "valueName": value.value_name,
                "itemType": schema_type_json(&value.item_type, names),
                "min": value.min,
                "max": value.max,
                "separator": value.separator,
                "verbatim": value.verbatim,
                "acceptsStdio": value.accepts_stdio,
                "doc": value.doc,
            })),
        },
        "options": body.options.iter().map(|value| option_json(value, &tool.schema, names)).collect::<Vec<_>>(),
        "flags": body.flags.iter().map(flag_json).collect::<Vec<_>>(),
        "constraints": body.constraints.iter().map(|value| constraint_json(value, &tool.schema)).collect::<Vec<_>>(),
        "stdin": body.stdin,
        "stdout": body.stdout,
        "stderr": body.stderr,
        "result": body.result.as_ref().map(|value| json!({
            "type": schema_type_json(&value.type_, names),
            "formatters": value.formatters,
            "defaultFormatter": value.default_formatter,
            "doc": value.doc,
        })),
        "errors": body.errors.iter().map(|value| json!({
            "name": value.name,
            "kind": serde_json::to_value(value.kind).unwrap(),
            "exitCode": value.exit_code,
            "payload": value.payload.as_ref().map(|payload| schema_type_json(payload, names)),
            "doc": value.doc,
        })).collect::<Vec<_>>(),
        "annotations": body.annotations.as_ref().map(|value| json!({
            "readOnly": value.read_only,
            "destructive": value.destructive,
            "idempotent": value.idempotent,
            "openWorld": value.open_world,
        })),
    }));

    json!({
        "name": node.name,
        "aliases": node.aliases,
        "doc": node.doc,
        "globals": globals,
        "body": body,
        "subcommands": node.subcommands.iter().map(|child| command_json(child.as_usize().unwrap(), tool, names)).collect::<Vec<_>>(),
    })
}

fn canonical_tool_json(tool: &Tool, expected: &Value) -> Value {
    let mut names = tool
        .schema
        .defs
        .iter()
        .filter_map(|definition| {
            definition
                .name
                .clone()
                .map(|name| (definition.id.to_string(), name))
        })
        .collect::<BTreeMap<_, _>>();
    let expected_definitions = expected["schemaDefinitions"].as_object().unwrap();
    for definition in &tool.schema.defs {
        if !names.contains_key(&definition.id.to_string()) {
            let body = schema_type_json(&definition.body, &names);
            let name = expected_definitions
                .iter()
                .find_map(|(name, expected_body)| (expected_body == &body).then(|| name.clone()))
                .unwrap_or_else(|| panic!("unmatched unnamed rich fixture definition: {body}"));
            names.insert(definition.id.to_string(), name);
        }
    }
    let definitions = tool
        .schema
        .defs
        .iter()
        .map(|definition| {
            (
                names[&definition.id.to_string()].clone(),
                schema_type_json(&definition.body, &names),
            )
        })
        .collect::<serde_json::Map<_, _>>();

    json!({
        "version": tool.version,
        "requiresFilesystem": tool.requires_filesystem,
        "name": tool.name().unwrap(),
        "schemaDefinitions": definitions,
        "commands": command_json(0, tool, &names),
    })
}

fn mismatch_paths(expected: &Value, actual: &Value) -> Vec<String> {
    fn collect(expected: &Value, actual: &Value, path: &str, mismatches: &mut Vec<String>) {
        match (expected, actual) {
            (Value::Object(expected), Value::Object(actual)) => {
                let keys = expected
                    .keys()
                    .chain(actual.keys())
                    .collect::<BTreeSet<_>>();
                for key in keys {
                    let child_path = format!("{path}.{key}");
                    match (expected.get(key), actual.get(key)) {
                        (Some(expected), Some(actual)) => {
                            collect(expected, actual, &child_path, mismatches)
                        }
                        _ => mismatches.push(child_path),
                    }
                }
            }
            (Value::Array(expected), Value::Array(actual)) => {
                for index in 0..expected.len().max(actual.len()) {
                    let child_path = format!("{path}[{index}]");
                    match (expected.get(index), actual.get(index)) {
                        (Some(expected), Some(actual)) => {
                            collect(expected, actual, &child_path, mismatches)
                        }
                        _ => mismatches.push(child_path),
                    }
                }
            }
            _ if expected != actual => mismatches.push(path.to_string()),
            _ => {}
        }
    }

    let mut mismatches = Vec::new();
    collect(expected, actual, "$.tool", &mut mismatches);
    mismatches
}

fn assert_matches_contract(language: &str, expected: &Value, actual: &Value) {
    let mismatches = mismatch_paths(expected, actual);
    assert!(
        mismatches.is_empty(),
        "{language} rich tool metadata mismatched at:\n{}",
        mismatches.join("\n")
    );
}

fn canonicalize_inlined_definitions(expected: &Value, actual: &mut Value) {
    fn replace(
        value: &mut Value,
        definitions: &serde_json::Map<String, Value>,
        referenced: &mut BTreeSet<String>,
    ) {
        if let Some((name, _)) = definitions
            .iter()
            .find(|(_, definition)| value == *definition)
        {
            referenced.insert(name.clone());
            *value = json!({ "kind": "ref", "id": name });
            return;
        }

        match value {
            Value::Array(values) => {
                for value in values {
                    replace(value, definitions, referenced);
                }
            }
            Value::Object(object) => {
                for value in object.values_mut() {
                    replace(value, definitions, referenced);
                }
            }
            _ => {}
        }
    }

    let definitions = expected["schemaDefinitions"].as_object().unwrap();
    let actual_object = actual.as_object_mut().unwrap();
    let mut referenced = BTreeSet::new();
    for (name, value) in actual_object.iter_mut() {
        if name != "schemaDefinitions" {
            replace(value, definitions, &mut referenced);
        }
    }
    let actual_definitions = actual_object["schemaDefinitions"].as_object_mut().unwrap();
    for name in referenced {
        actual_definitions
            .entry(name.clone())
            .or_insert_with(|| definitions[&name].clone());
    }
}

fn rust_fixture_source() -> String {
    let source = std::fs::read_to_string(
        workspace_path().join("sdks/rust/golem-rust/tests/rich_tool_conformance.rs"),
    )
    .unwrap();
    let body = source
        .split_once("mod fixture {")
        .unwrap()
        .1
        .split_once("    fn type_json(")
        .unwrap()
        .0;
    let body = body
        .lines()
        .filter(|line| {
            !line.contains("use serde_json::{Value, json};") && !line.contains("use test_r::test;")
        })
        .map(|line| line.strip_prefix("    ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"{body}

use golem_rust::tool_implementation;

struct ArtifactImpl;

#[tool_implementation]
impl Artifact for ArtifactImpl {{
    fn render(&self, _region: Region, _trace: bool, _profile: Profile) -> RenderSubtree {{
        RenderSubtree
    }}
}}
"#
    )
}

fn typescript_fixture_source() -> String {
    let source = std::fs::read_to_string(
        workspace_path().join("sdks/ts/packages/golem-ts-sdk/tests/components/gol-40-rich-tool.ts"),
    )
    .unwrap();
    let definition = source.split_once("const wire =").unwrap().0.replace(
        "import { typedSchemaValueToWit } from '../../src/internal/schema-model';\n",
        "",
    );
    format!(
        r#"{definition}
artifactDefinition.implement({{
  render: command(
    () => ok({{
      artifactId: 18446744073709551614n,
      digest: 'deadbeef',
      labels: new Map([['tier', 'gold'], ['team', 'runtime']]),
      warnings: ['unsigned metadata'],
    }}),
    {{ status: () => ok('ready') }},
  ),
}});
"#
    )
}

fn moonbit_fixture_source() -> String {
    let source = std::fs::read_to_string(
        workspace_path()
            .join("sdks/moonbit/golem_sdk_tools/test-data/gol40-conformance/artifact.mbt"),
    )
    .unwrap();
    format!(
        r#"{source}

///|
fn Option::drop_sync(self : @asyncCore.Stream[Byte]?) -> Unit {{
  match self {{
    Some(stream) => stream.drop_sync()
    None => ()
  }}
}}

///|
fn main {{}}
"#
    )
}

fn scala_fixture_source() -> String {
    let source = std::fs::read_to_string(
        workspace_path()
            .join("sdks/scala/core/js/src/test/scala/golem/runtime/tool/ToolTestFixtures.scala"),
    )
    .unwrap();
    format!(
        r#"{source}

object Gol40RichMetadataRegistration {{
  @scala.scalajs.js.annotation.JSExportTopLevel("__golemGol40RichMetadataRegistration")
  val register: Unit = ToolRegistry.register(ToolTestFixtures.gol40RichTool)
}}
"#
    )
}

fn write_fixture_sources(ctx: &TestContext) {
    std::fs::write(
        ctx.cwd_path_join("rust-fixture/src/counter_agent.rs"),
        rust_fixture_source(),
    )
    .unwrap();
    std::fs::write(
        ctx.cwd_path_join("typescript-fixture/src/main.ts"),
        typescript_fixture_source(),
    )
    .unwrap();

    let moonbit_dir = ctx.cwd_path_join("moonbit-fixture");
    for entry in std::fs::read_dir(&moonbit_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "mbt") {
            std::fs::remove_file(path).unwrap();
        }
    }
    std::fs::write(moonbit_dir.join("artifact.mbt"), moonbit_fixture_source()).unwrap();

    let scala_source = ctx.cwd_path_join("scala-fixture/src/main/scala");
    std::fs::remove_dir_all(&scala_source).unwrap();
    let registry_dir = scala_source.join("golem/runtime/tool");
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::write(
        registry_dir.join("ToolTestFixtures.scala"),
        scala_fixture_source(),
    )
    .unwrap();
    let capability_dir = scala_source.join("scala_fixture");
    std::fs::create_dir_all(&capability_dir).unwrap();
    std::fs::write(
        capability_dir.join("MetadataCapability.scala"),
        r#"package scala_fixture

import golem.runtime.annotations.{toolDefinition, toolImplementation}

@toolDefinition(name = "metadata-capability")
trait MetadataCapability {
  def ping(): String
}

@toolImplementation()
final class MetadataCapabilityImpl extends MetadataCapability {
  def ping(): String = "ok"
}
"#,
    )
    .unwrap();
}

async fn extracted_tool(ctx: &TestContext, component_name: &str, wasm_path: &str) -> Tool {
    let wasm_path = ctx.cwd_path_join(wasm_path);
    let metadata: ExtractedComponentMetadata =
        golem_cli::model::agent::extraction::extract_component_metadata(&wasm_path, false)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "failed to extract metadata for {component_name} from {}: {error:#}",
                    wasm_path.display()
                )
            });
    metadata
        .tools
        .into_iter()
        .find(|tool| tool.name() == Some("artifact"))
        .unwrap_or_else(|| panic!("missing artifact tool for {component_name}"))
}

#[test]
#[timeout("30 minutes")]
async fn rich_tool_metadata_matches_across_rust_typescript_scala_and_moonbit() {
    let mut ctx = TestContext::new();
    std::fs::create_dir_all(ctx.cwd_path_join(APP_NAME)).unwrap();
    ctx.cd(APP_NAME);

    for (template, component_name) in [
        ("rust", "rich-tool-metadata:rust-fixture"),
        ("ts", "rich-tool-metadata:typescript-fixture"),
        ("scala", "rich-tool-metadata:scala-fixture"),
        ("moonbit", "rich-tool-metadata:moonbit-fixture"),
    ] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                component_name,
            ])
            .await;
        assert!(output.success_or_dump());
    }

    write_fixture_sources(&ctx);
    let output = ctx.cli([cmd::BUILD]).await;
    assert!(output.success_or_dump());

    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(
            workspace_path().join("test-data/gol-40/rich-tool-conformance-v1.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = &contract["tool"];

    for (language, component_name, wasm_path) in [
        (
            "Rust",
            "rich-tool-metadata:rust-fixture",
            "golem-temp/agents/rich_tool_metadata_rust_fixture_debug.wasm",
        ),
        (
            "TypeScript",
            "rich-tool-metadata:typescript-fixture",
            "golem-temp/agents/rich_tool_metadata_typescript_fixture.wasm",
        ),
        (
            "Scala",
            "rich-tool-metadata:scala-fixture",
            "golem-temp/agents/rich_tool_metadata_scala_fixture.wasm",
        ),
        (
            "MoonBit",
            "rich-tool-metadata:moonbit-fixture",
            "_build/wasm/debug/rich_tool_metadata_moonbit_fixture.agent.wasm",
        ),
    ] {
        let mut actual = canonical_tool_json(
            &extracted_tool(&ctx, component_name, wasm_path).await,
            expected,
        );
        canonicalize_inlined_definitions(expected, &mut actual);
        assert_matches_contract(language, expected, &actual);
    }
}

#[test]
fn metadata_mismatches_report_exact_order_sensitive_paths() {
    let expected = json!({
        "aliases": ["art"],
        "default": "release",
        "constraints": [{ "kind": "requires-all" }],
        "subcommands": [{ "name": "render" }],
        "errors": [{ "name": "invalid-request", "exitCode": 2 }],
    });
    let actual = json!({
        "aliases": [],
        "default": "debug",
        "constraints": [],
        "subcommands": [{ "name": "status" }],
        "errors": [{ "name": "invalid-request", "exitCode": 70 }],
    });

    assert_eq!(
        mismatch_paths(&expected, &actual),
        vec![
            "$.tool.aliases[0]",
            "$.tool.constraints[0]",
            "$.tool.default",
            "$.tool.errors[0].exitCode",
            "$.tool.subcommands[0].name",
        ]
    );
}

#[test]
fn metadata_object_order_is_semantically_unordered() {
    let expected =
        serde_json::from_str::<Value>(r#"{"A":{"kind":"string"},"B":{"kind":"u64"}}"#).unwrap();
    let actual =
        serde_json::from_str::<Value>(r#"{"B":{"kind":"u64"},"A":{"kind":"string"}}"#).unwrap();
    assert!(mismatch_paths(&expected, &actual).is_empty());
}
