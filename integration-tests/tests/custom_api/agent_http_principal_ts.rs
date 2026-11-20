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

use super::assert_json_content_type;
use crate::custom_api::http_test_context::{HttpTestContext, make_test_context};
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentSecurity, TestSessionHeaderAgentSecurity,
};
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use serde_json::json;
use test_r::{define_matrix_dimension, test_dep};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("postgres")]
    EnvBasedTestDependencies
);
inherit_test_dep!(
    #[tagged_as("sqlite")]
    EnvBasedTestDependencies
);

async fn build_test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    make_test_context(
        deps,
        vec![(
            AgentTypeName("PrincipalAgent".to_string()),
            HttpApiDeploymentAgentOptions {
                security: Some(HttpApiDeploymentAgentSecurity::TestSessionHeader(
                    TestSessionHeaderAgentSecurity {
                        header_name: "x-golem-test-session".to_string(),
                    },
                )),
            },
        )],
        "golem_it_agent_sdk_ts",
        "golem-it:agent-sdk-ts",
    )
    .await
    .unwrap()
}

#[test_dep(scope = PerWorker)]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    build_test_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "postgres")]
async fn test_context_postgres(
    #[tagged_as("postgres")] deps: &EnvBasedTestDependencies,
) -> HttpTestContext {
    build_test_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite")]
async fn test_context_sqlite(
    #[tagged_as("sqlite")] deps: &EnvBasedTestDependencies,
) -> HttpTestContext {
    build_test_context(deps).await
}

/// An anonymous caller has no reference.
fn assert_anonymous(value: &serde_json::Value) {
    assert_eq!(value["kind"], "anonymous");
    assert!(
        value.get("ref").is_none_or(serde_json::Value::is_null),
        "{value}"
    );
}

define_matrix_dimension!(db: HttpTestContext -> "postgres", "sqlite");

#[test]
#[tracing::instrument]
async fn principal_auto_injection(#[dimension(db)] agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/principal-agent/test-agent/echo-principal")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_json_content_type(&response);

    let body: serde_json::Value = response.json().await?;
    assert_anonymous(&body["value"]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn principal_auto_injection_middle_segment(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/principal-agent/test-agent/echo-principal-mid/foo-value/1")?,
        )
        .send()
        .await?;

    assert_json_content_type(&response);
    let body: serde_json::Value = response.json().await?;

    assert_anonymous(&body["value"]);
    assert_eq!(body["foo"], "foo-value");
    assert_eq!(body["bar"], 1.0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn principal_auto_injection_last_segment(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/principal-agent/test-agent/echo-principal-last/foo-value/2")?,
        )
        .send()
        .await?;

    assert_json_content_type(&response);
    let body: serde_json::Value = response.json().await?;

    assert_anonymous(&body["value"]);
    assert_eq!(body["foo"], "foo-value");
    assert_eq!(body["bar"], 2.0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn default_test_header_oidc_principal(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/principal-agent/test-agent/authed-principal")?,
        )
        .header("x-golem-test-session", "{}")
        .send()
        .await?;

    assert_json_content_type(&response);
    let body: serde_json::Value = response.json().await?;
    // The pair is the session's issuer and subject, never the email or the name.
    assert_eq!(
        body,
        json!({
            "value": {
                "kind": "user",
                "ref": { "issuer": "http://test-idp.com", "subject": "test-user" }
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_header_oidc_principal_with_overrides(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/principal-agent/test-agent/authed-principal")?,
        )
        .header(
            "x-golem-test-session",
            "{ \"subject\": \"bob\", \"email\": \"bob@golem.cloud\"}",
        )
        .send()
        .await?;

    assert_json_content_type(&response);
    let body: serde_json::Value = response.json().await?;
    // The email of the session never takes part in the pair.
    assert_eq!(
        body,
        json!({
            "value": {
                "kind": "user",
                "ref": { "issuer": "http://test-idp.com", "subject": "bob" }
            }
        })
    );

    Ok(())
}

/// The callers of the `authedPrincipal` starts in an oplog.
fn authed_principal_callers(
    oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
) -> Vec<golem_common::model::caller::PublicCaller> {
    use golem_common::model::oplog::public_oplog_entry::AgentInvocationStartedParams;
    use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntry};
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                invocation: PublicAgentInvocation::AgentMethodInvocation(params),
                ..
            }) if params.method_name == "authedPrincipal" => Some(params.caller.clone()),
            _ => None,
        })
        .collect()
}

/// Shares a permission of the owner of `agent` with `target`.
async fn share_with(
    agent: &HttpTestContext,
    target: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
    name: &str,
    permission: String,
) -> anyhow::Result<()> {
    use golem_client::api::RegistryServiceClient;
    use golem_common::model::permission_share::{
        PermissionShareCreation, PermissionShareData, PermissionShareName,
    };
    use golem_test_framework::config::TestDependencies;
    let client = agent
        .user
        .deps
        .registry_service()
        .client(&agent.user.token)
        .await;
    client
        .create_permission_share(
            &agent.user.account_id.0,
            &PermissionShareCreation {
                target_account_email: target.account_email.clone(),
                name: PermissionShareName(name.to_string()),
                data: PermissionShareData {
                    lower_positive: vec![permission],
                    lower_negative: Vec::new(),
                    upper_positive: Vec::new(),
                    upper_negative: Vec::new(),
                },
            },
        )
        .await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn public_oplog_shows_claims_only_on_request_to_an_oplog_reader(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    use golem_client::api::{AgentClient, WorkerClient, WorkerError};
    use golem_common::model::caller::{Pair, PublicPrincipal};
    use golem_common::model::oplog::{OplogInclude, OplogIndex};
    use golem_test_framework::config::TestDependencies;
    use golem_test_framework::dsl::TestDsl;

    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/principal-agent/test-agent/authed-principal")?,
        )
        .header(
            "x-golem-test-session",
            "{ \"subject\": \"bob\", \"email\": \"bob@golem.cloud\"}",
        )
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // The agent that the route created, found by its key.
    let key = golem_common::agent_key!("PrincipalAgent", "test-agent");
    let reference = agent
        .user
        .deps
        .worker_service()
        .agent_http_client(&agent.user.token)
        .await
        .lookup_agent(
            &agent.env_id.0,
            "PrincipalAgent",
            &golem_client::model::AgentKeyRequest {
                parameters: golem_common::schema::ExternalSchemaValue::try_from(
                    key.parameters.value().clone(),
                )
                .map_err(anyhow::Error::msg)?,
            },
        )
        .await?;
    let agent_id = golem_common::model::OwnedAgentId::new(
        agent.env_id,
        &golem_common::model::AgentId(reference.agent_id),
    );
    let bob = Pair::new("http://test-idp.com", "bob");

    // The owner reads the oplog: bob in both roles, without claims.
    let callers =
        authed_principal_callers(&agent.user.get_oplog(&agent_id, OplogIndex::INITIAL).await?);
    assert!(!callers.is_empty());
    for caller in &callers {
        assert!(matches!(&caller.original, PublicPrincipal::User(user) if user.pair == bob));
        assert_eq!(caller.immediate, caller.original);
        assert!(caller.original.claims().is_none(), "{caller:?}");
    }

    // The owner asks for the claims: both roles show them.
    let callers = authed_principal_callers(
        &agent
            .user
            .get_oplog_with_claims(&agent_id, OplogIndex::INITIAL)
            .await?,
    );
    assert!(!callers.is_empty());
    for caller in &callers {
        for role in [&caller.original, &caller.immediate] {
            assert_eq!(
                role.claims().unwrap().to_value().unwrap()["sub"],
                "bob",
                "{caller:?}"
            );
        }
    }

    // Another value of `include` is refused.
    let deps = &agent.user.deps;
    let owner_client = deps
        .worker_service()
        .worker_http_client(&agent.user.token)
        .await;
    let mut url = owner_client.context.base_url.clone();
    url.path_segments_mut().unwrap().extend([
        "v1",
        "runtime",
        "envs",
        &agent.env_id.0.to_string(),
        "agents",
        &agent_id.agent_id.to_string(),
        "oplog",
    ]);
    url.query_pairs_mut()
        .append_pair("count", "10")
        .append_pair("include", "other");
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(agent.user.token.secret())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    // An account that may see the application but not the oplog gets no claims.
    let viewer = deps.user().await?;
    share_with(
        agent,
        &viewer,
        "app-view",
        format!(
            "application({}/{}) @ {} : view :",
            agent.user.account_email.as_str(),
            agent.application_name,
            viewer.account_email.as_str()
        ),
    )
    .await?;
    let viewer_client = deps
        .worker_service()
        .worker_http_client(&viewer.token)
        .await;
    let result = viewer_client
        .get_oplog(
            &agent.env_id.0,
            &agent_id.agent_id.0,
            Some(1),
            100,
            None,
            None,
            Some(&OplogInclude::Claims),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(
        matches!(
            result,
            Err(golem_client::Error::Item(WorkerError::Error403(_)))
        ),
        "{result:?}"
    );

    // An account with the oplog read permission gets the claims.
    let reader = deps.user().await?;
    share_with(
        agent,
        &reader,
        "oplog-read",
        format!(
            "oplog({}/{}/{}/{}/{}) @ {} : read : *",
            agent.user.account_email.as_str(),
            agent.application_name,
            agent.environment_name,
            "golem-it:agent-sdk-ts",
            agent_id.agent_id,
            reader.account_email.as_str()
        ),
    )
    .await?;
    let reader_client = deps
        .worker_service()
        .worker_http_client(&reader.token)
        .await;
    let page = reader_client
        .get_oplog(
            &agent.env_id.0,
            &agent_id.agent_id.0,
            Some(1),
            100,
            None,
            None,
            Some(&OplogInclude::Claims),
            None,
            None,
            None,
            None,
        )
        .await?;
    let entries = page
        .entries
        .into_iter()
        .filter_map(|entry| match entry {
            golem_common::model::oplog::PublicOplogPageEntry::Included(entry) => Some(entry),
            golem_common::model::oplog::PublicOplogPageEntry::Omitted(_) => None,
        })
        .collect::<Vec<_>>();
    let callers = authed_principal_callers(&entries);
    assert!(!callers.is_empty());
    for caller in &callers {
        assert_eq!(
            caller.original.claims().unwrap().to_value().unwrap()["sub"],
            "bob",
            "{caller:?}"
        );
    }

    Ok(())
}
