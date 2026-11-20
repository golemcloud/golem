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
/// with the result. S3 paths retain their exact encoding and separators.
pub fn sign_request(
    head: &Parts,
    payload_hash: &str,
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    time: SystemTime,
) -> Result<HeaderMap, SigningFailure> {
    let mut settings = SigningSettings::default();
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
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
        let mut value = HeaderValue::from_str(value).map_err(|_| SigningFailure::InvalidRequest)?;
        value.set_sensitive(matches!(name, "authorization" | "x-amz-security-token"));
        result.insert(
            http::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| SigningFailure::InvalidRequest)?,
            value,
        );
        Ok(())
    })?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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
}
