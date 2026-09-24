use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use test_r::test;

type ReadResult = Result<wire::DurableStreamBatch, wire::DurableStreamError>;
type WriteResult = Result<AppendReceipt, wire::DurableStreamError>;
thread_local! {
    static READERS: RefCell<Vec<wire::DurableStreamReaderOptions>> = const { RefCell::new(Vec::new()) };
    static WRITERS: RefCell<Vec<wire::DurableStreamWriterOptions>> = const { RefCell::new(Vec::new()) };
    static READ_HANDLES: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static WRITE_HANDLES: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static DROPPED_READERS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static DROPPED_WRITERS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static READS: RefCell<Vec<wire::DurableStreamReadRequest>> = const { RefCell::new(Vec::new()) };
    static READ_RESULTS: RefCell<VecDeque<Option<ReadResult>>> = const { RefCell::new(VecDeque::new()) };
    static WRITES: RefCell<Vec<wire::DurableStreamAppendRequest>> = const { RefCell::new(Vec::new()) };
    static WRITE_RESULTS: RefCell<VecDeque<Option<WriteResult>>> = const { RefCell::new(VecDeque::new()) };
    pub(super) static WAITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

pub(super) mod resources {
    use super::*;

    pub struct DurableStreamReader(usize);

    impl DurableStreamReader {
        pub fn new(options: &wire::DurableStreamReaderOptions, _auth: Option<&Secret>) -> Self {
            Self(READERS.with_borrow_mut(|readers| {
                let id = readers.len();
                readers.push(options.clone());
                id
            }))
        }

        pub async fn read(&self, request: wire::DurableStreamReadRequest) -> ReadResult {
            READ_HANDLES.with_borrow_mut(|handles| handles.push(self.0));
            READS.with_borrow_mut(|reads| reads.push(request));
            match READ_RESULTS
                .with_borrow_mut(|results| results.pop_front().expect("unexpected read"))
            {
                Some(result) => result,
                None => std::future::pending().await,
            }
        }
    }

    impl Drop for DurableStreamReader {
        fn drop(&mut self) {
            DROPPED_READERS.with_borrow_mut(|handles| handles.push(self.0));
        }
    }

    pub struct DurableStreamWriter(usize);

    impl DurableStreamWriter {
        pub fn new(options: &wire::DurableStreamWriterOptions, _auth: Option<&Secret>) -> Self {
            Self(WRITERS.with_borrow_mut(|writers| {
                let id = writers.len();
                writers.push(options.clone());
                id
            }))
        }

        pub async fn append(&self, request: wire::DurableStreamAppendRequest) -> WriteResult {
            WRITE_HANDLES.with_borrow_mut(|handles| handles.push(self.0));
            WRITES.with_borrow_mut(|writes| writes.push(request));
            match WRITE_RESULTS
                .with_borrow_mut(|results| results.pop_front().expect("unexpected append"))
            {
                Some(result) => result,
                None => std::future::pending().await,
            }
        }
    }

    impl Drop for DurableStreamWriter {
        fn drop(&mut self) {
            DROPPED_WRITERS.with_borrow_mut(|handles| handles.push(self.0));
        }
    }
}

fn reset() {
    READERS.with_borrow_mut(Vec::clear);
    WRITERS.with_borrow_mut(Vec::clear);
    READ_HANDLES.with_borrow_mut(Vec::clear);
    WRITE_HANDLES.with_borrow_mut(Vec::clear);
    DROPPED_READERS.with_borrow_mut(Vec::clear);
    DROPPED_WRITERS.with_borrow_mut(Vec::clear);
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

fn batch(payload: &[u8], offset: &str, closed: bool) -> wire::DurableStreamBatch {
    wire::DurableStreamBatch {
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

fn failure(kind: ErrorKind) -> wire::DurableStreamError {
    wire::DurableStreamError {
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
    READERS.with_borrow(|r| assert_eq!(r.len(), 1));
    READ_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0]));
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
    let stream = ExternalDurableStream::<u64>::json("https://example.test", ReadOptions::default());
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
            ExternalDurableStream::bytes("https://example.test", ReadOptions::default()).collect()
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
    READ_RESULTS.with_borrow_mut(|r| r.push_back(Some(Ok(batch(br#"[7,"bad",9]"#, "end", true)))));
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
    READERS.with_borrow(|r| assert_eq!(r.len(), 1));
    READ_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0, 0, 0]));
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
            assert_eq!(request.sequence, 0);
            assert!(!request.close);
            let wire::DurableStreamAppendPayload::Json(values) = &request.payload else {
                panic!()
            };
            assert_eq!(values, &["[9007199254740993,3]"]);
        }
        assert!(r[2].close);
        assert_eq!(r[2].sequence, 1);
    });
    WRITERS.with_borrow(|r| {
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://example.test/stream");
        assert_eq!(r[0].content_type, "application/json");
        assert_eq!(r[0].producer_id, "producer");
        assert_eq!(r[0].producer_epoch, 7);
        assert_eq!(r[0].timeout_ms, 30_000);
    });
    WRITE_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0, 0]));
    drop(writer);
    DROPPED_WRITERS.with_borrow(|r| assert_eq!(r, &[0]));
}

#[test]
fn invalid_or_diverged_ack_never_clears_pending() {
    reset();
    let mut writer = writer();
    writer.producer.sequence = 4;
    writer
        .prepare(wire::DurableStreamAppendPayload::Bytes(vec![3, 1]), false)
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
    WRITERS.with_borrow(|r| assert_eq!(r.len(), 1));
    WRITE_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0]));
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
            assert_eq!(r.sequence, 0);
            let wire::DurableStreamAppendPayload::Bytes(bytes) = &r.payload else {
                panic!()
            };
            assert_eq!(bytes, &[13, 0, 255, 2]);
        }
    });
    WAITS.with_borrow(|r| assert_eq!(r, &[100]));
    WRITERS.with_borrow(|r| assert_eq!(r.len(), 1));
    WRITE_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0]));
    assert!(matches!(
        ready(writer.append_bytes(&[1], false)).unwrap_err().kind,
        ErrorKind::Closed
    ));
}

#[test]
fn close_only_and_producer_limit_are_checked_without_renumbering() {
    reset();
    let mut writer = writer();
    assert!(
        writer
            .prepare(wire::DurableStreamAppendPayload::Bytes(vec![]), false)
            .is_err()
    );
    writer.producer.sequence = MAX_PRODUCER_NUMBER;
    writer
        .prepare(wire::DurableStreamAppendPayload::Bytes(vec![]), true)
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

#[test]
fn independent_readers_capture_descriptors_once_and_release_on_drop() {
    reset();
    let mut options = ReadOptions {
        timeout_ms: 17_123,
        ..ReadOptions::default()
    };
    let mut json = ExternalDurableStream::<u64>::json("https://example.test/json", options.clone());
    options.timeout_ms = 29_321;
    let bytes = ExternalDurableStream::bytes("https://example.test/bytes", options);
    READS.with_borrow(|r| assert!(r.is_empty()));
    READERS.with_borrow(|r| {
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].url, "https://example.test/json");
        assert_eq!(r[0].mode, wire::DurableStreamMode::Json);
        assert_eq!(r[0].timeout_ms, 17_123);
        assert_eq!(r[1].url, "https://example.test/bytes");
        assert_eq!(r[1].mode, wire::DurableStreamMode::Bytes);
        assert_eq!(r[1].timeout_ms, 29_321);
    });
    READ_RESULTS.with_borrow_mut(|r| {
        r.extend([
            Some(Ok(batch(&[5, 0, 255], "bytes-end", true))),
            Some(Ok(batch(b"[17]", "json-next", false))),
            Some(Ok(batch(b"[29]", "json-end", true))),
        ]);
    });
    assert_eq!(ready(bytes.collect()).unwrap(), [5, 0, 255]);
    assert_eq!(ready(json.next()).unwrap(), Some(17));
    assert_eq!(ready(json.next()).unwrap(), Some(29));
    json.close();
    assert_eq!(ready(json.next()).unwrap(), None);
    READERS.with_borrow(|r| assert_eq!(r.len(), 2));
    READ_HANDLES.with_borrow(|r| assert_eq!(r, &[1, 0, 0]));
    DROPPED_READERS.with_borrow(|r| assert_eq!(r, &[1]));
    drop(json);
    DROPPED_READERS.with_borrow(|r| assert_eq!(r, &[1, 0]));
}

#[test]
fn independent_writers_keep_descriptors_and_drop_uncertain_operations() {
    reset();
    let mut first = writer();
    let mut second = DurableStreamWriter::new(
        "https://example.test/bytes",
        "application/octet-stream",
        WriteOptions {
            producer_id: Some("second-producer".into()),
            epoch: 11,
            timeout_ms: 12_345,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    WRITES.with_borrow(|r| assert!(r.is_empty()));
    let mut second_ack = receipt(0, false);
    second_ack.epoch = 11;
    WRITE_RESULTS.with_borrow_mut(|r| {
        r.extend([Some(Ok(second_ack)), None]);
    });
    ready(second.append_bytes(&[9, 31], false)).unwrap();
    {
        let append = first.append_json(&[23], false);
        assert!(
            std::pin::pin!(append)
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert!(first.has_pending());
    assert_eq!(first.producer().sequence, 0);
    assert_eq!(second.producer().sequence, 1);
    WRITERS.with_borrow(|r| {
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].producer_id, "producer");
        assert_eq!(r[0].producer_epoch, 7);
        assert_eq!(r[1].url, "https://example.test/bytes");
        assert_eq!(r[1].content_type, "application/octet-stream");
        assert_eq!(r[1].producer_id, "second-producer");
        assert_eq!(r[1].producer_epoch, 11);
        assert_eq!(r[1].timeout_ms, 12_345);
    });
    WRITE_HANDLES.with_borrow(|r| assert_eq!(r, &[1, 0]));
    drop(first);
    drop(second);
    DROPPED_WRITERS.with_borrow(|r| assert_eq!(r, &[0, 1]));
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
    READERS.with_borrow(|r| assert_eq!(r.len(), 1));
    READ_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0]));
    drop(stream);
    DROPPED_READERS.with_borrow(|r| assert_eq!(r, &[0]));

    let unread = ExternalDurableStream::bytes("https://example.test", ReadOptions::default())
        .into_agent_stream();
    drop(unread);
    READ_HANDLES.with_borrow(|r| assert_eq!(r, &[0, 0]));
    DROPPED_READERS.with_borrow(|r| assert_eq!(r, &[0, 1]));
}
