// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::model::oplog::OplogEntry;
use std::fmt::Debug;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tokio::task::{yield_now, JoinHandle};
use tracing::debug;

use crate::durable_host::replay_state::ReplayState;
use crate::error::GolemError;
use crate::services::oplog::Oplog;

pub(crate) struct SyncHelper {
    handle: JoinHandle<()>,
    tx: UnboundedSender<SyncHelperCommand>,
    mutex: Arc<Mutex<()>>,
    error: Arc<Mutex<Option<GolemError>>>,
    queue_size: Arc<AtomicUsize>,
}

impl SyncHelper {
    pub fn new(oplog: Arc<dyn Oplog + Send + Sync>, replay_state: ReplayState) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mutex = Arc::new(Mutex::new(()));
        let mutex_clone = mutex.clone();
        let error = Arc::new(Mutex::new(None));
        let error_clone = error.clone();
        let queue_size = Arc::new(AtomicUsize::new(0));
        let queue_size_clone = queue_size.clone();

        let handle = tokio::spawn(async move {
            Self::event_processor(
                oplog,
                rx,
                mutex_clone,
                error_clone,
                queue_size_clone,
                replay_state,
            )
            .await;
        });

        Self {
            handle,
            tx,
            mutex,
            error,
            queue_size,
        }
    }

    pub fn write_oplog_entry(&self, entry: OplogEntry) {
        debug!("enqueue write oplog entry: {:?}", entry);
        self.queue_size
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.tx
            .send(SyncHelperCommand::WriteOplogEntry { entry })
            .expect("Failed to send command to sync helper");
    }

    pub fn skip_oplog_entry(
        &self,
        check: Box<dyn (Fn(&OplogEntry) -> bool) + Send + Sync>,
        expectation: &str,
    ) {
        debug!("enqueue skip oplog entry: {}", expectation);
        self.queue_size
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.tx
            .send(SyncHelperCommand::SkipOplogEntry {
                check,
                expectation: expectation.to_string(),
            })
            .expect("Failed to send command to sync helper");
    }

    pub async fn sync(&self) -> Result<SyncHelperPermit, GolemError> {
        while self.queue_size.load(std::sync::atomic::Ordering::Acquire) != 0 {
            yield_now().await;
        }

        let permit = Mutex::lock_owned(self.mutex.clone()).await;
        let mut error = self.error.lock().await;
        if let Some(error) = error.take() {
            Err(error)
        } else {
            Ok(SyncHelperPermit::new(permit))
        }
    }

    async fn event_processor(
        oplog: Arc<dyn Oplog + Send + Sync>,
        mut rx: UnboundedReceiver<SyncHelperCommand>,
        mutex: Arc<Mutex<()>>,
        error: Arc<Mutex<Option<GolemError>>>,
        queue_size: Arc<AtomicUsize>,
        mut replay_state: ReplayState,
    ) {
        loop {
            let mut last;
            match rx.recv().await {
                None => {
                    break;
                }
                Some(command) => {
                    last = Some(command);
                    queue_size.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            debug!("received command {last:?}, acquiring lock");
            let mut permit = Some(mutex.lock().await);
            let retry = loop {
                match last.take().map(Ok).unwrap_or({
                    let cmd = rx.try_recv();
                    if cmd.is_ok() {
                        queue_size.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                    }
                    cmd
                }) {
                    Ok(command) => {
                        debug!("processing command {command:?}");
                        match command {
                            SyncHelperCommand::WriteOplogEntry { entry } => {
                                oplog.add(entry).await;
                            }
                            SyncHelperCommand::SkipOplogEntry { check, expectation } => loop {
                                let (_, oplog_entry) = replay_state.get_oplog_entry().await;
                                if check(&oplog_entry) {
                                    break;
                                } else if oplog_entry.is_hint() {
                                } else {
                                    let mut error = error.lock().await;
                                    *error = Some(GolemError::unexpected_oplog_entry(
                                        expectation,
                                        format!("{:?}", oplog_entry),
                                    ));
                                    break;
                                }
                            },
                        }
                    }
                    Err(TryRecvError::Empty) => {
                        debug!("no more commands enqueued, releasing lock");
                        let _ = permit.take();
                        break true;
                    }
                    Err(TryRecvError::Disconnected) => {
                        break false;
                    }
                }
            };

            if retry {
                continue;
            } else {
                break;
            }
        }
    }
}

impl Drop for SyncHelper {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(crate) struct SyncHelperPermit {
    _permit: tokio::sync::OwnedMutexGuard<()>,
}

impl SyncHelperPermit {
    pub fn new(_permit: tokio::sync::OwnedMutexGuard<()>) -> Self {
        Self { _permit }
    }
}

impl Drop for SyncHelperPermit {
    fn drop(&mut self) {}
}

enum SyncHelperCommand {
    WriteOplogEntry {
        entry: OplogEntry,
    },
    SkipOplogEntry {
        check: Box<dyn (Fn(&OplogEntry) -> bool) + Send + Sync>,
        expectation: String,
    },
}

impl Debug for SyncHelperCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncHelperCommand::WriteOplogEntry { .. } => f.debug_struct("WriteOplogEntry").finish(),
            SyncHelperCommand::SkipOplogEntry { .. } => f.debug_struct("SkipOplogEntry").finish(),
        }
    }
}
