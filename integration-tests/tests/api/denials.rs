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

//! A denial for an agent that a request names by its ID is byte-identical to an unknown ID
//! (5.3 Rule 21).

use golem_common::agent_key;
use golem_common::model::AgentId;
use golem_common::model::agent::AgentTypeName;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use integration_tests::agents::agent_type_id;
use pretty_assertions::assert_eq;
use reqwest::StatusCode;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(EnvBasedTestDependencies);

type User = TestUserContext<EnvBasedTestDependencies>;

/// What a caller receives: the status, every header except `date`, and the body.
#[derive(Debug, PartialEq)]
struct Received {
    status: StatusCode,
    headers: Vec<(String, Vec<u8>)>,
    body: String,
}

async fn get_agent(user: &User, environment: Uuid, agent_id: &AgentId) -> anyhow::Result<Received> {
    let worker_service = user.deps.worker_service();
    let url = format!(
        "http://{}:{}/v1/runtime/envs/{environment}/agents/{agent_id}",
        worker_service.http_host(),
        worker_service.http_port(),
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.token.secret())
        .send()
        .await?;
    let status = response.status();
    let mut headers = response
        .headers()
        .iter()
        .filter(|(name, _)| *name != reqwest::header::DATE)
        .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();
    headers.sort();
    let body = response.text().await?;
    Ok(Received {
        status,
        headers,
        body,
    })
}

#[test]
#[timeout("4m")]
async fn a_denied_agent_id_gets_the_answer_of_an_unknown_agent_id(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let (_, env) = owner.app_and_env().await?;
    let component = owner
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let stranger = deps.user().await?;
    let environment = component.environment_id.0;
    let counter_type = agent_type_id(&owner, &component, &AgentTypeName("Counter".into())).await?;
    let existing = owner
        .start_agent(&component.id, agent_key!("Counter", "guarded"))
        .await?
        .agent_id;
    let unknown = AgentId::random(counter_type);

    let unknown_to_owner = get_agent(&owner, environment, &unknown).await?;
    assert_eq!(unknown_to_owner.status, StatusCode::NOT_FOUND);

    let unknown_to_stranger = get_agent(&stranger, environment, &unknown).await?;
    assert_eq!(unknown_to_stranger, unknown_to_owner);

    // The denied answer differs from the unknown one only in the ID that the request named.
    let denied = get_agent(&stranger, environment, &existing).await?;
    let denied_with_the_unknown_id = Received {
        body: denied
            .body
            .replace(&existing.to_string(), &unknown.to_string()),
        ..denied
    };
    assert_eq!(denied_with_the_unknown_id, unknown_to_owner);
    Ok(())
}
