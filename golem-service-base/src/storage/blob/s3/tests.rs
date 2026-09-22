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

use super::{BodyLengthError, RETRIABLE_SERVICE_ERROR_CODES, S3BlobStorage, cut_range, read_body};
use crate::config::{S3BlobStorageConfig, S3BlobStorageCredentialsConfig};
use crate::storage::blob::{
    BlobMissingError, BlobNameError, BlobRangeError, BlobStorage, BlobStorageNamespace,
    ExistsResult, ListedBlob, PutIfAbsent, agent_path_segment,
};
use anyhow::anyhow;
use aws_runtime::retries::classifiers::{THROTTLING_ERRORS, TRANSIENT_ERRORS};
use aws_sdk_s3::config::http::{HttpRequest, HttpResponse};
use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, StalledStreamProtectionConfig,
};
use aws_sdk_s3::primitives::SdkBody;
use aws_smithy_runtime_api::client::http::{
    HttpConnector, HttpConnectorFuture, SharedHttpConnector, http_client_fn,
};
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::http::StatusCode;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Response, StatusCode as ServerStatus};
use axum::routing::put;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use golem_common::model::AgentId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use http_body::Frame;
use http_body_util::StreamBody;
use pretty_assertions::assert_eq;
use std::collections::TryReserveError;
use std::convert::Infallible;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use test_r::{test, timeout};
use tracing::instrument::WithSubscriber;
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, span};
use uuid::Uuid;

/// A request that the scripted transport received.
#[derive(Debug, Clone)]
struct SentRequest {
    method: String,
    uri: String,
    range: Option<String>,
    copy_source: Option<String>,
    if_none_match: Option<String>,
    body: String,
}

impl SentRequest {
    fn from_http(request: &HttpRequest) -> Self {
        Self {
            method: request.method().to_string(),
            uri: request.uri().to_string(),
            range: request.headers().get("range").map(str::to_string),
            copy_source: request
                .headers()
                .get("x-amz-copy-source")
                .map(str::to_string),
            if_none_match: request.headers().get("if-none-match").map(str::to_string),
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

    /// Gives the keys in the body of a `DeleteObjects` request, in the order of the body.
    fn deleted_keys(&self) -> Vec<String> {
        self.body
            .split("<Key>")
            .skip(1)
            .filter_map(|part| part.split_once("</Key>").map(|(key, _)| key.to_string()))
            .collect()
    }
}

/// The response of the scripted transport to one request.
///
/// The response has a `Content-Length` only when `content_length` is set. When `body_reads` is
/// set, the body is one frame, and a read of that frame adds 1 to it. Else, when `frame_bytes` is
/// set, the body is frames of that number of bytes, and the last frame can be shorter, as a body
/// from S3 comes in many frames. Else, the body is one frame. When `transport_error` is set, the
/// transport gives that error and no response.
struct Answer {
    status: u16,
    content_range: Option<&'static str>,
    content_length: Option<usize>,
    last_modified: Option<&'static str>,
    body_reads: Option<Arc<AtomicUsize>>,
    frame_bytes: Option<usize>,
    body: String,
    transport_error: Option<ConnectorError>,
}

impl Answer {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_range: None,
            content_length: None,
            last_modified: None,
            body_reads: None,
            frame_bytes: None,
            body: body.into(),
            transport_error: None,
        }
    }

    /// The response to a `HEAD` of an object that is in the bucket. `get_metadata` reads the
    /// time of the last change of the object from the header, so a response without it makes
    /// `get_metadata` panic.
    fn object_head() -> Self {
        Self {
            last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT"),
            ..Self::new(200, "")
        }
    }

    /// An error of the transport, in place of a response.
    fn transport_error(error: ConnectorError) -> Self {
        Self {
            transport_error: Some(error),
            ..Self::new(0, "")
        }
    }

    fn partial(content_range: Option<&'static str>, body: &str) -> Self {
        Self {
            content_range,
            ..Self::new(206, body)
        }
    }

    /// The full object with the status 200 and its `Content-Length`, the response of a server
    /// that ignores the range. The backend uses the body as the full object. A read of the body
    /// adds 1 to `body_reads`.
    fn whole_object(body: &str, body_reads: &Arc<AtomicUsize>) -> Self {
        Self {
            content_length: Some(body.len()),
            body_reads: Some(body_reads.clone()),
            ..Self::new(200, body)
        }
    }

    /// Gives the HTTP response. When the script gave an error of the transport in place of a
    /// response, gives that error and no response.
    fn into_response(mut self) -> Result<HttpResponse, ConnectorError> {
        match self.transport_error.take() {
            Some(error) => Err(error),
            None => Ok(self.into_http_response()),
        }
    }

    fn into_http_response(self) -> HttpResponse {
        let status = StatusCode::try_from(self.status).unwrap();
        let content_range = self.content_range;
        let content_length = self.content_length;
        let last_modified = self.last_modified;
        let mut response = HttpResponse::new(status, self.into_body());
        response
            .headers_mut()
            .insert("content-type", "application/xml");
        if let Some(content_range) = content_range {
            response
                .headers_mut()
                .insert("content-range", content_range);
        }
        if let Some(content_length) = content_length {
            response
                .headers_mut()
                .insert("content-length", content_length.to_string());
        }
        if let Some(last_modified) = last_modified {
            response
                .headers_mut()
                .insert("last-modified", last_modified);
        }
        response
    }

    /// Gives the body as one frame. When `body_reads` is set, a read of that frame adds 1 to
    /// it. Else, when `frame_bytes` is set, gives the body as frames of that number of bytes.
    fn into_body(self) -> SdkBody {
        match (self.body_reads, self.frame_bytes) {
            (Some(body_reads), _) => SdkBody::from_body_1_x(StreamBody::new(
                futures::stream::iter([Ok::<_, Infallible>(Frame::data(Bytes::from(self.body)))])
                    .inspect(move |_| {
                        body_reads.fetch_add(1, Ordering::SeqCst);
                    }),
            )),
            (None, Some(frame_bytes)) => {
                let body = Bytes::from(self.body);
                let frames = body
                    .chunks(frame_bytes.max(1))
                    .map(|frame| Ok::<_, Infallible>(Frame::data(body.slice_ref(frame))))
                    .collect::<Vec<_>>();
                SdkBody::from_body_1_x(StreamBody::new(futures::stream::iter(frames)))
            }
            (None, None) => SdkBody::from(self.body),
        }
    }
}

type Script = dyn Fn(&SentRequest, usize) -> Answer + Send + Sync;

type SentRequests = Arc<Mutex<Vec<SentRequest>>>;

/// An HTTP transport that records each request and sends the response that a script gives for
/// it. When the script gives an error of the transport in place of a response, the transport
/// gives that error and sends no response.
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
        HttpConnectorFuture::ready((self.script)(&sent, earlier).into_response())
    }
}

/// Makes an S3 blob storage that sends its requests to a script.
///
/// The SDK does not retry, so each attempt of the blob storage is one request. The blob storage
/// makes 3 attempts with a delay of 1 ms.
fn scripted_storage(
    object_prefix: &str,
    script: impl Fn(&SentRequest, usize) -> Answer + Send + Sync + 'static,
) -> (S3BlobStorage, SentRequests) {
    scripted_storage_with(RetryConfig::disabled(), 3, object_prefix, script)
}

/// Makes an S3 blob storage that sends its requests to a script.
///
/// `sdk_retries` is the retry setting of the AWS SDK, which retries inside one attempt of the
/// blob storage. `attempts` is the number of attempts of the blob storage. The client reads no
/// environment, no profile file and no instance metadata. The blob storage waits 1 ms between
/// its attempts.
fn scripted_storage_with(
    sdk_retries: RetryConfig,
    attempts: u32,
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
        .retry_config(sdk_retries)
        .stalled_stream_protection(StalledStreamProtectionConfig::disabled())
        .http_client(http_client_fn(move |_, _| connector.clone()))
        .build();
    let mut config = S3BlobStorageConfig {
        object_prefix: object_prefix.to_string(),
        ..Default::default()
    };
    config.retries.max_attempts = attempts;
    config.retries.min_delay = Duration::from_millis(1);
    config.retries.max_delay = Duration::from_millis(1);
    config.retries.max_jitter_factor = None;
    (S3BlobStorage::with_sdk_config(config, sdk_config), requests)
}

fn sent(requests: &SentRequests) -> Vec<SentRequest> {
    requests.lock().unwrap().clone()
}

/// Gives the path of the URI of each request, without the endpoint and the query. The path style
/// of the client puts the bucket first in the path.
fn request_paths(requests: &SentRequests) -> Vec<String> {
    sent(requests)
        .iter()
        .map(|request| {
            request
                .uri
                .strip_prefix("http://s3.test")
                .unwrap_or(&request.uri)
                .split('?')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Gives the `BlobRangeError` of an error of the blob storage, or `None` for another error.
fn range_error(error: anyhow::Error) -> Option<BlobRangeError> {
    error.downcast_ref::<BlobRangeError>().copied()
}

/// Gives the `BodyLengthError` of an error of the blob storage, or `None` for another error.
fn length_error(error: anyhow::Error) -> Option<BodyLengthError> {
    error.downcast_ref::<BodyLengthError>().copied()
}

/// Gives the chunks as a stream of chunks without an error.
fn chunks(parts: &[&'static str]) -> impl Stream<Item = Result<Bytes, anyhow::Error>> + use<> {
    futures::stream::iter(
        parts
            .iter()
            .map(|part| Ok(Bytes::from_static(part.as_bytes())))
            .collect::<Vec<_>>(),
    )
}

/// Gives a text of `length` bytes, the letters from `a` to `z` again and again.
fn letters(length: usize) -> String {
    ('a'..='z').cycle().take(length).collect()
}

/// Gives the `BlobNameError` of an error of the blob storage, or `None` for another error.
fn name_error(error: anyhow::Error) -> Option<BlobNameError> {
    error.downcast_ref::<BlobNameError>().cloned()
}

/// Gives the `BlobMissingError` of an error of the blob storage, or `None` for another error.
///
/// `blob_store_error` in `golem_worker_executor::services::blob_store` downcasts the same way,
/// and makes a `BlobStoreError::NotFound`, which is permanent, of what it gets.
fn missing_error(error: anyhow::Error) -> Option<BlobMissingError> {
    error.downcast_ref::<BlobMissingError>().cloned()
}

/// The lines that a subscriber wrote, one JSON object for each event.
#[derive(Clone, Default)]
struct LogLines(Arc<Mutex<Vec<u8>>>);

impl LogLines {
    /// Gives the `op_label` of each event of the retry loop of `golem_common::retries`, in the
    /// order of the events.
    fn retry_ops(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["target"] == "golem_common::retries")
            .map(|event| {
                event["fields"]["op_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

impl std::io::Write for LogLines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogLines {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A subscriber that records no event, and that is interested in each callsite.
///
/// `open_each_callsite` makes it the default subscriber of the process.
struct OpenCallsites;

impl tracing::Subscriber for OpenCallsites {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, _event: &Event<'_>) {}

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

/// Makes `OpenCallsites` the default subscriber of the process, one time.
///
/// `tracing` keeps one `Interest` for each callsite, for the full process, and the first event
/// of a callsite makes that `Interest` from the subscriber of the thread that gives the event.
/// A thread with no subscriber makes `Interest::never`, and then `tracing` keeps each later
/// event of that callsite away from every subscriber, also from the subscriber that
/// `with_error_log` attaches to its own future. The tests share one process and more than one
/// thread, so the thread that first gives an event of the retry loop is not always the thread
/// of the test that reads the error log. A service sets its default subscriber before its
/// first request, and gets the `Interest` of each callsite from that subscriber.
///
/// `OpenCallsites` is interested in each callsite, so each callsite gets `Interest::sometimes`,
/// `tracing` asks the subscriber of the thread about each event, and the subscriber of
/// `with_error_log` gets each error that its future gives. `OpenCallsites` records no event, so
/// a test that attaches no subscriber gets no output.
fn open_each_callsite() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        tracing::subscriber::set_global_default(OpenCallsites)
            .expect("the tests set no other default subscriber of the process");
    });
}

/// Runs a future with a subscriber that records each event of the `error` level, and no event
/// of a lower level.
///
/// Gives the output of the future, and the `op_label` of each recorded event of the retry loop,
/// in the order of the events.
async fn with_error_log<T>(future: impl Future<Output = T>) -> (T, Vec<String>) {
    open_each_callsite();
    let lines = LogLines::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(Level::ERROR)
        .with_writer(lines.clone())
        .finish();
    let output = future.with_subscriber(subscriber).await;
    (output, lines.retry_ops())
}

/// Gives the number of failures that the metric `external_call_failure_total` counts for the
/// given operation, on every target.
///
/// The counter is cumulative over the process, so a test reads it before and after the
/// operation and uses the difference.
fn external_call_failures(op_label: &str) -> f64 {
    prometheus::gather()
        .iter()
        .filter(|family| family.name() == "external_call_failure_total")
        .flat_map(|family| family.get_metric())
        .filter(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| (label.name(), label.value()) == ("op", op_label))
        })
        .map(|metric| metric.get_counter().value())
        .sum()
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

/// The body of the error of a fault of the server itself. S3 gives the code `InternalError`
/// with the status 500, and the `PutObject` classifier of the SDK adds the code to
/// `TRANSIENT_ERRORS`, so a 4xx that carries the code asks for one more attempt as well.
const INTERNAL_ERROR: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>InternalError</Code><Message>We encountered an internal error</Message></Error>"#;

const NO_SUCH_KEY: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"#;

/// The body of the error of a bucket that is not there. S3 and MinIO give the code
/// `NoSuchBucket` with the status 404, as they give `NoSuchKey` with the status 404, so the
/// code is the one part of the error that a missing bucket and a missing source key do not
/// share.
const NO_SUCH_BUCKET: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist.</Message></Error>"#;

/// The body of the response of a `CopyObject` that S3 did.
const COPY_RESULT: &str = r#"<?xml version="1.0" encoding="UTF-8"?><CopyObjectResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><LastModified>2015-10-21T07:28:00.000Z</LastModified><ETag>"9b2cf535f27731c974343645a3985328"</ETag></CopyObjectResult>"#;

/// The body of the error of a body that is over the 5 GiB that one `PutObject` accepts. S3 and
/// MinIO give the code `EntityTooLarge` with the status 400 (`ErrEntityTooLarge` in
/// `cmd/api-errors.go`). The S3 model names no error of `PutObject` for this code, so the SDK
/// gives it as `PutObjectError::Unhandled` and keeps the code in the metadata of the error.
const ENTITY_TOO_LARGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>EntityTooLarge</Code><Message>Your proposed upload exceeds the maximum allowed object size.</Message></Error>"#;

/// The body of the error of a request that S3 asks the client to send again. S3 gives the code
/// `RequestTimeout` with the status 400, and `TRANSIENT_ERRORS` in
/// `aws_runtime::retries::classifiers` holds the code, so the status alone must not make a
/// permanent error of it.
const REQUEST_TIMEOUT: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>RequestTimeout</Code><Message>Your socket connection to the server was not read from or written to within the timeout period.</Message></Error>"#;

/// The body of the answer of a server that asks the client to send fewer requests. S3 and
/// MinIO give the code `SlowDown` with the status 503.
const SLOW_DOWN: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>SlowDown</Code><Message>Please reduce your request rate.</Message></Error>"#;

/// The body of another answer of a server that asks the client to send fewer requests.
/// `THROTTLING_ERRORS` in `aws_runtime::retries::classifiers` holds `ThrottlingException`, and
/// the classifier of the SDK reads the code and not the status of the response, so a 4xx that
/// carries the code asks for one more attempt.
const THROTTLING_EXCEPTION: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>ThrottlingException</Code><Message>Rate exceeded.</Message></Error>"#;

/// The body of the error of a conditional write to a key that already has an object. S3 gives
/// the code `PreconditionFailed` with the status 412 for a `PutObject` with `If-None-Match: *`,
/// and so does MinIO (`ErrPreconditionFailed` in `cmd/api-errors.go`).
const PRECONDITION_FAILED: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>PreconditionFailed</Code><Message>At least one of the pre-conditions you specified did not hold</Message><Condition>If-None-Match</Condition></Error>"#;

/// The body of the error of a conditional write that met a request on the same key. S3 gives
/// the code `ConditionalRequestConflict` with the status 409, and its documentation of
/// conditional writes says that a `PutObject` can go again after it.
const CONDITIONAL_REQUEST_CONFLICT: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>ConditionalRequestConflict</Code><Message>A conflicting conditional operation is currently in progress against this resource.</Message></Error>"#;

/// The body of the error of a request that is not valid. The S3 model names `InvalidRequest` as
/// an error of `PutObject`, so the SDK gives the `PutObjectError::InvalidRequest` variant for
/// the code, whatever status the response carries.
const INVALID_REQUEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>InvalidRequest</Code><Message>A parameter or header in your request is not valid.</Message></Error>"#;

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
        (result.map_err(range_error), sent(&requests).len()),
        (Err(Some(BlobRangeError { start: 3, end: 2 })), 0)
    );
}

#[test]
async fn get_raw_slice_refuses_an_inverted_range_before_it_checks_the_path() {
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, "abcdef"));

    let result = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("../x"),
            3,
            2,
        )
        .await;

    assert_eq!(
        (result.map_err(range_error), sent(&requests).len()),
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
            result.map_err(range_error),
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
async fn get_raw_slice_keeps_a_416_and_a_missing_object_out_of_the_error_log() {
    // The 416 and the missing key are not retriable, so each of those reads makes 1 attempt.
    // The read that gets a server error is the control. The blob storage makes 3 attempts for
    // it: the first 2 record a warning, which is not in the error log, and the last records an
    // error and counts a failure.
    let (storage, requests) = scripted_storage("", |request, _| {
        if request.uri.contains("outside") {
            Answer::new(416, INVALID_RANGE)
        } else if request.uri.contains("missing") {
            Answer::new(404, NO_SUCH_KEY)
        } else {
            Answer::new(500, INTERNAL_ERROR)
        }
    });
    let ops = [
        "get-raw-slice-416",
        "get-raw-slice-404",
        "get-raw-slice-500",
    ];
    let read = |path: &'static str, op_label: &'static str| {
        storage.get_raw_slice("test", op_label, namespace(), Path::new(path), 6, 9)
    };
    let failures_before = ops.map(external_call_failures);

    let ((outside, missing, failing), errors) = with_error_log(async {
        (
            read("outside", ops[0]).await.map_err(range_error),
            read("missing", ops[1]).await.map_err(range_error),
            read("failing", ops[2]).await.map_err(range_error),
        )
    })
    .await;
    let failures = ops
        .map(external_call_failures)
        .iter()
        .zip(failures_before)
        .map(|(after, before)| after - before)
        .collect::<Vec<_>>();

    assert_eq!(
        (
            outside,
            missing,
            failing,
            errors,
            failures,
            sent(&requests).len()
        ),
        (
            Err(Some(BlobRangeError { start: 6, end: 9 })),
            Ok(None),
            Err(None),
            vec![ops[2].to_string()],
            vec![0.0, 0.0, 1.0],
            5
        )
    );
}

#[test]
async fn get_raw_slice_takes_the_range_out_of_a_200_response() {
    // The script gives 200 with the full object, no content range and no content length. The
    // backend uses the body as the full object, which is what RFC 9110 lets a server that
    // ignores the range send (sections 14.2 and 15.5.17). The last range is the one that a
    // guest reaches the host with after it gives a negative offset for the start and the end.
    // The S3 case of `get_raw_slice_uses_inclusive_ranges` in `tests/blob_storage.rs` sends
    // the same range to MinIO.
    let (storage, requests) = scripted_storage("", |request, _| {
        if request.uri.contains("empty") {
            Answer::new(200, "")
        } else {
            Answer::new(200, "abcdef")
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

    let inside = read("blob", 1, 3).await.unwrap();
    let last_byte = read("blob", 5, 5).await.unwrap();
    let whole = read("blob", 0, 5).await.unwrap();
    let outside = [
        ("blob", 0, 6),
        ("blob", 6, 6),
        ("empty", 0, 0),
        ("blob", u64::MAX, u64::MAX),
    ];
    let range_errors = futures::stream::iter(outside)
        .then(|(path, start, end)| async move { read(path, start, end).await.map_err(range_error) })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        (
            inside,
            last_byte,
            whole,
            range_errors,
            sent(&requests).len()
        ),
        (
            Some(b"bcd".to_vec()),
            Some(b"f".to_vec()),
            Some(b"abcdef".to_vec()),
            outside
                .map(|(_, start, end)| Err(Some(BlobRangeError { start, end })))
                .to_vec(),
            7
        )
    );
}

#[test]
async fn get_raw_slice_refuses_a_range_past_the_content_length_without_reading_the_body() {
    // The script gives 200 with the full object and its content length, and counts the
    // bodies that get read.
    let body_reads = Arc::new(AtomicUsize::new(0));
    let (storage, requests) = scripted_storage("", {
        let body_reads = body_reads.clone();
        move |_, _| Answer::whole_object("abcdef", &body_reads)
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

    let outside = [(0, 6), (6, 6), (7, u64::MAX)];
    let range_errors = futures::stream::iter(outside)
        .then(|(start, end)| async move { read(start, end).await.map_err(range_error) })
        .collect::<Vec<_>>()
        .await;
    let bodies_read_outside = body_reads.load(Ordering::SeqCst);
    let inside = read(1, 3).await.unwrap();

    assert_eq!(
        (
            range_errors,
            bodies_read_outside,
            inside,
            body_reads.load(Ordering::SeqCst),
            sent(&requests).len()
        ),
        (
            outside
                .map(|(start, end)| Err(Some(BlobRangeError { start, end })))
                .to_vec(),
            0,
            Some(b"bcd".to_vec()),
            1,
            4
        )
    );
}

#[test]
async fn get_raw_slice_reads_the_status_of_the_attempt_that_gave_the_output() {
    // The SDK makes 2 attempts for the one request of the blob storage. The first gets a
    // server error, and the second gets the full object with the status 200.
    let (storage, requests) = scripted_storage_with(
        RetryConfig::standard()
            .with_max_attempts(2)
            .with_initial_backoff(Duration::from_millis(1)),
        1,
        "",
        |_, earlier| {
            if earlier == 0 {
                Answer::new(500, INTERNAL_ERROR)
            } else {
                Answer::new(200, "abcdef")
            }
        },
    );

    let result = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            1,
            3,
        )
        .await
        .unwrap();

    assert_eq!((result, sent(&requests).len()), (Some(b"bcd".to_vec()), 2));
}

#[test]
async fn get_raw_slice_checks_the_range_that_s3_returns() {
    let (storage, _) = scripted_storage("", |request, _| match request.range.as_deref() {
        Some("bytes=1-3") => Answer::partial(Some("bytes 1-3/6"), "bcd"),
        Some("bytes=5-5") => Answer::partial(Some("bytes 5-5/6"), "f"),
        Some("bytes=4-9") => Answer::partial(Some("bytes 4-5/6"), "ef"),
        Some("bytes=2-4") => Answer::partial(Some("bytes 0-3/6"), "abcd"),
        Some("bytes=1-4") => Answer::partial(Some("bytes 1-5/6"), "bcde"),
        Some("bytes=0-1") => Answer::partial(Some("bytes 0-3/6"), "abcd"),
        Some("bytes=0-2") => Answer::partial(Some("bytes 0-2/6"), "a"),
        Some("bytes=3-4") => Answer::partial(Some("bytes 3-4/6"), "def"),
        Some("bytes=3-5") => Answer::partial(Some("bytes 1-5/6"), "def"),
        _ => Answer {
            content_length: Some(6),
            ..Answer::partial(None, "abcdef")
        },
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
    let read_error = |start, end| async move { read(start, end).await.map_err(range_error) };

    let inside = read(1, 3).await.unwrap();
    let one_byte = read(5, 5).await.unwrap();
    let after_the_end = read_error(4, 9).await;
    let from_another_byte = read_error(2, 4).await;
    // The content range starts before the range, and the body has the bytes of the range.
    let starting_before_the_range = read_error(3, 5).await;
    // The content range ends after the range, and the body has the bytes of the range.
    let ending_after_the_range = read_error(1, 4).await;
    // The content range ends after the range, and the body has the bytes of the content range.
    let more_than_the_range = read_error(0, 1).await;
    let short_body = read_error(0, 2).await;
    let long_body = read_error(3, 4).await;
    let without_content_range = read_error(0, 5).await;
    // The backend does not use the body of a 206 response without a content range as the full
    // object, so its content length does not tell the backend whether the range is in the
    // object.
    let partial_without_content_range = read_error(0, 9).await;

    assert_eq!(
        (
            inside,
            one_byte,
            after_the_end,
            from_another_byte,
            starting_before_the_range,
            ending_after_the_range,
            more_than_the_range,
            short_body,
            long_body,
            without_content_range,
            partial_without_content_range
        ),
        (
            Some(b"bcd".to_vec()),
            Some(b"f".to_vec()),
            Err(Some(BlobRangeError { start: 4, end: 9 })),
            Err(None),
            Err(None),
            Err(None),
            Err(None),
            Err(None),
            Err(None),
            Err(None),
            Err(None)
        )
    );
}

#[test]
async fn read_body_reads_the_chunks_into_one_buffer_of_the_expected_length() {
    let with_length = read_body(chunks(&["abc", "de", "f"]), Some(6))
        .await
        .unwrap();
    let without_length = read_body(chunks(&["abc", "de", "f"]), None).await.unwrap();
    let empty = read_body(chunks(&[]), Some(0)).await.unwrap();

    assert_eq!(
        (
            with_length.capacity(),
            with_length,
            without_length,
            empty.capacity(),
            empty
        ),
        (6, b"abcdef".to_vec(), b"abcdef".to_vec(), 0, Vec::new())
    );
}

#[test]
async fn read_body_refuses_a_body_with_another_length() {
    let short = read_body(chunks(&["abc", "de"]), Some(6)).await;
    let long = read_body(chunks(&["abc", "def"]), Some(4)).await;
    let empty = read_body(chunks(&[]), Some(1)).await;

    assert_eq!(
        (
            short.map_err(length_error),
            long.map_err(length_error),
            empty.map_err(length_error)
        ),
        (
            Err(Some(BodyLengthError {
                expected: 6,
                read: 5
            })),
            Err(Some(BodyLengthError {
                expected: 4,
                read: 6
            })),
            Err(Some(BodyLengthError {
                expected: 1,
                read: 0
            }))
        )
    );
}

#[test]
async fn read_body_stops_at_the_first_chunk_past_the_expected_length() {
    // The read gets the error of the transport only if it reads the chunk after the one that
    // goes past the length.
    let body = futures::stream::iter([
        Ok(Bytes::from_static(b"abc")),
        Ok(Bytes::from_static(b"def")),
        Err(anyhow!("the connection closed")),
    ]);

    let result = read_body(body, Some(4)).await;

    assert_eq!(
        result.map_err(length_error),
        Err(Some(BodyLengthError {
            expected: 4,
            read: 6
        }))
    );
}

#[test]
async fn read_body_gives_the_error_of_a_chunk() {
    let body = || {
        futures::stream::iter([
            Ok(Bytes::from_static(b"ab")),
            Err(anyhow!("the connection closed")),
        ])
    };

    let with_length = read_body(body(), Some(6)).await;
    let without_length = read_body(body(), None).await;

    assert_eq!(
        (
            with_length.map_err(|error| error.to_string()),
            without_length.map_err(|error| error.to_string())
        ),
        (
            Err("the connection closed".to_string()),
            Err("the connection closed".to_string())
        )
    );
}

#[test]
async fn read_body_gives_an_error_for_a_length_that_it_cannot_reserve() {
    let result = read_body(chunks(&["abc"]), Some(u64::MAX)).await;

    assert_eq!(
        result.map_err(|error| error.is::<TryReserveError>()),
        Err(true)
    );
}

#[test]
fn cut_range_keeps_the_buffer_of_the_blob() {
    let blob = || b"abcdef".to_vec();
    let inside = cut_range(blob(), 1, 3).unwrap();
    let whole = cut_range(blob(), 0, 5).unwrap();
    let last_byte = cut_range(blob(), 5, 5).unwrap();
    let outside = [(0, 6), (6, 6), (3, 2), (u64::MAX, u64::MAX)];

    assert_eq!(
        (
            (inside.capacity(), inside),
            (whole.capacity(), whole),
            (last_byte.capacity(), last_byte),
            outside.map(|(start, end)| cut_range(blob(), start, end)),
            cut_range(Vec::new(), 0, 0)
        ),
        (
            (6, b"bcd".to_vec()),
            (6, b"abcdef".to_vec()),
            (6, b"f".to_vec()),
            outside.map(|(start, end)| Err(BlobRangeError { start, end })),
            Err(BlobRangeError { start: 0, end: 0 })
        )
    );
}

#[test]
async fn get_raw_reads_the_body_into_a_buffer_of_its_content_length() {
    // The body comes in frames. A buffer that grows as the frames arrive gets more capacity than
    // the body.
    let body = letters(1_000);
    let (storage, _) = scripted_storage("", {
        let body = body.clone();
        move |_, _| Answer {
            content_length: Some(body.len()),
            frame_bytes: Some(100),
            ..Answer::new(200, body.clone())
        }
    });

    let bytes = storage
        .get_raw("test", "get-raw", namespace(), Path::new("blob"))
        .await
        .unwrap()
        .unwrap();

    assert_eq!((bytes.capacity(), bytes), (1_000, body.into_bytes()));
}

#[test]
async fn get_raw_slice_reads_the_range_into_a_buffer_of_its_length() {
    // The body comes in frames. A buffer that grows as the frames arrive gets more capacity than
    // the body.
    let body = letters(1_000);
    let (storage, _) = scripted_storage("", {
        let body = body.clone();
        move |_, _| Answer {
            frame_bytes: Some(100),
            ..Answer::partial(Some("bytes 1-1000/2000"), &body)
        }
    });

    let bytes = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            1,
            1_000,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!((bytes.capacity(), bytes), (1_000, body.into_bytes()));
}

#[test]
async fn get_raw_slice_cuts_the_range_out_of_a_200_response_in_its_buffer() {
    // A buffer of the length of the range would show that the backend made a copy of the range.
    let body = letters(1_000);
    let body_reads = Arc::new(AtomicUsize::new(0));
    let (storage, _) = scripted_storage("", {
        let body = body.clone();
        move |_, _| Answer::whole_object(&body, &body_reads)
    });

    let bytes = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            1,
            3,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!((bytes.capacity(), bytes), (1_000, b"bcd".to_vec()));
}

#[test]
async fn get_raw_slice_refuses_a_body_with_another_length_than_the_range() {
    let (storage, _) = scripted_storage("", |request, _| match request.range.as_deref() {
        Some("bytes=0-2") => Answer::partial(Some("bytes 0-2/6"), "a"),
        _ => Answer::partial(Some("bytes 3-4/6"), "def"),
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

    let short = read(0, 2).await.unwrap_err();
    let long = read(3, 4).await.unwrap_err();

    assert_eq!(
        (
            short.to_string(),
            length_error(short),
            long.to_string(),
            length_error(long)
        ),
        (
            "the byte range 0-2".to_string(),
            Some(BodyLengthError {
                expected: 3,
                read: 1
            }),
            "the byte range 3-4".to_string(),
            Some(BodyLengthError {
                expected: 2,
                read: 3
            })
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
            Answer::new(200, "abcdef")
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
async fn get_raw_slice_retries_a_transport_error() {
    // The first attempt gets an I/O error of the transport, with no response, and the second
    // gets the full object. The SDK gives the error as `SdkError::DispatchFailure`, so the test
    // holds the `_` arm of `is_get_object_error_retriable` through that variant. A
    // `ConnectorError::timeout` takes the same path: `SdkError::TimeoutError` comes from the
    // attempt timeout of the SDK, not from the transport. The assertion cannot tell the error
    // from a 500, which a script could send in its place; what the test holds is that the arm
    // keeps `true`, which a 500 does not reach.
    let (storage, requests) = scripted_storage("", |_, earlier| match earlier {
        0 => Answer::transport_error(ConnectorError::io("connection reset".into())),
        _ => Answer::new(200, "abcdef"),
    });

    let result = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("blob"),
            0,
            2,
        )
        .await
        .unwrap();

    assert_eq!((result, sent(&requests).len()), (Some(b"abc".to_vec()), 2));
}

#[test]
async fn a_put_that_s3_rejects_sends_one_request() {
    // One `PutObject` carries the whole blob and accepts 5 GiB of it, so an attempt that sends
    // a body which S3 rejects costs the time of that body and gives the same answer again. The
    // retry loop makes 3 attempts, and it stops at the first of them here: the write of the
    // blob, and the write of the marker object of a directory, which `create_dir` sends
    // through the same predicate.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(400, ENTITY_TOO_LARGE));

    let written = storage
        .put_raw("test", "put-raw", namespace(), Path::new("blob"), b"x")
        .await;
    let created = storage
        .create_dir("test", "create-dir", namespace(), Path::new("dir"))
        .await;

    assert_eq!(
        (
            written.is_err(),
            created.is_err(),
            sent(&requests)
                .iter()
                .map(|request| request.method.clone())
                .collect::<Vec<_>>()
        ),
        (true, true, vec!["PUT".to_string(), "PUT".to_string()])
    );
}

#[test]
async fn a_put_that_the_model_names_a_fault_of_the_request_sends_one_request() {
    // The S3 model names `InvalidRequest` as an error of `PutObject`, so the SDK gives the
    // `PutObjectError::InvalidRequest` variant for the code and the backend reads the variant
    // and not the status. The script gives the code with the status 500, which the status rule
    // alone would send again: the model says that the request is the fault, so one more
    // attempt of the same request gets the same answer.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(500, INVALID_REQUEST));

    let written = storage
        .put_raw("test", "put-raw", namespace(), Path::new("blob"), b"x")
        .await;

    assert_eq!((written.is_err(), sent(&requests).len()), (true, 1));
}

#[test]
async fn a_put_that_one_more_attempt_can_pass_goes_again() {
    // The retry loop makes 3 attempts. A fault of the server keeps the loop, and so does a 4xx
    // whose code asks for one more attempt: S3 gives `RequestTimeout` with the status 400, and
    // `SlowDown` comes with the status 503, which keeps the loop by its status.
    let (after_a_server_error, server_error_requests) =
        scripted_storage("", |_, earlier| match earlier {
            0 => Answer::new(500, INTERNAL_ERROR),
            _ => Answer::new(200, ""),
        });
    let (timed_out, timeout_requests) =
        scripted_storage("", |_, _| Answer::new(400, REQUEST_TIMEOUT));
    let (slowed_down, slow_down_requests) =
        scripted_storage("", |_, _| Answer::new(503, SLOW_DOWN));
    let write = |storage: S3BlobStorage| async move {
        storage
            .put_raw("test", "put-raw", namespace(), Path::new("blob"), b"x")
            .await
            .is_ok()
    };

    let after_a_server_error = write(after_a_server_error).await;
    let timed_out = write(timed_out).await;
    let slowed_down = write(slowed_down).await;

    assert_eq!(
        (
            (after_a_server_error, sent(&server_error_requests).len()),
            (timed_out, sent(&timeout_requests).len()),
            (slowed_down, sent(&slow_down_requests).len())
        ),
        ((true, 2), (false, 3), (false, 3))
    );
}

#[test]
async fn a_put_that_a_throttling_code_answers_goes_again() {
    // `RETRIABLE_SERVICE_ERROR_CODES` holds every code of `THROTTLING_ERRORS` and of
    // `TRANSIENT_ERRORS`, which are the codes that the SDK itself sends again. A service behind
    // the S3 API can give a throttling code with a 4xx that is not 408 and not 429, and the
    // status rule alone would make a permanent error of that answer: the retry loop would stop
    // at an answer which asks for one more attempt. The loop makes 3 attempts and makes all 3
    // here. The filesystem snapshot of a worker writes to S3, so a `PutObject` that a throttle
    // answers must not reach the caller as a permanent error.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(400, THROTTLING_EXCEPTION));

    let written = storage
        .put_raw("test", "put-raw", namespace(), Path::new("blob"), b"x")
        .await;

    assert_eq!((written.is_err(), sent(&requests).len()), (true, 3));
}

#[test]
async fn a_put_that_an_internal_error_code_answers_goes_again() {
    // The `PutObject` classifier of the SDK adds `InternalError` to `TRANSIENT_ERRORS`, so
    // `RETRIABLE_SERVICE_ERROR_CODES` holds the code as well. S3 gives the code with the status
    // 500, which keeps the retry loop by its status, and a service behind the S3 API can give
    // the code with a 4xx, which the status rule alone would make a permanent error of. The
    // loop makes 3 attempts and makes all 3 here. The filesystem snapshot of a worker writes to
    // S3, so a `PutObject` that this code answers must not reach the caller as a permanent
    // error.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(400, INTERNAL_ERROR));

    let written = storage
        .put_raw("test", "put-raw", namespace(), Path::new("blob"), b"x")
        .await;

    assert_eq!((written.is_err(), sent(&requests).len()), (true, 3));
}

/// The backend holds its own copy of the codes that the SDK sends again, because `aws-runtime`
/// is the runtime support of the SDK and says that nothing uses it directly. The crate is a dev
/// dependency, so this test reads the two lists and the backend does not. A code that a later
/// version of the SDK adds to either list fails this test.
///
/// The test reads the two lists and nothing else. The classifier of one operation can add a
/// code of its own, which is in no list and is no constant: the `PutObject` classifier adds
/// `InternalError` (`RuntimePlugin for PutObject` in the generated `aws-sdk-s3` source
/// `src/operation/put_object.rs`). This test cannot see such a code, so a reader who wants to
/// check `RETRIABLE_SERVICE_ERROR_CODES` against it reads that classifier.
#[test]
fn the_retriable_codes_hold_every_code_that_the_sdk_sends_again() {
    let missing = THROTTLING_ERRORS
        .iter()
        .chain(TRANSIENT_ERRORS)
        .copied()
        .filter(|code| !RETRIABLE_SERVICE_ERROR_CODES.contains(code))
        .collect::<Vec<_>>();

    assert_eq!(missing, Vec::<&str>::new());
}

/// The key namespace of S3 is flat, so a blob at `a` and the marker of the directory `a` are two
/// objects that S3 holds at the same time. `list_dir` gives the path `a` for the blob, and the
/// parent of the marker, which is also the path `a`.
///
/// MinIO holds both objects, but its `ListObjectsV2` gives one key of the two, not both, so it
/// cannot give this response. The scripted transport is the one seam that holds this rule.
#[test]
async fn list_dir_gives_a_blob_and_the_marker_of_its_directory_one_time() {
    let prefix = namespace_prefix();
    let listing = list_page(
        &[
            (format!("{prefix}/a"), 5),
            (format!("{prefix}/a/__dir_marker"), 0),
        ],
        None,
    );
    let (storage, _) = scripted_storage("", move |_, _| Answer::new(200, listing.clone()));

    let entries = storage
        .list_dir("test", "list-dir", namespace(), Path::new(""))
        .await
        .unwrap();

    assert_eq!(entries, vec![PathBuf::from("a")]);
}

/// A `delete` of the blob at `a` removes the key `a` and keeps the marker of the directory `a`,
/// because the backend does not remove a marker on a write or on a delete of a blob. The
/// directory that `create_dir` made is still there, so `list_dir` still gives the path `a`.
#[test]
async fn list_dir_gives_a_directory_whose_blob_is_deleted() {
    let prefix = namespace_prefix();
    let listing = list_page(&[(format!("{prefix}/a/__dir_marker"), 0)], None);
    let (storage, _) = scripted_storage("", move |_, _| Answer::new(200, listing.clone()));

    let entries = storage
        .list_dir("test", "list-dir", namespace(), Path::new(""))
        .await
        .unwrap();

    assert_eq!(entries, vec![PathBuf::from("a")]);
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

#[test]
async fn put_raw_rejects_a_name_that_breaks_a_rule_without_a_request() {
    // The key of `namespace()` in a storage without an object prefix is the 36 bytes of the
    // nil UUID, `/`, and the name. The last name has 988 bytes, so its key has 1025.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));
    let names = [
        "a\0b".to_string(),
        " . ".to_string(),
        "a/ .. /b".to_string(),
        "a\\..\\b".to_string(),
        "dir/__dir_marker".to_string(),
        "a".repeat(988),
    ];

    let errors = futures::stream::iter(&names)
        .then(|name| async {
            storage
                .put_raw("test", "put-raw", namespace(), Path::new(name), b"x")
                .await
                .map_err(name_error)
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        (errors, sent(&requests).len()),
        (
            vec![
                Err(Some(BlobNameError::NulByte)),
                Err(Some(BlobNameError::DotSegment {
                    segment: " . ".to_string()
                })),
                Err(Some(BlobNameError::DotSegment {
                    segment: " .. ".to_string()
                })),
                Err(Some(BlobNameError::DotSegment {
                    segment: "..".to_string()
                })),
                Err(Some(BlobNameError::Reserved {
                    marker: "__dir_marker"
                })),
                Err(Some(BlobNameError::TooLong {
                    length: 1025,
                    max: 1024
                })),
            ],
            0
        )
    );
}

#[test]
async fn the_key_limit_counts_bytes_of_utf8_and_not_characters() {
    // Each name has 600 characters, which is under the limit. The first has 1200 bytes,
    // because `é` has 2 bytes of UTF-8, so its key has 1237. The second has 600 bytes.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));
    let storage = &storage;
    let write = |name: String| async move {
        storage
            .put_raw("test", "put-raw", namespace(), Path::new(&name), b"x")
            .await
            .map_err(name_error)
    };

    let multi_byte = write("é".repeat(600)).await;
    let single_byte = write("a".repeat(600)).await;

    assert_eq!(
        (multi_byte, single_byte, sent(&requests).len()),
        (
            Err(Some(BlobNameError::TooLong {
                length: 1237,
                max: 1024
            })),
            Ok(()),
            1
        )
    );
}

#[test]
async fn the_key_limit_counts_the_namespace_prefix() {
    // The prefix of `namespace()` is `objects/`, the 36 bytes of the nil UUID, and `/`: 45
    // bytes. A name of 979 bytes gives a key of 1024 bytes, and a name of 980 bytes a key of
    // 1025, although both names have fewer than 1024 bytes on their own.
    let (storage, requests) = scripted_storage("objects", |_, _| Answer::new(200, ""));
    let storage = &storage;
    let write = |name: String| async move {
        storage
            .put_raw("test", "put-raw", namespace(), Path::new(&name), b"x")
            .await
            .map_err(name_error)
    };

    let at_the_limit = write("a".repeat(979)).await;
    let over_the_limit = write("a".repeat(980)).await;

    assert_eq!(
        (
            at_the_limit,
            over_the_limit,
            sent(&requests)
                .iter()
                .map(|request| {
                    request.uri.contains(&format!(
                        "/objects/{}/{}?",
                        namespace_prefix(),
                        "a".repeat(979)
                    ))
                })
                .collect::<Vec<_>>()
        ),
        (
            Ok(()),
            Err(Some(BlobNameError::TooLong {
                length: 1025,
                max: 1024
            })),
            vec![true]
        )
    );
}

#[test]
async fn a_name_that_ends_with_the_marker_is_rejected_and_the_error_names_it() {
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));

    let written = storage
        .put_raw(
            "test",
            "put-raw",
            namespace(),
            Path::new("dir/__dir_marker"),
            b"x",
        )
        .await
        .unwrap_err();
    let created = storage
        .create_dir("test", "create-dir", namespace(), Path::new("__dir_marker"))
        .await
        .unwrap_err();

    assert_eq!(
        (
            written.to_string().contains("__dir_marker"),
            created.to_string().contains("__dir_marker"),
            name_error(written),
            name_error(created),
            sent(&requests).len()
        ),
        (
            true,
            true,
            Some(BlobNameError::Reserved {
                marker: "__dir_marker"
            }),
            Some(BlobNameError::Reserved {
                marker: "__dir_marker"
            }),
            0
        )
    );
}

#[test]
async fn a_name_with_the_marker_in_the_middle_is_written_read_and_listed() {
    // The marker is reserved as the last segment only. This test shows what a name with the
    // marker as a middle segment does: the backend sends it to S3 as the guest wrote it, gives
    // the object back under it, and keeps it in the blob listing, because the filter of the
    // listing reads the last segment only.
    let prefix = namespace_prefix();
    let listing = list_page(
        &[
            (format!("{prefix}/tree/__dir_marker/b"), 1),
            (format!("{prefix}/tree/x/__dir_marker"), 0),
        ],
        None,
    );
    let (storage, requests) = scripted_storage("", move |request, _| {
        if request.is_list_objects() {
            Answer::new(200, listing.clone())
        } else if request.method == "GET" {
            Answer::new(200, "b")
        } else {
            Answer::new(200, "")
        }
    });
    let path = Path::new("tree/__dir_marker/b");

    storage
        .put_raw("test", "put-raw", namespace(), path, b"b")
        .await
        .unwrap();
    let read = storage
        .get_raw("test", "get-raw", namespace(), path)
        .await
        .unwrap();
    let listed = storage
        .list_blobs_below("test", "list", namespace(), Path::new("tree"))
        .await
        .unwrap();

    assert_eq!(
        (
            read,
            listed.into_vec(),
            sent(&requests)
                .iter()
                .map(|request| (
                    request.method.clone(),
                    request.uri.contains("/tree/__dir_marker/b")
                ))
                .collect::<Vec<_>>()
        ),
        (
            Some(b"b".to_vec()),
            vec![listed_blob("tree/__dir_marker/b", 1)],
            vec![
                ("PUT".to_string(), true),
                ("GET".to_string(), true),
                ("GET".to_string(), false)
            ]
        )
    );
}

#[test]
async fn create_dir_writes_the_marker_and_the_listing_leaves_it_out() {
    let prefix = namespace_prefix();
    let listing = list_page(
        &[
            (format!("{prefix}/tree/__dir_marker"), 0),
            (format!("{prefix}/tree/a"), 1),
        ],
        None,
    );
    // A `HEAD` of `tree` itself finds nothing, because S3 has no directory object of its own,
    // and the listing that `exists` falls back on (the one with `start-after`) finds nothing
    // either. The object at `tree/__dir_marker` is therefore the one thing that can make
    // `exists` give `Directory`.
    let (storage, requests) = scripted_storage("", move |request, _| {
        if request.is_list_objects() {
            if request.uri.contains("start-after") {
                Answer::new(200, list_page(&[], None))
            } else {
                Answer::new(200, listing.clone())
            }
        } else if request.method == "HEAD" && !request.uri.ends_with("__dir_marker") {
            Answer::new(404, "")
        } else {
            Answer::new(200, "")
        }
    });

    storage
        .create_dir("test", "create-dir", namespace(), Path::new("tree"))
        .await
        .unwrap();
    let marked = storage
        .exists("test", "exists", namespace(), Path::new("tree"))
        .await
        .unwrap();
    let listed = storage
        .list_blobs_below("test", "list", namespace(), Path::new("tree"))
        .await
        .unwrap();

    assert_eq!(
        (
            marked,
            listed.into_vec(),
            sent(&requests)
                .iter()
                .map(|request| (
                    request.method.clone(),
                    request
                        .uri
                        .contains(&format!("/{prefix}/tree/__dir_marker"))
                ))
                .collect::<Vec<_>>()
        ),
        (
            ExistsResult::Directory,
            vec![listed_blob("tree/a", 1)],
            vec![
                ("PUT".to_string(), true),
                ("HEAD".to_string(), false),
                ("HEAD".to_string(), true),
                ("GET".to_string(), false)
            ]
        )
    );
}

#[test]
async fn create_dir_rejects_a_directory_whose_marker_does_not_fit_the_key_limit() {
    // The name has 987 bytes, so its key has 1024 bytes and a blob can have it. The key of
    // the marker of a directory with the name has 13 bytes more.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));
    let name = "a".repeat(987);

    let written = storage
        .put_raw("test", "put-raw", namespace(), Path::new(&name), b"x")
        .await
        .map_err(name_error);
    let created = storage
        .create_dir("test", "create-dir", namespace(), Path::new(&name))
        .await
        .map_err(name_error);

    assert_eq!(
        (written, created, sent(&requests).len()),
        (
            Ok(()),
            Err(Some(BlobNameError::TooLong {
                length: 1037,
                max: 1024
            })),
            1
        )
    );
}

#[test]
async fn exists_tells_the_truth_for_a_name_whose_marker_does_not_fit_the_key_limit() {
    // The key of a name of 975 bytes has 1012 bytes, so the key of the marker of a directory
    // with that name has 1025 and does not fit. A child of that directory still fits: the key
    // of a child with a name of one byte has 1014 bytes. No marker object of such a directory
    // can be there, so `exists` sends no request for one, and the objects below the prefix
    // decide. A name that is too long for a marker is a name that a blob can still have, so
    // an error here would tell a guest that its question is invalid when `exists` can give
    // `Directory` or `DoesNotExist`.
    let name = "a".repeat(975);
    let prefix = namespace_prefix();
    let with_a_child = list_page(&[(format!("{prefix}/{name}/c"), 1)], None);
    let (with_child, with_child_requests) = scripted_storage("", move |request, _| {
        if request.is_list_objects() {
            Answer::new(200, with_a_child.clone())
        } else {
            Answer::new(404, "")
        }
    });
    let (empty, empty_requests) = scripted_storage("", |request, _| {
        if request.is_list_objects() {
            Answer::new(200, list_page(&[], None))
        } else {
            Answer::new(404, "")
        }
    });

    let directory = with_child
        .exists("test", "exists", namespace(), Path::new(&name))
        .await
        .map_err(name_error);
    let nothing = empty
        .exists("test", "exists", namespace(), Path::new(&name))
        .await
        .map_err(name_error);

    let with_child_sent = sent(&with_child_requests);
    let empty_sent = sent(&empty_requests);
    assert_eq!(
        (
            directory,
            nothing,
            with_child_sent
                .iter()
                .chain(empty_sent.iter())
                .map(|request| (request.method.clone(), request.uri.contains("__dir_marker")))
                .collect::<Vec<_>>()
        ),
        (
            Ok(ExistsResult::Directory),
            Ok(ExistsResult::DoesNotExist),
            vec![
                ("HEAD".to_string(), false),
                ("GET".to_string(), false),
                ("HEAD".to_string(), false),
                ("GET".to_string(), false)
            ]
        )
    );
}

#[test]
async fn get_metadata_gives_none_for_a_missing_name_whose_marker_does_not_fit_the_key_limit() {
    // The key of a name of 987 bytes has 1024 bytes, the most that S3 accepts, so the key of
    // the marker of a directory with that name has 1037 and does not fit. `put_raw` writes a
    // blob at that name, so a question about it is a valid question, and nothing is there:
    // one `HEAD` of the name, no request for a marker, and `None`.
    let name = "a".repeat(987);
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(404, ""));

    let metadata = storage
        .get_metadata("test", "get-metadata", namespace(), Path::new(&name))
        .await
        .map(|metadata| metadata.is_some())
        .map_err(name_error);

    assert_eq!(
        (
            metadata,
            sent(&requests)
                .iter()
                .map(|request| (request.method.clone(), request.uri.contains("__dir_marker")))
                .collect::<Vec<_>>()
        ),
        (Ok(false), vec![("HEAD".to_string(), false)])
    );
}

#[test]
async fn the_marker_key_of_a_root_path_has_one_separator() {
    // The key of a root path is the prefix of the namespace and a `/` after it, so a marker
    // key that always puts a separator of its own before the marker would have `//` in it.
    // MinIO rejects such a key with `XMinioInvalidObjectName`, and no rule of `BlobNameError`
    // reads the marker key, so nothing else would catch it. `get_metadata` is the one method
    // that reads the marker of a root path: `exists` gives `Directory` for such a path and
    // sends no request, and `create_dir` leaves no directory at the root.
    let prefix = namespace_prefix();
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(404, ""));

    let root = storage
        .get_metadata("test", "get-metadata", namespace(), Path::new(""))
        .await
        .map(|metadata| metadata.is_some())
        .map_err(name_error);

    assert_eq!(
        (
            root,
            sent(&requests)
                .iter()
                .map(|request| request.uri.clone())
                .collect::<Vec<_>>()
        ),
        (
            Ok(false),
            vec![
                format!("http://s3.test/custom-data/{prefix}/"),
                format!("http://s3.test/custom-data/{prefix}/__dir_marker"),
            ]
        )
    );
}

#[test]
async fn exists_gives_a_directory_for_a_root_path_and_sends_no_request() {
    // The root of a namespace is a directory, also when the bucket holds no object under its
    // prefix, so the answer needs no request of its own.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(404, ""));

    let root = storage
        .exists("test", "exists", namespace(), Path::new("."))
        .await
        .map_err(name_error);

    assert_eq!(
        (root, sent(&requests).len()),
        (Ok(ExistsResult::Directory), 0)
    );
}

#[test]
async fn the_reserved_rule_keeps_the_marker_object_of_a_directory_free() {
    // `create_dir("x")` records the directory `x` with an object at the key
    // `<prefix>/x/__dir_marker`, and `exists` reads that object as the mark of the directory.
    // `exists` sends its first `HEAD` for the key of the path itself, as the second request
    // below shows, so `exists("x/__dir_marker")` would send a `HEAD` for
    // `<prefix>/x/__dir_marker`, find the marker object of `x`, and give `File` for a
    // directory that the guest had just made. `get_metadata` would give the size of that
    // object, and `create_dir("x/__dir_marker")` would write its own marker one level below
    // it. The reserved rule keeps that one key for the backend: the three calls give the
    // permanent error and send no request.
    let prefix = namespace_prefix();
    let (storage, requests) = scripted_storage("", |request, _| {
        if request.method != "HEAD" {
            Answer::new(200, "")
        } else if request.uri.ends_with("__dir_marker") {
            Answer::object_head()
        } else {
            Answer::new(404, "")
        }
    });
    let marker_path = Path::new("x/__dir_marker");

    storage
        .create_dir("test", "create-dir", namespace(), Path::new("x"))
        .await
        .unwrap();
    let directory = storage
        .exists("test", "exists", namespace(), Path::new("x"))
        .await
        .map_err(name_error);
    let created = storage
        .create_dir("test", "create-dir", namespace(), marker_path)
        .await
        .map_err(name_error);
    let checked = storage
        .exists("test", "exists", namespace(), marker_path)
        .await
        .map_err(name_error);
    let described = storage
        .get_metadata("test", "get-metadata", namespace(), marker_path)
        .await
        .map(|metadata| metadata.is_some())
        .map_err(name_error);

    assert_eq!(
        (
            directory,
            created,
            checked,
            described,
            sent(&requests)
                .iter()
                .map(|request| (request.method.clone(), request.uri.clone()))
                .collect::<Vec<_>>()
        ),
        (
            Ok(ExistsResult::Directory),
            Err(Some(BlobNameError::Reserved {
                marker: "__dir_marker"
            })),
            Err(Some(BlobNameError::Reserved {
                marker: "__dir_marker"
            })),
            Err(Some(BlobNameError::Reserved {
                marker: "__dir_marker"
            })),
            vec![
                (
                    "PUT".to_string(),
                    format!("http://s3.test/custom-data/{prefix}/x/__dir_marker?x-id=PutObject")
                ),
                (
                    "HEAD".to_string(),
                    format!("http://s3.test/custom-data/{prefix}/x")
                ),
                (
                    "HEAD".to_string(),
                    format!("http://s3.test/custom-data/{prefix}/x/__dir_marker")
                ),
            ]
        )
    );
}

#[test]
async fn create_dir_at_a_root_path_sends_no_request() {
    // A path with no name in it is the root of the namespace, which needs no marker object:
    // the prefix of the namespace is there as soon as one object is below it. This is what
    // the matrix test `create_dir_at_a_root_path_leaves_nothing_behind` states for every
    // backend, and the S3 half of it belongs here, because a marker object at the root of a
    // namespace stays out of every listing of the backend: `list_dir` and `list_blobs_below`
    // leave the marker of the directory that they list out, so no call of `BlobStorage` could
    // see a stray one.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));

    for root_path in ["", ".", "./", "././"] {
        storage
            .create_dir("test", "create-dir", namespace(), Path::new(root_path))
            .await
            .unwrap_or_else(|err| panic!("create_dir({root_path:?}) gave {err}"));
    }

    assert_eq!(
        sent(&requests)
            .iter()
            .map(|request| request.uri.clone())
            .collect::<Vec<_>>(),
        Vec::<String>::new()
    );
}

/// Gives the method, the target and the copy source of each request, in the order of the
/// requests. A `CopyObject` is a `PUT` of the target key with the source key in its
/// `x-amz-copy-source` header, so these three show which request is a copy.
fn copy_requests(requests: &SentRequests) -> Vec<(String, String, Option<String>)> {
    sent(requests)
        .iter()
        .map(|request| {
            (
                request.method.clone(),
                request.uri.clone(),
                request.copy_source.clone(),
            )
        })
        .collect()
}

/// Gives the one `CopyObject` request of a copy from `from` to `to` in `namespace()`.
fn one_copy_request(from: &str, to: &str) -> Vec<(String, String, Option<String>)> {
    let prefix = namespace_prefix();
    vec![(
        "PUT".to_string(),
        format!("http://s3.test/custom-data/{prefix}/{to}?x-id=CopyObject"),
        Some(format!("/custom-data/{prefix}/{from}")),
    )]
}

#[test]
async fn copy_gives_a_missing_error_for_a_source_that_is_not_there_and_sends_one_request() {
    // The S3 model names one error of `CopyObject`, `ObjectNotInActiveTierError`, so a source
    // key that is not there comes as the code `NoSuchKey` in the body of the response, which
    // the SDK keeps in the metadata of a `CopyObjectError::Unhandled`. The backend reads that
    // code, gives the `BlobMissingError` that the default `copy` gives, and sends the request
    // one time: a retry cannot make the bucket hold the source key. The storage sends 3
    // requests for a retriable error, which `copy_retries_a_server_error` holds.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(404, NO_SUCH_KEY));

    let result = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("from"),
            Path::new("to"),
        )
        .await;

    assert_eq!(
        (result.map_err(missing_error), copy_requests(&requests)),
        (
            Err(Some(BlobMissingError {
                path: PathBuf::from("from")
            })),
            one_copy_request("from", "to")
        )
    );
}

#[test]
async fn move_gives_a_missing_error_for_a_source_that_is_not_there_and_deletes_nothing() {
    // `move` is the default one of `BlobStorage`: the copy comes first, and the delete of the
    // source comes after it. The copy gives the error, so the delete does not run and the one
    // request of the move is that copy. No backend has a `move` of its own.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(404, NO_SUCH_KEY));

    let result = storage
        .r#move(
            "test",
            "move",
            namespace(),
            Path::new("from"),
            Path::new("to"),
        )
        .await;

    assert_eq!(
        (result.map_err(missing_error), copy_requests(&requests)),
        (
            Err(Some(BlobMissingError {
                path: PathBuf::from("from")
            })),
            one_copy_request("from", "to")
        )
    );
}

#[test]
async fn copy_retries_a_server_error() {
    // The first request gets a 500 and the second gets the result of the copy. The test of a
    // missing source key matches one code, so every other error of the service keeps its retry.
    let (storage, requests) = scripted_storage("", |_, earlier| match earlier {
        0 => Answer::new(500, INTERNAL_ERROR),
        _ => Answer::new(200, COPY_RESULT),
    });

    let result = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("from"),
            Path::new("to"),
        )
        .await;

    assert_eq!(
        (result.map_err(missing_error), sent(&requests).len()),
        (Ok(()), 2)
    );
}

#[test]
async fn copy_keeps_the_retry_of_a_bucket_that_is_not_there() {
    // A source key that is not there and a bucket that is not there are two conditions, and
    // the backend reads the code of the error to know which one it has. `NoSuchKey` says that
    // the bucket holds no object at the source key, which no retry can change, so the copy
    // gives a `BlobMissingError` and stops. `NoSuchBucket` says that the bucket itself is not
    // there, which is of the configuration of the storage, so the error keeps its retry and
    // the guest does not read "no blob at the path" for it.
    //
    // Each code comes with the status 404 (`com.amazonaws.s3#NoSuchBucket` in the S3 model,
    // and `ErrNoSuchBucket` in `cmd/api-errors.go` of MinIO), so a test of the status, or of
    // any code with that status, gives the same result for the two. The storage sends 3
    // requests for a retriable error, as `copy_retries_a_server_error` holds, and 1 request
    // for a source key that is not there.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(404, NO_SUCH_BUCKET));

    let result = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("from"),
            Path::new("to"),
        )
        .await;

    assert_eq!(
        (result.map_err(missing_error), copy_requests(&requests)),
        (Err(None), vec![one_copy_request("from", "to"); 3].concat())
    );
}

#[test]
async fn a_copy_onto_the_same_path_reads_the_key_of_the_source_and_no_more() {
    // The copy needs a blob at its source path, and the `HeadObject` of the key of that blob
    // gives the answer. A directory at the same path is not a blob, so the copy asks nothing
    // about it: it sends no `HeadObject` for the marker object and no listing of the keys below
    // the path. The copy onto the same path writes nothing, so the one request is that head.
    let (storage, requests) = scripted_storage("", |request, _| {
        if request.method == "HEAD" && request.uri.ends_with("/from") {
            Answer::object_head()
        } else {
            Answer::new(500, INTERNAL_ERROR)
        }
    });

    let result = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("from"),
            Path::new("./from"),
        )
        .await;

    let prefix = namespace_prefix();
    assert_eq!(
        (
            result.map_err(missing_error),
            sent(&requests)
                .iter()
                .map(|request| (request.method.clone(), request.uri.clone()))
                .collect::<Vec<_>>()
        ),
        (
            Ok(()),
            vec![(
                "HEAD".to_string(),
                format!("http://s3.test/custom-data/{prefix}/from")
            )]
        )
    );
}

#[test]
async fn a_copy_onto_the_same_path_gives_the_missing_error_at_the_head_of_the_source() {
    // The head of the key of the source tells that the bucket holds no blob at the path, and
    // that answer is final: a directory at the path holds no blob either. So the copy gives the
    // permanent `BlobMissingError` at that head, and a later request cannot turn it into an
    // error that one more attempt can pass. The script gives an error of the transport to each
    // request after the head, so a copy that asked more would give that error to the guest and
    // the executor would retry a copy whose source is not there.
    let (storage, requests) = scripted_storage("", |request, _| {
        if request.method == "HEAD" && request.uri.ends_with("/from") {
            // A response to a `HEAD` has no body, so the SDK reads its status.
            Answer::new(404, "")
        } else {
            Answer::transport_error(ConnectorError::io("connection reset".into()))
        }
    });

    let result = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("from"),
            Path::new("from"),
        )
        .await;

    assert_eq!(
        (result.map_err(missing_error), sent(&requests).len()),
        (
            Err(Some(BlobMissingError {
                path: PathBuf::from("from")
            })),
            1
        )
    );
}

#[test]
async fn copy_names_the_source_path_as_the_guest_wrote_it() {
    // A guest picks the source container name and the source object name, so the guest writes
    // the path of the source. `./from` and `from` are two forms of one path, and the backend
    // normalizes the path before it makes the key of the object. The `BlobMissingError` names
    // the path as the guest wrote it, as each `BlobNameError` does, because the guest reads the
    // message.
    //
    // The copy onto the same path reads the source and sends no `CopyObject`; the copy onto
    // another path sends one. Each gives the error, and each names `./from`.
    let (storage, _) = scripted_storage("", |request, _| {
        if request.method == "HEAD" {
            // A response to a `HEAD` has no body, so the SDK reads its status.
            Answer::new(404, "")
        } else if request.is_list_objects() {
            Answer::new(200, list_page(&[], None))
        } else {
            Answer::new(404, NO_SUCH_KEY)
        }
    });

    let onto_the_same_path = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("./from"),
            Path::new("from"),
        )
        .await;
    let onto_another_path = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("./from"),
            Path::new("to"),
        )
        .await;

    assert_eq!(
        (
            onto_the_same_path.map_err(missing_error),
            onto_another_path.map_err(missing_error)
        ),
        (
            Err(Some(BlobMissingError {
                path: PathBuf::from("./from")
            })),
            Err(Some(BlobMissingError {
                path: PathBuf::from("./from")
            }))
        )
    );
}

#[test]
async fn copy_reads_the_code_of_the_error_and_not_the_status_of_the_response() {
    // A `CopyObject` can get an error in a response with the status 200: the SDK reads the body
    // of such a response, sees the `Error` element and makes the error of it
    // (`CopyObjectResponseDeserializer` in `aws_sdk_s3::operation::copy_object`, and "Response
    // and special errors" in the S3 API reference of `CopyObject`). The backend reads the code
    // of the error out of its metadata, so the status of the response does not change what the
    // copy gives.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, NO_SUCH_KEY));

    let result = storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("from"),
            Path::new("to"),
        )
        .await;

    assert_eq!(
        (result.map_err(missing_error), copy_requests(&requests)),
        (
            Err(Some(BlobMissingError {
                path: PathBuf::from("from")
            })),
            one_copy_request("from", "to")
        )
    );
}

#[test]
async fn copy_keeps_a_source_that_is_not_there_out_of_the_error_log() {
    // The missing source key is not retriable, so that copy sends 1 request. The copy that gets
    // a server error is the control. The blob storage sends 3 requests for it: the first 2
    // record a warning, which is not in the error log, and the last records an error and counts
    // a failure. A missing key of a `GetObject` and of a `HeadObject` has the same policy: the
    // guest picks the name, S3 did the work of the request, and the error goes to the guest.
    //
    // The source key of a copy is in the `x-amz-copy-source` header, and the target key is in
    // the URI, so the script reads the header to find which copy the request is.
    let (storage, requests) = scripted_storage("", |request, _| {
        if request
            .copy_source
            .as_deref()
            .is_some_and(|source| source.ends_with("missing"))
        {
            Answer::new(404, NO_SUCH_KEY)
        } else {
            Answer::new(500, INTERNAL_ERROR)
        }
    });
    let ops = ["copy-404", "copy-500"];
    let copy = |from: &'static str, op_label: &'static str| {
        storage.copy(
            "test",
            op_label,
            namespace(),
            Path::new(from),
            Path::new("to"),
        )
    };
    let failures_before = ops.map(external_call_failures);

    let ((missing, failing), errors) = with_error_log(async {
        (
            copy("missing", ops[0]).await.map_err(missing_error),
            copy("failing", ops[1]).await.map_err(missing_error),
        )
    })
    .await;
    let failures = ops
        .map(external_call_failures)
        .iter()
        .zip(failures_before)
        .map(|(after, before)| after - before)
        .collect::<Vec<_>>();

    assert_eq!(
        (missing, failing, errors, failures, sent(&requests).len()),
        (
            Err(Some(BlobMissingError {
                path: PathBuf::from("missing")
            })),
            Err(None),
            vec![ops[1].to_string()],
            vec![0.0, 1.0],
            4
        )
    );
}

/// The bodies a fake S3 received, and the status it sends for each `PUT`.
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

/// Makes a blob storage that talks to a server that sends the next status for each `PUT`.
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

#[test]
async fn put_raw_if_absent_sends_if_none_match_and_writes_where_the_key_has_no_object() {
    // A 200 is the answer of S3 to a conditional write for a key that has no object. Without
    // the header, S3 writes over an object that is there, so the header is what makes the
    // write conditional.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));

    let result = storage
        .put_raw_if_absent(
            "test",
            "put-if-absent",
            namespace(),
            Path::new("blob"),
            b"x",
        )
        .await
        .unwrap();

    assert_eq!(
        (
            result,
            sent(&requests)
                .iter()
                .map(|request| (request.method.clone(), request.if_none_match.clone()))
                .collect::<Vec<_>>()
        ),
        (
            PutIfAbsent::Written,
            vec![("PUT".to_string(), Some("*".to_string()))]
        )
    );
}

#[test]
async fn put_raw_if_absent_gives_already_exists_for_a_412_after_one_request_and_logs_no_error() {
    // A 412 is the answer and not a failure: the key has an object. The loop sends no second
    // request for it, records no error and counts no failure. The write that gets a server
    // error is the control: it sends 3 requests, records an error and counts a failure.
    let (storage, requests) = scripted_storage("", |request, _| {
        // The URI of a `PutObject` ends with a query, so the script reads the path before it.
        if request
            .uri
            .split('?')
            .next()
            .is_some_and(|path| path.ends_with("/exists"))
        {
            Answer::new(412, PRECONDITION_FAILED)
        } else {
            Answer::new(500, INTERNAL_ERROR)
        }
    });
    let ops = ["put-if-absent-412", "put-if-absent-500"];
    let failures_before = ops.map(external_call_failures);

    let ((exists, failing), errors) = with_error_log(async {
        (
            storage
                .put_raw_if_absent("test", ops[0], namespace(), Path::new("exists"), b"x")
                .await
                .map_err(|error| error.to_string()),
            storage
                .put_raw_if_absent("test", ops[1], namespace(), Path::new("failing"), b"x")
                .await
                .is_err(),
        )
    })
    .await;
    let failures = ops
        .map(external_call_failures)
        .iter()
        .zip(failures_before)
        .map(|(after, before)| after - before)
        .collect::<Vec<_>>();

    assert_eq!(
        (exists, failing, errors, failures, sent(&requests).len()),
        (
            Ok(PutIfAbsent::AlreadyExists),
            true,
            vec![ops[1].to_string()],
            vec![0.0, 1.0],
            4
        )
    );
}

#[test]
async fn put_raw_if_absent_sends_a_write_that_a_409_answers_again() {
    // S3 gives a 409 when a request on the same key ran at the same time, for example a delete that
    // finished before the write. Its documentation says that a `PutObject` can go again after it.
    // The status rule of `put_raw` stops at a 4xx. So this is the one answer that the conditional
    // write treats in another way.
    let (storage, requests) = scripted_storage("", |_, earlier| match earlier {
        0 => Answer::new(409, CONDITIONAL_REQUEST_CONFLICT),
        _ => Answer::new(200, ""),
    });

    let result = storage
        .put_raw_if_absent(
            "test",
            "put-if-absent",
            namespace(),
            Path::new("blob"),
            b"x",
        )
        .await
        .unwrap();

    assert_eq!((result, sent(&requests).len()), (PutIfAbsent::Written, 2));
}

#[test]
async fn put_raw_if_absent_after_a_lost_response_can_find_its_own_object() {
    // The first attempt reaches S3 and writes the object, but its response does not arrive.
    // The transport gives an error, which keeps the loop, and the second attempt finds the
    // object of the first. The call gives `AlreadyExists`, although it wrote the object, as the
    // documentation of the method says.
    let (storage, requests) = scripted_storage("", |_, earlier| match earlier {
        0 => Answer::transport_error(ConnectorError::io("connection reset".into())),
        _ => Answer::new(412, PRECONDITION_FAILED),
    });

    let result = storage
        .put_raw_if_absent(
            "test",
            "put-if-absent",
            namespace(),
            Path::new("blob"),
            b"x",
        )
        .await
        .unwrap();

    assert_eq!(
        (result, sent(&requests).len()),
        (PutIfAbsent::AlreadyExists, 2)
    );
}

#[test]
async fn put_raw_if_absent_stops_at_a_fault_of_the_request() {
    // The conditional write keeps the rule of `put_raw` for every answer other than a 409. A 4xx
    // that reports a fault of the request gets the same answer again, so the loop stops.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(400, ENTITY_TOO_LARGE));

    let result = storage
        .put_raw_if_absent(
            "test",
            "put-if-absent",
            namespace(),
            Path::new("blob"),
            b"x",
        )
        .await;

    assert_eq!((result.is_err(), sent(&requests).len()), (true, 1));
}

#[test]
async fn put_raw_if_absent_rejects_a_name_that_breaks_a_rule_without_a_request() {
    // The key of `namespace()` in a storage without an object prefix is the 36 bytes of the
    // nil UUID, `/`, and the name. The last name has 988 bytes, so its key has 1025. A root
    // path is a directory, and a blob cannot be where a directory is.
    let (storage, requests) = scripted_storage("", |_, _| Answer::new(200, ""));
    let names = [
        "a\0b".to_string(),
        "a/ .. /b".to_string(),
        "dir/__dir_marker".to_string(),
        "a".repeat(988),
        "./".to_string(),
    ];

    let errors = futures::stream::iter(&names)
        .then(|name| async {
            storage
                .put_raw_if_absent("test", "put-if-absent", namespace(), Path::new(name), b"x")
                .await
                .map_err(name_error)
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        (errors, sent(&requests).len()),
        (
            vec![
                Err(Some(BlobNameError::NulByte)),
                Err(Some(BlobNameError::DotSegment {
                    segment: " .. ".to_string()
                })),
                Err(Some(BlobNameError::Reserved {
                    marker: "__dir_marker"
                })),
                Err(Some(BlobNameError::TooLong {
                    length: 1025,
                    max: 1024
                })),
                Err(Some(BlobNameError::NoName {
                    path: PathBuf::from("")
                })),
            ],
            0
        )
    );
}

#[test]
async fn a_filesystem_snapshot_blob_goes_to_its_own_bucket_and_to_the_key_of_its_agent() {
    // The agent name holds a `..` segment, which the rules of a key refuse. So the key holds the
    // bounded segment of the agent and not the agent name. The path style of the client puts the
    // bucket first in the URI.
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::nil()),
        agent_id: r#"counter("a/../b")"#.to_string(),
    };
    let namespace = BlobStorageNamespace::FilesystemSnapshots {
        environment_id: EnvironmentId(Uuid::nil()),
        agent_id: agent_id.clone(),
    };
    let segment = agent_path_segment(&agent_id);
    let (plain, plain_requests) = scripted_storage("", |_, _| Answer::new(200, ""));
    let (prefixed, prefixed_requests) = scripted_storage("prefix", |_, _| Answer::new(200, ""));

    plain
        .put_raw(
            "test",
            "put-raw",
            namespace.clone(),
            Path::new("config"),
            b"x",
        )
        .await
        .unwrap();
    prefixed
        .put_raw("test", "put-raw", namespace, Path::new("config"), b"x")
        .await
        .unwrap();

    assert_eq!(
        (
            request_paths(&plain_requests),
            request_paths(&prefixed_requests)
        ),
        (
            vec![format!(
                "/filesystem-snapshots/{}/{segment}/config",
                Uuid::nil()
            )],
            vec![format!(
                "/filesystem-snapshots/prefix/{}/{segment}/config",
                Uuid::nil()
            )]
        )
    );
}

#[test]
async fn an_oplog_payload_goes_to_the_key_of_its_agent_path_segment() {
    // The agent name holds a `..` segment, which the rules of a key refuse. So the key holds the
    // bounded segment of the agent, and not the component id and the agent name. The path style
    // of the client puts the bucket first in the URI.
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::nil()),
        agent_id: r#"counter("a/../b")"#.to_string(),
    };
    let namespace = BlobStorageNamespace::OplogPayload {
        environment_id: EnvironmentId(Uuid::nil()),
        agent_id: agent_id.clone(),
        agent_mode: AgentMode::Durable,
    };
    let segment = agent_path_segment(&agent_id);
    let (plain, plain_requests) = scripted_storage("", |_, _| Answer::new(200, ""));
    let (prefixed, prefixed_requests) = scripted_storage("prefix", |_, _| Answer::new(200, ""));

    let written = (
        plain
            .put_raw(
                "test",
                "put-raw",
                namespace.clone(),
                Path::new("payload"),
                b"x",
            )
            .await
            .map_err(|error| error.to_string()),
        prefixed
            .put_raw("test", "put-raw", namespace, Path::new("payload"), b"x")
            .await
            .map_err(|error| error.to_string()),
    );

    assert_eq!(
        (
            written,
            request_paths(&plain_requests),
            request_paths(&prefixed_requests)
        ),
        (
            (Ok(()), Ok(())),
            vec![format!(
                "/oplog-payload/durable/{}/{segment}/payload",
                Uuid::nil()
            )],
            vec![format!(
                "/oplog-payload/prefix/durable/{}/{segment}/payload",
                Uuid::nil()
            )]
        )
    );
}
