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

use super::super::{ToolOutputEntry, ToolOutputWriterEntry, ToolStdinEntry, ToolStdinWriterEntry};
use super::*;
use test_r::test;
use tokio::sync::mpsc;
use wasmtime::component::{Component, Linker, Source, StreamConsumer, StreamReader};
use wasmtime::{Config, Engine, Store, StoreContextMut};

fn pair(limit: usize) -> (AttachmentProducer, AttachmentConsumer, AttachmentObserver) {
    attachment_pair(limit, AttachmentMemory::inert())
}

#[test]
async fn offered_replay_buffer_holds_hydration_slot_until_transfer_or_drop() {
    for disposition in 0..3 {
        let slot = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = slot.clone().acquire_owned().await.unwrap();
        let mut buffer = AttachmentStreamBuffer::new(Ok(vec![7, 11, 19]), Some(permit));
        buffer.skip(0);
        assert!(slot.try_acquire().is_err());
        match disposition {
            0 => {
                let mut transferred = None;
                buffer.take(1, &mut |items| {
                    assert!(slot.try_acquire().is_err());
                    // SAFETY: WriteBuffer transfers these initialized items once.
                    transferred = Some(unsafe { items[0].assume_init_read() }.unwrap());
                });
                assert_eq!(transferred.unwrap(), [7, 11, 19]);
                assert!(buffer.remaining().is_empty());
            }
            1 => buffer.skip(1),
            _ => drop(buffer),
        }
        assert!(slot.try_acquire().is_ok());
    }
}

#[test]
async fn owner_reservation_survives_producer_drop_until_committed_publication() {
    let (producer, consumer, _) = pair(4);
    producer.configure_live();
    producer.writer().set_identity(ToolAttachmentId {
        creation_start: golem_common::model::OplogIndex::from_u64(3),
        slot: golem_common::model::oplog::attachment::ToolAttachmentSlot::Output,
    });
    let controller = producer.controller();
    controller.reserve_owner_terminal(ByteStreamFailure::ResourceExhausted);
    drop(producer);
    let read = consumer.read_next();
    tokio::pin!(read);
    assert!(futures::poll!(&mut read).is_pending());
    let (terminal, publication) = controller
        .accept_owner_terminal(ByteStreamFailure::Cancelled, |terminal, publication| {
            (terminal, publication)
        })
        .unwrap();
    assert_eq!(
        terminal,
        golem_common::model::oplog::attachment::ToolAttachmentTerminal::HostResourceExhausted
    );
    assert!(futures::poll!(&mut read).is_pending());
    assert!(
        controller
            .accept_owner_terminal(ByteStreamFailure::Cancelled, |_, _| ())
            .is_none()
    );
    publication.publish(golem_common::model::OplogIndex::from_u64(9));
    assert!(matches!(
        read.await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
}

#[test]
async fn owner_terminal_waits_for_durable_producer_restoration() {
    let (producer, _consumer, _) = pair(4);
    let controller = producer.controller();
    let end = golem_common::model::OplogIndex::from_u64(9);
    let restored = controller.await_terminal_publication(Some(end));
    tokio::pin!(restored);
    assert!(futures::poll!(&mut restored).is_pending());
    producer.cancel().unwrap();
    assert!(futures::poll!(&mut restored).is_pending());
    controller
        .restore_terminal(
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
            true,
            end,
            true,
        )
        .unwrap();
    assert!(futures::poll!(&mut restored).is_pending());
    controller
        .restore_terminal(
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
            true,
            end,
            false,
        )
        .unwrap();
    restored.await.unwrap();
    assert!(
        controller
            .accept_owner_terminal(ByteStreamFailure::Cancelled, |_, _| panic!(
                "durable producer selection must not append another control"
            ))
            .is_none()
    );
}

#[derive(Default)]
struct WakeCount(AtomicU64);

impl std::task::Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
#[test_r::timeout("30s")]
async fn staged_historical_writes_keep_queue_and_offered_allocations_owned() {
    let memory = AttachmentMemory::with_test_reservation(false, |bytes| async move {
        Some(MemoryGrant::inert(bytes))
    });
    let (producer, consumer, observer) = attachment_pair(8, memory);
    producer.configure_live();
    let writer = producer.writer();
    let first = writer
        .stage_historical_write(
            AttachmentWriteBody::Inline(b"abcdefgh".to_vec()),
            AcceptedAttachmentWriteEnd(golem_common::model::OplogIndex::from_u64(3)),
        )
        .unwrap();
    assert_eq!(observer.attachment.metadata().charged_bytes, 8);
    first.publish().unwrap();
    let mut source = consumer.into_stream_producer();
    let offered = std::future::poll_fn(|cx| source.poll_read(cx))
        .await
        .unwrap();
    assert!(
        matches!(offered, AttachmentStreamRead::Chunk { ref bytes, .. } if bytes == b"abcdefgh")
    );
    let second = writer
        .stage_historical_write(
            AttachmentWriteBody::Inline(b"QRSTUVWX".to_vec()),
            AcceptedAttachmentWriteEnd(golem_common::model::OplogIndex::from_u64(5)),
        )
        .unwrap();
    assert_eq!(observer.attachment.metadata().charged_bytes, 16);
    assert!(
        writer
            .stage_historical_write(
                AttachmentWriteBody::Inline(vec![99]),
                AcceptedAttachmentWriteEnd(golem_common::model::OplogIndex::from_u64(7)),
            )
            .is_err()
    );
    second.publish().unwrap();
    drop(offered);
    let next = std::future::poll_fn(|cx| source.poll_read(cx))
        .await
        .unwrap();
    assert!(matches!(next, AttachmentStreamRead::Chunk { ref bytes, .. } if bytes == b"QRSTUVWX"));
    assert_eq!(observer.attachment.metadata().charged_bytes, 8);
    drop(next);
    drop(source);
    assert_eq!(observer.attachment.metadata().charged_bytes, 0);
}

#[test]
#[test_r::timeout("30s")]
async fn recorded_historical_body_is_lazy_and_fenced_during_hydration() {
    for stalled_admission in [false, true] {
        for discard in [false, true] {
            let memory = AttachmentMemory::with_test_reservation(true, move |bytes| async move {
                if stalled_admission {
                    std::future::pending::<()>().await;
                }
                Some(MemoryGrant::inert(bytes))
            });
            let (producer, consumer, observer) = attachment_pair(8, memory);
            producer.configure_live();
            let (send, receive) = tokio::sync::oneshot::channel::<Vec<u8>>();
            let staged = producer
                .writer()
                .stage_historical_write(
                    AttachmentWriteBody::Recorded {
                        length: 3,
                        hydrate: Box::pin(async move { Ok(receive.await?) }),
                    },
                    AcceptedAttachmentWriteEnd(golem_common::model::OplogIndex::from_u64(3)),
                )
                .unwrap();
            let mut source = consumer.into_stream_producer();
            let wakes = Arc::new(WakeCount::default());
            let waker = Waker::from(wakes.clone());
            let mut cx = Context::from_waker(&waker);
            assert!(source.poll_read(&mut cx).is_pending());
            assert_eq!(observer.attachment.metadata().buffered_bytes, 3);
            staged.publish().unwrap();
            assert!(source.poll_read(&mut cx).is_pending());
            assert_eq!(observer.attachment.metadata().buffered_bytes, 0);
            wakes.0.store(0, Ordering::SeqCst);
            let controller = producer.writer().controller();
            if discard {
                controller.restore_control(
                    &golem_common::model::oplog::attachment::ToolAttachmentControl::ReaderDiscarded,
                    golem_common::model::OplogIndex::from_u64(5),
                    false,
                ).unwrap();
            } else {
                controller.fence_owner();
            }
            assert!(wakes.0.load(Ordering::SeqCst) > 0);
            let result = source.poll_read(&mut cx);
            if discard {
                assert!(matches!(
                    result,
                    Poll::Ready(Ok(AttachmentStreamRead::Terminal {
                        result: StreamResult::Cancelled,
                        ..
                    }))
                ));
            } else {
                assert!(matches!(result, Poll::Ready(Err(_))));
            }
            assert_eq!(observer.attachment.metadata().charged_bytes, 0);
            assert!(send.send(b"abc".to_vec()).is_err());
        }
    }
}

#[test]
fn dropped_unpublished_historical_head_wakes_reader_for_published_successor() {
    let memory = AttachmentMemory::with_test_reservation(false, |bytes| async move {
        Some(MemoryGrant::inert(bytes))
    });
    let (producer, consumer, observer) = attachment_pair(8, memory);
    producer.configure_live();
    let writer = producer.writer();
    let first = writer
        .stage_historical_write(
            AttachmentWriteBody::Inline(b"abc".to_vec()),
            AcceptedAttachmentWriteEnd(golem_common::model::OplogIndex::from_u64(3)),
        )
        .unwrap();
    writer
        .stage_historical_write(
            AttachmentWriteBody::Inline(b"WXYZ".to_vec()),
            AcceptedAttachmentWriteEnd(golem_common::model::OplogIndex::from_u64(5)),
        )
        .unwrap()
        .publish()
        .unwrap();
    let mut source = consumer.into_stream_producer();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(source.poll_read(&mut cx).is_pending());
    drop(first);
    assert!(wakes.0.load(Ordering::SeqCst) > 0);
    assert_eq!(observer.attachment.metadata().charged_bytes, 4);
    assert!(
        matches!(source.poll_read(&mut cx), Poll::Ready(Ok(AttachmentStreamRead::Chunk { bytes, .. })) if bytes == b"WXYZ")
    );
    drop(source);
    assert_eq!(observer.attachment.metadata().charged_bytes, 0);
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_terminal_restores_recorded_selection_and_original_end() {
    let (producer, consumer, observer) = pair(4);
    producer.configure_live();
    producer.cancel().unwrap();
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
    ));
    producer
        .controller()
        .restore_terminal(
            ByteStreamCloseCause::Failed(ByteStreamFailure::Failed("recorded".to_string())),
            false,
            golem_common::model::OplogIndex::from_u64(9),
            false,
        )
        .unwrap();
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Failed(reason)) if reason == "recorded"
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason))) if reason == "recorded"
    ));
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
    assert_eq!(
        observer.attachment.state.lock().unwrap().terminal_end,
        Some(golem_common::model::OplogIndex::from_u64(9))
    );
    assert!(
        producer
            .controller()
            .restore_terminal(
                ByteStreamCloseCause::Finished,
                false,
                golem_common::model::OplogIndex::from_u64(10),
                false,
            )
            .is_err()
    );
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_terminal_parks_typed_and_raw_readers_until_publication() {
    let (typed, typed_consumer, _) = pair(4);
    let (raw, raw_consumer, _) = pair(4);
    typed.configure_live();
    raw.configure_live();
    let typed_publication = typed
        .writer()
        .prepare_terminal(ByteStreamCloseCause::Finished)
        .unwrap()
        .ok()
        .unwrap()
        .accept(std::convert::identity)
        .unwrap()
        .unwrap();
    let raw_publication = raw
        .writer()
        .prepare_terminal(ByteStreamCloseCause::Finished)
        .unwrap()
        .ok()
        .unwrap()
        .accept(std::convert::identity)
        .unwrap()
        .unwrap();
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let typed_reader =
        StreamReader::new(&mut store, typed_consumer.into_stream_producer()).unwrap();
    let raw_reader =
        StreamReader::new(&mut store, raw_consumer.into_raw_stream_producer()).unwrap();
    let (items, mut typed_received) = mpsc::unbounded_channel();
    let (chunks, mut raw_received) = mpsc::unbounded_channel();
    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            accessor.with(|mut store| typed_reader.pipe(&mut store, CollectConsumer { items }))?;
            accessor
                .with(|mut store| raw_reader.pipe(&mut store, CollectRawConsumer { chunks }))?;
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), typed_received.recv())
                    .await
                    .is_err()
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), raw_received.recv())
                    .await
                    .is_err()
            );
            typed_publication.publish(golem_common::model::OplogIndex::from_u64(11));
            raw_publication.publish(golem_common::model::OplogIndex::from_u64(12));
            assert!(typed_received.recv().await.is_none());
            assert!(raw_received.recv().await.is_none());
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_terminal_hides_publication_and_retains_dropped_acknowledgment() {
    let (producer, consumer, observer) = pair(4);
    producer.configure_live();
    let writer = producer.writer();
    let publication = writer
        .prepare_terminal(ByteStreamCloseCause::Finished)
        .unwrap()
        .ok()
        .unwrap()
        .accept(std::convert::identity)
        .unwrap()
        .unwrap();
    assert!(observer.terminal_snapshot().is_none());
    assert!(matches!(
        writer.finish(),
        Err(StreamWriteError::ConcurrentOperation)
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), consumer.read_next())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            observer.wait_terminal()
        )
        .await
        .is_err()
    );
    let (commit, committed) = tokio::sync::oneshot::channel();
    let (ack, acknowledgment) = tokio::sync::oneshot::channel();
    let continuation = tokio::spawn(async move {
        committed.await.unwrap();
        publication.publish(golem_common::model::OplogIndex::from_u64(42));
        let _ = ack.send(());
    });
    drop(acknowledgment);
    drop(writer);
    drop(producer);
    assert!(observer.terminal().is_none());
    commit.send(()).unwrap();
    continuation.await.unwrap();
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Finished
    ));
    assert_eq!(
        observer.attachment.state.lock().unwrap().terminal_end,
        Some(golem_common::model::OplogIndex::from_u64(42))
    );
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_terminal_repeated_sanitized_transition_and_selection_race() {
    let (producer, _consumer, observer) = pair(4);
    let writer = producer.writer();
    let cause = AttachmentWriter::effective_terminal(Some(ByteStreamFailure::Failed("a\n".into())));
    let publication = writer
        .prepare_terminal(cause.clone())
        .unwrap()
        .ok()
        .unwrap()
        .accept(std::convert::identity)
        .unwrap()
        .unwrap();
    publication.publish(golem_common::model::OplogIndex::from_u64(7));
    assert!(matches!(writer.prepare_terminal(cause), Ok(Err(()))));
    writer
        .fail(ByteStreamFailure::Failed("different private detail".into()))
        .unwrap();
    assert!(matches!(writer.finish(),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(ByteStreamFailure::Failed(message))))
            if message == "stream producer failed"));
    assert_eq!(
        observer.attachment.state.lock().unwrap().terminal_end,
        Some(golem_common::model::OplogIndex::from_u64(7))
    );

    let (producer, _consumer, _) = pair(4);
    let prepared = producer
        .writer()
        .prepare_terminal(ByteStreamCloseCause::Finished)
        .unwrap()
        .ok()
        .unwrap();
    producer.cancel().unwrap();
    assert!(matches!(
        prepared.accept(|_| panic!("must not enqueue")),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )))
    ));
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_terminal_failure_fences_without_publishing_selected_cause() {
    for exhaustion in [false, true] {
        let (producer, consumer, observer) = pair(4);
        producer.configure_live();
        let writer = producer.writer();
        let prepared = if exhaustion {
            let mut write = writer.prepare_write(vec![1; 5]).await.unwrap().unwrap();
            let AttachmentWriteAcceptance::ResourceExhausted(prepared) =
                write.accept(std::convert::identity).unwrap()
            else {
                panic!("oversized write must prepare exhaustion")
            };
            prepared
        } else {
            writer
                .prepare_terminal(ByteStreamCloseCause::Finished)
                .unwrap()
                .ok()
                .unwrap()
        };
        let publication = prepared.accept(std::convert::identity).unwrap().unwrap();
        assert!(observer.terminal_snapshot().is_none());
        producer.controller().fence_owner();
        publication.publish(golem_common::model::OplogIndex::from_u64(42));
        assert!(matches!(
            observer.wait_terminal().await,
            ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
        ));
        assert!(
            !observer
                .terminal_snapshot()
                .unwrap()
                .host_resource_exhausted
        );
        assert!(matches!(
            consumer.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
        ));
        assert_eq!(observer.attachment.state.lock().unwrap().terminal_end, None);
    }
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_write_hides_terminal_and_retains_cancelled_prefix() {
    let (producer, consumer, _) = pair(4);
    producer.configure_live();
    let mut prepared = producer
        .writer()
        .prepare_write(vec![1, 2, 3, 4])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(producer.controller().metadata().accepted_bytes, 0);
    assert_eq!(producer.attachment.activity().charged_bytes, 4);
    let mut enqueued = false;
    let AttachmentWriteAcceptance::Accepted(publication) = prepared
        .accept(|publication| {
            assert!(producer.attachment.state.try_lock().is_err());
            enqueued = true;
            publication
        })
        .unwrap()
    else {
        panic!("write not accepted")
    };
    assert!(enqueued);
    assert_eq!(producer.controller().metadata().buffered_bytes, 4);
    assert!(matches!(
        producer.write(vec![9]).await,
        Err(StreamWriteError::ConcurrentOperation)
    ));
    producer.cancel().unwrap();
    assert_eq!(producer.attachment.activity().charged_bytes, 4);
    let mut read = Box::pin(consumer.read_next());
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(read.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    publication.publish(None);
    assert!(matches!(read.await, AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]));
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
    ));
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_write_discard_fence_and_failed_publication_release_memory() {
    for action in 0..3 {
        let (producer, consumer, _) = pair(4);
        producer.configure_live();
        let mut prepared = producer
            .writer()
            .prepare_write(vec![1; 4])
            .await
            .unwrap()
            .unwrap();
        let AttachmentWriteAcceptance::Accepted(publication) =
            prepared.accept(std::convert::identity).unwrap()
        else {
            panic!("write not accepted")
        };
        match action {
            0 => {
                producer.cancel().unwrap();
                drop(consumer);
                publication.publish(None);
            }
            1 => {
                producer.controller().fence_owner();
                publication.publish(None);
            }
            _ => drop(publication),
        }
        assert_eq!(producer.attachment.activity().charged_bytes, 0);
        assert_eq!(producer.controller().metadata().buffered_bytes, 0);
        assert_eq!(producer.controller().metadata().accepted_bytes, 4);
        if action == 2 {
            assert!(!producer.controller().metadata().terminal_selected);
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_write_revalidates_capacity_and_terminal() {
    let (producer, consumer, _) = pair(4);
    producer.configure_live();
    producer.write(vec![1; 4]).await.unwrap();
    let mut prepared = producer
        .writer()
        .prepare_write(vec![2; 4])
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        prepared.accept(|_| panic!("must not enqueue")),
        Ok(AttachmentWriteAcceptance::Retry)
    ));
    assert_eq!(producer.controller().metadata().accepted_bytes, 4);
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(_))
    ));
    prepared.wait_ready().await;
    producer.cancel().unwrap();
    assert!(matches!(
        prepared.accept(|_| panic!("must not enqueue")),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )))
    ));
    drop(prepared);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_write_parks_typed_poll_before_cancellation() {
    let (producer, consumer, _) = pair(4);
    producer.configure_live();
    let mut prepared = producer
        .writer()
        .prepare_write(vec![1; 4])
        .await
        .unwrap()
        .unwrap();
    let AttachmentWriteAcceptance::Accepted(publication) =
        prepared.accept(std::convert::identity).unwrap()
    else {
        panic!("write not accepted")
    };
    producer.cancel().unwrap();
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let reader = StreamReader::new(&mut store, consumer.into_stream_producer()).unwrap();
    let (items, mut received) = mpsc::unbounded_channel();
    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            accessor.with(|mut store| reader.pipe(&mut store, CollectConsumer { items }))?;
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), received.recv())
                    .await
                    .is_err()
            );
            publication.publish(None);
            assert!(matches!(received.recv().await, Some(Ok(bytes)) if bytes == vec![1; 4]));
            assert!(matches!(
                received.recv().await,
                Some(Err(ByteStreamFailure::Cancelled))
            ));
            assert!(received.recv().await.is_none());
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
}

#[test]
#[test_r::timeout("30s")]
async fn prepared_write_parks_raw_poll_before_eof() {
    let (producer, consumer, _) = pair(4);
    producer.configure_live();
    let mut prepared = producer
        .writer()
        .prepare_write(vec![1; 4])
        .await
        .unwrap()
        .unwrap();
    let AttachmentWriteAcceptance::Accepted(publication) =
        prepared.accept(std::convert::identity).unwrap()
    else {
        panic!("write not accepted")
    };
    producer
        .attachment
        .select_terminal(ByteStreamCloseCause::Finished)
        .unwrap();
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
    let (chunks, mut received) = mpsc::unbounded_channel();
    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            accessor.with(|mut store| reader.pipe(&mut store, CollectRawConsumer { chunks }))?;
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), received.recv())
                    .await
                    .is_err()
            );
            publication.publish(None);
            assert_eq!(received.recv().await, Some(vec![1; 4]));
            assert!(received.recv().await.is_none());
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
}

fn rejecting_memory() -> AttachmentMemory {
    AttachmentMemory {
        reserve_tracked: Arc::new(|_| Box::pin(async { None })),
        tracking_enabled: Arc::new(AtomicBool::new(true)),
        tracking_pending: Arc::new(AtomicBool::new(false)),
        prepared_grant: Arc::new(Mutex::new(None)),
        historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
        live_activation: Arc::new(tokio::sync::Mutex::new(())),
    }
}

fn blocked_memory() -> AttachmentMemory {
    AttachmentMemory {
        reserve_tracked: Arc::new(|_| Box::pin(std::future::pending())),
        tracking_enabled: Arc::new(AtomicBool::new(true)),
        tracking_pending: Arc::new(AtomicBool::new(false)),
        prepared_grant: Arc::new(Mutex::new(None)),
        historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
        live_activation: Arc::new(tokio::sync::Mutex::new(())),
    }
}

async fn wait_for_producer_operation(producer: &AttachmentProducer) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !producer
            .attachment
            .producer_operation
            .load(Ordering::Acquire)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_charged_bytes(producer: &AttachmentProducer, expected: u64) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while producer.attachment.activity().charged_bytes != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

struct CollectConsumer {
    items: mpsc::UnboundedSender<Result<Vec<u8>, ByteStreamFailure>>,
}

impl<D> StreamConsumer<D> for CollectConsumer {
    type Item = Result<Vec<u8>, ByteStreamFailure>;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'_, D>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let count = source.remaining(store.as_context_mut());
        let mut received = Vec::with_capacity(count);
        source.read(store.as_context_mut(), &mut received)?;
        for item in received {
            let _ = self.items.send(item);
        }
        Poll::Ready(Ok(if finish {
            StreamResult::Dropped
        } else {
            StreamResult::Completed
        }))
    }
}

struct CollectRawConsumer {
    chunks: mpsc::UnboundedSender<Vec<u8>>,
}

impl<D> StreamConsumer<D> for CollectRawConsumer {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut source = source.as_direct(store.as_context_mut());
        let remaining = source.remaining();
        let count = remaining.len();
        if !remaining.is_empty() {
            let _ = self.chunks.send(remaining.to_vec());
        }
        source.mark_read(count);
        Poll::Ready(Ok(if finish {
            StreamResult::Dropped
        } else {
            StreamResult::Completed
        }))
    }
}

struct UpgradeInFlightRawConsumer {
    controller: AttachmentController,
    upgraded: bool,
    completed: mpsc::UnboundedSender<()>,
}

impl<D> StreamConsumer<D> for UpgradeInFlightRawConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut source = source.as_direct(store.as_context_mut());
        let count = source.remaining().len();
        if count != 0 && !self.upgraded {
            assert!(futures::executor::block_on(
                self.controller.prepare_live_memory_accounting()
            ));
            self.controller.commit_live_memory_accounting();
            self.upgraded = true;
        }
        source.mark_read(count);
        if finish {
            let _ = self.completed.send(());
        }
        Poll::Ready(Ok(if finish {
            StreamResult::Dropped
        } else {
            StreamResult::Completed
        }))
    }
}

async fn drain_with_store(
    store: &mut Store<()>,
    consumer: AttachmentConsumer,
) -> Vec<Result<Vec<u8>, ByteStreamFailure>> {
    let (items, mut received) = mpsc::unbounded_channel();
    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<_>> {
            accessor.with(|mut store| {
                StreamReader::new(&mut store, consumer.into_stream_producer())?
                    .pipe(&mut store, CollectConsumer { items })
            })?;
            let mut result = Vec::new();
            while let Some(item) = received.recv().await {
                result.push(item);
            }
            Ok(result)
        })
        .await
        .unwrap()
        .unwrap()
}

#[test]
async fn live_stream_preserves_order_and_terminal() {
    let (producer, consumer, observer) = pair(8);
    assert!(producer.configure_live());
    producer.write(vec![0, 1, 2]).await.unwrap();
    producer.write(vec![3, 4]).await.unwrap();
    producer.finish().unwrap();
    producer.finish().unwrap();
    assert!(matches!(
        producer.fail(ByteStreamFailure::Cancelled),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Finished))
    ));

    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![0, 1, 2]
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![3, 4]
    ));
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Finished
    ));
}

#[test]
async fn empty_successful_writes_never_become_stream_items() {
    let (producer, consumer, _) = pair(8);
    assert!(producer.configure_live());
    producer.write(Vec::new()).await.unwrap();
    producer.write(vec![42]).await.unwrap();
    producer.finish().unwrap();

    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![42]
    ));
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
}

#[test]
async fn blocked_live_write_wakes_after_consumer_progress() {
    let (producer, consumer, _) = pair(4);
    assert!(producer.configure_live());
    producer.write(vec![1, 2, 3, 4]).await.unwrap();

    let writer = producer.writer();
    let blocked = tokio::spawn(async move { writer.write(vec![5, 6]).await });
    wait_for_producer_operation(&producer).await;
    wait_for_charged_bytes(&producer, 6).await;
    assert!(!blocked.is_finished());
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]
    ));
    blocked.await.unwrap().unwrap();
    producer.finish().unwrap();
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![5, 6]
    ));
}

#[test]
async fn completion_attachment_accepts_exact_limit_and_rejects_crossing_chunk_atomically() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_completion());
    producer.write(vec![1, 2, 3, 4]).await.unwrap();
    let error = producer.write(vec![5]).await.unwrap_err();
    assert!(matches!(
        error,
        StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::ResourceExhausted
        ))
    ));
    assert_eq!(producer.attachment.activity().buffered_bytes, 4);
    assert!(producer.publish_completion());
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
    ));
    let snapshot = observer.terminal_snapshot().unwrap();
    assert!(snapshot.host_resource_exhausted);
    assert!(matches!(
        snapshot.cause,
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
    ));
}

#[test]
async fn completion_reader_remains_silent_until_terminal_publication() {
    let (producer, consumer, _) = pair(16);
    assert!(producer.configure_completion());
    producer.write(vec![1, 2, 3]).await.unwrap();

    let mut read = tokio::spawn(async move { consumer.read_next().await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
            .await
            .is_err()
    );
    producer.finish().unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
            .await
            .is_err()
    );
    assert!(producer.publish_completion());
    assert!(matches!(
        read.await.unwrap(),
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
    ));
}

#[test]
async fn no_body_terminal_publication_handles_every_attachment_mode() {
    let (replaying, replaying_consumer, _) = pair(16);
    let controller = replaying.controller();
    controller.prepare_no_body_terminal();
    replaying.retire();
    let read = replaying_consumer.read_next();
    tokio::pin!(read);
    assert!(futures::poll!(&mut read).is_pending());
    let terminal = golem_common::model::OplogIndex::from_u64(13);
    controller
        .restore_terminal(
            ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned),
            false,
            terminal,
            true,
        )
        .unwrap();
    assert!(futures::poll!(&mut read).is_pending());
    controller
        .restore_terminal(
            ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned),
            false,
            terminal,
            false,
        )
        .unwrap();
    assert!(matches!(
        read.await,
        AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
    ));

    let (pending, pending_consumer, _) = pair(16);
    let pending_controller = pending.controller();
    pending.write(vec![1, 2, 3]).await.unwrap();
    let mut pending_read = tokio::spawn(async move {
        (
            pending_consumer.read_next().await,
            pending_consumer.read_next().await,
            pending_consumer.read_next().await,
        )
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending_read)
            .await
            .is_err()
    );
    pending_controller.cancel().unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending_read)
            .await
            .is_err()
    );
    assert!(pending_controller.publish_no_body_terminal());
    assert_eq!(
        pending_controller.metadata().mode,
        ToolAttachmentModeMetadata::TerminalOnly
    );
    assert_eq!(pending_controller.metadata().buffered_bytes, 0);
    assert_eq!(pending_controller.metadata().charged_bytes, 0);
    assert!(pending_controller.publish_no_body_terminal());
    let (failure, end, repeated_end) = pending_read.await.unwrap();
    assert!(matches!(
        failure,
        AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
    ));
    assert!(matches!(end, AttachmentRead::End));
    assert!(matches!(repeated_end, AttachmentRead::End));

    let (completion, completion_consumer, _) = pair(16);
    let completion_controller = completion.controller();
    assert!(completion_controller.configure_completion());
    completion.write(vec![4, 5]).await.unwrap();
    completion_controller.cancel().unwrap();
    assert!(completion_controller.publish_no_body_terminal());
    assert_eq!(
        completion_controller.metadata().mode,
        ToolAttachmentModeMetadata::CompletionPublished
    );
    assert!(completion_controller.publish_no_body_terminal());
    assert!(matches!(
        completion_consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![4, 5]
    ));
    assert!(matches!(
        completion_consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
    ));
    assert!(matches!(
        completion_consumer.read_next().await,
        AttachmentRead::End
    ));

    let (live, live_consumer, _) = pair(16);
    let live_controller = live.controller();
    assert!(live_controller.configure_live());
    live.write(vec![6, 7]).await.unwrap();
    live_controller.cancel().unwrap();
    assert!(live_controller.publish_no_body_terminal());
    assert_eq!(
        live_controller.metadata().mode,
        ToolAttachmentModeMetadata::Live
    );
    assert!(matches!(
        live_consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![6, 7]
    ));
    assert!(matches!(
        live_consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
    ));
    assert!(matches!(
        live_consumer.read_next().await,
        AttachmentRead::End
    ));

    let discard = discard_producer(AttachmentMemory::inert());
    let discard_controller = discard.controller();
    discard_controller.cancel().unwrap();
    assert!(discard_controller.publish_no_body_terminal());
    assert_eq!(
        discard_controller.metadata().mode,
        ToolAttachmentModeMetadata::Discard
    );

    let (unterminated, _consumer, _) = pair(16);
    assert!(!unterminated.controller().publish_no_body_terminal());
}

#[test]
async fn no_body_terminal_publication_wakes_a_wasmtime_reader() {
    let (producer, consumer, _) = pair(16);
    let controller = producer.controller();
    assert!(controller.configure_completion());
    controller.cancel().unwrap();

    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let drain = drain_with_store(&mut store, consumer);
    tokio::pin!(drain);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut drain)
            .await
            .is_err()
    );

    assert!(controller.publish_no_body_terminal());
    let items = tokio::time::timeout(std::time::Duration::from_secs(1), &mut drain)
        .await
        .unwrap();
    assert!(matches!(
        items.as_slice(),
        [Err(ByteStreamFailure::Cancelled)]
    ));
}

#[test]
async fn unconfigured_stdout_target_drop_publishes_only_abandonment() {
    let (producer, consumer, _) = pair(4);
    let target = ToolOutputEntry {
        producer: Some(producer),
        completion_only: false,
    };
    let mut read =
        tokio::spawn(async move { (consumer.read_next().await, consumer.read_next().await) });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
            .await
            .is_err()
    );

    drop(target);

    let (failure, end) = read.await.unwrap();
    assert!(matches!(
        failure,
        AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
    ));
    assert!(matches!(end, AttachmentRead::End));
}

#[test]
async fn unconfigured_stdout_rejection_publishes_bounded_failure_context() {
    let (producer, consumer, _) = pair(4);
    let target = ToolOutputEntry {
        producer: Some(producer),
        completion_only: false,
    };
    target
        .producer
        .as_ref()
        .unwrap()
        .reject_unconfigured()
        .unwrap();
    drop(target);

    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason)))
            if reason == "tool invocation rejected"
    ));
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
}

#[test]
async fn endpoint_drop_causes_are_directional_and_wake_waiters() {
    let (producer, consumer, observer) = pair(4);
    assert!(consumer.configure_live());
    drop(producer);
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)
    ));

    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    drop(consumer);
    assert!(matches!(
        producer.write(vec![1]).await,
        Err(StreamWriteError::Closed(
            ByteStreamCloseCause::ConsumerCancelled
        ))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::ConsumerCancelled
    ));
}

#[test]
async fn endpoint_role_drop_order_freezes_the_first_terminal() {
    let (producer, consumer, observer) = pair(4);
    drop(producer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));
    drop(consumer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));

    let (producer, consumer, observer) = pair(4);
    drop(consumer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));
    drop(producer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));

    let (producer, reader, observer) = pair(4);
    let target = ToolOutputEntry {
        producer: Some(producer),
        completion_only: false,
    };
    drop(target);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));
    drop(reader);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));

    let (producer, reader, observer) = pair(4);
    let target = ToolOutputEntry {
        producer: Some(producer),
        completion_only: false,
    };
    drop(reader);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));
    drop(target);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));

    let (producer, reader, observer) = pair(4);
    let writer: ToolOutputWriterEntry = ToolOutputEntry {
        producer: Some(producer),
        completion_only: false,
    }
    .into_writer();
    drop(writer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));
    drop(reader);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));

    let (producer, reader, observer) = pair(4);
    let writer: ToolOutputWriterEntry = ToolOutputEntry {
        producer: Some(producer),
        completion_only: false,
    }
    .into_writer();
    drop(reader);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));
    drop(writer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));

    let (producer, consumer, observer) = pair(4);
    let writer = ToolStdinWriterEntry { producer };
    let source = ToolStdinEntry { consumer };
    drop(writer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));
    drop(source);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    ));

    let (producer, consumer, observer) = pair(4);
    let writer = ToolStdinWriterEntry { producer };
    let source = ToolStdinEntry { consumer };
    drop(source);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));
    drop(writer);
    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::ConsumerCancelled)
    ));
}

#[test]
async fn blocked_live_read_wakes_on_source_abandonment() {
    let (producer, consumer, observer) = pair(4);
    assert!(consumer.configure_live());
    let mut read = tokio::spawn(async move { consumer.read_next().await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut read)
            .await
            .is_err()
    );

    drop(producer);

    assert!(matches!(
        read.await.unwrap(),
        AttachmentRead::Item(Err(ByteStreamFailure::Abandoned))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)
    ));
}

#[test]
async fn discard_output_never_backpressures_and_retains_terminal() {
    let producer = discard_producer(AttachmentMemory::inert());
    let observer = producer.observer();
    producer.write(vec![0; 1024]).await.unwrap();
    producer.finish().unwrap();
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Finished
    ));
}

#[test]
async fn owner_fenced_wasmtime_reader_traps_without_delivering_cancellation() {
    let (producer, consumer, _) = pair(4);
    let controller = consumer.controller();
    producer.write(vec![1, 2, 3, 4]).await.unwrap();
    controller.fence_owner();
    assert!(controller.metadata().owner_fenced);
    assert_eq!(
        controller.metadata().mode,
        ToolAttachmentModeMetadata::Pending
    );
    assert_eq!(controller.metadata().buffered_bytes, 0);
    assert_eq!(controller.metadata().charged_bytes, 0);
    assert!(!controller.publish_no_body_terminal());

    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let (items, mut received) = mpsc::unbounded_channel();
    let result = store
        .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<_>> {
            accessor.with(|mut store| {
                StreamReader::new(&mut store, consumer.into_stream_producer())?
                    .pipe(&mut store, CollectConsumer { items })
            })?;
            let mut result = Vec::new();
            while let Some(item) = received.recv().await {
                result.push(item);
            }
            Ok(result)
        })
        .await;
    let error = result.expect_err("owner-fenced stream must trap");
    assert!(error.to_string().contains("owner generation fenced"));
}

#[test]
async fn closing_wasmtime_reader_cancels_the_consumer_and_cleans_transmit_state() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let mut reader = StreamReader::new(&mut store, consumer.into_stream_producer()).unwrap();
    reader.close(store.as_context_mut()).unwrap();

    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::ConsumerCancelled
    ));
    assert!(matches!(
        producer.write(vec![1]).await,
        Err(StreamWriteError::Closed(
            ByteStreamCloseCause::ConsumerCancelled
        ))
    ));
}

#[test]
async fn cancelling_raw_wasmtime_read_operation_preserves_and_resumes_the_attachment() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    let mut config = Config::new();
    config.concurrency_support(true);
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(
        &engine,
        r#"
(component
  (import "cancelled" (func $cancelled))
  (core func $cancelled (canon lower (func $cancelled)))
  (core module $memory (memory (export "mem") 1))
  (core instance $memory (instantiate $memory))
  (core module $core
    (import "" "mem" (memory 1))
    (import "" "stream.read-async" (func $stream.read-async (param i32 i32 i32) (result i32)))
    (import "" "stream.read-sync" (func $stream.read-sync (param i32 i32 i32) (result i32)))
    (import "" "stream.cancel-read" (func $stream.cancel-read (param i32) (result i32)))
    (import "" "stream.drop-readable" (func $stream.drop-readable (param i32)))
    (import "" "cancelled" (func $cancelled))
    (func (export "run") (param $stream i32)
      (local $result i32)
      (local.set $result (call $stream.read-async (local.get $stream) (i32.const 8) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (local.set $result (call $stream.cancel-read (local.get $stream)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
      (call $cancelled)
      (local.set $result (call $stream.read-sync (local.get $stream) (i32.const 8) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const 48)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 8)) (i32.const 4)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 9)) (i32.const 5)) (then unreachable))
      (if (i32.ne (i32.load8_u (i32.const 10)) (i32.const 6)) (then unreachable))
      (call $stream.drop-readable (local.get $stream))
    )
  )
  (type $stream (stream u8))
  (core func $stream.read-async (canon stream.read $stream async (memory (core memory $memory "mem"))))
  (core func $stream.read-sync (canon stream.read $stream (memory (core memory $memory "mem"))))
  (core func $stream.cancel-read (canon stream.cancel-read $stream))
  (core func $stream.drop-readable (canon stream.drop-readable $stream))
  (core instance $core (instantiate $core (with "" (instance
    (export "mem" (memory $memory "mem"))
    (export "stream.read-async" (func $stream.read-async))
    (export "stream.read-sync" (func $stream.read-sync))
    (export "stream.cancel-read" (func $stream.cancel-read))
    (export "stream.drop-readable" (func $stream.drop-readable))
    (export "cancelled" (func $cancelled))
  ))))
  (func (export "run") async (param "stream" (stream u8))
    (canon lift (core func $core "run") (memory (core memory $memory "mem"))))
)
            "#,
    )
    .unwrap();
    let (cancelled, mut cancellation) = mpsc::unbounded_channel();
    let mut linker = Linker::new(&engine);
    linker
        .root()
        .func_wrap("cancelled", move |_, (): ()| {
            cancelled.send(()).unwrap();
            Ok(())
        })
        .unwrap();
    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let run = instance
        .get_typed_func::<(StreamReader<u8>,), ()>(&mut store, "run")
        .unwrap();
    let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
    let observer_during_cancellation = observer.clone();

    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            tokio::try_join!(
                async {
                    run.call_concurrent(accessor, (reader,)).await?;
                    wasmtime::error::Ok(())
                },
                async {
                    cancellation.recv().await.unwrap();
                    assert!(observer_during_cancellation.terminal().is_none());
                    producer.write(vec![4, 5, 6]).await.unwrap();
                    producer.finish().unwrap();
                    wasmtime::error::Ok(())
                }
            )?;
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();

    assert!(matches!(
        observer.terminal(),
        Some(ByteStreamCloseCause::Finished)
    ));
}

#[test]
async fn closing_raw_wasmtime_reader_still_cancels_the_consumer() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let mut reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
    reader.close(store.as_context_mut()).unwrap();

    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::ConsumerCancelled
    ));
    assert!(matches!(
        producer.write(vec![1]).await,
        Err(StreamWriteError::Closed(
            ByteStreamCloseCause::ConsumerCancelled
        ))
    ));
}

#[test]
#[test_r::timeout("30s")]
async fn native_raw_read_observer_feasibility() {
    use wasmtime::component::TerminalConsumption;

    struct ObservedProducer {
        inner: RawAttachmentStreamProducer,
        registered: bool,
        defer: bool,
        events: Arc<Mutex<Vec<(usize, TerminalConsumption)>>>,
        polls: Arc<Mutex<Vec<(usize, bool)>>>,
        ready: Arc<AtomicBool>,
    }

    impl StreamProducer<()> for ObservedProducer {
        type Item = u8;
        type Buffer = Bytes;

        fn poll_produce<'a>(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            mut store: StoreContextMut<'a, ()>,
            mut dst: Destination<'a, u8, Bytes>,
            finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            let capacity = dst.remaining(&mut store).unwrap();
            self.polls.lock().unwrap().push((capacity, finish));
            if !self.registered {
                let events = self.events.clone();
                dst.register_read_observer(&mut store, move |event| {
                    events.lock().unwrap().push((capacity, event));
                })?;
                self.registered = true;
            }
            if self.defer && !finish {
                self.defer = false;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let result = Pin::new(&mut self.inner).poll_produce(
                cx,
                store.as_context_mut(),
                dst.reborrow(),
                finish,
            );
            if result.is_ready() {
                self.registered = false;
                self.ready.store(true, Ordering::Release);
            }
            result
        }
    }

    let mut config = Config::new();
    config.concurrency_support(true);
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::new(&config).unwrap();
    for (name, body, defer, expected) in [
        (
            "leftover",
            "(call $report (call $read (local.get $s) (i32.const 8) (i32.const 2)) (i32.load16_u (i32.const 8))) (call $report (call $read (local.get $s) (i32.const 8) (i32.const 2)) (i32.load16_u (i32.const 8))) (call $drop (local.get $s))",
            false,
            vec![
                (2, TerminalConsumption::Delivered),
                (2, TerminalConsumption::Delivered),
            ],
        ),
        (
            "underfilled",
            "(call $report (call $read (local.get $s) (i32.const 8) (i32.const 8)) (i32.load (i32.const 8))) (call $drop (local.get $s))",
            false,
            vec![(8, TerminalConsumption::Delivered)],
        ),
        (
            "zero-cancel",
            "(call $report (call $read (local.get $s) (i32.const 8) (i32.const 2)) (i32.const 0)) (call $report (call $cancel (local.get $s)) (i32.const 0)) (call $drop (local.get $s))",
            true,
            vec![(2, TerminalConsumption::NotDelivered)],
        ),
        (
            "positive-cancel",
            "(call $report (call $read (local.get $s) (i32.const 8) (i32.const 2)) (i32.const 0)) (block $ready (loop $wait (br_if $ready (call $ready)) (drop (call $yield)) (br $wait))) (call $report (call $cancel (local.get $s)) (i32.load16_u (i32.const 8))) (call $drop (local.get $s))",
            true,
            vec![(2, TerminalConsumption::Delivered)],
        ),
        (
            "readiness",
            "(call $report (call $read (local.get $s) (i32.const 8) (i32.const 0)) (i32.const 0)) (call $drop (local.get $s))",
            false,
            vec![(0, TerminalConsumption::Delivered)],
        ),
        (
            "readiness-cancel",
            "(call $report (call $read (local.get $s) (i32.const 8) (i32.const 0)) (i32.const 0)) (block $ready (loop $wait (br_if $ready (call $ready)) (drop (call $yield)) (br $wait))) (call $report (call $cancel (local.get $s)) (i32.const 0)) (call $drop (local.get $s))",
            true,
            vec![(0, TerminalConsumption::NotDelivered)],
        ),
        (
            "pending-drop",
            "(drop (call $read (local.get $s) (i32.const 8) (i32.const 2))) (call $drop (local.get $s))",
            true,
            vec![],
        ),
        (
            "teardown",
            "(drop (call $read (local.get $s) (i32.const 8) (i32.const 2))) unreachable",
            true,
            vec![],
        ),
    ] {
        let wat = format!(
            r#"(component
              (import "report" (func $report (param "result" u32) (param "bytes" u32)))
              (import "ready" (func $ready (result u32)))
              (core func $report (canon lower (func $report)))
              (core func $ready (canon lower (func $ready)))
              (core module $memory (memory (export "mem") 1))
              (core instance $memory (instantiate $memory))
              (core module $m
                (import "" "mem" (memory 1))
                (import "" "read" (func $read (param i32 i32 i32) (result i32)))
                (import "" "cancel" (func $cancel (param i32) (result i32)))
                (import "" "drop" (func $drop (param i32)))
                (import "" "yield" (func $yield (result i32)))
                (import "" "report" (func $report (param i32 i32)))
                (import "" "ready" (func $ready (result i32)))
                (func (export "run") (param $s i32) {body}))
              (type $s (stream u8))
              (core func $read (canon stream.read $s async (memory (core memory $memory "mem"))))
              (core func $cancel (canon stream.cancel-read $s))
              (core func $drop (canon stream.drop-readable $s))
              (canon thread.yield (core func $yield))
              (core instance $m (instantiate $m (with "" (instance
                (export "mem" (memory $memory "mem"))
                (export "read" (func $read)) (export "cancel" (func $cancel))
                (export "drop" (func $drop)) (export "yield" (func $yield))
                (export "ready" (func $ready))
                (export "report" (func $report))))))
              (func (export "run") async (param "s" $s)
                (canon lift (core func $m "run") (memory (core memory $memory "mem")))))"#
        );
        let component = Component::new(&engine, wat).unwrap();
        let (producer, consumer, _) = pair(8);
        assert!(producer.configure_live());
        producer.write(vec![0x13, 0xa7, 0x42, 0xe9]).await.unwrap();
        let attachment = producer.attachment.clone();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let reported = reports.clone();
        let ready = Arc::new(AtomicBool::new(false));
        let ready_for_guest = ready.clone();
        let mut linker = Linker::new(&engine);
        linker
            .root()
            .func_wrap("ready", move |_, (): ()| {
                Ok((u32::from(ready_for_guest.load(Ordering::Acquire)),))
            })
            .unwrap();
        linker
            .root()
            .func_wrap("report", move |_, values: (u32, u32)| {
                if name == "leftover" && reported.lock().unwrap().is_empty() {
                    assert_eq!(attachment.activity().charged_bytes, 4);
                }
                reported.lock().unwrap().push(values);
                Ok(())
            })
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let polls = Arc::new(Mutex::new(Vec::new()));
        let mut store = Store::new(&engine, ());
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let reader = StreamReader::new(
            &mut store,
            ObservedProducer {
                inner: consumer.into_raw_stream_producer(),
                registered: false,
                defer,
                events: events.clone(),
                polls: polls.clone(),
                ready,
            },
        )
        .unwrap();
        let result = instance
            .get_typed_func::<(StreamReader<u8>,), ()>(&mut store, "run")
            .unwrap()
            .call_async(&mut store, (reader,))
            .await;
        if name == "teardown" || name == "pending-drop" {
            let error = result.unwrap_err();
            println!("native observer {name}: trap={error:?}");
            if name == "pending-drop" {
                assert!(format!("{error:?}").contains("cannot remove busy stream"));
            }
        } else {
            result.unwrap();
        }
        drop(store);
        assert_eq!(producer.attachment.activity().charged_bytes, 0, "{name}");
        println!(
            "native observer {name}: guest={:?} callbacks={:?} polls={:?}",
            reports.lock().unwrap(),
            events.lock().unwrap(),
            polls.lock().unwrap()
        );
        let expected_reports = match name {
            "leftover" => vec![(32, 0xa713), (32, 0xe942)],
            "underfilled" => vec![(64, 0xe942a713)],
            "zero-cancel" => vec![(u32::MAX, 0), (2, 0)],
            "positive-cancel" => vec![(u32::MAX, 0), (34, 0xa713)],
            "readiness" => vec![(0, 0)],
            "readiness-cancel" => vec![(u32::MAX, 0), (2, 0)],
            _ => vec![],
        };
        assert_eq!(*reports.lock().unwrap(), expected_reports, "{name}");
        assert_eq!(*events.lock().unwrap(), expected, "{name}");
    }
}

#[test]
async fn unread_raw_completion_stream_retains_grants_until_wasmtime_consumes_bytes() {
    let (producer, consumer, _) = pair(8);
    assert!(producer.configure_completion());
    producer.write(vec![1, 2, 3, 4]).await.unwrap();
    producer.finish().unwrap();
    assert!(producer.publish_completion());

    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
    assert_eq!(producer.attachment.activity().charged_bytes, 4);

    let (chunks, mut received) = mpsc::unbounded_channel();
    let collected = store
        .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<u8>> {
            accessor.with(|mut store| reader.pipe(&mut store, CollectRawConsumer { chunks }))?;
            let mut collected = Vec::new();
            while let Some(chunk) = received.recv().await {
                collected.extend(chunk);
            }
            Ok(collected)
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(collected, vec![1, 2, 3, 4]);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
}

#[test]
async fn wasmtime_in_flight_historical_chunk_is_included_in_live_upgrade() {
    let reservations = Arc::new(Mutex::new(Vec::new()));
    let reservations_for_memory = reservations.clone();
    let memory = AttachmentMemory::with_test_reservation(false, move |bytes| {
        reservations_for_memory.lock().unwrap().push(bytes);
        async move { Some(MemoryGrant::inert(bytes)) }
    });
    let (producer, consumer, _) = attachment_pair(8, memory);
    let controller = producer.controller();
    producer.write(vec![1, 2, 3, 4]).await.unwrap();
    assert!(producer.configure_live());
    producer.finish().unwrap();

    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
    let (completed, mut completion) = mpsc::unbounded_channel();

    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            accessor.with(|mut store| {
                reader.pipe(
                    &mut store,
                    UpgradeInFlightRawConsumer {
                        controller,
                        upgraded: false,
                        completed,
                    },
                )
            })?;
            completion.recv().await;
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(*reservations.lock().unwrap(), vec![4]);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
}

#[test]
async fn concurrent_producer_operations_are_rejected() {
    let (producer, consumer, _) = pair(2);
    assert!(producer.configure_live());
    producer.write(vec![1, 2]).await.unwrap();

    let writer = producer.writer();
    let blocked = tokio::spawn(async move { writer.write(vec![3]).await });
    wait_for_producer_operation(&producer).await;
    assert!(!blocked.is_finished());
    assert!(matches!(
        producer.finish(),
        Err(StreamWriteError::ConcurrentOperation)
    ));

    drop(consumer);
    assert!(matches!(
        blocked.await.unwrap(),
        Err(StreamWriteError::Closed(
            ByteStreamCloseCause::ConsumerCancelled
        ))
    ));
}

#[test]
async fn blocked_write_wakes_when_reader_cancels() {
    let (producer, consumer, observer) = pair(2);
    assert!(producer.configure_live());
    producer.write(vec![1, 2]).await.unwrap();

    let writer = producer.writer();
    let blocked = tokio::spawn(async move { writer.write(vec![3]).await });
    wait_for_producer_operation(&producer).await;
    assert!(!blocked.is_finished());
    drop(consumer);

    assert!(matches!(
        blocked.await.unwrap(),
        Err(StreamWriteError::Closed(
            ByteStreamCloseCause::ConsumerCancelled
        ))
    ));
    assert_eq!(
        producer.attachment.activity(),
        AttachmentActivity {
            producer_active: true,
            consumer_active: false,
            buffered_bytes: 0,
            charged_bytes: 0,
        }
    );
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::ConsumerCancelled
    ));
}

#[test]
async fn cancelling_one_output_reader_does_not_affect_the_other_output() {
    let (stdout, stdout_reader, stdout_observer) = pair(2);
    let (stderr, stderr_reader, stderr_observer) = pair(2);
    assert!(stdout.configure_live());
    assert!(stderr.configure_live());
    stdout.write(vec![1, 2]).await.unwrap();

    let stdout_writer = stdout.writer();
    let blocked_stdout = tokio::spawn(async move { stdout_writer.write(vec![3]).await });
    wait_for_producer_operation(&stdout).await;
    drop(stdout_reader);

    assert!(matches!(
        blocked_stdout.await.unwrap(),
        Err(StreamWriteError::Closed(
            ByteStreamCloseCause::ConsumerCancelled
        ))
    ));
    assert!(matches!(
        stdout_observer.wait_terminal().await,
        ByteStreamCloseCause::ConsumerCancelled
    ));

    stderr.write(vec![9, 8]).await.unwrap();
    stderr.finish().unwrap();
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![9, 8]
    ));
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::End
    ));
    assert!(matches!(
        stderr_observer.wait_terminal().await,
        ByteStreamCloseCause::Finished
    ));
}

#[test]
async fn stdout_and_stderr_make_progress_independently_above_live_capacity() {
    let (stdout, stdout_reader, _) = pair(2);
    let (stderr, stderr_reader, _) = pair(2);
    assert!(stdout.configure_live());
    assert!(stderr.configure_live());
    stdout.write(vec![1, 2]).await.unwrap();
    stderr.write(vec![9, 8]).await.unwrap();

    let stdout_writer = stdout.writer();
    let stderr_writer = stderr.writer();
    let stdout_blocked = tokio::spawn(async move { stdout_writer.write(vec![3]).await });
    let stderr_blocked = tokio::spawn(async move { stderr_writer.write(vec![7]).await });
    wait_for_producer_operation(&stdout).await;
    wait_for_producer_operation(&stderr).await;

    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![9, 8]
    ));
    stderr_blocked.await.unwrap().unwrap();
    assert!(!stdout_blocked.is_finished());
    assert!(matches!(
        stdout_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2]
    ));
    stdout_blocked.await.unwrap().unwrap();

    stdout.finish().unwrap();
    stderr.finish().unwrap();
    assert!(matches!(
        stdout_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![3]
    ));
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![7]
    ));
}

#[test]
async fn output_limits_and_shared_memory_admission_are_independent_constraints() {
    let admitted = Arc::new(AtomicU64::new(0));
    let memory = |admitted: Arc<AtomicU64>| {
        AttachmentMemory::with_test_reservation(true, move |bytes| {
            let admitted = admitted.clone();
            async move {
                admitted
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                        (current + bytes <= 6).then_some(current + bytes)
                    })
                    .ok()
                    .map(|_| MemoryGrant::inert(bytes))
            }
        })
    };
    let (stdout, _stdout_reader, stdout_observer) = attachment_pair(4, memory(admitted.clone()));
    let (stderr, stderr_reader, stderr_observer) = attachment_pair(4, memory(admitted.clone()));
    assert!(stdout.configure_completion());
    assert!(stderr.configure_completion());

    stdout.write(vec![1, 2, 3, 4]).await.unwrap();
    let error = stderr.write(vec![9, 8, 7]).await.unwrap_err();
    assert!(matches!(
        error,
        StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::ResourceExhausted
        ))
    ));
    assert_eq!(stdout.attachment.activity().buffered_bytes, 4);
    assert_eq!(stderr.attachment.activity().buffered_bytes, 0);
    assert_eq!(admitted.load(Ordering::Acquire), 4);
    assert!(stdout_observer.terminal_snapshot().is_none());
    assert!(matches!(
        stderr_observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
    ));
    assert!(stderr.publish_completion());
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
}

#[test]
async fn host_cancellation_interrupts_an_outstanding_write() {
    let (producer, consumer, observer) = pair(2);
    assert!(producer.configure_live());
    producer.write(vec![1, 2]).await.unwrap();

    let writer = producer.writer();
    let blocked = tokio::spawn(async move { writer.write(vec![3]).await });
    wait_for_producer_operation(&producer).await;
    assert!(!blocked.is_finished());
    producer.cancel().unwrap();

    assert!(matches!(
        blocked.await.unwrap(),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
    ));
    drop(consumer);
}

#[test]
async fn host_cancellation_interrupts_a_pending_memory_reservation() {
    let (producer, consumer, observer) = attachment_pair(2, blocked_memory());
    assert!(producer.configure_live());
    let writer = producer.writer();
    let blocked = tokio::spawn(async move { writer.write(vec![1]).await });
    wait_for_producer_operation(&producer).await;
    assert!(!blocked.is_finished());

    producer.cancel().unwrap();

    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap(),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
    ));
    drop(consumer);
}

#[test]
async fn rejected_memory_reservation_selects_resource_exhaustion_without_buffering() {
    let (producer, consumer, observer) = attachment_pair(4, rejecting_memory());
    assert!(consumer.configure_completion());

    assert!(matches!(
        producer.write(vec![1]).await,
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::ResourceExhausted
        )))
    ));
    assert_eq!(producer.attachment.activity().buffered_bytes, 0);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
    ));
    assert!(
        observer
            .terminal_snapshot()
            .unwrap()
            .host_resource_exhausted
    );
}

#[test]
async fn historical_writes_bypass_measured_admission_until_live_upgrade() {
    let reservation_count = Arc::new(AtomicU64::new(0));
    let reservation_count_for_memory = reservation_count.clone();
    let memory = AttachmentMemory {
        reserve_tracked: Arc::new(move |bytes| {
            reservation_count_for_memory.fetch_add(1, Ordering::AcqRel);
            Box::pin(async move { Some(MemoryGrant::inert(bytes)) })
        }),
        tracking_enabled: Arc::new(AtomicBool::new(false)),
        tracking_pending: Arc::new(AtomicBool::new(false)),
        prepared_grant: Arc::new(Mutex::new(None)),
        historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
        live_activation: Arc::new(tokio::sync::Mutex::new(())),
    };
    let (producer, consumer, _) = attachment_pair(8, memory);
    let controller = producer.controller();

    producer.write(vec![1, 2, 3]).await.unwrap();
    assert_eq!(reservation_count.load(Ordering::Acquire), 0);
    assert!(controller.prepare_live_memory_accounting().await);
    assert_eq!(reservation_count.load(Ordering::Acquire), 1);
    controller.commit_live_memory_accounting();
    assert!(controller.prepare_live_memory_accounting().await);
    assert_eq!(reservation_count.load(Ordering::Acquire), 1);

    assert!(producer.configure_live());
    producer.write(vec![4]).await.unwrap();
    assert_eq!(reservation_count.load(Ordering::Acquire), 2);
    producer.finish().unwrap();
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![4]
    ));
}

#[test]
async fn rejected_historical_upgrade_selects_resource_exhaustion() {
    let reservation_count = Arc::new(AtomicU64::new(0));
    let reservation_count_for_memory = reservation_count.clone();
    let memory = AttachmentMemory {
        reserve_tracked: Arc::new(move |_| {
            reservation_count_for_memory.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { None })
        }),
        tracking_enabled: Arc::new(AtomicBool::new(false)),
        tracking_pending: Arc::new(AtomicBool::new(false)),
        prepared_grant: Arc::new(Mutex::new(None)),
        historical_charges: Arc::new(HistoricalAttachmentCharges::default()),
        live_activation: Arc::new(tokio::sync::Mutex::new(())),
    };
    let (producer, consumer, observer) = attachment_pair(8, memory);
    let controller = producer.controller();

    producer.write(vec![1, 2, 3]).await.unwrap();
    assert_eq!(reservation_count.load(Ordering::Acquire), 0);
    assert!(!controller.prepare_live_memory_accounting().await);
    assert_eq!(reservation_count.load(Ordering::Acquire), 1);
    assert!(observer.terminal_snapshot().is_none());
    controller.complete_rejected_live_memory_accounting();
    assert_eq!(controller.metadata().buffered_bytes, 0);
    assert_eq!(controller.metadata().charged_bytes, 0);
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
    ));
    assert!(
        observer
            .terminal_snapshot()
            .unwrap()
            .host_resource_exhausted
    );
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
}

#[test]
async fn reader_cancellation_after_terminal_releases_queued_memory_without_rewriting_terminal() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    producer.write(vec![1, 2, 3, 4]).await.unwrap();
    producer.finish().unwrap();
    assert_eq!(producer.attachment.activity().charged_bytes, 4);

    drop(consumer);

    assert_eq!(producer.attachment.activity().buffered_bytes, 0);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Finished
    ));
}

#[test]
async fn buffered_chunks_normalize_and_charge_their_exact_allocation() {
    let (producer, consumer, _) = pair(4);
    assert!(producer.configure_live());
    let mut bytes = Vec::with_capacity(1024);
    bytes.extend([1, 2, 3, 4]);

    producer.write(bytes).await.unwrap();

    assert_eq!(producer.attachment.activity().buffered_bytes, 4);
    assert_eq!(producer.attachment.activity().charged_bytes, 4);
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3, 4]
    ));
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
}

#[test]
async fn oversized_write_in_streaming_mode_exhausts_resource() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    assert!(matches!(
        producer.write(vec![1, 2, 3, 4, 5]).await,
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::ResourceExhausted
        )))
    ));
    assert_eq!(producer.attachment.activity().buffered_bytes, 0);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
    assert!(
        observer
            .terminal_snapshot()
            .unwrap()
            .host_resource_exhausted
    );
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
}

#[test]
async fn pending_crossing_write_is_rejected_atomically_after_completion_selection() {
    let (producer, consumer, observer) = pair(4);
    producer.write(vec![1, 2, 3]).await.unwrap();
    let writer = producer.writer();
    let pending = tokio::spawn(async move { writer.write(vec![4, 5]).await });
    wait_for_producer_operation(&producer).await;
    wait_for_charged_bytes(&producer, 5).await;
    assert!(!pending.is_finished());

    assert!(consumer.configure_completion());
    assert!(matches!(
        pending.await.unwrap(),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::ResourceExhausted
        )))
    ));
    assert_eq!(producer.attachment.activity().buffered_bytes, 3);
    assert_eq!(producer.attachment.activity().charged_bytes, 3);
    assert!(
        observer
            .terminal_snapshot()
            .unwrap()
            .host_resource_exhausted
    );
    assert!(producer.publish_completion());
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
}

#[test]
async fn caller_declared_resource_exhaustion_is_not_host_enforced() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    producer.fail(ByteStreamFailure::ResourceExhausted).unwrap();

    let snapshot = observer.terminal_snapshot().unwrap();
    assert!(!snapshot.host_resource_exhausted);
    assert!(matches!(
        snapshot.cause,
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::ResourceExhausted))
    ));
}

#[test]
async fn explicit_failure_cancellation_and_writer_clone_drop_remain_distinct() {
    let (producer, consumer, observer) = pair(4);
    assert!(producer.configure_live());
    drop(producer.writer());
    assert!(observer.terminal().is_none());
    producer
        .fail(ByteStreamFailure::Failed(
            "request contents must not escape".to_string(),
        ))
        .unwrap();
    producer
        .fail(ByteStreamFailure::Failed(
            "a different request must not escape either".to_string(),
        ))
        .unwrap();
    assert!(
        !observer
            .terminal_snapshot()
            .unwrap()
            .host_resource_exhausted
    );
    assert!(matches!(
        producer.cancel(),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
            ByteStreamFailure::Failed(reason)
        ))) if reason == "stream producer failed"
    ));
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason)))
            if reason == "stream producer failed"
    ));
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));

    let (producer, consumer, observer) = pair(4);
    assert!(consumer.configure_live());
    producer.cancel().unwrap();
    producer.cancel().unwrap();
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Cancelled))
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled)
    ));
}

#[test]
async fn completion_terminal_precedes_publication_and_charges_follow_buffer_drain() {
    let (producer, consumer, observer) = pair(5);
    assert!(producer.configure_completion());
    producer.write(Vec::new()).await.unwrap();
    producer.write(vec![0, 255]).await.unwrap();
    producer.write(vec![1, 2, 3]).await.unwrap();
    assert_eq!(producer.attachment.activity().buffered_bytes, 5);
    assert_eq!(producer.attachment.activity().charged_bytes, 5);
    producer.finish().unwrap();

    assert!(matches!(
        observer.wait_terminal().await,
        ByteStreamCloseCause::Finished
    ));
    assert_eq!(producer.attachment.activity().buffered_bytes, 5);
    assert!(producer.publish_completion());
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![0, 255]
    ));
    assert_eq!(producer.attachment.activity().charged_bytes, 3);
    assert!(matches!(
        consumer.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2, 3]
    ));
    assert_eq!(producer.attachment.activity().buffered_bytes, 0);
    assert_eq!(producer.attachment.activity().charged_bytes, 0);
    assert!(matches!(consumer.read_next().await, AttachmentRead::End));
}

#[test]
async fn separate_wasmtime_stores_drive_live_and_completion_attachments() {
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut sidecar_store = Store::new(&engine, ());
    let mut owner_store = Store::new(&engine, ());

    let input_bytes = (0..=u16::MAX)
        .map(|value| (value % 251) as u8)
        .collect::<Vec<_>>();
    let expected_input = input_bytes.clone();
    let (input, sidecar_input, _) = pair(1024);
    assert!(input.configure_live());
    let input_writer = tokio::spawn(async move {
        for chunk in input_bytes.chunks(1024) {
            input.write(chunk.to_vec()).await.unwrap();
        }
        input.finish().unwrap();
    });
    let received_input = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        drain_with_store(&mut sidecar_store, sidecar_input),
    )
    .await
    .unwrap();
    input_writer.await.unwrap();
    let received_input = received_input
        .into_iter()
        .flat_map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(received_input, expected_input);

    let (output, owner_output, _) = pair(6);
    assert!(output.configure_completion());
    output.write(vec![10, 11]).await.unwrap();
    output.write(vec![12, 13, 14, 15]).await.unwrap();
    output.finish().unwrap();
    let drain = drain_with_store(&mut owner_store, owner_output);
    tokio::pin!(drain);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut drain)
            .await
            .is_err()
    );
    assert!(output.publish_completion());
    let received_output = tokio::time::timeout(std::time::Duration::from_secs(1), &mut drain)
        .await
        .unwrap();
    let received_output = received_output
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(received_output, vec![vec![10, 11], vec![12, 13, 14, 15]]);
}
