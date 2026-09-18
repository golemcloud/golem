// Copyright 2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! External Durable Streams clients. The host owns HTTP, framing and authentication;
//! these clients retain only replay-reconstructed checkpoints, buffers and producer state.
//! Each client owns one `golem:agent/durable-streams@2.0.0` resource. Construction
//! journals its immutable descriptor without HTTP; dropping the client releases it.
//!
//! Appends use the current Golem idempotence policy unchanged. With idempotence disabled,
//! recovery of an interrupted POST can fail closed. Exactly-once effects require the peer
//! to atomically retain producer deduplication state. Golem forks copy producer identity
//! and pending data; divergent branches are not independent external producers.

use crate::bindings::golem::agent::durable_streams as wire;
use crate::schema::wit::wire::Secret;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::sync::Arc;

#[cfg(test)]
use tests::resources as calls;
#[cfg(not(test))]
use wire as calls;

pub use wire::{
    DurableStreamAppendReceipt as AppendReceipt, DurableStreamCheckpoint as Checkpoint,
    DurableStreamErrorKind as ErrorKind, DurableStreamTransport as Transport,
};

/// Producer identity and acknowledged sequence progress, reconstructed by replay.
#[derive(Clone, Debug)]
pub struct Producer {
    pub id: String,
    pub epoch: u64,
    pub sequence: u64,
}

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

impl From<wire::DurableStreamError> for Error {
    fn from(value: wire::DurableStreamError) -> Self {
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
    reader: calls::DurableStreamReader,
    mode: wire::DurableStreamMode,
    request: wire::DurableStreamReadRequest,
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
            wire::DurableStreamMode::Json,
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
            wire::DurableStreamMode::Bytes,
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
        mut options: ReadOptions,
        mode: wire::DurableStreamMode,
        decode: fn(&Items, usize) -> Result<T, Error>,
    ) -> Self {
        let reader = calls::DurableStreamReader::new(
            &wire::DurableStreamReaderOptions {
                url,
                mode,
                timeout_ms: options.timeout_ms,
            },
            options.auth.as_deref(),
        );
        options.auth = None;
        Self {
            reader,
            mode,
            request: wire::DurableStreamReadRequest {
                checkpoint: options.checkpoint.clone(),
                transport: Transport::CatchUp,
                content_type: None,
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

    fn install(&mut self, batch: wire::DurableStreamBatch) -> Result<(), Error> {
        if batch.next.offset == "now" {
            return Err(Error::new(
                ErrorKind::ProtocolError,
                "server did not resolve now",
            ));
        }
        let items = match self.mode {
            wire::DurableStreamMode::Json => Items::Json(if batch.payload.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(&batch.payload).map_err(|e| {
                    Error::new(
                        ErrorKind::ProtocolError,
                        format!("JSON batch decode failed: {e}"),
                    )
                })?
            }),
            wire::DurableStreamMode::Bytes => Items::Bytes(batch.payload),
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
            match self.reader.read(self.request.clone()).await {
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
    request: wire::DurableStreamAppendRequest,
    failures: u32,
    delay_ms: Option<u64>,
}

/// A non-pipelining producer. A cancelled or failed append remains pending;
/// call `retry_pending` to resolve that exact payload before supplying new data.
/// Dropping this writer does not undo an external write.
pub struct DurableStreamWriter {
    writer: calls::DurableStreamWriter,
    producer: Producer,
    retry: RetryOptions,
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
            writer: calls::DurableStreamWriter::new(
                &wire::DurableStreamWriterOptions {
                    url: url.into(),
                    content_type: content_type.into(),
                    producer_id: id.clone(),
                    producer_epoch: options.epoch,
                    timeout_ms: options.timeout_ms,
                },
                options.auth.as_deref(),
            ),
            producer: Producer {
                id,
                epoch: options.epoch,
                sequence: 0,
            },
            retry: options.retry,
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
        payload: wire::DurableStreamAppendPayload,
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
            wire::DurableStreamAppendPayload::Json(v) => v.is_empty(),
            wire::DurableStreamAppendPayload::Bytes(v) => v.is_empty(),
        };
        if empty && !close {
            return Err(Error::new(
                ErrorKind::InvalidRequest,
                "empty append requires close",
            ));
        }
        self.pending = Some(PendingAppend {
            request: wire::DurableStreamAppendRequest {
                payload,
                sequence: self.producer.sequence,
                close,
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
        self.prepare(wire::DurableStreamAppendPayload::Json(encoded), close)?;
        self.retry_pending().await
    }

    pub async fn append_bytes(
        &mut self,
        bytes: &[u8],
        close: bool,
    ) -> Result<AppendReceipt, Error> {
        self.prepare(
            wire::DurableStreamAppendPayload::Bytes(bytes.to_vec()),
            close,
        )?;
        self.retry_pending().await
    }

    /// Close-only consumes a sequence just like an append-and-close.
    pub async fn close(&mut self) -> Result<AppendReceipt, Error> {
        self.prepare(wire::DurableStreamAppendPayload::Bytes(Vec::new()), true)?;
        self.retry_pending().await
    }

    fn acknowledge(&mut self, receipt: &AppendReceipt) -> Result<(), Error> {
        let pending = self
            .pending
            .as_ref()
            .expect("acknowledgement requires a pending request");
        if receipt.epoch != self.producer.epoch || receipt.sequence < pending.request.sequence {
            return Err(Error::new(
                ErrorKind::ProtocolError,
                "invalid producer acknowledgement",
            ));
        }
        if receipt.sequence > pending.request.sequence {
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
        self.producer.sequence = pending.request.sequence + 1;
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
            match self.writer.append(pending.request.clone()).await {
                Ok(receipt) => {
                    self.acknowledge(&receipt)?;
                    return Ok(receipt);
                }
                Err(error) => {
                    let error = Error::from(error);
                    let Some(delay) = self.retry.delay(&error, pending.failures) else {
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
mod tests;
