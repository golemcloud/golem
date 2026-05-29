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
    let graph = SchemaGraph::anonymous(SchemaType::Secret(SecretSpec::default()));
    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::SecretNotAllowed {
        scope: SchemaScope::Constructor,
    }));
}

#[test]
fn secret_in_persisted_is_allowed() {
    let graph = SchemaGraph::anonymous(SchemaType::Secret(SecretSpec::default()));
    assert!(validate_placement(&graph, SchemaScope::Persisted).is_ok());
}

#[test]
fn quota_token_in_constructor_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::QuotaToken(Default::default()));
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
                element: Box::new(SchemaType::Union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "t".to_string(),
                        body: SchemaType::String,
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: String::new(),
                        },
                        metadata: Default::default(),
                    }],
                })),
            },
            metadata: multimodal,
        }],
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
                element: Box::new(SchemaType::Union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "t".to_string(),
                        body: SchemaType::String,
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: String::new(),
                        },
                        metadata: Default::default(),
                    }],
                })),
            },
            metadata: multimodal,
        }],
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
            metadata: multimodal,
            body: SchemaType::List {
                element: Box::new(SchemaType::Union(UnionSpec {
                    branches: vec![UnionBranch {
                        tag: "t".to_string(),
                        body: SchemaType::String,
                        discriminator: DiscriminatorRule::Prefix {
                            prefix: String::new(),
                        },
                        metadata: Default::default(),
                    }],
                })),
            },
        }],
        root: SchemaType::Ref(TypeId::new("ParticipantList")),
    };

    let errors = validate_placement(&graph, SchemaScope::Constructor).expect_err("should fail");
    assert!(errors.contains(&PlacementError::MultimodalListNotAllowedInConstructor));
}

#[test]
fn plain_primitives_allowed_in_every_scope() {
    let graph = SchemaGraph::anonymous(SchemaType::Bool);
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
