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

use golem_client::api::{
    RegistryServiceClient, RegistryServiceCurrentLoginTokenError, RegistryServiceGetAccountError,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn can_get_information_about_own_token(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    {
        let token_info = client.current_login_token().await?;
        assert_eq!(token_info.account_id, user.account_id)
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn standalone_services_answer_missing_credentials_with_json_401(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let registry = deps.registry_service();
    let worker = deps.worker_service();
    let urls = [
        format!(
            "http://{}:{}/v1/me/token",
            registry.http_host(),
            registry.http_port()
        ),
        format!(
            "http://{}:{}/v1/components/{}/workers/agent",
            worker.http_host(),
            worker.http_port(),
            uuid::Uuid::new_v4()
        ),
    ];

    let client = reqwest::Client::builder().no_proxy().build()?;
    for url in urls {
        let response = client.get(&url).send().await?;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{url}"
        );
        let body = response.text().await?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).ok(),
            Some(serde_json::json!({
                "code": "AUTH_UNAUTHORIZED",
                "error": "Could not authenticate user using token",
            })),
            "{url}: {body}"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleting_account_revokes_tokens(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let account = client.get_account(&user.account_id.0).await?;
    client
        .delete_account(&account.id.0, account.revision.into())
        .await?;

    {
        let result = client.current_login_token().await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceCurrentLoginTokenError::Error401(_)
            ))
        ));
    }

    {
        let result = client.get_account(&user.account_id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetAccountError::Error401(_)
            ))
        ));
    }

    Ok(())
}
