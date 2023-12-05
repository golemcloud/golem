use ringbuf::*;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::*;

use crate::metrics::events::{record_broadcast_event, record_event};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkerEvent {
    StdOut(Vec<u8>),
    StdErr(Vec<u8>),
    Log {
        level: LogLevel,
        context: String,
        message: String,
    },
    Close,
}

/// Per-worker event stream
pub trait WorkerEventService {
    fn emit_event(&self, event: WorkerEvent);

    fn emit_stdout(&self, data: Vec<u8>) {
        self.emit_event(WorkerEvent::StdOut(data))
    }

    fn emit_stderr(&self, data: Vec<u8>) {
        self.emit_event(WorkerEvent::StdErr(data))
    }

    fn emit_log(&self, log_level: LogLevel, context: &str, message: &str) {
        self.emit_event(WorkerEvent::Log {
            level: log_level,
            context: context.to_string(),
            message: message.to_string(),
        })
    }

    fn receiver(&self) -> WorkerEventReceiver;
}

pub struct WorkerEventReceiver {
    history: Vec<WorkerEvent>,
    receiver: Receiver<WorkerEvent>,
}

impl WorkerEventReceiver {
    pub async fn recv(&mut self) -> Result<WorkerEvent, RecvError> {
        match self.history.pop() {
            Some(event) => Ok(event),
            None => self.receiver.recv().await,
        }
    }
}

pub struct WorkerEventServiceDefault {
    sender: Sender<WorkerEvent>,
    ring: HeapRb<WorkerEvent>,
}

impl WorkerEventServiceDefault {
    pub fn new(channel_capacity: usize, ring_capacity: usize) -> WorkerEventServiceDefault {
        let (tx, _) = channel(channel_capacity);
        let ring = HeapRb::new(ring_capacity);
        // ring.sub
        WorkerEventServiceDefault { sender: tx, ring }
    }
}

impl Drop for WorkerEventServiceDefault {
    fn drop(&mut self) {
        self.emit_event(WorkerEvent::Close);
    }
}

impl WorkerEventService for WorkerEventServiceDefault {
    fn emit_event(&self, event: WorkerEvent) {
        record_event(label(&event));

        if self.sender.receiver_count() > 0 {
            record_broadcast_event(label(&event));

            let _ = self.sender.send(event.clone());
        }
        let _ = unsafe { Producer::new(&self.ring) }.push(event);
    }

    fn receiver(&self) -> WorkerEventReceiver {
        let receiver = self.sender.subscribe();
        let history = self.ring.iter().cloned().rev().collect();
        WorkerEventReceiver { history, receiver }
    }
}

fn label(event: &WorkerEvent) -> &'static str {
    match event {
        WorkerEvent::StdOut(_) => "stdout",
        WorkerEvent::StdErr(_) => "stderr",
        WorkerEvent::Log { .. } => "log",
        WorkerEvent::Close => "close",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use crate::services::worker_event::{
        WorkerEvent, WorkerEventService, WorkerEventServiceDefault,
    };

    #[tokio::test]
    pub async fn both_subscriber_gets_events_small() {
        let svc = Arc::new(WorkerEventServiceDefault::new(4, 16));
        let rx1_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));
        let rx2_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));

        let svc1 = svc.clone();
        let rx1_events_clone = rx1_events.clone();
        let task1 = tokio::task::spawn(async move {
            let mut rx1 = svc1.receiver();
            drop(svc1);
            loop {
                match rx1.recv().await.unwrap() {
                    WorkerEvent::Close => break,
                    event => {
                        rx1_events_clone.lock().await.push(event);
                    }
                }
            }
        });

        for b in 1..5u8 {
            svc.emit_event(WorkerEvent::StdOut(vec![b]));
        }

        let svc2 = svc.clone();
        let rx2_events_clone = rx2_events.clone();
        let task2 = tokio::task::spawn(async move {
            let mut rx2 = svc2.receiver();
            drop(svc2);
            loop {
                match rx2.recv().await.unwrap() {
                    WorkerEvent::Close => break,
                    event => {
                        rx2_events_clone.lock().await.push(event);
                    }
                }
            }
        });

        for b in 5..9u8 {
            svc.emit_event(WorkerEvent::StdOut(vec![b]));
        }

        drop(svc);

        task1.await.unwrap();
        task2.await.unwrap();

        let result1: Vec<WorkerEvent> = rx1_events.lock().await.iter().cloned().collect();
        let result2: Vec<WorkerEvent> = rx2_events.lock().await.iter().cloned().collect();

        assert_eq!(
            result1
                == vec![
                    WorkerEvent::StdOut(vec![1]),
                    WorkerEvent::StdOut(vec![2]),
                    WorkerEvent::StdOut(vec![3]),
                    WorkerEvent::StdOut(vec![5]),
                    WorkerEvent::StdOut(vec![6]),
                    WorkerEvent::StdOut(vec![7]),
                    WorkerEvent::StdOut(vec![8]),
                ],
            result2
                == vec![
                    WorkerEvent::StdOut(vec![1]),
                    WorkerEvent::StdOut(vec![2]),
                    WorkerEvent::StdOut(vec![3]),
                    WorkerEvent::StdOut(vec![5]),
                    WorkerEvent::StdOut(vec![6]),
                    WorkerEvent::StdOut(vec![7]),
                    WorkerEvent::StdOut(vec![8]),
                ]
        )
    }

    #[tokio::test]
    pub async fn both_subscriber_gets_events_large() {
        let svc = Arc::new(WorkerEventServiceDefault::new(4, 4));
        let rx1_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));
        let rx2_events = Arc::new(Mutex::new(Vec::<WorkerEvent>::new()));

        let svc1 = svc.clone();
        let rx1_events_clone = rx1_events.clone();
        let task1 = tokio::task::spawn(async move {
            let mut rx1 = svc1.receiver();
            drop(svc1);
            loop {
                match rx1.recv().await.unwrap() {
                    WorkerEvent::Close => break,
                    event => {
                        rx1_events_clone.lock().await.push(event);
                    }
                }
            }
        });

        for b in 1..1001 {
            let s = format!("{}", b);
            svc.emit_event(WorkerEvent::StdOut(s.as_bytes().into()));
        }

        let svc2 = svc.clone();
        let rx2_events_clone = rx2_events.clone();
        let task2 = tokio::task::spawn(async move {
            let mut rx2 = svc2.receiver();
            drop(svc2);
            loop {
                match rx2.recv().await.unwrap() {
                    WorkerEvent::Close => break,
                    event => {
                        rx2_events_clone.lock().await.push(event);
                    }
                }
            }
        });

        for b in 1001..1005 {
            let s = format!("{}", b);
            svc.emit_event(WorkerEvent::StdOut(s.as_bytes().into()));
        }

        drop(svc);

        task1.await.unwrap();
        task2.await.unwrap();

        let result1: Vec<WorkerEvent> = rx1_events.lock().await.iter().cloned().collect();
        let result2: Vec<WorkerEvent> = rx2_events.lock().await.iter().cloned().collect();

        assert_eq!(
            result1.len() == 1004,
            result2
                == vec![
                    WorkerEvent::StdOut("997".as_bytes().into()),
                    WorkerEvent::StdOut("998".as_bytes().into()),
                    WorkerEvent::StdOut("999".as_bytes().into()),
                    WorkerEvent::StdOut("1000".as_bytes().into()),
                    WorkerEvent::StdOut("1001".as_bytes().into()),
                    WorkerEvent::StdOut("1002".as_bytes().into()),
                    WorkerEvent::StdOut("1003".as_bytes().into()),
                    WorkerEvent::StdOut("1004".as_bytes().into()),
                ]
        )
    }
}
