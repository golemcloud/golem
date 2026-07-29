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

//! HTTP integration tests for the read-only agent method feature (issue #3393,
//! tests H1..H5).
//!
//! All six tests (H4 is split into principal-unaware / principal-aware halves)
//! exercise the `ReadonlyAgent` agent declared in the shared
//! `agent-sdk-rust` test component. They drive the worker-service HTTP layer
//! through `HttpTestContext` and assert the cache-header contract documented
//! in `golem-worker-service/src/custom_api/call_agent/cache_headers.rs`:
//!
//! - `Cache-Control: <visibility>, no-cache` for `UntilWrite`
//! - `Cache-Control: <visibility>, max-age=<floor(d,1s)>` for `Ttl(d)`
//! - `ETag` only when the cache policy supports revalidation
//! - `If-None-Match` short-circuits to `304 Not Modified` when the ETag
//!   matches the current `(fingerprint, oplog-index)`
//! - `Vary` includes `Authorization` (or the configured session header) for
//!   principal-aware methods

use super::assert_json_content_type;
use crate::custom_api::http_test_context::{HttpTestContext, make_test_context};
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use reqwest::header::{CACHE_CONTROL, CONTENT_LENGTH, ETAG, HeaderValue, IF_NONE_MATCH, VARY};
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

const AGENT_PATH_PREFIX: &str = "/readonly-agents/h-agent";

async fn build_test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    make_test_context(
        deps,
        vec![(
            AgentTypeName("ReadonlyAgent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )],
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

fn header_str(value: Option<&HeaderValue>) -> Option<&str> {
    value.and_then(|v| v.to_str().ok())
}

/// True if `header_value` contains `token` as a complete comma-separated
/// entry, case-insensitive. Used for `Vary` assertions so that an entry like
/// `X-Authorization-Mode` cannot satisfy a check for `Authorization`.
fn header_has_token(header_value: &str, token: &str) -> bool {
    header_value
        .split(',')
        .any(|part| part.trim().eq_ignore_ascii_case(token))
}

// ---------------------------------------------------------------------------
// H1 — GET on a read-only method emits ETag and Cache-Control
// ---------------------------------------------------------------------------

#[test]
#[tracing::instrument]
async fn h1_get_returns_etag_and_cache_control(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join(&format!("{AGENT_PATH_PREFIX}/count"))?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_json_content_type(&response);

    let cache_control =
        header_str(response.headers().get(CACHE_CONTROL)).expect("expected Cache-Control header");
    assert_eq!(
        cache_control, "public, no-cache",
        "principal-unaware UntilWrite must emit `public, no-cache`"
    );

    let etag = header_str(response.headers().get(ETAG))
        .expect("expected ETag header for an UntilWrite read-only method");
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "ETag must be a strong validator (quoted), got {etag}"
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, serde_json::json!(0));

    Ok(())
}

// ---------------------------------------------------------------------------
// H2 — If-None-Match with current ETag returns 304 Not Modified
// ---------------------------------------------------------------------------

#[test]
#[tracing::instrument]
async fn h2_if_none_match_returns_304(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    // Use a distinct path so this test doesn't interfere with H3's writes.
    let path = "/readonly-agents/h2-agent/count";

    let first = agent.client.get(agent.base_url.join(path)?).send().await?;
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    let etag = header_str(first.headers().get(ETAG))
        .expect("expected ETag header on first GET")
        .to_string();

    let revalidate = agent
        .client
        .get(agent.base_url.join(path)?)
        .header(IF_NONE_MATCH, etag.clone())
        .send()
        .await?;

    assert_eq!(
        revalidate.status(),
        reqwest::StatusCode::NOT_MODIFIED,
        "If-None-Match with the current ETag must return 304"
    );

    // 304 responses must echo the same ETag and Cache-Control so caches can
    // refresh their freshness state without re-downloading the body.
    let revalidate_etag =
        header_str(revalidate.headers().get(ETAG)).expect("304 response must include an ETag");
    assert_eq!(revalidate_etag, etag, "304 must preserve the original ETag");
    let revalidate_cache_control = header_str(revalidate.headers().get(CACHE_CONTROL))
        .expect("304 response must include Cache-Control");
    assert_eq!(revalidate_cache_control, "public, no-cache");

    // 304 responses must not include a body.
    let content_length = header_str(revalidate.headers().get(CONTENT_LENGTH));
    if let Some(len) = content_length {
        assert_eq!(len, "0", "304 response must have Content-Length: 0");
    }
    let body = revalidate.text().await?;
    assert!(body.is_empty(), "304 response body must be empty");

    Ok(())
}

// ---------------------------------------------------------------------------
// H3 — write between reads invalidates the ETag (200 with new ETag)
// ---------------------------------------------------------------------------

#[test]
#[tracing::instrument]
async fn h3_etag_invalidates_after_write(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let path = "/readonly-agents/h3-agent/count";
    let increment_path = "/readonly-agents/h3-agent/increment";

    // Initial GET to mint an ETag.
    let first = agent.client.get(agent.base_url.join(path)?).send().await?;
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    let initial_etag = header_str(first.headers().get(ETAG))
        .expect("expected ETag on first GET")
        .to_string();

    // Mutating write: increment the counter.
    let write = agent
        .client
        .post(agent.base_url.join(increment_path)?)
        .send()
        .await?;
    assert_eq!(write.status(), reqwest::StatusCode::OK);

    // Re-validate with the stale ETag: must NOT be 304.
    let revalidate = agent
        .client
        .get(agent.base_url.join(path)?)
        .header(IF_NONE_MATCH, initial_etag.clone())
        .send()
        .await?;
    assert_eq!(
        revalidate.status(),
        reqwest::StatusCode::OK,
        "after a write, a stale If-None-Match must NOT return 304"
    );

    let new_etag = header_str(revalidate.headers().get(ETAG))
        .expect("expected ETag on post-write GET")
        .to_string();
    assert_ne!(
        new_etag, initial_etag,
        "ETag must advance after a non-read-only write"
    );

    let body: serde_json::Value = revalidate.json().await?;
    assert_eq!(body, serde_json::json!(1));

    Ok(())
}

// ---------------------------------------------------------------------------
// H4 — principal-unaware uses `public`, principal-aware uses `private` + Vary
// ---------------------------------------------------------------------------

#[test]
#[tracing::instrument]
async fn h4_principal_unaware_uses_public_cache_directive(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/readonly-agents/h4u-agent/count")?)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let cache_control =
        header_str(response.headers().get(CACHE_CONTROL)).expect("expected Cache-Control header");
    assert_eq!(
        cache_control, "public, no-cache",
        "principal-unaware UntilWrite read must emit `public, no-cache`"
    );

    // No Vary on principal-carrying request header is required for a
    // principal-unaware method.
    let vary = header_str(response.headers().get(VARY)).unwrap_or("");
    assert!(
        !header_has_token(vary, "Authorization"),
        "principal-unaware method must NOT add `Authorization` to Vary, got: {vary:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn h4_principal_aware_uses_private_cache_directive_and_vary(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/readonly-agents/h4p-agent/count-for")?,
        )
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let cache_control =
        header_str(response.headers().get(CACHE_CONTROL)).expect("expected Cache-Control header");
    assert_eq!(
        cache_control, "private, no-cache",
        "principal-aware UntilWrite read must emit `private, no-cache`"
    );

    let vary = header_str(response.headers().get(VARY))
        .expect("expected Vary header for principal-aware read");
    assert!(
        header_has_token(vary, "Authorization"),
        "principal-aware method must add `Authorization` to Vary, got: {vary}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// H5 — Ttl cache policy emits `max-age=<seconds>`
// ---------------------------------------------------------------------------

#[test]
#[tracing::instrument]
async fn h5_ttl_method_returns_max_age(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/readonly-agents/h5-agent/ttl-count")?)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let cache_control =
        header_str(response.headers().get(CACHE_CONTROL)).expect("expected Cache-Control header");
    // The agent is declared with `#[read_only(cache = "ttl", ttl = "2s")]`
    // and the method is principal-unaware, so visibility must be `public`
    // and `max-age` must be 2 seconds (floor of 2s).
    assert_eq!(
        cache_control, "public, max-age=2",
        "TTL read must emit `public, max-age=2`, got {cache_control}"
    );

    // TTL still supports revalidation, so an ETag must be emitted as well.
    assert!(
        response.headers().get(ETAG).is_some(),
        "TTL read must also emit an ETag for conditional GETs"
    );

    Ok(())
}
