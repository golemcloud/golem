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

use std::cmp::min;
use std::io::{IoSlice, IoSliceMut};
#[cfg(unix)]
use std::os::fd::BorrowedFd;
use std::string::FromUtf8Error;
use std::sync::Arc;

use anyhow::anyhow;
use bytes::{Buf, BufMut, Bytes};
use golem_common::model::{InvocationKey, WorkerId};
use tokio::sync::{mpsc, Mutex};

use crate::error::GolemError;
use crate::services::invocation_key::InvocationKeyService;

#[derive(Clone)]
pub struct ManagedStandardIo {
    current: Arc<Mutex<Option<State>>>,
    instance_id: WorkerId,
    invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
    current_enqueue: Arc<Mutex<Option<mpsc::Sender<Event>>>>,
    enqueue_capacity: usize,
}

#[derive(Debug)]
enum State {
    Live,
    SingleCall {
        input: Bytes,
        pos: usize,
        captured: Vec<u8>,
    },
    EventLoopIdle {
        enqueue: mpsc::Sender<Event>,
        dequeue: mpsc::Receiver<Event>,
    },
    EventLoopProcessing {
        enqueue: mpsc::Sender<Event>,
        dequeue: mpsc::Receiver<Event>,
        input: Bytes,
        pos: usize,
        invocation_key: InvocationKey,
        captured: Option<Vec<u8>>,
    },
}

#[derive(Debug, Clone)]
struct Event {
    input: Bytes,
    invocation_key: InvocationKey,
}

/// Maps to the type defined in the WASI IO package
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedStreamStatus {
    Open,
    Ended,
}

/// Capacity of the mspc channels used to send events to the IO loop
const DEFAULT_ENQUEUE_CAPACITY: usize = 128;

impl ManagedStandardIo {
    pub fn new(
        instance_id: WorkerId,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
    ) -> Self {
        Self {
            current: Arc::new(Mutex::new(Some(State::Live))),
            instance_id,
            invocation_key_service,
            current_enqueue: Arc::new(Mutex::new(None)),
            enqueue_capacity: DEFAULT_ENQUEUE_CAPACITY,
        }
    }

    pub async fn enqueue(&self, message: Bytes, invocation_key: InvocationKey) {
        let event = Event {
            input: message,
            invocation_key,
        };
        let mut current_enqueue = self.current_enqueue.lock().await;
        match &*current_enqueue {
            Some(enqueue) => enqueue
                .send(event)
                .await
                .expect("Failed to enqueue event in ManagedStandardIo"),
            None => {
                let (enqueue, dequeue) = mpsc::channel(self.enqueue_capacity);
                enqueue
                    .send(event)
                    .await
                    .expect("Failed to enqueue event in ManagedStandardIo");
                *current_enqueue = Some(enqueue.clone());
                *self.current.lock().await = Some(State::EventLoopIdle { enqueue, dequeue });
            }
        }
    }

    pub async fn finish_single_stdio_call(&self) -> Result<String, FromUtf8Error> {
        let mut current = self.current.lock().await;

        match current
            .take()
            .expect("ManagedStandardIo is in an invalid state")
        {
            State::SingleCall { captured, .. } => {
                *current = Some(State::Live);
                String::from_utf8(captured)
            }
            _ => panic!("finish_single_stdio_call called in unexpected state of ManagedStandardIo"),
        }
    }

    pub async fn start_single_stdio_call(&self, input: String) {
        let input = Bytes::from(input);
        *self.current.lock().await = Some(State::SingleCall {
            input,
            pos: 0,
            captured: Vec::new(),
        });
    }

    pub async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        if let Some(State::EventLoopProcessing { invocation_key, .. }) = &*self.current.lock().await
        {
            Some(invocation_key.clone())
        } else {
            None
        }
    }

    // The following functions are implementations for the WASI InputStream and OutputStream traits,
    // but here we don't have the bindings available so the actual wiring of the trait implementation
    // must be done in the library user's code.

    #[cfg(unix)]
    pub fn pollable_read(&self) -> Option<BorrowedFd> {
        None
    }

    #[cfg(windows)]
    pub fn pollable_read(&self) -> Option<io_extras::os::windows::BorrowedHandleOrSocket> {
        None
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> anyhow::Result<(u64, ManagedStreamStatus)> {
        loop {
            let mut current = self.current.lock().await;

            match current
                .take()
                .expect("ManagedStandardIo is in an invalid state")
            {
                state @ State::Live => {
                    *current = Some(state);
                    break Err(disabled_stdin_error());
                }
                State::SingleCall {
                    input,
                    pos,
                    captured,
                } => {
                    let input = input;
                    let mut pos = pos;
                    let result = read_from_bytes(&input, &mut pos, buf);
                    *current = Some(State::SingleCall {
                        input,
                        pos,
                        captured,
                    });
                    break Ok(result);
                }
                State::EventLoopIdle { dequeue, enqueue } => {
                    let mut dequeue = dequeue;
                    match dequeue.recv().await {
                        Some(event) => {
                            let input = event.input;
                            let mut pos = 0;
                            let result = read_from_bytes(&input, &mut pos, buf);
                            *current = Some(State::EventLoopProcessing {
                                enqueue,
                                dequeue,
                                input,
                                pos,
                                invocation_key: event.invocation_key,
                                captured: Some(Vec::new()),
                            });
                            break Ok(result);
                        }
                        None => {
                            *current = Some(State::EventLoopIdle { dequeue, enqueue });
                            break Ok((0, ManagedStreamStatus::Ended));
                        }
                    }
                }
                State::EventLoopProcessing {
                    enqueue,
                    dequeue,
                    input,
                    pos,
                    invocation_key,
                    captured,
                } => {
                    let mut pos = pos;
                    if pos < input.len() {
                        let result = read_from_bytes(&input, &mut pos, buf);
                        *current = Some(State::EventLoopProcessing {
                            enqueue,
                            dequeue,
                            input,
                            pos,
                            invocation_key,
                            captured,
                        });
                        break Ok(result);
                    } else if captured.is_none() {
                        // Captured response already sent, we can get the next event
                        *current = Some(State::EventLoopIdle { dequeue, enqueue });
                        continue;
                    } else {
                        *current = Some(State::EventLoopProcessing {
                            enqueue,
                            dequeue,
                            input,
                            pos,
                            invocation_key,
                            captured,
                        });
                        break Err(must_write_first_error());
                    }
                }
            }
        }
    }

    pub async fn read_vectored<'a>(
        &'a mut self,
        bufs: &'a mut [IoSliceMut<'a>],
    ) -> anyhow::Result<(u64, ManagedStreamStatus)> {
        let mut total = 0;
        let mut final_eof = ManagedStreamStatus::Open;
        for slice in bufs {
            let mut buf = vec![0; slice.len()];
            let (n, eof) = self.read(&mut buf).await?;
            slice.copy_from_slice(&buf);
            total += n;
            final_eof = eof;
        }
        Ok((total, final_eof))
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub async fn skip(&mut self, nelem: u64) -> anyhow::Result<(u64, ManagedStreamStatus)> {
        let mut buf = vec![0; nelem as usize];
        self.read(&mut buf).await
    }

    pub async fn num_ready_bytes(&self) -> anyhow::Result<u64> {
        let current = self.current.lock().await;
        match &*current {
            None => Ok(0),
            Some(State::Live) => Ok(0),
            Some(State::SingleCall { input, pos, .. }) => Ok((input.len() - pos) as u64),
            Some(State::EventLoopIdle { .. }) => Ok(0),
            Some(State::EventLoopProcessing { input, pos, .. }) => Ok((input.len() - pos) as u64),
        }
    }

    pub async fn readable(&self) -> anyhow::Result<()> {
        match &*self.current.lock().await {
            None => Err(disabled_stdin_error()),
            Some(State::Live) => Err(disabled_stdin_error()),
            Some(State::SingleCall { .. }) => Ok(()),
            Some(State::EventLoopIdle { .. }) => Ok(()),
            Some(State::EventLoopProcessing { .. }) => Ok(()),
        }
    }

    #[cfg(unix)]
    pub fn pollable_write(&self) -> Option<BorrowedFd> {
        None
    }

    #[cfg(windows)]
    pub fn pollable_write(&self) -> Option<io_extras::os::windows::BorrowedHandleOrSocket> {
        None
    }

    pub async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        let mut current = self.current.lock().await;
        match current
            .take()
            .expect("ManagedStandardIo is in an invalid state")
        {
            State::Live => {
                *current = Some(State::Live);
                Ok(())
            }
            State::SingleCall {
                input,
                pos,
                captured,
            } => {
                let mut captured = captured;
                captured.put_slice(buf);
                *current = Some(State::SingleCall {
                    input,
                    pos,
                    captured,
                });
                Ok(())
            }
            State::EventLoopIdle { enqueue, dequeue } => {
                *current = Some(State::EventLoopIdle { enqueue, dequeue });
                Err(must_read_first_error())
            }
            State::EventLoopProcessing {
                enqueue,
                dequeue,
                input,
                pos,
                invocation_key,
                captured,
            } => {
                match captured {
                    Some(captured) => {
                        let mut captured = captured;
                        captured.put_slice(buf);

                        if let Ok(decoded) = std::str::from_utf8(&captured) {
                            if let Some(index) = decoded.find('\n') {
                                if index < decoded.len() - 1 {
                                    Err(must_read_first_error())
                                } else {
                                    let result = String::from_utf8(captured)
                                        .map(|captured_string| {
                                            vec![golem_wasm_rpc::protobuf::Val {
                                                val: Some(
                                                    golem_wasm_rpc::protobuf::val::Val::String(
                                                        captured_string,
                                                    ),
                                                ),
                                            }]
                                        })
                                        .map_err(|_| GolemError::Runtime {
                                            details: "stdout did not contain a valid utf-8 string"
                                                .to_string(),
                                        });
                                    self.invocation_key_service.confirm_key(
                                        &self.instance_id,
                                        &invocation_key,
                                        result,
                                    );
                                    *current = Some(State::EventLoopProcessing {
                                        enqueue,
                                        dequeue,
                                        input,
                                        pos,
                                        invocation_key,
                                        captured: None,
                                    });
                                    Ok(())
                                }
                            } else {
                                // No newline so far
                                *current = Some(State::EventLoopProcessing {
                                    enqueue,
                                    dequeue,
                                    input,
                                    pos,
                                    invocation_key,
                                    captured: Some(captured),
                                });
                                Ok(())
                            }
                        } else {
                            // Not at character boundary
                            *current = Some(State::EventLoopProcessing {
                                enqueue,
                                dequeue,
                                input,
                                pos,
                                invocation_key,
                                captured: Some(captured),
                            });
                            Ok(())
                        }
                    }
                    None => {
                        *current = Some(State::EventLoopProcessing {
                            enqueue,
                            dequeue,
                            input,
                            pos,
                            invocation_key,
                            captured,
                        });
                        Err(must_read_first_error())
                    }
                }
            }
        }
    }

    pub async fn write_vectored<'a>(
        &mut self,
        bufs: &[IoSlice<'a>],
    ) -> anyhow::Result<(u64, ManagedStreamStatus)> {
        let mut total = 0;
        for buf in bufs {
            total += buf.len();
            self.write(buf).await?;
        }
        Ok((total as u64, ManagedStreamStatus::Open))
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub async fn writable(&self) -> anyhow::Result<()> {
        match &*self.current.lock().await {
            None => panic!("ManagedStandardIo is in an invalid state"),
            Some(State::Live) => Ok(()),
            Some(State::SingleCall { .. }) => Ok(()),
            Some(State::EventLoopIdle { .. }) => Err(must_read_first_error()),
            Some(State::EventLoopProcessing { captured, .. }) => match captured {
                None => Err(must_read_first_error()),
                Some(_) => Ok(()),
            },
        }
    }
}

fn read_from_bytes(input: &Bytes, pos: &mut usize, buf: &mut [u8]) -> (u64, ManagedStreamStatus) {
    let slice_len = min(input.len() - *pos, buf.len());
    let mut slice = input.slice(*pos..(*pos + slice_len));
    slice.copy_to_slice(&mut buf[0..slice_len]);
    *pos += slice_len;
    (
        slice_len as u64,
        if *pos >= input.len() {
            ManagedStreamStatus::Ended
        } else {
            ManagedStreamStatus::Open
        },
    )
}

pub fn disabled_stdin_error() -> anyhow::Error {
    anyhow!("standard input is disabled")
}

fn must_read_first_error() -> anyhow::Error {
    anyhow!("standard output is disabled until the next input is read")
}

fn must_write_first_error() -> anyhow::Error {
    anyhow!("standard input is disabled until writing a response for the previously read event")
}
