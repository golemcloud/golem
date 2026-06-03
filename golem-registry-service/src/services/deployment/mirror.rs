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

//! Parallel agent-metadata mirror exposed by the registry-service.
//!
//! The mirror pairs a [`DeployedRegisteredAgentType`] (or
//! [`ResolvedAgentType`]) with the schema-layer [`AgentTypeSchema`]
//! computed from it via [`agent_type_to_schema`]. Both are returned
//! side-by-side so a migrated consumer can read the schema form while
//! unmigrated consumers continue reading the existing one.
//!
//! Scope:
//!
//! - This mirror is purely in-process. It does not change any wire
//!   format (gRPC, HTTP, OpenAPI), persistence column, or oplog payload.
//! - The schema is recomputed on every call. The forward adapter is
//!   deterministic and cheap; caching would add invalidation complexity
//!   without observable benefit at this stage.
//! - Today the adapter produces an [`AgentTypeSchema`] with an empty
//!   [`SchemaGraph`]. When SDK code generation starts hoisting shared
//!   named definitions, registry storage will need to carry the schema
//!   form directly rather than reconstructing it here.

// TODO: delete this module and all `*_with_schema` methods on `DeploymentService`
// once `AgentType` is fully replaced by `AgentTypeSchema` across the registry,
// services, and consumers.

use super::read::DeploymentError;
use golem_common::model::agent::{DeployedRegisteredAgentType, ResolvedAgentType};
use golem_common::schema::adapters::agent::agent_type_to_schema;
use golem_common::schema::adapters::error::SchemaAdapterError;
use golem_common::schema::agent::AgentTypeSchema;

/// A deployed agent type paired with its schema-layer mirror.
#[derive(Debug, Clone, PartialEq)]
pub struct DeployedAgentTypeMirror {
    pub legacy: DeployedRegisteredAgentType,
    pub schema: AgentTypeSchema,
}

impl DeployedAgentTypeMirror {
    pub fn from_legacy(legacy: DeployedRegisteredAgentType) -> Result<Self, SchemaAdapterError> {
        let schema = agent_type_to_schema(&legacy.agent_type)?;
        Ok(Self { legacy, schema })
    }
}

/// A resolved agent type paired with its schema-layer mirror.
#[derive(Clone)]
pub struct ResolvedAgentTypeMirror {
    pub legacy: ResolvedAgentType,
    pub schema: AgentTypeSchema,
}

impl ResolvedAgentTypeMirror {
    pub fn from_legacy(legacy: ResolvedAgentType) -> Result<Self, SchemaAdapterError> {
        let schema = agent_type_to_schema(&legacy.registered_agent_type.agent_type)?;
        Ok(Self { legacy, schema })
    }
}

pub(super) fn schema_mirror_error(err: SchemaAdapterError) -> DeploymentError {
    DeploymentError::InternalError(
        anyhow::Error::new(err).context("failed to build agent metadata schema mirror"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::{
        AgentConstructor, AgentDependency, AgentMethod, AgentMode, AgentType, AgentTypeName,
        ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
        NamedElementSchemas, RegisteredAgentType, RegisteredAgentTypeImplementer, Snapshotting,
    };
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::schema::adapters::agent::schema_agent_type_to_legacy;
    use golem_common::schema::graph::SchemaGraph;
    use golem_wasm::analysis::analysed_type::str;
    use test_r::test;
    use uuid::Uuid;

    fn named_field(name: &str) -> NamedElementSchema {
        NamedElementSchema {
            name: name.to_string(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: str(),
            }),
        }
    }

    fn empty_data_schema() -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas { elements: vec![] })
    }

    fn tuple_data_schema(names: &[&str]) -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: names.iter().map(|n| named_field(n)).collect(),
        })
    }

    fn multimodal_data_schema(names: &[&str]) -> DataSchema {
        DataSchema::Multimodal(NamedElementSchemas {
            elements: names.iter().map(|n| named_field(n)).collect(),
        })
    }

    /// Build a representative agent type with a constructor, three
    /// methods (empty output, multi-field tuple output, multimodal
    /// output) and one dependency. Shapes are chosen so the schema →
    /// legacy round trip is name-preserving.
    fn representative_agent_type() -> AgentType {
        AgentType {
            type_name: AgentTypeName("note-agent".to_string()),
            description: "an agent".to_string(),
            source_language: "rust".to_string(),
            constructor: AgentConstructor {
                name: Some("new".to_string()),
                description: "ctor".to_string(),
                prompt_hint: None,
                input_schema: tuple_data_schema(&["title", "body"]),
            },
            methods: vec![
                AgentMethod {
                    name: "fetch".to_string(),
                    description: "fetch".to_string(),
                    prompt_hint: None,
                    input_schema: empty_data_schema(),
                    output_schema: empty_data_schema(),
                    http_endpoint: vec![],
                    read_only: None,
                },
                AgentMethod {
                    name: "describe".to_string(),
                    description: "describe".to_string(),
                    prompt_hint: None,
                    input_schema: tuple_data_schema(&["query"]),
                    output_schema: tuple_data_schema(&["summary", "details"]),
                    http_endpoint: vec![],
                    read_only: None,
                },
                AgentMethod {
                    name: "preview".to_string(),
                    description: "preview".to_string(),
                    prompt_hint: None,
                    input_schema: empty_data_schema(),
                    output_schema: multimodal_data_schema(&["thumbnail", "caption"]),
                    http_endpoint: vec![],
                    read_only: None,
                },
            ],
            dependencies: vec![AgentDependency {
                type_name: "log-sink".to_string(),
                description: Some("audit log dependency".to_string()),
                constructor: AgentConstructor {
                    name: Some("connect".to_string()),
                    description: "connect".to_string(),
                    prompt_hint: None,
                    input_schema: tuple_data_schema(&["endpoint"]),
                },
                methods: vec![AgentMethod {
                    name: "write".to_string(),
                    description: "write a log line".to_string(),
                    prompt_hint: None,
                    input_schema: tuple_data_schema(&["message"]),
                    output_schema: empty_data_schema(),
                    http_endpoint: vec![],
                    read_only: None,
                }],
            }],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    fn deployed(agent_type: AgentType) -> DeployedRegisteredAgentType {
        DeployedRegisteredAgentType {
            agent_type,
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: ComponentId(Uuid::new_v4()),
                component_revision: ComponentRevision::INITIAL,
            },
            webhook_prefix_authority_and_path: Some("example.com/webhooks".to_string()),
        }
    }

    #[test]
    fn mirror_schema_matches_direct_adapter() {
        let legacy = deployed(representative_agent_type());
        let mirror = DeployedAgentTypeMirror::from_legacy(legacy.clone()).unwrap();

        let direct = agent_type_to_schema(&legacy.agent_type).unwrap();
        assert_eq!(mirror.schema, direct);
    }

    #[test]
    fn mirror_preserves_legacy_payload() {
        let legacy = deployed(representative_agent_type());
        let mirror = DeployedAgentTypeMirror::from_legacy(legacy.clone()).unwrap();
        assert_eq!(mirror.legacy, legacy);
    }

    #[test]
    fn mirror_graph_is_empty_for_legacy_input() {
        let legacy = deployed(representative_agent_type());
        let mirror = DeployedAgentTypeMirror::from_legacy(legacy).unwrap();
        assert_eq!(mirror.schema.schema, SchemaGraph::empty());
        assert!(
            !mirror.schema.dependencies.is_empty(),
            "fixture must exercise at least one dependency",
        );
        for dep in &mirror.schema.dependencies {
            assert_eq!(dep.schema, SchemaGraph::empty());
        }
    }

    #[test]
    fn mirror_roundtrip_for_name_preserving_shapes() {
        let original = representative_agent_type();
        let mirror = DeployedAgentTypeMirror::from_legacy(deployed(original.clone())).unwrap();
        let round_tripped = schema_agent_type_to_legacy(&mirror.schema).unwrap();
        assert_eq!(round_tripped, original);
    }

    #[test]
    fn resolved_mirror_preserves_resolution_metadata() {
        let agent_type = representative_agent_type();
        let implementer = RegisteredAgentTypeImplementer {
            component_id: ComponentId(Uuid::new_v4()),
            component_revision: ComponentRevision::INITIAL,
        };
        let resolved = ResolvedAgentType {
            registered_agent_type: RegisteredAgentType {
                agent_type: agent_type.clone(),
                implemented_by: implementer.clone(),
            },
            environment_id: EnvironmentId(Uuid::new_v4()),
            deployment_revision: DeploymentRevision::INITIAL,
            current_deployment_revision: Some(CurrentDeploymentRevision::INITIAL),
        };

        let mirror = ResolvedAgentTypeMirror::from_legacy(resolved.clone()).unwrap();

        assert_eq!(
            mirror.legacy.registered_agent_type.agent_type,
            resolved.registered_agent_type.agent_type
        );
        assert_eq!(
            mirror.legacy.registered_agent_type.implemented_by,
            implementer
        );
        assert_eq!(mirror.legacy.environment_id, resolved.environment_id);
        assert_eq!(
            mirror.legacy.deployment_revision,
            resolved.deployment_revision
        );
        assert_eq!(
            mirror.legacy.current_deployment_revision,
            resolved.current_deployment_revision
        );

        let direct = agent_type_to_schema(&agent_type).unwrap();
        assert_eq!(mirror.schema, direct);
    }
}
