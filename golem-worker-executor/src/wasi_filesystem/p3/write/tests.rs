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
use test_r::test;

#[test]
fn dropped_p3_write_progress_sender_reports_error() {
    let (chunks_tx, _chunks_rx) = tokio::sync::mpsc::unbounded_channel();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let mut consumer = FilesystemWriteConsumer {
        chunks_tx: Some(chunks_tx),
        pending_chunk: Some(PendingFilesystemWriteChunk { result_rx }),
    };
    drop(result_tx);

    let mut cx = Context::from_waker(std::task::Waker::noop());
    assert!(matches!(
        consumer.poll_pending_result(&mut cx),
        Poll::Ready(Err(_))
    ));
}

#[test]
async fn dropped_p3_result_future_does_not_cancel_the_stream_write() {
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    drop(result_rx);
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completed_by_operation = completed.clone();

    complete_filesystem_write_task(result_tx, async move {
        completed_by_operation.store(true, std::sync::atomic::Ordering::Release);
        Ok(Ok(()))
    })
    .await;

    assert!(completed.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
#[test_r::timeout("30s")]
async fn interrupted_p3_result_observation_does_not_cancel_task() {
    let (sender, receive) = tokio::sync::oneshot::channel();
    let resume = Arc::new(tokio::sync::Notify::new());
    let effect = tokio::spawn({
        let resume = resume.clone();
        async move {
            resume.notified().await;
            sender.send(Ok(Ok(()))).is_err()
        }
    });
    let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
    let error = wait_filesystem_task_result(receive, Box::pin(std::future::ready(kind)))
        .await
        .unwrap_err();
    assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&kind));
    assert!(!effect.is_finished());
    resume.notify_one();
    assert!(effect.await.unwrap());
}

#[test]
async fn p3_result_completion_remains_authoritative() {
    let (sender, receive) = tokio::sync::oneshot::channel();
    sender
        .send(Ok(Err(types::ErrorCode::NotPermitted)))
        .unwrap();
    let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
    assert!(matches!(
        wait_filesystem_task_result(receive, Box::pin(std::future::ready(kind)))
            .await
            .unwrap(),
        Err(types::ErrorCode::NotPermitted)
    ));
}
