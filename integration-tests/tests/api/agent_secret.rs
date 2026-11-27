// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateAgentSecretError,
    RegistryServiceDeleteAgentSecretError, RegistryServiceGetEnvironmentAgentSecretError,
    RegistryServiceUpdateAgentSecretError,
};
use golem_client::model::{AgentSecretCreation, AgentSecretDto, AgentSecretUpdate};
use golem_common::model::agent_secret::{
    AgentSecretOwner, AgentSecretPath, AgentSecretReadMode, AgentSecretRevision,
    AgentSecretRevisionSelector, CanonicalAgentSecretPath,
};
use golem_common::model::api::error_code;
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName,
};
use golem_common::schema::{ExternalSchemaValue, SchemaGraph, SchemaType, SchemaValue};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService as _,
};
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

fn external(value: SchemaValue) -> ExternalSchemaValue {
    ExternalSchemaValue::try_from(value).unwrap()
}

const SECRET_MARKERS: [&str; 2] = ["private-api-secret-original", "private-api-secret-updated"];

async fn checked_secret_response(response: Response, status: StatusCode) -> anyhow::Result<Value> {
    assert_eq!(response.status(), status);
    let body = response.text().await?;
    for marker in SECRET_MARKERS {
        assert!(!body.contains(marker), "Response leaked a secret marker");
    }
    let value: Value = serde_json::from_str(&body)?;
    assert_no_secret_value(&value);
    Ok(value)
}

fn assert_no_secret_value(value: &Value) {
    match value {
        Value::Object(fields) => {
            assert!(!fields.contains_key("secretValue"));
            for value in fields.values() {
                assert_no_secret_value(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                assert_no_secret_value(value);
            }
        }
        _ => {}
    }
}

pub(super) async fn read_secret_value(
    deps: &EnvBasedTestDependencies,
    secret: &AgentSecretDto,
) -> anyhow::Result<Option<SchemaValue>> {
    let registry_service = deps.registry_service();
    let client = GrpcRegistryService::new(&GrpcRegistryServiceConfig {
        host: registry_service.grpc_host(),
        port: registry_service.grpc_port(),
        client_config: GrpcClientConfig::default(),
        invalidation_event_subscriber: Default::default(),
    });
    let revision = client
        .get_agent_secret_revision(
            secret.environment_id,
            AgentSecretOwner::Environment,
            secret.id,
            secret.path.clone(),
            AgentSecretRevisionSelector::Fixed(secret.revision),
            AgentSecretReadMode::Live,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Expected the agent secret revision to exist"))?;
    Ok(revision.secret_value)
}

#[test]
#[tracing::instrument]
async fn create_agent_secret_with_value(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["foo".to_string(), "bar".to_string()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let result = client.create_agent_secret(&env.id.0, &creation).await?;

    assert_eq!(
        result.path,
        CanonicalAgentSecretPath(vec!["foo".to_string(), "bar".to_string()])
    );
    assert_eq!(result.secret_type, creation.secret_type);
    assert!(result.has_value);
    assert_eq!(
        read_secret_value(deps, &result).await?,
        creation.secret_value.map(ExternalSchemaValue::into_inner)
    );
    assert_eq!(result.revision, AgentSecretRevision::INITIAL);

    {
        let fetched_secret = client.get_agent_secret(&result.id.0).await?;
        assert_eq!(fetched_secret, result);
    }

    {
        let fetched_secret = client
            .get_environment_agent_secret(&env.id.0, &result.path.0)
            .await?;
        assert_eq!(fetched_secret, result);

        let other_user = deps.user().await?;
        let other_client = deps.registry_service().client(&other_user.token).await;
        let other_result = other_client
            .get_environment_agent_secret(&env.id.0, &result.path.0)
            .await;
        assert_matches!(
            other_result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentAgentSecretError::Error404(_)
            ))
        );
    }

    {
        let all_environment_secrets = client.list_environment_agent_secrets(&env.id.0).await?;
        assert!(all_environment_secrets.values.contains(&result));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn granted_secret_view_works_without_environment_view(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let owner_client = deps.registry_service().client(&owner.token).await;
    let (app, env) = owner.app_and_env().await?;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["foo".to_string(), "bar".to_string()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };
    let secret = owner_client
        .create_agent_secret(&env.id.0, &creation)
        .await?;

    let grantee = deps.user().await?;
    let grantee_client = deps.registry_service().client(&grantee.token).await;

    // Before any grant the by-path lookup is not visible to the grantee.
    let before = grantee_client
        .get_environment_agent_secret(&env.id.0, &secret.path.0)
        .await;
    assert_matches!(
        before,
        Err(golem_client::Error::Item(
            RegistryServiceGetEnvironmentAgentSecretError::Error404(_)
        ))
    );

    // Grant view on this ONE secret only — no environment-view permission.
    owner_client
        .create_permission_share(
            &owner.account_id.0,
            &PermissionShareCreation {
                target_account_email: grantee.account_email.clone(),
                name: PermissionShareName("secret-view".to_string()),
                data: PermissionShareData {
                    lower_positive: vec![format!(
                        "environment.agent-secret({}/{}/{}) @ {} : view : {}",
                        owner.account_email.as_str(),
                        app.name.0,
                        env.name.0,
                        grantee.account_email.as_str(),
                        secret.path.0.join("."),
                    )],
                    lower_negative: Vec::new(),
                    upper_positive: Vec::new(),
                    upper_negative: Vec::new(),
                },
            },
        )
        .await?;

    // With only the resource grant the by-path lookup now succeeds, matching the by-id lookup.
    let fetched = grantee_client
        .get_environment_agent_secret(&env.id.0, &secret.path.0)
        .await?;
    assert_eq!(fetched, secret);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn secret_path_is_canonicalized_when_reading(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec![
            "firstPathSegment".to_string(),
            "SecondPathSegment".to_string(),
            "third_path_segment".to_string(),
        ]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let result = client.create_agent_secret(&env.id.0, &creation).await?;

    assert_eq!(
        result.path,
        CanonicalAgentSecretPath(vec![
            "firstPathSegment".to_string(),
            "secondPathSegment".to_string(),
            "thirdPathSegment".to_string()
        ])
    );
    assert_eq!(result.secret_type, creation.secret_type);
    assert!(result.has_value);
    assert_eq!(
        read_secret_value(deps, &result).await?,
        creation.secret_value.map(ExternalSchemaValue::into_inner)
    );
    assert_eq!(result.revision, AgentSecretRevision::INITIAL);

    {
        let fetched_secret = client.get_agent_secret(&result.id.0).await?;
        assert_eq!(fetched_secret, result);
    }

    {
        let fetched_secret = client
            .get_environment_agent_secret(&env.id.0, &creation.path.0)
            .await?;
        assert_eq!(fetched_secret, result);
    }

    {
        let all_environment_secrets = client.list_environment_agent_secrets(&env.id.0).await?;
        assert!(all_environment_secrets.values.contains(&result));
    }

    Ok(())
}

#[test]
async fn create_agent_secret_without_value(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["no".into(), "value".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: None,
    };

    let result = client.create_agent_secret(&env.id.0, &creation).await?;

    assert!(!result.has_value);
    assert_eq!(read_secret_value(deps, &result).await?, None);
    assert_eq!(result.revision, AgentSecretRevision::INITIAL);

    Ok(())
}

#[test]
async fn creating_same_path_twice_should_fail(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["dup".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    client.create_agent_secret(&env.id.0, &creation).await?;

    let result = client.create_agent_secret(&env.id.0, &creation).await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateAgentSecretError::Error409(_)
        ))
    );

    Ok(())
}

#[test]
async fn creating_same_path_in_different_casing_should_fail(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                destinations: Vec::new(),
                path: AgentSecretPath(vec!["secret_path".into()]),
                secret_type: SchemaGraph::anonymous(SchemaType::bool()),
                secret_value: Some(external(SchemaValue::Bool(true))),
            },
        )
        .await?;

    let result = client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                destinations: Vec::new(),
                path: AgentSecretPath(vec!["secretPath".into()]),
                secret_type: SchemaGraph::anonymous(SchemaType::bool()),
                secret_value: Some(external(SchemaValue::Bool(true))),
            },
        )
        .await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateAgentSecretError::Error409(_)
        ))
    );

    Ok(())
}

#[test]
async fn update_secret_increments_revision(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["rev".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    let updated = client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                destinations: None,
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(external(SchemaValue::Bool(false))),
            },
        )
        .await?;

    assert!(updated.has_value);
    assert_eq!(
        read_secret_value(deps, &updated).await?,
        Some(SchemaValue::Bool(false))
    );
    assert!(updated.revision > created.revision);

    Ok(())
}

#[test]
async fn update_with_stale_revision_should_fail(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["stale".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                destinations: None,
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(external(SchemaValue::Bool(false))),
            },
        )
        .await?;

    let stale_update = client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                destinations: None,
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(external(SchemaValue::Bool(true))),
            },
        )
        .await;

    assert_matches!(
        stale_update,
        Err(golem_client::Error::Item(
            RegistryServiceUpdateAgentSecretError::Error409(_)
        ))
    );

    Ok(())
}

#[test]
async fn unset_secret_value(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["unset".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(external(SchemaValue::String("hello".to_string()))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    let updated = client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                destinations: None,
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Unset,
            },
        )
        .await?;

    assert!(!updated.has_value);
    assert_eq!(read_secret_value(deps, &updated).await?, None);
    assert!(updated.revision > created.revision);

    Ok(())
}

#[test]
async fn delete_secret(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["delete".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    client
        .delete_agent_secret(&created.id.0, created.revision.into())
        .await?;

    Ok(())
}

#[test]
async fn delete_with_stale_revision_should_fail(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["delete-stale".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                destinations: None,
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(external(SchemaValue::Bool(false))),
            },
        )
        .await?;

    let stale_delete = client
        .delete_agent_secret(&created.id.0, created.revision.into())
        .await;

    assert_matches!(
        stale_delete,
        Err(golem_client::Error::Item(
            RegistryServiceDeleteAgentSecretError::Error409(_)
        ))
    );

    Ok(())
}

#[test]
async fn delete_and_recreate_same_path(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["recreate".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;
    client
        .delete_agent_secret(&created.id.0, created.revision.into())
        .await?;

    let recreated = client.create_agent_secret(&env.id.0, &creation).await?;

    assert_eq!(recreated.path, creation.path.into());
    assert_ne!(recreated.id, created.id);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_agent_secret_with_value_type_mismatch_should_fail(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["type".into(), "creation-mismatch".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::String("not-a-bool".to_string()))),
    };

    let result = client.create_agent_secret(&env.id.0, &creation).await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateAgentSecretError::Error400(_)
        ))
    );

    let all = client.list_environment_agent_secrets(&env.id.0).await?;

    assert_eq!(all.values, Vec::new());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_agent_secret_with_wrong_type_should_fail(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        destinations: Vec::new(),
        path: AgentSecretPath(vec!["update".into(), "type-mismatch".into()]),
        secret_type: SchemaGraph::anonymous(SchemaType::bool()),
        secret_value: Some(external(SchemaValue::Bool(true))),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    let update = AgentSecretUpdate {
        destinations: None,
        current_revision: created.revision,
        secret_value: OptionalFieldUpdate::Set(external(SchemaValue::String(
            "not-a-bool".to_string(),
        ))),
    };

    let result = client.update_agent_secret(&created.id.0, &update).await;

    assert!(result.is_err());

    Ok(())
}

#[test]
async fn secret_rest_responses_hide_values_and_report_presence(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let context = deps.registry_service().client_context(&user.token).await;
    let collection = context
        .base_url
        .join(&format!("/v1/envs/{}/agent-secrets", env.id.0))?;

    for (path, secret_type, secret_value, has_value) in [
        (
            "marker",
            SchemaType::string(),
            Some(SchemaValue::String(SECRET_MARKERS[0].into())),
            true,
        ),
        (
            "false",
            SchemaType::bool(),
            Some(SchemaValue::Bool(false)),
            true,
        ),
        (
            "empty",
            SchemaType::string(),
            Some(SchemaValue::String(String::new())),
            true,
        ),
        (
            "optional",
            SchemaType::option(SchemaType::string()),
            Some(SchemaValue::Option { inner: None }),
            true,
        ),
        ("absent", SchemaType::string(), None, false),
    ] {
        let response = context
            .client
            .post(collection.clone())
            .bearer_auth(user.token.secret())
            .json(
                &json!({"path": [path], "secretType": SchemaGraph::anonymous(secret_type),
                "secretValue": secret_value.clone().map(external)}),
            )
            .send()
            .await?;
        let body = checked_secret_response(response, StatusCode::OK).await?;
        assert_eq!(body["hasValue"], json!(has_value));
        assert_eq!(body["destinations"], json!([]));
        let created: AgentSecretDto = serde_json::from_value(body)?;
        assert_eq!(created.has_value, has_value);
        assert_eq!(read_secret_value(deps, &created).await?, secret_value);
        let by_id = context
            .base_url
            .join(&format!("/v1/agent-secrets/{}", created.id.0))?;
        let by_path = context
            .base_url
            .join(&format!("/v1/envs/{}/agent-secrets/by-path", env.id.0))?;
        for response in [
            context
                .client
                .get(by_id.clone())
                .bearer_auth(user.token.secret())
                .send()
                .await?,
            context
                .client
                .get(by_path)
                .query(&[("path", path)])
                .bearer_auth(user.token.secret())
                .send()
                .await?,
        ] {
            let body = checked_secret_response(response, StatusCode::OK).await?;
            let fetched: AgentSecretDto = serde_json::from_value(body)?;
            assert_eq!(fetched, created);
        }
        let body = checked_secret_response(
            context
                .client
                .get(collection.clone())
                .bearer_auth(user.token.secret())
                .send()
                .await?,
            StatusCode::OK,
        )
        .await?;
        let listed: Vec<AgentSecretDto> = serde_json::from_value(body["values"].clone())?;
        assert!(listed.contains(&created));

        let replacement = (path == "marker").then(|| SchemaValue::String(SECRET_MARKERS[1].into()));
        let value_update = match &replacement {
            Some(value) => json!({"op": "set", "value": external(value.clone())}),
            None => json!({"op": "unset"}),
        };
        let body = checked_secret_response(
            context
                .client
                .patch(by_id)
                .bearer_auth(user.token.secret())
                .json(&json!({"currentRevision": created.revision, "secretValue": value_update}))
                .send()
                .await?,
            StatusCode::OK,
        )
        .await?;
        let updated: AgentSecretDto = serde_json::from_value(body)?;
        assert_eq!(updated.has_value, replacement.is_some());
        assert_eq!(read_secret_value(deps, &updated).await?, replacement);
        let delete = context
            .base_url
            .join(&format!("/v1/agent-secret/{}", created.id.0))?;
        let body = checked_secret_response(
            context
                .client
                .delete(delete)
                .query(&[("current_revision", updated.revision)])
                .bearer_auth(user.token.secret())
                .send()
                .await?,
            StatusCode::OK,
        )
        .await?;
        let deleted: AgentSecretDto = serde_json::from_value(body)?;
        assert_eq!(deleted.id, created.id);
        assert!(!deleted.has_value);
        assert!(deleted.destinations.is_empty());
    }
    Ok(())
}

#[test]
async fn secret_destinations_roundtrip_and_updates_are_atomic(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let context = deps.registry_service().client_context(&user.token).await;
    let collection = context
        .base_url
        .join(&format!("/v1/envs/{}/agent-secrets", env.id.0))?;
    let original = SchemaValue::String(SECRET_MARKERS[0].into());
    let replacement = SchemaValue::String(SECRET_MARKERS[1].into());
    let initial_destinations = json!(["https://api.example.com", "https://*.example.org"]);
    let body = checked_secret_response(context.client.post(collection.clone())
        .bearer_auth(user.token.secret())
        .json(&json!({"path": ["destinations"], "secretType": SchemaGraph::anonymous(SchemaType::string()),
            "secretValue": external(original.clone()), "destinations": initial_destinations}))
        .send().await?, StatusCode::OK).await?;
    assert_eq!(
        body["destinations"],
        json!(["https://*.example.org", "https://api.example.com"])
    );
    let mut current: AgentSecretDto = serde_json::from_value(body)?;
    let by_id = context
        .base_url
        .join(&format!("/v1/agent-secrets/{}", current.id.0))?;
    let by_path = context
        .base_url
        .join(&format!("/v1/envs/{}/agent-secrets/by-path", env.id.0))?;
    for response in [
        context
            .client
            .get(by_id.clone())
            .bearer_auth(user.token.secret())
            .send()
            .await?,
        context
            .client
            .get(by_path)
            .query(&[("path", "destinations")])
            .bearer_auth(user.token.secret())
            .send()
            .await?,
    ] {
        let body = checked_secret_response(response, StatusCode::OK).await?;
        let fetched: AgentSecretDto = serde_json::from_value(body)?;
        assert_eq!(fetched, current);
    }
    let stale_revision = current.revision;
    for (destinations, value) in [
        (Some(json!(["https://destination-only.example.com"])), None),
        (
            Some(json!([
                "https://combined.example.com",
                "https://*.combined.example.org"
            ])),
            Some(replacement.clone()),
        ),
        (None, Some(original.clone())),
        (Some(json!([])), None),
    ] {
        let mut update = json!({"currentRevision": current.revision});
        if let Some(destinations) = &destinations {
            update["destinations"] = destinations.clone();
        }
        if let Some(value) = &value {
            update["secretValue"] = json!({"op": "set", "value": external(value.clone())});
        }
        let body = checked_secret_response(
            context
                .client
                .patch(by_id.clone())
                .bearer_auth(user.token.secret())
                .json(&update)
                .send()
                .await?,
            StatusCode::OK,
        )
        .await?;
        let mut expected_destinations: Vec<String> = match destinations {
            Some(destinations) => serde_json::from_value(destinations)?,
            None => current.destinations.clone(),
        };
        expected_destinations.sort();
        assert_eq!(body["destinations"], json!(expected_destinations));
        let updated: AgentSecretDto = serde_json::from_value(body)?;
        assert_eq!(updated.revision, current.revision.next()?);
        assert!(updated.has_value);
        assert_eq!(
            read_secret_value(deps, &updated).await?,
            Some(value.unwrap_or(original.clone()))
        );
        current = updated;
    }

    for (update, status, destination_error) in [
        (
            json!({"currentRevision": current.revision, "destinations": ["ftp://x.com"],
            "secretValue": {"op": "set", "value": external(replacement.clone())}}),
            StatusCode::BAD_REQUEST,
            true,
        ),
        (
            json!({"currentRevision": current.revision, "destinations": ["https://rejected.example.com"],
            "secretValue": {"op": "set", "value": external(SchemaValue::List { elements: vec![original.clone()] })}}),
            StatusCode::BAD_REQUEST,
            false,
        ),
        (
            json!({"currentRevision": stale_revision, "destinations": ["https://stale.example.com"],
            "secretValue": {"op": "set", "value": external(replacement.clone())}}),
            StatusCode::CONFLICT,
            false,
        ),
    ] {
        let error = checked_secret_response(
            context
                .client
                .patch(by_id.clone())
                .bearer_auth(user.token.secret())
                .json(&update)
                .send()
                .await?,
            status,
        )
        .await?;
        if destination_error {
            assert_eq!(error["code"], error_code::AGENT_SECRET_INVALID_DESTINATION);
            assert!(error["errors"].to_string().contains("ftp://x.com"));
        }
        let body = checked_secret_response(
            context
                .client
                .get(by_id.clone())
                .bearer_auth(user.token.secret())
                .send()
                .await?,
            StatusCode::OK,
        )
        .await?;
        let fetched: AgentSecretDto = serde_json::from_value(body)?;
        assert_eq!(fetched, current);
        assert_eq!(
            read_secret_value(deps, &fetched).await?,
            Some(original.clone())
        );
    }
    for (path, value, destinations, destination_error) in [
        (
            "invalid-destination",
            external(original.clone()),
            json!(["ftp://x.com"]),
            true,
        ),
        (
            "invalid-value",
            external(original.clone()),
            json!(["https://valid.example.com"]),
            false,
        ),
    ] {
        let secret_type = if destination_error {
            SchemaType::string()
        } else {
            SchemaType::bool()
        };
        let error = checked_secret_response(
            context
                .client
                .post(collection.clone())
                .bearer_auth(user.token.secret())
                .json(
                    &json!({"path": [path], "secretType": SchemaGraph::anonymous(secret_type),
                "secretValue": value, "destinations": destinations}),
                )
                .send()
                .await?,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        if destination_error {
            assert_eq!(error["code"], error_code::AGENT_SECRET_INVALID_DESTINATION);
            assert!(error["errors"].to_string().contains("ftp://x.com"));
        }
    }
    let body = checked_secret_response(
        context
            .client
            .get(collection)
            .bearer_auth(user.token.secret())
            .send()
            .await?,
        StatusCode::OK,
    )
    .await?;
    let listed: Vec<AgentSecretDto> = serde_json::from_value(body["values"].clone())?;
    assert_eq!(listed, vec![current]);
    Ok(())
}

#[test]
async fn secret_destination_updates_require_view_and_update(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let (app, env) = owner.app_and_env().await?;
    let owner_client = deps.registry_service().client(&owner.token).await;
    let context = deps.registry_service().client_context(&owner.token).await;
    let collection = context
        .base_url
        .join(&format!("/v1/envs/{}/agent-secrets", env.id.0))?;
    let original = SchemaValue::String(SECRET_MARKERS[0].into());
    let body = checked_secret_response(context.client.post(collection)
        .bearer_auth(owner.token.secret())
        .json(&json!({"path": ["permissions"], "secretType": SchemaGraph::anonymous(SchemaType::string()),
            "secretValue": external(original.clone()), "destinations": ["https://original.example.com"]}))
        .send().await?, StatusCode::OK).await?;
    let mut current: AgentSecretDto = serde_json::from_value(body)?;

    for (name, verbs, status) in [
        ("view-only", vec!["view"], StatusCode::FORBIDDEN),
        ("view-create", vec!["view", "create"], StatusCode::FORBIDDEN),
        ("update-only", vec!["update"], StatusCode::NOT_FOUND),
        ("view-update", vec!["view", "update"], StatusCode::OK),
    ] {
        let grantee = deps.user().await?;
        owner_client
            .create_permission_share(
                &owner.account_id.0,
                &PermissionShareCreation {
                    target_account_email: grantee.account_email.clone(),
                    name: PermissionShareName(name.into()),
                    data: PermissionShareData {
                        lower_positive: verbs
                            .iter()
                            .map(|verb| {
                                format!(
                                    "environment.agent-secret({}/{}/{}) @ {} : {} : {}",
                                    owner.account_email.as_str(),
                                    app.name.0,
                                    env.name.0,
                                    grantee.account_email.as_str(),
                                    verb,
                                    current.path.0.join("."),
                                )
                            })
                            .collect(),
                        lower_negative: Vec::new(),
                        upper_positive: Vec::new(),
                        upper_negative: Vec::new(),
                    },
                },
            )
            .await?;
        let grantee_context = deps.registry_service().client_context(&grantee.token).await;
        let by_id = grantee_context
            .base_url
            .join(&format!("/v1/agent-secrets/{}", current.id.0))?;
        if verbs.contains(&"view") {
            let body = checked_secret_response(
                grantee_context
                    .client
                    .get(by_id.clone())
                    .bearer_auth(grantee.token.secret())
                    .send()
                    .await?,
                StatusCode::OK,
            )
            .await?;
            assert_eq!(
                body["destinations"],
                serde_json::to_value(&current.destinations)?
            );
            let viewed: AgentSecretDto = serde_json::from_value(body)?;
            assert_eq!(viewed, current);
        }
        for change_value in [false, true] {
            let destinations = if change_value {
                json!(["https://combined-grant.example.com"])
            } else {
                json!(["https://destination-grant.example.com"])
            };
            let mut update =
                json!({"currentRevision": current.revision, "destinations": destinations});
            if change_value {
                update["secretValue"] = json!({"op": "set", "value": external(SchemaValue::String(SECRET_MARKERS[1].into()))});
            }
            let body = checked_secret_response(
                grantee_context
                    .client
                    .patch(by_id.clone())
                    .bearer_auth(grantee.token.secret())
                    .json(&update)
                    .send()
                    .await?,
                status,
            )
            .await?;
            if status == StatusCode::OK {
                assert_eq!(body["destinations"], destinations);
                let updated: AgentSecretDto = serde_json::from_value(body)?;
                assert_eq!(updated.revision, current.revision.next()?);
                current = updated;
            }
            let owner_url = context
                .base_url
                .join(&format!("/v1/agent-secrets/{}", current.id.0))?;
            let body = checked_secret_response(
                context
                    .client
                    .get(owner_url)
                    .bearer_auth(owner.token.secret())
                    .send()
                    .await?,
                StatusCode::OK,
            )
            .await?;
            let fetched: AgentSecretDto = serde_json::from_value(body)?;
            assert_eq!(fetched, current);
            let expected_value = if status == StatusCode::OK && change_value {
                SchemaValue::String(SECRET_MARKERS[1].into())
            } else {
                original.clone()
            };
            assert_eq!(
                read_secret_value(deps, &fetched).await?,
                Some(expected_value)
            );
        }
    }
    Ok(())
}
