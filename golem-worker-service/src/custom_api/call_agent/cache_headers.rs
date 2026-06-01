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
//
//! HTTP cache-header helpers used when binding read-only `AgentMethod` HTTP routes.
//!
//! The worker-service emits `Cache-Control`, `ETag`, and (when the read-only
//! method depends on the caller's principal) `Vary: Authorization` for every
//! cacheable HTTP method (`GET` / `HEAD`) that resolves to an `AgentMethod`
//! whose `read-only` config is `Some(_)`.
//!
//! Conversely, for `If-None-Match` revalidation requests, the worker-service
//! can produce a `304 Not Modified` response without invoking the executor
//! when the supplied `ETag` matches the agent's current oplog index.
//!
//! See [#3392](https://github.com/golemcloud/golem/issues/3392) and
//! [`read-only-agent-methods.md`](../../../../read-only-agent-methods.md) for
//! the full design.

use golem_common::model::agent::{CachePolicy, ReadOnlyConfig};
use golem_common::model::{AgentFingerprint, AgentId, OplogIndex};
use http::HeaderName;
use http::header;

/// `Cache-Control` visibility for a read-only method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheVisibility {
    /// Caches may be shared across users (e.g. a CDN). Set when the method
    /// does **not** depend on the caller's principal.
    Public,
    /// Only per-user caches (e.g. a browser cache) may store this response.
    /// Set when the method depends on the caller's principal.
    Private,
}

impl CacheVisibility {
    pub fn for_read_only(read_only: &ReadOnlyConfig) -> Self {
        if read_only.uses_principal {
            Self::Private
        } else {
            Self::Public
        }
    }

    pub fn as_token(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

/// Build the value for the `ETag` response header for a read-only invocation.
///
/// The format is `"<agent-id>/<fingerprint>:<oplog-index>"`, with surrounding
/// quotes so the value is syntactically a strong validator. The agent name is
/// URL-encoded via [`AgentId::agent_name_encoded`] so it can never contain
/// commas, quotes, or other characters that would otherwise confuse HTTP
/// parsers or the `If-None-Match` splitter.
///
/// Including the per-instance agent fingerprint guards against the (rare)
/// case of an agent being deleted and a fresh agent being created at the same
/// `AgentId` that happens to reach the same oplog index: without the
/// fingerprint, a previously-cached client-side ETag would incorrectly match
/// the new instance.
pub fn build_etag_value(
    agent_id: &AgentId,
    fingerprint: AgentFingerprint,
    oplog_index: OplogIndex,
) -> String {
    format!(
        "\"{}:{}\"",
        agent_id_etag_token(agent_id, fingerprint),
        u64::from(oplog_index)
    )
}

/// Same format as [`build_etag_value`] but without the surrounding quotes,
/// used for parsing/comparison.
pub fn agent_id_etag_token(agent_id: &AgentId, fingerprint: AgentFingerprint) -> String {
    format!(
        "{}/{}/{}",
        agent_id.component_id,
        agent_id.agent_name_encoded(),
        fingerprint,
    )
}

/// Returns true when the read-only method's cache policy allows HTTP
/// revalidation (`ETag` + `If-None-Match` 304). [`CachePolicy::NoCache`]
/// explicitly opts out of any HTTP caching, including conditional requests,
/// so we must not emit an `ETag` or short-circuit on `If-None-Match` for it.
pub fn supports_http_revalidation(read_only: &ReadOnlyConfig) -> bool {
    !matches!(read_only.cache_policy, CachePolicy::NoCache(_))
}

/// Build the `Cache-Control` header value for the given read-only config.
///
/// Mapping:
/// - [`CachePolicy::NoCache`] → `no-store`
/// - [`CachePolicy::UntilWrite`] → `<vis>, no-cache`
/// - [`CachePolicy::Ttl(d)`] → `<vis>, max-age=<floor(d, 1s)>`
///
/// Sub-second TTL values floor to `max-age=0`, which causes downstream caches
/// to revalidate via `ETag` on every request. The deploy-time validator emits
/// a warning in that case so the agent author can pick a different TTL.
pub fn build_cache_control_value(read_only: &ReadOnlyConfig) -> String {
    let visibility = CacheVisibility::for_read_only(read_only);
    match &read_only.cache_policy {
        CachePolicy::NoCache(_) => "no-store".to_string(),
        CachePolicy::UntilWrite(_) => format!("{}, no-cache", visibility.as_token()),
        CachePolicy::Ttl(ttl) => {
            let seconds = ttl_seconds_floor(ttl.duration_nanos);
            format!("{}, max-age={}", visibility.as_token(), seconds)
        }
    }
}

/// Floor the TTL value (in nanoseconds) to whole seconds.
pub fn ttl_seconds_floor(duration_nanos: u64) -> u64 {
    duration_nanos / 1_000_000_000
}

/// Parse a header value containing one or more `If-None-Match` ETag tokens,
/// returning the parsed (component-id, agent-name, oplog-index) tuples.
///
/// Unknown / malformed entries are silently skipped. The `*` wildcard does
/// not match agent-specific revalidation and is also ignored.
///
/// We only ever **compare** against a previously-emitted ETag, so a stricter
/// parser would not give us any additional safety.
pub fn parse_if_none_match_entries(header_value: &str) -> Vec<(String, u64)> {
    let mut entries = Vec::new();
    for raw in header_value.split(',') {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "*" {
            continue;
        }
        // Tolerate weak-validator prefix "W/" (we don't issue weak ETags
        // ourselves but a proxy might rewrite our strong ETag into a weak one).
        let trimmed = trimmed.strip_prefix("W/").unwrap_or(trimmed);
        let inner = trimmed
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(trimmed);
        if let Some((token, idx_str)) = inner.rsplit_once(':')
            && let Ok(idx) = idx_str.parse::<u64>()
        {
            entries.push((token.to_string(), idx));
        }
    }
    entries
}

/// Returns true if any entry from `parse_if_none_match_entries` matches the
/// given agent id, agent fingerprint, and oplog index. Both halves of the
/// validator must match: a client-side ETag that was minted for a previous
/// instance of the same `AgentId` (different fingerprint) will not match the
/// current instance, even if the oplog index happens to coincide.
pub fn if_none_match_hits(
    entries: &[(String, u64)],
    agent_id: &AgentId,
    fingerprint: AgentFingerprint,
    current_oplog: OplogIndex,
) -> bool {
    let expected_token = agent_id_etag_token(agent_id, fingerprint);
    let expected_idx = u64::from(current_oplog);
    entries
        .iter()
        .any(|(token, idx)| token == &expected_token && *idx == expected_idx)
}

/// Names of HTTP headers used for read-only HTTP caching.
pub mod headers {
    use http::HeaderName;

    pub const ETAG: HeaderName = http::header::ETAG;
    pub const CACHE_CONTROL: HeaderName = http::header::CACHE_CONTROL;
    pub const IF_NONE_MATCH: HeaderName = http::header::IF_NONE_MATCH;
}

/// Append `header_name` to the `Vary` value already present in `existing`
/// (if any), preserving duplicates / casing of previous entries. Returns the
/// new combined value. Match is case-insensitive, so an existing
/// `Vary: authorization` is preserved as-is when adding `Authorization`.
pub fn merge_vary_with_header(existing: Option<&str>, header_name: &str) -> String {
    let mut tokens: Vec<String> = match existing {
        Some(value) => value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    };
    if !tokens.iter().any(|t| t.eq_ignore_ascii_case(header_name)) {
        tokens.push(header_name.to_string());
    }
    tokens.join(", ")
}

/// Insert a `Vary: <header_name>` header into a header map, merging with any
/// existing `Vary` value. Call repeatedly to add multiple varying headers.
pub fn add_vary_header(
    headers: &mut std::collections::HashMap<HeaderName, String>,
    header_name: &str,
) {
    let existing = headers.get(&header::VARY).cloned();
    let new_value = merge_vary_with_header(existing.as_deref(), header_name);
    headers.insert(header::VARY, new_value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::CachePolicyTtl;
    use golem_common::model::component::ComponentId;
    use test_r::test;
    use uuid::uuid;

    fn agent_id_for_test() -> AgentId {
        AgentId {
            component_id: ComponentId(uuid!("00000000-0000-0000-0000-000000000001")),
            agent_id: "demo".to_string(),
        }
    }

    fn fingerprint_for_test() -> AgentFingerprint {
        AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000aaa"))
    }

    fn public_no_cache() -> ReadOnlyConfig {
        ReadOnlyConfig {
            cache_policy: CachePolicy::UntilWrite(Empty {}),
            uses_principal: false,
        }
    }

    fn private_no_cache() -> ReadOnlyConfig {
        ReadOnlyConfig {
            cache_policy: CachePolicy::UntilWrite(Empty {}),
            uses_principal: true,
        }
    }

    fn ttl_public(nanos: u64) -> ReadOnlyConfig {
        ReadOnlyConfig {
            cache_policy: CachePolicy::Ttl(CachePolicyTtl {
                duration_nanos: nanos,
            }),
            uses_principal: false,
        }
    }

    fn no_store_public() -> ReadOnlyConfig {
        ReadOnlyConfig {
            cache_policy: CachePolicy::NoCache(Empty {}),
            uses_principal: false,
        }
    }

    #[test]
    fn cache_control_no_store_for_no_cache_policy() {
        assert_eq!(build_cache_control_value(&no_store_public()), "no-store");
    }

    #[test]
    fn cache_control_until_write_public_when_principal_unaware() {
        assert_eq!(
            build_cache_control_value(&public_no_cache()),
            "public, no-cache"
        );
    }

    #[test]
    fn cache_control_until_write_private_when_principal_aware() {
        assert_eq!(
            build_cache_control_value(&private_no_cache()),
            "private, no-cache"
        );
    }

    #[test]
    fn cache_control_ttl_floors_to_seconds() {
        assert_eq!(
            build_cache_control_value(&ttl_public(2_500_000_000)),
            "public, max-age=2"
        );
    }

    #[test]
    fn cache_control_sub_second_ttl_renders_as_max_age_zero() {
        assert_eq!(
            build_cache_control_value(&ttl_public(500_000_000)),
            "public, max-age=0"
        );
    }

    #[test]
    fn etag_value_contains_component_agent_id_and_fingerprint() {
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();
        let etag = build_etag_value(&agent_id, fingerprint, OplogIndex::from_u64(42));
        assert!(etag.starts_with('"') && etag.ends_with('"'));
        assert!(etag.contains("00000000-0000-0000-0000-000000000001"));
        assert!(etag.contains(&fingerprint.to_string()));
        assert!(etag.contains(":42\""));
        assert!(etag.contains("/demo/"));
    }

    #[test]
    fn parse_if_none_match_recognises_quoted_entry() {
        let raw = "\"00000000-0000-0000-0000-000000000001/demo/00000000-0000-0000-0000-000000000aaa:42\", \"00000000-0000-0000-0000-000000000001/demo/00000000-0000-0000-0000-000000000aaa:43\"";
        let entries = parse_if_none_match_entries(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            (
                "00000000-0000-0000-0000-000000000001/demo/00000000-0000-0000-0000-000000000aaa"
                    .to_string(),
                42
            )
        );
        assert_eq!(
            entries[1],
            (
                "00000000-0000-0000-0000-000000000001/demo/00000000-0000-0000-0000-000000000aaa"
                    .to_string(),
                43
            )
        );
    }

    #[test]
    fn parse_if_none_match_recognises_weak_etag() {
        let entries = parse_if_none_match_entries(
            "W/\"00000000-0000-0000-0000-000000000001/demo/00000000-0000-0000-0000-000000000aaa:7\"",
        );
        assert_eq!(
            entries,
            vec![(
                "00000000-0000-0000-0000-000000000001/demo/00000000-0000-0000-0000-000000000aaa"
                    .to_string(),
                7
            )]
        );
    }

    #[test]
    fn parse_if_none_match_ignores_wildcard_and_empty() {
        assert!(parse_if_none_match_entries("*").is_empty());
        assert!(parse_if_none_match_entries(",").is_empty());
        assert!(parse_if_none_match_entries("").is_empty());
    }

    #[test]
    fn if_none_match_hits_when_token_and_idx_match() {
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();
        let entries = parse_if_none_match_entries(&build_etag_value(
            &agent_id,
            fingerprint,
            OplogIndex::from_u64(10),
        ));
        assert!(if_none_match_hits(
            &entries,
            &agent_id,
            fingerprint,
            OplogIndex::from_u64(10)
        ));
    }

    #[test]
    fn if_none_match_misses_when_idx_differs() {
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();
        let entries = parse_if_none_match_entries(&build_etag_value(
            &agent_id,
            fingerprint,
            OplogIndex::from_u64(10),
        ));
        assert!(!if_none_match_hits(
            &entries,
            &agent_id,
            fingerprint,
            OplogIndex::from_u64(11)
        ));
    }

    /// A previously-cached ETag minted for a now-deleted instance of the
    /// agent must NOT match the new instance, even if both reached the same
    /// oplog index. This is what the fingerprint half of the validator
    /// guards against.
    #[test]
    fn if_none_match_misses_when_fingerprint_differs() {
        let agent_id = agent_id_for_test();
        let old_fingerprint = AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000aaa"));
        let new_fingerprint = AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000bbb"));
        let oplog = OplogIndex::from_u64(10);

        let stale_etag = build_etag_value(&agent_id, old_fingerprint, oplog);
        let entries = parse_if_none_match_entries(&stale_etag);

        assert!(!if_none_match_hits(
            &entries,
            &agent_id,
            new_fingerprint,
            oplog
        ));
    }

    #[test]
    fn merge_vary_appends_header_when_missing() {
        let merged = merge_vary_with_header(Some("Accept-Encoding"), "Authorization");
        assert_eq!(merged, "Accept-Encoding, Authorization");
    }

    #[test]
    fn merge_vary_keeps_header_unique_case_insensitive() {
        let merged =
            merge_vary_with_header(Some("Accept-Encoding, authorization"), "Authorization");
        assert_eq!(merged, "Accept-Encoding, authorization");
    }

    #[test]
    fn merge_vary_creates_value_when_absent() {
        let merged = merge_vary_with_header(None, "Authorization");
        assert_eq!(merged, "Authorization");
    }

    #[test]
    fn merge_vary_supports_custom_header_names() {
        let merged = merge_vary_with_header(None, "X-Tenant");
        assert_eq!(merged, "X-Tenant");
    }

    #[test]
    fn supports_http_revalidation_is_false_for_no_cache() {
        assert!(!supports_http_revalidation(&no_store_public()));
    }

    #[test]
    fn supports_http_revalidation_is_true_for_until_write() {
        assert!(supports_http_revalidation(&public_no_cache()));
    }

    #[test]
    fn supports_http_revalidation_is_true_for_ttl() {
        assert!(supports_http_revalidation(&ttl_public(1_000_000_000)));
    }

    // ----------------------------------------------------------------
    // Section 14.4 H1..H5: HTTP read-only cache behaviour
    //
    // These scenarios are described end-to-end in
    // `read-only-agent-methods.md` (section 14.4). The full server-to-server
    // integration coverage lives in `integration-tests/tests/custom_api/`
    // (follow-up); the tests below cover the cache-header layer that powers
    // the H1..H5 behaviours and which the executor's `oplog_index` plumbing
    // is consumed by.
    // ----------------------------------------------------------------

    /// H1: `GET` on a read-only method emits both `ETag` and `Cache-Control`.
    #[test]
    fn h1_get_returns_etag_and_cache_control_for_until_write() {
        let read_only = public_no_cache();
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();

        let etag = build_etag_value(&agent_id, fingerprint, OplogIndex::from_u64(42));
        let cache_control = build_cache_control_value(&read_only);

        assert!(etag.starts_with('"') && etag.ends_with('"'));
        assert_eq!(cache_control, "public, no-cache");
        assert!(supports_http_revalidation(&read_only));
    }

    /// H2: an `If-None-Match` whose ETag matches the agent's current oplog
    /// index revalidates with `304 Not Modified` and does not invoke the
    /// executor.
    #[test]
    fn h2_if_none_match_with_current_oplog_hits() {
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();
        let current = OplogIndex::from_u64(99);
        let etag = build_etag_value(&agent_id, fingerprint, current);

        let entries = parse_if_none_match_entries(&etag);

        assert!(if_none_match_hits(
            &entries,
            &agent_id,
            fingerprint,
            current
        ));
    }

    /// H3: after a non-read-only write the agent's oplog index advances, so
    /// the previous ETag no longer matches and the response is no longer a
    /// cache hit. (The read-only cache's epoch bump on writes is what causes
    /// the next invocation to record a strictly greater index in the
    /// `oplog_index` field returned by the executor.)
    #[test]
    fn h3_etag_invalidates_after_write_bumps_oplog() {
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();
        let before_write = OplogIndex::from_u64(10);
        let after_write = OplogIndex::from_u64(11);

        let stale_etag = build_etag_value(&agent_id, fingerprint, before_write);
        let entries = parse_if_none_match_entries(&stale_etag);

        // Old ETag must not match the post-write oplog index.
        assert!(!if_none_match_hits(
            &entries,
            &agent_id,
            fingerprint,
            after_write
        ));
    }

    /// H4: a read-only method whose `uses_principal == false` is cacheable by
    /// shared caches (CDNs) — emit `Cache-Control: public, ...`.
    #[test]
    fn h4_principal_unaware_uses_public_cache_directive() {
        let read_only = public_no_cache();
        let cache_control = build_cache_control_value(&read_only);

        assert!(cache_control.starts_with("public,"));
        assert_eq!(
            CacheVisibility::for_read_only(&read_only),
            CacheVisibility::Public
        );
    }

    /// H4 (counterpart): a read-only method whose `uses_principal == true`
    /// must mark its response `private`, so a shared cache cannot serve one
    /// user's representation for another.
    #[test]
    fn h4_principal_aware_uses_private_cache_directive() {
        let read_only = private_no_cache();
        let cache_control = build_cache_control_value(&read_only);

        assert!(cache_control.starts_with("private,"));
        assert_eq!(
            CacheVisibility::for_read_only(&read_only),
            CacheVisibility::Private
        );
    }

    /// H5: a `CachePolicy::Ttl(d)` method's `Cache-Control` ends in
    /// `max-age=<floor(d, 1s)>`.
    #[test]
    fn h5_ttl_method_returns_max_age_floored_to_seconds() {
        // 2.5s -> floors to max-age=2
        assert_eq!(
            build_cache_control_value(&ttl_public(2_500_000_000)),
            "public, max-age=2"
        );
        // exactly 1s -> max-age=1
        assert_eq!(
            build_cache_control_value(&ttl_public(1_000_000_000)),
            "public, max-age=1"
        );
    }

    /// `CachePolicy::NoCache` must not emit `ETag` and must not be
    /// revalidatable via `If-None-Match`. This is what
    /// `add_read_only_cache_headers` and `try_handle_if_none_match` short
    /// circuit on.
    #[test]
    fn no_cache_policy_opts_out_of_http_revalidation() {
        let read_only = no_store_public();
        assert_eq!(build_cache_control_value(&read_only), "no-store");
        assert!(!supports_http_revalidation(&read_only));
    }

    #[test]
    fn etag_token_is_url_encoded_to_survive_commas_and_quotes() {
        // ParsedAgentId rendering can include commas, parentheses and quotes;
        // the etag token must encode them so the comma-delimited
        // `If-None-Match` parser still round-trips.
        let agent_id = AgentId {
            component_id: ComponentId(uuid!("00000000-0000-0000-0000-000000000001")),
            agent_id: "agent-7([12,13,14])".to_string(),
        };
        let fingerprint = fingerprint_for_test();

        let etag = build_etag_value(&agent_id, fingerprint, OplogIndex::from_u64(7));

        // No raw comma, no nested quote in the rendered token (the encoded
        // agent name is the second '/'-separated component of the token,
        // sitting between the component id and the fingerprint).
        let token_part = etag.trim_matches('"');
        let mut parts = token_part.splitn(3, '/');
        let _component_id = parts.next().unwrap();
        let encoded_name = parts.next().unwrap();
        assert!(
            !encoded_name.contains(','),
            "expected encoded agent name to be comma-free, got {encoded_name}"
        );
        assert!(!encoded_name.contains('"'));

        // Round-trip: If-None-Match → parsed entries → hit.
        let entries = parse_if_none_match_entries(&etag);
        assert!(if_none_match_hits(
            &entries,
            &agent_id,
            fingerprint,
            OplogIndex::from_u64(7)
        ));
    }
}
