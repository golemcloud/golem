// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::super::{RichRequest, RouteExecutionResult};
use chrono::{DateTime, SecondsFormat, Utc};
use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    StreamSessionExpiryPolicy, stream_session_expiry_policy,
};
use http::{HeaderName, HeaderValue};

const MAX_CACHE_SECONDS: u64 = 31_536_000;

pub(super) fn parse_expiry_policy(
    request: &RichRequest,
) -> Result<Option<StreamSessionExpiryPolicy>, ()> {
    parse_expiry_policy_at(request.headers(), Utc::now().timestamp_millis())
}

fn parse_expiry_policy_at(
    headers: &http::HeaderMap,
    now_millis: i64,
) -> Result<Option<StreamSessionExpiryPolicy>, ()> {
    let ttl = single_header(headers, "stream-ttl")?;
    let expires_at = single_header(headers, "stream-expires-at")?;
    match (ttl, expires_at) {
        (Some(_), Some(_)) => Err(()),
        (Some(value), None) => {
            if value.is_empty()
                || (value != "0" && value.starts_with('0'))
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(());
            }
            let ttl_seconds = value.parse::<u64>().map_err(|_| ())?;
            Ok(Some(StreamSessionExpiryPolicy {
                kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(ttl_seconds)),
            }))
        }
        (None, Some(value)) => {
            if !matches!(value.as_bytes().get(10), Some(b'T' | b't')) {
                return Err(());
            }
            let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| ())?;
            let expires_at_millis = parsed.timestamp_millis();
            if expires_at_millis <= now_millis {
                return Err(());
            }
            Ok(Some(StreamSessionExpiryPolicy {
                kind: Some(stream_session_expiry_policy::Kind::ExpiresAtMillis(
                    u64::try_from(expires_at_millis).map_err(|_| ())?,
                )),
            }))
        }
        (None, None) => Ok(None),
    }
}

fn single_header<'a>(headers: &'a http::HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(());
    }
    value
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose()
}

pub(super) fn policies_match(
    actual: &Option<StreamSessionExpiryPolicy>,
    requested: &Option<StreamSessionExpiryPolicy>,
) -> bool {
    normalized_kind(actual) == normalized_kind(requested)
}

fn normalized_kind(
    policy: &Option<StreamSessionExpiryPolicy>,
) -> stream_session_expiry_policy::Kind {
    policy
        .as_ref()
        .and_then(|policy| policy.kind)
        .unwrap_or(stream_session_expiry_policy::Kind::None(Empty {}))
}

pub(super) fn add_expiry_headers(
    response: &mut RouteExecutionResult,
    policy: &Option<StreamSessionExpiryPolicy>,
) {
    match normalized_kind(policy) {
        stream_session_expiry_policy::Kind::None(_) => {}
        stream_session_expiry_policy::Kind::TtlSeconds(ttl_seconds) => {
            response.headers.insert(
                HeaderName::from_static("stream-ttl"),
                HeaderValue::from(ttl_seconds),
            );
        }
        stream_session_expiry_policy::Kind::ExpiresAtMillis(expires_at_millis) => {
            if let Some(value) = canonical_expires_at(expires_at_millis) {
                response.headers.insert(
                    HeaderName::from_static("stream-expires-at"),
                    HeaderValue::from_str(&value).expect("RFC 3339 timestamp is a valid header"),
                );
            }
        }
    }
}

fn canonical_expires_at(expires_at_millis: u64) -> Option<String> {
    i64::try_from(expires_at_millis)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

pub(super) fn cache_control(
    policy: &Option<StreamSessionExpiryPolicy>,
    expiry_deadline_millis: Option<u64>,
    cacheable: bool,
) -> String {
    cache_control_at(
        policy,
        expiry_deadline_millis,
        cacheable,
        Utc::now().timestamp_millis(),
    )
}

fn cache_control_at(
    policy: &Option<StreamSessionExpiryPolicy>,
    expiry_deadline_millis: Option<u64>,
    cacheable: bool,
    now_millis: i64,
) -> String {
    if !cacheable {
        return "no-store".into();
    }
    if matches!(
        normalized_kind(policy),
        stream_session_expiry_policy::Kind::None(_)
    ) {
        return "public, max-age=31536000, immutable".into();
    }
    let Some(remaining_millis) = expiry_deadline_millis.and_then(|deadline| {
        u64::try_from(now_millis)
            .ok()
            .and_then(|now| deadline.checked_sub(now))
    }) else {
        return "no-store".into();
    };
    let seconds = (remaining_millis / 1_000).min(MAX_CACHE_SECONDS);
    if seconds == 0 {
        "no-store".into()
    } else {
        format!("public, max-age={seconds}, must-revalidate")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderMap, HeaderValue};
    use test_r::test;

    #[test]
    fn expiry_parser_is_strict_and_normalizes_absolute_time() {
        let now = 1_700_000_000_000i64;
        let mut headers = HeaderMap::new();
        headers.insert("stream-ttl", HeaderValue::from_static("0"));
        assert!(matches!(
            parse_expiry_policy_at(&headers, now).unwrap().unwrap().kind,
            Some(stream_session_expiry_policy::Kind::TtlSeconds(0))
        ));
        for invalid in ["", "00", "01", "+1", "-1", " 1", "1 "] {
            headers.insert("stream-ttl", HeaderValue::from_str(invalid).unwrap());
            assert!(
                parse_expiry_policy_at(&headers, now).is_err(),
                "{invalid:?}"
            );
        }

        headers.clear();
        headers.insert(
            "stream-expires-at",
            HeaderValue::from_static("2023-11-14T23:13:20.1234+01:00"),
        );
        let parsed = parse_expiry_policy_at(&headers, now).unwrap().unwrap();
        let Some(stream_session_expiry_policy::Kind::ExpiresAtMillis(millis)) = parsed.kind else {
            panic!("absolute expiry expected")
        };
        assert_eq!(
            canonical_expires_at(millis).as_deref(),
            Some("2023-11-14T22:13:20.123Z")
        );

        headers.insert("stream-ttl", HeaderValue::from_static("1"));
        assert!(parse_expiry_policy_at(&headers, now).is_err());
        headers.remove("stream-ttl");
        headers.insert(
            "stream-expires-at",
            HeaderValue::from_static("2023-11-14T22:13:20Z"),
        );
        assert!(parse_expiry_policy_at(&headers, now).is_err());

        headers.clear();
        headers.append("stream-ttl", HeaderValue::from_static("1"));
        headers.append("stream-ttl", HeaderValue::from_static("2"));
        assert!(parse_expiry_policy_at(&headers, now).is_err());
    }

    #[test]
    fn absolute_expiry_rejects_non_rfc3339_space_separator() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "stream-expires-at",
            HeaderValue::from_static("2099-01-01 00:00:00Z"),
        );
        assert!(parse_expiry_policy_at(&headers, 1_700_000_000_000).is_err());
    }

    #[test]
    fn expiring_cache_lifetime_is_floored_and_clamped() {
        let policy = Some(StreamSessionExpiryPolicy {
            kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(60)),
        });
        assert_eq!(
            cache_control_at(&policy, Some(12_999), true, 10_000),
            "public, max-age=2, must-revalidate"
        );
        assert_eq!(
            cache_control_at(&policy, Some(10_999), true, 10_000),
            "no-store"
        );
        assert_eq!(
            cache_control_at(&policy, Some(u64::MAX), true, 0),
            "public, max-age=31536000, must-revalidate"
        );
        assert_eq!(
            cache_control_at(&policy, Some(20_000), false, 10_000),
            "no-store"
        );
        assert_eq!(
            cache_control_at(&None, None, true, 10_000),
            "public, max-age=31536000, immutable"
        );
    }
}
