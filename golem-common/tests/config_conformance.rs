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

use golem_common::model::agent::extraction::extract_agent_type_schemas;
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::config::{
    ConfigCatalogSources, ConfigFieldKind, ConfigFieldSchema, ConfigLayer, ConfigOrigin,
    ConfigSchema, ConfigSourcePlan,
};
use golem_common::schema::render::to_json_value;
use golem_common::schema::validation::is_equivalent_cross_graph;
use golem_common::schema::{SchemaGraph, SchemaType};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use test_r::test;

test_r::enable!();

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Corpus {
    version: u32,
    schemas: BTreeMap<String, MetadataFixture>,
    static_cases: Vec<StaticCase>,
    source_cases: Vec<SourceCase>,
    reference: Reference,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    schema: String,
    agent_type: String,
}

#[derive(Deserialize)]
struct MetadataFixture {
    graph: SchemaGraph,
    declarations: Vec<AgentConfigDeclarationSchema>,
}

#[derive(Deserialize)]
struct StaticCase {
    id: String,
    schema: String,
    #[serde(flatten)]
    inputs: Inputs,
}

#[derive(Deserialize)]
struct SourceCase {
    id: String,
    kind: BindingKind,
    #[serde(rename = "type")]
    value_type: SchemaType,
    #[serde(flatten)]
    inputs: Inputs,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum BindingKind {
    Shared,
    Secret,
}

#[derive(Deserialize)]
struct Inputs {
    catalog: Value,
    layers: Vec<Value>,
    expected: Option<Value>,
    error: Option<String>,
}

fn origin(layer: &str) -> ConfigOrigin {
    ConfigOrigin {
        file: format!("{layer}.yaml").into(),
        path: vec!["config".into(), "local".into()],
        layer: layer.into(),
    }
}

fn resolve(schema: &ConfigSchema, inputs: &Inputs) -> Result<Value, String> {
    let mut catalog = ConfigCatalogSources::default();
    catalog
        .add(
            &serde_json::from_value(inputs.catalog.clone()).map_err(|e| e.to_string())?,
            &origin("catalog"),
        )
        .map_err(|e| e.to_string())?;
    let layers = inputs
        .layers
        .iter()
        .enumerate()
        .map(|(index, value)| ConfigLayer {
            origin: origin(&format!("layer{index}")),
            value: value.clone(),
        })
        .collect::<Vec<_>>();
    let plan = schema
        .resolve(&catalog, &layers, &origin("code"))
        .map_err(|e| e.to_string())?;
    assert_eq!(plan.len(), schema.fields.len());
    let mut values = serde_json::Map::new();
    for (field, plan) in schema.fields.iter().zip(plan) {
        assert_eq!(field.path, plan.path);
        let value = match plan.source {
            ConfigSourcePlan::Static(value) => {
                to_json_value(&schema.graph, &field.value_type, &value)
                    .map_err(|e| e.to_string())?
            }
            ConfigSourcePlan::Shared { catalog_path } => {
                json!({"kind":"shared", "catalogPath":catalog_path})
            }
            ConfigSourcePlan::Secret { catalog_path } => {
                json!({"kind":"secret", "catalogPath":catalog_path})
            }
            ConfigSourcePlan::CapturedAbsence => json!({"kind":"captured-absence"}),
            ConfigSourcePlan::Descriptor(_) => panic!("this corpus has no runtime descriptors"),
        };
        values.insert(field.path.join("."), value);
    }
    Ok(Value::Object(values))
}

fn check(id: &str, inputs: &Inputs, result: Result<Value, String>) {
    match (&inputs.expected, &inputs.error, result) {
        (Some(expected), None, Ok(actual)) => assert_eq!(&actual, expected, "{id}"),
        (None, Some(expected), Err(actual)) => {
            assert!(
                actual.contains(expected),
                "{id}: expected {expected:?}, got {actual:?}"
            )
        }
        (_, _, result) => panic!("{id}: invalid expectation or unexpected result: {result:?}"),
    }
}

#[test]
fn common_config_corpus() {
    let corpus: Corpus =
        serde_json::from_str(include_str!("../../test-data/config-conformance/v1.json")).unwrap();
    assert_eq!(corpus.version, 1);
    let mut ids = HashSet::new();
    for case in &corpus.static_cases {
        assert!(ids.insert(&case.id), "duplicate case {}", case.id);
        let fixture = &corpus.schemas[&case.schema];
        let result =
            ConfigSchema::from_declarations(&fixture.graph, &fixture.declarations, &origin("code"))
                .map_err(|e| e.to_string())
                .and_then(|schema| resolve(&schema, &case.inputs));
        check(&case.id, &case.inputs, result);
    }
    for case in &corpus.source_cases {
        assert!(ids.insert(&case.id), "duplicate case {}", case.id);
        let schema = ConfigSchema {
            graph: SchemaGraph::empty(),
            descriptors: BTreeMap::new(),
            fields: vec![ConfigFieldSchema {
                path: vec!["setting".into()],
                kind: match case.kind {
                    BindingKind::Shared => ConfigFieldKind::Shared,
                    BindingKind::Secret => ConfigFieldKind::Secret,
                },
                value_type: case.value_type.clone(),
                default: None,
            }],
        };
        let result = resolve(&schema, &case.inputs).map(|value| value["setting"].clone());
        check(&case.id, &case.inputs, result);
    }
}

#[test]
async fn static_config_reference_exports_corpus_metadata() -> anyhow::Result<()> {
    check_reference_metadata("../test-components/golem_it_agent_sdk_rust_release.wasm").await
}

#[test]
async fn ts_static_config_exports_corpus_metadata() -> anyhow::Result<()> {
    check_reference_metadata("../test-components/golem_it_agent_sdk_ts.wasm").await
}

async fn check_reference_metadata(component: &str) -> anyhow::Result<()> {
    let corpus: Corpus =
        serde_json::from_str(include_str!("../../test-data/config-conformance/v1.json"))?;
    let agents = extract_agent_type_schemas(Path::new(component), true, false).await?;
    let agent = agents
        .iter()
        .find(|agent| agent.type_name.0 == corpus.reference.agent_type)
        .expect("the actual guest must export the reference agent");
    let expected = &corpus.schemas[&corpus.reference.schema];
    assert_eq!(agent.config.len(), expected.declarations.len());
    for field in &expected.declarations {
        let actual = agent
            .config
            .iter()
            .find(|actual| actual.path == field.path)
            .expect("the actual guest must retain exact declaration paths");
        assert_eq!(actual.source, field.source, "{:?}", field.path);
        let actual_default = actual
            .default_value
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        let expected_default = field
            .default_value
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        assert_eq!(actual_default, expected_default, "{:?}", field.path);
        assert!(
            is_equivalent_cross_graph(
                &agent.schema,
                &actual.value_type,
                &expected.graph,
                &field.value_type,
            ),
            "the actual guest schema differs at {:?}",
            field.path
        );
    }
    Ok(())
}
