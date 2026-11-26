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

use super::{S3VolumeConfig, connect, place};
use crate::aws::AwsCredentials;
use crate::volume::connection::ConnectionPolicy;
use crate::volume::placement::VersionRead;
use crate::volume::{Cursor, Entry, FileState, Node, ReadStream, VolumeError, VolumePath};
use axum::{Router, body::Body};
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use http::{Request, Response};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::io::Write;
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};
use tokio::sync::mpsc;
use tracing::instrument::WithSubscriber;

fn credentials() -> AwsCredentials {
    AwsCredentials::from_record(&serde_json::json!({
        "accessKeyId": "volume-access", "secretAccessKey": "volume-secret", "sessionToken": "volume-token"
    })).unwrap()
}

fn config(endpoint: &str) -> S3VolumeConfig {
    S3VolumeConfig {
        endpoint: Some(endpoint.into()),
        region: "us-east-1".into(),
        bucket: "test-bucket".into(),
        prefix: "corpus/".into(),
    }
}

fn allowed() -> ConnectionPolicy {
    ConnectionPolicy {
        allow_loopback: true,
        ..ConnectionPolicy::new(Box::new([]))
    }
}

fn path(path: &str) -> VolumePath {
    VolumePath::new(path).unwrap()
}

fn response(status: u16, body: impl Into<Body>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml")
        .body(body.into())
        .unwrap()
}

fn metadata(size: u64, etag: &str, version: &str) -> Response<Body> {
    Response::builder()
        .status(200)
        .header("content-length", size)
        .header("etag", etag)
        .header("x-amz-version-id", version)
        .body(Body::empty())
        .unwrap()
}

async fn bytes(stream: ReadStream) -> Bytes {
    stream
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .concat()
        .into()
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
            let body = axum::body::to_bytes(body, 1024).await.unwrap();
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
async fn connect_sends_nothing_and_open_file_short_circuits_empty_and_past_eof_reads() {
    let (endpoint, mut requests, server) =
        server(vec![metadata(7, "\"first\"", "version-7")]).await;
    let (client, _recovery) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    assert!(requests.try_recv().is_err());
    let file = client.open(&path("book.txt")).await.unwrap();
    assert_eq!(file.size(), 7);
    assert!(file.replayable());
    assert!(bytes(file.read(7..19).await.unwrap()).await.is_empty());
    assert!(bytes(file.read(25..30).await.unwrap()).await.is_empty());
    assert!(bytes(file.read(3..3).await.unwrap()).await.is_empty());
    let request = requests.try_recv().unwrap();
    assert_eq!(request.method(), "HEAD");
    assert_eq!(request.uri().path(), "/test-bucket/corpus/book.txt");
    assert!(requests.try_recv().is_err());
    assert!(
        request.headers()["authorization"]
            .to_str()
            .unwrap()
            .starts_with("AWS4-HMAC-SHA256 ")
    );
    server.abort();
}

fn listing(objects: &[(&str, u64)], continuation: Option<&str>) -> Response<Body> {
    let contents = objects
        .iter()
        .map(|(key, size)| format!("<Contents><Key>{key}</Key><Size>{size}</Size></Contents>"))
        .collect::<String>();
    let next = continuation
        .map(|token| format!("<NextContinuationToken>{token}</NextContinuationToken>"))
        .unwrap_or_default();
    response(
        200,
        format!(
            "<ListBucketResult><IsTruncated>{}</IsTruncated>{next}{contents}</ListBucketResult>",
            continuation.is_some()
        ),
    )
}

#[test]
#[timeout("30s")]
async fn listing_projects_visible_direct_children_from_exactly_one_recursive_page() {
    let (endpoint, mut requests, server) = server(vec![
        listing(
            &[
                ("corpus/empty/", 0),
                ("corpus/hidden/../file", 3),
                ("corpus/hidden//file", 3),
                ("corpus/literal%2Fname", 19),
                ("corpus/nested/a", 5),
                ("corpus/nested/b", 6),
                ("corpus/nested/marker/", 0),
            ],
            Some("next-page"),
        ),
        listing(&[("corpus/nested/c", 7), ("corpus/z.txt", 23)], None),
    ])
    .await;
    let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let first = client.list(&path(""), None).await.unwrap();
    assert_eq!(
        first.entries.as_ref(),
        [
            Entry {
                path: path("literal%2Fname"),
                node: Node::File { size: 19 }
            },
            Entry {
                path: path("nested"),
                node: Node::Directory
            },
        ]
    );
    let first_request = requests.try_recv().unwrap();
    assert_eq!(first_request.method(), "GET");
    let query = first_request.uri().query().unwrap();
    assert!(query.contains("list-type=2"));
    assert!(query.contains("prefix=corpus%2F"));
    assert!(!query.contains("delimiter"));
    assert!(!query.contains("encoding-type"));
    assert!(requests.try_recv().is_err());
    let second = client.list(&path(""), first.after.as_ref()).await.unwrap();
    assert_eq!(
        second.entries.as_ref(),
        [Entry {
            path: path("z.txt"),
            node: Node::File { size: 23 }
        }]
    );
    assert!(second.after.is_none());
    let second_request = requests.try_recv().unwrap();
    assert!(
        second_request
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=next-page")
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn listing_keeps_file_precedence_across_intervening_siblings_and_descendant_pages() {
    futures::stream::iter(["", "nested"])
        .flat_map(|directory| {
            futures::stream::iter([vec![14], vec![2, 2, 3, 2, 3, 2]])
                .map(move |counts| (directory, counts))
        })
        .for_each(|(directory, counts)| async move {
            let directory_prefix = if directory.is_empty() {
                String::new()
            } else {
                format!("{directory}/")
            };
            let objects = [
                ("a", 11),
                ("a-", 12),
                ("a-/01", 13),
                ("a-/02", 14),
                ("a-/03", 15),
                ("a-999", 17),
                ("a/", 0),
                ("a/01", 18),
                ("a/02", 19),
                ("a/03", 20),
                ("dir-/marker/", 0),
                ("dir/one", 3),
                ("dir/two", 4),
                ("z", 23),
            ]
            .map(|(name, size)| (format!("corpus/{directory_prefix}{name}"), size));
            let responses = counts
                .iter()
                .scan(0, |start, count| {
                    let end = *start + count;
                    let page = objects[*start..end]
                        .iter()
                        .map(|(key, size)| (key.as_str(), *size))
                        .collect::<Vec<_>>();
                    let next = (end < objects.len()).then(|| format!("next-{end}"));
                    *start = end;
                    Some(listing(&page, next.as_deref()))
                })
                .collect();
            let (endpoint, requests, server) = server(responses).await;
            let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
            let entries = futures::stream::iter(counts.iter())
                .fold(
                    (None, Vec::new(), requests),
                    |(after, mut entries, mut requests), _| {
                        let client = &client;
                        async move {
                            let page = client.list(&path(directory), after.as_ref()).await.unwrap();
                            let request = requests.try_recv().unwrap();
                            assert_eq!(request.method(), "GET");
                            assert!(request.uri().query().unwrap().contains("list-type=2"));
                            assert!(
                                requests.try_recv().is_err(),
                                "one LIST, without per-object HEAD"
                            );
                            entries.extend(page.entries);
                            let restored = page.after.map(|cursor| {
                                let encoded = serde_json::to_string(&cursor).unwrap();
                                let restored = serde_json::from_str::<Cursor>(&encoded).unwrap();
                                assert_eq!(restored, cursor);
                                restored
                            });
                            (restored, entries, requests)
                        }
                    },
                )
                .await;
            assert!(entries.0.is_none());
            assert_eq!(
                entries.1,
                [
                    Entry {
                        path: path(&format!("{directory_prefix}a")),
                        node: Node::File { size: 11 }
                    },
                    Entry {
                        path: path(&format!("{directory_prefix}a-")),
                        node: Node::File { size: 12 }
                    },
                    Entry {
                        path: path(&format!("{directory_prefix}a-999")),
                        node: Node::File { size: 17 }
                    },
                    Entry {
                        path: path(&format!("{directory_prefix}dir")),
                        node: Node::Directory
                    },
                    Entry {
                        path: path(&format!("{directory_prefix}z")),
                        node: Node::File { size: 23 }
                    },
                ]
            );
            server.abort();
        })
        .await;
}

#[test]
#[timeout("30s")]
async fn stat_requires_a_visible_descendant_and_treats_root_as_a_directory() {
    let (endpoint, mut requests, server) = server(vec![
        metadata(41, "\"file\"", "version-7"),
        response(404, ""),
        listing(&[("corpus/empty/", 0), ("corpus/empty/hidden/", 0)], None),
        response(404, ""),
        listing(&[("corpus/real/marker/", 0)], Some("later")),
        listing(&[("corpus/real/sub/file", 11)], None),
    ])
    .await;
    let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    assert_eq!(client.stat(&path("")).await.unwrap(), Some(Node::Directory));
    assert!(requests.try_recv().is_err());
    assert_eq!(
        client.stat(&path("file")).await.unwrap(),
        Some(Node::File { size: 41 })
    );
    assert_eq!(client.stat(&path("empty")).await.unwrap(), None);
    assert_eq!(
        client.stat(&path("real")).await.unwrap(),
        Some(Node::Directory)
    );
    let seen = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        seen.iter()
            .map(|request| request.method().as_str())
            .collect::<Vec<_>>(),
        ["HEAD", "HEAD", "GET", "HEAD", "GET", "GET"]
    );
    assert!(
        seen[5]
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=later")
    );
    server.abort();
}

fn data(body: &'static str, etag: &str, version: &str, range: Option<&str>) -> Response<Body> {
    let mut response = Response::builder()
        .status(if range.is_some() { 206 } else { 200 })
        .header("etag", etag)
        .header("x-amz-version-id", version);
    if let Some(range) = range {
        response = response.header("content-range", range);
    }
    response.body(Body::from(body)).unwrap()
}

#[test]
#[timeout("30s")]
async fn placement_reads_a_non_null_version_and_keeps_lifecycle_denial_as_a_finding() {
    let (endpoint, mut requests, server) = server(vec![
        response(
            200,
            "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
        ),
        listing(&[("corpus/null", 4), ("corpus/book", 7)], None),
        metadata(4, "\"null\"", "null"),
        metadata(7, "\"old\"", "old-7"),
        data("b", "\"old\"", "old-7", Some("bytes 0-0/7")),
        response(403, "<Error><Code>AccessDenied</Code></Error>"),
    ])
    .await;
    let result = place(config(&endpoint), credentials(), allowed())
        .await
        .unwrap();
    assert_eq!(result.version_read, VersionRead::Verified);
    assert_eq!(
        result.lifecycle,
        Err(VolumeError::Denied {
            action: "GetLifecycleConfiguration".into()
        })
    );
    let seen = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(seen.len(), 6);
    assert_eq!(
        seen.iter()
            .map(|request| request.method().as_str())
            .collect::<Vec<_>>(),
        ["GET", "GET", "HEAD", "HEAD", "GET", "GET"]
    );
    assert!(seen[0].uri().query().unwrap().contains("versioning"));
    assert!(seen[1].uri().query().unwrap().contains("list-type=2"));
    assert!(
        seen.iter()
            .all(|request| !request.uri().query().unwrap_or("").contains("versions"))
    );
    assert!(seen[1].uri().query().unwrap().contains("prefix=corpus%2F"));
    assert_eq!(seen[4].uri().path(), "/test-bucket/corpus/book");
    assert!(seen[4].uri().query().unwrap().contains("versionId=old-7"));
    assert_eq!(seen[4].headers()["range"], "bytes=0-0");
    assert_eq!(seen[4].headers()["if-match"], "\"old\"");
    assert!(seen[5].uri().query().unwrap().contains("lifecycle"));
    server.abort();
}

#[test]
#[timeout("30s")]
async fn recorded_open_and_each_read_select_the_recorded_version_and_clamp_ranges() {
    let recorded = FileState::from_reference("\"old\"".into(), Some("old-7".into()));
    let recorded = serde_json::from_str(&serde_json::to_string(&recorded).unwrap()).unwrap();
    let (endpoint, mut requests, server) = server(vec![
        metadata(7, "\"old\"", "old-7"),
        data("cdefg", "\"old\"", "old-7", Some("bytes 2-6/7")),
        data("a", "\"old\"", "old-7", Some("bytes 0-0/7")),
        response(404, ""),
    ])
    .await;
    let (_, recovery) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let file = recovery
        .open_recorded(&path("book"), &recorded)
        .await
        .unwrap();
    assert_eq!(file.size(), 7);
    assert_eq!(file.state(), &recorded);
    assert_eq!(bytes(file.read(2..u64::MAX).await.unwrap()).await, "cdefg");
    assert_eq!(bytes(file.read(0..1).await.unwrap()).await, "a");
    assert_eq!(
        recovery
            .open_recorded(&path("book"), &recorded)
            .await
            .unwrap_err(),
        VolumeError::Changed
    );
    let seen = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(seen.len(), 4);
    seen.iter().for_each(|request| {
        assert!(request.uri().query().unwrap().contains("versionId=old-7"));
        assert_eq!(request.headers()["if-match"], "\"old\"");
    });
    assert_eq!(seen[1].headers()["range"], "bytes=2-6");
    assert_eq!(seen[2].headers()["range"], "bytes=0-0");
    server.abort();
}

#[test]
#[timeout("30s")]
async fn etag_only_reads_and_recovery_accept_new_versions_without_selecting_them() {
    let (endpoint, mut requests, server) = server(vec![
        metadata(4, "\"same\"", "null"),
        data("bc", "\"same\"", "v2", Some("bytes 1-2/4")),
        metadata(4, "\"same\"", "v3"),
        data("d", "\"same\"", "v4", Some("bytes 3-3/4")),
        metadata(4, "\"same\"", "v5"),
        data("d", "\"same\"", "v6", Some("bytes 3-3/4")),
    ])
    .await;
    let (client, recovery) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let file = client.open(&path("book")).await.unwrap();
    let selected = FileState::from_reference("\"same\"".into(), None);
    assert_eq!(file.state(), &selected);
    assert!(!file.replayable());
    assert_eq!(bytes(file.read(1..3).await.unwrap()).await, "bc");
    let restored_null: FileState = serde_json::from_value(serde_json::json!({
        "etag": "\"same\"", "version": "null"
    }))
    .unwrap();
    futures::stream::iter([selected.clone(), restored_null])
        .for_each(|recorded| {
            let recovery = &recovery;
            let selected = &selected;
            async move {
                let recovered = recovery
                    .open_recorded(&path("book"), &recorded)
                    .await
                    .unwrap();
                assert_eq!(recovered.state(), selected);
                assert!(!recovered.replayable());
                assert_eq!(bytes(recovered.read(3..4).await.unwrap()).await, "d");
            }
        })
        .await;
    let seen = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(seen.len(), 6);
    seen.iter().for_each(|request| {
        assert!(!request.uri().query().unwrap_or("").contains("versionId"));
    });
    seen.iter().skip(1).for_each(|request| {
        assert_eq!(request.headers()["if-match"], "\"same\"");
    });
    server.abort();
}

#[test]
#[timeout("30s")]
async fn null_versions_are_not_replayable_and_changed_reads_never_fall_back_to_head() {
    let (endpoint, mut requests, server) = server(vec![
        metadata(9, "\"old\"", "null"),
        response(412, "<Error><Code>PreconditionFailed</Code></Error>"),
        response(416, "<Error><Code>InvalidRange</Code></Error>"),
    ])
    .await;
    let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let file = client.open(&path("book")).await.unwrap();
    assert!(!file.replayable());
    assert_eq!(
        file.state(),
        &FileState::from_reference("\"old\"".into(), None)
    );
    assert_eq!(file.read(0..1).await.err(), Some(VolumeError::Changed));
    assert_eq!(
        file.read(1..2).await.err(),
        Some(VolumeError::InvalidResponse)
    );
    let seen = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        seen.iter()
            .map(|request| request.method().as_str())
            .collect::<Vec<_>>(),
        ["HEAD", "GET", "GET"]
    );
    assert!(
        seen.iter()
            .all(|request| !request.uri().query().unwrap_or("").contains("versionId"))
    );
    server.abort();
}

#[test]
#[timeout("30s")]
async fn read_rejects_a_different_etag_version_range_or_total_size() {
    futures::stream::iter([
        data("abc", "\"new\"", "old-7", Some("bytes 0-2/7")),
        data("abc", "\"old\"", "new-8", Some("bytes 0-2/7")),
        data("abc", "\"old\"", "null", Some("bytes 0-2/7")),
        data("abc", "\"old\"", "old-7", Some("bytes 1-3/7")),
        data("abc", "\"old\"", "old-7", Some("bytes 0-2/8")),
        data("abc", "\"old\"", "old-7", Some("bytes 0-3/7")),
        data("abc", "\"old\"", "old-7", None),
    ])
    .then(|bad| async move {
        let (endpoint, mut requests, server) =
            server(vec![metadata(7, "\"old\"", "old-7"), bad]).await;
        let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
        let file = client.open(&path("book")).await.unwrap();
        assert_eq!(
            file.read(0..3).await.err(),
            Some(VolumeError::InvalidResponse)
        );
        assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 2);
        server.abort();
    })
    .collect::<()>()
    .await;
}

#[test]
#[timeout("30s")]
async fn listing_does_not_scan_a_marker_only_page_and_binds_cursors_to_the_directory() {
    let (endpoint, mut requests, server) = server(vec![
        listing(&[("corpus/nested/foreign/", 0)], Some("next")),
        listing(&[("corpus/nested/real/file", 8)], None),
    ])
    .await;
    let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let page = client.list(&path("nested"), None).await.unwrap();
    assert!(page.entries.is_empty());
    assert!(page.after.is_some());
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 1);
    assert_eq!(
        client.list(&path("other"), page.after.as_ref()).await,
        Err(VolumeError::InvalidRequest)
    );
    assert_eq!(
        client
            .list(&path("nested"), Some(&Cursor::new("invalid")))
            .await,
        Err(VolumeError::InvalidRequest)
    );
    assert!(requests.try_recv().is_err());
    let second = client
        .list(&path("nested"), page.after.as_ref())
        .await
        .unwrap();
    assert_eq!(
        second.entries.as_ref(),
        [Entry {
            path: path("nested/real"),
            node: Node::Directory
        }]
    );
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 1);
    server.abort();
}

fn assert_opaque_debug(value: &impl Debug) {
    [format!("{value:?}"), format!("{value:#?}")]
        .iter()
        .for_each(|debug| {
            assert!(
                !debug.contains("volume-secret"),
                "Debug disclosed the secret key"
            );
            assert!(
                !debug.contains("volume-token"),
                "Debug disclosed the session token"
            );
        });
}

#[test]
#[timeout("30s")]
async fn reflected_metadata_is_opaque_in_debug_but_unchanged_in_reads_and_cursor_round_trips() {
    let (endpoint, mut requests, server) = server(vec![
        listing(
            &[("corpus/volume-secret-volume-token", 4)],
            Some("volume-token-volume-secret"),
        ),
        metadata(4, "\"volume-secret\"", "volume-token"),
        data(
            "read",
            "\"volume-secret\"",
            "volume-token",
            Some("bytes 0-3/4"),
        ),
        listing(&[], None),
    ])
    .await;
    let (client, recovery) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let first = client.list(&path(""), None).await.unwrap();
    assert_opaque_debug(&first);
    assert_eq!(first.entries[0].path.as_str(), "volume-secret-volume-token");
    let file = client.open(&first.entries[0].path).await.unwrap();
    assert_opaque_debug(&file);
    assert_opaque_debug(&file.state());
    assert_opaque_debug(&client);
    assert_opaque_debug(&recovery);
    let serialized = serde_json::to_string(file.state()).unwrap();
    assert!(serialized.contains("volume-secret") && serialized.contains("volume-token"));
    let restored: FileState = serde_json::from_str(&serialized).unwrap();
    assert_eq!(&restored, file.state());
    assert_eq!(bytes(file.read(0..4).await.unwrap()).await, "read");
    let serialized = serde_json::to_string(first.after.as_ref().unwrap()).unwrap();
    let restored: Cursor = serde_json::from_str(&serialized).unwrap();
    assert_eq!(first.after.as_ref(), Some(&restored));
    assert!(
        client
            .list(&path(""), Some(&restored))
            .await
            .unwrap()
            .after
            .is_none()
    );
    let seen = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(seen.len(), 4);
    assert!(
        seen[2]
            .uri()
            .query()
            .unwrap()
            .contains("versionId=volume-token")
    );
    assert_eq!(seen[2].headers()["if-match"], "\"volume-secret\"");
    assert!(
        seen[3]
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=volume-token-volume-secret")
    );
    server.abort();
}

#[test]
#[timeout("30s")]
async fn placement_debug_hides_reflected_lifecycle_text_without_changing_its_values() {
    use crate::volume::placement::LifecycleRuleStatus;
    let (endpoint, mut requests, server) = server(vec![
        response(200, "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>"),
        listing(&[], None),
        response(200, "<LifecycleConfiguration><Rule><Status>volume-token</Status><Filter><Prefix>corpus/volume-secret/</Prefix></Filter></Rule></LifecycleConfiguration>"),
    ]).await;
    let placed = place(config(&endpoint), credentials(), allowed())
        .await
        .unwrap();
    assert_opaque_debug(&placed);
    let rules = placed.lifecycle.unwrap();
    assert_eq!(
        rules[0].status,
        LifecycleRuleStatus::Unknown("volume-token".into())
    );
    assert_eq!(rules[0].prefix.as_deref(), Some("corpus/volume-secret/"));
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 3);
    server.abort();
}

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

#[test]
#[timeout("30s")]
async fn sdk_diagnostics_and_public_errors_do_not_disclose_reflected_credentials() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let capture = Capture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .without_time()
        .with_ansi(false)
        .with_writer(move || capture.clone())
        .finish();
    let (endpoint, mut requests, server) = server(vec![
        metadata(7, "\"old\"", "old-7"),
        response(403, "<Error><Code>AccessDenied</Code><Message>volume-secret volume-token</Message><RequestId>volume-token</RequestId></Error>"),
        response(500, "<Error><Code>NoSuchKey</Code><Message>volume-secret volume-token</Message></Error>"),
        response(503, "<Error><Code>AccessDenied</Code><Message>volume-secret volume-token</Message></Error>"),
        response(599, "<Error><Code>PreconditionFailed</Code><Message>volume-secret volume-token</Message></Error>"),
        response(503, "<Error><Code>ServiceUnavailable</Code><Message>volume-secret volume-token</Message></Error>"),
    ]).await;
    async {
        let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
        let file = client.open(&path("book")).await.unwrap();
        let error = file.read(0..1).await.err().unwrap();
        assert_eq!(
            error,
            VolumeError::Denied {
                action: "GetObjectVersion".into()
            }
        );
        assert_opaque_debug(&error);
        assert!(!error.to_string().contains("volume-secret"));
        assert!(!error.to_string().contains("volume-token"));
        futures::stream::iter(0..3)
            .then(|_| async {
                let error = file.read(0..1).await.err().unwrap();
                assert_eq!(error, VolumeError::Unanswered);
                assert_opaque_debug(&error);
            })
            .collect::<()>()
            .await;
        let error = client.open(&path("book")).await.unwrap_err();
        assert_eq!(error, VolumeError::Unanswered);
        assert_eq!(
            error.to_string(),
            "the volume request failed without a usable answer"
        );
        assert_eq!(serde_json::to_value(&error).unwrap(), "Unanswered");
    }
    .with_subscriber(subscriber)
    .await;
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains("volume-secret"));
    assert!(!logs.contains("volume-token"));
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 6);
    server.abort();
}

#[test]
#[timeout("30s")]
async fn placement_skips_null_versions_across_pages_but_reads_a_versioned_empty_object() {
    let (endpoint, mut requests, server) = server(vec![
        response(
            200,
            "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
        ),
        listing(&[("corpus/first", 4)], Some("next-page")),
        metadata(4, "\"first\"", "null"),
        listing(&[("corpus/second", 5)], None),
        metadata(5, "\"second\"", "null"),
        response(
            404,
            "<Error><Code>NoSuchLifecycleConfiguration</Code></Error>",
        ),
        response(
            200,
            "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
        ),
        listing(&[("corpus/empty", 0)], None),
        metadata(0, "\"empty\"", "empty-3"),
        data("", "\"empty\"", "empty-3", None),
        response(200, "<LifecycleConfiguration/>"),
    ])
    .await;
    let nulls = place(config(&endpoint), credentials(), allowed())
        .await
        .unwrap();
    assert_eq!(nulls.version_read, VersionRead::NoObject);
    assert!(nulls.lifecycle.unwrap().is_empty());
    let null_requests = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(null_requests.len(), 6);
    assert!(
        null_requests[3]
            .uri()
            .query()
            .unwrap()
            .contains("continuation-token=next-page")
    );
    assert!(
        null_requests.iter().all(|request| !request
            .uri()
            .query()
            .unwrap_or("")
            .contains("versionId"))
    );
    assert_eq!(
        null_requests
            .iter()
            .map(|request| request.method().as_str())
            .collect::<Vec<_>>(),
        ["GET", "GET", "HEAD", "GET", "HEAD", "GET"]
    );
    let empty = place(config(&endpoint), credentials(), allowed())
        .await
        .unwrap();
    assert_eq!(empty.version_read, VersionRead::Verified);
    let empty_requests = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(empty_requests.len(), 5);
    assert!(
        empty_requests[3]
            .uri()
            .query()
            .unwrap()
            .contains("versionId=empty-3")
    );
    assert!(!empty_requests[3].headers().contains_key("range"));
    server.abort();
}

#[test]
#[timeout("30s")]
async fn placement_names_the_required_action_even_when_lifecycle_is_also_denied() {
    let (endpoint, mut requests, server) = server(vec![
        response(
            403,
            "<Error><Code>AccessDenied</Code><Message>volume-secret volume-token</Message></Error>",
        ),
        response(403, "<Error><Code>AccessDenied</Code></Error>"),
        response(
            200,
            "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
        ),
        listing(&[("corpus/book", 7)], None),
        metadata(7, "\"old\"", "old-7"),
        response(403, "<Error><Code>AccessDenied</Code></Error>"),
        response(403, "<Error><Code>AccessDenied</Code></Error>"),
    ])
    .await;
    assert_eq!(
        place(config(&endpoint), credentials(), allowed()).await,
        Err(VolumeError::Denied {
            action: "GetBucketVersioning".into()
        })
    );
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 2);
    assert_eq!(
        place(config(&endpoint), credentials(), allowed()).await,
        Err(VolumeError::Denied {
            action: "GetObjectVersion".into()
        })
    );
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 5);
    server.abort();
}

#[test]
#[timeout("30s")]
async fn placement_preserves_lifecycle_expiration_observations() {
    use crate::volume::placement::{LifecycleRule, LifecycleRuleStatus, NoncurrentExpiration};
    let (endpoint, mut requests, server) = server(vec![
        response(200, "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>"),
        listing(&[], None),
        response(200, "<LifecycleConfiguration><Rule><Status>Enabled</Status><Filter><And><Prefix>corpus/</Prefix><Tag><Key>scope</Key><Value>archive</Value></Tag></And></Filter><NoncurrentVersionExpiration><NoncurrentDays>17</NoncurrentDays><NewerNoncurrentVersions>5</NewerNoncurrentVersions></NoncurrentVersionExpiration></Rule></LifecycleConfiguration>"),
        response(200, "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>"),
        listing(&[], None),
        response(503, "<Error><Code>NoSuchLifecycleConfiguration</Code></Error>"),
    ]).await;
    let result = place(config(&endpoint), credentials(), allowed())
        .await
        .unwrap();
    assert_eq!(
        result.lifecycle.unwrap().as_ref(),
        [LifecycleRule {
            status: LifecycleRuleStatus::Enabled,
            prefix: Some("corpus/".into()),
            noncurrent_expiration: Some(NoncurrentExpiration {
                days: 17,
                newer_versions: Some(5)
            }),
        }]
    );
    let unavailable = place(config(&endpoint), credentials(), allowed())
        .await
        .unwrap();
    assert_eq!(unavailable.version_read, VersionRead::NoObject);
    assert_eq!(unavailable.lifecycle, Err(VolumeError::Unanswered));
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 6);
    server.abort();
}

#[test]
#[timeout("30s")]
async fn invalid_listing_protocol_never_becomes_a_phantom_directory_or_a_cursor() {
    futures::stream::iter([
        response(200, "<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>"),
        response(200, "<ListBucketResult><IsTruncated>false</IsTruncated><CommonPrefixes><Prefix>corpus/phantom/</Prefix></CommonPrefixes></ListBucketResult>"),
        listing(&[("outside/volume", 4)], None),
        response(200, "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>false</IsTruncated></ListBucketResult>"),
    ]).then(|bad| async move {
        let (endpoint, mut requests, server) = server(vec![bad]).await;
        let (client, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
        assert_eq!(client.list(&path(""), None).await, Err(VolumeError::InvalidResponse));
        assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 1);
        server.abort();
    }).collect::<()>().await;
}

#[test]
#[timeout("30s")]
async fn key_byte_limits_include_the_prefix_and_invalid_ranges_send_nothing() {
    let (endpoint, mut requests, server) =
        server(vec![metadata(13, "\"file\"", "version-7")]).await;
    let (client, recovery) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    let too_long = path(&"é".repeat(509));
    assert_eq!(
        client.open(&too_long).await.unwrap_err(),
        VolumeError::UnrepresentablePath
    );
    assert_eq!(
        client.list(&too_long, None).await,
        Err(VolumeError::UnrepresentablePath)
    );
    assert_eq!(
        client.open(&path("")).await.unwrap_err(),
        VolumeError::NotFound
    );
    let invalid = FileState::from_reference("*".into(), None);
    assert_eq!(
        recovery
            .open_recorded(&path("book"), &invalid)
            .await
            .unwrap_err(),
        VolumeError::InvalidRequest
    );
    assert!(requests.try_recv().is_err());
    let longest = path(&"x".repeat(1017));
    let file = client.open(&longest).await.unwrap();
    let reversed = std::ops::Range { start: 9, end: 8 };
    assert_eq!(
        file.read(reversed).await.err(),
        Some(VolumeError::InvalidRequest)
    );
    assert_eq!(
        requests.try_recv().unwrap().uri().path().len(),
        "/test-bucket/".len() + 1024
    );
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[test]
#[timeout("30s")]
async fn refused_connections_say_nothing_was_sent_and_redirects_are_not_followed() {
    let (endpoint, mut requests, server) = server(vec![
        Response::builder()
            .status(307)
            .header("location", "http://169.254.169.254/latest/meta-data/")
            .body(Body::empty())
            .unwrap(),
    ])
    .await;
    let (refused, _) = connect(
        config(&endpoint),
        credentials(),
        ConnectionPolicy::new(Box::new([])),
    )
    .unwrap();
    let error = refused.open(&path("book")).await.unwrap_err();
    assert_eq!(error, VolumeError::ConnectionRefused);
    assert!(error.to_string().contains("nothing was sent"));
    let policy = ConnectionPolicy {
        protected_endpoints: [endpoint.parse().unwrap()].into(),
        ..allowed()
    };
    let (protected, _) = connect(config(&endpoint), credentials(), policy).unwrap();
    assert_eq!(
        protected.open(&path("book")).await.unwrap_err(),
        VolumeError::ConnectionRefused
    );
    assert!(requests.try_recv().is_err());
    let (redirected, _) = connect(config(&endpoint), credentials(), allowed()).unwrap();
    assert_eq!(
        redirected.open(&path("book")).await.unwrap_err(),
        VolumeError::InvalidResponse
    );
    assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 1);
    server.abort();
}
