// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::*;
use golem_common::model::agent::CachePolicyTtl;
use golem_schema::Empty;
use test_r::test;

#[test]
fn privacy_floors_override_freshness_on_every_route() {
    for kind in [
        CacheRouteKind::Typed(None),
        CacheRouteKind::File,
        CacheRouteKind::RawHandler,
        CacheRouteKind::OpenApi,
        CacheRouteKind::Protocol,
        CacheRouteKind::Error,
    ] {
        let mut headers = HeaderMap::new();
        headers.append(header::CACHE_CONTROL, "public, max-age=60".parse().unwrap());
        headers.append(header::CACHE_CONTROL, "s-maxage=3600".parse().unwrap());
        headers.insert(
            header::EXPIRES,
            "Thu, 01 Dec 2094 16:00:00 GMT".parse().unwrap(),
        );
        apply_response_cache_headers(kind, &mut headers, true, false, false);
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-cache");
        assert_eq!(headers.get_all(header::CACHE_CONTROL).iter().count(), 1);
        assert!(!headers.contains_key(header::EXPIRES));
    }
    for (authenticated, token, credential) in [
        (false, true, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
    ] {
        let mut headers = HeaderMap::new();
        apply_response_cache_headers(
            CacheRouteKind::Protocol,
            &mut headers,
            authenticated,
            token,
            credential,
        );
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    }
}

#[test]
fn authenticated_no_store_survives_repeated_fields_and_quoted_extensions() {
    for values in [
        vec!["public, max-age=60", "No-StOrE"],
        vec!["No-StOrE", "public, max-age=60"],
        vec![r#"extension="a,\"b,no-cache", no-store"#],
    ] {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::CACHE_CONTROL, value.parse().unwrap());
        }
        apply_response_cache_headers(CacheRouteKind::RawHandler, &mut headers, true, false, false);
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        r#"extension="x,no-store,y""#.parse().unwrap(),
    );
    apply_response_cache_headers(CacheRouteKind::RawHandler, &mut headers, true, false, false);
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-cache");
}

#[test]
fn anonymous_read_only_ttl_is_floored_and_varies_on_credentials() {
    for (nanos, expected) in [
        (90_500_000_000, "public, max-age=90"),
        (999_999_999, "public, max-age=0"),
        (1_000_000_000, "public, max-age=1"),
    ] {
        let policy = CachePolicy::Ttl(CachePolicyTtl {
            duration_nanos: nanos,
        });
        let mut headers = HeaderMap::new();
        apply_response_cache_headers(
            CacheRouteKind::Typed(Some(&policy)),
            &mut headers,
            false,
            false,
            false,
        );
        assert_eq!(headers[header::CACHE_CONTROL], expected);
        assert_eq!(headers[header::VARY], "Authorization, Cookie, Golem-Token");
    }
    for (policy, expected) in [
        (CachePolicy::UntilWrite(Empty {}), "public, no-cache"),
        (CachePolicy::NoCache(Empty {}), "no-store"),
    ] {
        let mut headers = HeaderMap::new();
        apply_response_cache_headers(
            CacheRouteKind::Typed(Some(&policy)),
            &mut headers,
            false,
            false,
            false,
        );
        assert_eq!(headers[header::CACHE_CONTROL], expected);
    }
}

#[test]
fn anonymous_raw_headers_are_preserved_and_vary_is_merged() {
    for cache in [
        "no-store",
        "public, max-age=60",
        r#"public, extension="a,b""#,
    ] {
        let mut headers = HeaderMap::new();
        headers.append(header::CACHE_CONTROL, cache.parse().unwrap());
        headers.append(header::CACHE_CONTROL, "must-revalidate".parse().unwrap());
        headers.append(
            header::VARY,
            "Accept-Language, authorization".parse().unwrap(),
        );
        headers.append(header::VARY, "Origin, ACCEPT-LANGUAGE".parse().unwrap());
        apply_response_cache_headers(
            CacheRouteKind::RawHandler,
            &mut headers,
            false,
            false,
            false,
        );
        assert_eq!(
            headers
                .get_all(header::CACHE_CONTROL)
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>(),
            [cache, "must-revalidate"]
        );
        assert_eq!(
            headers[header::VARY],
            "Accept-Language, authorization, Origin, Cookie, Golem-Token"
        );
    }
    let mut headers = HeaderMap::new();
    headers.append(header::VARY, "Accept-Language".parse().unwrap());
    headers.append(header::VARY, "*".parse().unwrap());
    apply_response_cache_headers(CacheRouteKind::File, &mut headers, false, false, false);
    assert_eq!(headers[header::VARY], "*");
}

#[test]
fn absent_declared_headers_cannot_remove_privacy_or_make_errors_fresh() {
    for authenticated in [false, true] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CACHE_CONTROL,
            "public, max-age=600".parse().unwrap(),
        );
        headers.remove(header::CACHE_CONTROL);
        apply_response_cache_headers(
            CacheRouteKind::Error,
            &mut headers,
            authenticated,
            false,
            false,
        );
        assert_eq!(
            headers[header::CACHE_CONTROL],
            if authenticated {
                "private, no-cache"
            } else {
                "no-cache"
            }
        );
    }
}

#[test]
fn adversarial_quoted_extensions_preserve_only_real_no_store() {
    for extension in [
        r#"extension="no-store""#,
        r#"extension="a,no-store,b""#,
        r#"extension="a\",no-store,b""#,
        r#"extension="a\\", other="no-store""#,
        r#"extension="a\"\\,no-store,b""#,
    ] {
        for real_no_store in [false, true] {
            for separate_field in [false, true] {
                let mut headers = HeaderMap::new();
                let value = if real_no_store && !separate_field {
                    format!("{extension}, No-StOrE")
                } else {
                    extension.to_owned()
                };
                headers.append(header::CACHE_CONTROL, value.parse().unwrap());
                if real_no_store && separate_field {
                    headers.append(header::CACHE_CONTROL, "No-StOrE".parse().unwrap());
                }
                apply_response_cache_headers(
                    CacheRouteKind::RawHandler,
                    &mut headers,
                    true,
                    false,
                    false,
                );
                assert_eq!(
                    headers[header::CACHE_CONTROL],
                    if real_no_store {
                        "private, no-store"
                    } else {
                        "private, no-cache"
                    },
                    "extension={extension}; separate_field={separate_field}"
                );
            }
        }
    }
}

#[test]
fn adversarial_vary_wildcards_and_opaque_values_cannot_enable_reuse() {
    for value in [
        HeaderValue::from_static("Accept-Encoding, * , Origin"),
        HeaderValue::from_bytes(b"\xff").unwrap(),
    ] {
        let mut headers = HeaderMap::new();
        headers.append(header::VARY, "Accept-Language".parse().unwrap());
        headers.append(header::VARY, value);
        headers.append(header::VARY, "Cookie".parse().unwrap());
        apply_response_cache_headers(CacheRouteKind::File, &mut headers, false, false, false);
        assert_eq!(headers[header::VARY], "*");
        assert_eq!(headers.get_all(header::VARY).iter().count(), 1);
    }
}

#[test]
fn adversarial_credential_facts_override_even_zero_ttl_and_opaque_cache_control() {
    for nanos in [0, u64::MAX] {
        let policy = CachePolicy::Ttl(CachePolicyTtl {
            duration_nanos: nanos,
        });
        for authenticated in [false, true] {
            for token in [false, true] {
                for credential in [false, true] {
                    let mut headers = HeaderMap::new();
                    headers.append(header::CACHE_CONTROL, "public, max-age=60".parse().unwrap());
                    headers.append(
                        header::CACHE_CONTROL,
                        HeaderValue::from_bytes(b"\xff").unwrap(),
                    );
                    apply_response_cache_headers(
                        CacheRouteKind::Typed(Some(&policy)),
                        &mut headers,
                        authenticated,
                        token,
                        credential,
                    );
                    if authenticated || token || credential {
                        assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
                    } else {
                        assert_eq!(headers.get_all(header::CACHE_CONTROL).iter().count(), 2);
                    }
                }
            }
        }
    }
}
