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

use crate::durable_host::replay_state::ReplayState;
use crate::error::GolemError;
use crate::services::oplog::Oplog;
use golem_common::model::oplog::OplogEntry;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error};

pub(crate) struct SyncHelper {
    handle: JoinHandle<()>,
    tx: UnboundedSender<SyncHelperCommand>,
    semaphore: Arc<Semaphore>,
    error: Arc<Mutex<Option<GolemError>>>,
}

impl SyncHelper {
    pub fn new(oplog: Arc<dyn Oplog + Send + Sync>, replay_state: ReplayState) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let semaphore = Arc::new(Semaphore::new(1));
        let semaphore_clone = semaphore.clone();
        let error = Arc::new(Mutex::new(None));
        let error_clone = error.clone();
        let handle = tokio::spawn(async move {
            Self::event_processor(oplog, rx, semaphore_clone, error_clone, replay_state).await;
        });
        Self {
            handle,
            tx,
            semaphore,
            error,
        }
    }

    pub fn write_oplog_entry(&self, entry: OplogEntry) {
        self.tx
            .send(SyncHelperCommand::WriteOplogEntry { entry })
            .expect("Failed to send command to sync helper");
    }

    pub fn skip_oplog_entry(
        &self,
        check: Box<dyn (Fn(&OplogEntry) -> bool) + Send + Sync>,
        expectation: &str,
    ) {
        self.tx
            .send(SyncHelperCommand::SkipOplogEntry {
                check,
                expectation: expectation.to_string(),
            })
            .expect("Failed to send command to sync helper");
    }

    pub async fn sync(&self) -> Result<SyncHelperPermit, GolemError> {
        debug!(
            "SYNC ACQUIRING PERMIT {}",
            self.semaphore.available_permits()
        );
        if let Ok(permit) = Semaphore::acquire_owned(self.semaphore.clone()).await {
            debug!("SYNC GOT PERMIT");

            let mut error = self.error.lock().await;
            if let Some(error) = error.take() {
                Err(error)
            } else {
                debug!("SYNC PERMIT READY");
                Ok(SyncHelperPermit::new(permit))
            }
        } else {
            error!("Semaphore was closed");
            Err(GolemError::unknown("Semaphore was closed"))
        }
    }

    async fn event_processor(
        oplog: Arc<dyn Oplog + Send + Sync>,
        mut rx: UnboundedReceiver<SyncHelperCommand>,
        semaphore: Arc<Semaphore>,
        error: Arc<Mutex<Option<GolemError>>>,
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
                }
            }
            debug!(
                "SYNC GOT COMMAND {last:?} ACQUIRING PERMIT {}",
                semaphore.available_permits()
            );
            let mut permit = Some(semaphore.acquire().await.unwrap());
            let retry = loop {
                match last.take().map(Ok).unwrap_or(rx.try_recv()) {
                    Ok(command) => match command {
                        SyncHelperCommand::WriteOplogEntry { entry } => {
                            debug!("SYNC WRITING OPLOG ENTRY");
                            oplog.add(entry).await;
                        }
                        SyncHelperCommand::SkipOplogEntry { check, expectation } => {
                            debug!("SYNC SKIPPING OPLOG ENTRY");
                            loop {
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
                            }
                        }
                    },
                    Err(TryRecvError::Empty) => {
                        debug!("SYNC RELEASING PERMIT");
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
        debug!("SYNC DROPPING");
        self.handle.abort();
        debug!("SYNC DROPPED");
    }
}

pub(crate) struct SyncHelperPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl SyncHelperPermit {
    pub fn new(_permit: tokio::sync::OwnedSemaphorePermit) -> Self {
        Self { _permit }
    }
}

impl Drop for SyncHelperPermit {
    fn drop(&mut self) {
        debug!("SYNC RELEASED PERMIT");
    }
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
