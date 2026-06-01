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

use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::schema_type::{
    DiscriminatorRule, NamedFieldType, SchemaType, SecretSpec, UnionBranch, UnionSpec,
};
use crate::schema::validation::placement::{PlacementError, SchemaScope, validate_placement};
use test_r::test;

#[test]
fn secret_in_constructor_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::secret(SecretSpec::default()));
    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::SecretNotAllowed {
        scope: SchemaScope::Constructor,
    }));
}

#[test]
fn secret_in_persisted_is_allowed() {
    let graph = SchemaGraph::anonymous(SchemaType::secret(SecretSpec::default()));
    assert!(validate_placement(&graph, SchemaScope::Persisted).is_ok());
}

#[test]
fn quota_token_in_constructor_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::quota_token(Default::default()));
    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::QuotaTokenNotAllowed {
        scope: SchemaScope::Constructor,
    }));
}

#[test]
fn multimodal_list_in_constructor_is_rejected_via_field_metadata() {
    let mut multimodal = MetadataEnvelope::default();
    multimodal.role = Some(Role::Multimodal);

    let graph = SchemaGraph::anonymous(SchemaType::Record {
        fields: vec![NamedFieldType {
            name: "parts".to_string(),
            body: SchemaType::List {
                element: Box::new(SchemaType::union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "t".to_string(),
                        body: SchemaType::string(),
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: String::new(),
                        },
                        metadata: Default::default(),
                    }],
                })),
                metadata: Default::default(),
            },
            metadata: multimodal,
        }],
        metadata: Default::default(),
    });

    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::MultimodalListNotAllowedInConstructor));
}

#[test]
fn multimodal_list_in_persisted_is_allowed() {
    let mut multimodal = MetadataEnvelope::default();
    multimodal.role = Some(Role::Multimodal);

    let graph = SchemaGraph::anonymous(SchemaType::Record {
        fields: vec![NamedFieldType {
            name: "parts".to_string(),
            body: SchemaType::List {
                element: Box::new(SchemaType::union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "t".to_string(),
                        body: SchemaType::string(),
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: String::new(),
                        },
                        metadata: Default::default(),
                    }],
                })),
                metadata: Default::default(),
            },
            metadata: multimodal,
        }],
        metadata: Default::default(),
    });

    assert!(validate_placement(&graph, SchemaScope::Persisted).is_ok());
}

#[test]
fn multimodal_list_in_constructor_is_rejected_via_def_metadata() {
    let mut multimodal = MetadataEnvelope::default();
    multimodal.role = Some(Role::Multimodal);

    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new("ParticipantList"),
            name: None,
            body: SchemaType::List {
                element: Box::new(SchemaType::union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "t".to_string(),
                        body: SchemaType::string(),
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: String::new(),
                        },
                        metadata: Default::default(),
                    }],
                })),
                metadata: multimodal,
            },
        }],
        root: SchemaType::ref_to(TypeId::new("ParticipantList")),
    };

    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::MultimodalListNotAllowedInConstructor));
}

#[test]
fn multimodal_list_in_constructor_is_rejected_via_inner_union_metadata() {
    // Mirrors the shape produced by `data_schema_to_output_schema` for
    // multimodal: the `Role::Multimodal` marker lives on the inner
    // `Union`, not on the wrapping `List` or its enclosing field.
    let mut inner_union = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "text".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Prefix {
                prefix: String::new(),
            },
            metadata: Default::default(),
        }],
    });
    inner_union.metadata_mut().role = Some(Role::Multimodal);

    let graph = SchemaGraph::anonymous(SchemaType::Record {
        fields: vec![NamedFieldType {
            name: "parts".to_string(),
            body: SchemaType::List {
                element: Box::new(inner_union),
                metadata: Default::default(),
            },
            metadata: Default::default(),
        }],
        metadata: Default::default(),
    });

    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::MultimodalListNotAllowedInConstructor));
}

#[test]
fn plain_primitives_allowed_in_every_scope() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    for scope in [
        SchemaScope::Constructor,
        SchemaScope::Persisted,
        SchemaScope::Boundary,
        SchemaScope::Docs,
        SchemaScope::Custom,
    ] {
        assert!(validate_placement(&graph, scope).is_ok());
    }
}

// --------------------------------------------------------------------------
// Agent-aware placement
// --------------------------------------------------------------------------

mod agent {
    use super::*;
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, AgentTypeName, Snapshotting};
    use crate::schema::agent::{
        AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
        InputSchema, NamedField, OutputSchema,
    };
    use crate::schema::schema_type::SecretSpec;
    use crate::schema::validation::placement::{
        validate_agent_dependency_placement, validate_agent_type_placement,
    };
    use test_r::test;

    fn empty_agent(name: &str) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName(name.into()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    #[test]
    fn empty_agent_passes_placement() {
        let agent = empty_agent("a");
        validate_agent_type_placement(&agent).expect("empty agent should pass");
    }

    #[test]
    fn secret_in_constructor_input_is_rejected_at_agent_layer() {
        let mut agent = empty_agent("a");
        agent.constructor.input_schema = InputSchema::Parameters(vec![NamedField::user_supplied(
            "creds",
            SchemaType::secret(SecretSpec::default()),
        )]);

        let errors = validate_agent_type_placement(&agent)
            .expect_err("secret in constructor input should be rejected");
        assert!(errors.contains(&PlacementError::SecretNotAllowed {
            scope: SchemaScope::Constructor,
        }));
    }

    #[test]
    fn secret_in_method_input_is_allowed() {
        // Method inputs are checked under `Boundary` scope, which permits
        // secrets (they are forbidden only in constructor / agent-id
        // scope per the §4.18 matrix).
        let mut agent = empty_agent("a");
        agent.methods.push(AgentMethodSchema {
            name: "login".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "token",
                SchemaType::secret(SecretSpec::default()),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        validate_agent_type_placement(&agent)
            .expect("secret in method input should pass at Boundary scope");
    }

    #[test]
    fn dep_constructor_input_secret_is_rejected() {
        let mut agent = empty_agent("a");
        agent.dependencies.push(AgentDependencySchema {
            type_name: "dep".into(),
            description: None,
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "secret",
                    SchemaType::secret(SecretSpec::default()),
                )]),
            },
            methods: vec![],
        });
        let errors = validate_agent_type_placement(&agent)
            .expect_err("dep constructor secret should be rejected");
        assert!(errors.contains(&PlacementError::SecretNotAllowed {
            scope: SchemaScope::Constructor,
        }));
    }

    #[test]
    fn ref_in_constructor_resolves_via_agent_graph() {
        // A constructor parameter that is `SchemaType::Ref(secret_id)` —
        // where `secret_id` resolves to `SchemaType::Secret` in the
        // agent's `schema.defs` — must still be caught by the
        // constructor-scope check.
        let secret_id = TypeId::new("a.b.SecretBag");
        let mut agent = empty_agent("a");
        agent.schema = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: secret_id.clone(),
                name: None,
                body: SchemaType::secret(SecretSpec::default()),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        };
        agent.constructor.input_schema = InputSchema::Parameters(vec![NamedField::user_supplied(
            "creds",
            SchemaType::ref_to(secret_id.clone()),
        )]);

        let errors = validate_agent_type_placement(&agent)
            .expect_err("ref-to-secret in constructor must fail");
        assert!(errors.contains(&PlacementError::SecretNotAllowed {
            scope: SchemaScope::Constructor,
        }));
    }

    #[test]
    fn agent_sentinel_root_is_not_walked() {
        // Even if `schema.root` is set to something that would be invalid
        // under Constructor scope, the agent validators must ignore it.
        let mut agent = empty_agent("a");
        agent.schema.root = SchemaType::secret(SecretSpec::default());
        validate_agent_type_placement(&agent)
            .expect("agent placement must ignore schema.root sentinel");
    }

    #[test]
    fn secret_def_referenced_from_method_input_is_allowed() {
        let secret_id = TypeId::new("a.SecretBag");
        let mut agent = empty_agent("a");
        agent.schema.defs.push(SchemaTypeDef {
            id: secret_id.clone(),
            name: None,
            body: SchemaType::secret(SecretSpec::default()),
        });
        agent.methods.push(AgentMethodSchema {
            name: "login".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "token",
                SchemaType::ref_to(secret_id),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        validate_agent_type_placement(&agent)
            .expect("secret def via method input ref must pass at Boundary scope");
    }

    #[test]
    fn secret_in_method_output_is_allowed() {
        let mut agent = empty_agent("a");
        agent.methods.push(AgentMethodSchema {
            name: "issue_token".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
            output_schema: OutputSchema::Single(SchemaType::secret(SecretSpec::default())),
            http_endpoint: vec![],
            read_only: None,
        });
        validate_agent_type_placement(&agent)
            .expect("secret in method output must pass at Boundary scope");
    }

    #[test]
    fn quota_token_in_constructor_input_is_rejected_at_agent_layer() {
        let mut agent = empty_agent("a");
        agent.constructor.input_schema = InputSchema::Parameters(vec![NamedField::user_supplied(
            "quota",
            SchemaType::quota_token(Default::default()),
        )]);
        let errors = validate_agent_type_placement(&agent)
            .expect_err("quota-token in constructor input must be rejected");
        assert!(errors.contains(&PlacementError::QuotaTokenNotAllowed {
            scope: SchemaScope::Constructor,
        }));
    }

    #[test]
    fn multimodal_list_in_constructor_field_metadata_is_rejected() {
        // The multimodal marker lives on the `NamedField.metadata`, and
        // the field body is `list<union<…>>`. Constructor scope must
        // reject this even though the multimodal role itself lives on
        // the enclosing field metadata rather than on the list/union
        // nodes.
        let mut field_md = MetadataEnvelope::default();
        field_md.role = Some(Role::Multimodal);

        let list_union = SchemaType::List {
            element: Box::new(SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "t".to_string(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix {
                        prefix: "t-".into(),
                    },
                    metadata: MetadataEnvelope::default(),
                }],
            })),
            metadata: MetadataEnvelope::default(),
        };

        let mut agent = empty_agent("a");
        agent.constructor.input_schema = InputSchema::Parameters(vec![NamedField {
            name: "parts".into(),
            source: crate::schema::agent::FieldSource::UserSupplied,
            schema: list_union,
            metadata: field_md,
        }]);

        let errors = validate_agent_type_placement(&agent)
            .expect_err("multimodal list in constructor field must be rejected");
        assert!(errors.contains(&PlacementError::MultimodalListNotAllowedInConstructor));
    }

    #[test]
    fn standalone_dependency_validation_uses_dep_graph() {
        let secret_id = TypeId::new("dep.SecretBag");
        let dep = AgentDependencySchema {
            type_name: "dep".into(),
            description: None,
            schema: SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: secret_id.clone(),
                    name: None,
                    body: SchemaType::secret(SecretSpec::default()),
                }],
                root: SchemaType::Record {
                    fields: vec![],
                    metadata: Default::default(),
                },
            },
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "creds",
                    SchemaType::ref_to(secret_id),
                )]),
            },
            methods: vec![],
        };
        let errors = validate_agent_dependency_placement(&dep)
            .expect_err("dep ref-to-secret in constructor must fail");
        assert!(errors.contains(&PlacementError::SecretNotAllowed {
            scope: SchemaScope::Constructor,
        }));
    }
}
