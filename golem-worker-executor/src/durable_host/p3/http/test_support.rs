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
use crate::services::oplog::{CommitLevel, Oplog, OrderedOplogStart};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::oplog::payload::types::{
    SerializableDnsErrorPayload, SerializableFieldSizePayload, SerializableHttpErrorCode,
    SerializableP3HttpRequestBodyFrame, SerializableTlsAlertReceivedPayload,
};
use golem_common::model::oplog::{
    HostRequest, HostStreamKind, OplogEntry, OplogIndex, OplogPayload, PayloadId, PersistenceLevel,
    RawOplogPayload,
};
use http::{HeaderMap, HeaderValue};
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use wasmtime_wasi::ResourceTable;
use wasmtime_wasi_http::p3::{WasiHttpCtxView, WasiHttpHooks, WasiHttpView};
use wasmtime_wasi_http::{FieldMap, WasiHttpCtx};

/// Minimal in-memory `Oplog` for driving durable request-body machinery:
/// stores appended entries, serves inline payloads, and gates
/// `upload_raw_payload` on a semaphore so tests can hold frame recordings in
/// flight.
#[derive(Debug)]
pub(super) struct FrameTestOplog {
    entries: std::sync::Mutex<Vec<OplogEntry>>,
    upload_gate: tokio::sync::Semaphore,
}

impl FrameTestOplog {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: std::sync::Mutex::new(Vec::new()),
            upload_gate: tokio::sync::Semaphore::new(tokio::sync::Semaphore::MAX_PERMITS),
        })
    }

    /// An oplog whose frame recordings stay in flight until
    /// [`Self::release_uploads`] grants permits.
    pub(super) fn gated() -> Arc<Self> {
        Arc::new(Self {
            entries: std::sync::Mutex::new(Vec::new()),
            upload_gate: tokio::sync::Semaphore::new(0),
        })
    }

    pub(super) fn release_uploads(&self, n: usize) {
        self.upload_gate.add_permits(n);
    }

    pub(super) fn entry_count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// All appended entries, in oplog order.
    pub(super) fn entries(&self) -> Vec<OplogEntry> {
        self.entries.lock().unwrap().clone()
    }

    /// The request-body frames recorded for `parent`, in oplog order. Frames
    /// recorded for other parents are ignored.
    pub(super) fn recorded_frames_for(
        &self,
        parent: OplogIndex,
    ) -> Vec<SerializableP3HttpRequestBodyFrame> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter_map(|entry| {
                let OplogEntry::HostStreamFrame {
                    parent_start_index,
                    kind,
                    payload,
                    ..
                } = entry
                else {
                    panic!("unexpected oplog entry: {entry:?}");
                };
                assert_eq!(*kind, HostStreamKind::P3HttpRequestBody);
                if *parent_start_index != parent {
                    return None;
                }
                let OplogPayload::SerializedInline { bytes, .. } = payload else {
                    panic!("expected an inline frame payload: {payload:?}");
                };
                let HostRequest::P3HttpClientRequestBodyFrame(frame) =
                    golem_common::serialization::deserialize::<HostRequest>(bytes).unwrap()
                else {
                    panic!("expected a request-body frame payload");
                };
                Some(frame.frame)
            })
            .collect()
    }
}

#[async_trait]
impl Oplog for FrameTestOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        OplogIndex::from_u64(entries.len() as u64)
    }

    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        let mut entries = self.entries.lock().unwrap();
        entries.push(start);
        let first_idx = OplogIndex::from_u64(entries.len() as u64);
        entries.push(make_second(first_idx));
        let second_idx = OplogIndex::from_u64(entries.len() as u64);
        (first_idx, second_idx)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        _serialized_request: Vec<u8>,
        _build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
    ) -> Result<OrderedOplogStart, String> {
        unimplemented!()
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        0
    }

    async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        BTreeMap::new()
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        OplogIndex::from_u64(self.entries.lock().unwrap().len() as u64)
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        true
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let entries = self.entries.lock().unwrap();
        let idx: u64 = oplog_index.into();
        entries[(idx - 1) as usize].clone()
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        let entries = self.entries.lock().unwrap();
        let start: u64 = oplog_index.into();
        let mut result = BTreeMap::new();
        for i in start..(start + n) {
            if let Some(entry) = entries.get((i - 1) as usize) {
                result.insert(OplogIndex::from_u64(i), entry.clone());
            }
        }
        result
    }

    async fn length(&self) -> u64 {
        self.entries.lock().unwrap().len() as u64
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        let permit = self
            .upload_gate
            .acquire()
            .await
            .map_err(|err| err.to_string())?;
        permit.forget();
        Ok(RawOplogPayload::SerializedInline(data))
    }

    async fn download_raw_payload(
        &self,
        _payload_id: PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        unimplemented!()
    }

    async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
}

/// A guest body producing the given frames, then ending (or failing when
/// `error` is set).
pub(super) fn frame_body(
    frames: Vec<Frame<Bytes>>,
    error: Option<ErrorCode>,
) -> UnsyncBoxBody<Bytes, ErrorCode> {
    let items = frames
        .into_iter()
        .map(Ok)
        .chain(error.into_iter().map(Err))
        .collect::<Vec<_>>();
    http_body_util::StreamBody::new(futures::stream::iter(items)).boxed_unsync()
}

#[derive(Default)]
pub(super) struct TestHttpCtx {
    pub(super) table: ResourceTable,
    ctx: WasiHttpCtx,
    hooks: TestHttpHooks,
}

#[derive(Default)]
struct TestHttpHooks;

impl WasiHttpHooks for TestHttpHooks {}

impl WasiHttpView for TestHttpCtx {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            hooks: &mut self.hooks,
            table: &mut self.table,
            ctx: &mut self.ctx,
        }
    }
}

pub(super) fn short_content_length_request() -> (
    wasmtime_wasi_http::p3::Request,
    impl Future<Output = Result<(), ErrorCode>> + Send + 'static,
) {
    let mut headers = HeaderMap::new();
    headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("4"));

    wasmtime_wasi_http::p3::Request::new(
        http::Method::POST,
        Some(http::uri::Scheme::HTTP),
        Some(http::uri::Authority::from_static("example.com")),
        Some(http::uri::PathAndQuery::from_static("/upload")),
        FieldMap::new_immutable(headers),
        None,
        Full::new(Bytes::from_static(b"x"))
            .map_err(|never| match never {})
            .boxed_unsync(),
    )
}

/// A request whose outgoing body deterministically fails with an
/// `ErrorCode`, carrying no `content-length` header. Without a content-length
/// validation wrapper, this error can only reach the guest through the
/// transmission future, not through `into_http`'s content-length channel.
pub(super) fn erroring_body_request_without_content_length() -> (
    wasmtime_wasi_http::p3::Request,
    impl Future<Output = Result<(), ErrorCode>> + Send + 'static,
) {
    let body = http_body_util::StreamBody::new(futures::stream::once(async {
        Err::<http_body::Frame<Bytes>, ErrorCode>(ErrorCode::HttpProtocolError)
    }))
    .boxed_unsync();

    wasmtime_wasi_http::p3::Request::new(
        http::Method::POST,
        Some(http::uri::Scheme::HTTP),
        Some(http::uri::Authority::from_static("example.com")),
        Some(http::uri::PathAndQuery::from_static("/upload")),
        FieldMap::new_immutable(HeaderMap::new()),
        None,
        body,
    )
}

/// Every `SerializableHttpErrorCode` variant, each carrying a distinct
/// payload so a mismatched arm between `serialize_error_code` and
/// `deserialize_error_code` (or a dropped payload field) is detected.
pub(super) fn all_serializable_error_codes() -> Vec<SerializableHttpErrorCode> {
    use SerializableHttpErrorCode::*;
    vec![
        DnsTimeout,
        DnsError(SerializableDnsErrorPayload {
            rcode: Some("NXDOMAIN".to_string()),
            info_code: Some(3),
        }),
        DestinationNotFound,
        DestinationUnavailable,
        DestinationIpProhibited,
        DestinationIpUnroutable,
        ConnectionRefused,
        ConnectionTerminated,
        ConnectionTimeout,
        ConnectionReadTimeout,
        ConnectionWriteTimeout,
        ConnectionLimitReached,
        TlsProtocolError,
        TlsCertificateError,
        TlsAlertReceived(SerializableTlsAlertReceivedPayload {
            alert_id: Some(42),
            alert_message: Some("handshake failure".to_string()),
        }),
        HttpRequestDenied,
        HttpRequestLengthRequired,
        HttpRequestBodySize(Some(1024)),
        HttpRequestMethodInvalid,
        HttpRequestUriInvalid,
        HttpRequestUriTooLong,
        HttpRequestHeaderSectionSize(Some(8192)),
        HttpRequestHeaderSize(Some(SerializableFieldSizePayload {
            field_name: Some("authorization".to_string()),
            field_size: Some(64),
        })),
        HttpRequestTrailerSectionSize(Some(256)),
        HttpRequestTrailerSize(SerializableFieldSizePayload {
            field_name: Some("x-checksum".to_string()),
            field_size: Some(32),
        }),
        HttpResponseIncomplete,
        HttpResponseHeaderSectionSize(Some(4096)),
        HttpResponseHeaderSize(SerializableFieldSizePayload {
            field_name: Some("content-type".to_string()),
            field_size: Some(16),
        }),
        HttpResponseBodySize(Some(2048)),
        HttpResponseTrailerSectionSize(Some(128)),
        HttpResponseTrailerSize(SerializableFieldSizePayload {
            field_name: Some("x-trailer".to_string()),
            field_size: Some(8),
        }),
        HttpResponseTransferCoding(Some("chunked".to_string())),
        HttpResponseContentCoding(Some("gzip".to_string())),
        HttpResponseTimeout,
        HttpUpgradeFailed,
        HttpProtocolError,
        LoopDetected,
        ConfigurationError,
        InternalError(Some("boom".to_string())),
    ]
}
