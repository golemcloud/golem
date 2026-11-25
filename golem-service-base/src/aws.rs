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

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode,
};
use http::{HeaderMap, HeaderValue, request::Parts};
use std::fmt::{Debug, Formatter};
use std::time::SystemTime;

/// Static host-side credentials. This type has no serialization implementation.
#[derive(Clone)]
pub struct AwsCredentials(pub(crate) Credentials);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SigningFailure {
    #[error("invalid awsSigV4 field: {0}")]
    InvalidField(&'static str),
    #[error("cannot sign the AWS request")]
    InvalidRequest,
}

impl AwsCredentials {
    /// Reads an `awsSigV4` record. An error names the field, not its value.
    pub fn from_record(record: &serde_json::Value) -> Result<Self, SigningFailure> {
        let field = |name| {
            record
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty() && HeaderValue::from_str(value).is_ok())
                .ok_or(SigningFailure::InvalidField(name))
        };
        let access_key = field("accessKeyId")?;
        let secret_key = field("secretAccessKey")?;
        let token = record
            .get("sessionToken")
            .filter(|value| !value.is_null())
            .map(|_| field("sessionToken"))
            .transpose()?;
        Ok(Self(Credentials::new(
            access_key,
            secret_key,
            token.map(str::to_owned),
            None,
            "volume",
        )))
    }
}

impl Debug for AwsCredentials {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("AwsCredentials([redacted])")
    }
}

/// Signs a final request head and a payload hash without I/O. Replace the request's headers
/// with the result. The signing names `s3`, `s3-outposts`, and `s3-object-lambda` retain
/// exact path encoding and separators. Other services use path normalization and double encoding.
///
/// Supports HTTP(S) absolute targets and slash-leading origin targets with an explicit Host.
/// Returns `InvalidRequest` for unsupported targets, an empty or ambiguous Host, more than
/// 24,571 input header values or 6,548 names, or a rebuilt path and query above 65,534 bytes.
/// Stale signing fields count toward these limits. The time must be in years 0000 through 9999.
/// Every returned value is sensitive. A final header name that contains a configured credential
/// as an ASCII case-insensitive substring returns `InvalidRequest`.
pub fn sign_request(
    head: &Parts,
    payload_hash: &str,
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    time: SystemTime,
) -> Result<HeaderMap, SigningFailure> {
    // Leave five names below the collision-growth threshold of http's largest header map.
    // The separate value limit also bounds the signer's initial map allocation.
    if head.headers.len() > 24_571 || head.headers.keys().count() > 6_548 {
        return Err(SigningFailure::InvalidRequest);
    }
    let supported_target = match head.uri.scheme_str() {
        Some("http" | "https") => head.uri.host().is_some_and(|host| !host.is_empty()),
        None => head.uri.authority().is_none(),
        _ => false,
    };
    if !supported_target || !head.uri.path().starts_with('/') {
        return Err(SigningFailure::InvalidRequest);
    }
    let mut hosts = head.headers.get_all(http::header::HOST).iter();
    let host = hosts
        .next()
        .map(HeaderValue::to_str)
        .transpose()
        .map_err(|_| SigningFailure::InvalidRequest)?
        .or_else(|| head.uri.authority().map(|authority| authority.as_str()))
        .ok_or(SigningFailure::InvalidRequest)?;
    if hosts.next().is_some() || host.trim().is_empty() || HeaderValue::from_str(host).is_err() {
        return Err(SigningFailure::InvalidRequest);
    }
    // QueryWriter rebuilds the original path and percent-encoded decoded query pairs.
    url::form_urlencoded::parse(head.uri.query().unwrap_or_default().as_bytes())
        .try_fold(head.uri.path().len(), |length, (key, value)| {
            key.bytes()
                .chain(value.bytes())
                .try_fold(length.checked_add(2)?, |length, byte| {
                    length.checked_add(if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                        1
                    } else {
                        3
                    })
                })
        })
        .filter(|length| *length <= 65_534)
        .ok_or(SigningFailure::InvalidRequest)?;
    let timestamp = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok(),
        Err(error) => i128::try_from(error.duration().as_nanos())
            .ok()
            .and_then(i128::checked_neg),
    }
    .ok_or(SigningFailure::InvalidRequest)?;
    let date = time::OffsetDateTime::from_unix_timestamp_nanos(timestamp)
        .map_err(|_| SigningFailure::InvalidRequest)?;
    if !(0..=9999).contains(&date.year()) {
        return Err(SigningFailure::InvalidRequest);
    }
    let mut settings = SigningSettings::default();
    if matches!(service, "s3" | "s3-outposts" | "s3-object-lambda") {
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    }
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    settings
        .excluded_headers
        .get_or_insert_default()
        .extend(["traceparent".into(), "tracestate".into()]);
    let identity = credentials.0.clone().into();
    let params = aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(service)
        .time(time)
        .settings(settings)
        .build()
        .map_err(|_| SigningFailure::InvalidRequest)?
        .into();
    let headers = head
        .headers
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "authorization" | "x-amz-date" | "x-amz-security-token" | "x-amz-content-sha256"
            )
        })
        .map(|(name, value)| value.to_str().map(|value| (name.as_str(), value)))
        .collect::<Result<Box<[_]>, _>>()
        .map_err(|_| SigningFailure::InvalidRequest)?;
    let request = SignableRequest::new(
        head.method.as_str(),
        head.uri.to_string(),
        headers.into_iter(),
        SignableBody::Precomputed(payload_hash.to_owned()),
    )
    .map_err(|_| SigningFailure::InvalidRequest)?;
    // The upstream signer traces its credential identity and request headers.
    let (instructions, _) =
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            aws_sigv4::http_request::sign(request, &params)
        })
        .map_err(|_| SigningFailure::InvalidRequest)?
        .into_parts();
    let mut result = head.headers.clone();
    [
        "authorization",
        "x-amz-date",
        "x-amz-security-token",
        "x-amz-content-sha256",
    ]
    .into_iter()
    .for_each(|name| {
        result.remove(name);
    });
    instructions.headers().try_for_each(|(name, value)| {
        let value = HeaderValue::from_str(value).map_err(|_| SigningFailure::InvalidRequest)?;
        result
            .try_insert(
                http::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| SigningFailure::InvalidRequest)?,
                value,
            )
            .map_err(|_| SigningFailure::InvalidRequest)?;
        Ok(())
    })?;
    if result.keys().any(|name| {
        [
            Some(credentials.0.access_key_id()),
            Some(credentials.0.secret_access_key()),
            credentials.0.session_token(),
        ]
        .into_iter()
        .flatten()
        .any(|credential| {
            name.as_str()
                .as_bytes()
                .windows(credential.len())
                .any(|part| part.eq_ignore_ascii_case(credential.as_bytes()))
        })
    }) {
        return Err(SigningFailure::InvalidRequest);
    }
    result
        .values_mut()
        .for_each(|value| value.set_sensitive(true));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    use test_r::test;

    const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn expected_authorization(
        canonical_request: &str,
        signed_headers: &str,
        region: &str,
        service: &str,
    ) -> Box<str> {
        let signing_key = ["20130524", region, service, "aws4_request"]
            .into_iter()
            .fold(
                b"AWS4volume-secret".to_vec().into_boxed_slice(),
                |key, value| {
                    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
                    mac.update(value.as_bytes());
                    mac.finalize().into_bytes().to_vec().into_boxed_slice()
                },
            );
        let scope = format!("20130524/{region}/{service}/aws4_request").into_boxed_str();
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n20130524T000000Z\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        )
        .into_boxed_str();
        let mut mac = Hmac::<Sha256>::new_from_slice(&signing_key).unwrap();
        mac.update(string_to_sign.as_bytes());
        format!(
            "AWS4-HMAC-SHA256 Credential=volume-access/{scope}, SignedHeaders={signed_headers}, Signature={}",
            hex::encode(mac.finalize().into_bytes())
        )
        .into_boxed_str()
    }

    #[test]
    fn service_specific_paths_have_independent_complete_signatures() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        let (head, _) = http::Request::builder()
            .uri("https://examplebucket.s3.amazonaws.com/a//b/../c%2Fd?z=two%20words&a=%2B&a=1")
            .header("range", "bytes=0-9")
            .body(())
            .unwrap()
            .into_parts();
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1369353600);
        [
            ("lambda", "/a/c%252Fd"),
            ("s3", "/a//b/../c%2Fd"),
            ("s3-outposts", "/a//b/../c%2Fd"),
            ("s3-object-lambda", "/a//b/../c%2Fd"),
            ("s3-custom", "/a/c%252Fd"),
        ]
        .into_iter()
        .for_each(|(service, canonical_path)| {
            let canonical_request = format!(
                "GET\n{canonical_path}\na=%2B&a=1&z=two%20words\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\nx-amz-content-sha256:{EMPTY_HASH}\nx-amz-date:20130524T000000Z\n\nhost;range;x-amz-content-sha256;x-amz-date\n{EMPTY_HASH}"
            ).into_boxed_str();
            let expected = expected_authorization(
                &canonical_request,
                "host;range;x-amz-content-sha256;x-amz-date",
                "us-east-1",
                service,
            );
            let signed = sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", service, time).unwrap();
            assert_eq!(signed["authorization"].to_str().unwrap(), expected.as_ref(), "{service}");
            assert_eq!(head.uri.path(), "/a//b/../c%2Fd");
        });
    }

    #[test]
    fn session_token_and_nondefault_region_have_an_independent_complete_signature() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret", "sessionToken": "volume-token"
        })).unwrap();
        let (head, _) = http::Request::builder()
            .uri("https://examplebucket.s3.amazonaws.com/a//b/../c%2Fd?z=two%20words&a=%2B&a=1")
            .header("range", "bytes=0-9")
            .body(())
            .unwrap()
            .into_parts();
        let canonical_request = concat!(
            "GET\n/a//b/../c%2Fd\na=%2B&a=1&z=two%20words\n",
            "host:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\n",
            "x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
            "x-amz-date:20130524T000000Z\nx-amz-security-token:volume-token\n\n",
            "host;range;x-amz-content-sha256;x-amz-date;x-amz-security-token\n",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let expected = expected_authorization(
            canonical_request,
            "host;range;x-amz-content-sha256;x-amz-date;x-amz-security-token",
            "eu-west-2",
            "s3",
        );
        let signed = sign_request(
            &head,
            EMPTY_HASH,
            &credentials,
            "eu-west-2",
            "s3",
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1369353600),
        )
        .unwrap();
        assert_eq!(signed["authorization"].to_str().unwrap(), expected.as_ref());
        assert_eq!(signed["x-amz-security-token"], "volume-token");
    }

    #[test]
    fn origin_form_requires_an_explicit_host() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        let (mut head, _) = http::Request::builder()
            .uri("/key?empty=&same=1&same=2")
            .body(())
            .unwrap()
            .into_parts();
        assert_eq!(
            sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH
            ),
            Err(SigningFailure::InvalidRequest)
        );
        head.headers
            .insert("host", "example.com:8080".parse().unwrap());
        assert!(
            sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH
            )
            .is_ok()
        );
    }

    #[test]
    fn unsupported_request_target_forms_return_an_opaque_error() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        ["example.com:443", "*", "ftp://example.com/key"]
            .into_iter()
            .for_each(|target| {
                let (head, _) = http::Request::builder()
                    .method("CONNECT")
                    .uri(target)
                    .header("host", "example.com")
                    .body(())
                    .unwrap()
                    .into_parts();
                let error = sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH,
                )
                .unwrap_err();
                assert_eq!(error, SigningFailure::InvalidRequest);
                assert_eq!(format!("{error}"), "cannot sign the AWS request");
            });
    }

    #[test]
    fn explicit_host_must_be_nonempty_and_unambiguous() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        [
            "https://example.com/key",
            "http://example.com:8080/key",
            "/key",
        ]
        .into_iter()
        .for_each(|target| {
            let (mut head, _) = http::Request::builder()
                .uri(target)
                .header("host", "explicit.example.com")
                .body(())
                .unwrap()
                .into_parts();
            assert!(
                sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH
                )
                .is_ok()
            );
            head.headers.insert("host", HeaderValue::from_static(""));
            assert_eq!(
                sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH
                ),
                Err(SigningFailure::InvalidRequest)
            );
            head.headers
                .insert("host", HeaderValue::from_static(" \t "));
            assert_eq!(
                sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH
                ),
                Err(SigningFailure::InvalidRequest)
            );
            head.headers
                .insert("host", HeaderValue::from_static("example.com"));
            head.headers
                .append("host", HeaderValue::from_static("example.com"));
            assert_eq!(
                sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH
                ),
                Err(SigningFailure::InvalidRequest)
            );
        });
    }

    #[test]
    fn header_value_limit_counts_duplicates_and_stale_fields() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret", "sessionToken": "volume-token"
        })).unwrap();
        [
            "x-repeat",
            "authorization",
            "x-amz-date",
            "x-amz-security-token",
            "x-amz-content-sha256",
        ]
        .into_iter()
        .for_each(|name| {
            let (mut head, _) = http::Request::builder()
                .uri("https://example.com/key")
                .body(())
                .unwrap()
                .into_parts();
            (0..24_570).for_each(|_| {
                head.headers
                    .try_append("x-repeat", HeaderValue::from_static("a"))
                    .unwrap();
            });
            head.headers
                .try_append(name, HeaderValue::from_static("a"))
                .unwrap();
            assert_eq!(head.headers.len(), 24_571);
            assert!(
                sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH
                )
                .is_ok()
            );
            head.headers
                .try_append(name, HeaderValue::from_static("a"))
                .unwrap();
            assert_eq!(
                sign_request(
                    &head,
                    EMPTY_HASH,
                    &credentials,
                    "us-east-1",
                    "s3",
                    SystemTime::UNIX_EPOCH
                ),
                Err(SigningFailure::InvalidRequest)
            );
        });
    }

    #[test]
    fn excessive_duplicate_values_return_an_error_before_dependency_allocation() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        let (mut head, _) = http::Request::builder()
            .uri("https://example.com/key")
            .body(())
            .unwrap()
            .into_parts();
        (0..24_577).for_each(|_| {
            head.headers
                .try_append("x-repeat", HeaderValue::from_static("a"))
                .unwrap();
        });
        assert_eq!(
            sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH
            ),
            Err(SigningFailure::InvalidRequest)
        );
    }

    #[test]
    fn distinct_header_name_limit_is_separate_from_the_value_limit() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret", "sessionToken": "volume-token"
        })).unwrap();
        let (mut head, _) = http::Request::builder()
            .uri("https://example.com/key")
            .body(())
            .unwrap()
            .into_parts();
        (0..6_548).for_each(|index| {
            head.headers
                .try_insert(
                    http::header::HeaderName::from_bytes(format!("x-header-{index}").as_bytes())
                        .unwrap(),
                    HeaderValue::from_static("a"),
                )
                .unwrap();
        });
        assert_eq!(head.headers.keys().count(), 6_548);
        assert!(
            sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH
            )
            .is_ok()
        );
        head.headers
            .try_insert("x-one-more", HeaderValue::from_static("a"))
            .unwrap();
        assert_eq!(
            sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH
            ),
            Err(SigningFailure::InvalidRequest)
        );
    }

    #[test]
    fn canonical_query_expansion_returns_an_error_instead_of_panicking() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        let (head, _) = http::Request::builder()
            .uri(format!("https://example.com/?q={}", "!".repeat(22_000)))
            .body(())
            .unwrap()
            .into_parts();
        assert_eq!(
            sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH
            ),
            Err(SigningFailure::InvalidRequest)
        );
    }

    #[test]
    fn canonical_query_limit_counts_decoded_bytes_and_every_yielded_pair() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        [
            ("".into(), true),
            ("=".into(), true),
            ("&=&".into(), true),
            ("&&&".into(), true),
            ("=value&key&same=1&same=2&&".into(), true),
            (format!("q={}a", "!".repeat(21_843)).into_boxed_str(), true),
            (
                format!("q={}aa", "!".repeat(21_843)).into_boxed_str(),
                false,
            ),
            (format!("q={}a", "%FF".repeat(7_281)).into_boxed_str(), true),
            (
                format!("q={}aa", "%FF".repeat(7_281)).into_boxed_str(),
                false,
            ),
            (
                format!("q={}xx&=", "!".repeat(21_842)).into_boxed_str(),
                true,
            ),
            (
                format!("q={}&=", "!".repeat(21_843)).into_boxed_str(),
                false,
            ),
            (
                format!("q={}xx&=&", "!".repeat(21_842)).into_boxed_str(),
                true,
            ),
            (
                format!("q={}xxx&=&", "!".repeat(21_842)).into_boxed_str(),
                false,
            ),
            (
                format!("q={}xx&key", "!".repeat(21_841)).into_boxed_str(),
                true,
            ),
            (
                format!("q={}xxx&key", "!".repeat(21_841)).into_boxed_str(),
                false,
            ),
            (
                format!("q={}x&=v", "!".repeat(21_842)).into_boxed_str(),
                true,
            ),
            (
                format!("q={}xx&=v", "!".repeat(21_842)).into_boxed_str(),
                false,
            ),
            (
                format!("q={}x&q=", "!".repeat(21_842)).into_boxed_str(),
                true,
            ),
            (
                format!("q={}xx&q=", "!".repeat(21_842)).into_boxed_str(),
                false,
            ),
            (
                format!("q={}&&&&", "!".repeat(21_843)).into_boxed_str(),
                true,
            ),
        ]
        .into_iter()
        .for_each(|(query, accepted): (Box<str>, bool)| {
            let (head, _) = http::Request::builder()
                .uri(format!("https://example.com/?{query}"))
                .body(())
                .unwrap()
                .into_parts();
            let original = head.uri.clone();
            let result = sign_request(
                &head,
                EMPTY_HASH,
                &credentials,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH,
            );
            if accepted {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(SigningFailure::InvalidRequest));
            }
            assert_eq!(head.uri, original);
        });
        [("xx", true), ("xxx", false)]
            .into_iter()
            .for_each(|(suffix, accepted)| {
                let (head, _) = http::Request::builder()
                    .uri(format!(
                        "https://example.com/longer/path?q={}{suffix}",
                        "!".repeat(21_839)
                    ))
                    .body(())
                    .unwrap()
                    .into_parts();
                assert_eq!(
                    sign_request(
                        &head,
                        EMPTY_HASH,
                        &credentials,
                        "us-east-1",
                        "s3",
                        SystemTime::UNIX_EPOCH
                    )
                    .is_ok(),
                    accepted
                );
            });
    }

    #[test]
    fn four_digit_signing_years_have_subsecond_boundaries() {
        use std::time::Duration;

        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        let (head, _) = http::Request::builder()
            .uri("https://example.com/key")
            .body(())
            .unwrap()
            .into_parts();
        let first = SystemTime::UNIX_EPOCH.checked_sub(Duration::from_secs(62_167_219_200));
        let after_last = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(253_402_300_800))
            .unwrap();
        let nanosecond = Duration::from_nanos(1);
        std::iter::once(Duration::from_nanos(100))
            .chain(
                SystemTime::UNIX_EPOCH
                    .checked_sub(nanosecond)
                    .filter(|time| {
                        SystemTime::UNIX_EPOCH.duration_since(*time).ok() == Some(nanosecond)
                    })
                    .map(|_| nanosecond),
            )
            .flat_map(|step| {
                [
                    first.map(|time| (time, Some("00000101T000000Z"))),
                    first
                        .and_then(|time| time.checked_sub(step))
                        .map(|time| (time, None)),
                    Some((
                        after_last.checked_sub(step).unwrap(),
                        Some("99991231T235959Z"),
                    )),
                    Some((after_last, None)),
                    Some((
                        SystemTime::UNIX_EPOCH.checked_sub(step).unwrap(),
                        Some("19691231T235959Z"),
                    )),
                    Some((SystemTime::UNIX_EPOCH, Some("19700101T000000Z"))),
                ]
                .into_iter()
                .flatten()
            })
            .for_each(|(time, expected)| {
                let result = sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", "s3", time);
                match expected {
                    Some(date) => assert_eq!(result.unwrap()["x-amz-date"], date),
                    None => assert_eq!(result, Err(SigningFailure::InvalidRequest)),
                }
            });
    }

    #[test]
    fn extreme_representable_system_times_return_an_error() {
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
        }))
        .unwrap();
        let (head, _) = http::Request::builder()
            .uri("https://example.com/key")
            .body(())
            .unwrap()
            .into_parts();
        let duration = std::time::Duration::from_secs(1 << 62);
        [
            Some(
                SystemTime::UNIX_EPOCH
                    .checked_add(std::time::Duration::from_secs(1 << 39))
                    .expect("the test requires a representable far-future SystemTime"),
            ),
            SystemTime::UNIX_EPOCH.checked_add(duration),
            SystemTime::UNIX_EPOCH.checked_sub(duration),
        ]
        .into_iter()
        .flatten()
        .for_each(|time| {
            assert_eq!(
                sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", "s3", time),
                Err(SigningFailure::InvalidRequest)
            );
        });
    }

    #[test]
    fn omitted_and_null_session_tokens_sign_without_a_token() {
        let (head, _) = http::Request::builder()
            .uri("https://examplebucket.s3.amazonaws.com/key")
            .body(())
            .unwrap()
            .into_parts();
        [
            serde_json::json!({"accessKeyId": "access", "secretAccessKey": "secret"}),
            serde_json::json!({"accessKeyId": "access", "secretAccessKey": "secret", "sessionToken": null}),
        ]
        .iter()
        .for_each(|record| {
            let credentials = AwsCredentials::from_record(record).unwrap();
            let signed = sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", "s3", SystemTime::UNIX_EPOCH).unwrap();
            assert!(!signed.contains_key("x-amz-security-token"));
            assert!(!signed["authorization"].to_str().unwrap().contains("x-amz-security-token"));
        });
    }

    #[test]
    fn published_s3_signature() {
        {
            let credentials = AwsCredentials::from_record(&serde_json::json!({
                "accessKeyId": "volume-access", "secretAccessKey": "volume-secret"
            }))
            .unwrap();
            let (mut head, _) = http::Request::builder()
                .uri("https://examplebucket.s3.amazonaws.com/a//b/../c%2Fd?z=two%20words&a=%2B&a=1")
                .header("range", "bytes=0-9")
                .body(())
                .unwrap()
                .into_parts();
            let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1369353600);
            let signed =
                sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", "s3", time).unwrap();
            assert_eq!(signed["x-amz-date"], "20130524T000000Z");
            assert_eq!(signed["x-amz-content-sha256"], EMPTY_HASH);
            assert_eq!(
                signed["authorization"],
                "AWS4-HMAC-SHA256 Credential=volume-access/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f983e6563a5be82265be8f1136d2864f149054a9c6a7319c1f3c39c463f3da85"
            );
            head.headers
                .insert("traceparent", "00-trace-parent-01".parse().unwrap());
            head.headers
                .insert("tracestate", "vendor=state".parse().unwrap());
            let traced =
                sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", "s3", time).unwrap();
            assert_eq!(traced["authorization"], signed["authorization"]);
            assert_eq!(traced["traceparent"], head.headers["traceparent"]);
            assert_eq!(traced["tracestate"], head.headers["tracestate"]);
        }
        // https://docs.aws.amazon.com/AmazonS3/latest/developerguide/sig-v4-header-based-auth.html
        let credentials = AwsCredentials::from_record(&serde_json::json!({
            "accessKeyId": "AKIAIOSFODNN7EXAMPLE",
            "secretAccessKey": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        }))
        .unwrap();
        let (head, _) = http::Request::builder()
            .uri("https://examplebucket.s3.amazonaws.com/test.txt")
            .header("range", "bytes=0-9")
            .body(())
            .unwrap()
            .into_parts();
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1369353600);
        let headers =
            sign_request(&head, EMPTY_HASH, &credentials, "us-east-1", "s3", time).unwrap();
        assert_eq!(headers["x-amz-date"], "20130524T000000Z");
        assert_eq!(headers["x-amz-content-sha256"], EMPTY_HASH);
        assert_eq!(
            headers["authorization"],
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn credentials_and_signing_headers_redact_secrets() {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let record = serde_json::json!({"accessKeyId":"hidden-access", "secretAccessKey":"hidden-secret", "sessionToken":"hidden-token"});
        let credentials = AwsCredentials::from_record(&record).unwrap();
        let (head, _) = http::Request::builder()
            .uri("https://s3.example.com/a")
            .header("x-amz-security-token", "old-token")
            .header("authorization", "old-signature")
            .header("x-amz-date", "old-date")
            .header("x-amz-content-sha256", "old-hash")
            .header("x-unrelated", "kept")
            .header("if-match", "\"hidden-secret\"")
            .header("x-reflected", "innocuous")
            .header("x-reflected", "prefix hidden-token suffix")
            .header("x-reflected", "hidden-access")
            .header("traceparent", "hidden-token")
            .body(())
            .unwrap()
            .into_parts();
        let signed = sign_request(
            &head,
            EMPTY_HASH,
            &credentials,
            "us-east-1",
            "s3",
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        assert_eq!(signed["x-amz-security-token"], "hidden-token");
        assert!(signed["authorization"].is_sensitive());
        assert!(signed["x-amz-security-token"].is_sensitive());
        assert_eq!(signed["x-amz-date"], "19700101T000000Z");
        assert_eq!(signed["x-amz-content-sha256"], EMPTY_HASH);
        assert_eq!(signed["x-unrelated"], "kept");
        assert_eq!(signed["if-match"].as_bytes(), b"\"hidden-secret\"");
        assert!(
            signed
                .get_all("x-reflected")
                .iter()
                .map(HeaderValue::as_bytes)
                .eq([
                    b"innocuous".as_slice(),
                    b"prefix hidden-token suffix".as_slice(),
                    b"hidden-access".as_slice(),
                ])
        );
        assert_eq!(signed["traceparent"].as_bytes(), b"hidden-token");
        assert_ne!(signed["authorization"], "old-signature");
        [
            format!("{credentials:?}"),
            format!("{signed:?}"),
            format!("{credentials:#?}"),
            format!("{signed:#?}"),
        ]
        .map(String::into_boxed_str)
        .iter()
        .for_each(|debug| {
            assert!(!debug.contains("hidden-secret"));
            assert!(!debug.contains("hidden-token"));
            assert!(!debug.contains("hidden-access"));
        });
        assert!(signed.values().all(HeaderValue::is_sensitive));
        assert!(head.headers.values().all(|value| !value.is_sensitive()));
        ["accessKeyId", "secretAccessKey", "sessionToken"]
            .into_iter()
            .for_each(|field| {
                let mut invalid = record.clone();
                invalid[field] = serde_json::json!("hidden-secret\r\nhidden-token");
                let error = AwsCredentials::from_record(&invalid).unwrap_err();
                assert_eq!(error, SigningFailure::InvalidField(field));
                assert!(!format!("{error:?} {error}").contains("hidden-secret"));
            });
        ["accessKeyId", "secretAccessKey", "sessionToken"]
            .into_iter()
            .for_each(|field| {
                [
                    serde_json::Value::Null,
                    serde_json::json!(17),
                    serde_json::json!(false),
                    serde_json::json!(""),
                    serde_json::json!("hidden-secret\0hidden-token"),
                    serde_json::json!("hidden-secret\u{7f}hidden-token"),
                ]
                .into_iter()
                .filter(|value| field != "sessionToken" || !value.is_null())
                .for_each(|value| {
                    let mut invalid = record.clone();
                    invalid[field] = value;
                    let error = AwsCredentials::from_record(&invalid).unwrap_err();
                    assert_eq!(error, SigningFailure::InvalidField(field));
                    let diagnostic = format!("{error:?} {error}").into_boxed_str();
                    assert!(!diagnostic.contains("hidden-secret"));
                    assert!(!diagnostic.contains("hidden-token"));
                });
            });
        ["accessKeyId", "secretAccessKey"]
            .into_iter()
            .for_each(|field| {
                let mut invalid = record.clone();
                invalid.as_object_mut().unwrap().remove(field);
                assert_eq!(
                    AwsCredentials::from_record(&invalid).unwrap_err(),
                    SigningFailure::InvalidField(field)
                );
            });
        let mut without_token = record.clone();
        without_token
            .as_object_mut()
            .unwrap()
            .remove("sessionToken");
        let without_token = AwsCredentials::from_record(&without_token).unwrap();
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let signed = tracing::subscriber::with_default(subscriber, || {
            let signed = sign_request(
                &head,
                EMPTY_HASH,
                &without_token,
                "us-east-1",
                "s3",
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();
            tracing::info!("after direct public signing");
            signed
        });
        assert!(!signed.contains_key("x-amz-security-token"));
        assert!(signed["authorization"].is_sensitive());
        assert_eq!(signed["x-amz-date"], "19700101T000000Z");
        assert_eq!(signed["x-amz-content-sha256"], EMPTY_HASH);
        assert_eq!(signed["x-unrelated"], "kept");
        let bytes = capture.0.lock().unwrap();
        let log = std::str::from_utf8(&bytes).unwrap();
        assert!(log.contains("after direct public signing"));
        [
            "hidden-access",
            "hidden-secret",
            "hidden-token",
            "aws_sigv4",
        ]
        .iter()
        .for_each(|value| assert!(!log.contains(value)));
    }

    #[test]
    fn returned_header_names_reject_credential_substrings_with_an_opaque_error() {
        ["accessKeyId", "secretAccessKey", "sessionToken"]
            .into_iter()
            .for_each(|field| {
                [
                    (Some("x-before-HIDDEN-CREDENTIAL-after"), "HiDdEn-CrEdEnTiAl"),
                    (Some("traceparent"), "RaCePaRe"),
                    (Some("tracestate"), "RaCeStAt"),
                    (None, "AuThOrIzAtIoN"),
                    (None, "AmZ-Da"),
                    (None, "CoNtEnT-ShA"),
                    (None, "SeCuRiTy-ToKeN"),
                ]
                .into_iter()
                .for_each(|(name, credential)| {
                    let mut request = http::Request::builder().uri("https://example.com/key");
                    if let Some(name) = name {
                        request = request.header(name, "harmless");
                    }
                    let (head, _) = request.body(()).unwrap().into_parts();
                    let mut record = serde_json::json!({
                        "accessKeyId": "volume-access", "secretAccessKey": "volume-secret", "sessionToken": "volume-token"
                    });
                    record[field] = serde_json::json!(credential);
                    let credentials = AwsCredentials::from_record(&record).unwrap();
                    let error = sign_request(
                        &head,
                        EMPTY_HASH,
                        &credentials,
                        "us-east-1",
                        "s3",
                        SystemTime::UNIX_EPOCH,
                    )
                    .unwrap_err();
                    assert_eq!(error, SigningFailure::InvalidRequest, "{field}");
                    assert_eq!(error.to_string(), "cannot sign the AWS request");
                    assert_eq!(format!("{error:?}"), "InvalidRequest");
                    assert_eq!(format!("{error:#?}"), "InvalidRequest");

                    record[field] = serde_json::json!(format!("{credential}-noncolliding"));
                    let credentials = AwsCredentials::from_record(&record).unwrap();
                    let signed = sign_request(
                        &head,
                        EMPTY_HASH,
                        &credentials,
                        "us-east-1",
                        "s3",
                        SystemTime::UNIX_EPOCH,
                    )
                    .unwrap();
                    assert!(signed.values().all(HeaderValue::is_sensitive));
                    if let Some(name) = name {
                        assert_eq!(signed[name].as_bytes(), b"harmless");
                    }
                });
            });
    }
}
