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

use golem_client::api::RegistryServiceClient;
use golem_common::base_model::retry_policy::{
    ApiCountBoxPolicy, ApiPeriodicPolicy, ApiPredicate, ApiPredicateTrue, ApiPredicateValue,
    ApiPropertyComparison, ApiRetryPolicy, ApiTextValue,
};
use golem_common::model::retry_policy::{
    RetryPolicyCreation, RetryPolicyRevision, RetryPolicyUpdate,
};
use golem_common::model::UntypedJsonBody;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

fn predicate(predicate: ApiPredicate) -> UntypedJsonBody {
    UntypedJsonBody(serde_json::to_value(predicate).expect("API predicate must serialize"))
}

fn policy(policy: ApiRetryPolicy) -> UntypedJsonBody {
    UntypedJsonBody(serde_json::to_value(policy).expect("API retry policy must serialize"))
}

fn simple_predicate() -> UntypedJsonBody {
    predicate(ApiPredicate::True(ApiPredicateTrue {}))
}

fn simple_policy() -> UntypedJsonBody {
    policy(ApiRetryPolicy::Periodic(ApiPeriodicPolicy {
        delay_ms: 1000,
    }))
}

#[test]
#[tracing::instrument]
async fn create_and_get_retry_policy(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "test-policy".to_string(),
        priority: 10,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };

    let created = client.create_retry_policy(&env.id.0, &creation).await?;

    assert_eq!(created.name, "test-policy");
    assert_eq!(created.priority, 10);
    assert_eq!(created.predicate, creation.predicate);
    assert_eq!(created.policy, creation.policy);
    assert_eq!(created.revision, RetryPolicyRevision::INITIAL);

    {
        let fetched = client.get_retry_policy(&created.id.0).await?;
        assert_eq!(fetched, created);
    }

    {
        let all = client.get_environment_retry_policies(&env.id.0).await?;
        assert!(all.values.contains(&created));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_retry_policy(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "update-me".to_string(),
        priority: 5,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };

    let created = client.create_retry_policy(&env.id.0, &creation).await?;

    let new_predicate = predicate(ApiPredicate::PropEq(ApiPropertyComparison {
        property: "status".to_string(),
        value: ApiPredicateValue::Text(ApiTextValue {
            value: "error".to_string(),
        }),
    }));

    let new_policy = policy(ApiRetryPolicy::CountBox(ApiCountBoxPolicy {
        max_retries: 3,
        inner: Box::new(ApiRetryPolicy::Periodic(ApiPeriodicPolicy {
            delay_ms: 500,
        })),
    }));

    let update = RetryPolicyUpdate {
        current_revision: created.revision,
        priority: Some(20),
        predicate: Some(new_predicate.clone()),
        policy: Some(new_policy.clone()),
    };

    let updated = client.update_retry_policy(&created.id.0, &update).await?;

    assert_eq!(updated.priority, 20);
    assert_eq!(updated.predicate, new_predicate);
    assert_eq!(updated.policy, new_policy);
    assert!(updated.revision > created.revision);

    {
        let fetched = client.get_retry_policy(&updated.id.0).await?;
        assert_eq!(fetched, updated);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_retry_policy(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "delete-me".to_string(),
        priority: 1,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };

    let created = client.create_retry_policy(&env.id.0, &creation).await?;

    client
        .delete_retry_policy(&created.id.0, created.revision.into())
        .await?;

    let all = client.get_environment_retry_policies(&env.id.0).await?;
    assert!(
        !all.values.iter().any(|p| p.id == created.id),
        "deleted policy should not appear in environment list"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_multiple_policies_different_priorities(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let mut created_ids = Vec::new();
    for (name, priority) in [("low", 1), ("medium", 50), ("high", 100)] {
        let creation = RetryPolicyCreation {
            name: name.to_string(),
            priority,
            predicate: simple_predicate(),
            policy: simple_policy(),
        };
        let created = client.create_retry_policy(&env.id.0, &creation).await?;
        created_ids.push(created.id);
    }

    let all = client.get_environment_retry_policies(&env.id.0).await?;

    for id in &created_ids {
        assert!(
            all.values.iter().any(|p| &p.id == id),
            "policy {id} should appear in environment list"
        );
    }
    assert_eq!(all.values.len(), 3);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn environment_policy_visible_to_agent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "env-visible".to_string(),
        priority: 10,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };
    client.create_retry_policy(&env.id.0, &creation).await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "env-policy-visible");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let has = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "has_retry_policy",
            data_value!("env-visible".to_string()),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(has, Value::Bool(true));

    let names = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "list_retry_policy_names",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    match &names {
        Value::List(items) => {
            assert!(items.contains(&Value::String("env-visible".to_string())));
        }
        other => panic!("expected List, got {other:?}"),
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn multiple_environment_policies_visible_to_agent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    for (name, priority) in [("alpha", 1), ("beta", 50), ("gamma", 100)] {
        let creation = RetryPolicyCreation {
            name: name.to_string(),
            priority,
            predicate: simple_predicate(),
            policy: simple_policy(),
        };
        client.create_retry_policy(&env.id.0, &creation).await?;
    }

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "multi-env-policies");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let count = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_retry_policy_count",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    match &count {
        Value::U64(n) => assert!(*n >= 3, "expected at least 3 policies, got {n}"),
        other => panic!("expected U64, got {other:?}"),
    }

    let names = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "list_retry_policy_names",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    match &names {
        Value::List(items) => {
            for expected in ["alpha", "beta", "gamma"] {
                assert!(
                    items.contains(&Value::String(expected.to_string())),
                    "expected {expected} in list"
                );
            }
        }
        other => panic!("expected List, got {other:?}"),
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn runtime_overlay_coexists_with_environment_policy(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "env-pol".to_string(),
        priority: 10,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };
    client.create_retry_policy(&env.id.0, &creation).await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "rt-overlay-coexists");
    user.start_agent(&component.id, agent_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &agent_id,
        "set_simple_count_retry_policy",
        data_value!("rt-pol".to_string(), 5u32, 3u32),
    )
    .await?;

    let names = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "list_retry_policy_names",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    match &names {
        Value::List(items) => {
            assert!(
                items.contains(&Value::String("env-pol".to_string())),
                "expected env-pol in list"
            );
            assert!(
                items.contains(&Value::String("rt-pol".to_string())),
                "expected rt-pol in list"
            );
        }
        other => panic!("expected List, got {other:?}"),
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn runtime_overlay_overrides_environment_policy_same_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "shared-name".to_string(),
        priority: 10,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };
    client.create_retry_policy(&env.id.0, &creation).await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "rt-override-same-name");
    user.start_agent(&component.id, agent_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &agent_id,
        "set_simple_count_retry_policy",
        data_value!("shared-name".to_string(), 99u32, 1u32),
    )
    .await?;

    let names = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "list_retry_policy_names",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    match &names {
        Value::List(items) => {
            let occurrences = items
                .iter()
                .filter(|v| *v == &Value::String("shared-name".to_string()))
                .count();
            assert_eq!(occurrences, 1, "shared-name should appear exactly once");
        }
        other => panic!("expected List, got {other:?}"),
    }

    let count = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_retry_policy_count",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(count, Value::U64(1));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn environment_policy_created_while_agent_running(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "late-policy-creation");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let count = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_retry_policy_count",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(count, Value::U64(0));

    let creation = RetryPolicyCreation {
        name: "late-arrival".to_string(),
        priority: 10,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };
    client.create_retry_policy(&env.id.0, &creation).await?;

    let has = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "has_retry_policy",
            data_value!("late-arrival".to_string()),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(has, Value::Bool(true));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn environment_policy_deleted_while_agent_running(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "to-delete".to_string(),
        priority: 10,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };
    let created = client.create_retry_policy(&env.id.0, &creation).await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "policy-deletion");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let has = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "has_retry_policy",
            data_value!("to-delete".to_string()),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(has, Value::Bool(true));

    client
        .delete_retry_policy(&created.id.0, created.revision.into())
        .await?;

    let has = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "has_retry_policy",
            data_value!("to-delete".to_string()),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(has, Value::Bool(false));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn environment_policy_updated_while_agent_running(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = RetryPolicyCreation {
        name: "updatable".to_string(),
        priority: 5,
        predicate: simple_predicate(),
        policy: simple_policy(),
    };
    let created = client.create_retry_policy(&env.id.0, &creation).await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "policy-update");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let has = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "has_retry_policy",
            data_value!("updatable".to_string()),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(has, Value::Bool(true));

    let update = RetryPolicyUpdate {
        current_revision: created.revision,
        priority: Some(50),
        predicate: None,
        policy: None,
    };
    client.update_retry_policy(&created.id.0, &update).await?;

    let has = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "has_retry_policy",
            data_value!("updatable".to_string()),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(has, Value::Bool(true));

    let count = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_retry_policy_count",
            data_value!(),
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(count, Value::U64(1));

    Ok(())
}
