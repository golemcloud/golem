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

use crate::custom_api::http_test_context::{test_context_internal, HttpTestContext};
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use reqwest::Url;
use serde_json::json;
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test_dep]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    test_context_internal(deps, "http_rust_debug", "http:rust")
        .await
        .unwrap()
}

#[test]
#[tracing::instrument]
async fn string_path_var(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/string-path-var/foo")?,
        )
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "value": "foo" }));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn multi_path_vars(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/multi-path-vars/foo/bar")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "joined": "foo:bar" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn remaining_path_variable(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/rest/a/b/c/d")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body,
        json!({
            "tail": "a/b/c/d"
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn remaining_path_missing(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/http-agents/test-agent/rest")?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn path_and_query(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/path-and-query/item-123?limit=10")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body,
        json!({
            "id": "item-123",
            "limit": 10
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn path_and_header(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/path-and-header/res-42")?,
        )
        .header("x-request-id", "req-abc")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body,
        json!({
            "resource-id": "res-42",
            "request-id": "req-abc"
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn json_body(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/json-body/item-1")?,
        )
        .json(&json!({
            "name": "test",
            "count": 42
        }))
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "ok": true }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn json_body_missing_field(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/json-body/item-1")?,
        )
        .json(&json!({
            "name": "test"
        }))
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn json_body_wrong_type(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/json-body/item-1")?,
        )
        .json(&json!({
            "name": "test",
            "count": "not-a-number"
        }))
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn unrestricted_unstructured_binary_inline(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/unrestricted-unstructured-binary/my-bucket")?,
        )
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(vec![1u8, 2, 3, 4, 5])
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!(5));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn unrestricted_unstructured_binary_missing_body(
    agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/unrestricted-unstructured-binary/my-bucket")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!(0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn unrestricted_unstructured_binary_json_content_type(
    agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/unrestricted-unstructured-binary/my-bucket")?,
        )
        .json(&json!({ "oops": true }))
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!(13));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn restricted_unstructured_binary_inline(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/restricted-unstructured-binary/my-bucket")?,
        )
        .header(reqwest::header::CONTENT_TYPE, "image/gif")
        .body(vec![1u8, 2, 3, 4, 5])
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!(5));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn restricted_unstructured_binary_missing_body(
    agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/restricted-unstructured-binary/my-bucket")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn restricted_unstructured_binary_unsupported_mime_type(
    agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/restricted-unstructured-binary/my-bucket")?,
        )
        .json(&json!({ "oops": true }))
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_no_content(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/no-content")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(response.bytes().await?.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_json(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/http-agents/test-agent/resp/json")?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "value": "ok" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_optional_found(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/optional/true")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "value": "yes" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_optional_not_found(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/optional/false")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(response.bytes().await?.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_result_ok(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/result-json-json/true")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "value": "ok" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_result_err(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/result-json-json/false")?,
        )
        .send()
        .await?;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "error": "boom" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_result_void_err(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/result-void-json")?,
        )
        .send()
        .await?;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "error": "fail" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_result_json_void(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/resp/result-json-void")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "value": "ok" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn response_binary(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/http-agents/test-agent/resp/binary")?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()?;
    assert_eq!(content_type, "application/octet-stream");

    let body = response.bytes().await?;
    assert_eq!(&body[..], &[1, 2, 3, 4]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn negative_missing_path_var(agent: &HttpTestContext) -> anyhow::Result<()> {
    // second path variable missing
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/multi-path-vars/foo")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn negative_extra_path_segment(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/string-path-var/foo/bar")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn negative_missing_query_param(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/path-and-query/item-123")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn negative_invalid_query_param_type(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/path-and-query/item-123?limit=not-a-number")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn negative_missing_header(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/path-and-header/res-42")?,
        )
        // no x-request-id header
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn cors_preflight_wildcard(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .request(
            reqwest::Method::OPTIONS,
            agent.base_url.join("/cors-agents/test-agent/wildcard")?,
        )
        .header("Origin", "https://any-origin.com")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .unwrap()
        .to_str()?;
    assert_eq!(allow_origin, "https://any-origin.com");

    let vary = response.headers().get("vary").unwrap().to_str()?;
    assert_eq!(vary, "Origin");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cors_preflight_specific_origin(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .request(
            reqwest::Method::OPTIONS,
            agent
                .base_url
                .join("/cors-agents/test-agent/preflight-required")?,
        )
        .header("Origin", "https://app.example.com")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .unwrap()
        .to_str()?;
    assert_eq!(allow_origin, "https://app.example.com");

    let allow_methods = response
        .headers()
        .get("access-control-allow-methods")
        .unwrap()
        .to_str()?;
    assert!(allow_methods.contains("POST"));

    let vary = response.headers().get("vary").unwrap().to_str()?;
    assert_eq!(vary, "Origin");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cors_get_with_origin_header(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/cors-agents/test-agent/inherited")?)
        .header("Origin", "https://mount.example.com")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .unwrap()
        .to_str()?;
    assert_eq!(allow_origin, "https://mount.example.com");

    let vary = response.headers().get("vary").unwrap().to_str()?;
    assert_eq!(vary, "Origin");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cors_get_with_origin_header_invalid(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/cors-agents/test-agent/inherited")?)
        .header("Origin", "https://not-allowed.com")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cors_get_wildcard_origin(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/cors-agents/test-agent/wildcard")?)
        .header("Origin", "https://random-origin.com")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .unwrap()
        .to_str()?;
    assert_eq!(allow_origin, "https://random-origin.com");

    let vary = response.headers().get("vary").unwrap().to_str()?;
    assert_eq!(vary, "Origin");

    Ok(())
}

// TODO; refactor serving of webhook request (between TS and Rust)
#[test]
#[tracing::instrument]
async fn webhook_callback_rust(agent: &HttpTestContext) -> anyhow::Result<()> {
    use axum::{body::Bytes, routing::post, Router};
    use reqwest::Client;
    use std::sync::Arc;
    use tokio::spawn;
    use tokio::sync::Mutex;

    let host_header = agent.host_header.clone();
    let (agent_host, agent_port) = agent.base_url.authority().split_once(':').unwrap();
    let agent_host = agent_host.to_string();
    let agent_port = agent_port.parse::<u16>().unwrap();

    let received_webhook_request = Arc::new(Mutex::new(Vec::new()));
    let received_webhook_request_clone = received_webhook_request.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let http_server = spawn(async move {
        let route = Router::new().route(
            "/",
            post(move |body: Bytes| {
                let received_webhook_request_clone = received_webhook_request_clone.clone();
                async move {
                    let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let webhook_url_str = body_json["webhookUrl"].as_str().unwrap();

                    let mut lock = received_webhook_request_clone.lock().await;
                    *lock = body.to_vec();

                    let mut url: Url = webhook_url_str.parse().unwrap();
                    url.set_host(Some(&agent_host)).unwrap();
                    url.set_port(Some(agent_port)).unwrap();

                    let client = Client::new();
                    let payload = vec![1u8, 2, 3, 4, 5];
                    client
                        .post(url)
                        .header("Host", host_header.clone())
                        .body(payload.clone())
                        .send()
                        .await
                        .unwrap();

                    "ok"
                }
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let test_server_url = format!("http://127.0.0.1:{}/", port);
    agent
        .client
        .post(
            agent
                .base_url
                .join("/webhook-agents/test-agent/set-test-server-url")?,
        )
        .json(&serde_json::json!({ "test-server-url": test_server_url }))
        .send()
        .await?
        .error_for_status()?;

    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/webhook-agents/test-agent/test-webhook")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "payload-length": 5 }));

    http_server.abort();

    Ok(())
}
