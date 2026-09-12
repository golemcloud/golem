// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::custom_api::http_test_context::{HttpTestContext, make_test_context};
use base64::Engine;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::{AgentId, OplogIndex};
use golem_test_framework::config::EnvBasedTestDependencies;
use golem_test_framework::dsl::TestDsl;
use pretty_assertions::assert_eq;
use reqwest::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::time::{Duration, Instant};
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep, timeout};
use uuid::Uuid;

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
        [
            "DurableStreamAgent",
            "EphemeralStreamAgent",
            "PhantomStreamAgent",
        ]
        .into_iter()
        .map(|name| {
            (
                AgentTypeName(name.to_string()),
                HttpApiDeploymentAgentOptions::default(),
            )
        })
        .collect(),
        "golem_it_agent_sdk_rust_release",
        "golem-it:agent-sdk-rust",
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

define_matrix_dimension!(db: HttpTestContext -> "postgres", "sqlite");

fn stream_path(method: &str, session: &str, slot: &str, delay_ms: u64) -> String {
    format!(
        "/durable-stream-agents/ds3-{session}/{method}/invocations/{session}/streams/{slot}?delay_ms={delay_ms}"
    )
}

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("missing {name} header"))
        .to_str()
        .unwrap()
        .to_string()
}

async fn create(agent: &HttpTestContext, path: &str) -> anyhow::Result<reqwest::Response> {
    Ok(agent.client.put(agent.base_url.join(path)?).send().await?)
}

async fn wait_for_closed(agent: &HttpTestContext, path: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let response = agent.client.head(agent.base_url.join(path)?).send().await?;
            assert_eq!(response.status(), StatusCode::OK);
            if header(&response, "stream-closed") == "true" {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

async fn observations(agent: &HttpTestContext, id: &str) -> anyhow::Result<Vec<u64>> {
    let response = agent
        .client
        .put(
            agent
                .base_url
                .join(&format!("/durable-stream-agents/{id}/observations"))?,
        )
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(response.json().await?)
}

async fn wait_for_observations(
    agent: &HttpTestContext,
    id: &str,
    expected: &[u64],
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let actual = observations(agent, id).await?;
            if actual == expected {
                return Ok::<_, anyhow::Error>(());
            }
            eprintln!("waiting for observations {expected:?}, current value is {actual:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

fn echo_stream_path(session: &str, slot: &str) -> String {
    format!("/durable-stream-agents/ds3-{session}/echo/invocations/{session}/streams/{slot}")
}

async fn append_json(
    agent: &HttpTestContext,
    path: &str,
    value: Value,
    close: bool,
) -> anyhow::Result<reqwest::Response> {
    let mut request = agent
        .client
        .post(agent.base_url.join(path)?)
        .header(CONTENT_TYPE, "application/json")
        .json(&value);
    if close {
        request = request.header("stream-closed", "true");
    }
    Ok(request.send().await?)
}

#[test]
#[timeout("120s")]
async fn post_input_accepts_json_batches_and_is_idempotently_closed(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let input = echo_stream_path(&session, "input");
    let output = echo_stream_path(&session, "output");

    let batch = append_json(agent, &input, serde_json::json!(["a", "b"]), false).await?;
    assert_eq!(batch.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&batch, "stream-closed"), "false");
    let first_offset = header(&batch, "stream-next-offset");
    let single = append_json(agent, &input, serde_json::json!("c"), false).await?;
    assert_eq!(single.status(), StatusCode::NO_CONTENT);
    assert!(header(&single, "stream-next-offset") > first_offset);

    for value in ["true", "TRUE"] {
        let closed = agent
            .client
            .post(agent.base_url.join(&input)?)
            .header("stream-closed", value)
            .send()
            .await?;
        assert_eq!(closed.status(), StatusCode::NO_CONTENT);
        assert_eq!(header(&closed, "stream-closed"), "true");
    }
    wait_for_closed(agent, &output).await?;
    let echoed = agent
        .client
        .get(agent.base_url.join(&output)?)
        .send()
        .await?;
    assert_eq!(echoed.status(), StatusCode::OK);
    assert_eq!(
        echoed.json::<Value>().await?,
        serde_json::json!(["a", "b", "c"])
    );

    let read_only = agent
        .client
        .post(agent.base_url.join(&output)?)
        .json(&serde_json::json!("no"))
        .send()
        .await?;
    assert_eq!(read_only.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&read_only, ALLOW.as_str()), "PUT, HEAD, GET, DELETE");

    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&input)?)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        append_json(agent, &input, serde_json::json!("gone"), false)
            .await?
            .status(),
        StatusCode::GONE
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn post_input_frames_binary_batches_and_json_records(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let binary_session = Uuid::new_v4().to_string();
    let binary_base = format!(
        "/durable-stream-agents/ds3-{binary_session}/echo-bytes/invocations/{binary_session}/streams"
    );
    let binary_input = format!("{binary_base}/input");
    let binary_output = format!("{binary_base}/$result");
    for (bytes, close) in [(&b"\x00\x01\xfe"[..], false), (&b"hello\xff"[..], true)] {
        let response = agent
            .client
            .post(agent.base_url.join(&binary_input)?)
            .header(CONTENT_TYPE, "application/octet-stream")
            .header("stream-closed", close.to_string())
            .body(bytes)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    wait_for_closed(agent, &binary_output).await?;
    let response = agent
        .client
        .get(agent.base_url.join(&binary_output)?)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await?.as_ref(), b"\x00\x01\xfehello\xff");

    let record_session = Uuid::new_v4().to_string();
    let record_base = format!(
        "/durable-stream-agents/ds3-{record_session}/echo-records/invocations/{record_session}/streams"
    );
    let record_input = format!("{record_base}/input");
    let record_output = format!("{record_base}/$result");
    assert_eq!(
        append_json(
            agent,
            &record_input,
            serde_json::json!({"name": "one", "number": 1}),
            false,
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        append_json(
            agent,
            &record_input,
            serde_json::json!([
                {"name": "two", "number": 2},
                {"name": "three", "number": 3}
            ]),
            false,
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let invalid = append_json(
        agent,
        &record_input,
        serde_json::json!([
            {"name": "must-not-append", "number": 4},
            {"name": "invalid", "number": "not-a-number"}
        ]),
        false,
    )
    .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let problem = invalid.text().await?;
    assert!(problem.contains("$[1]"), "missing item path: {problem}");
    assert!(
        problem.contains("number"),
        "missing field path/detail: {problem}"
    );
    let close = agent
        .client
        .post(agent.base_url.join(&record_input)?)
        .header("stream-closed", "true")
        .send()
        .await?;
    assert_eq!(close.status(), StatusCode::NO_CONTENT);
    wait_for_closed(agent, &record_output).await?;
    let records = agent
        .client
        .get(agent.base_url.join(&record_output)?)
        .send()
        .await?
        .json::<Value>()
        .await?;
    assert_eq!(
        records,
        serde_json::json!([
            {"name": "one", "number": 1},
            {"name": "two", "number": 2},
            {"name": "three", "number": 3}
        ])
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn post_input_requires_explicit_body_arguments_before_lazy_start(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let base = format!("/durable-stream-agents/ds3-{session}/prefixed/invocations/{session}");
    let input = format!("{base}/streams/input");
    let output = format!("{base}/streams/$result");
    let rejected = append_json(agent, &input, serde_json::json!("value"), false).await?;
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
    let problem = rejected.text().await?;
    assert!(
        problem.contains("PUT"),
        "missing PUT instruction: {problem}"
    );
    assert!(problem.contains(&base), "missing session URL: {problem}");

    let created = agent
        .client
        .put(agent.base_url.join(&base)?)
        .json(&serde_json::json!({"prefix": "prefix:"}))
        .send()
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(
        append_json(agent, &input, serde_json::json!("value"), true)
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    wait_for_closed(agent, &output).await?;
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(&output)?)
            .send()
            .await?
            .json::<Value>()
            .await?,
        serde_json::json!(["prefix:value"])
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn post_input_validates_body_content_type_and_control_headers(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let input = echo_stream_path(&session, "input");
    let url = agent.base_url.join(&input)?;

    for request in [
        agent
            .client
            .post(url.clone())
            .body("\"missing content type\""),
        agent
            .client
            .post(url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body("not json"),
        agent
            .client
            .post(url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body("[]"),
        agent.client.post(url.clone()),
    ] {
        assert_eq!(request.send().await?.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        agent
            .client
            .post(url.clone())
            .header(CONTENT_TYPE, "application/octet-stream")
            .body("x")
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        agent
            .client
            .post(url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(vec![b'x'; 1024 * 1024 + 1])
            .send()
            .await?
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        agent
            .client
            .post(url.clone())
            .json(&vec!["tiny"; 4097])
            .send()
            .await?
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let ignored_close = append_json(agent, &input, serde_json::json!("open"), false).await?;
    assert_eq!(ignored_close.status(), StatusCode::NO_CONTENT);
    let ignored_close = agent
        .client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header("stream-closed", "yes")
        .body("\"still-open\"")
        .send()
        .await?;
    assert_eq!(ignored_close.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&ignored_close, "stream-closed"), "false");
    Ok(())
}

#[test]
#[timeout("120s")]
async fn post_input_enforces_external_producer_sequence_and_epoch(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let input = echo_stream_path(&session, "input");
    let url = agent.base_url.join(&input)?;
    let post = |epoch: &str, sequence: &str, value: &str| {
        agent
            .client
            .post(url.clone())
            .header(CONTENT_TYPE, "application/json")
            .header("producer-id", "producer")
            .header("producer-epoch", epoch)
            .header("producer-seq", sequence)
            .body(format!("\"{value}\""))
    };

    for (epoch, sequence) in [("-1", "0"), ("0", "1.0"), ("9007199254740992", "0")] {
        assert_eq!(
            post(epoch, sequence, "bad").send().await?.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        agent
            .client
            .post(url.clone())
            .header("producer-id", "producer")
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );

    let first = post("7", "0", "zero").send().await?;
    assert_eq!(first.status(), StatusCode::OK);
    let first_offset = header(&first, "stream-next-offset");
    let second = post("7", "1", "one").send().await?;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(header(&second, "producer-seq"), "1");
    let duplicate = post("7", "0", "ignored").send().await?;
    assert_eq!(duplicate.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&duplicate, "stream-next-offset"), first_offset);
    assert_eq!(header(&duplicate, "producer-seq"), "1");

    let gap = post("7", "3", "gap").send().await?;
    assert_eq!(gap.status(), StatusCode::CONFLICT);
    assert_eq!(header(&gap, "producer-expected-seq"), "2");
    assert_eq!(header(&gap, "producer-received-seq"), "3");
    let fenced = post("6", "2", "fenced").send().await?;
    assert_eq!(fenced.status(), StatusCode::FORBIDDEN);
    assert_eq!(header(&fenced, "producer-epoch"), "7");

    let new_epoch = post("8", "0", "new-epoch").send().await?;
    assert_eq!(new_epoch.status(), StatusCode::OK);
    assert_eq!(header(&new_epoch, "producer-seq"), "0");

    let closed = post("8", "1", "closed")
        .header("stream-closed", "true")
        .send()
        .await?;
    assert_eq!(closed.status(), StatusCode::OK);
    let final_offset = header(&closed, "stream-next-offset");
    let retried = post("8", "1", "different")
        .header("stream-closed", "true")
        .send()
        .await?;
    assert_eq!(retried.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&retried, "producer-seq"), "1");
    assert_eq!(header(&retried, "stream-next-offset"), final_offset);

    let after_close = post("8", "2", "after-close").send().await?;
    assert_eq!(after_close.status(), StatusCode::CONFLICT);
    assert_eq!(header(&after_close, "stream-closed"), "true");
    assert_eq!(header(&after_close, "stream-next-offset"), final_offset);

    let output = echo_stream_path(&session, "output");
    wait_for_closed(agent, &output).await?;
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(&output)?)
            .send()
            .await?
            .json::<Value>()
            .await?,
        serde_json::json!(["zero", "one", "new-epoch", "closed"])
    );
    Ok(())
}

#[test]
#[timeout("180s")]
async fn post_input_offsets_and_retry_survive_reconstruction(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let id = format!("ds3-{session}");
    let component_id = agent
        .client
        .put(
            agent
                .base_url
                .join(&format!("/durable-stream-agents/{id}/component-id"))?,
        )
        .send()
        .await?
        .json::<String>()
        .await?;
    let worker = AgentId {
        component_id: ComponentId(Uuid::parse_str(&component_id)?),
        agent_id: format!("DurableStreamAgent(\"{id}\")"),
    };
    let input = echo_stream_path(&session, "input");
    let output = echo_stream_path(&session, "output");
    let mut offsets = Vec::new();
    for sequence in 0..100 {
        let response = agent
            .client
            .post(agent.base_url.join(&input)?)
            .header(CONTENT_TYPE, "application/json")
            .header("producer-id", "sequential")
            .header("producer-epoch", "0")
            .header("producer-seq", sequence.to_string())
            .json(&format!("value-{sequence:03}"))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "sequence {sequence}");
        offsets.push(header(&response, "stream-next-offset"));
    }
    assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));

    // The per-stream append budget is exactly 100 requests per one-second window.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    agent.user.simulated_crash(&worker).await?;

    let retry = agent
        .client
        .post(agent.base_url.join(&input)?)
        .header(CONTENT_TYPE, "application/json")
        .header("producer-id", "sequential")
        .header("producer-epoch", "0")
        .header("producer-seq", "99")
        .header("stream-closed", "true")
        .json("value-099")
        .send()
        .await?;
    assert_eq!(retry.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&retry, "stream-next-offset"), offsets[99]);
    assert_eq!(header(&retry, "producer-seq"), "99");

    let close = agent
        .client
        .post(agent.base_url.join(&input)?)
        .header("stream-closed", "true")
        .send()
        .await?;
    assert_eq!(close.status(), StatusCode::NO_CONTENT);
    wait_for_closed(agent, &output).await?;
    let values = agent
        .client
        .get(agent.base_url.join(&output)?)
        .send()
        .await?
        .json::<Vec<String>>()
        .await?;
    assert_eq!(
        values,
        (0..100)
            .map(|i| format!("value-{i:03}"))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn lifecycle_json_binary_head_and_methods(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let json = stream_path("json/3", &session, "$result", 0);

    let absent = agent
        .client
        .head(agent.base_url.join(&json)?)
        .send()
        .await?;
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);

    let rejected = agent
        .client
        .put(agent.base_url.join(&json)?)
        .body("ignored input must not create a session")
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let mismatch = agent
        .client
        .put(agent.base_url.join(&json)?)
        .header(CONTENT_TYPE, "application/octet-stream")
        .send()
        .await?;
    assert_eq!(mismatch.status(), StatusCode::CONFLICT);
    assert_eq!(
        agent
            .client
            .head(agent.base_url.join(&json)?)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    let first = create(agent, &json).await?;
    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(header(&first, CONTENT_TYPE.as_str()), "application/json");
    assert!(first.headers().contains_key("stream-next-offset"));
    let repeated = create(agent, &json).await?;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(
        agent
            .client
            .put(agent.base_url.join(&json)?)
            .body("must not be discarded on attach either")
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );

    let head = agent
        .client
        .head(agent.base_url.join(&json)?)
        .send()
        .await?;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(header(&head, CONTENT_TYPE.as_str()), "application/json");
    assert_eq!(header(&head, CACHE_CONTROL.as_str()), "no-store");
    assert!(head.bytes().await?.is_empty());

    wait_for_closed(agent, &json).await?;
    let get = agent.client.get(agent.base_url.join(&json)?).send().await?;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        get.json::<Value>().await?,
        serde_json::json!(["message-0000", "message-0001", "message-0002"])
    );

    let response = agent
        .client
        .post(agent.base_url.join(&json)?)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&response, ALLOW.as_str()), "PUT, HEAD, GET, DELETE");

    let session_path = format!("/durable-stream-agents/ds3-{session}/json/3/invocations/{session}");
    for _ in 0..2 {
        assert_eq!(
            agent
                .client
                .delete(agent.base_url.join(&session_path)?)
                .send()
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    let ended = agent.client.get(agent.base_url.join(&json)?).send().await?;
    assert_eq!(ended.status(), StatusCode::OK);
    assert_eq!(header(&ended, "stream-closed"), "true");
    assert!(!ended.headers().contains_key("stream-cancelled"));
    assert_eq!(
        ended.json::<Value>().await?,
        serde_json::json!(["message-0000", "message-0001", "message-0002"])
    );
    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&json)?)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    for method in [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::DELETE,
        Method::PUT,
    ] {
        let expected = if method == Method::PUT {
            StatusCode::CONFLICT
        } else {
            StatusCode::GONE
        };
        assert_eq!(
            agent
                .client
                .request(method, agent.base_url.join(&json)?)
                .send()
                .await?
                .status(),
            expected
        );
    }

    let bytes_session = Uuid::new_v4().to_string();
    let bytes_path = stream_path("bytes/260", &bytes_session, "$result", 0);
    assert_eq!(
        create(agent, &bytes_path).await?.status(),
        StatusCode::CREATED
    );
    let head = agent
        .client
        .head(agent.base_url.join(&bytes_path)?)
        .send()
        .await?;
    assert_eq!(
        header(&head, CONTENT_TYPE.as_str()),
        "application/octet-stream"
    );
    wait_for_closed(agent, &bytes_path).await?;
    let mut offset = "-1".to_owned();
    let mut bytes = Vec::new();
    loop {
        let response = agent
            .client
            .get(
                agent
                    .base_url
                    .join(&format!("{bytes_path}&offset={offset}"))?,
            )
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let done = response
            .headers()
            .get("stream-up-to-date")
            .is_some_and(|value| value == "true");
        let next = header(&response, "stream-next-offset");
        bytes.extend_from_slice(&response.bytes().await?);
        if done {
            break;
        }
        assert_ne!(
            next, offset,
            "catch-up must advance while more bytes remain"
        );
        offset = next;
    }
    assert_eq!(bytes, (0..260).map(|i| (i % 251) as u8).collect::<Vec<_>>());

    let echo_session = Uuid::new_v4().to_string();
    let echo_input = stream_path("echo", &echo_session, "input", 0);
    assert_eq!(
        create(agent, &echo_input).await?.status(),
        StatusCode::CREATED
    );
    for slot in ["input", "output"] {
        let path = stream_path("echo", &echo_session, slot, 0);
        let response = agent
            .client
            .head(agent.base_url.join(&path)?)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, CONTENT_TYPE.as_str()), "application/json");
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn generated_session_location_and_manifest(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let base = format!("/durable-stream-agents/generated-{}/json/2", Uuid::new_v4());
    let created = create(agent, &format!("{base}?delay_ms=0")).await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let location = header(&created, "location");
    let session = location
        .strip_prefix(&format!("{base}/invocations/"))
        .expect("Location must identify a session below the original method URL");
    assert!(!session.is_empty());
    assert!(!session.contains('/'));
    let stream = format!("{location}/streams/$result");
    wait_for_closed(agent, &stream).await?;

    let metadata = agent
        .client
        .get(agent.base_url.join(&location)?)
        .send()
        .await?;
    assert_eq!(metadata.status(), StatusCode::OK);
    assert_eq!(header(&metadata, "cache-control"), "no-store");
    let metadata = metadata.json::<Value>().await?;
    assert_eq!(metadata["session"], session);
    assert_eq!(metadata["closed"], true);
    let slots = metadata["streams"].as_array().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["name"], "$result");
    assert_eq!(slots[0]["contentType"], "application/json");
    assert_eq!(slots[0]["closed"], true);
    assert_eq!(slots[0]["cancelled"], false);

    let head = agent
        .client
        .head(agent.base_url.join(&location)?)
        .send()
        .await?;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(header(&head, "content-type"), "application/json");
    assert_eq!(header(&head, "stream-closed"), "true");
    assert!(head.bytes().await?.is_empty());
    assert_eq!(
        create(agent, &format!("{location}?delay_ms=0"))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        create(agent, &format!("{location}?delay_ms=1"))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let data = agent
        .client
        .get(agent.base_url.join(&stream)?)
        .send()
        .await?;
    assert_eq!(data.status(), StatusCode::OK);
    assert_eq!(
        data.json::<Value>().await?,
        serde_json::json!(["message-0000", "message-0001"])
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn scalar_argument_does_not_hide_named_output(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let base = format!("/durable-stream-agents/ds3-{session}/shadow/payload/invocations/{session}");
    let path = format!("{base}/streams/output");
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    wait_for_closed(agent, &path).await?;
    let data = agent.client.get(agent.base_url.join(&path)?).send().await?;
    assert_eq!(data.status(), StatusCode::OK);
    assert_eq!(data.json::<Value>().await?, serde_json::json!(["payload"]));
    let manifest = agent.client.get(agent.base_url.join(&base)?).send().await?;
    assert_eq!(manifest.status(), StatusCode::OK);
    let manifest = manifest.json::<Value>().await?;
    let slots = manifest["streams"].as_array().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["name"], "output");
    Ok(())
}

#[test]
#[timeout("120s")]
async fn phantom_and_ephemeral_sessions_have_stable_distinct_identities(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    for mode in ["phantom", "ephemeral"] {
        let session = Uuid::new_v4().to_string();
        let base = format!("/{mode}-stream-agents/identity/first");
        let path = format!("{base}/invocations/{session}/streams/$result");
        assert_eq!(
            create(agent, &path).await?.status(),
            StatusCode::CREATED,
            "{mode}"
        );
        wait_for_closed(agent, &path).await?;
        assert_eq!(create(agent, &path).await?.status(), StatusCode::OK);
        let mismatch =
            format!("/{mode}-stream-agents/identity/second/invocations/{session}/streams/$result");
        assert_eq!(
            create(agent, &mismatch).await?.status(),
            StatusCode::CONFLICT
        );
        let data = agent.client.get(agent.base_url.join(&path)?).send().await?;
        assert_eq!(data.status(), StatusCode::OK);
        let first = data.json::<Vec<String>>().await?;
        assert_eq!(first.len(), 2);
        assert_eq!(first[0], "first");

        let created = create(agent, &base).await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let generated = header(&created, "location");
        let generated_path = format!("{generated}/streams/$result");
        wait_for_closed(agent, &generated_path).await?;
        let data = agent
            .client
            .get(agent.base_url.join(&generated_path)?)
            .send()
            .await?;
        assert_eq!(data.status(), StatusCode::OK);
        let second = data.json::<Vec<String>>().await?;
        assert_eq!(second.len(), 2);
        assert_eq!(second[0], "first");
        assert_ne!(
            first[1], second[1],
            "different sessions must address distinct {mode} agents"
        );
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn session_put_binds_body_query_header_and_path(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let base =
        format!("/durable-stream-agents/ds3-{session}/bound/route-value/invocations/{session}");
    let session_url = agent.base_url.join(&format!("{base}?count=3"))?;
    let stream = format!("{base}/streams/$result?count=3");
    assert_eq!(
        create(agent, &stream).await?.status(),
        StatusCode::BAD_REQUEST
    );

    for request in [
        agent
            .client
            .put(session_url.clone())
            .json(&serde_json::json!({"message": "body-value"})),
        agent
            .client
            .put(session_url.clone())
            .header("x-label", "header-value")
            .json(&serde_json::json!({"message": 42})),
        agent
            .client
            .put(session_url.clone())
            .header("x-label", "header-value")
            .json(&serde_json::json!("body-value")),
    ] {
        assert_eq!(request.send().await?.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            agent
                .client
                .head(session_url.clone())
                .send()
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    for expected in [StatusCode::CREATED, StatusCode::OK] {
        let response = agent
            .client
            .put(session_url.clone())
            .header("x-label", "header-value")
            .json(&serde_json::json!({"message": "body-value"}))
            .send()
            .await?;
        assert_eq!(response.status(), expected);
    }
    for (label, message) in [
        ("different-header", "body-value"),
        ("header-value", "different-body"),
    ] {
        let response = agent
            .client
            .put(session_url.clone())
            .header("x-label", label)
            .json(&serde_json::json!({"message": message}))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    assert_eq!(create(agent, &stream).await?.status(), StatusCode::OK);
    wait_for_closed(agent, &stream).await?;
    let response = agent
        .client
        .get(agent.base_url.join(&stream)?)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await?,
        serde_json::json!([
            "route-value|header-value|0|body-value",
            "route-value|header-value|1|body-value",
            "route-value|header-value|2|body-value"
        ])
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn session_conflicts_unknown_slots_and_offsets(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("isolated-a/2", &session, "$result", 0);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);

    let changed_args = stream_path("isolated-a/3", &session, "$result", 0);
    assert_eq!(
        create(agent, &changed_args).await?.status(),
        StatusCode::CONFLICT
    );
    let other_method = stream_path("isolated-b/2", &session, "$result", 0);
    assert_eq!(
        create(agent, &other_method).await?.status(),
        StatusCode::CONFLICT
    );

    let unknown = stream_path("isolated-a/2", &session, "missing", 0);
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(&unknown)?)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let fresh_unknown = stream_path("isolated-a/2", &Uuid::new_v4().to_string(), "missing", 0);
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(&fresh_unknown)?)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    let malformed = format!("{path}&offset=not-an-offset");
    assert_eq!(
        agent
            .client
            .get(agent.base_url.join(&malformed)?)
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let now = format!("{path}&offset=now");
    let now_response = agent.client.get(agent.base_url.join(&now)?).send().await?;
    assert_eq!(now_response.status(), StatusCode::OK);
    assert_eq!(now_response.json::<Value>().await?, serde_json::json!([]));

    let beyond = format!("{path}&offset=0100000000000000ffffffffffffffff0000000000000000");
    let beyond = agent
        .client
        .get(agent.base_url.join(&beyond)?)
        .send()
        .await?;
    assert_eq!(beyond.status(), StatusCode::OK);
    assert_eq!(beyond.json::<Value>().await?, serde_json::json!([]));
    Ok(())
}

#[test]
#[timeout("120s")]
async fn websocket_input_is_readable_through_http(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    use crate::api::streaming_rpc::{
        connect_public_invocation_socket, receive_public_response, send_public_request,
    };
    use golem_common::model::invocation_session_public::{
        DecimalU64, INVOCATION_SESSION_VERSION, InvocationSelector, PublicClientMessage,
        PublicInvocationOutcome, PublicServerMessage,
    };

    let session = Uuid::new_v4().to_string();
    let input_reference = Uuid::new_v4();
    let mut socket =
        connect_public_invocation_socket(&agent.user.deps, Some(&agent.user.token)).await?;
    send_public_request(
        &mut socket,
        &PublicClientMessage::InvocationStart {
            attempt_id: Uuid::new_v4(),
            config: Vec::new(),
            idempotency_key: session.clone(),
            method_parameters: serde_json::json!({
                "input": { "$stream": { "provisionalRef": input_reference } }
            }),
            selector: Box::new(InvocationSelector {
                agent_type: "DurableStreamAgent".into(),
                application: agent.application_name.clone(),
                constructor_parameters: serde_json::json!({"id": format!("ds3-{session}")}),
                environment: agent.environment_name.clone(),
                method: "echo".into(),
                phantom_id: None,
            }),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    let accepted = receive_public_response(&mut socket).await?;
    let input_channel = match accepted {
        PublicServerMessage::InvocationAccepted { mappings, .. } => {
            mappings
                .iter()
                .find(|mapping| mapping.provisional_ref == Some(input_reference))
                .ok_or_else(|| anyhow::anyhow!("missing input mapping"))?
                .channel
        }
        other => anyhow::bail!("invocation was not accepted: {other:?}"),
    };
    send_public_request(
        &mut socket,
        &PublicClientMessage::InputStreamItem {
            channel: input_channel,
            sequence: DecimalU64(0),
            value: serde_json::json!("ws-first"),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    let mut output = Vec::new();
    loop {
        if let PublicServerMessage::OutputStreamItem { value, .. } =
            receive_public_response(&mut socket).await?
        {
            output.push(value);
            break;
        }
    }
    let input_path = echo_stream_path(&session, "input");
    assert_eq!(
        append_json(agent, &input_path, serde_json::json!("post-middle"), false)
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    loop {
        if let PublicServerMessage::OutputStreamItem { value, .. } =
            receive_public_response(&mut socket).await?
        {
            output.push(value);
            break;
        }
    }
    send_public_request(
        &mut socket,
        &PublicClientMessage::InputStreamItem {
            channel: input_channel,
            sequence: DecimalU64(1),
            value: serde_json::json!("ws-last"),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    send_public_request(
        &mut socket,
        &PublicClientMessage::InputStreamEnd {
            channel: input_channel,
            sequence: DecimalU64(2),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    loop {
        match receive_public_response(&mut socket).await? {
            PublicServerMessage::OutputStreamItem { value, .. } => output.push(value),
            PublicServerMessage::InvocationFinished { outcome, .. } => {
                assert!(matches!(outcome, PublicInvocationOutcome::Success));
                break;
            }
            PublicServerMessage::InvocationRejected { code, message, .. } => {
                anyhow::bail!("invocation rejected ({code:?}): {message}");
            }
            _ => {}
        }
    }
    let values = serde_json::json!(["ws-first", "post-middle", "ws-last"]);
    assert_eq!(serde_json::json!(output), values);
    for slot in ["input", "output"] {
        let path = stream_path("echo", &session, slot, 0);
        wait_for_closed(agent, &path).await?;
        let head = agent
            .client
            .head(agent.base_url.join(&path)?)
            .send()
            .await?;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(header(&head, "content-type"), "application/json");
        let response = agent.client.get(agent.base_url.join(&path)?).send().await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, "stream-closed"), "true");
        assert_eq!(response.json::<Value>().await?, values);
    }
    Ok(())
}

#[test]
#[timeout("180s")]
async fn session_delete_is_cooperative_and_survives_reconstruction(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let id = format!("ds3-{session}");
    let path = stream_path("cancellation-output/5", &session, "$result", 5000);
    let session_path = format!(
        "/durable-stream-agents/{id}/cancellation-output/5/invocations/{session}?delay_ms=5000"
    );
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);

    let first = loop {
        let response = agent.client.get(agent.base_url.join(&path)?).send().await?;
        if response.content_length() != Some(2) {
            break response;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        first.json::<Value>().await?,
        serde_json::json!(["cancel-0"])
    );
    for _ in 0..2 {
        assert_eq!(
            agent
                .client
                .delete(agent.base_url.join(&session_path)?)
                .send()
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    wait_for_observations(agent, &id, &[0, 0, 2, 1, 1, 0, 0]).await?;
    let history = agent.client.get(agent.base_url.join(&path)?).send().await?;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(header(&history, "stream-closed"), "true");
    assert_eq!(header(&history, "stream-cancelled"), "true");
    assert_eq!(
        history.json::<Value>().await?,
        serde_json::json!(["cancel-0"])
    );

    let marked = agent
        .client
        .put(
            agent
                .base_url
                .join(&format!("/durable-stream-agents/{id}/mark/37"))?,
        )
        .send()
        .await?;
    assert_eq!(marked.status(), StatusCode::OK);
    assert_eq!(marked.json::<Value>().await?, serde_json::json!(37));
    assert_eq!(observations(agent, &id).await?, vec![0, 0, 2, 1, 1, 37, 0]);

    let component_id = agent
        .client
        .put(
            agent
                .base_url
                .join(&format!("/durable-stream-agents/{id}/component-id"))?,
        )
        .send()
        .await?
        .json::<String>()
        .await?;
    let worker = AgentId {
        component_id: ComponentId(Uuid::parse_str(&component_id)?),
        agent_id: format!("DurableStreamAgent(\"{id}\")"),
    };
    let before = agent.user.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert!(!before.is_empty());
    agent.user.simulated_crash(&worker).await?;
    agent.user.resume(&worker, false).await?;
    assert_eq!(observations(agent, &id).await?, vec![0, 0, 2, 1, 1, 37, 0]);
    let history = agent.client.get(agent.base_url.join(&path)?).send().await?;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(header(&history, "stream-cancelled"), "true");
    assert_eq!(
        history.json::<Value>().await?,
        serde_json::json!(["cancel-0"])
    );
    assert!(
        agent
            .user
            .get_oplog(&worker, OplogIndex::INITIAL)
            .await?
            .len()
            >= before.len()
    );
    Ok(())
}

#[test]
#[timeout("180s")]
async fn input_slot_delete_is_guest_observable_and_tombstoned(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    use crate::api::streaming_rpc::{
        connect_public_invocation_socket, receive_public_response, send_public_request,
    };
    use golem_common::model::invocation_session_public::{
        DecimalU64, INVOCATION_SESSION_VERSION, InvocationSelector, PublicClientMessage,
        PublicServerMessage,
    };

    let session = Uuid::new_v4().to_string();
    let id = format!("ds3-{session}");
    let input_reference = Uuid::new_v4();
    let mut socket =
        connect_public_invocation_socket(&agent.user.deps, Some(&agent.user.token)).await?;
    send_public_request(
        &mut socket,
        &PublicClientMessage::InvocationStart {
            attempt_id: Uuid::new_v4(),
            config: Vec::new(),
            idempotency_key: session.clone(),
            method_parameters: serde_json::json!({
                "input": { "$stream": { "provisionalRef": input_reference } }
            }),
            selector: Box::new(InvocationSelector {
                agent_type: "DurableStreamAgent".into(),
                application: agent.application_name.clone(),
                constructor_parameters: serde_json::json!({"id": id}),
                environment: agent.environment_name.clone(),
                method: "echo".into(),
                phantom_id: None,
            }),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    let input_channel = match receive_public_response(&mut socket).await? {
        PublicServerMessage::InvocationAccepted { mappings, .. } => {
            mappings
                .into_iter()
                .find(|mapping| mapping.provisional_ref == Some(input_reference))
                .ok_or_else(|| anyhow::anyhow!("missing input mapping"))?
                .channel
        }
        other => anyhow::bail!("invocation was not accepted: {other:?}"),
    };
    send_public_request(
        &mut socket,
        &PublicClientMessage::InputStreamItem {
            channel: input_channel,
            sequence: DecimalU64(0),
            value: serde_json::json!("kept"),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    loop {
        if matches!(
            receive_public_response(&mut socket).await?,
            PublicServerMessage::OutputStreamItem { .. }
        ) {
            break;
        }
    }
    let input = stream_path("echo", &session, "input", 0);
    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&input)?)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    for method in [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::DELETE,
        Method::PUT,
    ] {
        let expected = if method == Method::PUT {
            StatusCode::CONFLICT
        } else {
            StatusCode::GONE
        };
        assert_eq!(
            agent
                .client
                .request(method, agent.base_url.join(&input)?)
                .send()
                .await?
                .status(),
            expected
        );
    }
    wait_for_observations(agent, &id, &[1, 1, 1, 0, 1, 0, 0]).await?;
    Ok(())
}

#[test]
#[timeout("180s")]
async fn output_slot_delete_is_guest_observable_and_tombstoned(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let id = format!("ds3-{session}");
    let output = stream_path("cancellation-output/5", &session, "$result", 5000);
    assert_eq!(create(agent, &output).await?.status(), StatusCode::CREATED);
    let first = loop {
        let response = agent
            .client
            .get(agent.base_url.join(&output)?)
            .send()
            .await?;
        if response.content_length() != Some(2) {
            break response;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        first.json::<Value>().await?,
        serde_json::json!(["cancel-0"])
    );
    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&output)?)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    for method in [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::DELETE,
        Method::PUT,
    ] {
        let expected = if method == Method::PUT {
            StatusCode::CONFLICT
        } else {
            StatusCode::GONE
        };
        assert_eq!(
            agent
                .client
                .request(method, agent.base_url.join(&output)?)
                .send()
                .await?
                .status(),
            expected
        );
    }
    wait_for_observations(agent, &id, &[0, 0, 2, 1, 1, 0, 0]).await?;

    let component_id = agent
        .client
        .put(
            agent
                .base_url
                .join(&format!("/durable-stream-agents/{id}/component-id"))?,
        )
        .send()
        .await?
        .json::<String>()
        .await?;
    let worker = AgentId {
        component_id: ComponentId(Uuid::parse_str(&component_id)?),
        agent_id: format!("DurableStreamAgent(\"{id}\")"),
    };
    agent.user.simulated_crash(&worker).await?;
    agent.user.resume(&worker, false).await?;
    for method in [Method::GET, Method::HEAD, Method::POST, Method::DELETE] {
        assert_eq!(
            agent
                .client
                .request(method, agent.base_url.join(&output)?)
                .send()
                .await?
                .status(),
            StatusCode::GONE
        );
    }
    assert_eq!(observations(agent, &id).await?, vec![0, 0, 2, 1, 1, 0, 0]);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn live_reader_limit_and_disconnect_release(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let session = Uuid::new_v4().to_string();
    let path = stream_path("echo", &session, "input", 0);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    let url = agent
        .base_url
        .join(&format!("{path}&offset=now&live=sse"))?;
    let mut readers = Vec::new();
    for _ in 0..15 {
        let mut response = agent.client.get(url.clone()).send().await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.chunk().await?.is_some());
        readers.push(response);
    }
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).await?;
    socket
        .write_all(
            format!(
                "GET {}?{} HTTP/1.1\r\nHost: {}\r\n\r\n",
                url.path(),
                url.query().unwrap(),
                agent.host_header.to_str()?
            )
            .as_bytes(),
        )
        .await?;
    let mut initial = Vec::new();
    while !initial.windows(2).any(|bytes| bytes == b"\n\n") {
        assert_ne!(socket.read_buf(&mut initial).await?, 0);
    }
    assert!(initial.starts_with(b"HTTP/1.1 200"));
    let rejected = agent.client.get(url.clone()).send().await?;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(header(&rejected, "retry-after"), "1");
    socket.shutdown().await?;
    drop(socket);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let response = agent.client.get(url.clone()).send().await?;
            if response.status() == StatusCode::OK {
                readers.push(response);
                break;
            }
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let rejected = agent.client.get(url).send().await?;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(header(&rejected, "retry-after"), "1");
    Ok(())
}

#[test]
#[timeout("120s")]
async fn catch_up_burst_is_rate_limited_and_recovers(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("json/1", &session, "$result", 0);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    wait_for_closed(agent, &path).await?;
    let url = agent.base_url.join(&path)?;
    let responses =
        futures::future::join_all((0..256).map(|_| agent.client.get(url.clone()).send())).await;
    let mut rejected = 0;
    let mut accepted = 0;
    for response in responses {
        let response = response?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            rejected += 1;
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.json::<Value>().await?,
                serde_json::json!(["message-0000"])
            );
            accepted += 1;
        }
    }
    assert!(accepted > 0);
    assert!(rejected > 0, "catch-up burst never reached the rate limit");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let recovered = agent.client.get(url).send().await?;
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(
        recovered.json::<Value>().await?,
        serde_json::json!(["message-0000"])
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn long_poll_wakes_and_times_out(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("json/2", &session, "$result", 300);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);

    let started = Instant::now();
    let wake = agent
        .client
        .get(
            agent
                .base_url
                .join(&format!("{path}&offset=now&live=long-poll"))?,
        )
        .send()
        .await?;
    assert_eq!(wake.status(), StatusCode::OK);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!wake.json::<Value>().await?.as_array().unwrap().is_empty());

    let timeout_session = Uuid::new_v4().to_string();
    let timeout_path = stream_path("echo", &timeout_session, "input", 0);
    assert_eq!(
        create(agent, &timeout_path).await?.status(),
        StatusCode::CREATED
    );
    let head = agent
        .client
        .head(agent.base_url.join(&timeout_path)?)
        .send()
        .await?;
    let offset = header(&head, "stream-next-offset");
    let started = Instant::now();
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join(&format!("{timeout_path}&offset={offset}&live=long-poll"))?,
        )
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(started.elapsed() >= Duration::from_secs(29));
    assert_eq!(header(&response, "stream-next-offset"), offset);
    assert!(response.headers().contains_key("stream-cursor"));
    assert_eq!(header(&response, CACHE_CONTROL.as_str()), "no-store");
    Ok(())
}

#[test]
#[timeout("180s")]
async fn output_stream_survives_guest_sleep_suspension(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("json/1", &session, "$result", 15000);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    wait_for_closed(agent, &path).await?;
    let result = agent.client.get(agent.base_url.join(&path)?).send().await?;
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(header(&result, "stream-closed"), "true");
    assert!(!result.headers().contains_key("stream-cancelled"));
    assert_eq!(
        result.json::<Value>().await?,
        serde_json::json!(["message-0000"])
    );
    Ok(())
}

#[test]
#[timeout("180s")]
async fn cancellation_closes_pending_output_without_waiting_for_guest(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("delayed-output/10000", &session, "$result", 0);
    let session_path =
        format!("/durable-stream-agents/ds3-{session}/delayed-output/10000/invocations/{session}");
    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&session_path)?)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    let head = agent
        .client
        .head(agent.base_url.join(&path)?)
        .send()
        .await?;
    assert_eq!(header(&head, "stream-closed"), "false");
    let wrong_method = format!("/durable-stream-agents/ds3-{session}/json/1/invocations/{session}");
    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&wrong_method)?)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        agent
            .client
            .delete(agent.base_url.join(&session_path)?)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let result = agent
        .client
        .get(agent.base_url.join(&format!("{path}&live=long-poll"))?)
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    assert_eq!(result.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&result, "stream-closed"), "true");
    assert_eq!(header(&result, "stream-cancelled"), "true");
    // The method later produces its output; that must not reopen the cancelled slot.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let result = agent.client.get(agent.base_url.join(&path)?).send().await?;
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(header(&result, "stream-closed"), "true");
    assert_eq!(header(&result, "stream-cancelled"), "true");
    assert_eq!(result.json::<Value>().await?, serde_json::json!([]));
    Ok(())
}

#[test]
#[timeout("180s")]
async fn pending_output_and_data_share_long_poll_wait_budget(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("delayed-output/10000", &session, "$result", 60000);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    let started = Instant::now();
    let result = agent
        .client
        .get(agent.base_url.join(&format!("{path}&live=long-poll"))?)
        .send()
        .await?;
    assert_eq!(result.status(), StatusCode::NO_CONTENT);
    assert!(started.elapsed() >= Duration::from_secs(29));
    assert!(
        started.elapsed() < Duration::from_secs(36),
        "waiting for an output handle must not start a second 30s data wait: {:?}",
        started.elapsed()
    );
    assert_eq!(header(&result, "stream-closed"), "false");
    assert_eq!(header(&result, "stream-up-to-date"), "true");
    Ok(())
}

#[test]
#[timeout("180s")]
async fn sse_reconnect_etag_and_binary_encoding(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = stream_path("json/1000", &session, "$result", 1);
    assert_eq!(create(agent, &path).await?.status(), StatusCode::CREATED);
    let mut first = agent
        .client
        .get(agent.base_url.join(&format!("{path}&live=sse"))?)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await?;
    assert_eq!(header(&first, CONTENT_TYPE.as_str()), "text/event-stream");
    assert_eq!(header(&first, CACHE_CONTROL.as_str()), "no-store");
    let disconnect_after = 100 + (Uuid::new_v4().as_u128() % 700) as usize;
    let mut messages = Vec::<String>::new();
    let mut pending = Vec::new();
    let resume_offset = 'read: loop {
        let chunk = first
            .chunk()
            .await?
            .ok_or_else(|| anyhow::anyhow!("SSE ended before the selected reconnect point"))?;
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.windows(2).position(|bytes| bytes == b"\n\n") {
            let event = String::from_utf8(pending.drain(..end + 2).collect())?;
            if let Some(data) = event.strip_prefix("event: data\ndata: ") {
                messages.extend(serde_json::from_str::<Vec<String>>(data.trim_end())?);
            } else if let Some(data) = event.strip_prefix("event: control\ndata: ") {
                let control: Value = serde_json::from_str(data.trim_end())?;
                if messages.len() >= disconnect_after {
                    assert!(
                        messages.len() < 1000,
                        "must reconnect before consuming the entire stream"
                    );
                    break 'read control["streamNextOffset"]
                        .as_str()
                        .expect("control offset")
                        .to_owned();
                }
            }
        }
    };
    drop(first);
    let resumed = agent
        .client
        .get(
            agent
                .base_url
                .join(&format!("{path}&live=sse&offset={resume_offset}"))?,
        )
        .send()
        .await?
        .text()
        .await?;
    let (remaining, final_offset) = parse_sse(&resumed)?;
    messages.extend(remaining);
    assert_eq!(
        messages,
        (0..1000)
            .map(|i| format!("message-{i:04}"))
            .collect::<Vec<_>>()
    );

    let reconnect = agent
        .client
        .get(
            agent
                .base_url
                .join(&format!("{path}&live=sse&offset={final_offset}"))?,
        )
        .send()
        .await?
        .text()
        .await?;
    assert!(
        parse_sse(&reconnect)?.0.is_empty(),
        "reconnect at the terminal offset duplicated data"
    );

    let catch_up = agent.client.get(agent.base_url.join(&path)?).send().await?;
    assert_eq!(header(&catch_up, "stream-closed"), "false");
    assert_eq!(
        header(&catch_up, CACHE_CONTROL.as_str()),
        "public, max-age=31536000, immutable"
    );
    let etag = header(&catch_up, ETAG.as_str());
    let conditional = agent
        .client
        .get(agent.base_url.join(&path)?)
        .header(IF_NONE_MATCH, etag)
        .send()
        .await?;
    assert_eq!(conditional.status(), StatusCode::NOT_MODIFIED);
    assert!(conditional.bytes().await?.is_empty());

    let binary_session = Uuid::new_v4().to_string();
    let binary = stream_path("bytes/8", &binary_session, "$result", 0);
    assert_eq!(create(agent, &binary).await?.status(), StatusCode::CREATED);
    let response = agent
        .client
        .get(agent.base_url.join(&format!("{binary}&live=sse"))?)
        .send()
        .await?;
    assert_eq!(header(&response, "stream-sse-data-encoding"), "base64");
    let body = response.text().await?;
    let encoded = sse_data_events(&body)
        .next()
        .expect("binary SSE data event");
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(encoded)?,
        (0..8).collect::<Vec<u8>>()
    );
    Ok(())
}

fn sse_data_events(body: &str) -> impl Iterator<Item = &str> {
    body.split("\n\n")
        .filter_map(|event| event.strip_prefix("event: data\ndata: ").map(str::trim_end))
}

fn parse_sse(body: &str) -> anyhow::Result<(Vec<String>, String)> {
    let mut messages = Vec::new();
    let mut offset = None;
    for event in body.split("\n\n") {
        if let Some(data) = event.strip_prefix("event: data\ndata: ") {
            messages.extend(serde_json::from_str::<Vec<String>>(data.trim_end())?);
        } else if let Some(data) = event.strip_prefix("event: control\ndata: ") {
            let control: Value = serde_json::from_str(data.trim_end())?;
            offset = control["streamNextOffset"].as_str().map(ToOwned::to_owned);
        }
    }
    Ok((
        messages,
        offset.ok_or_else(|| anyhow::anyhow!("SSE control event missing offset"))?,
    ))
}
