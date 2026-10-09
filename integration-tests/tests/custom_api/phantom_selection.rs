use super::http_test_context::{HttpTestContext, make_test_context};
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::phantom_agent_id;
use golem_test_framework::config::EnvBasedTestDependencies;
use reqwest::{Response, StatusCode};
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
#[timeout("180s")]
async fn selected_phantom_state_and_constructor_isolation(
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
    let unused = Uuid::new_v4();
    state(&agent, &selected(&format!("{base}/state"), Some(unused)), 0).await?;
    state(
        &agent,
        &selected(&format!("/selected-query/other-{id}/state"), Some(fork)),
        0,
    )
    .await?;
    set(&agent, &selected(&format!("{base}/set/42"), Some(fork))).await?;
    state(&agent, &selected(&format!("{base}/state"), Some(fork)), 42).await?;
    state(&agent, &format!("{base}/state"), 11).await?;

    for prefix in [
        "selected-required-query",
        "selected-path",
        "selected-required-path",
    ] {
        let path_binding = prefix != "selected-required-query";
        let base = if path_binding {
            format!("/{prefix}/{fork}/{id}")
        } else {
            format!("/{prefix}/{id}")
        };
        let url = |suffix: &str, selector: Uuid| {
            if path_binding {
                format!("/{prefix}/{selector}/{id}/{suffix}")
            } else {
                selected(&format!("{base}/{suffix}"), Some(selector))
            }
        };
        set(&agent, &url("set/21", fork)).await?;
        set(&agent, &url("set/34", unused)).await?;
        state(&agent, &url("state", fork), 21).await?;
        state(&agent, &url("state", unused), 34).await?;
        let omitted = format!("/{prefix}/{id}/state");
        if prefix == "selected-path" {
            state(&agent, &omitted, 0).await?;
            set(&agent, &format!("/{prefix}/{id}/set/5")).await?;
            state(&agent, &omitted, 5).await?;
            state(&agent, &url("state", fork), 21).await?;
        } else {
            let response = agent
                .client
                .get(agent.base_url.join(&omitted)?)
                .send()
                .await?;
            assert_eq!(
                response.status(),
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
        let second = agent
            .client
            .get(
                agent
                    .base_url
                    .join(&format!("{path}?instance={}", Uuid::new_v4()))?,
            )
            .send()
            .await?
            .error_for_status()?
            .json::<String>()
            .await?;
        assert_eq!(first == second, same, "unbound identity policy for {path}");
    }
    let key = Uuid::new_v4().to_string();
    let path = "/ephemeral-stream-agents/instance";
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
                .await?,
        );
    }
    assert_ne!(results[0], results[1]);
    Ok(())
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
    let p = Uuid::new_v4();
    let q = Uuid::new_v4();
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

    let root = phantom_agent_id!("SelectedQueryAgent", p, id.clone()).to_string();
    let hash = blake3::hash(format!("{root}\0branch").as_bytes());
    let mut bytes: [u8; 16] = hash.as_bytes()[..16].try_into()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let known_fork = Uuid::from_bytes(bytes);
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

    let generated = agent
        .client
        .put(agent.base_url.join(&selected(&base, Some(p)))?)
        .send()
        .await?;
    assert_eq!(generated.status(), StatusCode::CREATED);
    let location = header(&generated, "location");
    assert!(location.ends_with(&format!("?instance={p}")));
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
    let p = Uuid::new_v4();
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
