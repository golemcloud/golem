use super::*;
use serde_json::Value;
use test_r::test;

fn raw(value: &Value) -> RawHeaders {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|header| {
            if let Some(pair) = header.as_array() {
                (
                    pair[0].as_str().unwrap().as_bytes().to_vec(),
                    pair[1].as_str().unwrap().as_bytes().to_vec(),
                )
            } else {
                let name = header["name"].as_str().unwrap().as_bytes().to_vec();
                let hex = header["value_hex"].as_str().unwrap();
                let value = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect();
                (name, value)
            }
        })
        .collect()
}

fn observed(map: &HeaderMap) -> Vec<(String, Vec<u8>)> {
    map.iter()
        .map(|(n, v)| (n.as_str().to_owned(), v.as_bytes().to_vec()))
        .collect()
}

fn compare_header_entries(
    actual: &[(String, Vec<u8>)],
    expected: &[(String, Vec<u8>)],
) -> Result<(), String> {
    fn grouped(entries: &[(String, Vec<u8>)]) -> Vec<(String, Vec<Vec<u8>>)> {
        let mut names = entries
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
            .into_iter()
            .map(|name| {
                let values = entries
                    .iter()
                    .filter(|(candidate, _)| candidate == &name)
                    .map(|(_, value)| value.clone())
                    .collect();
                (name, values)
            })
            .collect()
    }

    if actual
        .iter()
        .any(|(name, _)| name != &name.to_ascii_lowercase())
        || expected
            .iter()
            .any(|(name, _)| name != &name.to_ascii_lowercase())
    {
        return Err("header names must be canonical lowercase".to_owned());
    }
    if grouped(actual) != grouped(expected) {
        return Err("header names or per-name value order differ".to_owned());
    }
    Ok(())
}

#[test]
fn oversized_guest_header_map_is_rejected_without_panicking() {
    let headers = (0..32_769)
        .map(|index| (format!("x-header-{index}").into_bytes(), vec![b'x']))
        .collect();
    for status in [200, 205] {
        assert_eq!(
            process_response_head("GET", status, &headers).unwrap_err(),
            EnvelopeError::InvalidHeader
        );
    }
}

#[test]
fn actual_envelope_corpus_cases() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let cases: Vec<_> = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["suite"] == "envelope")
        .collect();
    assert_eq!(cases.len(), 15);
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let input = &case["input"];
            let expect = &case["expect"];
            let empty = Value::Array(vec![]);
            let headers = raw(input.get("headers").unwrap_or(&empty));
            match input["action"].as_str().unwrap() {
                "origin" => {
                    let result = derive_origin(
                        input["connection_scheme"].as_str().unwrap(),
                        input["host"].as_str().unwrap(),
                        &headers,
                        input["trusted_ingress"].as_bool().unwrap(),
                    );
                    if expect["status"] == 400 {
                        assert_eq!(result.unwrap_err(), EnvelopeError::InvalidOrigin);
                    } else {
                        let result = result.unwrap();
                        assert_eq!(result.scheme, expect["scheme"]);
                        assert_eq!(result.authority, expect["authority"]);
                    }
                }
                "request" => {
                    let result = process_request_head(
                        input["method"].as_str().unwrap(),
                        input["target"].as_str().unwrap(),
                        &headers,
                        HttpVersion::Http1,
                        input.get("authority").and_then(Value::as_str),
                    );
                    if expect["status"] == 400 {
                        assert_eq!(
                            result.unwrap_err().request_status(),
                            StatusCode::BAD_REQUEST
                        );
                    } else {
                        let result = result.unwrap();
                        assert_eq!(result.method, expect["method"]);
                        assert_eq!(result.path, expect["path"]);
                        assert_eq!(
                            result.query.as_deref(),
                            expect.get("query").and_then(Value::as_str)
                        );
                        assert_expected_headers(&result.headers, expect);
                    }
                }
                "response" => {
                    let result = process_response_head(
                        input["method"].as_str().unwrap(),
                        input["status"].as_u64().unwrap() as u16,
                        &headers,
                    );
                    if expect["status"] == 502 {
                        assert!(result.is_err(), "{id} accepted an invalid response head");
                    } else {
                        let result = result.unwrap();
                        assert_eq!(
                            result.status.as_u16(),
                            expect["status"].as_u64().unwrap() as u16
                        );
                        assert_expected_headers(&result.headers, expect);
                    }
                }
                _ => unreachable!(),
            }
        }));
        if outcome.is_err() {
            panic!("envelope corpus case {id} failed (this test covers head observations only)");
        }
    }
}

fn assert_expected_headers(actual: &HeaderMap, expect: &Value) {
    if let Some(headers) = expect.get("headers") {
        let expected = raw(headers)
            .into_iter()
            .map(|(name, value)| (String::from_utf8(name).unwrap(), value))
            .collect::<Vec<_>>();
        compare_header_entries(&observed(actual), &expected).unwrap();
    }
    if let Some(absent) = expect.get("absent_headers") {
        for name in absent.as_array().unwrap() {
            assert!(!actual.contains_key(name.as_str().unwrap()));
        }
    }
}

#[test]
fn expected_header_comparison_groups_by_name() {
    let expected = vec![
        ("x-a".to_owned(), b"1".to_vec()),
        ("x-a".to_owned(), b"2".to_vec()),
        ("x-b".to_owned(), b"3".to_vec()),
    ];
    let reordered = vec![
        ("x-b".to_owned(), b"3".to_vec()),
        ("x-a".to_owned(), b"1".to_vec()),
        ("x-a".to_owned(), b"2".to_vec()),
    ];
    assert!(compare_header_entries(&reordered, &expected).is_ok());
    let mut reversed = reordered.clone();
    reversed.swap(1, 2);
    assert!(compare_header_entries(&reversed, &expected).is_err());
    let uppercase = vec![("X-A".to_owned(), b"1".to_vec())];
    assert!(compare_header_entries(&uppercase, &uppercase).is_err());
}

#[test]
fn asymmetric_header_and_framing_edges() {
    let headers = vec![
        (b"X-A".to_vec(), b"1".to_vec()),
        (b"x-b".to_vec(), b"z".to_vec()),
        (b"x-a".to_vec(), b"2".to_vec()),
    ];
    let head = process_response_head("GET", 200, &headers).unwrap();
    let grouped = vec![
        ("x-a".into(), b"1".to_vec()),
        ("x-a".into(), b"2".to_vec()),
        ("x-b".into(), b"z".to_vec()),
    ];
    assert!(compare_header_entries(&observed(&head.headers), &grouped).is_ok());
    assert!(
        compare_header_entries(
            &observed(&head.headers),
            &[
                ("x-a".into(), b"2".to_vec()),
                ("x-a".into(), b"1".to_vec()),
                ("x-b".into(), b"z".to_vec())
            ]
        )
        .is_err()
    );
    for byte in [0, 10, 13, 31, 127] {
        assert_eq!(
            process_response_head("GET", 200, &vec![(b"x".to_vec(), vec![byte])]).unwrap_err(),
            EnvelopeError::InvalidHeader
        );
    }
    assert!(process_response_head("GET", 200, &vec![(b"x".to_vec(), vec![9, 0x80, 0xff])]).is_ok());
    for headers in [
        vec![
            (b"content-length".to_vec(), b"1".to_vec()),
            (b"transfer-encoding".to_vec(), b"chunked".to_vec()),
        ],
        vec![
            (b"transfer-encoding".to_vec(), b"chunked".to_vec()),
            (b"content-length".to_vec(), b"1".to_vec()),
        ],
    ] {
        assert_eq!(
            process_request_head("POST", "/", &headers, HttpVersion::Http1, None).unwrap_err(),
            EnvelopeError::InvalidFraming
        );
    }
    for value in [b"1,1".as_slice(), b"1, 2"] {
        assert_eq!(
            process_request_head(
                "POST",
                "/",
                &vec![(b"content-length".to_vec(), value.to_vec())],
                HttpVersion::Http1,
                None,
            )
            .unwrap_err(),
            EnvelopeError::InvalidFraming
        );
    }
}

#[test]
fn response_preserves_optional_whitespace_bytes() {
    let head = process_response_head(
        "GET",
        200,
        &vec![(b"x-verbatim".to_vec(), b"\t value \t".to_vec())],
    )
    .unwrap();

    assert_eq!(
        head.headers.get("x-verbatim").unwrap().as_bytes(),
        b"\t value \t"
    );
}

#[test]
fn framing_promise_survives_hop_header_removal() {
    let headers = vec![
        (b"connection".to_vec(), b"content-length".to_vec()),
        (b"content-length".to_vec(), b"7".to_vec()),
    ];
    let request = process_request_head("POST", "/", &headers, HttpVersion::Http1, None).unwrap();
    let response = process_response_head("GET", 200, &headers).unwrap();
    assert_eq!(request.content_length, Some(7));
    assert_eq!(response.content_length, Some(7));
    assert!(!request.headers.contains_key(CONTENT_LENGTH));
    assert!(!response.headers.contains_key(CONTENT_LENGTH));
    let mut conflicting = headers;
    conflicting.push((b"content-length".to_vec(), b"7".to_vec()));
    assert!(process_request_head("POST", "/", &conflicting, HttpVersion::Http1, None).is_err());
    assert!(process_response_head("GET", 200, &conflicting).is_err());
}

#[test]
fn all_bodyless_responses_defer_commit_and_dispose() {
    for (method, status, length) in [
        ("HEAD", 200, "9"),
        ("GET", 204, "9"),
        ("GET", 205, "9"),
        ("GET", 304, "9"),
        ("GET", 200, "0"),
    ] {
        let head = process_response_head(
            method,
            status,
            &vec![(b"content-length".to_vec(), length.as_bytes().to_vec())],
        )
        .unwrap();
        assert_eq!(head.body_policy, ResponseBodyPolicy::Bodyless);
    }
}

#[test]
fn origin_connection_and_length_boundaries() {
    assert_eq!(
        derive_origin("https", "[2001:DB8::1]:443", &vec![], false)
            .unwrap()
            .authority,
        "[2001:db8::1]:443"
    );
    let bad_forwarded = vec![(b"x-forwarded-proto".to_vec(), b"https".to_vec())];
    assert_eq!(
        derive_origin("http", "internal", &bad_forwarded, true).unwrap_err(),
        EnvelopeError::InvalidOrigin
    );
    assert_eq!(
        derive_origin("http", "internal", &vec![], true).unwrap_err(),
        EnvelopeError::InvalidOrigin
    );
    let trusted = vec![
        (b"x-forwarded-proto".to_vec(), b" HTTPS ".to_vec()),
        (b"x-forwarded-host".to_vec(), b"Example.COM:00443".to_vec()),
    ];
    assert_eq!(
        derive_origin("http", "internal", &trusted, true).unwrap(),
        Origin {
            scheme: "https".to_owned(),
            authority: "example.com:00443".to_owned(),
        }
    );
    for authority in [
        "example.com:",
        "example.com:65536",
        "example.com:nope",
        "2001:db8::1",
        "[2001:db8::1",
        "[2001:db8::zz]",
        "bad_host",
        "999.1.1.1",
    ] {
        assert_eq!(
            derive_origin("https", authority, &vec![], false).unwrap_err(),
            EnvelopeError::InvalidOrigin,
            "authority {authority}"
        );
    }
    let host = vec![(b"host".to_vec(), b"a.test".to_vec())];
    assert_eq!(
        process_request_head("GET", "/?", &host, HttpVersion::Http2, Some("b.test")).unwrap_err(),
        EnvelopeError::InvalidOrigin
    );
    let no_query = process_request_head("GET", "/", &vec![], HttpVersion::Http1, None).unwrap();
    let empty_query = process_request_head("GET", "/?", &vec![], HttpVersion::Http1, None).unwrap();
    assert_eq!(
        (no_query.query, no_query.content_length, no_query.chunked),
        (None, None, false)
    );
    assert_eq!(empty_query.query.as_deref(), Some(""));
    for target in ["*", "example.com:443", "https://example.com/path"] {
        assert_eq!(
            process_request_head("GET", target, &vec![], HttpVersion::Http1, None).unwrap_err(),
            EnvelopeError::InvalidTarget,
            "target {target}"
        );
    }
    let forwarding = vec![
        (b"forwarded".to_vec(), b" for=proxy ".to_vec()),
        (b"x-forwarded-proto".to_vec(), b" https ".to_vec()),
        (b"x-forwarded-host".to_vec(), b" example.com ".to_vec()),
        (b"expect".to_vec(), b" 100-continue\t".to_vec()),
        (b"x-kept".to_vec(), b"\t value \t".to_vec()),
    ];
    let forwarded =
        process_request_head("POST", "/", &forwarding, HttpVersion::Http1, None).unwrap();
    assert_eq!(forwarded.headers.get("x-kept").unwrap(), "value");
    for absent in [
        "forwarded",
        "x-forwarded-proto",
        "x-forwarded-host",
        "expect",
    ] {
        assert!(!forwarded.headers.contains_key(absent));
    }
    for value in ["0", "18446744073709551615"] {
        assert_eq!(
            process_response_head(
                "GET",
                200,
                &vec![(b"content-length".to_vec(), value.as_bytes().to_vec())]
            )
            .unwrap()
            .content_length,
            Some(value.parse().unwrap())
        );
    }
    assert_eq!(
        process_response_head(
            "GET",
            200,
            &vec![(b"content-length".to_vec(), b"18446744073709551616".to_vec())]
        )
        .unwrap_err(),
        EnvelopeError::InvalidFraming
    );
    let all_connection = vec![
        (b"connection".to_vec(), b"x-a".to_vec()),
        (b"connection".to_vec(), b"X-B, keep-alive".to_vec()),
        (b"x-a".to_vec(), b"1".to_vec()),
        (b"x-b".to_vec(), b"2".to_vec()),
    ];
    assert!(
        process_response_head("GET", 200, &all_connection)
            .unwrap()
            .headers
            .is_empty()
    );
    assert_eq!(
        process_response_head(
            "GET",
            200,
            &vec![(b"connection".to_vec(), b"x-a,,x-b".to_vec())]
        )
        .unwrap_err(),
        EnvelopeError::InvalidHeader
    );
    assert_eq!(
        process_response_head(
            "GET",
            304,
            &vec![(b"content-length".to_vec(), b"99".to_vec())]
        )
        .unwrap()
        .content_length,
        Some(99)
    );
}
