// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

//! Response cache policy shared by HTTP export handlers.

use golem_common::model::agent::CachePolicy;
use http::{HeaderMap, HeaderValue, header};

/// The response's actual serving path. Read-only policy is supplied only for
/// eligible typed GET/HEAD successes and conditional revalidation responses.
pub enum CacheRouteKind<'a> {
    Typed(Option<&'a CachePolicy>),
    File,
    RawHandler,
    OpenApi,
    Protocol,
    Error,
}

/// Apply privacy floors after code and declared response headers are combined.
/// Authentication describes the verified request, while credential issuance
/// describes the response (including OAuth codes, tokens and session cookies).
pub fn apply_response_cache_headers(
    kind: CacheRouteKind<'_>,
    headers: &mut HeaderMap,
    request_authenticated: bool,
    token_issued: bool,
    credential_issued: bool,
) {
    if !headers.contains_key(header::CACHE_CONTROL) {
        let value = match kind {
            CacheRouteKind::Typed(Some(CachePolicy::NoCache(_))) => Some("no-store".into()),
            CacheRouteKind::Typed(Some(CachePolicy::UntilWrite(_))) | CacheRouteKind::OpenApi => {
                Some("public, no-cache".into())
            }
            CacheRouteKind::Typed(Some(CachePolicy::Ttl(ttl))) => Some(format!(
                "public, max-age={}",
                ttl.duration_nanos / 1_000_000_000
            )),
            CacheRouteKind::Error => Some("no-cache".into()),
            _ => None,
        };
        if let Some(value) = value {
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_str(&value).unwrap(),
            );
        }
    }
    let no_store = token_issued
        || credential_issued
        || headers.get_all(header::CACHE_CONTROL).iter().any(|value| {
            value
                .to_str()
                .map_or(true, |value| has_cache_directive(value, "no-store"))
        });
    if request_authenticated || token_issued || credential_issued {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(if no_store {
                "private, no-store"
            } else {
                "private, no-cache"
            }),
        );
        // A declared expiry cannot add freshness to a private response.
        headers.remove(header::EXPIRES);
    } else {
        add_vary_headers(headers, &["Authorization", "Cookie", "Golem-Token"]);
    }
}

fn has_cache_directive(value: &str, directive: &str) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    value
        .split(|character| {
            if escaped {
                escaped = false;
            } else if quoted && character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = !quoted;
            } else if character == ',' && !quoted {
                return true;
            }
            false
        })
        .any(|value| {
            value
                .split_once('=')
                .map_or(value, |(name, _)| name)
                .trim()
                .eq_ignore_ascii_case(directive)
        })
}

/// Merge all Vary field values, preserving wildcard semantics and comparing
/// header names case-insensitively.
pub fn add_vary_headers(headers: &mut HeaderMap, names: &[&str]) {
    let mut tokens = Vec::<String>::new();
    for value in headers.get_all(header::VARY) {
        let Ok(value) = value.to_str() else {
            // An unparseable field must never accidentally enable cache reuse.
            headers.insert(header::VARY, HeaderValue::from_static("*"));
            return;
        };
        for token in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if token == "*" {
                headers.insert(header::VARY, HeaderValue::from_static("*"));
                return;
            }
            if !tokens.iter().any(|name| name.eq_ignore_ascii_case(token)) {
                tokens.push(token.to_owned());
            }
        }
    }
    for name in names {
        if !tokens.iter().any(|token| token.eq_ignore_ascii_case(name)) {
            tokens.push((*name).to_owned());
        }
    }
    headers.insert(
        header::VARY,
        HeaderValue::from_str(&tokens.join(", ")).unwrap(),
    );
}

#[cfg(test)]
mod tests;
