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
use tokio::sync::mpsc;
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
    S3Volume::new(
        S3VolumeConfig {
            endpoint: Some(endpoint.into()),
            region: "us-east-1".into(),
            bucket: "test-bucket".into(),
            prefix: "corpus/".into(),
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
        ..Default::default()
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
    let contents = objects.iter().map(|(key, size)| format!(
        "<Contents><Key>{key}</Key><Size>{size}</Size><ETag>&quot;listed-A&quot;</ETag></Contents>"
    )).collect::<String>().into_boxed_str();
    let token = continuation
        .map(|token| {
            format!("<NextContinuationToken>{token}</NextContinuationToken>").into_boxed_str()
        })
        .unwrap_or_default();
    response(
        200,
        format!(
            "<ListBucketResult><IsTruncated>{}</IsTruncated>{token}{contents}</ListBucketResult>",
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
            "successful response Debug disclosed a configured credential"
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

#[test]
#[timeout("30s")]
async fn encryption_and_preconditions_are_signed_on_writes() {
    stream::iter([
        (None, None, None),
        (Some(Encryption::S3), Some("AES256"), None),
        (Some(Encryption::Kms { key_id: "kms-key".into() }), Some("aws:kms"), Some("kms-key")),
    ]).then(|(encryption, algorithm, kms)| async move {
        let (endpoint, mut requests, server) = server(vec![response(200, ""), response(200,
            "<InitiateMultipartUploadResult><UploadId>upload-1</UploadId></InitiateMultipartUploadResult>")]).await;
        let volume = client(&endpoint, encryption, allowed());
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
        let authorization = request.headers()["authorization"].to_str().unwrap();
        assert!(authorization.contains("/us-east-1/s3/aws4_request"));
        assert!(authorization.contains("if-none-match"));
        if algorithm.is_some() { assert!(authorization.contains("x-amz-server-side-encryption")); }
        assert_eq!(volume.start_multipart(WriteTarget::File("large")).await.unwrap().payload().as_ref(), "upload-1");
        let request = requests.recv().await.unwrap();
        assert_eq!(request.headers().get("x-amz-server-side-encryption").map(|v| v.to_str().unwrap()), algorithm);
        assert_eq!(request.headers().get("x-amz-server-side-encryption-aws-kms-key-id").map(|v| v.to_str().unwrap()), kms);
        server.abort();
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn conditional_failures_redact_errors_and_never_retry() {
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
            .delete(WriteTarget::File("key"), "\"stale\"", 1)
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

    assert_opaque_debug(&(
        &result,
        metadata,
        &metadata.version,
        Some(metadata),
        [metadata],
    ));
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
        listing_response(
            &[("corpus/volume-secret/volume-token", 4)],
            Some("volume-secret+volume-token/token"),
        ),
        metadata_response(4, "\"volume-secret\"", "volume-token"),
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
    let read = volume
        .read(
            "key",
            version.version_id.as_deref(),
            Some(&version.etag),
            None,
        )
        .await;
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
    assert_eq!(&listed.objects[0].version, version);
    assert_eq!(
        listed.continuation.as_deref(),
        Some("volume-secret+volume-token/token")
    );
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert_eq!(requests.recv().await.unwrap().method(), "HEAD");
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
    let deleted = volume.delete(target, &version.etag, 4).await;
    assert_eq!(deleted.as_ref().unwrap(), version);
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
        &listed.objects[0].version,
        Some(&listed.objects[0]),
    ));
    assert_eq!(read.unwrap().body.collect().await.unwrap(), "body");
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
            let request = requests.recv().await.unwrap();
            assert_eq!(request.method(), method);
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

    let value = ResponseValue(NoDebug("volume-secret volume-token".into()));
    assert_opaque_debug(&value);
    assert_eq!(value.payload().0.as_ref(), "volume-secret volume-token");
    assert_eq!(
        value.into_payload().0.as_ref(),
        "volume-secret volume-token"
    );
    assert_opaque_debug(&ResponseValue(FailingDebug));
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
    let read = volume.read("key", None, None, None).await.unwrap();
    let error = read.body.collect().await.unwrap_err();
    assert_eq!(error, VolumeError::RequestFailed);
    let debug = format!("{error:?} {error}").into_boxed_str();
    assert!(!debug.contains("volume-token"));
    assert!(!debug.contains("[118, 111, 108, 117, 109, 101, 45, 116, 111, 107, 101, 110]"));
    let mut read = volume.read("key", None, None, None).await.unwrap();
    assert_eq!(
        read.body.next().await.unwrap(),
        Some(Bytes::from_static(b"body"))
    );
    assert_eq!(read.body.next().await, Err(VolumeError::RequestFailed));
    server.abort();
}

#[test]
#[timeout("30s")]
async fn the_prefix_root_marker_is_hidden_from_stat_and_read() {
    let (endpoint, mut requests, server) = server(vec![response(200, ""), response(200, "")]).await;
    let volume = client(&endpoint, None, allowed());
    assert_eq!(volume.stat("", None).await, Err(VolumeError::NotFound));
    assert!(matches!(
        volume.read("", None, None, None).await,
        Err(VolumeError::NotFound)
    ));
    [
        requests.recv().await.unwrap(),
        requests.recv().await.unwrap(),
    ]
    .into_iter()
    .for_each(|request| assert_eq!(request.uri().path(), "/test-bucket/corpus/"));
    server.abort();
}

#[test]
#[timeout("30s")]
async fn an_explicit_null_version_is_checked_for_stat_and_read() {
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
            let read = volume.read("key", Some("null"), None, None).await;
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
async fn reads_reject_ignored_ranges_and_contradictory_versions_or_etags() {
    let ranged = |content_range: &'static str| {
        let mut result = response(206, "cde");
        result
            .headers_mut()
            .insert("content-range", content_range.parse().unwrap());
        result
    };
    let (endpoint, _requests, server) = server(vec![
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
        volume.read("key", None, None, Some(2..=4)).await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume
            .read("key", Some("requested-version"), None, None)
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume
            .read("key", None, Some("\"requested-etag\""), None)
            .await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume.read("key", None, None, Some(2..=4)).await,
        Err(VolumeError::InvalidResponse)
    ));
    assert!(matches!(
        volume.read("key", None, None, Some(2..=4)).await,
        Err(VolumeError::InvalidResponse)
    ));
    let tail = volume.read("key", None, None, Some(2..=100)).await.unwrap();
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
            .delete(WriteTarget::File("nonempty/"), "\"reference\"", 7)
            .await
            .is_ok()
    );
    assert_eq!(requests.recv().await.unwrap().method(), "DELETE");
    assert_eq!(
        volume
            .delete(WriteTarget::File("marker/"), "\"marker\"", 0)
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
    async {
        let (endpoint, _requests, server) = server(vec![response(
            403,
            "<Error><Code>AccessDenied</Code><Message>volume-secret volume-token</Message></Error>",
        )])
        .await;
        let volume = client(&endpoint, None, allowed());
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
        tracing::info!("after the volume request");
        server.abort();
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
}

#[test]
#[timeout("30s")]
async fn wildcard_and_multiple_etags_cannot_bypass_write_conditions() {
    let (endpoint, mut requests, server) = server(Vec::new()).await;
    let volume = client(&endpoint, None, allowed());
    stream::iter(["*", " * ", "\t*\t", "", "\"one\", \"two\"", "W/\"weak\""])
        .then(|etag| async {
            assert_eq!(
                volume.delete(WriteTarget::File("key"), etag, 1).await,
                Err(VolumeError::InvalidRequest)
            );
        })
        .collect::<()>()
        .await;
    assert!(requests.try_recv().is_err());
    server.abort();
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
async fn successful_object_results_require_a_usable_reference_etag() {
    stream::iter([
        None,
        Some(""),
        Some("*"),
        Some("W/\"weak\""),
        Some("\"one\", \"two\""),
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
        let (endpoint, _requests, server) =
            server(vec![without_reference(), without_reference()]).await;
        let volume = client(&endpoint, None, allowed());
        assert!(matches!(
            volume.read("key", None, None, None).await,
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
            Err(VolumeError::InvalidResponse)
        );
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn list_returns_the_complete_head_observation_after_discovery() {
    let states = [
        (4, "\"listed-A\"", "version-A"),
        (19, "\"head-B\"", "version-B"),
    ];
    let orders = states
        .into_iter()
        .flat_map(|listed| states.into_iter().map(move |head| (listed, head)));
    stream::iter(orders).then(|((listed_size, listed_etag, _), (size, etag, version_id))| async move {
        let (endpoint, mut requests, server) = server(vec![
            response(200, format!("<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>corpus/key</Key><Size>{listed_size}</Size><ETag>{listed_etag}</ETag></Contents></ListBucketResult>")),
            metadata_response(size, etag, version_id),
        ]).await;
        let volume = client(&endpoint, None, allowed());
        let page = volume.list("", None).await.unwrap();
        assert_eq!(page.objects.as_ref(), [ObjectMetadata {
            key: "key".into(), size,
            version: ObjectVersion { etag: etag.into(), version_id: Some(version_id.into()) },
        }]);
        assert_eq!(requests.recv().await.unwrap().method(), "GET");
        let head = requests.recv().await.unwrap();
        assert_eq!(head.method(), "HEAD");
        assert_eq!(head.uri().path(), "/test-bucket/corpus/key");
        assert!(!head.uri().query().unwrap_or_default().contains("versionId"));
        assert!(requests.try_recv().is_err());
        server.abort();
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn list_checks_all_marker_and_file_observation_orders_without_scanning() {
    let cases = [0, 9].into_iter().flat_map(|listed_size| {
        [0, 9].into_iter().flat_map(move |head_size| {
            [("version-B", Some("version-B")), ("null", None)]
                .into_iter()
                .map(move |versions| (listed_size, head_size, versions))
        })
    });
    stream::iter(cases)
        .then(
            |(listed_size, head_size, (version_id, expected_version))| async move {
                let (endpoint, mut requests, server) = server(vec![
                    listing_response(&[("corpus/slash/", listed_size)], Some("next+opaque/token")),
                    metadata_response(head_size, "\"head-B\"", version_id),
                ])
                .await;
                let volume = client(&endpoint, None, allowed());
                let page = volume
                    .list("", Some("previous+opaque/token"))
                    .await
                    .unwrap();
                assert_eq!(page.continuation.as_deref(), Some("next+opaque/token"));
                if head_size == 0 {
                    assert!(page.objects.is_empty());
                } else {
                    assert_eq!(
                        page.objects.as_ref(),
                        [ObjectMetadata {
                            key: "slash/".into(),
                            size: 9,
                            version: ObjectVersion {
                                etag: "\"head-B\"".into(),
                                version_id: expected_version.map(Into::into)
                            },
                        }]
                    );
                }
                let list = requests.recv().await.unwrap();
                assert_eq!(list.method(), "GET");
                assert!(list.uri().query().unwrap().contains("list-type=2"));
                assert!(
                    list.uri()
                        .query()
                        .unwrap()
                        .contains("continuation-token=previous%2Bopaque%2Ftoken")
                );
                let head = requests.recv().await.unwrap();
                assert_eq!(head.method(), "HEAD");
                assert_eq!(head.uri().path(), "/test-bucket/corpus/slash/");
                assert!(requests.try_recv().is_err());
                server.abort();
            },
        )
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn list_fails_the_whole_page_on_any_head_failure_in_either_key_order() {
    let cases = [0, 9].into_iter().flat_map(|listed_size| {
        [404, 403, 503, 200].into_iter().flat_map(move |status| {
            [false, true]
                .into_iter()
                .map(move |failure_first| (listed_size, status, failure_first))
        })
    });
    stream::iter(cases)
        .then(|(listed_size, status, failure_first)| async move {
            let keys = if failure_first {
                [("corpus/slash/", listed_size), ("corpus/other", 5)]
            } else {
                [("corpus/other", 5), ("corpus/slash/", listed_size)]
            };
            let mut failed_head = response(status, "");
            failed_head.headers_mut().remove("etag");
            let good_head = metadata_response(5, "\"other\"", "other-version");
            let heads = if failure_first {
                [failed_head, good_head]
            } else {
                [good_head, failed_head]
            };
            let script = std::iter::once(listing_response(&keys, Some("unused-page")))
                .chain(heads)
                .collect();
            let (endpoint, mut requests, server) = server(script).await;
            let volume = client(&endpoint, None, allowed());
            let expected = match status {
                404 => VolumeError::NotFound,
                403 => VolumeError::Denied,
                503 => VolumeError::RequestFailed,
                200 => VolumeError::InvalidResponse,
                _ => unreachable!(),
            };
            assert_eq!(volume.list("", None).await.unwrap_err(), expected);
            assert_eq!(requests.recv().await.unwrap().method(), "GET");
            let head_count = if failure_first { 1 } else { 2 };
            let mut requests = stream::iter(keys.into_iter().take(head_count))
                .fold(requests, |mut requests, (key, _)| async move {
                    let request = requests.recv().await.unwrap();
                    assert_eq!(request.method(), "HEAD");
                    assert_eq!(request.uri().path(), format!("/test-bucket/{key}"));
                    requests
                })
                .await;
            assert!(requests.try_recv().is_err());
            server.abort();
        })
        .collect::<()>()
        .await;
}

#[test]
#[timeout("30s")]
async fn list_rejects_outside_keys_even_when_they_look_like_markers() {
    stream::iter(["other/dir/", "corpus2/dir/", "corpus/../dir/"])
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
async fn reads_keep_versions_ranges_and_hide_only_zero_byte_markers() {
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
    let (endpoint, mut requests, server) = server(vec![head, ranged, response(200,
        "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>next+token</NextContinuationToken><Contents><Key>corpus/dir/</Key><Size>0</Size></Contents><Contents><Key>corpus/dir/file</Key><Size>0</Size><ETag>&quot;file&quot;</ETag></Contents><Contents><Key>corpus/nonempty/</Key><Size>9</Size><ETag>&quot;nonempty&quot;</ETag></Contents></ListBucketResult>"),
        metadata_response(0, "\"marker\"", "marker-version"),
        metadata_response(0, "\"file\"", "file-version"),
        metadata_response(9, "\"nonempty\"", "nonempty-version"),
    ]).await;
    let volume = client(&endpoint, None, allowed());
    let head = volume.stat("key", None).await.unwrap();
    assert_eq!(head.size, 12);
    assert_eq!(head.version.version_id, None);
    assert_eq!(head.version.etag.as_ref(), "\"reference\"");
    requests.recv().await.unwrap();
    let read = volume
        .read("key", Some("old+version"), Some("\"old\""), Some(2..=4))
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
    let page = volume.list("", Some("previous+token")).await.unwrap();
    assert_eq!(page.continuation.as_deref(), Some("next+token"));
    assert!(
        page.objects
            .iter()
            .map(|o| o.key.as_ref())
            .eq(["dir/file", "nonempty/"])
    );
    let request = requests.recv().await.unwrap();
    assert!(
        request
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=previous%2Btoken")
    );
    let mut requests = stream::iter(["dir/", "dir/file", "nonempty/"])
        .fold(requests, |mut requests, key| async move {
            let head = requests.recv().await.unwrap();
            assert_eq!(head.method(), "HEAD");
            assert_eq!(head.uri().path(), format!("/test-bucket/corpus/{key}"));
            requests
        })
        .await;
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn multipart_completion_is_conditional_and_probe_keys_stay_reserved() {
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
    assert_eq!(requests.recv().await.unwrap().method(), "DELETE");
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
        .delete(WriteTarget::Probe("check-1"), "\"reference\"", 0)
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
        volume.delete(WriteTarget::File("key"), "*", 0).await,
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
        assert_eq!(
            result
                .as_ref()
                .unwrap()
                .as_ref()
                .map(|status| status.payload().as_str()),
            status
        );
        assert_eq!(requests.recv().await.unwrap().method(), "GET");
        assert!(requests.try_recv().is_err());
        server.abort();
        assert_opaque_debug(&result);
        assert_opaque_debug(result.as_ref().unwrap());
        if let Some(value) = result.unwrap() {
            assert_opaque_debug(&value);
            assert_eq!(Some(value.into_payload().as_str()), status);
        }
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn bucket_checks_return_versioning_and_noncurrent_lifecycle_rules_without_probes() {
    use aws_sdk_s3::types::{
        AbortIncompleteMultipartUpload, ExpirationStatus, LifecycleExpiration,
        LifecycleRuleAndOperator, LifecycleRuleFilter, NoncurrentVersionExpiration,
        NoncurrentVersionTransition, Tag, Transition, TransitionStorageClass,
    };
    let expected = LifecycleRule::builder()
        .id("volume-secret-volume-token")
        .status(ExpirationStatus::from("volume-secret-volume-token"))
        .filter(
            LifecycleRuleFilter::builder()
                .and(
                    LifecycleRuleAndOperator::builder()
                        .prefix("corpus/volume-secret/volume-token/")
                        .tags(
                            Tag::builder()
                                .key("volume-secret")
                                .value("volume-token")
                                .build()
                                .unwrap(),
                        )
                        .tags(Tag::builder().key("second").value("tag").build().unwrap())
                        .object_size_greater_than(2)
                        .object_size_less_than(900)
                        .build(),
                )
                .build(),
        )
        .expiration(LifecycleExpiration::builder().days(30).build())
        .transitions(
            Transition::builder()
                .days(3)
                .storage_class(TransitionStorageClass::from("volume-secret"))
                .build(),
        )
        .transitions(
            Transition::builder()
                .days(5)
                .storage_class(TransitionStorageClass::Glacier)
                .build(),
        )
        .noncurrent_version_transitions(
            NoncurrentVersionTransition::builder()
                .noncurrent_days(2)
                .newer_noncurrent_versions(5)
                .storage_class(TransitionStorageClass::from("volume-token"))
                .build(),
        )
        .noncurrent_version_expiration(
            NoncurrentVersionExpiration::builder()
                .noncurrent_days(7)
                .newer_noncurrent_versions(6)
                .build(),
        )
        .abort_incomplete_multipart_upload(
            AbortIncompleteMultipartUpload::builder()
                .days_after_initiation(4)
                .build(),
        )
        .build()
        .unwrap();
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
    ]).await;
    let volume = client(&endpoint, None, allowed());
    assert_eq!(
        volume
            .versioning()
            .await
            .unwrap()
            .map(ResponseValue::into_payload),
        Some(BucketVersioningStatus::Enabled)
    );
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    let result = volume.lifecycle().await;
    let rules = result.as_ref().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].payload(), &expected);
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert!(volume.lifecycle().await.unwrap().is_empty());
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert!(volume.lifecycle().await.unwrap().is_empty());
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert_eq!(volume.versioning().await, Err(VolumeError::Denied));
    assert_eq!(requests.recv().await.unwrap().method(), "GET");
    assert!(requests.try_recv().is_err());
    server.abort();
    assert_opaque_debug(&(&result, rules, &rules[0], Some(&rules[0])));
    rules.iter().for_each(assert_opaque_debug);
    let rules = result
        .unwrap()
        .into_iter()
        .map(ResponseValue::into_payload)
        .collect::<Box<[_]>>();
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
    let default = client(&endpoint, None, ConnectionPolicy::default());
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
