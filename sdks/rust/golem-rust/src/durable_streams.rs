// Copyright 2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! External Durable Streams clients. The host owns HTTP, framing and authentication;
//! these clients retain only replay-reconstructed checkpoints, buffers and producer state.
//!
//! Appends use the current Golem idempotence policy unchanged. With idempotence disabled,
//! recovery of an interrupted POST can fail closed. Exactly-once effects require the peer
//! to atomically retain producer deduplication state. Golem forks copy producer identity
//! and pending data; divergent branches are not independent external producers.

use crate::bindings::golem::agent::host;
use crate::schema::wit::wire::Secret;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::sync::Arc;

#[cfg(not(test))]
use host as calls;
#[cfg(test)]
use tests as calls;

pub use host::{
    DurableStreamAppendReceipt as AppendReceipt, DurableStreamCheckpoint as Checkpoint,
    DurableStreamErrorKind as ErrorKind, DurableStreamProducer as Producer,
    DurableStreamTransport as Transport,
};

/// A host protocol failure or a local codec/state error. Errors never mean EOF.
#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub retry_after_ms: Option<u64>,
    pub producer_epoch: Option<u64>,
    pub expected_sequence: Option<u64>,
}

impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_ms: None,
            producer_epoch: None,
            expected_sequence: None,
        }
    }
}

impl From<host::DurableStreamError> for Error {
    fn from(value: host::DurableStreamError) -> Self {
        Self {
            kind: value.kind,
            message: value.message,
            retry_after_ms: value.retry_after_ms,
            producer_epoch: value.producer_epoch,
            expected_sequence: value.expected_sequence,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Error {}

/// Consecutive failure budget, independent of each HTTP attempt's deadline.
#[derive(Clone, Debug)]
pub struct RetryOptions {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryOptions {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 10_000,
        }
    }
}

impl RetryOptions {
    fn delay(&self, error: &Error, failures: u32) -> Option<u64> {
        if failures >= self.max_retries
            || !matches!(
                error.kind,
                ErrorKind::Timeout
                    | ErrorKind::Transport
                    | ErrorKind::RateLimited
                    | ErrorKind::Unavailable
            )
        {
            return None;
        }
        let backoff = self
            .initial_delay_ms
            .saturating_mul(1u64 << failures.min(63))
            .min(self.max_delay_ms);
        Some(backoff.max(error.retry_after_ms.unwrap_or(0)))
    }
}

async fn wait(delay_ms: u64) {
    #[cfg(not(test))]
    crate::wasip3::clocks::monotonic_clock::wait_for(delay_ms.saturating_mul(1_000_000)).await;
    #[cfg(test)]
    tests::WAITS.with_borrow_mut(|waits| waits.push(delay_ms));
}

#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub checkpoint: Checkpoint,
    /// Transport used after catch-up reaches the tail. The initial call is always catch-up.
    pub transport: Transport,
    pub timeout_ms: u64,
    pub idle_delay_ms: u64,
    pub retry: RetryOptions,
    /// A secret capability, never its revealed value. Shared between independent readers/writers.
    pub auth: Option<Arc<Secret>>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            checkpoint: Checkpoint {
                offset: "-1".into(),
                cursor: None,
            },
            transport: Transport::LongPoll,
            timeout_ms: 30_000,
            idle_delay_ms: 100,
            retry: RetryOptions::default(),
            auth: None,
        }
    }
}

enum Items {
    Json(Vec<Box<RawValue>>),
    Bytes(Vec<u8>),
}

impl Items {
    fn len(&self) -> usize {
        match self {
            Self::Json(items) => items.len(),
            Self::Bytes(items) => items.len(),
        }
    }
}

struct Batch {
    items: Items,
    index: usize,
    next: Checkpoint,
    up_to_date: bool,
    closed: bool,
}

/// A single-reader, one-batch-buffered external source. `&mut self` serializes reads.
/// A delivered batch is drained before its opaque checkpoint is promoted.
/// Dropping a pending `next` cancels that attempt without advancing the checkpoint.
/// Live readers are reconstructed by replay, not serialized as native stream handles.
pub struct ExternalDurableStream<T> {
    request: host::DurableStreamReadRequest,
    options: ReadOptions,
    decode: fn(&Items, usize) -> Result<T, Error>,
    pending: Option<Batch>,
    closed: bool,
    failures: u32,
    delay_ms: Option<u64>,
}

impl<T: DeserializeOwned> ExternalDurableStream<T> {
    pub fn json(url: impl Into<String>, options: ReadOptions) -> Self {
        Self::new(
            url.into(),
            options,
            host::DurableStreamMode::Json,
            |items, index| {
                let Items::Json(items) = items else {
                    unreachable!()
                };
                serde_json::from_str(items[index].get()).map_err(|e| {
                    Error::new(
                        ErrorKind::ProtocolError,
                        format!("JSON item decode failed: {e}"),
                    )
                })
            },
        )
    }
}

impl ExternalDurableStream<u8> {
    /// Bytes are a sequence, not the original append chunk boundaries.
    pub fn bytes(url: impl Into<String>, options: ReadOptions) -> Self {
        Self::new(
            url.into(),
            options,
            host::DurableStreamMode::Bytes,
            |items, index| {
                let Items::Bytes(items) = items else {
                    unreachable!()
                };
                Ok(items[index])
            },
        )
    }
}

impl<T> ExternalDurableStream<T> {
    fn new(
        url: String,
        options: ReadOptions,
        mode: host::DurableStreamMode,
        decode: fn(&Items, usize) -> Result<T, Error>,
    ) -> Self {
        Self {
            request: host::DurableStreamReadRequest {
                url,
                checkpoint: options.checkpoint.clone(),
                mode,
                transport: Transport::CatchUp,
                content_type: None,
                timeout_ms: options.timeout_ms,
            },
            options,
            decode,
            pending: None,
            closed: false,
            failures: 0,
            delay_ms: None,
        }
    }

    /// Last fully drained checkpoint. A partially consumed batch retains its original offset.
    pub fn checkpoint(&self) -> &Checkpoint {
        &self.request.checkpoint
    }

    /// Stop pulling and discard buffered items. To cancel an active pull, drop its future first.
    pub fn close(&mut self) {
        self.closed = true;
        self.pending = None;
    }

    fn install(&mut self, batch: host::DurableStreamBatch) -> Result<(), Error> {
        if batch.next.offset == "now" {
            return Err(Error::new(
                ErrorKind::ProtocolError,
                "server did not resolve now",
            ));
        }
        let items = match self.request.mode {
            host::DurableStreamMode::Json => Items::Json(if batch.payload.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(&batch.payload).map_err(|e| {
                    Error::new(
                        ErrorKind::ProtocolError,
                        format!("JSON batch decode failed: {e}"),
                    )
                })?
            }),
            host::DurableStreamMode::Bytes => Items::Bytes(batch.payload),
        };
        // The host validates MIME equivalence against the original pin.
        self.request.content_type.get_or_insert(batch.content_type);
        self.pending = Some(Batch {
            items,
            index: 0,
            next: batch.next,
            up_to_date: batch.up_to_date,
            closed: batch.closed,
        });
        self.failures = 0;
        Ok(())
    }

    pub async fn next(&mut self) -> Result<Option<T>, Error> {
        loop {
            if self.closed {
                return Ok(None);
            }
            if let Some(batch) = &mut self.pending {
                if batch.index < batch.items.len() {
                    let item = (self.decode)(&batch.items, batch.index)?;
                    batch.index += 1;
                    return Ok(Some(item));
                }
                let batch = self.pending.take().unwrap();
                self.request.checkpoint = batch.next;
                self.closed = batch.closed;
                self.request.transport = if batch.up_to_date {
                    self.options.transport
                } else {
                    Transport::CatchUp
                };
                if batch.items.len() == 0 && batch.up_to_date && !batch.closed {
                    self.delay_ms = Some(self.options.idle_delay_ms);
                }
                continue;
            }
            if let Some(delay) = self.delay_ms {
                wait(delay).await;
                self.delay_ms = None;
            }
            match calls::read_durable_stream_batch(
                self.request.clone(),
                self.options.auth.as_deref(),
            )
            .await
            {
                Ok(batch) => self.install(batch)?,
                Err(error) => {
                    let error = Error::from(error);
                    let Some(delay) = self.options.retry.delay(&error, self.failures) else {
                        return Err(error);
                    };
                    self.failures += 1;
                    self.delay_ms = Some(delay);
                }
            }
        }
    }

    pub async fn collect(mut self) -> Result<Vec<T>, Error> {
        let mut items = Vec::new();
        while let Some(item) = self.next().await? {
            items.push(item);
        }
        Ok(items)
    }
}

#[cfg(feature = "export_golem_agentic")]
impl<T: crate::IntoSchema + crate::FromSchema + Send + 'static> ExternalDurableStream<T> {
    /// Preserve ordinary native stream schemas, including nested values and RPC inputs.
    /// Local native reads return String errors; a forwarded producer failure traps, never EOF.
    pub fn into_agent_stream(self) -> crate::agentic::AgentStream<T> {
        crate::agentic::AgentStream::from_schema_stream(
            crate::schema::SchemaValueStream::from_source(self),
            |value| T::from_value(&value).map_err(|e| e.to_string()),
        )
    }
}

#[cfg(feature = "export_golem_agentic")]
impl<T: crate::IntoSchema + Send + 'static> crate::schema::stream::SchemaValueStreamSource
    for ExternalDurableStream<T>
{
    fn next(
        &mut self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Option<crate::schema::wit::wire::SchemaValueTree>, String>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            match ExternalDurableStream::next(self)
                .await
                .map_err(|e| e.to_string())?
            {
                Some(value) => crate::schema::wit::encode_value_async(&value.to_value())
                    .await
                    .map(Some)
                    .map_err(|e| e.to_string()),
                None => Ok(None),
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// None generates one durable identity at writer construction.
    pub producer_id: Option<String>,
    pub epoch: u64,
    pub timeout_ms: u64,
    pub retry: RetryOptions,
    pub auth: Option<Arc<Secret>>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            producer_id: None,
            epoch: 0,
            timeout_ms: 30_000,
            retry: RetryOptions::default(),
            auth: None,
        }
    }
}

struct PendingAppend {
    request: host::DurableStreamAppendRequest,
    failures: u32,
    delay_ms: Option<u64>,
}

/// A non-pipelining producer. A cancelled or failed append remains pending;
/// call `retry_pending` to resolve that exact payload before supplying new data.
/// Dropping this writer does not undo an external write.
pub struct DurableStreamWriter {
    url: String,
    content_type: String,
    producer: Producer,
    options: WriteOptions,
    pending: Option<PendingAppend>,
    closed: bool,
}

const MAX_PRODUCER_NUMBER: u64 = (1u64 << 53) - 1;

impl DurableStreamWriter {
    pub fn new(
        url: impl Into<String>,
        content_type: impl Into<String>,
        options: WriteOptions,
    ) -> Result<Self, Error> {
        if options.epoch > MAX_PRODUCER_NUMBER {
            return Err(Error::new(
                ErrorKind::InvalidRequest,
                "producer epoch exceeds 2^53-1",
            ));
        }
        let id = options
            .producer_id
            .clone()
            .unwrap_or_else(|| crate::generate_idempotency_key().to_string());
        Ok(Self {
            url: url.into(),
            content_type: content_type.into(),
            producer: Producer {
                id,
                epoch: options.epoch,
                sequence: 0,
            },
            options,
            pending: None,
            closed: false,
        })
    }

    pub fn producer(&self) -> &Producer {
        &self.producer
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn prepare(
        &mut self,
        payload: host::DurableStreamAppendPayload,
        close: bool,
    ) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::new(
                ErrorKind::SequenceConflict,
                "resolve the pending append before supplying new data",
            ));
        }
        if self.closed {
            return Err(Error::new(ErrorKind::Closed, "writer is closed"));
        }
        if self.producer.sequence > MAX_PRODUCER_NUMBER {
            return Err(Error::new(
                ErrorKind::InvalidRequest,
                "producer sequence exhausted",
            ));
        }
        let empty = match &payload {
            host::DurableStreamAppendPayload::Json(v) => v.is_empty(),
            host::DurableStreamAppendPayload::Bytes(v) => v.is_empty(),
        };
        if empty && !close {
            return Err(Error::new(
                ErrorKind::InvalidRequest,
                "empty append requires close",
            ));
        }
        self.pending = Some(PendingAppend {
            request: host::DurableStreamAppendRequest {
                url: self.url.clone(),
                content_type: self.content_type.clone(),
                payload,
                producer: self.producer.clone(),
                close,
                timeout_ms: self.options.timeout_ms,
            },
            failures: 0,
            delay_ms: None,
        });
        Ok(())
    }

    /// Each element is encoded separately; an array-valued element stays one message.
    pub async fn append_json<T: Serialize>(
        &mut self,
        values: &[T],
        close: bool,
    ) -> Result<AppendReceipt, Error> {
        let encoded = values
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidRequest,
                    format!("JSON encode failed: {e}"),
                )
            })?;
        self.prepare(host::DurableStreamAppendPayload::Json(encoded), close)?;
        self.retry_pending().await
    }

    pub async fn append_bytes(
        &mut self,
        bytes: &[u8],
        close: bool,
    ) -> Result<AppendReceipt, Error> {
        self.prepare(
            host::DurableStreamAppendPayload::Bytes(bytes.to_vec()),
            close,
        )?;
        self.retry_pending().await
    }

    /// Close-only consumes a sequence just like an append-and-close.
    pub async fn close(&mut self) -> Result<AppendReceipt, Error> {
        self.prepare(host::DurableStreamAppendPayload::Bytes(Vec::new()), true)?;
        self.retry_pending().await
    }

    fn acknowledge(&mut self, receipt: &AppendReceipt) -> Result<(), Error> {
        let pending = self
            .pending
            .as_ref()
            .expect("acknowledgement requires a pending request");
        if receipt.epoch != pending.request.producer.epoch
            || receipt.sequence < pending.request.producer.sequence
        {
            return Err(Error::new(
                ErrorKind::ProtocolError,
                "invalid producer acknowledgement",
            ));
        }
        if receipt.sequence > pending.request.producer.sequence {
            return Err(Error::new(
                ErrorKind::ProducerDiverged,
                "peer acknowledged a later producer sequence",
            ));
        }
        if pending.request.close && !receipt.closed {
            return Err(Error::new(
                ErrorKind::ProtocolError,
                "peer did not acknowledge closure",
            ));
        }
        self.producer.sequence = pending.request.producer.sequence + 1;
        self.closed = receipt.closed;
        self.pending = None;
        Ok(())
    }

    pub async fn retry_pending(&mut self) -> Result<AppendReceipt, Error> {
        loop {
            let pending = self
                .pending
                .as_mut()
                .ok_or_else(|| Error::new(ErrorKind::InvalidRequest, "no pending append"))?;
            if let Some(delay) = pending.delay_ms {
                wait(delay).await;
                pending.delay_ms = None;
            }
            match calls::append_durable_stream_batch(
                pending.request.clone(),
                self.options.auth.as_deref(),
            )
            .await
            {
                Ok(receipt) => {
                    self.acknowledge(&receipt)?;
                    return Ok(receipt);
                }
                Err(error) => {
                    let error = Error::from(error);
                    let Some(delay) = self.options.retry.delay(&error, pending.failures) else {
                        return Err(error);
                    };
                    pending.failures += 1;
                    pending.delay_ms = Some(delay);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    use test_r::test;

    type ReadResult = Result<host::DurableStreamBatch, host::DurableStreamError>;
    type WriteResult = Result<AppendReceipt, host::DurableStreamError>;
    thread_local! {
        static READS: RefCell<Vec<host::DurableStreamReadRequest>> = const { RefCell::new(Vec::new()) };
        static READ_RESULTS: RefCell<VecDeque<Option<ReadResult>>> = const { RefCell::new(VecDeque::new()) };
        static WRITES: RefCell<Vec<host::DurableStreamAppendRequest>> = const { RefCell::new(Vec::new()) };
        static WRITE_RESULTS: RefCell<VecDeque<Option<WriteResult>>> = const { RefCell::new(VecDeque::new()) };
        pub(super) static WAITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) async fn read_durable_stream_batch(
        request: host::DurableStreamReadRequest,
        _auth: Option<&Secret>,
    ) -> ReadResult {
        READS.with_borrow_mut(|reads| reads.push(request));
        match READ_RESULTS.with_borrow_mut(|results| results.pop_front().expect("unexpected read"))
        {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }

    pub(super) async fn append_durable_stream_batch(
        request: host::DurableStreamAppendRequest,
        _auth: Option<&Secret>,
    ) -> WriteResult {
        WRITES.with_borrow_mut(|writes| writes.push(request));
        match WRITE_RESULTS
            .with_borrow_mut(|results| results.pop_front().expect("unexpected append"))
        {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }

    fn reset() {
        READS.with_borrow_mut(Vec::clear);
        WRITES.with_borrow_mut(Vec::clear);
        READ_RESULTS.with_borrow_mut(VecDeque::clear);
        WRITE_RESULTS.with_borrow_mut(VecDeque::clear);
        WAITS.with_borrow_mut(Vec::clear);
    }

    fn ready<T>(future: impl Future<Output = T>) -> T {
        match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("expected ready future"),
        }
    }

    fn batch(payload: &[u8], offset: &str, closed: bool) -> host::DurableStreamBatch {
        host::DurableStreamBatch {
            payload: payload.to_vec(),
            content_type: "application/json".into(),
            next: Checkpoint {
                offset: offset.into(),
                cursor: Some(format!("cursor-{offset}")),
            },
            up_to_date: true,
            closed,
        }
    }

    fn receipt(sequence: u64, closed: bool) -> AppendReceipt {
        AppendReceipt {
            next_offset: Some(format!("opaque-{sequence}")),
            epoch: 7,
            sequence,
            closed,
        }
    }

    fn writer() -> DurableStreamWriter {
        DurableStreamWriter::new(
            "https://example.test/stream",
            "application/json",
            WriteOptions {
                producer_id: Some("producer".into()),
                epoch: 7,
                ..WriteOptions::default()
            },
        )
        .unwrap()
    }

    fn failure(kind: ErrorKind) -> host::DurableStreamError {
        host::DurableStreamError {
            kind,
            message: "failure".into(),
            retry_after_ms: None,
            producer_epoch: None,
            expected_sequence: None,
        }
    }

    #[test]
    fn drains_exact_json_before_checkpoint_and_final_eof() {
        reset();
        READ_RESULTS.with_borrow_mut(|r| {
            r.push_back(Some(Ok(batch(
                b"[[9007199254740993,18446744073709551615],[3,5]]",
                "opaque-tail",
                true,
            ))))
        });
        let mut stream =
            ExternalDurableStream::<Vec<u64>>::json("https://example.test", ReadOptions::default());
        assert_eq!(
            ready(stream.next()).unwrap(),
            Some(vec![9_007_199_254_740_993, u64::MAX])
        );
        assert_eq!(stream.checkpoint().offset, "-1");
        assert_eq!(ready(stream.next()).unwrap(), Some(vec![3, 5]));
        assert_eq!(ready(stream.next()).unwrap(), None);
        assert_eq!(stream.checkpoint().offset, "opaque-tail");
        assert_eq!(
            stream.checkpoint().cursor.as_deref(),
            Some("cursor-opaque-tail")
        );
        READS.with_borrow(|r| assert_eq!(r.len(), 1));
    }

    #[test]
    fn now_is_resolved_once_and_sse_pins_content_type_and_cursor() {
        reset();
        READ_RESULTS.with_borrow_mut(|r| {
            r.extend([
                Some(Ok(batch(b"[]", "tail", false))),
                Some(Ok(batch(b"[23]", "next", true))),
            ])
        });
        let options = ReadOptions {
            checkpoint: Checkpoint {
                offset: "now".into(),
                cursor: None,
            },
            transport: Transport::Sse,
            ..ReadOptions::default()
        };
        let mut stream = ExternalDurableStream::<u64>::json("https://example.test", options);
        assert_eq!(ready(stream.next()).unwrap(), Some(23));
        assert_eq!(ready(stream.next()).unwrap(), None);
        READS.with_borrow(|r| {
            assert_eq!(r.len(), 2);
            assert_eq!(r[0].checkpoint.offset, "now");
            assert!(matches!(r[0].transport, Transport::CatchUp));
            assert_eq!(r[1].checkpoint.offset, "tail");
            assert_eq!(r[1].checkpoint.cursor.as_deref(), Some("cursor-tail"));
            assert!(matches!(r[1].transport, Transport::Sse));
            assert_eq!(r[1].content_type.as_deref(), Some("application/json"));
        });
        WAITS.with_borrow(|waits| assert_eq!(waits, &[100]));
    }

    #[test]
    fn equivalent_mime_spellings_keep_the_original_pin() {
        reset();
        let original = "Application/JSON; Charset=utf-8; profile=example";
        let equivalent = "application/json; profile=example; charset=\"utf-8\"";
        let mut first = batch(b"[3]", "first", false);
        first.content_type = original.into();
        let mut second = batch(b"[17]", "second", false);
        second.content_type = equivalent.into();
        let mut last = batch(b"[29]", "end", true);
        last.content_type = equivalent.into();
        READ_RESULTS.with_borrow_mut(|r| {
            r.extend([Some(Ok(first)), Some(Ok(second)), Some(Ok(last))]);
        });
        let stream =
            ExternalDurableStream::<u64>::json("https://example.test", ReadOptions::default());
        assert_eq!(ready(stream.collect()).unwrap(), [3, 17, 29]);
        READS.with_borrow(|r| {
            assert_eq!(r.len(), 3);
            assert_eq!(r[0].content_type, None);
            assert_eq!(r[1].content_type.as_deref(), Some(original));
            assert_eq!(r[2].content_type.as_deref(), Some(original));
        });
    }

    #[test]
    fn bytes_cross_batches_without_exposing_chunk_boundaries() {
        reset();
        let mut first = batch(&[3, 255], "first", false);
        first.content_type = "application/octet-stream".into();
        first.up_to_date = false;
        let mut final_batch = batch(&[0, 19, 4], "end", true);
        final_batch.content_type = "application/octet-stream".into();
        READ_RESULTS.with_borrow_mut(|r| r.extend([Some(Ok(first)), Some(Ok(final_batch))]));
        assert_eq!(
            ready(
                ExternalDurableStream::bytes("https://example.test", ReadOptions::default())
                    .collect()
            )
            .unwrap(),
            [3, 255, 0, 19, 4]
        );
        READS.with_borrow(|r| {
            assert_eq!(r[1].checkpoint.offset, "first");
            assert!(matches!(r[1].transport, Transport::CatchUp));
        });
    }

    #[test]
    fn malformed_item_stays_at_same_index_and_is_not_eof() {
        reset();
        READ_RESULTS
            .with_borrow_mut(|r| r.push_back(Some(Ok(batch(br#"[7,"bad",9]"#, "end", true)))));
        let mut stream =
            ExternalDurableStream::<u64>::json("https://example.test", ReadOptions::default());
        assert_eq!(ready(stream.next()).unwrap(), Some(7));
        for _ in 0..2 {
            assert!(matches!(
                ready(stream.next()).unwrap_err().kind,
                ErrorKind::ProtocolError
            ));
            assert_eq!(stream.checkpoint().offset, "-1");
        }
        READS.with_borrow(|r| assert_eq!(r.len(), 1));
    }

    #[test]
    fn read_retry_budget_and_retry_after_do_not_change_request() {
        reset();
        let mut throttled = failure(ErrorKind::RateLimited);
        throttled.retry_after_ms = Some(17_000);
        READ_RESULTS.with_borrow_mut(|r| {
            r.extend([
                Some(Err(throttled)),
                Some(Err(failure(ErrorKind::Timeout))),
                Some(Err(failure(ErrorKind::Transport))),
                Some(Err(failure(ErrorKind::Unavailable))),
            ])
        });
        let mut stream =
            ExternalDurableStream::<u64>::json("https://example.test", ReadOptions::default());
        assert!(matches!(
            ready(stream.next()).unwrap_err().kind,
            ErrorKind::Unavailable
        ));
        READS.with_borrow(|r| {
            assert_eq!(r.len(), 4);
            assert!(r.iter().all(|r| r.checkpoint.offset == "-1"));
        });
        WAITS.with_borrow(|r| assert_eq!(r, &[17_000, 200, 400]));
        assert_eq!(stream.failures, 3);
        assert!(
            RetryOptions::default()
                .delay(&Error::new(ErrorKind::Gone, "gone"), 0)
                .is_none()
        );
    }

    #[test]
    fn cancelled_append_retains_exact_tuple_and_payload_until_ack() {
        reset();
        WRITE_RESULTS.with_borrow_mut(|r| {
            r.extend([
                None,
                Some(Ok(receipt(0, false))),
                Some(Ok(receipt(1, true))),
            ])
        });
        let mut writer = writer();
        {
            let values = [vec![9_007_199_254_740_993u64, 3]];
            let pending = writer.append_json(&values, false);
            assert!(
                std::pin::pin!(pending)
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        assert!(writer.has_pending());
        assert_eq!(writer.producer().sequence, 0);
        assert!(matches!(
            ready(writer.append_bytes(&[2, 3], false)).unwrap_err().kind,
            ErrorKind::SequenceConflict
        ));
        ready(writer.retry_pending()).unwrap();
        assert_eq!(writer.producer().sequence, 1);
        ready(writer.close()).unwrap();
        assert_eq!(writer.producer().sequence, 2);
        WRITES.with_borrow(|r| {
            assert_eq!(r.len(), 3);
            for request in &r[..2] {
                assert_eq!(request.producer.id, "producer");
                assert_eq!(request.producer.epoch, 7);
                assert_eq!(request.producer.sequence, 0);
                assert!(!request.close);
                let host::DurableStreamAppendPayload::Json(values) = &request.payload else {
                    panic!()
                };
                assert_eq!(values, &["[9007199254740993,3]"]);
            }
            assert!(r[2].close);
            assert_eq!(r[2].producer.sequence, 1);
        });
    }

    #[test]
    fn invalid_or_diverged_ack_never_clears_pending() {
        reset();
        let mut writer = writer();
        writer.producer.sequence = 4;
        writer
            .prepare(host::DurableStreamAppendPayload::Bytes(vec![3, 1]), false)
            .unwrap();
        for (epoch, sequence, expected) in [
            (8, 4, ErrorKind::ProtocolError),
            (7, 3, ErrorKind::ProtocolError),
            (7, 5, ErrorKind::ProducerDiverged),
        ] {
            let mut receipt = receipt(sequence, false);
            receipt.epoch = epoch;
            assert_eq!(writer.acknowledge(&receipt).unwrap_err().kind, expected);
            assert!(writer.has_pending());
            assert_eq!(writer.producer.sequence, 4);
        }
        writer.acknowledge(&receipt(4, false)).unwrap();
        assert!(!writer.has_pending());
        assert_eq!(writer.producer.sequence, 5);
    }

    #[test]
    fn acknowledgement_without_offset_advances_sequence_and_preserves_none() {
        reset();
        let mut missing_offset = receipt(0, false);
        missing_offset.next_offset = None;
        let mut close_receipt = receipt(1, true);
        close_receipt.next_offset = None;
        WRITE_RESULTS.with_borrow_mut(|r| {
            r.extend([Some(Ok(missing_offset)), Some(Ok(close_receipt))]);
        });
        let mut writer = writer();
        let appended = ready(writer.append_json(&["value"], false)).unwrap();
        assert_eq!(appended.next_offset, None);
        assert_eq!(writer.producer().sequence, 1);
        assert!(!writer.has_pending());
        let closed = ready(writer.close()).unwrap();
        assert_eq!(closed.next_offset, None);
        assert!(closed.closed);
        assert_eq!(writer.producer().sequence, 2);
        assert!(!writer.has_pending());
    }

    #[test]
    fn append_retry_does_not_renumber_or_reencode() {
        reset();
        WRITE_RESULTS.with_borrow_mut(|r| {
            r.extend([
                Some(Err(failure(ErrorKind::Transport))),
                Some(Ok(receipt(0, true))),
            ])
        });
        let mut writer = writer();
        ready(writer.append_bytes(&[13, 0, 255, 2], true)).unwrap();
        WRITES.with_borrow(|r| {
            assert_eq!(r.len(), 2);
            for r in r {
                assert!(r.close);
                assert_eq!(r.producer.sequence, 0);
                let host::DurableStreamAppendPayload::Bytes(bytes) = &r.payload else {
                    panic!()
                };
                assert_eq!(bytes, &[13, 0, 255, 2]);
            }
        });
        WAITS.with_borrow(|r| assert_eq!(r, &[100]));
        assert!(matches!(
            ready(writer.append_bytes(&[1], false)).unwrap_err().kind,
            ErrorKind::Closed
        ));
    }

    #[test]
    fn close_only_and_producer_limit_are_checked_without_renumbering() {
        let mut writer = writer();
        assert!(
            writer
                .prepare(host::DurableStreamAppendPayload::Bytes(vec![]), false)
                .is_err()
        );
        writer.producer.sequence = MAX_PRODUCER_NUMBER;
        writer
            .prepare(host::DurableStreamAppendPayload::Bytes(vec![]), true)
            .unwrap();
        assert!(
            writer
                .acknowledge(&receipt(MAX_PRODUCER_NUMBER, false))
                .is_err()
        );
        writer
            .acknowledge(&receipt(MAX_PRODUCER_NUMBER, true))
            .unwrap();
        assert_eq!(writer.producer.sequence, MAX_PRODUCER_NUMBER + 1);
    }

    #[cfg(feature = "export_golem_agentic")]
    #[test]
    fn native_source_is_lazy_cancel_safe_and_keeps_errors() {
        reset();
        READ_RESULTS.with_borrow_mut(|r| r.extend([None, Some(Err(failure(ErrorKind::Gone)))]));
        let external =
            ExternalDurableStream::<u64>::json("https://example.test", ReadOptions::default());
        let mut stream = external.into_agent_stream();
        READS.with_borrow(|r| assert!(r.is_empty()));
        {
            let next = stream.next();
            assert!(
                std::pin::pin!(next)
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        let error = ready(stream.next()).unwrap_err();
        assert!(error.contains("Gone"));
        READS.with_borrow(|r| {
            assert_eq!(r.len(), 2);
            assert_eq!(r[1].checkpoint.offset, "-1");
        });
    }
}
