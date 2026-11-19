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
//! The worker-service emits ETags for eligible read-only GET/HEAD responses.
//! Cache-Control and credential-related Vary headers use the shared response
//! cache policy, independently of the executor's principal-aware cache key.
//!
//! Conversely, for `If-None-Match` revalidation requests, the worker-service
//! can produce a `304 Not Modified` response without invoking the executor
//! when the supplied `ETag` matches the agent's current oplog index.
//!
//! See [#3392](https://github.com/golemcloud/golem/issues/3392) and
//! [`read-only-agent-methods.md`](../../../../read-only-agent-methods.md) for
//! the full design.

use golem_common::model::agent::{CachePolicy, ReadOnlyConfig};
use golem_common::model::{AgentFingerprint, OplogIndex, OwnedAgentId};

/// Build the value for the `ETag` response header for a read-only invocation.
///
/// The format is `"<environment-id>/<agent-id>/<fingerprint>:<oplog-index>"`, with
/// surrounding quotes so the value is syntactically a strong validator. Every part is a
/// UUID, so it never contains commas, quotes, or other characters that would confuse HTTP
/// parsers or the `If-None-Match` splitter.
///
/// Including the per-instance agent fingerprint guards against the (rare)
/// case of an agent being deleted and a fresh agent being created at the same
/// `AgentId` that happens to reach the same oplog index: without the
/// fingerprint, a previously-cached client-side ETag would incorrectly match
/// the new instance.
pub fn build_etag_value(
    agent_id: &OwnedAgentId,
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
pub fn agent_id_etag_token(agent_id: &OwnedAgentId, fingerprint: AgentFingerprint) -> String {
    format!(
        "{}/{}/{}",
        agent_id.environment_id, agent_id.agent_id, fingerprint,
    )
}

/// Returns true when the read-only method's cache policy allows HTTP
/// revalidation (`ETag` + `If-None-Match` 304). [`CachePolicy::NoCache`]
/// explicitly opts out of any HTTP caching, including conditional requests,
/// so we must not emit an `ETag` or short-circuit on `If-None-Match` for it.
pub fn supports_http_revalidation(read_only: &ReadOnlyConfig) -> bool {
    !matches!(read_only.cache_policy, CachePolicy::NoCache(_))
}

/// Parse a header value containing one or more `If-None-Match` ETag tokens,
/// returning the parsed (agent-token, oplog-index) tuples.
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
    agent_id: &OwnedAgentId,
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
    pub const IF_NONE_MATCH: HeaderName = http::header::IF_NONE_MATCH;
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::agent::CachePolicyTtl;
    use golem_common::model::environment::EnvironmentId;
    use golem_schema::Empty;
    use test_r::test;
    use uuid::uuid;

    fn agent_id_for_test() -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId(uuid!("00000000-0000-0000-0000-000000000001")),
            &AgentId::from_parts(7, 0xde40),
        )
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
    fn etag_value_contains_environment_agent_id_and_fingerprint() {
        let agent_id = agent_id_for_test();
        let fingerprint = fingerprint_for_test();
        let etag = build_etag_value(&agent_id, fingerprint, OplogIndex::from_u64(42));
        assert_eq!(
            etag,
            format!(
                "\"00000000-0000-0000-0000-000000000001/{}/{fingerprint}:42\"",
                agent_id.agent_id
            )
        );
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

    /// `CachePolicy::NoCache` must not emit `ETag` and must not be
    /// revalidatable via `If-None-Match`. This is what
    /// `add_read_only_cache_headers` and `try_handle_if_none_match` short
    /// circuit on.
    #[test]
    fn no_cache_policy_opts_out_of_http_revalidation() {
        let read_only = no_store_public();
        assert!(!supports_http_revalidation(&read_only));
    }
}
