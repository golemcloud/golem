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

use super::*;
use axum::{Router, body::Body};
use futures::{StreamExt, stream};
use http::{Request, Response};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;
use test_r::{test, timeout};
use tokio::sync::{mpsc, oneshot};
use tracing::instrument::WithSubscriber;

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn credentials() -> AwsCredentials {
    AwsCredentials::from_record(&serde_json::json!({
        "accessKeyId": "volume-access", "secretAccessKey": "volume-secret", "sessionToken": "volume-token"
    })).unwrap()
}

fn client(endpoint: &str, encryption: Option<Encryption>, policy: ConnectionPolicy) -> S3Volume {
    client_with_prefix(endpoint, "corpus/", encryption, policy)
}

fn client_with_prefix(
    endpoint: &str,
    prefix: &str,
    encryption: Option<Encryption>,
    policy: ConnectionPolicy,
) -> S3Volume {
    S3Volume::new(
        S3VolumeConfig {
            endpoint: Some(endpoint.into()),
            region: "us-east-1".into(),
            bucket: "test-bucket".into(),
            prefix: prefix.into(),
            encryption,
        },
        credentials(),
        policy,
    )
    .unwrap()
}

fn allowed() -> ConnectionPolicy {
    ConnectionPolicy {
        allow_loopback: true,
        ..ConnectionPolicy::new(Box::new([]))
    }
}

fn reference(etag: &str, version_id: Option<&str>) -> ObjectVersion {
    ObjectVersion {
        etag: etag.into(),
        version_id: version_id.map(Into::into),
    }
}

fn metadata(key: &str, etag: &str, size: u64) -> ObjectMetadata {
    ObjectMetadata {
        key: key.into(),
        size,
        version: reference(etag, Some("version-7")),
    }
}

fn response(status: u16, body: impl Into<Body>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml")
        .header("etag", "\"reference\"")
        .header("x-amz-version-id", "version-7")
        .body(body.into())
        .unwrap()
}

fn listing_response(objects: &[(&str, u64)], continuation: Option<&str>) -> Response<Body> {
    let contents = objects.iter().map(|(key, size)| {
        let key = key.bytes().map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else { format!("%{byte:02X}") }
        }).collect::<String>();
        format!("<Contents><Key>{key}</Key><Size>{size}</Size><ETag>&quot;listed-A&quot;</ETag></Contents>")
    }).collect::<String>().into_boxed_str();
    let token = continuation
        .map(|token| {
            format!("<NextContinuationToken>{token}</NextContinuationToken>").into_boxed_str()
        })
        .unwrap_or_default();
    response(
        200,
        format!(
            "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>{}</IsTruncated>{token}{contents}</ListBucketResult>",
            continuation.is_some()
        ),
    )
}

fn metadata_response(size: u64, etag: &str, version_id: &str) -> Response<Body> {
    Response::builder()
        .status(200)
        .header("content-length", size)
        .header("etag", etag)
        .header("x-amz-version-id", version_id)
        .body(Body::empty())
        .unwrap()
}

fn assert_opaque_debug(value: &impl Debug) {
    [
        format!("{value:?}").into_boxed_str(),
        format!("{value:#?}").into_boxed_str(),
    ]
    .iter()
    .for_each(|debug| {
        assert!(
            !debug.contains("volume-secret") && !debug.contains("volume-token"),
            "volume Debug disclosed a configured credential"
        );
    });
}

async fn server(
    responses: Vec<Response<Body>>,
) -> (
    Box<str>,
    mpsc::UnboundedReceiver<Request<Bytes>>,
    tokio::task::JoinHandle<()>,
) {
    let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
    let (sent, received) = mpsc::unbounded_channel();
    let app = Router::new().fallback(move |request: Request<Body>| {
        let responses = responses.clone();
        let sent = sent.clone();
        async move {
            let (head, body) = request.into_parts();
            let body = axum::body::to_bytes(body, 16 * 1024 * 1024).await.unwrap();
            sent.send(Request::from_parts(head, body)).unwrap();
            responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| response(500, "<Error/>"))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap()).into_boxed_str();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, received, task)
}

#[derive(Debug)]
struct EofRequestGate {
    request: Request<Bytes>,
    observe: oneshot::Sender<()>,
    observed: oneshot::Receiver<u16>,
    respond: oneshot::Sender<()>,
}

impl EofRequestGate {
    async fn observe(self, status: u16) -> oneshot::Sender<()> {
        self.observe.send(()).unwrap();
        assert_eq!(self.observed.await.unwrap(), status);
        self.respond
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EofPhase {
    Original,
    Replaced,
    Deleted,
}

#[derive(Clone, Copy, Debug)]
enum EofEvent {
    Get,
    Head,
    Replace,
    Delete,
}

fn eof_transition(
    phase: EofPhase,
    event: EofEvent,
    version_id: Option<&str>,
    original: &ObjectMetadata,
    replacement: &ObjectMetadata,
    denied_head: bool,
) -> (EofPhase, Response<Body>) {
    match event {
        EofEvent::Get | EofEvent::Head => {
            let current = match phase {
                EofPhase::Original => Some(original),
                EofPhase::Replaced => Some(replacement),
                EofPhase::Deleted => None,
            };
            let selected =
                if version_id == original.version.version_id.as_deref() && version_id.is_some() {
                    Some(original)
                } else {
                    current.filter(|object| {
                        version_id.is_none() || version_id == object.version.version_id.as_deref()
                    })
                };
            let result = if matches!(event, EofEvent::Head) && denied_head {
                response(403, "")
            } else {
                match selected {
                    None => response(404, "<Error><Code>NoSuchKey</Code></Error>"),
                    Some(object) if object.version.etag != original.version.etag => {
                        response(412, "<Error><Code>PreconditionFailed</Code></Error>")
                    }
                    Some(_) if matches!(event, EofEvent::Get) => {
                        response(416, "<Error><Code>InvalidRange</Code></Error>")
                    }
                    Some(object) => metadata_response(
                        object.size,
                        &object.version.etag,
                        object.version.version_id.as_deref().unwrap_or("null"),
                    ),
                }
            };
            (phase, result)
        }
        EofEvent::Replace => {
            assert_eq!(phase, EofPhase::Original);
            (
                EofPhase::Replaced,
                metadata_response(
                    0,
                    &replacement.version.etag,
                    replacement.version.version_id.as_deref().unwrap_or("null"),
                ),
            )
        }
        EofEvent::Delete => {
            assert_eq!(phase, EofPhase::Original);
            (
                EofPhase::Deleted,
                Response::builder()
                    .status(204)
                    .header(
                        "x-amz-version-id",
                        if original.version.version_id.is_some() {
                            "delete-marker"
                        } else {
                            "null"
                        },
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
        }
    }
}

async fn gated_eof_store(
    versioned: bool,
    denied_head: bool,
) -> (
    Box<str>,
    mpsc::UnboundedReceiver<EofRequestGate>,
    tokio::task::JoinHandle<()>,
) {
    let original = ObjectMetadata {
        key: "key".into(),
        size: 4,
        version: reference("\"eof-A\"", versioned.then_some("version-A")),
    };
    let replacement = ObjectMetadata {
        key: "key".into(),
        size: 4,
        version: reference("\"eof-B\"", versioned.then_some("version-B")),
    };
    let phase = Arc::new(Mutex::new(EofPhase::Original));
    let (sent, received) = mpsc::unbounded_channel();
    let app = Router::new().fallback(move |request: Request<Body>| {
        let phase = phase.clone();
        let original = original.clone();
        let replacement = replacement.clone();
        let sent = sent.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = axum::body::to_bytes(body, 1024).await.unwrap();
            let request = Request::from_parts(parts, body);
            assert_eq!(request.uri().path(), "/test-bucket/corpus/key");
            assert_eq!(request.headers()["if-match"], "\"eof-A\"");
            let (observe, observation) = oneshot::channel();
            let (observed, status) = oneshot::channel();
            let (respond, response_allowed) = oneshot::channel();
            let method = request.method().clone();
            let event = match method.as_str() {
                "GET" => EofEvent::Get,
                "HEAD" => EofEvent::Head,
                "PUT" => EofEvent::Replace,
                "DELETE" => EofEvent::Delete,
                _ => panic!("unexpected object request: {method}"),
            };
            let version_id = request.uri().query().and_then(|query| {
                query
                    .split('&')
                    .find_map(|parameter| parameter.strip_prefix("versionId="))
                    .map(Box::<str>::from)
            });
            if method == "GET" || method == "HEAD" {
                assert_eq!(version_id.as_deref(), versioned.then_some("version-A"));
                if method == "GET" {
                    assert_eq!(request.headers()["range"], "bytes=4-9");
                } else {
                    assert!(!request.headers().contains_key("range"));
                }
            } else {
                assert!(version_id.is_none());
                assert_eq!(
                    request.body().as_ref(),
                    if method == "PUT" {
                        b"new!".as_slice()
                    } else {
                        b"".as_slice()
                    }
                );
            }
            sent.send(EofRequestGate {
                request,
                observe,
                observed: status,
                respond,
            })
            .unwrap();
            observation.await.unwrap();
            let result = {
                // Each request observes or changes the object once, before its reply gate opens.
                let mut phase = phase.lock().unwrap();
                let (next, result) = eof_transition(
                    *phase,
                    event,
                    version_id.as_deref(),
                    &original,
                    &replacement,
                    denied_head,
                );
                *phase = next;
                result
            };
            observed.send(result.status().as_u16()).unwrap();
            let _ = response_allowed.await;
            result
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap()).into_boxed_str();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, received, task)
}

#[test]
fn construction_rejects_invalid_configuration_without_io() {
    let config = || S3VolumeConfig {
        endpoint: Some("http://127.0.0.1:1".into()),
        region: "us-east-1".into(),
        bucket: "test-bucket".into(),
        prefix: "corpus/".into(),
        encryption: None,
    };
    ["bucket", "region", "prefix"]
        .into_iter()
        .for_each(|field| {
            let mut invalid = config();
            match field {
                "bucket" => invalid.bucket = "".into(),
                "region" => invalid.region = "".into(),
                "prefix" => invalid.prefix = "corpus".into(),
                _ => unreachable!(),
            }
            assert_eq!(
                S3Volume::new(invalid, credentials(), ConnectionPolicy::new(Box::new([])))
                    .unwrap_err(),
                VolumeError::InvalidRequest
            );
        });
    ["/absolute/", "./", "../", "corpus/./", "corpus/../"]
        .into_iter()
        .for_each(|prefix| {
            let mut invalid = config();
            invalid.prefix = prefix.into();
            assert_eq!(
                S3Volume::new(invalid, credentials(), ConnectionPolicy::new(Box::new([])))
                    .unwrap_err(),
                VolumeError::InvalidRequest
            );
        });
    [
        "not a uri[",
        "ftp://volume.invalid/",
        "/relative",
        "http:///missing",
        "http://user@volume.invalid/",
        "https://volume.invalid/?query=1",
    ]
    .into_iter()
    .for_each(|endpoint| {
        let mut invalid = config();
        invalid.endpoint = Some(endpoint.into());
        assert_eq!(
            S3Volume::new(invalid, credentials(), ConnectionPolicy::new(Box::new([]))).unwrap_err(),
            VolumeError::InvalidRequest,
            "{endpoint}"
        );
    });
    ["/relative", "ftp://protected.invalid/"]
        .into_iter()
        .for_each(|endpoint| {
            assert_eq!(
                S3Volume::new(
                    config(),
                    credentials(),
                    ConnectionPolicy::new([endpoint.parse().unwrap()].into())
                )
                .unwrap_err(),
                VolumeError::InvalidRequest
            );
        });
    ["", "corpus/"].into_iter().for_each(|prefix| {
        [
            None,
            Some("http://127.0.0.1:1"),
            Some("http://volume.invalid/"),
            Some("https://volume.invalid/"),
            Some("https://volume.invalid:9443/"),
            Some("http://[::1]:9000/"),
        ]
        .into_iter()
        .for_each(|endpoint| {
            let mut valid = config();
            valid.prefix = prefix.into();
            valid.endpoint = endpoint.map(Into::into);
            assert!(
                S3Volume::new(valid, credentials(), ConnectionPolicy::new(Box::new([]))).is_ok()
            );
        });
    });
    [
        "http://protected.invalid/",
        "https://protected.invalid/",
        "http://protected.invalid:9000/",
        "https://[::1]:9443/",
    ]
    .into_iter()
    .for_each(|endpoint| {
        assert!(
            S3Volume::new(
                config(),
                credentials(),
                ConnectionPolicy::new([endpoint.parse().unwrap()].into())
            )
            .is_ok()
        );
    });
}

#[test]
#[timeout("30s")]
async fn empty_bucket_root_is_missing_after_local_reference_validation() {
    let (endpoint, mut requests, server) = server(vec![]).await;
    let volume = client_with_prefix(&endpoint, "", None, allowed());
    assert_eq!(
        volume.stat("", None).await.unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .read("", &reference("\"reference\"", None), None)
            .await
            .unwrap_err(),
        VolumeError::NotFound
    );
    assert_eq!(
        volume
            .read("", &reference("bad-etag", None), None)
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert_eq!(
        volume
            .read(
                "",
                &reference("\"reference\"", None),
                Some(RangeInclusive::new(4, 3))
            )
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn full_key_byte_limits_include_the_configured_prefix_and_probe_reservation() {
    stream::iter([
        ("tenant/data/", "a".repeat(1012), "a".repeat(1005)),
        ("tenant/data/", "é".repeat(506), format!("{}x", "é".repeat(502))),
        ("tenant/é/", "a".repeat(1014), "a".repeat(1007)),
    ]).then(|(prefix, file, probe)| async move {
        stream::iter([false, true]).then(|is_probe| {
            let file = &file;
            let probe = &probe;
            async move {
                stream::iter([false, true]).then(|overlong| {
                    let name = if is_probe { probe } else { file };
                    let name = format!("{name}{}", if overlong { "x" } else { "" });
                    async move {
                        stream::iter(["stat", "read", "put", "delete", "start", "part", "complete", "abort"])
                            .then(|operation| {
                                let name = &name;
                                async move {
                                    let result = match operation {
                                        "stat" => metadata_response(3, "\"reference\"", "version-7"),
                                        "read" => response(200, "abc"),
                                        "start" => response(200, "<InitiateMultipartUploadResult><UploadId>upload</UploadId></InitiateMultipartUploadResult>"),
                                        "complete" => response(200, "<CompleteMultipartUploadResult><ETag>&quot;reference&quot;</ETag></CompleteMultipartUploadResult>"),
                                        "delete" | "abort" => response(204, ""),
                                        _ => response(200, ""),
                                    };
                                    let (endpoint, mut requests, server) = server(vec![result]).await;
                                    let volume = client_with_prefix(&endpoint, prefix, None, allowed());
                                    let target = if is_probe { WriteTarget::Probe(name) } else { WriteTarget::File(name) };
                                    let relative = if is_probe { format!(".golem/{name}") } else { name.to_string() };
                                    let parts = [UploadedPart { number: 1, etag: "\"part\"".into(), size: 3 }];
                                    let actual = match operation {
                                        "stat" => volume.stat(&relative, None).await.map(|_| ()),
                                        "read" => volume.read(&relative, &reference("\"reference\"", Some("version-7")), None).await.map(|_| ()),
                                        "put" => volume.put(target, WriteCondition::Absent, Bytes::from_static(b"abc")).await.map(|_| ()),
                                        "delete" => volume.delete(target, &metadata(&relative, "\"reference\"", 3)).await.map(|_| ()),
                                        "start" => volume.start_multipart(target).await.map(|_| ()),
                                        "part" => volume.upload_part(target, "upload", 1, Bytes::from_static(b"abc")).await.map(|_| ()),
                                        "complete" => volume.complete_multipart(target, "upload", &parts, WriteCondition::Absent).await.map(|_| ()),
                                        "abort" => volume.abort_multipart(target, "upload").await,
                                        _ => unreachable!(),
                                    };
                                    assert_eq!(actual, if overlong { Err(VolumeError::InvalidRequest) } else { Ok(()) }, "{prefix}, probe={is_probe}, overlong={overlong}, {operation}");
                                    if !overlong { assert!(requests.try_recv().is_ok()); }
                                    assert!(requests.try_recv().is_err());
                                    server.abort();
                                }
                            }).collect::<()>().await;
                    }
                }).collect::<()>().await;
            }
        }).collect::<()>().await;
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn encryption_and_preconditions_are_signed_on_writes() {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    stream::iter([
        (None, None, None),
        (Some(Encryption::S3), Some("AES256"), None),
        (Some(Encryption::Kms { key_id: "kms-key".into() }), Some("aws:kms"), Some("kms-key")),
    ]).then(|(encryption, algorithm, kms)| async move {
        let (endpoint, mut requests, server) = server(vec![response(200, ""), response(200,
            "<InitiateMultipartUploadResult><UploadId>upload-1</UploadId></InitiateMultipartUploadResult>"), response(200, "")]).await;
        let volume = S3Volume::new(
            S3VolumeConfig {
                endpoint: Some(endpoint.clone()),
                region: "eu-west-2".into(),
                bucket: "test-bucket".into(),
                prefix: "corpus/".into(),
                encryption,
            },
            credentials(),
            allowed(),
        ).unwrap();
        let result = volume.put(WriteTarget::File("report +%.txt"), WriteCondition::Absent, Bytes::from_static(b"content")).await.unwrap();
        assert_eq!(result, ObjectVersion { etag: "\"reference\"".into(), version_id: Some("version-7".into()) });
        let request = requests.recv().await.unwrap();
        assert_eq!(request.uri().path(), "/test-bucket/corpus/report%20%2B%25.txt");
        assert_eq!(request.body(), b"content".as_slice());
        assert_eq!(request.headers()["if-none-match"], "*");
        assert!(request.headers().get("if-match").is_none());
        assert_eq!(request.headers().get("x-amz-server-side-encryption").map(|v| v.to_str().unwrap()), algorithm);
        assert_eq!(request.headers().get("x-amz-server-side-encryption-aws-kms-key-id").map(|v| v.to_str().unwrap()), kms);
        assert_eq!(request.headers()["x-amz-security-token"], "volume-token");
        assert_eq!(request.headers()["x-amz-content-sha256"], "ed7002b439e9ac845f22357d822bac1444730fbdb6016d3ec9432297b9ec9f73");
        ["host", "if-none-match", "x-amz-date", "x-amz-content-sha256", "x-amz-security-token"]
            .into_iter().for_each(|name| assert!(request.headers().contains_key(name)));
        let body_hash = hex::encode(Sha256::digest(request.body())).into_boxed_str();
        assert_eq!(request.headers()["x-amz-content-sha256"], body_hash.as_ref());
        // Compute the complete signed set from the wire, not the signer's declared SignedHeaders.
        // The SDK adds its invocation and attempt telemetry after signing, before transmit.
        let mut names = request.headers().keys().map(|name| name.as_str()).filter(|name| !matches!(
            *name,
            "authorization" | "user-agent" | "x-amzn-trace-id" | "traceparent" | "tracestate"
                | "amz-sdk-invocation-id" | "amz-sdk-request"
                | "connection" | "keep-alive" | "transfer-encoding" | "te" | "trailer"
                | "upgrade" | "proxy-authorization" | "proxy-authenticate"
        )).collect::<Vec<_>>();
        names.sort_unstable();
        let names = names.into_boxed_slice();
        let expected_names = [
            "content-length", "content-type", "host", "if-none-match", "x-amz-content-sha256",
            "x-amz-date", "x-amz-security-token",
        ].into_iter()
            .chain(algorithm.map(|_| "x-amz-server-side-encryption"))
            .chain(kms.map(|_| "x-amz-server-side-encryption-aws-kms-key-id"))
            .chain(["x-amz-user-agent"])
            .collect::<Box<[_]>>();
        assert_eq!(names, expected_names);
        let signed_headers = names.join(";").into_boxed_str();
        let canonical_headers = names.iter().map(|name| {
            let values = request.headers().get_all(*name).iter().map(|value| {
                value.to_str().unwrap().split_ascii_whitespace().collect::<Box<[_]>>().join(" ").into_boxed_str()
            }).collect::<Box<[_]>>().join(",").into_boxed_str();
            format!("{name}:{values}\n")
        }).collect::<String>().into_boxed_str();
        let encode = |value: &str| value.bytes().map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => char::from(byte).to_string(),
            _ => format!("%{byte:02X}"),
        }).collect::<String>().into_boxed_str();
        let mut query = url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
            .map(|(key, value)| (encode(&key), encode(&value))).collect::<Vec<_>>();
        query.sort_unstable();
        let query = query.into_boxed_slice();
        let canonical_query = query.iter().map(|(key, value)| format!("{key}={value}").into_boxed_str()).collect::<Box<[_]>>().join("&").into_boxed_str();
        let canonical_request = format!("{}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{body_hash}", request.method(), request.uri().path()).into_boxed_str();
        let date_time = request.headers()["x-amz-date"].to_str().unwrap();
        assert_eq!(date_time.len(), 16);
        assert_eq!(&date_time[8..9], "T");
        assert_eq!(&date_time[15..], "Z");
        let date = &date_time[..8];
        let expected_scope = format!("{date}/eu-west-2/s3/aws4_request").into_boxed_str();
        let signing_key = [date, "eu-west-2", "s3", "aws4_request"].into_iter().fold(
            b"AWS4volume-secret".to_vec().into_boxed_slice(),
            |key, value| {
                let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
                mac.update(value.as_bytes());
                mac.finalize().into_bytes().to_vec().into_boxed_slice()
            },
        );
        let string_to_sign = format!("AWS4-HMAC-SHA256\n{date_time}\n{expected_scope}\n{}", hex::encode(Sha256::digest(canonical_request.as_bytes()))).into_boxed_str();
        let mut mac = Hmac::<Sha256>::new_from_slice(&signing_key).unwrap();
        mac.update(string_to_sign.as_bytes());
        let expected_authorization = format!("AWS4-HMAC-SHA256 Credential=volume-access/{expected_scope}, SignedHeaders={signed_headers}, Signature={}", hex::encode(mac.finalize().into_bytes())).into_boxed_str();
        assert_eq!(request.headers()["authorization"], expected_authorization.as_ref());
        assert_eq!(volume.start_multipart(WriteTarget::File("large")).await.unwrap().payload().as_ref(), "upload-1");
        let request = requests.recv().await.unwrap();
        assert_eq!(request.headers().get("x-amz-server-side-encryption").map(|v| v.to_str().unwrap()), algorithm);
        assert_eq!(request.headers().get("x-amz-server-side-encryption-aws-kms-key-id").map(|v| v.to_str().unwrap()), kms);
        let authorization = request.headers()["authorization"].to_str().unwrap();
        if algorithm.is_some() { assert!(authorization.contains("x-amz-server-side-encryption")); }
        if kms.is_some() { assert!(authorization.contains("x-amz-server-side-encryption-aws-kms-key-id")); }
        let part = volume.upload_part(WriteTarget::File("large"), "upload-1", 1, Bytes::from_static(b"part")).await.unwrap();
        assert_eq!(part.number, 1);
        assert_eq!(part.size, 4);
        let request = requests.try_recv().unwrap();
        assert_eq!(request.body(), b"part".as_slice());
        assert_eq!(request.headers()["x-amz-content-sha256"], "37a680133bd09342f934afb8dd2c7d9e1b624da5f35e3a38adb103e37c055ed1");
        assert!(requests.try_recv().is_err());
        server.abort();
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn conditional_failures_redact_errors_and_never_retry() {
    use VolumeError::{Conflict, Denied, Exists, NotFound, RequestFailed, Unsupported};

    stream::iter([
        (401, None, [Denied, Denied, Denied]),
        (403, None, [Denied, Denied, Denied]),
        (404, None, [NotFound, Conflict, NotFound]),
        (405, None, [Unsupported, Unsupported, Unsupported]),
        (409, None, [Conflict, Conflict, Conflict]),
        (412, None, [Exists, Conflict, Conflict]),
        (501, None, [Unsupported, Unsupported, Unsupported]),
        (503, None, [RequestFailed, RequestFailed, RequestFailed]),
        (404, Some(""), [NotFound, Conflict, NotFound]),
        (404, Some("Unknown"), [NotFound, NotFound, NotFound]),
        (503, Some("NoSuchKey"), [NotFound, Conflict, NotFound]),
        (503, Some("NotFound"), [NotFound, Conflict, NotFound]),
        (412, Some("NoSuchBucket"), [NotFound, NotFound, NotFound]),
        (404, Some("NoSuchUpload"), [NotFound, NotFound, NotFound]),
        (
            503,
            Some("PreconditionFailed"),
            [Exists, Conflict, Conflict],
        ),
        (
            503,
            Some("ConditionalRequestConflict"),
            [Conflict, Conflict, Conflict],
        ),
        (503, Some("AccessDenied"), [Denied, Denied, Denied]),
        (400, Some("ExpiredToken"), [Denied, Denied, Denied]),
        (400, Some("InvalidToken"), [Denied, Denied, Denied]),
        (400, Some("TokenRefreshRequired"), [Denied, Denied, Denied]),
        (412, Some("ExpiredToken"), [Denied, Denied, Denied]),
        (409, Some("InvalidToken"), [Denied, Denied, Denied]),
        (
            400,
            Some("PreconditionFailed"),
            [Exists, Conflict, Conflict],
        ),
        (
            400,
            Some("OtherTokenError"),
            [RequestFailed, RequestFailed, RequestFailed],
        ),
        (
            503,
            Some("NotImplemented"),
            [Unsupported, Unsupported, Unsupported],
        ),
    ])
    .then(|(status, code, expected)| async move {
        let body = match code {
            None => "<Error/>".into(),
            Some("") => "".into(),
            Some(code) => format!(
                "<Error><Code>{code}</Code><Message>volume-secret volume-token</Message></Error>"
            )
            .into_boxed_str(),
        };
        let (endpoint, requests, server) =
            server((0..3).map(|_| response(status, body.to_string())).collect()).await;
        let volume = client(&endpoint, None, allowed());
        let mut requests = stream::iter(expected.into_iter().enumerate())
            .fold(requests, |mut requests, (operation, expected)| {
                let volume = &volume;
                async move {
                    let result = match operation {
                        0 => volume
                            .put(
                                WriteTarget::File("key"),
                                WriteCondition::Absent,
                                Bytes::new(),
                            )
                            .await
                            .map(|_| ()),
                        1 => volume
                            .put(
                                WriteTarget::File("key"),
                                WriteCondition::Matches("\"old\""),
                                Bytes::new(),
                            )
                            .await
                            .map(|_| ()),
                        2 => volume
                            .read("key", &reference("\"reference\"", Some("version-7")), None)
                            .await
                            .map(|_| ()),
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        result,
                        Err(expected),
                        "status={status}, code={code:?}, operation={operation}"
                    );
                    let request = requests.try_recv().unwrap();
                    assert_eq!(request.method(), if operation == 2 { "GET" } else { "PUT" });
                    assert!(requests.try_recv().is_err());
                    let error = result.unwrap_err();
                    let diagnostic = format!("{error:?} {error}").into_boxed_str();
                    assert!(!diagnostic.contains("volume-secret"));
                    assert!(!diagnostic.contains("volume-token"));
                    requests
                }
            })
            .await;
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
    let (endpoint, requests, server) = server(vec![
        response(412, "<Error><Code>PreconditionFailed</Code><Message>volume-secret volume-token</Message></Error>"),
        response(412, "<Error><Code>PreconditionFailed</Code></Error>"),
        response(503, "<Error><Code>SlowDown</Code></Error>"),
        response(412, "<Error><Code>PreconditionFailed</Code></Error>"),
    ]).await;
    let volume = client(&endpoint, None, allowed());
    let error = volume
        .put(
            WriteTarget::File("key"),
            WriteCondition::Absent,
            Bytes::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error, VolumeError::Exists);
    assert!(!format!("{volume:?} {error:?} {error}").contains("volume-secret"));
    assert!(!format!("{volume:?} {error:?} {error}").contains("volume-token"));
    assert_eq!(
        volume
            .put(
                WriteTarget::File("key"),
                WriteCondition::Matches("\"stale\""),
                Bytes::new()
            )
            .await,
        Err(VolumeError::Conflict)
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("key"),
                WriteCondition::Absent,
                Bytes::new()
            )
            .await,
        Err(VolumeError::RequestFailed)
    );
    assert_eq!(
        volume
            .delete(WriteTarget::File("key"), &metadata("key", "\"stale\"", 1))
            .await,
        Err(VolumeError::Conflict)
    );
    let (conditions, mut requests) = stream::iter(0..4)
        .fold(
            (Vec::with_capacity(4), requests),
            |(mut seen, mut requests), _| async move {
                seen.push(requests.recv().await);
                (seen, requests)
            },
        )
        .await;
    let conditions = conditions.into_boxed_slice();
    assert_eq!(
        conditions[0].as_ref().unwrap().headers()["if-none-match"],
        "*"
    );
    assert_eq!(
        conditions[1].as_ref().unwrap().headers()["if-match"],
        "\"stale\""
    );
    assert_eq!(
        conditions[3].as_ref().unwrap().headers()["if-match"],
        "\"stale\""
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn invalid_kms_headers_are_known_no_send_errors() {
    let (endpoint, mut requests, server) = server(vec![]).await;
    let volume = client(
        &endpoint,
        Some(Encryption::Kms {
            key_id: "volume-secret\r\nvolume-token".into(),
        }),
        allowed(),
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("key"),
                WriteCondition::Absent,
                Bytes::new()
            )
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert_eq!(
        volume
            .start_multipart(WriteTarget::File("key"))
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn successful_metadata_debug_does_not_disclose_reflected_credentials() {
    let (endpoint, mut requests, server) = server(vec![metadata_response(
        4,
        "\"volume-secret\"",
        "volume-token",
    )])
    .await;
    let volume = client(&endpoint, None, allowed());
    let result = volume.stat("volume-secret/volume-token", None).await;
    let metadata = result.as_ref().unwrap();
    assert_eq!(metadata.key.as_ref(), "volume-secret/volume-token");
    assert_eq!(metadata.size, 4);
    assert_eq!(metadata.version.etag.as_ref(), "\"volume-secret\"");
    assert_eq!(metadata.version.version_id.as_deref(), Some("volume-token"));
    assert_eq!(requests.recv().await.unwrap().method(), "HEAD");
    assert!(requests.try_recv().is_err());
    server.abort();

    let configured = client(
        "https://volume.invalid/",
        Some(Encryption::Kms {
            key_id: "volume-secret-volume-token".into(),
        }),
        ConnectionPolicy::new(Box::new([])),
    );
    assert_opaque_debug(&configured);
    assert_opaque_debug(&(
        &result,
        metadata,
        &metadata.version,
        Some(metadata),
        [metadata],
    ));

    let condition = WriteCondition::Matches(&metadata.version.etag);
    let file = WriteTarget::File(&metadata.key);
    let probe = WriteTarget::Probe(&metadata.key);
    assert_opaque_debug(&(WriteCondition::Absent, condition, file, probe));
    assert_eq!(format!("{:?}", WriteCondition::Absent), "Absent");
    assert_eq!(format!("{condition:?}"), "Matches");
    assert_eq!(format!("{file:?}"), "File");
    assert_eq!(format!("{probe:?}"), "Probe");
}

#[test]
#[timeout("30s")]
async fn successful_object_debug_keeps_references_and_continuation_unchanged() {
    let reflected = |body| {
        let mut result = response(200, body);
        result
            .headers_mut()
            .insert("etag", "\"volume-secret\"".parse().unwrap());
        result
            .headers_mut()
            .insert("x-amz-version-id", "volume-token".parse().unwrap());
        result
    };
    let (endpoint, mut requests, server) = server(vec![
        reflected(""),
        reflected("body"),
        response(200, "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>volume-secret+volume-token/token</NextContinuationToken><Contents><Key>corpus/volume-secret/volume-token</Key><Size>4</Size><ETag>&quot;volume-secret&quot;</ETag></Contents></ListBucketResult>"),
        listing_response(&[], None),
        reflected(""),
    ])
    .await;
    let volume = client(&endpoint, None, allowed());
    let target = WriteTarget::File("key");
    let put = volume
        .put(target, WriteCondition::Absent, Bytes::from_static(b"body"))
        .await;
    let version = put.as_ref().unwrap();
    assert_eq!(version.etag.as_ref(), "\"volume-secret\"");
    assert_eq!(version.version_id.as_deref(), Some("volume-token"));
    assert_eq!(
        requests.recv().await.unwrap().headers()["if-none-match"],
        "*"
    );
    let read = volume.read("key", version, None).await;
    let object = read.as_ref().unwrap();
    assert_eq!(&object.version, version);
    assert_eq!(object.length, 4);
    let request = requests.recv().await.unwrap();
    assert_eq!(request.headers()["if-match"], "\"volume-secret\"");
    assert!(
        request
            .uri()
            .query()
            .unwrap()
            .contains("versionId=volume-token")
    );
    let page = volume.list("", None).await;
    let listed = page.as_ref().unwrap();
    assert_eq!(listed.objects.len(), 1);
    assert_eq!(listed.objects[0].key.as_ref(), "volume-secret/volume-token");
    assert_eq!(listed.objects[0].size, 4);
    assert_eq!(listed.objects[0].etag, version.etag);
    assert_eq!(
        listed.continuation.as_deref(),
        Some("volume-secret+volume-token/token")
    );
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    let next = volume
        .list("", listed.continuation.as_deref())
        .await
        .unwrap();
    assert!(next.objects.is_empty());
    assert!(next.continuation.is_none());
    assert!(
        requests
            .recv()
            .await
            .unwrap()
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=volume-secret%2Bvolume-token%2Ftoken")
    );
    let deleted = volume
        .delete(
            target,
            &ObjectMetadata {
                key: "key".into(),
                size: 4,
                version: version.clone(),
            },
        )
        .await;
    assert_eq!(
        deleted
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .payload()
            .as_ref(),
        "volume-token"
    );
    assert_eq!(
        requests.recv().await.unwrap().headers()["if-match"],
        "\"volume-secret\""
    );
    assert!(requests.try_recv().is_err());
    server.abort();
    assert_opaque_debug(&(
        &put,
        version,
        &read,
        object,
        &object.version,
        &object.body,
        &deleted,
    ));
    assert_opaque_debug(&(
        &page,
        listed,
        &listed.objects,
        &listed.objects[0],
        Some(&listed.objects[0]),
    ));
    assert_eq!(read.unwrap().body.collect().await.unwrap(), "body");
}

#[test]
#[timeout("30s")]
async fn success_version_ids_reject_empty_but_normalize_absent_and_null() {
    stream::iter(["stat", "read", "eof", "put", "complete", "delete"])
        .then(|operation| async move {
            stream::iter([None, Some(""), Some("   "), Some("null"), Some("version-B")])
                .then(|id| async move {
                    let mut result = match operation {
                        "stat" | "eof" => metadata_response(4, "\"reference\"", "unused"),
                        "complete" => response(200, "<CompleteMultipartUploadResult><ETag>&quot;reference&quot;</ETag></CompleteMultipartUploadResult>"),
                        "read" => response(200, "body"),
                        "delete" => response(204, ""),
                        _ => response(200, ""),
                    };
                    result.headers_mut().remove("x-amz-version-id");
                    if let Some(id) = id {
                        result.headers_mut().insert("x-amz-version-id", id.parse().unwrap());
                    }
                    let responses = if operation == "eof" {
                        vec![response(416, "<Error><Code>InvalidRange</Code></Error>"), result]
                    } else { vec![result] };
                    let (endpoint, mut requests, server) = server(responses).await;
                    let volume = client(&endpoint, None, allowed());
                    let expected = id.filter(|id| *id != "null");
                    let reference = reference("\"reference\"", expected);
                    let parts = [UploadedPart { number: 1, etag: "\"part\"".into(), size: 4 }];
                    let actual = match operation {
                        "stat" => volume.stat("key", None).await.map(|r| r.version.version_id),
                        "read" | "eof" => volume.read("key", &reference, (operation == "eof").then_some(4..=9)).await.map(|r| r.version.version_id),
                        "put" => volume.put(WriteTarget::File("key"), WriteCondition::Absent, Bytes::from_static(b"body")).await.map(|r| r.version_id),
                        "complete" => volume.complete_multipart(WriteTarget::File("key"), "upload", &parts, WriteCondition::Absent).await.map(|r| r.version_id),
                        "delete" => volume.delete(WriteTarget::File("key"), &metadata("key", "\"reference\"", 4)).await.map(|r| r.map(ResponseValue::into_payload)),
                        _ => unreachable!(),
                    };
                    if id.is_some_and(|id| id.trim().is_empty()) {
                        let error = if matches!(operation, "put" | "complete" | "delete") { VolumeError::RequestFailed } else { VolumeError::InvalidResponse };
                        assert_eq!(actual, Err(error), "operation={operation}, id={id:?}");
                    } else {
                        assert_eq!(actual.unwrap().as_deref(), expected, "operation={operation}, id={id:?}");
                    }
                    assert!(requests.try_recv().is_ok());
                    if operation == "eof" { assert_eq!(requests.try_recv().unwrap().method(), "HEAD"); }
                    assert!(requests.try_recv().is_err());
                    server.abort();
                }).collect::<()>().await;
        }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn initiation_requires_a_nonempty_opaque_upload_id() {
    stream::iter([None, Some(""), Some("null"), Some("upload-B")])
        .then(|id| async move {
            let field = id
                .map(|id| format!("<UploadId>{id}</UploadId>"))
                .unwrap_or_default();
            let (endpoint, mut requests, server) = server(vec![response(
                200,
                format!("<InitiateMultipartUploadResult>{field}</InitiateMultipartUploadResult>"),
            )])
            .await;
            let result = client(&endpoint, None, allowed())
                .start_multipart(WriteTarget::File("key"))
                .await;
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                assert_eq!(result.unwrap().payload().as_ref(), id);
            } else {
                assert_eq!(result.unwrap_err(), VolumeError::InvalidResponse);
            }
            assert_eq!(requests.try_recv().unwrap().method(), "POST");
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn delete_returns_only_the_normalized_delete_marker_version_id() {
    stream::iter([
        (Some("delete-marker-B"), Some("delete-marker-B")),
        (Some("null"), None),
        (None, None),
    ])
    .then(|(marker_id, expected)| async move {
        let mut deleted = response(204, "");
        deleted.headers_mut().remove("x-amz-version-id");
        if let Some(marker_id) = marker_id {
            deleted
                .headers_mut()
                .insert("x-amz-version-id", marker_id.parse().unwrap());
        }
        let (endpoint, mut requests, server) = server(vec![
            metadata_response(4, "\"object-A\"", "object-version-A"),
            deleted,
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        let reference = volume.stat("key", None).await.unwrap();
        assert_eq!(reference.version.etag.as_ref(), "\"object-A\"");
        assert_eq!(
            reference.version.version_id.as_deref(),
            Some("object-version-A")
        );
        assert_eq!(requests.try_recv().unwrap().method(), "HEAD");
        let result: Option<ResponseValue<Arc<str>>> = volume
            .delete(WriteTarget::File("key"), &reference)
            .await
            .unwrap();
        assert_eq!(
            result.as_ref().map(|value| value.payload().as_ref()),
            expected
        );
        let request = requests.try_recv().unwrap();
        assert_eq!(request.method(), "DELETE");
        assert_eq!(request.uri().path(), "/test-bucket/corpus/key");
        assert!(
            !request
                .uri()
                .query()
                .unwrap_or_default()
                .contains("versionId=")
        );
        assert_eq!(request.headers()["if-match"], "\"object-A\"");
        assert!(requests.try_recv().is_err());
        server.abort();
        assert_opaque_debug(&result);
        assert!(!format!("{result:?} {result:#?}").contains("delete-marker-B"));
        assert_eq!(result.map(ResponseValue::into_payload).as_deref(), expected);
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn multipart_debug_keeps_upload_ids_after_failed_completion() {
    let mut part_response = response(200, "");
    part_response
        .headers_mut()
        .insert("etag", "\"volume-secret-volume-token\"".parse().unwrap());
    let mut completed = response(
        200,
        "<CompleteMultipartUploadResult><ETag>&quot;volume-secret-volume-token&quot;</ETag></CompleteMultipartUploadResult>",
    );
    completed.headers_mut().insert(
        "x-amz-version-id",
        "volume-secret-volume-token".parse().unwrap(),
    );
    let (endpoint, mut requests, server) = server(vec![
        response(200, "<InitiateMultipartUploadResult><UploadId>volume-secret+volume-token/id</UploadId></InitiateMultipartUploadResult>"),
        part_response,
        response(412, "<Error><Code>PreconditionFailed</Code></Error>"),
        response(204, ""),
        completed,
    ]).await;
    let volume = client(&endpoint, None, allowed());
    let target = WriteTarget::File("large");
    let started = volume.start_multipart(target).await;
    let upload = started.as_ref().unwrap();
    assert_eq!(upload.payload().as_ref(), "volume-secret+volume-token/id");
    assert_eq!(requests.recv().await.unwrap().method(), "POST");
    let uploaded = volume
        .upload_part(target, upload.payload(), 1, Bytes::from_static(b"body"))
        .await;
    let parts = [uploaded.as_ref().unwrap().clone()];
    assert_eq!(parts[0].etag.as_ref(), "\"volume-secret-volume-token\"");
    assert_eq!(parts[0].size, 4);
    assert_eq!(parts[0].number, 1);
    assert_eq!(
        volume
            .complete_multipart(target, upload.payload(), &parts, WriteCondition::Absent)
            .await,
        Err(VolumeError::Exists)
    );
    volume
        .abort_multipart(target, upload.payload())
        .await
        .unwrap();
    let mut requests = stream::iter(["PUT", "POST", "DELETE"])
        .fold(requests, |mut requests, method| async move {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.method(), method);
            assert_eq!(request.uri().path(), "/test-bucket/corpus/large");
            assert!(
                request
                    .uri()
                    .query()
                    .unwrap()
                    .contains("uploadId=volume-secret%2Bvolume-token%2Fid")
            );
            if method == "POST" {
                assert_eq!(request.headers()["if-none-match"], "*");
                assert!(
                    std::str::from_utf8(request.body())
                        .unwrap()
                        .contains("volume-secret-volume-token")
                );
            }
            requests
        })
        .await;
    let published = volume
        .complete_multipart(target, "another-upload", &parts, WriteCondition::Absent)
        .await;
    let version = published.as_ref().unwrap();
    assert_eq!(version.etag.as_ref(), "\"volume-secret-volume-token\"");
    assert_eq!(
        version.version_id.as_deref(),
        Some("volume-secret-volume-token")
    );
    assert_eq!(requests.recv().await.unwrap().method(), "POST");
    assert!(requests.try_recv().is_err());
    server.abort();
    assert_opaque_debug(&(
        &started,
        upload,
        &uploaded,
        &parts,
        &parts[0],
        Some(&parts[0]),
        &published,
        version,
    ));
    assert_eq!(
        started.unwrap().into_payload().as_ref(),
        "volume-secret+volume-token/id"
    );
}

#[test]
fn response_value_debug_does_not_require_or_invoke_payload_debug() {
    struct NoDebug(Box<str>);
    struct FailingDebug;

    impl Debug for FailingDebug {
        fn fmt(&self, _: &mut Formatter<'_>) -> std::fmt::Result {
            panic!("payload Debug must not run");
        }
    }

    let value = ResponseValue::new(NoDebug("volume-secret volume-token".into()));
    assert_opaque_debug(&value);
    assert_eq!(value.payload().0.as_ref(), "volume-secret volume-token");
    assert_eq!(
        value.into_payload().0.as_ref(),
        "volume-secret volume-token"
    );
    assert_opaque_debug(&ResponseValue::new(FailingDebug));
}

#[test]
#[timeout("30s")]
async fn deletion_between_read_and_publish_is_a_conflict_not_missing_infrastructure() {
    let (endpoint, _requests, server) = server(vec![
        response(404, "<Error><Code>NoSuchKey</Code></Error>"),
        response(404, "<Error><Code>NoSuchKey</Code></Error>"),
        response(404, "<Error><Code>NoSuchBucket</Code></Error>"),
        response(404, "<Error><Code>NoSuchUpload</Code></Error>"),
    ])
    .await;
    let volume = client(&endpoint, None, allowed());
    let target = WriteTarget::File("key");
    let condition = WriteCondition::Matches("\"old\"");
    assert_eq!(
        volume.put(target, condition, Bytes::new()).await,
        Err(VolumeError::Conflict)
    );
    let parts = [UploadedPart {
        number: 1,
        etag: "\"part\"".into(),
        size: 1,
    }];
    assert_eq!(
        volume
            .complete_multipart(target, "upload", &parts, condition)
            .await,
        Err(VolumeError::Conflict)
    );
    assert_eq!(
        volume.put(target, condition, Bytes::new()).await,
        Err(VolumeError::NotFound)
    );
    assert_eq!(
        volume
            .complete_multipart(target, "missing", &parts, condition)
            .await,
        Err(VolumeError::NotFound)
    );
    server.abort();
}

#[test]
#[timeout("30s")]
async fn streamed_checksum_errors_do_not_disclose_reflected_credentials() {
    use base64::Engine;
    use base64::prelude::BASE64_STANDARD;
    let capture = Capture(Arc::new(Mutex::new(Vec::new())));
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let (diagnostic, operation_start) = async {
        let checksum: http::HeaderValue = BASE64_STANDARD.encode("volume-token").parse().unwrap();
        let reflected = || {
            let mut result = response(200, "body");
            result
                .headers_mut()
                .insert("x-amz-checksum-sha256", checksum.clone());
            result
        };
        let (endpoint, _requests, server) = server(vec![reflected(), reflected()]).await;
        let volume = client(&endpoint, None, allowed());
        let operation_start = capture.0.lock().unwrap().len();
        let read = volume
            .read("key", &reference("\"reference\"", Some("version-7")), None)
            .await
            .unwrap();
        let error = read.body.collect().await.unwrap_err();
        assert_eq!(error, VolumeError::RequestFailed);
        let debug = format!("{error:?} {error}").into_boxed_str();
        assert!(!debug.contains("volume-token"));
        assert!(!debug.contains("[118, 111, 108, 117, 109, 101, 45, 116, 111, 107, 101, 110]"));
        let mut read = volume
            .read("key", &reference("\"reference\"", Some("version-7")), None)
            .await
            .unwrap();
        assert_eq!(
            read.body.next().await.unwrap(),
            Some(Bytes::from_static(b"body"))
        );
        assert_eq!(read.body.next().await, Err(VolumeError::RequestFailed));
        tracing::info!("after body consumption");
        server.abort();
        (debug, operation_start)
    }
    .with_subscriber(subscriber)
    .await;
    let bytes = capture.0.lock().unwrap();
    let log = std::str::from_utf8(&bytes).unwrap();
    assert!(log.contains("after body consumption"));
    [
        "volume-token",
        "volume-secret",
        "dm9sdW1lLXRva2Vu",
        "766f6c756d652d746f6b656e",
        "[118, 111, 108, 117, 109, 101, 45, 116, 111, 107, 101, 110]",
        "\\x76\\x6f\\x6c\\x75\\x6d\\x65\\x2d\\x74\\x6f\\x6b\\x65\\x6e",
    ]
    .into_iter()
    .for_each(|value| {
        assert!(!diagnostic.contains(value));
        assert!(!log.contains(value));
    });
    let operation_log = &log[operation_start..];
    ["aws_sdk_s3", "aws_smithy", "aws_sigv4", "aws_runtime"]
        .into_iter()
        .for_each(|target| {
            assert!(
                !operation_log.contains(target),
                "body poll emitted {target} diagnostics: {operation_log}"
            )
        });
}

#[test]
#[timeout("30s")]
async fn the_prefix_root_marker_is_hidden_from_stat_and_read() {
    let (endpoint, mut requests, server) = server(vec![response(200, ""), response(200, "")]).await;
    let volume = client(&endpoint, None, allowed());
    assert_eq!(volume.stat("", None).await, Err(VolumeError::NotFound));
    assert!(matches!(
        volume
            .read("", &reference("\"reference\"", Some("version-7")), None)
            .await,
        Err(VolumeError::NotFound)
    ));
    [requests.try_recv().unwrap(), requests.try_recv().unwrap()]
        .into_iter()
        .for_each(|request| assert_eq!(request.uri().path(), "/test-bucket/corpus/"));
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn an_explicit_null_version_is_checked_for_stat_and_read() {
    stream::iter([
        (None, None, Ok(None), Ok(None)),
        (
            None,
            Some("version-7"),
            Ok(Some("version-7")),
            Err(VolumeError::InvalidResponse),
        ),
        (
            Some("version-7"),
            Some("version-7"),
            Ok(Some("version-7")),
            Ok(Some("version-7")),
        ),
        (
            Some("version-7"),
            None,
            Err(VolumeError::InvalidResponse),
            Err(VolumeError::InvalidResponse),
        ),
        (
            Some("version-7"),
            Some("null"),
            Err(VolumeError::InvalidResponse),
            Err(VolumeError::InvalidResponse),
        ),
        (
            Some("version-7"),
            Some("other"),
            Err(VolumeError::InvalidResponse),
            Err(VolumeError::InvalidResponse),
        ),
    ])
    .then(
        |(requested, actual, expected_stat, expected_read)| async move {
            let observed = || {
                let mut result = response(200, "body");
                result.headers_mut().remove("x-amz-version-id");
                if let Some(actual) = actual {
                    result
                        .headers_mut()
                        .insert("x-amz-version-id", actual.parse().unwrap());
                }
                result
            };
            let (endpoint, mut requests, server) = server(vec![observed(), observed()]).await;
            let volume = client(&endpoint, None, allowed());
            let stat = volume.stat("key", requested).await;
            let read = volume
                .read("key", &reference("\"reference\"", requested), None)
                .await;
            assert_eq!(
                stat.map(|stat| stat.version.version_id),
                expected_stat.map(|version| version.map(Arc::<str>::from))
            );
            match expected_read {
                Ok(version) => {
                    let read = read.unwrap();
                    assert_eq!(read.version.version_id.as_deref(), version);
                    assert_eq!(read.body.collect().await.unwrap(), "body");
                }
                Err(error) => {
                    assert_eq!(read.err(), Some(error));
                }
            }
            ["HEAD", "GET"].into_iter().for_each(|method| {
                let request = requests.try_recv().unwrap();
                assert_eq!(request.method(), method);
                assert_eq!(
                    request
                        .uri()
                        .query()
                        .is_some_and(|query| query.contains("versionId=version-7")),
                    requested.is_some()
                );
            });
            assert!(requests.try_recv().is_err());
            server.abort();
        },
    )
    .collect::<()>()
    .await;
    stream::iter([None, Some("null"), Some("wrong-version")])
        .then(|actual_version| async move {
            let object_response = || {
                let mut result = response(200, "body");
                result.headers_mut().remove("x-amz-version-id");
                if let Some(version) = actual_version {
                    result
                        .headers_mut()
                        .insert("x-amz-version-id", version.parse().unwrap());
                }
                result
            };
            let (endpoint, mut requests, server) =
                server(vec![object_response(), object_response()]).await;
            let volume = client(&endpoint, None, allowed());
            let stat = volume.stat("key", Some("null")).await;
            let read = volume
                .read("key", &reference("\"reference\"", Some("null")), None)
                .await;
            if actual_version == Some("wrong-version") {
                assert_eq!(stat, Err(VolumeError::InvalidResponse));
                assert!(matches!(read, Err(VolumeError::InvalidResponse)));
            } else {
                assert_eq!(stat.unwrap().version.version_id, None);
                let read = read.unwrap();
                assert_eq!(read.version.version_id, None);
                assert_eq!(read.body.collect().await.unwrap(), "body");
            }
            [
                requests.recv().await.unwrap(),
                requests.recv().await.unwrap(),
            ]
            .into_iter()
            .for_each(|request| {
                assert!(
                    request
                        .uri()
                        .query()
                        .unwrap()
                        .split('&')
                        .any(|parameter| parameter == "versionId=null")
                );
            });
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn get_success_requires_the_status_for_the_requested_range_before_hiding_markers() {
    stream::iter(["key", "dir/", ""])
        .then(|key| async move {
            stream::iter([(206, false), (200, true), (200, false), (206, true)])
                .then(|(status, ranged)| async move {
                    let marker = key != "key";
                    let mut result = response(status, if marker { "" } else { "cde" });
                    if ranged {
                        result.headers_mut().insert(
                            "content-range",
                            if marker {
                                "bytes 0-0/1"
                            } else {
                                "bytes 2-4/12"
                            }
                            .parse()
                            .unwrap(),
                        );
                    }
                    let (endpoint, mut requests, server) = server(vec![result]).await;
                    let volume = client(&endpoint, None, allowed());
                    let range = ranged.then_some(if marker { 0..=0 } else { 2..=4 });
                    let actual = volume
                        .read(key, &reference("\"reference\"", Some("version-7")), range)
                        .await;
                    if status != if ranged { 206 } else { 200 } || marker && ranged {
                        assert_eq!(actual.unwrap_err(), VolumeError::InvalidResponse);
                    } else if marker {
                        assert_eq!(actual.unwrap_err(), VolumeError::NotFound);
                    } else {
                        assert_eq!(actual.unwrap().body.collect().await.unwrap(), "cde");
                    }
                    assert_eq!(requests.try_recv().unwrap().method(), "GET");
                    assert!(requests.try_recv().is_err());
                    server.abort();
                })
                .collect::<()>()
                .await;
        })
        .collect::<()>()
        .await;
    stream::iter(["dir/", ""])
        .then(|key| async move {
            let (endpoint, mut requests, server) =
                server(vec![metadata_response(0, "\"reference\"", "")]).await;
            assert_eq!(
                client(&endpoint, None, allowed())
                    .read(key, &reference("\"reference\"", None), None)
                    .await
                    .unwrap_err(),
                VolumeError::InvalidResponse
            );
            assert_eq!(requests.try_recv().unwrap().method(), "GET");
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn overlapping_gets_keep_status_observations_local_to_each_operation() {
    stream::iter([
        ["full", "partial", "ranged"],
        ["full", "ranged", "partial"],
        ["partial", "full", "ranged"],
        ["partial", "ranged", "full"],
        ["ranged", "full", "partial"],
        ["ranged", "partial", "full"],
    ])
    .then(|order| async move {
        let (sent, received) = mpsc::unbounded_channel();
        let app = Router::new().fallback(move |request: Request<Body>| {
            let sent = sent.clone();
            async move {
                let (respond, response) = oneshot::channel::<Response<Body>>();
                sent.send((request.uri().path().to_owned().into_boxed_str(), respond))
                    .unwrap();
                response.await.unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let volume = Arc::new(client(&endpoint, None, allowed()));
        let reads = ["full", "partial", "ranged"].map(|key| {
            let volume = volume.clone();
            tokio::spawn(async move {
                let result = volume
                    .read(
                        key,
                        &reference("\"reference\"", Some("version-7")),
                        (key == "ranged").then_some(2..=4),
                    )
                    .await;
                if key == "partial" {
                    assert_eq!(result.unwrap_err(), VolumeError::InvalidResponse);
                } else {
                    assert_eq!(result.unwrap().body.collect().await.unwrap(), "cde");
                }
            })
        });
        let (mut gates, mut received) = stream::iter(0..3)
            .fold(
                (std::collections::BTreeMap::new(), received),
                |(mut gates, mut received), _| async move {
                    let (key, respond) = received.recv().await.unwrap();
                    gates.insert(key, respond);
                    (gates, received)
                },
            )
            .await;
        order.into_iter().for_each(|key| {
            let mut result = response(if key == "full" { 200 } else { 206 }, "cde");
            if key == "ranged" {
                result
                    .headers_mut()
                    .insert("content-range", "bytes 2-4/12".parse().unwrap());
            }
            gates
                .remove(format!("/test-bucket/corpus/{key}").as_str())
                .unwrap()
                .send(result)
                .unwrap();
        });
        futures::future::try_join_all(reads).await.unwrap();
        assert!(received.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn readonly_received_success_parse_errors_are_not_transport_failures() {
    stream::iter(["stat", "read", "list", "versioning", "lifecycle", "put"]).then(|operation| async move {
        let mut result = if operation == "list" {
            response(200, "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>not-a-boolean</IsTruncated></ListBucketResult>")
        } else if matches!(operation, "versioning" | "lifecycle") {
            response(200, "<broken")
        } else { response(200, "") };
        result.headers_mut().insert("x-amz-delete-marker", "not-a-boolean".parse().unwrap());
        if operation == "put" {
            result.headers_mut().insert("x-amz-server-side-encryption-bucket-key-enabled", "not-a-boolean".parse().unwrap());
        }
        let (endpoint, mut requests, server) = server(vec![result]).await;
        let volume = client(&endpoint, None, allowed());
        let actual = match operation {
            "stat" => volume.stat("key", None).await.map(|_| ()),
            "read" => volume.read("key", &reference("\"reference\"", Some("version-7")), None).await.map(|_| ()),
            "list" => volume.list("", None).await.map(|_| ()),
            "versioning" => volume.versioning().await.map(|_| ()),
            "lifecycle" => volume.lifecycle().await.map(|_| ()),
            "put" => volume.put(WriteTarget::File("key"), WriteCondition::Absent, Bytes::new()).await.map(|_| ()),
            _ => unreachable!(),
        };
        assert_eq!(actual, Err(if operation == "put" { VolumeError::RequestFailed } else { VolumeError::InvalidResponse }), "{operation}");
        assert!(requests.try_recv().is_ok());
        assert!(requests.try_recv().is_err());
        server.abort();
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn response_body_transport_errors_remain_uncertain_even_after_success_headers() {
    stream::iter(["list", "versioning", "lifecycle", "eof"])
        .then(|operation| async move {
            let chunks = stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"<")) })
                .chain(stream::once(async {
                    Err(std::io::Error::other("volume-secret volume-token"))
                }));
            let (endpoint, mut requests, server) = server(vec![response(
                if operation == "eof" { 416 } else { 200 },
                Body::from_stream(chunks),
            )])
            .await;
            let volume = client(&endpoint, None, allowed());
            let result = match operation {
                "list" => volume.list("", None).await.map(|_| ()),
                "versioning" => volume.versioning().await.map(|_| ()),
                "lifecycle" => volume.lifecycle().await.map(|_| ()),
                "eof" => volume
                    .read(
                        "key",
                        &reference("\"reference\"", Some("version-7")),
                        Some(4..=9),
                    )
                    .await
                    .map(|_| ()),
                _ => unreachable!(),
            };
            assert_eq!(result, Err(VolumeError::RequestFailed), "{operation}");
            assert!(requests.try_recv().is_ok());
            assert!(requests.try_recv().is_err());
            assert_opaque_debug(&result);
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn only_eligible_416_codes_can_confirm_ranged_eof() {
    stream::iter([
        (416, Some("ExpiredToken"), Some(VolumeError::Denied)),
        (416, Some("InvalidToken"), Some(VolumeError::Denied)),
        (416, Some("TokenRefreshRequired"), Some(VolumeError::Denied)),
        (416, Some("PreconditionFailed"), Some(VolumeError::Conflict)),
        (
            416,
            Some("ConditionalRequestConflict"),
            Some(VolumeError::Conflict),
        ),
        (416, Some("Unknown"), Some(VolumeError::RequestFailed)),
        (400, Some("InvalidRange"), Some(VolumeError::RequestFailed)),
        (416, None, None),
        (416, Some("InvalidRange"), None),
        (416, Some("RequestedRangeNotSatisfiable"), None),
    ])
    .then(|(status, code, expected)| async move {
        let body = code
            .map(|code| format!("<Error><Code>{code}</Code></Error>"))
            .unwrap_or_else(|| "<Error/>".into());
        let (endpoint, mut requests, server) = server(vec![
            response(status, body),
            metadata_response(4, "\"reference\"", "version-7"),
        ])
        .await;
        let actual = client(&endpoint, None, allowed())
            .read(
                "key",
                &reference("\"reference\"", Some("version-7")),
                Some(4..=9),
            )
            .await;
        assert_eq!(requests.try_recv().unwrap().method(), "GET");
        if let Some(expected) = expected {
            assert_eq!(actual.unwrap_err(), expected);
        } else {
            assert_eq!(actual.unwrap().length, 0);
            assert_eq!(requests.try_recv().unwrap().method(), "HEAD");
        }
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn reads_reject_ignored_ranges_and_contradictory_versions_or_etags() {
    stream::iter([
        (None, Some("bytes 2-4/12"), "cde"),
        (Some(2..=4), None, "cde"),
        (Some(2..=4), Some("items 2-4/12"), "cde"),
        (Some(2..=4), Some("bytes 2-4"), "cde"),
        (Some(2..=4), Some("bytes two-4/12"), "cde"),
        (Some(2..=4), Some("bytes 2-four/12"), "cde"),
        (Some(2..=4), Some("bytes 2-4/*"), "cde"),
        (Some(2..=4), Some("bytes 2-4/0"), "cde"),
        (Some(2..=4), Some("bytes 1-4/12"), "bcde"),
        (Some(2..=4), Some("bytes 2-3/12"), "cd"),
        (Some(2..=4), Some("bytes 2-4/12"), "cd"),
    ])
    .then(|(range, content_range, body)| async move {
        let mut result = response(206, body);
        if let Some(content_range) = content_range {
            result
                .headers_mut()
                .insert("content-range", content_range.parse().unwrap());
        }
        let (endpoint, mut requests, server) = server(vec![result]).await;
        let volume = client(&endpoint, None, allowed());
        assert!(matches!(
            volume
                .read("key", &reference("\"reference\"", Some("version-7")), range)
                .await,
            Err(VolumeError::InvalidResponse)
        ));
        assert_eq!(requests.try_recv().unwrap().method(), "GET");
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
    let (endpoint, mut requests, server) = server(vec![{
        let mut result = response(206, "cde");
        result
            .headers_mut()
            .insert("content-range", "bytes 2-4/5".parse().unwrap());
        result
    }])
    .await;
    let volume = client(&endpoint, None, allowed());
    assert!(matches!(
        volume
            .read(
                "key",
                &reference("\"reference\"", Some("version-7")),
                Some(RangeInclusive::new(5, 4))
            )
            .await,
        Err(VolumeError::InvalidRequest)
    ));
    assert!(requests.try_recv().is_err());
    let tail = volume
        .read(
            "key",
            &reference("\"reference\"", Some("version-7")),
            Some(2..=u64::MAX),
        )
        .await
        .unwrap();
    assert_eq!(tail.length, 3);
    assert_eq!(tail.body.collect().await.unwrap(), "cde");
    assert_eq!(
        requests.try_recv().unwrap().headers()["range"],
        "bytes=2-9223372036854775807"
    );
    assert!(requests.try_recv().is_err());
    server.abort();
    let ranged = |content_range: &'static str| {
        let mut result = response(206, "cde");
        result
            .headers_mut()
            .insert("content-range", content_range.parse().unwrap());
        result
    };
    let (endpoint, _requests, server) = self::server(vec![
        response(200, "whole-object"),
        response(200, "wrong-version"),
        response(200, "wrong-etag"),
        ranged("bytes 0-2/12"),
        ranged("bytes 2-5/12"),
        ranged("bytes 2-4/5"),
    ])
    .await;
    let volume = client(&endpoint, None, allowed());
    assert!(matches!(
        volume
            .read(
                "key",
                &reference("\"reference\"", Some("version-7")),
                Some(2..=4)
            )
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume
            .read(
                "key",
                &reference("\"reference\"", Some("requested-version")),
                None
            )
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume
            .read(
                "key",
                &reference("\"requested-etag\"", Some("version-7")),
                None
            )
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume
            .read(
                "key",
                &reference("\"reference\"", Some("version-7")),
                Some(2..=4)
            )
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume
            .read(
                "key",
                &reference("\"reference\"", Some("version-7")),
                Some(2..=4)
            )
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    let tail = volume
        .read(
            "key",
            &reference("\"reference\"", Some("version-7")),
            Some(2..=100),
        )
        .await
        .unwrap();
    assert_eq!(tail.length, 3);
    assert_eq!(tail.body.collect().await.unwrap(), "cde");
    server.abort();
}

#[test]
#[timeout("30s")]
async fn nonempty_slash_keys_remain_files_and_empty_slash_keys_are_not_created() {
    let (endpoint, mut requests, server) = server(vec![response(200, ""), response(204, "")]).await;
    let volume = client(&endpoint, None, allowed());
    assert!(
        volume
            .put(
                WriteTarget::File("nonempty/"),
                WriteCondition::Matches("\"old\""),
                Bytes::from_static(b"content")
            )
            .await
            .is_ok()
    );
    assert_eq!(
        requests.recv().await.unwrap().uri().path(),
        "/test-bucket/corpus/nonempty/"
    );
    assert_eq!(
        volume
            .put(
                WriteTarget::File("empty/"),
                WriteCondition::Absent,
                Bytes::new()
            )
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert!(
        volume
            .delete(
                WriteTarget::File("nonempty/"),
                &metadata("nonempty/", "\"reference\"", 7)
            )
            .await
            .is_ok()
    );
    assert_eq!(requests.recv().await.unwrap().method(), "DELETE");
    assert_eq!(
        volume
            .delete(
                WriteTarget::File("marker/"),
                &metadata("marker/", "\"marker\"", 0)
            )
            .await,
        Err(VolumeError::InvalidRequest)
    );
    let parts = [UploadedPart {
        number: 1,
        etag: "\"part\"".into(),
        size: 0,
    }];
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("marker/"),
                "upload",
                &parts,
                WriteCondition::Absent
            )
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn trace_logs_do_not_disclose_credentials_or_reflected_error_messages() {
    let capture = Capture(Arc::new(Mutex::new(Vec::new())));
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let operation_start = async {
        let mut deleted = response(204, "");
        deleted.headers_mut().insert("x-amz-version-id", "volume-secret-volume-token".parse().unwrap());
        let (endpoint, mut requests, server) = server(vec![response(
            403,
            "<Error><Code>AccessDenied</Code><Message>volume-secret volume-token</Message></Error>",
        ),
            metadata_response(4, "\"volume-secret\"", "volume-token"),
            response(200, "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>volume-secret-volume-token</NextContinuationToken><Contents><Key>corpus/volume-secret/volume-token</Key><Size>4</Size><ETag>&quot;volume-secret-volume-token&quot;</ETag></Contents></ListBucketResult>"),
            response(200, "body"),
            deleted,
            response(200, "<InitiateMultipartUploadResult><UploadId>volume-secret-volume-token</UploadId></InitiateMultipartUploadResult>"),
            response(200, ""),
            response(200, "<CompleteMultipartUploadResult><ETag>&quot;volume-secret-volume-token&quot;</ETag></CompleteMultipartUploadResult>"),
            response(204, ""),
            response(200, "<VersioningConfiguration><Status>volume-secret-volume-token</Status></VersioningConfiguration>"),
            response(200, "<LifecycleConfiguration><Rule><ID>volume-secret-volume-token</ID><Status>volume-secret-volume-token</Status><Filter><Prefix>volume-secret-volume-token</Prefix></Filter></Rule></LifecycleConfiguration>"),
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        let operation_start = capture.0.lock().unwrap().len();
        assert_eq!(
            volume
                .put(
                    WriteTarget::File("key"),
                    WriteCondition::Absent,
                    Bytes::new()
                )
                .await,
            Err(VolumeError::Denied)
        );
        let metadata = volume.stat("key", None).await.unwrap();
        assert_eq!(metadata.version.etag.as_ref(), "\"volume-secret\"");
        let page = volume.list("", None).await.unwrap();
        assert_eq!(page.objects[0].key.as_ref(), "volume-secret/volume-token");
        assert_eq!(page.objects[0].etag.as_ref(), "\"volume-secret-volume-token\"");
        assert_eq!(page.continuation.as_deref(), Some("volume-secret-volume-token"));
        let read = volume.read("key", &reference("\"reference\"", Some("version-7")), None).await.unwrap();
        assert_eq!(read.body.collect().await.unwrap(), "body");
        assert_eq!(volume.delete(WriteTarget::File("key"), &metadata).await.unwrap().unwrap().payload().as_ref(), "volume-secret-volume-token");
        let upload = volume.start_multipart(WriteTarget::File("large")).await.unwrap();
        assert_eq!(upload.payload().as_ref(), "volume-secret-volume-token");
        let part = volume.upload_part(WriteTarget::File("large"), upload.payload(), 1, Bytes::from_static(b"part")).await.unwrap();
        assert_eq!(part.number, 1);
        assert_eq!(volume.complete_multipart(WriteTarget::File("large"), upload.payload(), &[part], WriteCondition::Absent).await.unwrap().etag.as_ref(), "\"volume-secret-volume-token\"");
        volume.abort_multipart(WriteTarget::File("large"), upload.payload()).await.unwrap();
        assert_eq!(volume.versioning().await.unwrap(), BucketVersioning::Unknown(ResponseValue::new("volume-secret-volume-token".into())));
        assert_eq!(volume.lifecycle().await.unwrap().as_ref(), [LifecycleRule {
            status: LifecycleRuleStatus::Unknown(ResponseValue::new("volume-secret-volume-token".into())),
            prefix: Some(ResponseValue::new("volume-secret-volume-token".into())),
            noncurrent_expiration: None,
        }]);
        ["PUT", "HEAD", "GET", "GET", "DELETE", "POST", "PUT", "POST", "DELETE", "GET", "GET"]
            .into_iter().for_each(|method| assert_eq!(requests.try_recv().unwrap().method(), method));
        assert!(requests.try_recv().is_err());
        tracing::info!("after the volume request");
        server.abort();
        operation_start
    }
    .with_subscriber(subscriber)
    .await;
    let bytes = capture.0.lock().unwrap();
    let log = std::str::from_utf8(&bytes).unwrap();
    assert!(log.contains("after the volume request"));
    ["volume-access", "volume-secret", "volume-token"]
        .iter()
        .for_each(|credential| {
            assert!(!log.contains(credential), "trace disclosed {credential}");
        });
    let operation_log = &log[operation_start..];
    ["aws_sdk_s3", "aws_smithy", "aws_sigv4", "aws_runtime"]
        .into_iter()
        .for_each(|target| {
            assert!(
                !operation_log.contains(target),
                "operation emitted {target} diagnostics: {operation_log}"
            )
        });
}

#[test]
#[timeout("30s")]
async fn wildcard_and_multiple_etags_cannot_bypass_write_conditions() {
    let (endpoint, mut requests, server) = server(Vec::new()).await;
    let volume = client(&endpoint, None, allowed());
    stream::iter([
        "*",
        " * ",
        "\t*\t",
        "",
        "\"one\", \"two\"",
        "W/\"weak\"",
        "unquoted",
        "\"a\"b\"",
        "\"a b\"",
        "\"a\tb\"",
        "\"a\rb\"",
        "\"a\nb\"",
        "\"a\u{7f}b\"",
    ])
    .then(|etag| async {
        assert_eq!(
            volume
                .delete(WriteTarget::File("key"), &metadata("key", etag, 1))
                .await,
            Err(VolumeError::InvalidRequest)
        );
        assert_eq!(
            volume
                .put(
                    WriteTarget::File("key"),
                    WriteCondition::Matches(etag),
                    Bytes::new()
                )
                .await,
            Err(VolumeError::InvalidRequest)
        );
    })
    .collect::<()>()
    .await;
    assert!(requests.try_recv().is_err());
    server.abort();

    stream::iter(["\"\"", "\"!\""])
        .then(|etag| async move {
            let (endpoint, mut requests, server) =
                self::server(vec![response(200, ""), response(204, "")]).await;
            let volume = client(&endpoint, None, allowed());
            assert!(
                volume
                    .put(
                        WriteTarget::File("key"),
                        WriteCondition::Matches(etag),
                        Bytes::new()
                    )
                    .await
                    .is_ok()
            );
            assert_eq!(requests.try_recv().unwrap().headers()["if-match"], etag);
            assert!(
                volume
                    .delete(WriteTarget::File("key"), &metadata("key", etag, 1))
                    .await
                    .is_ok()
            );
            assert_eq!(requests.try_recv().unwrap().headers()["if-match"], etag);
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn multipart_errors_inside_success_responses_keep_their_conflict_meaning() {
    let (endpoint, _requests, server) = server(vec![
        response(200, "<Error><Code>PreconditionFailed</Code></Error>"),
        response(200, "<Error><Code>PreconditionFailed</Code></Error>"),
        response(200, "<Error><Code>AccessDenied</Code></Error>"),
        response(200, "<Error><Code>NoSuchKey</Code></Error>"),
        response(200, "<Error><Code>NoSuchUpload</Code></Error>"),
    ])
    .await;
    let volume = client(&endpoint, None, allowed());
    let parts = [UploadedPart {
        number: 1,
        etag: "\"part\"".into(),
        size: 1,
    }];
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("key"),
                "upload",
                &parts,
                WriteCondition::Matches("\"old\"")
            )
            .await,
        Err(VolumeError::Conflict)
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("key"),
                "upload",
                &parts,
                WriteCondition::Absent
            )
            .await,
        Err(VolumeError::Exists)
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("key"),
                "upload",
                &parts,
                WriteCondition::Absent
            )
            .await,
        Err(VolumeError::Denied)
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("key"),
                "upload",
                &parts,
                WriteCondition::Matches("\"old\"")
            )
            .await,
        Err(VolumeError::Conflict)
    );
    assert_eq!(
        volume
            .complete_multipart(
                WriteTarget::File("key"),
                "upload",
                &parts,
                WriteCondition::Absent
            )
            .await,
        Err(VolumeError::NotFound)
    );
    server.abort();
}

#[test]
#[timeout("30s")]
async fn ranged_reads_return_eof_only_after_validating_the_same_reference() {
    stream::iter([
        (4, 4),
        (4, 9),
        (0, 0),
        (0, 7),
        (4, i64::MAX as u64),
        (4, u64::MAX),
    ])
    .then(|(size, start)| async move {
        let (endpoint, mut requests, server) = server(vec![
            response(416, "<Error><Code>InvalidRange</Code></Error>"),
            metadata_response(size, "\"eof-object\"", "object-version-A"),
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        let reference = reference("\"eof-object\"", Some("object-version-A"));
        let mut read = volume
            .read("key", &reference, Some(start..=u64::MAX))
            .await
            .unwrap();
        assert_eq!(read.version, reference);
        assert_eq!(read.length, 0);
        assert!(read.body.next().await.unwrap().is_none());
        ["GET", "HEAD"].into_iter().for_each(|method| {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.method(), method);
            assert_eq!(request.headers()["if-match"], "\"eof-object\"");
            assert!(
                request
                    .uri()
                    .query()
                    .unwrap()
                    .contains("versionId=object-version-A")
            );
            assert_eq!(request.headers().contains_key("range"), method == "GET");
            if method == "GET" {
                assert_eq!(
                    request.headers()["range"],
                    format!("bytes={}-9223372036854775807", start.min(i64::MAX as u64))
                );
            }
        });
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
    stream::iter([
        (
            4,
            metadata_response(5, "\"eof-object\"", "object-version-A"),
            VolumeError::InvalidResponse,
        ),
        (
            4,
            metadata_response(4, "\"changed\"", "object-version-A"),
            VolumeError::InvalidResponse,
        ),
        (
            4,
            metadata_response(4, "\"eof-object\"", "different-version"),
            VolumeError::InvalidResponse,
        ),
        {
            let mut head = metadata_response(4, "\"eof-object\"", "object-version-A");
            head.headers_mut().remove("content-length");
            *head.body_mut() = Body::from_stream(stream::empty::<Result<Bytes, std::io::Error>>());
            (4, head, VolumeError::InvalidResponse)
        },
        (4, response(412, ""), VolumeError::Conflict),
        (4, response(404, ""), VolumeError::NotFound),
        (4, response(403, ""), VolumeError::Denied),
        (
            i64::MAX as u64,
            metadata_response(4, "\"changed\"", "object-version-A"),
            VolumeError::InvalidResponse,
        ),
        (
            i64::MAX as u64,
            metadata_response(4, "\"eof-object\"", "different-version"),
            VolumeError::InvalidResponse,
        ),
        (
            u64::MAX,
            metadata_response(4, "\"changed\"", "object-version-A"),
            VolumeError::InvalidResponse,
        ),
        (
            u64::MAX,
            metadata_response(4, "\"eof-object\"", "different-version"),
            VolumeError::InvalidResponse,
        ),
    ])
    .then(|(start, head, expected)| async move {
        let (endpoint, mut requests, server) = server(vec![
            response(416, "<Error><Code>InvalidRange</Code></Error>"),
            head,
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        assert_eq!(
            volume
                .read(
                    "key",
                    &reference("\"eof-object\"", Some("object-version-A")),
                    Some(start..=u64::MAX)
                )
                .await
                .unwrap_err(),
            expected
        );
        ["GET", "HEAD"].into_iter().for_each(|method| {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.method(), method);
            assert_eq!(request.headers()["if-match"], "\"eof-object\"");
            assert!(
                request
                    .uri()
                    .query()
                    .unwrap()
                    .contains("versionId=object-version-A")
            );
            assert_eq!(request.headers().contains_key("range"), method == "GET");
            if method == "GET" {
                assert_eq!(
                    request.headers()["range"],
                    format!("bytes={}-9223372036854775807", start.min(i64::MAX as u64))
                );
            }
        });
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EofMutation {
    Replace,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EofOrder {
    BeforeGet,
    BetweenGetAndHead,
    AfterHeadObservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EofHead {
    Valid,
    Denied,
    Unanswered,
}

async fn finish_eof_mutation(mutation: &mut Option<(EofRequestGate, tokio::task::JoinHandle<()>)>) {
    if let Some((request, task)) = mutation.take() {
        let status = if request.request.method() == "PUT" {
            200
        } else {
            204
        };
        request.observe(status).await.send(()).unwrap();
        task.await.unwrap();
    }
}

#[test]
#[timeout("60s")]
async fn ranged_eof_protocol_sweeps_public_replace_delete_orders() {
    use EofHead::{Denied, Unanswered, Valid};
    use EofMutation::{Delete, Replace};
    use EofOrder::{AfterHeadObservation, BeforeGet, BetweenGetAndHead};

    let cases = [false, true]
        .into_iter()
        .flat_map(|versioned| {
            [None, Some(Replace), Some(Delete)]
                .into_iter()
                .flat_map(move |change| {
                    [BeforeGet, BetweenGetAndHead, AfterHeadObservation]
                        .into_iter()
                        .filter(move |order| change.is_some() || *order == BeforeGet)
                        .map(move |order| (versioned, change, order))
                })
        })
        .flat_map(|(versioned, change, order)| {
            [Valid, Denied, Unanswered]
                .into_iter()
                .map(move |head| (versioned, change, order, head))
        });
    stream::iter(cases)
        .then(|(versioned, change, order, head)| async move {
            let case = format!("versioned={versioned}, {change:?}, {order:?}, {head:?}");
            let (endpoint, mut requests, server) = gated_eof_store(versioned, head == Denied).await;
            let volume = Arc::new(client(&endpoint, None, allowed()));
            let object_reference = reference("\"eof-A\"", versioned.then_some("version-A"));
            let read_volume = volume.clone();
            let read_reference = object_reference.clone();
            let mut read =
                tokio::spawn(
                    async move { read_volume.read("key", &read_reference, Some(4..=9)).await },
                );
            let get = requests.recv().await.unwrap();
            assert_eq!(get.request.method(), "GET", "{case}");
            let mut mutation = if let Some(change) = change {
                let volume = volume.clone();
                let object_reference = object_reference.clone();
                let task = tokio::spawn(async move {
                    match change {
                        Replace => {
                            assert_eq!(
                                volume
                                    .put(
                                        WriteTarget::File("key"),
                                        WriteCondition::Matches(&object_reference.etag),
                                        Bytes::from_static(b"new!"),
                                    )
                                    .await
                                    .unwrap(),
                                reference("\"eof-B\"", versioned.then_some("version-B"))
                            );
                        }
                        Delete => {
                            let reference = ObjectMetadata {
                                key: "key".into(),
                                size: 4,
                                version: object_reference,
                            };
                            let marker = volume
                                .delete(WriteTarget::File("key"), &reference)
                                .await
                                .unwrap();
                            assert_eq!(
                                marker.as_ref().map(|marker| marker.payload().as_ref()),
                                versioned.then_some("delete-marker")
                            );
                        }
                    }
                });
                let request = requests.recv().await.unwrap();
                assert_eq!(
                    request.request.method(),
                    if change == Replace { "PUT" } else { "DELETE" },
                    "{case}"
                );
                Some((request, task))
            } else {
                None
            };
            let changed_status = if change == Some(Delete) { 404 } else { 412 };
            let changed_error = if change == Some(Delete) {
                VolumeError::NotFound
            } else {
                VolumeError::Conflict
            };
            if order == BeforeGet {
                finish_eof_mutation(&mut mutation).await;
            }
            let get_status = if !versioned && change.is_some() && order == BeforeGet {
                changed_status
            } else {
                416
            };
            get.observe(get_status).await.send(()).unwrap();
            if get_status != 416 {
                assert_eq!(read.await.unwrap().unwrap_err(), changed_error, "{case}");
            } else {
                let request = requests.recv().await.unwrap();
                assert_eq!(request.request.method(), "HEAD", "{case}");
                if order == BetweenGetAndHead {
                    finish_eof_mutation(&mut mutation).await;
                }
                let head_status = if head == Denied {
                    403
                } else if !versioned && change.is_some() && order == BetweenGetAndHead {
                    changed_status
                } else {
                    200
                };
                let reply = request.observe(head_status).await;
                // Keep HEAD's response blocked until the later public mutation has completed.
                finish_eof_mutation(&mut mutation).await;
                if head == Unanswered {
                    assert!(futures::poll!(&mut read).is_pending(), "{case}");
                    read.abort();
                    assert!(read.await.unwrap_err().is_cancelled(), "{case}");
                    drop(reply);
                } else {
                    reply.send(()).unwrap();
                    let result = read.await.unwrap();
                    if head_status == 200 {
                        let mut read = result.unwrap();
                        assert_eq!(read.version, object_reference, "{case}");
                        assert_eq!(read.length, 0, "{case}");
                        assert!(read.body.next().await.unwrap().is_none(), "{case}");
                    } else {
                        assert_eq!(
                            result.unwrap_err(),
                            if head == Denied {
                                VolumeError::Denied
                            } else {
                                changed_error
                            },
                            "{case}"
                        );
                    }
                }
            }
            assert!(requests.try_recv().is_err(), "{case}");
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn successful_object_results_require_a_usable_reference_etag() {
    stream::iter([
        None,
        Some(""),
        Some("*"),
        Some("W/\"weak\""),
        Some("\"one\", \"two\""),
        Some("unquoted"),
        Some("\"a b\""),
        Some("\"a\"b\""),
    ])
    .then(|etag| async move {
        let without_reference = || {
            let mut result = response(200, "body");
            result.headers_mut().remove("etag");
            result
                .headers_mut()
                .insert("x-amz-version-id", "null".parse().unwrap());
            if let Some(etag) = etag {
                result.headers_mut().insert("etag", etag.parse().unwrap());
            }
            result
        };
        let completion = etag
            .map(|etag| format!("<ETag>{etag}</ETag>").into_boxed_str())
            .unwrap_or_default();
        let (endpoint, mut requests, server) = server(vec![
            without_reference(),
            without_reference(),
            without_reference(),
            without_reference(),
            response(
                200,
                format!(
                    "<CompleteMultipartUploadResult>{completion}</CompleteMultipartUploadResult>"
                ),
            ),
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        assert!(matches!(
            volume
                .read("key", &reference("\"reference\"", None), None)
                .await,
            Err(VolumeError::InvalidResponse)
        ));
        assert_eq!(
            volume
                .put(
                    WriteTarget::File("key"),
                    WriteCondition::Absent,
                    Bytes::new()
                )
                .await,
            Err(VolumeError::RequestFailed)
        );
        assert_eq!(
            volume.stat("key", None).await,
            Err(VolumeError::InvalidResponse)
        );
        assert_eq!(
            volume
                .upload_part(WriteTarget::File("key"), "upload", 1, Bytes::new())
                .await,
            Err(VolumeError::InvalidResponse)
        );
        assert_eq!(
            volume
                .complete_multipart(
                    WriteTarget::File("key"),
                    "upload",
                    &[UploadedPart {
                        number: 1,
                        etag: "\"part\"".into(),
                        size: 1
                    }],
                    WriteCondition::Absent
                )
                .await,
            Err(VolumeError::RequestFailed)
        );
        ["GET", "PUT", "HEAD", "PUT", "POST"]
            .into_iter()
            .for_each(|method| assert_eq!(requests.try_recv().unwrap().method(), method));
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn list_filters_unaddressable_full_keys_without_limiting_search_prefixes() {
    stream::iter(["", "tenant/data/"])
        .then(|prefix| async move {
            let valid = format!(
                "{prefix}{}",
                "é".repeat(if prefix.is_empty() { 512 } else { 506 })
            );
            let overlong = format!("{valid}x");
            let (endpoint, mut requests, server) = server(vec![
                listing_response(&[(prefix, 7), (&valid, 3), (&overlong, 4)], None),
                listing_response(&[], None),
            ])
            .await;
            let volume = client_with_prefix(&endpoint, prefix, None, allowed());
            let page = volume.list("", None).await.unwrap();
            assert!(
                page.objects
                    .iter()
                    .map(|object| object.key.as_ref())
                    .eq((!prefix.is_empty())
                        .then_some("")
                        .into_iter()
                        .chain([valid.strip_prefix(prefix).unwrap()]))
            );
            assert!(requests.try_recv().is_ok());
            assert!(
                volume
                    .list(&"x".repeat(1025), None)
                    .await
                    .unwrap()
                    .objects
                    .is_empty()
            );
            let request = requests.try_recv().unwrap();
            let query = url::form_urlencoded::parse(request.uri().query().unwrap().as_bytes())
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(query["prefix"], format!("{prefix}{}", "x".repeat(1025)));
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn public_constructor_prefix_reaches_object_paths_list_decoding_and_probes() {
    stream::iter([
        (
            "",
            "/test-bucket/sub/file",
            "/test-bucket/.golem/probe",
            "sub/",
            "sub/file",
        ),
        (
            "tenant/data/",
            "/test-bucket/tenant/data/sub/file",
            "/test-bucket/tenant/data/.golem/probe",
            "tenant/data/sub/",
            "tenant/data/sub/file",
        ),
        (
            "tenant/%2F/",
            "/test-bucket/tenant/%252F/sub/file",
            "/test-bucket/tenant/%252F/.golem/probe",
            "tenant/%2F/sub/",
            "tenant/%2F/sub/file",
        ),
    ])
    .then(
        |(prefix, file_path, probe_path, list_prefix, full_key)| async move {
            let (endpoint, mut requests, server) = server(vec![
                response(200, ""),
                metadata_response(4, "\"reference\"", "version-7"),
                response(200, "body"),
                response(204, ""),
                response(200, ""),
                response(204, ""),
                listing_response(&[(full_key, 4)], None),
            ])
            .await;
            let volume = client_with_prefix(&endpoint, prefix, None, allowed());
            let created = volume
                .put(
                    WriteTarget::File("sub/file"),
                    WriteCondition::Absent,
                    Bytes::from_static(b"body"),
                )
                .await
                .unwrap();
            let metadata = volume.stat("sub/file", None).await.unwrap();
            assert_eq!(metadata.key.as_ref(), "sub/file");
            assert_eq!(
                volume
                    .read("sub/file", &created, None)
                    .await
                    .unwrap()
                    .body
                    .collect()
                    .await
                    .unwrap(),
                "body"
            );
            volume
                .delete(WriteTarget::File("sub/file"), &metadata)
                .await
                .unwrap();
            let probe = volume
                .put(
                    WriteTarget::Probe("probe"),
                    WriteCondition::Absent,
                    Bytes::from_static(b"body"),
                )
                .await
                .unwrap();
            volume
                .delete(
                    WriteTarget::Probe("probe"),
                    &ObjectMetadata {
                        key: ".golem/probe".into(),
                        size: 4,
                        version: probe,
                    },
                )
                .await
                .unwrap();
            let page = volume.list("sub/", None).await.unwrap();
            assert_eq!(page.objects[0].key.as_ref(), "sub/file");
            ["PUT", "HEAD", "GET", "DELETE", "PUT", "DELETE"]
                .into_iter()
                .enumerate()
                .for_each(|(index, method)| {
                    let request = requests.try_recv().unwrap();
                    assert_eq!(request.method(), method);
                    assert_eq!(
                        request.uri().path(),
                        if index < 4 { file_path } else { probe_path }
                    );
                });
            let request = requests.try_recv().unwrap();
            assert_eq!(request.uri().path(), "/test-bucket/");
            let query = url::form_urlencoded::parse(request.uri().query().unwrap().as_bytes())
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(query["prefix"], list_prefix);
            assert_eq!(query["encoding-type"], "url");
            assert!(requests.try_recv().is_err());
            server.abort();
        },
    )
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn list_decodes_keys_exactly_once_without_decoding_tokens_or_etags() {
    let contents = [
        "corpus%2Fsub%2Fa%01b",
        "corpus%2Fsub%2F%252E%252E%2Fsafe",
        "corpus%2Fsub%2F%25",
        "corpus%2Fsub%2F%252G",
        "corpus%2Fsub%2Fa+b%2Bc",
        "corpus%2Fsub%2Fcaf%C3%A9",
    ]
    .into_iter()
    .map(|key| {
        format!(
            "<Contents><Key>{key}</Key><Size>3</Size><ETag>&quot;%2F+tag&quot;</ETag></Contents>"
        )
    })
    .collect::<String>();
    let (endpoint, mut requests, server) = server(vec![response(200, format!("<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>next%2F+opaque/token</NextContinuationToken>{contents}</ListBucketResult>")), listing_response(&[], None)]).await;
    let volume = client(&endpoint, None, allowed());
    let page = volume.list("sub/", None).await.unwrap();
    assert!(page.objects.iter().map(|object| object.key.as_ref()).eq([
        "sub/a\u{1}b",
        "sub/%2E%2E/safe",
        "sub/%",
        "sub/%2G",
        "sub/a+b+c",
        "sub/café",
    ]));
    assert!(
        page.objects
            .iter()
            .all(|object| object.etag.as_ref() == "\"%2F+tag\"")
    );
    assert_eq!(page.continuation.as_deref(), Some("next%2F+opaque/token"));
    let first = requests.try_recv().unwrap();
    assert!(first.uri().query().unwrap().contains("encoding-type=url"));
    volume
        .list("sub/", page.continuation.as_deref())
        .await
        .unwrap();
    assert!(
        requests
            .try_recv()
            .unwrap()
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=next%252F%2Bopaque%2Ftoken")
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn list_rejects_bad_escapes_and_keys_outside_the_requested_string_prefix() {
    stream::iter(["corpus%2Fsub%2F%", "corpus%2Fsub%2F%2", "corpus%2Fsub%2F%GG", "corpus%2Fsub%2F%FF", "corpus%2Fsub%2F%C0%AF", "corpus%2Fother%2Ffile", "corpus%2Fother%2Fdir%2F"])
        .then(|key| async move {
            let (endpoint, mut requests, server) = server(vec![response(200, format!("<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>false</IsTruncated><Contents><Key>{key}</Key><Size>0</Size><ETag>&quot;tag&quot;</ETag></Contents></ListBucketResult>"))]).await;
            assert_eq!(client(&endpoint, None, allowed()).list("sub/", None).await.unwrap_err(), VolumeError::InvalidResponse, "{key}");
            assert!(requests.try_recv().is_ok());
            assert!(requests.try_recv().is_err());
            server.abort();
        }).collect::<()>().await;
    let (endpoint, mut requests, server) = server(vec![listing_response(
        &[("corpus/submarine/file", 2)],
        None,
    )])
    .await;
    assert_eq!(
        client(&endpoint, None, allowed())
            .list("sub", None)
            .await
            .unwrap()
            .objects[0]
            .key
            .as_ref(),
        "submarine/file"
    );
    assert!(requests.try_recv().is_ok());
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn list_requires_explicit_encoding_and_consistent_progress_metadata() {
    let invalid = [
        "<IsTruncated>false</IsTruncated>",
        "<EncodingType></EncodingType><IsTruncated>false</IsTruncated>",
        "<EncodingType>unknown</EncodingType><IsTruncated>false</IsTruncated>",
        "<EncodingType>url</EncodingType>",
        "<EncodingType>url</EncodingType><NextContinuationToken>next</NextContinuationToken>",
        "<EncodingType>url</EncodingType><IsTruncated>true</IsTruncated>",
        "<EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken></NextContinuationToken>",
        "<EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>same%2F+token</NextContinuationToken>",
        "<EncodingType>url</EncodingType><IsTruncated>false</IsTruncated><NextContinuationToken>next</NextContinuationToken>",
        "<EncodingType>url</EncodingType><IsTruncated>false</IsTruncated><NextContinuationToken></NextContinuationToken>",
    ];
    stream::iter(invalid.into_iter().flat_map(|metadata| ["", "<Contents><Key>corpus%2Fdir%2F</Key><Size>0</Size><ETag>&quot;tag&quot;</ETag></Contents>"].into_iter().map(move |contents| (metadata, contents))))
        .then(|(metadata, contents)| async move {
            let (endpoint, mut requests, server) = server(vec![response(200, format!("<ListBucketResult>{metadata}{contents}</ListBucketResult>"))]).await;
            assert_eq!(client(&endpoint, None, allowed()).list("", Some("same%2F+token")).await.unwrap_err(), VolumeError::InvalidResponse, "{metadata}");
            assert!(requests.try_recv().is_ok());
            assert!(requests.try_recv().is_err());
            server.abort();
        }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn list_returns_page_metadata_without_per_object_requests() {
    let (endpoint, mut requests, server) = server(vec![response(200,
        "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>false</IsTruncated><Contents><Key>corpus/sub/first</Key><Size>4</Size><ETag>&quot;page-A&quot;</ETag></Contents><Contents><Key>corpus/sub/second</Key><Size>19</Size><ETag>&quot;page-B&quot;</ETag></Contents></ListBucketResult>"
    )]).await;
    let volume = client(&endpoint, None, allowed());
    let page = volume.list("sub/", None).await.unwrap();
    assert_eq!(page.objects.len(), 2);
    assert_eq!(page.objects[0].key.as_ref(), "sub/first");
    assert_eq!(page.objects[0].size, 4);
    assert_eq!(page.objects[0].etag.as_ref(), "\"page-A\"");
    assert_eq!(page.objects[1].key.as_ref(), "sub/second");
    assert_eq!(page.objects[1].size, 19);
    assert_eq!(page.objects[1].etag.as_ref(), "\"page-B\"");
    assert!(page.continuation.is_none());
    let request = requests.try_recv().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.uri().path(), "/test-bucket/");
    assert_eq!(
        request.uri().query().unwrap(),
        "list-type=2&encoding-type=url&prefix=corpus%2Fsub%2F"
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn list_keeps_empty_and_marker_only_pages_without_scanning() {
    let (endpoint, mut requests, server) = server(vec![
        listing_response(&[], Some("empty+page/token")),
        listing_response(&[("corpus/dir/", 0)], Some("marker+page/token")),
        listing_response(&[("corpus/empty-file", 0), ("corpus/nonempty/", 9)], None),
    ])
    .await;
    let volume = client(&endpoint, None, allowed());
    let empty = volume.list("", None).await.unwrap();
    assert!(empty.objects.is_empty());
    assert_eq!(empty.continuation.as_deref(), Some("empty+page/token"));
    let first = requests.try_recv().unwrap();
    assert_eq!(first.method(), "GET");
    assert_eq!(first.uri().path(), "/test-bucket/");
    assert_eq!(
        first.uri().query().unwrap(),
        "list-type=2&encoding-type=url&prefix=corpus%2F"
    );
    assert!(requests.try_recv().is_err());
    let marker = volume
        .list("", empty.continuation.as_deref())
        .await
        .unwrap();
    assert!(marker.objects.is_empty());
    assert_eq!(marker.continuation.as_deref(), Some("marker+page/token"));
    let second = requests.try_recv().unwrap();
    assert_eq!(second.method(), "GET");
    assert_eq!(
        second.uri().query().unwrap(),
        "list-type=2&encoding-type=url&prefix=corpus%2F&continuation-token=empty%2Bpage%2Ftoken"
    );
    assert!(requests.try_recv().is_err());
    let files = volume
        .list("", marker.continuation.as_deref())
        .await
        .unwrap();
    assert_eq!(
        files.objects.as_ref(),
        [
            ListedObject {
                key: "empty-file".into(),
                size: 0,
                etag: "\"listed-A\"".into()
            },
            ListedObject {
                key: "nonempty/".into(),
                size: 9,
                etag: "\"listed-A\"".into()
            },
        ]
    );
    assert!(files.continuation.is_none());
    let third = requests.try_recv().unwrap();
    assert_eq!(third.method(), "GET");
    assert_eq!(
        third.uri().query().unwrap(),
        "list-type=2&encoding-type=url&prefix=corpus%2F&continuation-token=marker%2Bpage%2Ftoken"
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn list_hides_only_zero_byte_prefix_root_markers() {
    stream::iter([0, 9])
        .then(|size| async move {
            let (endpoint, mut requests, server) =
                server(vec![listing_response(&[("corpus/", size)], None)]).await;
            let volume = client(&endpoint, None, allowed());
            let page = volume.list("", None).await.unwrap();
            if size == 0 {
                assert!(page.objects.is_empty());
            } else {
                assert_eq!(
                    page.objects.as_ref(),
                    [ListedObject {
                        key: "".into(),
                        size: 9,
                        etag: "\"listed-A\"".into(),
                    }]
                );
            }
            assert!(page.continuation.is_none());
            assert_eq!(requests.try_recv().unwrap().method(), "GET");
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn list_rejects_incomplete_or_invalid_page_metadata_in_either_order() {
    let invalid = [
        "<Size>4</Size><ETag>&quot;tag&quot;</ETag>",
        "<Key>corpus/key</Key><ETag>&quot;tag&quot;</ETag>",
        "<Key>corpus/key</Key><Size>-1</Size><ETag>&quot;tag&quot;</ETag>",
        "<Key>corpus/key</Key><Size>4</Size>",
        "<Key>corpus/key</Key><Size>4</Size><ETag></ETag>",
        "<Key>corpus/key</Key><Size>4</Size><ETag>tag</ETag>",
        "<Key>corpus/key</Key><Size>4</Size><ETag>W/&quot;tag&quot;</ETag>",
        "<Key>corpus/key</Key><Size>4</Size><ETag>&quot;tag with space&quot;</ETag>",
        "<Key>corpus/dir/</Key><Size>-1</Size><ETag>&quot;tag&quot;</ETag>",
    ];
    stream::iter(invalid.into_iter().flat_map(|invalid| [false, true].into_iter().map(move |first| (invalid, first))))
        .then(|(invalid, first)| async move {
            let valid = "<Contents><Key>corpus/other</Key><Size>7</Size><ETag>&quot;valid&quot;</ETag></Contents>";
            let contents = if first {
                format!("<Contents>{invalid}</Contents>{valid}").into_boxed_str()
            } else {
                format!("{valid}<Contents>{invalid}</Contents>").into_boxed_str()
            };
            let (endpoint, mut requests, server) = server(vec![response(200, format!("<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>unused-page</NextContinuationToken>{contents}</ListBucketResult>"))]).await;
            let volume = client(&endpoint, None, allowed());
            assert_eq!(volume.list("", None).await.unwrap_err(), VolumeError::InvalidResponse, "{invalid}, first={first}");
            assert_eq!(requests.try_recv().unwrap().method(), "GET");
            assert!(requests.try_recv().is_err());
            server.abort();
        }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn list_preserves_page_request_errors_without_retrying() {
    stream::iter([
        (404, VolumeError::NotFound),
        (403, VolumeError::Denied),
        (503, VolumeError::RequestFailed),
    ])
    .then(|(status, expected)| async move {
        let (endpoint, mut requests, server) = server(vec![response(status, "<Error/>")]).await;
        let volume = client(&endpoint, None, allowed());
        assert_eq!(volume.list("", None).await.unwrap_err(), expected);
        assert_eq!(requests.try_recv().unwrap().method(), "GET");
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn list_rejects_outside_keys_even_when_they_look_like_markers() {
    stream::iter(["other/dir/", "corpus2/dir/"])
        .then(|key| async move {
            let (endpoint, mut requests, server) =
                server(vec![listing_response(&[(key, 0)], None)]).await;
            let volume = client(&endpoint, None, allowed());
            assert_eq!(
                volume.list("", None).await.unwrap_err(),
                VolumeError::InvalidResponse
            );
            assert_eq!(requests.recv().await.unwrap().method(), "GET");
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn list_hides_unrepresentable_paths_and_keeps_the_page_token() {
    stream::iter([false, true]).then(|invalid_first| async move {
        let invalid = "<Contents><Key>corpus//x</Key></Contents><Contents><Key>corpus/a/../b</Key><Size>-1</Size></Contents><Contents><Key>corpus/./c</Key></Contents>";
        let valid = "<Contents><Key>corpus/visible</Key><Size>7</Size><ETag>&quot;visible-A&quot;</ETag></Contents>";
        let contents = if invalid_first { format!("{invalid}{valid}") } else { format!("{valid}{invalid}") };
        let (endpoint, mut requests, server) = server(vec![
            response(200, format!("<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>page-B</NextContinuationToken>{contents}</ListBucketResult>")),
            response(200, format!("<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>page-C</NextContinuationToken>{invalid}</ListBucketResult>")),
            listing_response(&[("corpus/last", 0)], None),
        ]).await;
        let volume = client(&endpoint, None, allowed());
        let first = volume.list("", None).await.unwrap();
        assert_eq!(first.objects.as_ref(), [ListedObject { key: "visible".into(), size: 7, etag: "\"visible-A\"".into() }]);
        assert_eq!(first.continuation.as_deref(), Some("page-B"));
        let second = volume.list("", first.continuation.as_deref()).await.unwrap();
        assert!(second.objects.is_empty());
        assert_eq!(second.continuation.as_deref(), Some("page-C"));
        let third = volume.list("", second.continuation.as_deref()).await.unwrap();
        assert_eq!(third.objects.len(), 1);
        assert_eq!(third.objects[0].key.as_ref(), "last");
        assert!(third.continuation.is_none());
        assert_eq!(requests.try_recv().unwrap().method(), "GET");
        ["page-B", "page-C"].into_iter().for_each(|token| {
            assert!(requests.try_recv().unwrap().uri().query().unwrap().contains(&format!("continuation-token={token}")));
        });
        assert!(requests.try_recv().is_err());
        server.abort();
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn reads_keep_versions_ranges_and_hide_only_zero_byte_markers() {
    stream::iter([
        ("empty-file", "", true),
        ("nonempty/", "slash content", true),
        ("empty-marker/", "", false),
    ])
    .then(|(key, body, visible)| async move {
        let (endpoint, mut requests, server) = server(vec![
            metadata_response(body.len() as u64, "\"reference\"", "version-7"),
            response(200, body),
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        let stat = volume.stat(key, None).await;
        let read = volume
            .read(key, &reference("\"reference\"", Some("version-7")), None)
            .await;
        if visible {
            let stat = stat.unwrap();
            assert_eq!(stat.key.as_ref(), key);
            assert_eq!(stat.size, body.len() as u64);
            let read = read.unwrap();
            assert_eq!(read.length, body.len() as u64);
            assert_eq!(read.version, stat.version);
            assert_eq!(read.body.collect().await.unwrap(), body);
        } else {
            assert_eq!(stat, Err(VolumeError::NotFound));
            assert!(matches!(read, Err(VolumeError::NotFound)));
        }
        ["HEAD", "GET"]
            .into_iter()
            .for_each(|method| assert_eq!(requests.try_recv().unwrap().method(), method));
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
    {
        let (endpoint, mut requests, server) = server(vec![response(
            200,
            "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated></ListBucketResult>",
        )])
        .await;
        let volume = client(&endpoint, None, allowed());
        assert_eq!(
            volume.list("", None).await.unwrap_err(),
            VolumeError::InvalidResponse
        );
        assert_eq!(requests.try_recv().unwrap().method(), "GET");
        assert!(requests.try_recv().is_err());
        server.abort();
    }
    let mut head = response(200, "");
    head.headers_mut()
        .insert("content-length", "12".parse().unwrap());
    head.headers_mut()
        .insert("x-amz-version-id", "null".parse().unwrap());
    let mut ranged = response(206, "cde");
    ranged
        .headers_mut()
        .insert("content-range", "bytes 2-4/12".parse().unwrap());
    ranged
        .headers_mut()
        .insert("x-amz-version-id", "old+version".parse().unwrap());
    ranged
        .headers_mut()
        .insert("etag", "\"old\"".parse().unwrap());
    let (endpoint, mut requests, server) = server(vec![head, ranged]).await;
    let volume = client(&endpoint, None, allowed());
    let head = volume.stat("key", None).await.unwrap();
    assert_eq!(head.size, 12);
    assert_eq!(head.version.version_id, None);
    assert_eq!(head.version.etag.as_ref(), "\"reference\"");
    requests.recv().await.unwrap();
    let read = volume
        .read(
            "key",
            &reference("\"old\"", Some("old+version")),
            Some(2..=4),
        )
        .await
        .unwrap();
    assert_eq!(read.length, 3);
    assert_eq!(read.version.version_id.as_deref(), Some("old+version"));
    assert_eq!(read.body.collect().await.unwrap(), "cde");
    let request = requests.recv().await.unwrap();
    assert!(
        request
            .uri()
            .query()
            .unwrap()
            .contains("versionId=old%2Bversion")
    );
    assert_eq!(request.headers()["range"], "bytes=2-4");
    assert_eq!(request.headers()["if-match"], "\"old\"");
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn multipart_completion_is_conditional_and_probe_keys_stay_reserved() {
    {
        let completed = || {
            response(
                200,
                "<CompleteMultipartUploadResult><ETag>&quot;complete&quot;</ETag></CompleteMultipartUploadResult>",
            )
        };
        let uploaded = |etag: &'static str| {
            let mut result = response(200, "");
            result.headers_mut().insert("etag", etag.parse().unwrap());
            result
        };
        let (endpoint, mut requests, server) = server(vec![
            uploaded("\"first\""),
            uploaded("\"last\""),
            completed(),
            completed(),
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        stream::iter([0, 10001])
            .then(|number| {
                let volume = &volume;
                async move {
                    assert_eq!(
                        volume
                            .upload_part(WriteTarget::File("large"), "upload", number, Bytes::new())
                            .await,
                        Err(VolumeError::InvalidRequest)
                    );
                }
            })
            .collect::<()>()
            .await;
        let invalid: [&[i32]; 6] = [&[], &[0], &[10001], &[-1], &[1, 1], &[2, 1]];
        stream::iter(invalid)
            .then(|numbers| {
                let volume = &volume;
                async move {
                    let parts = numbers
                        .iter()
                        .map(|number| UploadedPart {
                            number: *number,
                            etag: "\"part\"".into(),
                            size: 1,
                        })
                        .collect::<Box<[_]>>();
                    assert_eq!(
                        volume
                            .complete_multipart(
                                WriteTarget::File("large"),
                                "upload",
                                &parts,
                                WriteCondition::Absent
                            )
                            .await,
                        Err(VolumeError::InvalidRequest)
                    );
                }
            })
            .collect::<()>()
            .await;
        let zero = [UploadedPart {
            number: 1,
            etag: "\"zero\"".into(),
            size: 0,
        }];
        assert_eq!(
            volume
                .complete_multipart(
                    WriteTarget::File("slash/"),
                    "upload",
                    &zero,
                    WriteCondition::Absent
                )
                .await,
            Err(VolumeError::InvalidRequest)
        );
        stream::iter([
            "",
            ".golem",
            ".golem/unsafe",
            "../escape",
            "/absolute",
            "dir/../escape",
            "dir/./escape",
        ])
        .then(|key| {
            let volume = &volume;
            let zero = &zero;
            async move {
                let target = WriteTarget::File(key);
                assert_eq!(
                    volume
                        .put(target, WriteCondition::Absent, Bytes::from_static(b"body"))
                        .await,
                    Err(VolumeError::InvalidRequest)
                );
                assert_eq!(
                    volume.start_multipart(target).await,
                    Err(VolumeError::InvalidRequest)
                );
                assert_eq!(
                    volume
                        .complete_multipart(target, "upload", zero, WriteCondition::Absent)
                        .await,
                    Err(VolumeError::InvalidRequest)
                );
            }
        })
        .collect::<()>()
        .await;
        stream::iter(["../escape", "/absolute", "dir/./escape"])
            .then(|key| {
                let volume = &volume;
                async move {
                    assert_eq!(
                        volume.stat(key, None).await,
                        Err(VolumeError::InvalidRequest)
                    );
                    assert!(matches!(
                        volume
                            .read(key, &reference("\"reference\"", Some("version-7")), None)
                            .await,
                        Err(VolumeError::InvalidRequest)
                    ));
                    assert!(matches!(
                        volume.list(key, None).await,
                        Err(VolumeError::InvalidRequest)
                    ));
                }
            })
            .collect::<()>()
            .await;
        assert!(requests.try_recv().is_err());
        let first = volume
            .upload_part(
                WriteTarget::File("large"),
                "upload",
                1,
                Bytes::from_static(b"part"),
            )
            .await
            .unwrap();
        let last = volume
            .upload_part(WriteTarget::File("large"), "upload", 10000, Bytes::new())
            .await
            .unwrap();
        let parts = [first, last];
        assert_eq!(parts[0].number, 1);
        assert_eq!(parts[0].size, 4);
        assert_eq!(parts[0].etag.as_ref(), "\"first\"");
        assert_eq!(parts[1].number, 10000);
        assert_eq!(parts[1].size, 0);
        assert_eq!(parts[1].etag.as_ref(), "\"last\"");
        ["partNumber=1", "partNumber=10000"]
            .into_iter()
            .for_each(|parameter| {
                let request = requests.try_recv().unwrap();
                assert!(
                    request
                        .uri()
                        .query()
                        .unwrap()
                        .split('&')
                        .any(|part| part == parameter)
                );
            });
        assert_eq!(
            volume
                .complete_multipart(
                    WriteTarget::File("large"),
                    "upload",
                    &parts,
                    WriteCondition::Absent
                )
                .await
                .unwrap()
                .etag
                .as_ref(),
            "\"complete\""
        );
        let request = requests.try_recv().unwrap();
        assert_eq!(request.headers()["if-none-match"], "*");
        let xml = std::str::from_utf8(request.body()).unwrap();
        assert!(xml.contains("<PartNumber>1</PartNumber>"));
        assert!(xml.contains("<PartNumber>10000</PartNumber>"));
        assert_eq!(xml.matches("<Part>").count(), 2);
        xml.split("<Part>")
            .skip(1)
            .zip([
                (
                    "<ETag>&quot;first&quot;</ETag>",
                    "<PartNumber>1</PartNumber>",
                ),
                (
                    "<ETag>&quot;last&quot;</ETag>",
                    "<PartNumber>10000</PartNumber>",
                ),
            ])
            .for_each(|(part, (etag, number))| {
                let part = part.split("</Part>").next().unwrap();
                assert!(part.contains(etag));
                assert!(part.contains(number));
            });
        assert_eq!(
            volume
                .complete_multipart(
                    WriteTarget::File("slash/"),
                    "upload",
                    &parts,
                    WriteCondition::Absent
                )
                .await
                .unwrap()
                .etag
                .as_ref(),
            "\"complete\""
        );
        assert_eq!(
            requests.try_recv().unwrap().uri().path(),
            "/test-bucket/corpus/slash/"
        );
        assert!(requests.try_recv().is_err());
        server.abort();
    }
    let mut completed = response(
        200,
        "<CompleteMultipartUploadResult><ETag>&quot;complete&quot;</ETag></CompleteMultipartUploadResult>",
    );
    completed
        .headers_mut()
        .insert("x-amz-version-id", "v2".parse().unwrap());
    let (endpoint, mut requests, server) = server(vec![
        response(200, ""),
        completed,
        response(204, ""),
        response(200, ""),
        response(204, ""),
    ])
    .await;
    let volume = client(&endpoint, None, allowed());
    let part = volume
        .upload_part(
            WriteTarget::File("large"),
            "id+1",
            3,
            Bytes::from_static(b"part"),
        )
        .await
        .unwrap();
    assert_eq!(part.etag.as_ref(), "\"reference\"");
    assert_eq!(part.size, 4);
    let request = requests.recv().await.unwrap();
    assert!(request.uri().query().unwrap().contains("partNumber=3"));
    assert!(request.uri().query().unwrap().contains("uploadId=id%2B1"));
    let result = volume
        .complete_multipart(
            WriteTarget::File("large"),
            "id+1",
            &[part],
            WriteCondition::Matches("\"old\""),
        )
        .await
        .unwrap();
    assert_eq!(result.etag.as_ref(), "\"complete\"");
    assert_eq!(result.version_id.as_deref(), Some("v2"));
    let request = requests.recv().await.unwrap();
    assert_eq!(request.headers()["if-match"], "\"old\"");
    assert!(
        std::str::from_utf8(request.body())
            .unwrap()
            .contains("<PartNumber>3</PartNumber>")
    );
    volume
        .abort_multipart(WriteTarget::File("large"), "id+1")
        .await
        .unwrap();
    let aborted = requests.try_recv().unwrap();
    assert_eq!(aborted.method(), "DELETE");
    assert_eq!(aborted.uri().path(), "/test-bucket/corpus/large");
    assert!(aborted.uri().query().unwrap().contains("uploadId=id%2B1"));
    assert!(requests.try_recv().is_err());
    volume
        .put(
            WriteTarget::Probe("check-1"),
            WriteCondition::Absent,
            Bytes::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        requests.recv().await.unwrap().uri().path(),
        "/test-bucket/corpus/.golem/check-1"
    );
    volume
        .delete(
            WriteTarget::Probe("check-1"),
            &metadata(".golem/check-1", "\"reference\"", 0),
        )
        .await
        .unwrap();
    assert_eq!(
        requests.recv().await.unwrap().uri().path(),
        "/test-bucket/corpus/.golem/check-1"
    );
    stream::iter([
        ".golem/unsafe",
        "../escape",
        "/absolute",
        "dir/../escape",
        "dir/",
    ])
    .then(|key| async {
        assert_eq!(
            volume
                .put(WriteTarget::File(key), WriteCondition::Absent, Bytes::new())
                .await,
            Err(VolumeError::InvalidRequest)
        );
    })
    .collect::<()>()
    .await;
    assert_eq!(
        volume
            .delete(WriteTarget::File("key"), &metadata("key", "*", 0))
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn versioning_debug_keeps_absent_known_and_unknown_status_values() {
    stream::iter([
        None,
        Some("Enabled"),
        Some("Suspended"),
        Some("volume-secret-volume-token"),
    ])
    .then(|status| async move {
        let body = status
            .map(|status| {
                format!(
                    "<VersioningConfiguration><Status>{status}</Status></VersioningConfiguration>"
                )
                .into_boxed_str()
            })
            .unwrap_or_else(|| "<VersioningConfiguration/>".into());
        let (endpoint, mut requests, server) =
            server(vec![response(200, body.into_string())]).await;
        let volume = client(&endpoint, None, allowed());
        let result = volume.versioning().await;
        let expected = match status {
            None => BucketVersioning::Unversioned,
            Some("Enabled") => BucketVersioning::Enabled,
            Some("Suspended") => BucketVersioning::Suspended,
            Some(status) => BucketVersioning::Unknown(ResponseValue::new(status.into())),
        };
        assert_eq!(result.as_ref().unwrap(), &expected);
        assert_eq!(requests.recv().await.unwrap().method(), "GET");
        assert!(requests.try_recv().is_err());
        server.abort();
        assert_opaque_debug(&result);
        assert_opaque_debug(result.as_ref().unwrap());
        if let BucketVersioning::Unknown(value) = result.unwrap() {
            assert_opaque_debug(&value);
            assert_eq!(Some(value.into_payload().as_ref()), status);
        }
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn bucket_checks_return_versioning_and_noncurrent_lifecycle_rules_without_probes() {
    let expected = LifecycleRule {
        status: LifecycleRuleStatus::Unknown(ResponseValue::new(
            "volume-secret-volume-token".into(),
        )),
        prefix: Some(ResponseValue::new(
            "corpus/volume-secret/volume-token/".into(),
        )),
        noncurrent_expiration: Some(NoncurrentExpiration {
            days: 7,
            newer_versions: Some(6),
        }),
    };
    let (endpoint, mut requests, server) = server(vec![
        response(200, "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>"),
        response(200, "<LifecycleConfiguration><Rule>
            <ID>volume-secret-volume-token</ID><Status>volume-secret-volume-token</Status>
            <Filter><And><Prefix>corpus/volume-secret/volume-token/</Prefix>
                <Tag><Key>volume-secret</Key><Value>volume-token</Value></Tag>
                <Tag><Key>second</Key><Value>tag</Value></Tag>
                <ObjectSizeGreaterThan>2</ObjectSizeGreaterThan><ObjectSizeLessThan>900</ObjectSizeLessThan>
            </And></Filter>
            <Expiration><Days>30</Days></Expiration>
            <Transition><Days>3</Days><StorageClass>volume-secret</StorageClass></Transition>
            <Transition><Days>5</Days><StorageClass>GLACIER</StorageClass></Transition>
            <NoncurrentVersionTransition><NoncurrentDays>2</NoncurrentDays><NewerNoncurrentVersions>5</NewerNoncurrentVersions><StorageClass>volume-token</StorageClass></NoncurrentVersionTransition>
            <NoncurrentVersionExpiration><NoncurrentDays>7</NoncurrentDays><NewerNoncurrentVersions>6</NewerNoncurrentVersions></NoncurrentVersionExpiration>
            <AbortIncompleteMultipartUpload><DaysAfterInitiation>4</DaysAfterInitiation></AbortIncompleteMultipartUpload>
        </Rule></LifecycleConfiguration>"),
        response(200, "<LifecycleConfiguration/>"),
        response(404, "<Error><Code>NoSuchLifecycleConfiguration</Code></Error>"),
        response(403, "<Error><Code>AccessDenied</Code></Error>"),
        response(403, "<Error><Code>AccessDenied</Code></Error>"),
        response(404, "<Error><Code>NoSuchBucket</Code></Error>"),
        response(503, "<Error><Code>SlowDown</Code></Error>"),
    ]).await;
    let volume = client(&endpoint, None, allowed());
    assert_eq!(
        volume.versioning().await.unwrap(),
        BucketVersioning::Enabled
    );
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    let result = volume.lifecycle().await;
    let rules = result.as_ref().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0], expected);
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert!(volume.lifecycle().await.unwrap().is_empty());
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert!(volume.lifecycle().await.unwrap().is_empty());
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert_eq!(volume.versioning().await, Err(VolumeError::Denied));
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    stream::iter([
        VolumeError::Denied,
        VolumeError::NotFound,
        VolumeError::RequestFailed,
    ])
    .then(|expected| {
        let volume = &volume;
        async move {
            assert_eq!(volume.lifecycle().await, Err(expected));
        }
    })
    .collect::<()>()
    .await;
    [0; 3].into_iter().for_each(|_| {
        let request = requests.try_recv().unwrap();
        assert_eq!(request.method(), "GET");
        assert!(
            request
                .uri()
                .query()
                .unwrap()
                .split('&')
                .any(|part| part.starts_with("lifecycle"))
        );
    });
    assert!(requests.try_recv().is_err());
    server.abort();
    assert_opaque_debug(&(&result, rules, &rules[0], Some(&rules[0])));
    rules.iter().for_each(assert_opaque_debug);
    let rules = result.unwrap();
    assert_eq!(rules.as_ref(), [expected]);
}

#[test]
#[timeout("30s")]
async fn connections_and_redirects_cannot_reach_protected_endpoints() {
    let (endpoint, mut requests, server) = server(vec![
        Response::builder()
            .status(307)
            .header("location", "http://169.254.169.254/latest/meta-data/")
            .body(Body::empty())
            .unwrap(),
    ])
    .await;
    let default = client(&endpoint, None, ConnectionPolicy::new(Box::new([])));
    assert!(default.stat("key", None).await.is_err());
    let protected = client(
        &endpoint,
        None,
        ConnectionPolicy {
            protected_endpoints: [endpoint.parse().unwrap()].into(),
            ..allowed()
        },
    );
    assert!(protected.stat("key", None).await.is_err());
    assert!(requests.try_recv().is_err());
    let volume = client(&endpoint, None, allowed());
    assert!(volume.stat("key", None).await.is_err());
    requests.recv().await.unwrap();
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn eof_checks_null_versions_and_preserves_folder_marker_results() {
    stream::iter([
        ("key", None, "null", 0, None),
        ("key", Some("null"), "null", 0, None),
        (
            "key",
            None,
            "new-version",
            0,
            Some(VolumeError::InvalidResponse),
        ),
        ("dir/", None, "null", 0, Some(VolumeError::NotFound)),
        ("dir/", None, "null", 4, None),
    ])
    .then(|(key, requested, actual, size, expected)| async move {
        let (endpoint, mut requests, server) = server(vec![
            response(416, "<Error><Code>InvalidRange</Code></Error>"),
            metadata_response(size, "\"null-reference\"", actual),
        ])
        .await;
        let volume = client(&endpoint, None, allowed());
        let read = volume
            .read(
                key,
                &reference("\"null-reference\"", requested),
                Some(4..=9),
            )
            .await;
        if let Some(expected) = expected {
            assert_eq!(read.unwrap_err(), expected);
        } else {
            let read = read.unwrap();
            assert_eq!(read.version.version_id, None);
            assert_eq!(read.length, 0);
            assert!(read.body.collect().await.unwrap().is_empty());
        }
        ["GET", "HEAD"].into_iter().for_each(|method| {
            let request = requests.try_recv().unwrap();
            assert_eq!(request.method(), method);
            assert_eq!(
                request
                    .uri()
                    .query()
                    .unwrap_or_default()
                    .contains("versionId=null"),
                requested.is_some()
            );
        });
        assert!(requests.try_recv().is_err());
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn lifecycle_projection_keeps_prefixes_unknown_status_and_expiration_evidence() {
    let (endpoint, mut requests, task) = server(vec![response(200, "<LifecycleConfiguration>
        <Rule><Status>Enabled</Status><Prefix>corp</Prefix><NoncurrentVersionExpiration><NoncurrentDays>2</NoncurrentDays></NoncurrentVersionExpiration></Rule>
        <Rule><Status>Disabled</Status><Filter><Prefix>corpus/sub/</Prefix></Filter></Rule>
        <Rule><Status>volume-secret</Status><Filter><And><Prefix>corpus/..</Prefix><Tag><Key>scope</Key><Value>x</Value></Tag></And></Filter><NoncurrentVersionExpiration><NoncurrentDays>60</NoncurrentDays><NewerNoncurrentVersions>100</NewerNoncurrentVersions></NoncurrentVersionExpiration></Rule>
        <Rule><Status>Enabled</Status><Filter><Tag><Key>scope</Key><Value>x</Value></Tag></Filter><Expiration><Days>3</Days></Expiration></Rule>
        </LifecycleConfiguration>")]).await;
    let volume = client(&endpoint, None, allowed());
    let rules = volume.lifecycle().await.unwrap();
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[0].status, LifecycleRuleStatus::Enabled);
    assert_eq!(rules[0].prefix.as_ref().unwrap().payload().as_ref(), "corp");
    assert_eq!(
        rules[0].noncurrent_expiration,
        Some(NoncurrentExpiration {
            days: 2,
            newer_versions: None
        })
    );
    assert_eq!(rules[1].status, LifecycleRuleStatus::Disabled);
    assert_eq!(
        rules[1].prefix.as_ref().unwrap().payload().as_ref(),
        "corpus/sub/"
    );
    assert_eq!(rules[1].noncurrent_expiration, None);
    assert_eq!(
        rules[2].status,
        LifecycleRuleStatus::Unknown(ResponseValue::new("volume-secret".into()))
    );
    assert_eq!(
        rules[2].prefix.as_ref().unwrap().payload().as_ref(),
        "corpus/.."
    );
    assert_eq!(
        rules[2].noncurrent_expiration,
        Some(NoncurrentExpiration {
            days: 60,
            newer_versions: Some(100)
        })
    );
    assert_eq!(rules[3].prefix, None);
    assert_eq!(rules[3].noncurrent_expiration, None);
    assert_opaque_debug(&rules);
    assert_eq!(requests.try_recv().unwrap().method(), "GET");
    assert!(requests.try_recv().is_err());
    task.abort();

    stream::iter(["", "<NoncurrentDays>0</NoncurrentDays>", "<NoncurrentDays>-1</NoncurrentDays>", "<NoncurrentDays>3</NoncurrentDays><NewerNoncurrentVersions>0</NewerNoncurrentVersions>"])
        .then(|expiration| async move {
            let (endpoint, mut requests, server) = server(vec![response(200, format!("<LifecycleConfiguration><Rule><Status>Enabled</Status><NoncurrentVersionExpiration>{expiration}</NoncurrentVersionExpiration></Rule></LifecycleConfiguration>"))]).await;
            assert_eq!(client(&endpoint, None, allowed()).lifecycle().await, Err(VolumeError::InvalidResponse));
            assert_eq!(requests.try_recv().unwrap().method(), "GET");
            server.abort();
        }).collect::<()>().await;
}
