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

use super::*;
use crate::model::agent::OwnerKind;
use crate::model::component_metadata::{ComponentMetadata, KnownExports};
use crate::schema::agent::{AgentConfigDeclarationSchema, ComponentConfigSchema};
use crate::schema::{SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue, TypeId};
use test_r::test;

#[test]
fn enrich_component_baseline_config_uses_component_schema_without_agent_type() {
    let type_id = TypeId::new("baseline-value");
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        Vec::new(),
        None,
        None,
        Vec::new(),
        BTreeMap::new(),
    )
    .with_component_config(
        ComponentConfigSchema {
            schema: SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: type_id.clone(),
                    name: Some("BaselineValue".to_string()),
                    body: SchemaType::string(),
                }],
                root: SchemaType::record(Vec::new()),
            },
            declarations: vec![AgentConfigDeclarationSchema {
                source: AgentConfigSource::Local,
                path: vec!["baseline".to_string(), "value".to_string()],
                value_type: SchemaType::ref_to(type_id.clone()),
            }],
        },
        Default::default(),
    );

    let enriched = UntypedAgentConfigEntry {
        path: vec!["baseline".to_string(), "value".to_string()],
        value: SchemaValue::String("configured".to_string()),
    }
    .enrich_with_type(&metadata, OwnerKind::EphemeralExternalTool, None)
    .expect("component-baseline config must not require an agent type");

    assert_eq!(enriched.value.root_type(), &SchemaType::ref_to(type_id));
    assert_eq!(
        enriched.value.value(),
        &SchemaValue::String("configured".to_string())
    );
    assert_eq!(enriched.value.graph().defs.len(), 1);
}
