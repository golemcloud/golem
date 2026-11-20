use super::http_test_context::{HttpTestContext, make_test_context};
use golem_api_grpc::proto::golem::registry::v1::{
    GetActiveRoutesForDomainRequest, get_active_routes_for_domain_response,
    registry_service_client::RegistryServiceClient,
};
use golem_client::model::ComponentDto;
use golem_common::agent_key;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::agent::OwnedAgentId;
use golem_common::model::{AgentId, AgentStatus};
use golem_service_base::custom_api::{CompiledRoutes, RouteBehaviour};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use integration_tests::agents::{agent_type_id, create_agent};
use reqwest::{Method, Response, StatusCode};
use serde_json::{Value, json};
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(EnvBasedTestDependencies);

async fn context(deps: &EnvBasedTestDependencies) -> anyhow::Result<HttpTestContext> {
    make_test_context(
        deps,
        [
            "SelectedQueryAgent",
            "SelectedRequiredQueryAgent",
            "SelectedPathAgent",
            "SelectedRequiredPathAgent",
            "DurableStreamAgent",
            "PhantomStreamAgent",
            "EphemeralStreamAgent",
        ]
        .into_iter()
        .map(|name| {
            (
                AgentTypeName(name.into()),
                HttpApiDeploymentAgentOptions::default(),
            )
        })
        .collect(),
        "golem_it_agent_sdk_rust_release",
        "golem-it:agent-sdk-rust",
    )
    .await
}

fn selected(path: &str, selector: Option<Uuid>) -> String {
    match selector {
        Some(selector) => format!("{path}?instance={selector}"),
        None => path.into(),
    }
}

async fn state(agent: &HttpTestContext, path: &str, expected: u32) -> anyhow::Result<()> {
    let response = agent.client.get(agent.base_url.join(path)?).send().await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await?, json!(expected));
    Ok(())
}

async fn set(agent: &HttpTestContext, path: &str) -> anyhow::Result<()> {
    agent
        .client
        .post(agent.base_url.join(path)?)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

#[test]
#[timeout("240s")]
async fn selected_agent_ids_bind_existing_agents_and_never_create(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let agent = context(deps).await?;
    let id = Uuid::new_v4().to_string();
    let base = format!("/selected-query/{id}");
    set(&agent, &format!("{base}/set/11")).await?;
    let fork = agent
        .client
        .post(agent.base_url.join(&format!("{base}/fork/37"))?)
        .send()
        .await?
        .error_for_status()?
        .json::<String>()
        .await?
        .parse::<Uuid>()?;
    state(&agent, &selected(&format!("{base}/state"), Some(fork)), 37).await?;
    state(&agent, &format!("{base}/state"), 11).await?;

    let chosen = existing_agent(&agent, "SelectedQueryAgent", &id).await?;
    state(&agent, &selected(&format!("{base}/state"), Some(chosen)), 0).await?;
    set(&agent, &selected(&format!("{base}/set/42"), Some(chosen))).await?;
    state(
        &agent,
        &selected(&format!("{base}/state"), Some(chosen)),
        42,
    )
    .await?;
    state(&agent, &format!("{base}/state"), 11).await?;
    state(&agent, &selected(&format!("{base}/state"), Some(fork)), 37).await?;
    // The selector names the agent; the constructor segments of the path do not.
    state(
        &agent,
        &selected(&format!("/selected-query/other-{id}/state"), Some(chosen)),
        42,
    )
    .await?;

    let unknown = unknown_agent(&agent, "SelectedQueryAgent").await?;
    for selector in [unknown, Uuid::new_v4()] {
        for (method, suffix) in [(Method::GET, "state"), (Method::POST, "set/5")] {
            assert_eq!(
                status(
                    &agent,
                    method,
                    &selected(&format!("{base}/{suffix}"), Some(selector))
                )
                .await?,
                StatusCode::NOT_FOUND,
                "selector {selector} on {suffix}"
            );
        }
    }
    assert_not_created(&agent, unknown).await?;

    for (prefix, agent_type) in [
        ("selected-required-query", "SelectedRequiredQueryAgent"),
        ("selected-path", "SelectedPathAgent"),
        ("selected-required-path", "SelectedRequiredPathAgent"),
    ] {
        let path_binding = prefix != "selected-required-query";
        let url = |suffix: &str, selector: Uuid| {
            if path_binding {
                format!("/{prefix}/{selector}/{id}/{suffix}")
            } else {
                selected(&format!("/{prefix}/{id}/{suffix}"), Some(selector))
            }
        };
        let first = existing_agent(&agent, agent_type, &id).await?;
        let second = existing_agent(&agent, agent_type, &id).await?;
        set(&agent, &url("set/21", first)).await?;
        set(&agent, &url("set/34", second)).await?;
        state(&agent, &url("state", first), 21).await?;
        state(&agent, &url("state", second), 34).await?;

        let unknown = unknown_agent(&agent, agent_type).await?;
        for selector in [unknown, chosen] {
            for (method, suffix) in [(Method::GET, "state"), (Method::POST, "set/8")] {
                assert_eq!(
                    status(&agent, method, &url(suffix, selector)).await?,
                    StatusCode::NOT_FOUND,
                    "{prefix}: selector {selector} on {suffix}"
                );
            }
        }
        assert_not_created(&agent, unknown).await?;

        let omitted = format!("/{prefix}/{id}/state");
        if prefix == "selected-path" {
            state(&agent, &omitted, 0).await?;
            set(&agent, &format!("/{prefix}/{id}/set/5")).await?;
            state(&agent, &omitted, 5).await?;
            state(&agent, &url("state", first), 21).await?;
        } else {
            assert_eq!(
                status(&agent, Method::GET, &omitted).await?,
                if path_binding {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
        }
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn unbound_http_selection_policies_are_preserved(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let agent = context(deps).await?;
    for (path, same) in [
        (
            format!("/durable-stream-agents/{}/instance", Uuid::new_v4()),
            true,
        ),
        ("/phantom-stream-agents/instance".into(), false),
        ("/ephemeral-stream-agents/instance".into(), false),
    ] {
        let first = agent
            .client
            .get(agent.base_url.join(&path)?)
            .send()
            .await?
            .error_for_status()?
            .json::<String>()
            .await?;
        let unknown_selector = agent
            .client
            .get(
                agent
                    .base_url
                    .join(&format!("{path}?instance={}", Uuid::new_v4()))?,
            )
            .send()
            .await?;
        assert_eq!(unknown_selector.status(), StatusCode::BAD_REQUEST);
        let second = agent
            .client
            .get(agent.base_url.join(&path)?)
            .send()
            .await?
            .error_for_status()?
            .json::<String>()
            .await?;
        assert_eq!(first == second, same, "unbound identity policy for {path}");
    }

    // The gateway calls agents without a caller scope, so an idempotency key never joins two
    // requests: each request of a phantomAgent mount creates another durable agent without a
    // key, and each ephemeral request runs on an instance of its own.
    let phantom_type_id = agent_type_id(
        &agent.user,
        &component(&agent).await?,
        &AgentTypeName("PhantomStreamAgent".into()),
    )
    .await?;
    for path in [
        "/phantom-stream-agents/instance",
        "/ephemeral-stream-agents/instance",
    ] {
        let key = Uuid::new_v4().to_string();
        let mut results = Vec::new();
        for _ in 0..2 {
            results.push(
                agent
                    .client
                    .get(agent.base_url.join(path)?)
                    .header("idempotency-key", &key)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<String>()
                    .await?
                    .parse::<Uuid>()?,
            );
        }
        assert_ne!(results[0], results[1], "{path}");
        if path.starts_with("/phantom") {
            for created in results {
                let created = AgentId(created);
                assert_eq!(created.type_id(), Some(phantom_type_id));
                let metadata = agent
                    .user
                    .get_worker_metadata(&OwnedAgentId::new(agent.env_id, &created))
                    .await?;
                assert_eq!(
                    metadata.agent_type_name,
                    Some(AgentTypeName("PhantomStreamAgent".into()))
                );
                assert_eq!(metadata.canonical_key, None);
            }
        }
    }
    Ok(())
}

async fn component(agent: &HttpTestContext) -> anyhow::Result<ComponentDto> {
    agent
        .user
        .get_latest_component_revision(&agent.component_id)
        .await
}

/// Creates an agent of `agent_type` without a key and returns its ID.
async fn existing_agent(
    agent: &HttpTestContext,
    agent_type: &str,
    id: &str,
) -> anyhow::Result<Uuid> {
    let created = create_agent(
        &agent.user,
        &component(agent).await?,
        &agent_key!(agent_type, id.to_string()),
    )
    .await?;
    Ok(created.agent_id.0)
}

/// An ID with the type tag of `agent_type` that no agent has.
async fn unknown_agent(agent: &HttpTestContext, agent_type: &str) -> anyhow::Result<Uuid> {
    let type_id = agent_type_id(
        &agent.user,
        &component(agent).await?,
        &AgentTypeName(agent_type.into()),
    )
    .await?;
    Ok(AgentId::random(type_id).0)
}

async fn assert_not_created(agent: &HttpTestContext, agent_id: Uuid) -> anyhow::Result<()> {
    assert!(
        agent
            .user
            .get_worker_metadata_opt(&OwnedAgentId::new(agent.env_id, &AgentId(agent_id)))
            .await?
            .is_none(),
        "a selector must not create agent {agent_id}"
    );
    Ok(())
}

async fn status(agent: &HttpTestContext, method: Method, path: &str) -> anyhow::Result<StatusCode> {
    Ok(agent
        .client
        .request(method, agent.base_url.join(path)?)
        .send()
        .await?
        .status())
}

fn header(response: &Response, name: &str) -> String {
    response.headers()[name].to_str().unwrap().into()
}

async fn read(agent: &HttpTestContext, path: &str, expected: Value) -> anyhow::Result<Response> {
    let response = agent.client.get(agent.base_url.join(path)?).send().await?;
    assert_eq!(response.status(), StatusCode::OK);
    let offset = header(&response, "stream-next-offset");
    assert_eq!(response.json::<Value>().await?, expected);
    let head = agent.client.head(agent.base_url.join(path)?).send().await?;
    assert_eq!(header(&head, "stream-next-offset"), offset);
    Ok(head)
}

#[test]
#[timeout("240s")]
async fn selected_stream_roots_forks_receipts_and_links(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let agent = context(deps).await?;
    let id = Uuid::new_v4().to_string();
    let base = format!("/selected-query/{id}/echo");
    let source = format!("{base}/invocations/s1/streams/input");
    let fork_path = format!("{base}/forks/branch/invocations/s1/streams/input");
    let p = existing_agent(&agent, "SelectedQueryAgent", &id).await?;
    let q = existing_agent(&agent, "SelectedQueryAgent", &id).await?;
    let mut p_cut = String::new();
    for (root, label) in [(None, "original"), (Some(p), "P"), (Some(q), "Q")] {
        let source_url = selected(&source, root);
        let created = agent
            .client
            .put(agent.base_url.join(&source_url)?)
            .send()
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        agent
            .client
            .post(agent.base_url.join(&source_url)?)
            .json(&json!([label]))
            .send()
            .await?
            .error_for_status()?;
        let prefix = read(&agent, &source_url, json!([label])).await?;
        let cut = header(&prefix, "stream-next-offset");
        if root == Some(p) {
            p_cut = cut.clone();
        }
        let fork_url = selected(&fork_path, root);
        for expected_status in [StatusCode::CREATED, StatusCode::OK] {
            let response = agent
                .client
                .put(agent.base_url.join(&fork_url)?)
                .header("stream-forked-from", &source)
                .header("stream-fork-offset", &cut)
                .json(&json!([format!("fork-{label}")]))
                .send()
                .await?;
            assert_eq!(response.status(), expected_status);
            assert_eq!(header(&response, "location"), fork_url);
        }
        let response = agent
            .client
            .put(agent.base_url.join(&fork_url)?)
            .header("stream-forked-from", &source)
            .header("stream-fork-offset", &cut)
            .json(&json!(["different"]))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        read(&agent, &fork_url, json!([label, format!("fork-{label}")])).await?;
    }
    let p_fork = selected(&fork_path, Some(p));
    let p_source = selected(&source, Some(p));
    let uppercase = agent
        .client
        .put(agent.base_url.join(&format!(
            "{fork_path}?instance={}",
            p.to_string().to_uppercase()
        ))?)
        .header("stream-forked-from", &source)
        .header("stream-fork-offset", &p_cut)
        .json(&json!(["fork-P"]))
        .send()
        .await?;
    assert_eq!(uppercase.status(), StatusCode::OK);
    assert_eq!(header(&uppercase, "location"), p_fork);
    agent
        .client
        .post(agent.base_url.join(&p_source)?)
        .json(&json!(["P-source-only"]))
        .send()
        .await?
        .error_for_status()?;
    agent
        .client
        .post(agent.base_url.join(&p_fork)?)
        .json(&json!(["P-fork-only"]))
        .send()
        .await?
        .error_for_status()?;
    read(&agent, &p_source, json!(["P", "P-source-only"])).await?;
    let branch = read(&agent, &p_fork, json!(["P", "fork-P", "P-fork-only"])).await?;
    let nested_path = format!("{base}/forks/nested/invocations/s1/streams/input");
    let nested_url = selected(&nested_path, Some(p));
    let nested = agent
        .client
        .put(agent.base_url.join(&nested_url)?)
        .header("stream-forked-from", &fork_path)
        .header("stream-fork-offset", header(&branch, "stream-next-offset"))
        .json(&json!(["nested-only"]))
        .send()
        .await?;
    assert_eq!(nested.status(), StatusCode::CREATED);
    assert_eq!(header(&nested, "location"), nested_url);
    read(
        &agent,
        &nested_url,
        json!(["P", "fork-P", "P-fork-only", "nested-only"]),
    )
    .await?;

    let registry = deps.registry_service();
    let mut client = RegistryServiceClient::connect(format!(
        "http://{}:{}",
        registry.grpc_host(),
        registry.grpc_port()
    ))
    .await?;
    let response = client
        .get_active_routes_for_domain(GetActiveRoutesForDomainRequest {
            domain: agent.host_header.to_str()?.to_owned(),
        })
        .await?
        .into_inner();
    let Some(get_active_routes_for_domain_response::Result::Success(response)) = response.result
    else {
        anyhow::bail!("failed to resolve deployed routes: {response:?}");
    };
    let routes: CompiledRoutes = response
        .compiled_routes
        .ok_or_else(|| anyhow::anyhow!("missing deployed routes"))?
        .try_into()
        .map_err(anyhow::Error::msg)?;
    let family = routes
        .routes
        .iter()
        .find_map(|route| match &route.behavior {
            RouteBehaviour::CallAgent(call)
                if call.agent_type.0 == "SelectedQueryAgent" && call.method_name == "echo" =>
            {
                call.durable_streams.as_ref().map(|policy| policy.family_id)
            }
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("missing selected-query echo route family"))?;
    let scope = golem_common::model::durable_stream::durable_stream_route_scope(
        &routes.environment_id.to_string(),
        &routes.deployment_revision.to_string(),
        &family.to_string(),
    );
    let root = AgentId(p);
    let known_fork = AgentId::durable_stream_fork(routes.environment_id, root, &scope, "branch").0;
    read(
        &agent,
        &selected(&source, Some(known_fork)),
        json!(["P", "fork-P", "P-fork-only"]),
    )
    .await?;
    let direct_child_url = selected(
        &format!("{base}/forks/direct-child/invocations/s1/streams/input"),
        Some(known_fork),
    );
    let direct_child = agent
        .client
        .put(agent.base_url.join(&direct_child_url)?)
        .header("stream-forked-from", &source)
        .json(&json!(["direct-child"]))
        .send()
        .await?;
    assert_eq!(direct_child.status(), StatusCode::CREATED);
    read(
        &agent,
        &direct_child_url,
        json!(["P", "fork-P", "P-fork-only", "direct-child"]),
    )
    .await?;
    let deleted = agent
        .client
        .delete(agent.base_url.join(&p_source)?)
        .send()
        .await?;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(&p_source)?)
            .send()
            .await?
            .status(),
        StatusCode::GONE
    );
    read(&agent, &source, json!(["original"])).await?;
    read(&agent, &selected(&source, Some(q)), json!(["Q"])).await?;
    read(&agent, &p_fork, json!(["P", "fork-P", "P-fork-only"])).await?;

    let selected_worker = OwnedAgentId::new(routes.environment_id, &root);
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if agent
                .user
                .get_worker_metadata(&selected_worker)
                .await?
                .status
                == AgentStatus::Failed
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await??;
    let failed = agent
        .client
        .put(agent.base_url.join(&selected(&base, Some(p)))?)
        .send()
        .await?;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(failed.text().await?.contains("stream slot deleted"));

    let unknown = unknown_agent(&agent, "SelectedQueryAgent").await?;
    assert_eq!(
        status(&agent, Method::PUT, &selected(&base, Some(unknown))).await?,
        StatusCode::NOT_FOUND
    );
    assert_not_created(&agent, unknown).await?;

    let generated_root = existing_agent(&agent, "SelectedQueryAgent", &id).await?;
    let generated = agent
        .client
        .put(
            agent
                .base_url
                .join(&selected(&base, Some(generated_root)))?,
        )
        .send()
        .await?;
    assert_eq!(generated.status(), StatusCode::CREATED);
    let location = header(&generated, "location");
    assert!(location.ends_with(&format!("?instance={generated_root}")));
    let manifest = agent
        .client
        .get(agent.base_url.join(&location)?)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert!(
        manifest["streams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|slot| slot["name"] == "input")
    );
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(location.split('?').next().unwrap())?)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[test]
#[timeout("180s")]
async fn selected_phantom_signed_callback_stays_on_selected_identity(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    use axum::{Router, body::Bytes, routing::post};
    let agent = context(deps).await?;
    let id = Uuid::new_v4();
    let p = existing_agent(&agent, "SelectedQueryAgent", &id.to_string()).await?;
    let base = format!("/selected-query/{id}");
    set(&agent, &format!("{base}/set/11")).await?;
    set(&agent, &selected(&format!("{base}/set/37"), Some(p))).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let callback_server = format!("http://127.0.0.1:{}/", listener.local_addr()?.port());
    let gateway = agent.base_url.clone();
    let host = agent.host_header.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                post(move |body: Bytes| {
                    let gateway = gateway.clone();
                    let host = host.clone();
                    async move {
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        let mut url =
                            reqwest::Url::parse(body["webhookUrl"].as_str().unwrap()).unwrap();
                        url.set_host(gateway.host_str()).unwrap();
                        url.set_port(gateway.port()).unwrap();
                        let response = reqwest::Client::new()
                            .post(url)
                            .header("Host", host)
                            .json(&93)
                            .send()
                            .await
                            .unwrap();
                        assert!(response.status().is_success());
                        "ok"
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let result = agent
        .client
        .post(
            agent
                .base_url
                .join(&selected(&format!("{base}/webhook"), Some(p)))?,
        )
        .json(&json!({"callback_server": callback_server}))
        .send()
        .await;
    server.abort();
    let _ = server.await;
    let response = result?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await?, json!(93));
    state(&agent, &selected(&format!("{base}/state"), Some(p)), 93).await?;
    state(&agent, &format!("{base}/state"), 11).await?;
    Ok(())
}
