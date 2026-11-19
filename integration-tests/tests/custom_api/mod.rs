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

mod agent_http_principal_ts;
mod agent_http_routes_rust;
mod agent_http_routes_ts;
mod durable_streams;
mod http_test_context;
mod live_files;
mod mcp;
mod oidc_pkce;
mod openapi_generation;
mod phantom_selection;
mod raw_http_router;
mod readonly_http;

use golem_test_framework::config::EnvBasedTestDependencies;
use test_r::inherit_test_dep;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("postgres")]
    EnvBasedTestDependencies
);
inherit_test_dep!(
    #[tagged_as("sqlite")]
    EnvBasedTestDependencies
);

fn assert_json_content_type(response: &reqwest::Response) {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected Content-Type: application/json, got: {content_type:?}"
    );
}

async fn assert_query_bindings(agent: &http_test_context::HttpTestContext) -> anyhow::Result<()> {
    use pretty_assertions::assert_eq;
    use reqwest::{Method, StatusCode};
    use serde_json::{Value, json};

    let base = "/http-agents/query-agent";
    let collections = "values=7&values=2&labels%5B9%5D=true&labels%5B3%5D=false";
    let echo = json!(["values:7,2", "labels:3=false,9=true"]);
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join(&format!("{base}/query/item-a?q=a+%2Fb&{collections}"))?,
        )
        .header("x-request-id", "trace-b")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await?,
        json!([
            "item-a",
            "a /b",
            "trace-b",
            "values:7,2",
            "labels:3=false,9=true"
        ])
    );

    let response = agent
        .client
        .post(
            agent
                .base_url
                .join(&format!("{base}/query-explicit?{collections}"))?,
        )
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await?, echo);

    for (query, expected) in [
        ("", json!(["values:none", "labels:none"])),
        ("?values=7&values=2", json!(["values:7,2", "labels:none"])),
        (
            "?labels[9]=true&labels[3]=false",
            json!(["values:none", "labels:3=false,9=true"]),
        ),
    ] {
        let response = agent
            .client
            .get(
                agent
                    .base_url
                    .join(&format!("{base}/query-optional{query}"))?,
            )
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await?, expected);
    }

    let body = json!({"values": [7, 2], "labels": [[9, true], [3, false]]});
    let response = agent
        .client
        .post(agent.base_url.join(&format!("{base}/query-body"))?)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await?, echo);

    for (method, path, query, input) in [
        (Method::GET, "query/item-a", "q=a&labels[3]=false", "values"),
        (Method::GET, "query/item-a", "q=a&values=7", "labels"),
        (
            Method::GET,
            "query/item-a",
            "q=a&q=b&values=7&labels[3]=false",
            "search",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=-1&labels[3]=false",
            "values",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=7&labels[1]=true&labels[01]=false",
            "labels",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=7&labels[3]=true&labels[3]=false",
            "labels",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=7&labels=true",
            "labels",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=7&labels[]=true",
            "labels",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=7&labels[3][4]=true",
            "labels",
        ),
        (
            Method::GET,
            "query/item-a",
            "q=a&values=7&labels[3]=true&search=b",
            "search",
        ),
        (Method::GET, "query-optional", "values=", "values"),
        (Method::POST, "query-explicit", "values=7", "labels"),
        (Method::POST, "query-body", "values=7", "values"),
    ] {
        let response = agent
            .client
            .request(
                method,
                agent.base_url.join(&format!("{base}/{path}?{query}"))?,
            )
            .header("x-request-id", "trace-b")
            .json(&body)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}?{query}");
        let error = response.text().await?;
        assert!(error.contains(input), "expected {input} in {error}");
    }

    let response = agent
        .client
        .delete(
            agent
                .base_url
                .join(&format!("{base}/query-delete?value=23"))?,
        )
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await?, json!(23));
    for query in ["", "?value=23&value=24"] {
        let response = agent
            .client
            .delete(
                agent
                    .base_url
                    .join(&format!("{base}/query-delete{query}"))?,
            )
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let file = agent
        .client
        .get(agent.base_url.join(&format!("{base}/query-file?v=3"))?)
        .send()
        .await?;
    assert_eq!(file.status(), StatusCode::OK);
    assert_eq!(file.text().await?, "query file");
    let typed = agent
        .client
        .get(agent.base_url.join(&format!("{base}/query-optional?v=3"))?)
        .send()
        .await?;
    assert_eq!(typed.status(), StatusCode::BAD_REQUEST);
    assert!(typed.text().await?.contains("v"));
    Ok(())
}
