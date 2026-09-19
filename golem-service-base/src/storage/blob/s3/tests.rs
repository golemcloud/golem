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

use super::S3BlobStorage;
use crate::config::{S3BlobStorageConfig, S3BlobStorageCredentialsConfig};
use crate::storage::blob::{BlobRangeError, BlobStorage, BlobStorageNamespace, ListedBlob};
use aws_sdk_s3::config::http::{HttpRequest, HttpResponse};
use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, StalledStreamProtectionConfig,
};
use aws_sdk_s3::primitives::SdkBody;
use aws_smithy_runtime_api::client::http::{
    HttpConnector, HttpConnectorFuture, SharedHttpConnector, http_client_fn,
};
use aws_smithy_runtime_api::http::StatusCode;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Response, StatusCode as ServerStatus};
use axum::routing::put;
use bytes::Bytes;
use golem_common::model::environment::EnvironmentId;
use pretty_assertions::assert_eq;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{test, timeout};
use uuid::Uuid;

/// A request that the scripted transport received.
#[derive(Debug, Clone)]
struct SentRequest {
    method: String,
    uri: String,
    range: Option<String>,
    body: String,
}

impl SentRequest {
    fn from_http(request: &HttpRequest) -> Self {
        Self {
            method: request.method().to_string(),
            uri: request.uri().to_string(),
            range: request.headers().get("range").map(str::to_string),
            body: request
                .body()
                .bytes()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_default(),
        }
    }

    fn is_list_objects(&self) -> bool {
        self.method == "GET" && self.uri.contains("list-type=2")
    }

    fn is_delete_objects(&self) -> bool {
        self.method == "POST" && self.uri.contains("?delete")
    }

    /// Gives the keys in the body of a `DeleteObjects` request, in their order.
    fn deleted_keys(&self) -> Vec<String> {
        self.body
            .split("<Key>")
            .skip(1)
            .filter_map(|part| part.split_once("</Key>").map(|(key, _)| key.to_string()))
            .collect()
    }
}

/// The answer of the scripted transport to one request.
struct Answer {
    status: u16,
    content_range: Option<&'static str>,
    body: String,
}

impl Answer {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_range: None,
            body: body.into(),
        }
    }

    fn partial(content_range: Option<&'static str>, body: &str) -> Self {
        Self {
            status: 206,
            content_range,
            body: body.to_string(),
        }
    }
}

type Script = dyn Fn(&SentRequest, usize) -> Answer + Send + Sync;

type SentRequests = Arc<Mutex<Vec<SentRequest>>>;

/// An HTTP transport that records each request and answers it from a script.
///
/// The script gets the request and the number of requests before it.
#[derive(Clone)]
struct ScriptedTransport {
    requests: SentRequests,
    script: Arc<Script>,
}

impl Debug for ScriptedTransport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScriptedTransport")
    }
}

impl HttpConnector for ScriptedTransport {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let sent = SentRequest::from_http(&request);
        let earlier = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(sent.clone());
            requests.len() - 1
        };
        let answer = (self.script)(&sent, earlier);
        let content_length = answer.body.len().to_string();
        let mut response = HttpResponse::new(
            StatusCode::try_from(answer.status).unwrap(),
            SdkBody::from(answer.body),
        );
        response
            .headers_mut()
            .insert("content-type", "application/xml");
        // A server sends the length of the body that it sends. The range check of a ranged
        // read reads it.
        response
            .headers_mut()
            .insert("content-length", content_length);
        if let Some(content_range) = answer.content_range {
            response
                .headers_mut()
                .insert("content-range", content_range);
        }
        HttpConnectorFuture::ready(Ok(response))
    }
}

/// Makes an S3 blob storage that sends its requests to a script.
///
/// The client reads no environment, no profile file and no instance metadata. The SDK does not
/// retry, so each attempt of the blob storage is one request. The blob storage makes 3 attempts
/// with a delay of 1 ms.
fn scripted_storage(
    object_prefix: &str,
    script: impl Fn(&SentRequest, usize) -> Answer + Send + Sync + 'static,
) -> (S3BlobStorage, SentRequests) {
    let transport = ScriptedTransport {
        requests: Arc::default(),
        script: Arc::new(script),
    };
    let requests = transport.requests.clone();
    let connector = SharedHttpConnector::new(transport);
    let sdk_config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "test"))
        .endpoint_url("http://s3.test")
        .force_path_style(true)
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .retry_config(RetryConfig::disabled())
        .stalled_stream_protection(StalledStreamProtectionConfig::disabled())
        .http_client(http_client_fn(move |_, _| connector.clone()))
        .build();
    let mut config = S3BlobStorageConfig {
        object_prefix: object_prefix.to_string(),
        ..Default::default()
    };
    config.retries.max_attempts = 3;
    config.retries.min_delay = Duration::from_millis(1);
    config.retries.max_delay = Duration::from_millis(1);
    config.retries.max_jitter_factor = None;
    (S3BlobStorage::with_sdk_config(config, sdk_config), requests)
}

fn sent(requests: &SentRequests) -> Vec<SentRequest> {
    requests.lock().unwrap().clone()
}

fn namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(Uuid::nil()),
    }
}

/// Gives the key prefix of `namespace()` in a storage without an object prefix.
fn namespace_prefix() -> String {
    Uuid::nil().to_string()
}

fn bulk_keys(prefix: &str) -> Vec<String> {
    (0..2001)
        .map(|index| format!("{prefix}bulk/{index:04}"))
        .collect()
}

fn listed_blob(path: &str, size: u64) -> ListedBlob {
    ListedBlob {
        path: Path::new(path).into(),
        size,
    }
}

const EMPTY_DELETE_RESULT: &str = r#"<?xml version="1.0" encoding="UTF-8"?><DeleteResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"></DeleteResult>"#;

const INVALID_RANGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>InvalidRange</Code><Message>The requested range is not satisfiable</Message></Error>"#;

const INTERNAL_ERROR: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>InternalError</Code><Message>We encountered an internal error</Message></Error>"#;

const NO_SUCH_KEY: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"#;

fn delete_result_with_error(key: &str, code: &str, message: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><DeleteResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Error><Key>{key}</Key><Code>{code}</Code><Message>{message}</Message></Error></DeleteResult>"#
    )
}

/// Makes a listing page whose one `Contents` element has no `Size`.
fn list_page_without_size(key: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>bucket</Name><IsTruncated>false</IsTruncated><Contents><Key>{key}</Key></Contents></ListBucketResult>"#
    )
}

fn list_page(objects: &[(String, u64)], next_token: Option<&str>) -> String {
    let contents = objects
        .iter()
        .map(|(key, size)| format!("<Contents><Key>{key}</Key><Size>{size}</Size></Contents>"))
        .collect::<String>();
    let truncation = next_token.map_or_else(
        || "<IsTruncated>false</IsTruncated>".to_string(),
        |token| {
            format!(
                "<IsTruncated>true</IsTruncated><NextContinuationToken>{token}</NextContinuationToken>"
            )
        },
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>bucket</Name>{truncation}{contents}</ListBucketResult>"#
    )
}

#[test]
async fn delete_many_sends_at_most_1000_keys_in_each_request() {
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, EMPTY_DELETE_RESULT));
    let paths = bulk_keys("")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    storage
        .delete_many("test", "delete-many", namespace(), &paths)
        .await
        .unwrap();

    let requests = sent(&requests);
    assert_eq!(
        requests
            .iter()
            .map(|request| (
                request.is_delete_objects(),
                request.body.contains("<Quiet>true</Quiet>"),
                request.deleted_keys().len()
            ))
            .collect::<Vec<_>>(),
        vec![(true, true, 1000), (true, true, 1000), (true, true, 1)]
    );
    assert_eq!(
        requests
            .iter()
            .flat_map(SentRequest::deleted_keys)
            .collect::<Vec<_>>(),
        bulk_keys(&format!("{}/", namespace_prefix()))
    );
}

#[test]
async fn delete_dir_sends_at_most_1000_keys_in_each_request() {
    let keys = bulk_keys(&format!("{}/", namespace_prefix()));
    let listing = list_page(
        &keys.iter().map(|key| (key.clone(), 4)).collect::<Vec<_>>(),
        None,
    );
    let (storage, requests) = scripted_storage("", move |request, _| {
        if request.is_list_objects() {
            Answer::new(200, listing.clone())
        } else {
            Answer::new(200, EMPTY_DELETE_RESULT)
        }
    });

    let deleted = storage
        .delete_dir("test", "delete-dir", namespace(), Path::new("bulk"))
        .await
        .unwrap();

    let requests = sent(&requests);
    assert_eq!(
        (
            deleted,
            requests
                .iter()
                .map(|request| (request.is_list_objects(), request.deleted_keys().len()))
                .collect::<Vec<_>>()
        ),
        (
            true,
            vec![(true, 0), (false, 1000), (false, 1000), (false, 1)]
        )
    );
    assert_eq!(
        requests
            .iter()
            .flat_map(SentRequest::deleted_keys)
            .collect::<Vec<_>>(),
        keys
    );
}

#[test]
async fn delete_many_with_no_paths_sends_no_request() {
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, EMPTY_DELETE_RESULT));

    storage
        .delete_many("test", "delete-many", namespace(), &[])
        .await
        .unwrap();

    assert_eq!(sent(&requests).len(), 0);
}

#[test]
async fn delete_many_fails_when_s3_reports_a_key_error() {
    let key = format!("{}/bulk/0001", namespace_prefix());
    let answered_key = key.clone();
    let (storage, requests) = scripted_storage("", move |_, _| {
        Answer::new(
            200,
            delete_result_with_error(&answered_key, "AccessDenied", "Access Denied"),
        )
    });

    let error = storage
        .delete_many(
            "test",
            "delete-many",
            namespace(),
            &[PathBuf::from("bulk/0001"), PathBuf::from("bulk/0002")],
        )
        .await
        .unwrap_err();

    assert_eq!(
        (sent(&requests).len(), error.to_string()),
        (
            3,
            format!(
                "S3 did not delete 1 of 2 keys in bucket {}; first: {key}: AccessDenied: Access Denied",
                storage.config.custom_data_bucket
            )
        )
    );
}

#[test]
async fn delete_many_sends_a_request_with_a_key_error_again() {
    let key = format!("{}/bulk/0001", namespace_prefix());
    let (storage, requests) = scripted_storage("", move |_, earlier| {
        if earlier == 0 {
            Answer::new(
                200,
                delete_result_with_error(&key, "InternalError", "We encountered an internal error"),
            )
        } else {
            Answer::new(200, EMPTY_DELETE_RESULT)
        }
    });

    storage
        .delete_many(
            "test",
            "delete-many",
            namespace(),
            &[PathBuf::from("bulk/0001")],
        )
        .await
        .unwrap();

    assert_eq!(sent(&requests).len(), 2);
}

#[test]
async fn get_raw_slice_refuses_an_inverted_range_without_a_request() {
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, "abcdef"));

    let result = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            3,
            2,
        )
        .await;

    assert_eq!(
        (
            result.map_err(|error| error.downcast_ref::<BlobRangeError>().copied()),
            sent(&requests).len()
        ),
        (Err(Some(BlobRangeError { start: 3, end: 2 })), 0)
    );
}

#[test]
async fn get_raw_slice_turns_416_into_a_range_error_without_a_retry() {
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(416, INVALID_RANGE));

    let result = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            6,
            9,
        )
        .await;

    assert_eq!(
        (
            result.map_err(|error| error.downcast_ref::<BlobRangeError>().copied()),
            sent(&requests)
                .iter()
                .map(|request| request.range.clone())
                .collect::<Vec<_>>()
        ),
        (
            Err(Some(BlobRangeError { start: 6, end: 9 })),
            vec![Some("bytes=6-9".to_string())]
        )
    );
}

#[test]
async fn get_raw_slice_turns_an_ignored_range_into_a_range_error() {
    // The script answers 200 with the whole object and no content range, which is the
    // response of a server that does not apply the range (RFC 9110, section 14.2). The
    // range is the one that a guest reaches the host with after it gives a negative offset
    // for the start and the end. The S3 case of `get_raw_slice_uses_inclusive_ranges` in
    // `tests/blob_storage.rs` sends the same range to MinIO.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, "abcdef"));

    let result = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            u64::MAX,
            u64::MAX,
        )
        .await;

    assert_eq!(
        (
            result.map_err(|error| error.downcast_ref::<BlobRangeError>().copied()),
            sent(&requests)
                .iter()
                .map(|request| request.range.clone())
                .collect::<Vec<_>>()
        ),
        (
            Err(Some(BlobRangeError {
                start: u64::MAX,
                end: u64::MAX
            })),
            vec![Some(
                "bytes=18446744073709551615-18446744073709551615".to_string()
            )]
        )
    );
}

#[test]
async fn get_raw_slice_checks_the_range_that_s3_returns() {
    let (storage, _) = scripted_storage("", |request, _| match request.range.as_deref() {
        Some("bytes=1-3") => Answer::partial(Some("bytes 1-3/6"), "bcd"),
        Some("bytes=5-5") => Answer::partial(Some("bytes 5-5/6"), "f"),
        Some("bytes=4-9") => Answer::partial(Some("bytes 4-5/6"), "ef"),
        Some("bytes=2-4") => Answer::partial(Some("bytes 0-3/6"), "abcd"),
        _ => Answer::partial(None, "abcdef"),
    });
    let read = |start, end| {
        storage.get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            start,
            end,
        )
    };

    let inside = read(1, 3).await.unwrap();
    let one_byte = read(5, 5).await.unwrap();
    let after_the_end = read(4, 9)
        .await
        .map_err(|error| error.downcast_ref::<BlobRangeError>().copied());
    let from_another_byte = read(2, 4)
        .await
        .map_err(|error| error.downcast_ref::<BlobRangeError>().copied());
    let without_content_range = read(0, 5)
        .await
        .map_err(|error| error.downcast_ref::<BlobRangeError>().copied());
    // A 206 response without a content range does not hold the whole object, so its length
    // does not tell whether the range is in the object.
    let partial_without_content_range = read(0, 9)
        .await
        .map_err(|error| error.downcast_ref::<BlobRangeError>().copied());

    assert_eq!(
        (
            inside,
            one_byte,
            after_the_end,
            from_another_byte,
            without_content_range,
            partial_without_content_range
        ),
        (
            Some(b"bcd".to_vec()),
            Some(b"f".to_vec()),
            Err(Some(BlobRangeError { start: 4, end: 9 })),
            Err(None),
            Err(None),
            Err(None)
        )
    );
}

#[test]
async fn get_raw_slice_retries_a_server_error_but_not_a_missing_object() {
    let (storage, requests) = scripted_storage("", |request, earlier| {
        if request.uri.contains("missing") {
            Answer::new(404, NO_SUCH_KEY)
        } else if earlier == 0 {
            Answer::new(500, INTERNAL_ERROR)
        } else {
            Answer::partial(Some("bytes 0-2/6"), "abc")
        }
    });
    let read = |path: &'static str, start: u64, end: u64| {
        storage.get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new(path),
            start,
            end,
        )
    };

    let after_a_server_error = read("blob", 0, 2).await.unwrap();
    let missing = read("missing", 0, 2).await.unwrap();

    assert_eq!(
        (after_a_server_error, missing, sent(&requests).len()),
        (Some(b"abc".to_vec()), None, 3)
    );
}

#[test]
async fn list_blobs_below_skips_directory_markers_and_keeps_sizes() {
    let prefix = format!("objects/{}", namespace_prefix());
    let first_page = list_page(
        &[
            (format!("{prefix}/tree/"), 0),
            (format!("{prefix}/tree/a"), 1),
            (format!("{prefix}/tree/x/__dir_marker"), 0),
        ],
        Some("page-2"),
    );
    let second_page = list_page(
        &[
            (format!("{prefix}/tree/x/b"), 2),
            (format!("{prefix}/tree/x/y/c"), 3),
        ],
        None,
    );
    let (storage, requests) = scripted_storage("objects", move |request, _| {
        if request.uri.contains("continuation-token=page-2") {
            Answer::new(200, second_page.clone())
        } else {
            Answer::new(200, first_page.clone())
        }
    });

    let mut listed = storage
        .list_blobs_below("test", "list", namespace(), Path::new("tree"))
        .await
        .unwrap()
        .into_vec();
    listed.sort();

    assert_eq!(
        (listed, sent(&requests).len()),
        (
            vec![
                listed_blob("tree/a", 1),
                listed_blob("tree/x/b", 2),
                listed_blob("tree/x/y/c", 3)
            ],
            2
        )
    );
}

#[test]
async fn list_blobs_below_fails_when_a_key_has_no_size() {
    let key = format!("{}/tree/a", namespace_prefix());
    let page = list_page_without_size(&key);
    let (storage, _) = scripted_storage("", move |_, _| Answer::new(200, page.clone()));

    let error = storage
        .list_blobs_below("test", "list", namespace(), Path::new("tree"))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("S3 gave no size for the key {key}")
    );
}

/// The bodies a fake S3 received, and the status it answers each `PUT` with.
#[derive(Clone)]
struct PutServerState {
    bodies: Arc<Mutex<Vec<Bytes>>>,
    statuses: Arc<Mutex<Vec<ServerStatus>>>,
}

async fn handle_put(State(state): State<PutServerState>, body: Bytes) -> Response<Body> {
    state.bodies.lock().unwrap().push(body);
    let status = state.statuses.lock().unwrap().remove(0);
    let body = if status.is_success() {
        Body::empty()
    } else {
        Body::from(
            "<Error><Code>ForcedFailure</Code><Message>forced test failure</Message></Error>",
        )
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/xml")
        .body(body)
        .unwrap()
}

/// Makes a blob storage that talks to a server answering each `PUT` with the next status.
///
/// The SDK's own retries are off, so each request the server sees is one Golem retry.
async fn put_server_storage(
    statuses: Vec<ServerStatus>,
) -> (
    S3BlobStorage,
    Arc<Mutex<Vec<Bytes>>>,
    tokio::task::JoinHandle<()>,
) {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = PutServerState {
        bodies: bodies.clone(),
        statuses: Arc::new(Mutex::new(statuses)),
    };
    let app = Router::new().fallback(put(handle_put)).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut config = S3BlobStorageConfig {
        aws_endpoint_url: Some(endpoint.clone()),
        aws_credentials: Some(S3BlobStorageCredentialsConfig::new(
            "test-access-key",
            "test-secret-key",
            "test",
        )),
        aws_path_style: Some(true),
        ..Default::default()
    };
    config.retries.max_attempts = 3;
    config.retries.min_delay = Duration::ZERO;
    config.retries.max_delay = Duration::ZERO;
    config.retries.multiplier = 1.0;
    config.retries.max_jitter_factor = None;

    let sdk_config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(config.region.clone()))
        .credentials_provider(Credentials::new(
            "test-access-key",
            "test-secret-key",
            None,
            None,
            "test",
        ))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();

    (
        S3BlobStorage::with_sdk_config(config, sdk_config),
        bodies,
        server,
    )
}

#[test]
#[timeout("10s")]
async fn put_raw_golem_retries_preserve_nonempty_payload() {
    let (storage, bodies, server) = put_server_storage(vec![
        ServerStatus::INTERNAL_SERVER_ERROR,
        ServerStatus::INTERNAL_SERVER_ERROR,
        ServerStatus::OK,
    ])
    .await;
    let data = b"payload that must survive every Golem retry";

    let result = storage
        .put_raw(
            "test",
            "put_raw",
            BlobStorageNamespace::CustomStorage {
                environment_id: EnvironmentId::new(),
            },
            Path::new("object"),
            data,
        )
        .await;
    server.abort();

    result.unwrap();
    assert_eq!(
        bodies.lock().unwrap().as_slice(),
        [
            Bytes::from_static(data),
            Bytes::from_static(data),
            Bytes::from_static(data),
        ]
    );
}

#[test]
#[timeout("10s")]
async fn put_raw_golem_retries_preserve_empty_payload_and_final_error() {
    let (storage, bodies, server) = put_server_storage(vec![
        ServerStatus::INTERNAL_SERVER_ERROR,
        ServerStatus::INTERNAL_SERVER_ERROR,
        ServerStatus::INTERNAL_SERVER_ERROR,
    ])
    .await;

    let error = storage
        .put_raw(
            "test",
            "put_raw",
            BlobStorageNamespace::CustomStorage {
                environment_id: EnvironmentId::new(),
            },
            Path::new("object"),
            &[],
        )
        .await
        .unwrap_err();
    server.abort();

    assert!(format!("{error:#}").contains("forced test failure"));
    assert_eq!(
        bodies.lock().unwrap().as_slice(),
        [Bytes::new(), Bytes::new(), Bytes::new()]
    );
}
