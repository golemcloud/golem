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

//! The runtime REST API on `/v1/runtime/envs/{env}/...` addresses agents by key or by ID:
//! creating, upserting, looking up, listing and invoking agents, the idempotency of create,
//! expected fingerprints, the type tag of agent IDs, and the authorization of the management
//! endpoints of one agent.

use anyhow::anyhow;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::v1::{InvokeAgentRequest, invoke_agent_response};
use golem_client::api::{
    AgentClient, AgentClientLive, RegistryServiceClient, WorkerClient, WorkerClientLive,
};
use golem_client::model::{
    AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult, AgentKeyRequest,
    CompleteParameters, ComponentDto, ForkWorkerRequest, UpdateWorkerRequest,
    WorkersMetadataRequest,
};
use golem_common::base_model::type_id::TypeId;
use golem_common::base_model::worker::{
    AgentUpdateMode, RevertLastInvocations, RevertWorkerTarget,
};
use golem_common::model::agent::{AgentTypeName, OwnedAgentId, ParsedAgentKey};
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::{ExternalSchemaValue, SchemaValue};
use golem_common::{agent_key, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use integration_tests::agents::{agent_type_id, id_target, keyed_agent_id, proto_agent_key};
use pretty_assertions::assert_eq;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::HashSet;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(EnvBasedTestDependencies);

type User = TestUserContext<EnvBasedTestDependencies>;

async fn counters(deps: &EnvBasedTestDependencies) -> anyhow::Result<(User, ComponentDto)> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    Ok((user, component))
}

async fn agent_client(user: &User) -> AgentClientLive {
    user.deps
        .worker_service()
        .agent_http_client(&user.token)
        .await
}

async fn worker_client(user: &User) -> WorkerClientLive {
    user.deps
        .worker_service()
        .worker_http_client(&user.token)
        .await
}

async fn type_id(user: &User, component: &ComponentDto, name: &str) -> anyhow::Result<TypeId> {
    agent_type_id(user, component, &AgentTypeName(name.into())).await
}

fn parameters(key: &ParsedAgentKey) -> anyhow::Result<ExternalSchemaValue> {
    ExternalSchemaValue::try_from(key.parameters.value().clone()).map_err(anyhow::Error::msg)
}

fn no_parameters() -> anyhow::Result<ExternalSchemaValue> {
    ExternalSchemaValue::try_from(data_value!().into_parts().1).map_err(anyhow::Error::msg)
}

fn invocation(
    agent_id: Option<Uuid>,
    key: Option<&ParsedAgentKey>,
    method: &str,
    expected_fingerprint: Option<Uuid>,
) -> anyhow::Result<AgentInvocationRequest> {
    Ok(AgentInvocationRequest {
        agent_id,
        parameters: key.map(parameters).transpose()?,
        method: method.into(),
        method_parameters: no_parameters()?,
        mode: AgentInvocationMode::Await,
        schedule_at: None,
        expected_fingerprint,
    })
}

fn returned(result: &AgentInvocationResult) -> Option<SchemaValue> {
    result
        .result
        .clone()
        .map(|value| value.into_inner().into_parts().1)
}

/// Fails when a response of the agent API carries a component ID anywhere in its JSON.
fn assert_no_component_id(value: &Value, context: &str) {
    match value {
        Value::Object(fields) => {
            for (name, field) in fields {
                assert!(
                    name != "componentId" && name != "component_id",
                    "{context}: the response has a component ID: {value}"
                );
                assert_no_component_id(field, context);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| assert_no_component_id(item, context)),
        _ => {}
    }
}

/// Raw JSON requests to the runtime REST API of one environment. Every response is checked to
/// carry no component ID.
struct Rest {
    client: reqwest::Client,
    base: String,
    token: String,
}

impl Rest {
    fn new(user: &User, component: &ComponentDto) -> Self {
        let worker_service = user.deps.worker_service();
        Self {
            client: reqwest::Client::new(),
            base: format!(
                "http://{}:{}/v1/runtime/envs/{}",
                worker_service.http_host(),
                worker_service.http_port(),
                component.environment_id.0
            ),
            token: user.token.secret().to_string(),
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let mut request = self
            .client
            .request(method.clone(), format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        assert_no_component_id(&value, &format!("{method} {path}"));
        Ok((status, value))
    }

    async fn create(
        &self,
        key: &ParsedAgentKey,
        keyed: bool,
        owner: &str,
        idempotency_key: Option<&str>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        self.send(
            Method::POST,
            &format!("/agent-types/{}/agents", key.agent_type),
            Some(json!({
                "parameters": serde_json::to_value(parameters(key)?)?,
                "keyed": keyed,
                "env": { "OWNER": owner },
            })),
            idempotency_key,
        )
        .await
    }
}

fn agent_id_of(body: &Value) -> anyhow::Result<Uuid> {
    Ok(body["agentId"]
        .as_str()
        .ok_or_else(|| anyhow!("the response has no agent ID: {body}"))?
        .parse()?)
}

fn status<T, E: golem_client::ErrorInfo>(result: &Result<T, golem_client::Error<E>>) -> u16 {
    match result {
        Ok(_) => 200,
        Err(error) => error.status_code().unwrap_or(0),
    }
}

/// The constructor runs and the method invocations that the agent's oplog records.
async fn started_invocations(
    user: &User,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<(usize, Vec<String>)> {
    let mut constructors = 0;
    let mut methods = Vec::new();
    for entry in user.get_oplog(agent_id, OplogIndex::INITIAL).await? {
        if let PublicOplogEntry::AgentInvocationStarted(start) = entry.entry {
            match start.invocation {
                PublicAgentInvocation::AgentInitialization(_) => constructors += 1,
                PublicAgentInvocation::AgentMethodInvocation(method) => {
                    methods.push(method.method_name)
                }
                _ => {}
            }
        }
    }
    Ok((constructors, methods))
}

#[test]
#[timeout("4m")]
async fn keyed_create_returns_201_then_409_and_leaves_the_agent_unchanged(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let rest = Rest::new(&user, &component);
    let key = agent_key!("Counter", "keyed-create");

    let (status, body) = rest.create(&key, true, "first", None).await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let agent_id = OwnedAgentId::new(component.environment_id, &AgentId(agent_id_of(&body)?));
    assert_eq!(
        agent_id,
        keyed_agent_id(
            &component,
            type_id(&user, &component, "Counter").await?,
            &key
        )?
    );
    user.invoke_and_await_agent(&component, &key, "increment", data_value!())
        .await?;
    let before = user.get_worker_metadata(&agent_id).await?;

    for idempotency_key in [None, Some("keyed-create-retry")] {
        let (status, body) = rest.create(&key, true, "second", idempotency_key).await?;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(
            body["code"],
            json!(golem_common::base_model::api::error_code::AGENT_ALREADY_EXISTS)
        );
    }

    let after = user.get_worker_metadata(&agent_id).await?;
    assert_eq!(after.fingerprint, before.fingerprint);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.env.get("OWNER").map(String::as_str), Some("first"));
    assert_eq!(started_invocations(&user, &agent_id).await?.0, 1);
    let count = user
        .invoke_and_await_agent(&component, &key, "count", data_value!())
        .await?;
    assert_eq!(count.into_return_value(), Some(SchemaValue::U32(1)));
    Ok(())
}

#[test]
#[timeout("4m")]
async fn create_without_a_key_twice_creates_two_agents(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let rest = Rest::new(&user, &component);
    let client = agent_client(&user).await;
    let counter_type = type_id(&user, &component, "Counter").await?;
    let key = agent_key!("Counter", "unkeyed-create");

    let mut created = Vec::new();
    for _ in 0..2 {
        let (status, body) = rest.create(&key, false, "unkeyed", None).await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        created.push(AgentId(agent_id_of(&body)?));
    }
    assert_ne!(created[0], created[1]);
    for agent_id in &created {
        assert_eq!(agent_id.type_id(), Some(counter_type));
        let metadata = user
            .get_worker_metadata(&OwnedAgentId::new(component.environment_id, agent_id))
            .await?;
        assert_eq!(metadata.canonical_key, None);
        assert_eq!(
            metadata.agent_type_name,
            Some(AgentTypeName("Counter".into()))
        );
    }

    let first = client
        .invoke_agent(
            &component.environment_id.0,
            "Counter",
            None,
            &invocation(Some(created[0].0), None, "increment", None)?,
        )
        .await?;
    assert_eq!(returned(&first), Some(SchemaValue::U32(1)));
    let second = client
        .invoke_agent(
            &component.environment_id.0,
            "Counter",
            None,
            &invocation(Some(created[1].0), None, "count", None)?,
        )
        .await?;
    assert_eq!(returned(&second), Some(SchemaValue::U32(0)));

    // Neither agent owns the key.
    let lookup = client
        .lookup_agent(
            &component.environment_id.0,
            "Counter",
            &AgentKeyRequest {
                parameters: parameters(&key)?,
            },
        )
        .await;
    assert_eq!(status(&lookup), 404, "{lookup:?}");
    Ok(())
}

#[test]
#[timeout("4m")]
async fn create_retried_with_the_same_idempotency_key_returns_the_first_agent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let rest = Rest::new(&user, &component);
    let key = agent_key!("Counter", "idempotent-create");
    let idempotency_key = Uuid::new_v4().to_string();

    let mut created = Vec::new();
    for _ in 0..2 {
        let (status, body) = rest
            .create(&key, false, "retried", Some(&idempotency_key))
            .await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        created.push(body);
    }
    assert_eq!(created[0], created[1]);
    let agent_id = OwnedAgentId::new(
        component.environment_id,
        &AgentId(agent_id_of(&created[0])?),
    );
    assert_eq!(started_invocations(&user, &agent_id).await?.0, 1);

    let (status, body) = rest
        .create(&key, false, "retried", Some(&Uuid::new_v4().to_string()))
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_ne!(AgentId(agent_id_of(&body)?), agent_id.agent_id);

    let (status, body) = rest
        .create(
            &agent_key!("Counter", "idempotent-create-changed"),
            false,
            "retried",
            Some(&idempotency_key),
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(started_invocations(&user, &agent_id).await?.0, 1);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn the_idempotency_key_is_read_only_from_the_header(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let rest = Rest::new(&user, &component);
    let key = agent_key!("Counter", "header-only");
    let idempotency_key = Uuid::new_v4().to_string();

    let mut created = HashSet::new();
    for _ in 0..2 {
        let (status, body) = rest
            .send(
                Method::POST,
                "/agent-types/Counter/agents",
                Some(json!({
                    "parameters": serde_json::to_value(parameters(&key)?)?,
                    "idempotencyKey": idempotency_key,
                })),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        created.insert(agent_id_of(&body)?);
    }
    assert_eq!(created.len(), 2, "a body field must not join two creates");

    let mut request = serde_json::to_value(invocation(None, Some(&key), "increment", None)?)?;
    request["idempotencyKey"] = json!(idempotency_key);
    let mut results = Vec::new();
    for _ in 0..2 {
        let (status, body) = rest
            .send(
                Method::POST,
                "/agent-types/Counter/invoke",
                Some(request.clone()),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        results.push(body);
    }
    assert_ne!(results[0]["idempotencyKey"], results[1]["idempotencyKey"]);

    let request = serde_json::to_value(invocation(None, Some(&key), "increment", None)?)?;
    let mut keyed = Vec::new();
    for _ in 0..2 {
        let (status, body) = rest
            .send(
                Method::POST,
                "/agent-types/Counter/invoke",
                Some(request.clone()),
                Some(&idempotency_key),
            )
            .await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        keyed.push(body);
    }
    assert_eq!(keyed[0], keyed[1]);

    let count = user
        .invoke_and_await_agent(&component, &key, "count", data_value!())
        .await?;
    assert_eq!(count.into_return_value(), Some(SchemaValue::U32(3)));
    Ok(())
}

/// Internal ephemeral calls use the IDs allocated by their caller. REST allocates the
/// instance and reuses it when an authenticated caller repeats its idempotency key.
#[test]
#[timeout("4m")]
async fn ephemeral_grpc_invocations_preserve_explicit_ids_and_rest_retries_reuse_the_id(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let key = agent_key!("EphemeralCounter", "unscoped");
    let type_id = type_id(&deps.admin().await, &component, "EphemeralCounter").await?;
    let idempotency_key = Uuid::new_v4().to_string();

    let worker_service = deps.worker_service();
    let mut grpc = WorkerServiceClient::connect(format!(
        "http://{}:{}",
        worker_service.grpc_host(),
        worker_service.gprc_port()
    ))
    .await?;
    let method_parameters: golem_schema::proto::golem::schema::SchemaValue = data_value!()
        .into_parts()
        .1
        .try_into()
        .map_err(|error| anyhow!("{error:?}"))?;
    let mut unscoped = Vec::new();
    for _ in 0..2 {
        let expected_agent_id = AgentId::random(type_id);
        let response = grpc
            .invoke_agent(InvokeAgentRequest {
                target: Some(id_target(expected_agent_id)),
                constructor: Some(proto_agent_key(&key)?),
                method_name: Some("get_id".into()),
                method_parameters: Some(method_parameters.clone()),
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                idempotency_key: Some(IdempotencyKey::new(idempotency_key.clone()).into()),
                auth_ctx: Some(AuthCtx::System.into()),
                caller: Some(golem_common::model::caller::Caller::anonymous().into()),
                environment_id: Some(component.environment_id.into()),
                component_id: Some(component.id.into()),
                ..Default::default()
            })
            .await?
            .into_inner();
        let success = match response.result {
            Some(invoke_agent_response::Result::Success(success)) => success,
            other => anyhow::bail!("the unscoped invocation failed: {other:?}"),
        };
        let agent_id = success
            .agent_id
            .ok_or_else(|| anyhow!("the invocation reported no agent ID"))?;
        let agent_id = AgentId::try_from(agent_id).map_err(|error| anyhow!("{error:?}"))?;
        assert_eq!(agent_id, expected_agent_id);
        unscoped.push(agent_id);
    }
    assert_ne!(unscoped[0], unscoped[1]);

    let client = agent_client(&user).await;
    let mut scoped = Vec::new();
    for _ in 0..2 {
        let result = client
            .invoke_agent(
                &component.environment_id.0,
                "EphemeralCounter",
                Some(&idempotency_key),
                &invocation(None, Some(&key), "get_id", None)?,
            )
            .await?;
        scoped.push(result.agent_id);
    }
    assert_eq!(scoped[0], scoped[1]);
    assert!(!unscoped.iter().any(|agent_id| agent_id.0 == scoped[0]));
    Ok(())
}

#[test]
#[timeout("4m")]
async fn upsert_lookup_and_keyed_invoke_reach_the_keyed_agent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let client = agent_client(&user).await;
    let environment = component.environment_id.0;
    let counter_type = type_id(&user, &component, "Counter").await?;
    let key = agent_key!("Counter", "upserted");
    let request = AgentKeyRequest {
        parameters: parameters(&key)?,
    };

    for _ in 0..2 {
        let missing = client.lookup_agent(&environment, "Counter", &request).await;
        assert_eq!(status(&missing), 404, "{missing:?}");
    }
    let expected = keyed_agent_id(&component, counter_type, &key)?;
    assert!(user.get_worker_metadata_opt(&expected).await?.is_none());

    let first = client
        .upsert_agent(&environment, "Counter", &request)
        .await?;
    let second = client
        .upsert_agent(&environment, "Counter", &request)
        .await?;
    assert_eq!(first, second);
    assert_eq!(AgentId(first.agent_id), expected.agent_id);
    assert_eq!(expected.agent_id.type_tag(), counter_type.get());
    assert_eq!(
        client
            .lookup_agent(&environment, "Counter", &request)
            .await?,
        first
    );

    let invoked_key = agent_key!("Counter", "invoked-by-key");
    let invoked = client
        .invoke_agent(
            &environment,
            "Counter",
            None,
            &invocation(None, Some(&invoked_key), "increment", None)?,
        )
        .await?;
    assert_eq!(returned(&invoked), Some(SchemaValue::U32(1)));
    let invoked_id = keyed_agent_id(&component, counter_type, &invoked_key)?;
    assert_eq!(AgentId(invoked.agent_id), invoked_id.agent_id);
    let looked_up = client
        .lookup_agent(
            &environment,
            "Counter",
            &AgentKeyRequest {
                parameters: parameters(&invoked_key)?,
            },
        )
        .await?;
    assert_eq!(AgentId(looked_up.agent_id), invoked_id.agent_id);
    let metadata = user.get_worker_metadata(&invoked_id).await?;
    assert_eq!(looked_up.agent_fingerprint, metadata.fingerprint.0);
    assert_eq!(
        metadata.canonical_key,
        Some(r#"Counter("invoked-by-key")"#.to_string())
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_stale_expected_fingerprint_after_recreation_is_not_found_and_runs_nothing(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let client = agent_client(&user).await;
    let workers = worker_client(&user).await;
    let environment = component.environment_id.0;
    let key = agent_key!("Counter", "recreated");
    let request = AgentKeyRequest {
        parameters: parameters(&key)?,
    };

    let first = client
        .upsert_agent(&environment, "Counter", &request)
        .await?;
    client
        .invoke_agent(
            &environment,
            "Counter",
            None,
            &invocation(None, Some(&key), "increment", Some(first.agent_fingerprint))?,
        )
        .await?;
    workers
        .delete_worker(
            &environment,
            &first.agent_id,
            Some(&first.agent_fingerprint),
        )
        .await?;
    let second = client
        .upsert_agent(&environment, "Counter", &request)
        .await?;
    assert_eq!(second.agent_id, first.agent_id);
    assert_ne!(second.agent_fingerprint, first.agent_fingerprint);

    for (agent_id, key) in [(None, Some(&key)), (Some(first.agent_id), None)] {
        let stale = client
            .invoke_agent(
                &environment,
                "Counter",
                None,
                &invocation(agent_id, key, "increment", Some(first.agent_fingerprint))?,
            )
            .await;
        assert_eq!(status(&stale), 404, "{stale:?}");
    }
    let stale = workers
        .get_worker_metadata(
            &environment,
            &first.agent_id,
            Some(&first.agent_fingerprint),
        )
        .await;
    assert_eq!(status(&stale), 404, "{stale:?}");

    let agent_id = OwnedAgentId::new(component.environment_id, &AgentId(second.agent_id));
    assert_eq!(
        started_invocations(&user, &agent_id).await?.1,
        Vec::<String>::new()
    );
    let current = client
        .invoke_agent(
            &environment,
            "Counter",
            None,
            &invocation(
                Some(second.agent_id),
                None,
                "count",
                Some(second.agent_fingerprint),
            )?,
        )
        .await?;
    assert_eq!(returned(&current), Some(SchemaValue::U32(0)));
    Ok(())
}

#[test]
#[timeout("4m")]
async fn per_type_list_and_find_return_only_the_agents_of_the_type(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let client = agent_client(&user).await;
    let environment = component.environment_id.0;

    let mut expected: Vec<(&str, HashSet<Uuid>)> = Vec::new();
    for (agent_type, count) in [("Counter", 3), ("ScheduleCounter", 2)] {
        let mut ids = HashSet::new();
        for index in 0..count {
            let key = agent_key!(agent_type, format!("listed-{index}"));
            let reference = client
                .upsert_agent(
                    &environment,
                    agent_type,
                    &AgentKeyRequest {
                        parameters: parameters(&key)?,
                    },
                )
                .await?;
            ids.insert(reference.agent_id);
        }
        expected.push((agent_type, ids));
    }

    for (agent_type, ids) in &expected {
        for component_name in [
            None,
            Some(component.component_name.0.as_str()),
            Some("another-component"),
        ] {
            let listed = client
                .get_agents_metadata(
                    &environment,
                    agent_type,
                    component_name,
                    None,
                    None,
                    Some(100),
                    None,
                )
                .await?;
            let found = client
                .find_agents_metadata(
                    &environment,
                    agent_type,
                    component_name,
                    &WorkersMetadataRequest {
                        filter: None,
                        cursor: None,
                        count: Some(100),
                        precise: None,
                    },
                )
                .await?;
            for response in [listed, found] {
                assert!(response.cursor.is_none());
                assert!(
                    response.workers.iter().all(|worker| worker.agent_type_name
                        == Some(AgentTypeName(agent_type.to_string()))),
                    "{agent_type}: {:?}",
                    response.workers
                );
                assert_eq!(
                    &response
                        .workers
                        .iter()
                        .map(|worker| worker.agent_id.0)
                        .collect::<HashSet<_>>(),
                    &if component_name == Some("another-component") {
                        HashSet::new()
                    } else {
                        ids.clone()
                    }
                );
            }
        }
    }
    Ok(())
}

#[test]
#[timeout("4m")]
async fn agent_responses_have_no_component_id(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let rest = Rest::new(&user, &component);
    let key = agent_key!("Counter", "no-component-id");
    let key_body = json!({ "parameters": serde_json::to_value(parameters(&key)?)? });

    let (status, created) = rest.create(&key, true, "shape", None).await?;
    assert_eq!(status, StatusCode::CREATED);
    let agent_id = agent_id_of(&created)?;
    let (status, _) = rest.create(&key, true, "shape", None).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    for path in [
        "/agent-types/Counter/agents/upsert",
        "/agent-types/Counter/agents/lookup",
    ] {
        let (status, body) = rest
            .send(Method::POST, path, Some(key_body.clone()), None)
            .await?;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let (status, _) = rest
        .send(
            Method::POST,
            "/agent-types/Counter/invoke",
            Some(serde_json::to_value(invocation(
                None,
                Some(&key),
                "increment",
                None,
            )?)?),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    for (method, path, body) in [
        (Method::GET, "/agent-types/Counter/agents".to_string(), None),
        (
            Method::POST,
            "/agent-types/Counter/agents/find".to_string(),
            Some(json!({})),
        ),
        (Method::GET, format!("/agents/{agent_id}"), None),
        (
            Method::GET,
            format!("/agents/{agent_id}/oplog?from=0&count=100"),
            None,
        ),
    ] {
        let (status, body) = rest.send(method, &path, body, None).await?;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let unknown = AgentId::random(type_id(&user, &component, "Counter").await?);
    let (status, _) = rest
        .send(Method::GET, &format!("/agents/{}", unknown.0), None, None)
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[test]
#[timeout("6m")]
async fn the_type_tag_of_agent_ids_is_the_registry_type_id_across_redeploys(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (user, component) = counters(deps).await?;
    let client = agent_client(&user).await;
    let registry = user.registry_service_client().await;
    let environment = component.environment_id.0;

    let first_revision = user
        .get_last_deployment_revision(&component.environment_id)?
        .get();
    let first_type = registry
        .get_deployment_agent_type(&environment, first_revision, "SnapshotCounter")
        .await?
        .type_id;
    assert_eq!(
        type_id(&user, &component, "SnapshotCounter").await?,
        first_type
    );

    let before_key = agent_key!("SnapshotCounter", "before-redeploy");
    let before = client
        .upsert_agent(
            &environment,
            "SnapshotCounter",
            &AgentKeyRequest {
                parameters: parameters(&before_key)?,
            },
        )
        .await?;
    assert_eq!(AgentId(before.agent_id).type_tag(), first_type.get());
    client
        .invoke_agent(
            &environment,
            "SnapshotCounter",
            None,
            &invocation(None, Some(&before_key), "increment", None)?,
        )
        .await?;

    user.update_component(&component.id, "it_agent_counters_v2_release")
        .await?;
    let second_revision = user
        .get_last_deployment_revision(&component.environment_id)?
        .get();
    assert!(second_revision > first_revision);
    let second_type = registry
        .get_deployment_agent_type(&environment, second_revision, "SnapshotCounter")
        .await?
        .type_id;
    assert_eq!(second_type, first_type);

    let after_key = agent_key!("SnapshotCounter", "after-redeploy");
    let after = client
        .upsert_agent(
            &environment,
            "SnapshotCounter",
            &AgentKeyRequest {
                parameters: parameters(&after_key)?,
            },
        )
        .await?;
    assert_eq!(AgentId(after.agent_id).type_tag(), second_type.get());
    let version = client
        .invoke_agent(
            &environment,
            "SnapshotCounter",
            None,
            &invocation(Some(after.agent_id), None, "component_version", None)?,
        )
        .await?;
    assert_eq!(returned(&version), Some(SchemaValue::U32(2)));

    let again = client
        .upsert_agent(
            &environment,
            "SnapshotCounter",
            &AgentKeyRequest {
                parameters: parameters(&before_key)?,
            },
        )
        .await?;
    assert_eq!(again, before);
    let count = client
        .invoke_agent(
            &environment,
            "SnapshotCounter",
            None,
            &invocation(Some(before.agent_id), None, "get", None)?,
        )
        .await?;
    assert_eq!(returned(&count), Some(SchemaValue::U32(1)));
    Ok(())
}

/// The status of every management endpoint of one agent.
async fn management_statuses(
    client: &WorkerClientLive,
    environment: Uuid,
    agent: Uuid,
    revision: u64,
) -> Vec<(&'static str, u16)> {
    vec![
        (
            "get_worker_metadata",
            status(&client.get_worker_metadata(&environment, &agent, None).await),
        ),
        (
            "complete_promise",
            status(
                &client
                    .complete_promise(
                        &environment,
                        &agent,
                        &CompleteParameters {
                            oplog_idx: 1,
                            fingerprint: Uuid::new_v4(),
                            data: vec![],
                        },
                    )
                    .await,
            ),
        ),
        (
            "interrupt_worker",
            status(
                &client
                    .interrupt_worker(&environment, &agent, None, None)
                    .await,
            ),
        ),
        (
            "resume_worker",
            status(&client.resume_worker(&environment, &agent, None).await),
        ),
        (
            "update_worker",
            status(
                &client
                    .update_worker(
                        &environment,
                        &agent,
                        None,
                        &UpdateWorkerRequest {
                            mode: AgentUpdateMode::Automatic,
                            target_revision: revision,
                            disable_wakeup: None,
                        },
                    )
                    .await,
            ),
        ),
        (
            "get_oplog",
            status(
                &client
                    .get_oplog(
                        &environment,
                        &agent,
                        Some(0),
                        10,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await,
            ),
        ),
        (
            "get_oplog_entry",
            status(
                &client
                    .get_oplog_entry(&environment, &agent, 1, None, None)
                    .await,
            ),
        ),
        (
            "get_files",
            status(&client.get_files(&environment, &agent, "/", None).await),
        ),
        (
            "get_agent_wallet",
            status(&client.get_agent_wallet(&environment, &agent, None).await),
        ),
        (
            "get_file_content",
            status(
                &client
                    .get_file_content(&environment, &agent, "/missing.txt", None)
                    .await,
            ),
        ),
        (
            "activate_plugin",
            status(&client.activate_plugin(&environment, &agent, 0, None).await),
        ),
        (
            "deactivate_plugin",
            status(
                &client
                    .deactivate_plugin(&environment, &agent, 0, None)
                    .await,
            ),
        ),
        (
            "revert_worker",
            status(
                &client
                    .revert_worker(
                        &environment,
                        &agent,
                        None,
                        &RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                            number_of_invocations: 1,
                        }),
                    )
                    .await,
            ),
        ),
        (
            "fork_worker",
            status(
                &client
                    .fork_worker(
                        &environment,
                        &agent,
                        None,
                        &ForkWorkerRequest {
                            oplog_index_cutoff: 1,
                        },
                    )
                    .await,
            ),
        ),
        (
            "cancel_invocation",
            status(
                &client
                    .cancel_invocation(&environment, &agent, &Uuid::new_v4().to_string(), None)
                    .await,
            ),
        ),
        (
            "delete_worker",
            status(&client.delete_worker(&environment, &agent, None).await),
        ),
    ]
}

fn assert_all(statuses: Vec<(&'static str, u16)>, expected: u16, context: &str) {
    let unexpected = statuses
        .into_iter()
        .filter(|(_, status)| *status != expected)
        .collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "{context}: expected {expected} from every endpoint, got {unexpected:?}"
    );
}

#[test]
#[timeout("4m")]
async fn management_endpoints_deny_strangers_and_report_unknown_agents_to_the_owner(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (owner, component) = counters(deps).await?;
    let stranger = deps.user().await?;
    let environment = component.environment_id.0;
    let revision = component.revision.get();
    let counter_type = type_id(&owner, &component, "Counter").await?;

    let key = agent_key!("Counter", "guarded");
    let existing = owner.start_agent(&component.id, key.clone()).await?;
    let unknown = AgentId::random(counter_type);

    let strangers = worker_client(&stranger).await;
    assert_all(
        management_statuses(&strangers, environment, existing.agent_id.0, revision).await,
        403,
        "a stranger on an existing agent",
    );
    assert_all(
        management_statuses(&strangers, environment, unknown.0, revision).await,
        403,
        "a stranger on an unknown agent",
    );
    assert!(owner.get_worker_metadata_opt(&existing).await?.is_some());

    // The per-type endpoints do not show a stranger the agent type, so the existing and the
    // unknown agent give the same answer, and nothing is created.
    let stranger_agents = agent_client(&stranger).await;
    for agent_id in [existing.agent_id.0, unknown.0] {
        let invoked = stranger_agents
            .invoke_agent(
                &environment,
                "Counter",
                None,
                &invocation(Some(agent_id), None, "increment", None)?,
            )
            .await;
        assert_eq!(status(&invoked), 404, "{invoked:?}");
    }
    let unknown_key = agent_key!("Counter", "stranger-upsert");
    let upserted = stranger_agents
        .upsert_agent(
            &environment,
            "Counter",
            &AgentKeyRequest {
                parameters: parameters(&unknown_key)?,
            },
        )
        .await;
    assert_eq!(status(&upserted), 404, "{upserted:?}");
    assert!(
        owner
            .get_worker_metadata_opt(&keyed_agent_id(&component, counter_type, &unknown_key)?)
            .await?
            .is_none()
    );

    let owners = worker_client(&owner).await;
    assert_all(
        management_statuses(&owners, environment, unknown.0, revision).await,
        404,
        "the owner on an unknown agent",
    );
    assert!(
        owner
            .get_worker_metadata_opt(&OwnedAgentId::new(component.environment_id, &unknown))
            .await?
            .is_none()
    );
    let count = owner
        .invoke_and_await_agent(&component, &key, "count", data_value!())
        .await?;
    assert_eq!(count.into_return_value(), Some(SchemaValue::U32(0)));
    Ok(())
}
