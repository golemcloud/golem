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
    RegistryServiceDeleteAgentSecretError, RegistryServiceUpdateAgentSecretError,
};
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretPath, AgentSecretRevision, AgentSecretUpdate,
    CanonicalAgentSecretPath,
};
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use golem_wasm::analysis::analysed_type;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use serde_json::json;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_agent_secret_with_value(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        path: AgentSecretPath(vec!["foo".to_string(), "bar".to_string()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
    };

    let result = client.create_agent_secret(&env.id.0, &creation).await?;

    assert_eq!(
        result.path,
        CanonicalAgentSecretPath(vec!["foo".to_string(), "bar".to_string()])
    );
    assert_eq!(result.secret_type, creation.secret_type);
    assert_eq!(result.secret_value, creation.secret_value);
    assert_eq!(result.revision, AgentSecretRevision::INITIAL);

    {
        let fetched_secret = client.get_agent_secret(&result.id.0).await?;
        assert_eq!(fetched_secret, result);
    }

    {
        let all_environment_secrets = client.list_environment_agent_secrets(&env.id.0).await?;
        assert!(all_environment_secrets.values.contains(&result));
    }

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
        path: AgentSecretPath(vec![
            "firstPathSegment".to_string(),
            "SecondPathSegment".to_string(),
            "third_path_segment".to_string(),
        ]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
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
    assert_eq!(result.secret_value, creation.secret_value);
    assert_eq!(result.revision, AgentSecretRevision::INITIAL);

    {
        let fetched_secret = client.get_agent_secret(&result.id.0).await?;
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
        path: AgentSecretPath(vec!["no".into(), "value".into()]),
        secret_type: analysed_type::str(),
        secret_value: None,
    };

    let result = client.create_agent_secret(&env.id.0, &creation).await?;

    assert_eq!(result.secret_value, None);
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
        path: AgentSecretPath(vec!["dup".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
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
                path: AgentSecretPath(vec!["secret_path".into()]),
                secret_type: analysed_type::bool(),
                secret_value: Some(json!(true)),
            },
        )
        .await?;

    let result = client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(vec!["secretPath".into()]),
                secret_type: analysed_type::bool(),
                secret_value: Some(json!(true)),
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
        path: AgentSecretPath(vec!["rev".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    let updated = client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(json!(false)),
            },
        )
        .await?;

    assert_eq!(updated.secret_value, Some(json!(false)));
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
        path: AgentSecretPath(vec!["stale".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(json!(false)),
            },
        )
        .await?;

    let stale_update = client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(json!(true)),
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
        path: AgentSecretPath(vec!["unset".into()]),
        secret_type: analysed_type::str(),
        secret_value: Some(json!("hello")),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    let updated = client
        .update_agent_secret(
            &created.id.0,
            &AgentSecretUpdate {
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Unset,
            },
        )
        .await?;

    assert_eq!(updated.secret_value, None);
    assert!(updated.revision > created.revision);

    Ok(())
}

#[test]
async fn delete_secret(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = AgentSecretCreation {
        path: AgentSecretPath(vec!["delete".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
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
        path: AgentSecretPath(vec!["delete-stale".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    client
        .update_agent_secret(
            &created.id.0,
            &golem_common::model::agent_secret::AgentSecretUpdate {
                current_revision: created.revision,
                secret_value: OptionalFieldUpdate::Set(json!(false)),
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
        path: AgentSecretPath(vec!["recreate".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
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
        path: AgentSecretPath(vec!["type".into(), "creation-mismatch".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!("not-a-bool")),
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
        path: AgentSecretPath(vec!["update".into(), "type-mismatch".into()]),
        secret_type: analysed_type::bool(),
        secret_value: Some(json!(true)),
    };

    let created = client.create_agent_secret(&env.id.0, &creation).await?;

    let update = AgentSecretUpdate {
        current_revision: created.revision,
        secret_value: OptionalFieldUpdate::Set(json!("not-a-bool")),
    };

    let result = client.update_agent_secret(&created.id.0, &update).await;

    assert!(result.is_err());

    Ok(())
}
