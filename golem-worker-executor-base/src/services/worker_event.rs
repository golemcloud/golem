// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::metrics::events::{record_broadcast_event, record_event};
use futures_util::{stream, StreamExt};
use golem_common::model::{IdempotencyKey, LogLevel, WorkerEvent};
use ringbuf::storage::Heap;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::*;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::Stream;

/// Per-worker event stream
pub trait WorkerEventService {
    /// Emit an arbitrary worker event.
    ///
    /// There are helpers like `emit_stdout` for specific types.
    fn emit_event(&self, event: WorkerEvent, is_live: bool);

    /// Subscribes to the worker event stream and returns a receiver which can be either consumed one
    /// by one using `WorkerEventReceiver::recv` or converted to a tokio stream.
    fn receiver(&self) -> WorkerEventReceiver;

    /// Gets a string representation of the worker's stderr stream. The stream is truncated to the last
    /// N elements and may be further truncated by guest language specific matchers. The stream is
    /// guaranteed to contain information only emitted during the _last_ invocation.
    fn get_last_invocation_errors(&self) -> String;

    fn emit_stdout(&self, bytes: Vec<u8>, is_live: bool) {
        self.emit_event(WorkerEvent::stdout(bytes), is_live)
    }

    fn emit_stderr(&self, bytes: Vec<u8>, is_live: bool) {
        self.emit_event(WorkerEvent::stderr(bytes), is_live)
    }

    fn emit_log(&self, log_level: LogLevel, context: &str, message: &str, is_live: bool) {
        self.emit_event(WorkerEvent::log(log_level, context, message), is_live)
    }

    fn emit_invocation_start(
        &self,
        function: &str,
        idempotency_key: &IdempotencyKey,
        is_live: bool,
    ) {
        self.emit_event(
            WorkerEvent::invocation_start(function, idempotency_key),
            is_live,
        )
    }

    fn emit_invocation_finished(
        &self,
        function: &str,
        idempotency_key: &IdempotencyKey,
        is_live: bool,
    ) {
        self.emit_event(
            WorkerEvent::invocation_finished(function, idempotency_key),
            is_live,
        )
    }
}

#[derive(Clone)]
struct WorkerEventEntry {
    event: WorkerEvent,
    is_live: bool,
}

pub struct WorkerEventReceiver {
    history: VecDeque<WorkerEventEntry>,
    receiver: Receiver<WorkerEvent>,
}

impl WorkerEventReceiver {
    pub async fn recv(&mut self) -> Result<WorkerEvent, RecvError> {
        loop {
            let popped = self.history.pop_front();
            match popped {
                Some(entry) if entry.is_live => break Ok(entry.event),
                Some(_) => continue,
                None => break self.receiver.recv().await,
            }
        }
    }

    pub fn to_stream(self) -> impl Stream<Item = Result<WorkerEvent, BroadcastStreamRecvError>> {
        let Self { history, receiver } = self;
        stream::iter(history.into_iter().filter_map(
            |WorkerEventEntry { event, is_live }| {
                if is_live {
                    Some(Ok(event))
                } else {
                    None
                }
            },
        ))
        .chain(BroadcastStream::new(receiver))
    }
}

pub struct WorkerEventServiceDefault {
    sender: Sender<WorkerEvent>,
    ring_prod: Arc<Mutex<<SharedRb<Heap<WorkerEventEntry>> as Split>::Prod>>,
    ring_cons: Arc<Mutex<<SharedRb<Heap<WorkerEventEntry>> as Split>::Cons>>,
}

impl WorkerEventServiceDefault {
    pub fn new(channel_capacity: usize, ring_capacity: usize) -> WorkerEventServiceDefault {
        let (tx, _) = channel(channel_capacity);
        let (ring_prod, ring_cons) = HeapRb::new(ring_capacity).split();
        WorkerEventServiceDefault {
            sender: tx,
            ring_prod: Arc::new(Mutex::new(ring_prod)),
            ring_cons: Arc::new(Mutex::new(ring_cons)),
        }
    }
}

impl Drop for WorkerEventServiceDefault {
    fn drop(&mut self) {
        self.emit_event(WorkerEvent::Close, true);
    }
}

impl WorkerEventService for WorkerEventServiceDefault {
    fn emit_event(&self, event: WorkerEvent, is_live: bool) {
        if is_live {
            record_event(label(&event));

            if self.sender.receiver_count() > 0 {
                record_broadcast_event(label(&event));

                let _ = self.sender.send(event.clone());
            }
        }

        let entry = WorkerEventEntry { event, is_live };
        let mut ring_prod = self.ring_prod.lock().unwrap();
        while ring_prod.try_push(entry.clone()).is_err() {
            let mut ring_cons = self.ring_cons.lock().unwrap();
            let _ = ring_cons.try_pop();
        }
    }

    fn receiver(&self) -> WorkerEventReceiver {
        let receiver = self.sender.subscribe();
        let ring_cons = self.ring_cons.lock().unwrap();
        let history = ring_cons.iter().cloned().collect();
        WorkerEventReceiver { history, receiver }
    }

    fn get_last_invocation_errors(&self) -> String {
        let ring_cons = self.ring_cons.lock().unwrap();
        let history: Vec<_> = ring_cons.iter().cloned().collect();
        let mut stderr_chunks = Vec::new();
        for event in history.iter().rev() {
            match &event.event {
                WorkerEvent::StdErr { bytes, .. } => {
                    stderr_chunks.push(bytes.clone());
                }
                WorkerEvent::InvocationStart { .. } => break,
                _ => {}
            }
        }
        stderr_chunks.reverse();
        String::from_utf8_lossy(&stderr_chunks.concat()).to_string()
    }
}

fn label(event: &WorkerEvent) -> &'static str {
    match event {
        WorkerEvent::StdOut { .. } => "stdout",
        WorkerEvent::StdErr { .. } => "stderr",
        WorkerEvent::Log { .. } => "log",
        WorkerEvent::InvocationStart { .. } => "invocation_start",
        WorkerEvent::InvocationFinished { .. } => "invocation_finished",
        WorkerEvent::Close => "close",
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::{flaky, test, timeout};
    use tokio::sync::broadcast::error::RecvError;
    use tokio::sync::Mutex;
    use tracing::{info, Instrument};

    use crate::services::worker_event::{
        WorkerEvent, WorkerEventService, WorkerEventServiceDefault,
    };

    #[test]
    #[flaky(10)] // TODO: understand why is this flaky
    #[timeout(120000)]
    pub async fn both_subscriber_gets_events_small() {
        let svc = Arc::new(WorkerEventServiceDefault::new(4, 16));
        let rx1_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));
        let rx2_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));

        let svc1 = svc.clone();
        let rx1_events_clone = rx1_events.clone();
        let task1 = tokio::task::spawn(
            async move {
                let mut rx1 = svc1.receiver();
                drop(svc1);
                loop {
                    match rx1.recv().await {
                        Ok(WorkerEvent::Close) => break,
                        Ok(event) => {
                            rx1_events_clone.lock().await.push(event);
                        }
                        Err(RecvError::Closed) => break,
                        Err(RecvError::Lagged(n)) => {
                            info!("task1 lagged {n}");
                        }
                    }
                }
            }
            .in_current_span(),
        );

        for b in 1..=4u8 {
            svc.emit_event(WorkerEvent::stdout(vec![b]), true);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        loop {
            let received_count = rx1_events.lock().await.len();
            if received_count == 4 {
                break;
            }
        }

        let svc2 = svc.clone();
        let rx2_events_clone = rx2_events.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task2 = tokio::task::spawn(
            async move {
                let mut rx2 = svc2.receiver();
                drop(svc2);
                ready_tx.send(()).unwrap();
                loop {
                    match rx2.recv().await {
                        Ok(WorkerEvent::Close) => break,
                        Ok(event) => {
                            rx2_events_clone.lock().await.push(event);
                        }
                        Err(RecvError::Closed) => break,
                        Err(RecvError::Lagged(n)) => {
                            info!("task2 lagged {n}");
                        }
                    }
                }
            }
            .in_current_span(),
        );

        ready_rx.await.unwrap();

        for b in 5..=8u8 {
            svc.emit_event(WorkerEvent::stdout(vec![b]), true);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        drop(svc);

        task1.await.unwrap();
        task2.await.unwrap();

        let result1: Vec<WorkerEvent> = rx1_events.lock().await.iter().cloned().collect();
        let result2: Vec<WorkerEvent> = rx2_events.lock().await.iter().cloned().collect();

        assert_eq!(
            result1
                .into_iter()
                .filter_map(|event| match event {
                    WorkerEvent::StdOut { bytes, .. } => Some(bytes.to_vec()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![5],
                vec![6],
                vec![7],
                vec![8],
            ],
            "result1"
        );
        assert_eq!(
            result2
                .into_iter()
                .filter_map(|event| match event {
                    WorkerEvent::StdOut { bytes, .. } => Some(bytes.to_vec()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![5],
                vec![6],
                vec![7],
                vec![8],
            ],
            "result2"
        );
    }

    #[test]
    #[timeout(120000)]
    pub async fn both_subscriber_gets_events_stream_small() {
        let svc = Arc::new(WorkerEventServiceDefault::new(4, 16));
        let rx1_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));
        let rx2_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));

        let svc1 = svc.clone();
        let rx1_events_clone = rx1_events.clone();
        let task1 = tokio::task::spawn(
            async move {
                let rx1 = svc1.receiver();
                drop(svc1);
                rx1.to_stream()
                    .for_each(|item| async {
                        match item {
                            Ok(WorkerEvent::Close) => {}
                            Ok(event) => {
                                rx1_events_clone.lock().await.push(event);
                            }
                            Err(_) => {}
                        }
                    })
                    .await;
            }
            .in_current_span(),
        );

        for b in 1..=4u8 {
            svc.emit_event(WorkerEvent::stdout(vec![b]), true);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        loop {
            let received_count = rx1_events.lock().await.len();
            if received_count == 4 {
                break;
            }
        }

        let svc2 = svc.clone();
        let rx2_events_clone = rx2_events.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task2 = tokio::task::spawn(
            async move {
                let rx2 = svc2.receiver();
                drop(svc2);
                ready_tx.send(()).unwrap();
                rx2.to_stream()
                    .for_each(|item| async {
                        match item {
                            Ok(WorkerEvent::Close) => {}
                            Ok(event) => {
                                rx2_events_clone.lock().await.push(event);
                            }
                            Err(_) => {}
                        }
                    })
                    .await;
            }
            .in_current_span(),
        );

        ready_rx.await.unwrap();

        for b in 5..=8u8 {
            svc.emit_event(WorkerEvent::stdout(vec![b]), true);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        drop(svc);

        task1.await.unwrap();
        task2.await.unwrap();

        let result1: Vec<WorkerEvent> = rx1_events.lock().await.iter().cloned().collect();
        let result2: Vec<WorkerEvent> = rx2_events.lock().await.iter().cloned().collect();

        assert_eq!(
            result1
                .into_iter()
                .filter_map(|event| match event {
                    WorkerEvent::StdOut { bytes, .. } => Some(bytes.to_vec()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![5],
                vec![6],
                vec![7],
                vec![8],
            ],
            "result1"
        );
        assert_eq!(
            result2
                .into_iter()
                .filter_map(|event| match event {
                    WorkerEvent::StdOut { bytes, .. } => Some(bytes.to_vec()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![5],
                vec![6],
                vec![7],
                vec![8],
            ],
            "result2"
        );
    }

    #[test]
    #[flaky(10)] // TODO: understand why it is flaky
    #[timeout(120000)]
    pub async fn both_subscriber_gets_events_large() {
        let svc = Arc::new(WorkerEventServiceDefault::new(4, 4));
        let rx1_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));
        let rx2_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));

        let svc1 = svc.clone();
        let rx1_events_clone = rx1_events.clone();
        let task1 = tokio::task::spawn(
            async move {
                let mut rx1 = svc1.receiver();
                drop(svc1);
                loop {
                    match rx1.recv().await {
                        Ok(WorkerEvent::Close) => break,
                        Ok(event) => {
                            rx1_events_clone.lock().await.push(event);
                        }
                        Err(RecvError::Closed) => break,
                        Err(RecvError::Lagged(n)) => {
                            info!("task1 lagged {n}");
                        }
                    }
                }
            }
            .in_current_span(),
        );

        for b in 1..=1000 {
            let s = format!("{}", b);
            svc.emit_event(WorkerEvent::stdout(s.as_bytes().into()), true);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        loop {
            let received_count = rx1_events.lock().await.len();
            if received_count == 1000 {
                break;
            }
        }

        let svc2 = svc.clone();
        let rx2_events_clone = rx2_events.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task2 = tokio::task::spawn(
            async move {
                let mut rx2 = svc2.receiver();
                drop(svc2);
                ready_tx.send(()).unwrap();
                loop {
                    match rx2.recv().await {
                        Ok(WorkerEvent::Close) => break,
                        Ok(event) => {
                            rx2_events_clone.lock().await.push(event);
                        }
                        Err(RecvError::Closed) => break,
                        Err(RecvError::Lagged(n)) => {
                            info!("task1 lagged {n}");
                        }
                    }
                }
            }
            .in_current_span(),
        );

        ready_rx.await.unwrap();

        for b in 1001..=1004 {
            let s = format!("{}", b);
            svc.emit_event(WorkerEvent::stdout(s.as_bytes().into()), true);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        drop(svc);

        task1.await.unwrap();
        task2.await.unwrap();

        let result1: Vec<WorkerEvent> = rx1_events.lock().await.iter().cloned().collect();
        let result2: Vec<WorkerEvent> = rx2_events.lock().await.iter().cloned().collect();

        assert_eq!(result1.len(), 1004, "result1 length");
        assert_eq!(
            result2
                .into_iter()
                .filter_map(|event| match event {
                    WorkerEvent::StdOut { bytes, .. } =>
                        Some(String::from_utf8_lossy(&bytes).to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                "997".to_string(),
                "998".to_string(),
                "999".to_string(),
                "1000".to_string(),
                "1001".to_string(),
                "1002".to_string(),
                "1003".to_string(),
                "1004".to_string(),
            ],
            "result2"
        );
    }
}
