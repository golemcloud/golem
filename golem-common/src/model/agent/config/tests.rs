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

    for owner_kind in [OwnerKind::EphemeralExternalTool, OwnerKind::OplogProcessor] {
        let enriched = UntypedAgentConfigEntry {
            path: vec!["baseline".to_string(), "value".to_string()],
            value: SchemaValue::String("configured".to_string()),
        }
        .enrich_with_type(&metadata, owner_kind, None)
        .expect("component-baseline config must not require an agent type");

        assert_eq!(
            enriched.value.root_type(),
            &SchemaType::ref_to(type_id.clone())
        );
        assert_eq!(
            enriched.value.value(),
            &SchemaValue::String("configured".to_string())
        );
        assert_eq!(enriched.value.graph().defs.len(), 1);
    }
}

#[test]
fn metadata_redaction_redacts_the_capabilities_in_the_divergence_of_a_failed_update() {
    use crate::base_model::oplog::replay_divergence_test_values::{
        assert_requests_redacted, divergence_with_nested_capabilities,
    };
    use crate::base_model::worker::{FailedUpdate, UpdateRecord};
    use crate::model::agent::AgentId;
    use crate::model::agent::AgentUpdateMode;
    use crate::model::component::ComponentRevision;
    use crate::model::oplog::{OplogIndex, UpdateFailureClass};
    use crate::model::{AgentStatus, Timestamp};

    let mut metadata = AgentMetadataDto {
        agent_id: AgentId::from_parts(7, 1),
        owner_kind: OwnerKind::ComponentAgent,
        agent_type_name: Some(crate::model::agent::AgentTypeName("Agent".to_string())),
        canonical_key: Some("Agent()".to_string()),
        constructor_parameters: Some("()".to_string()),
        environment_id: crate::model::environment::EnvironmentId::new(),
        created_by: crate::model::account::AccountId::new(),
        env: Default::default(),
        config: vec![],
        status: AgentStatus::Idle,
        component_revision: ComponentRevision::INITIAL,
        retry_count: 0,
        pending_invocation_count: 0,
        updates: vec![UpdateRecord::FailedUpdate(FailedUpdate {
            timestamp: Timestamp::now_utc(),
            target_revision: ComponentRevision::new(1).unwrap(),
            details: None,
            pending_update_index: Some(OplogIndex::from_u64(5)),
            mode: AgentUpdateMode::Automatic,
            snapshot_assisted_details: None,
            failure_class: UpdateFailureClass::Divergence,
            divergence: Some(Box::new(divergence_with_nested_capabilities())),
        })],
        created_at: Timestamp::now_utc(),
        last_error: None,
        last_error_kind: None,
        component_size: 0,
        total_linear_memory_size: 0,
        exported_resource_instances: vec![],
        active_plugins: Default::default(),
        skipped_regions: vec![],
        deleted_regions: vec![],
        last_oplog_index: OplogIndex::from_u64(17),
        fingerprint: crate::model::AgentFingerprint(uuid::Uuid::nil()),
    };

    metadata.redact_host_managed_values_for_external();

    let UpdateRecord::FailedUpdate(update) = &metadata.updates[0] else {
        unreachable!()
    };
    assert_requests_redacted(update.divergence.as_ref().unwrap());
}
