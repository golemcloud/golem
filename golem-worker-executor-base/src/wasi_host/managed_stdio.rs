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
use tokio::sync::Mutex;

use golem_common::model::WorkerId;

use crate::services::invocation_queue::InvocationQueue;
use crate::workerctx::WorkerCtx;

pub struct ManagedStandardIo<Ctx: WorkerCtx> {
    current: Arc<Mutex<Option<State>>>,
    worker_id: WorkerId,
    invocation_queue: Arc<InvocationQueue<Ctx>>,
}

#[derive(Debug)]
enum State {
    Live,
    SingleCall {
        input: Bytes,
        pos: usize,
        captured: Vec<u8>,
    },
}

/// Maps to the type defined in the WASI IO package
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedStreamStatus {
    Open,
    Ended,
}

impl<Ctx: WorkerCtx> ManagedStandardIo<Ctx> {
    pub fn new(worker_id: WorkerId, invocation_queue: Arc<InvocationQueue<Ctx>>) -> Self {
        Self {
            current: Arc::new(Mutex::new(Some(State::Live))),
            worker_id,
            invocation_queue,
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
        let mut current = self.current.lock().await;

        match current
            .take()
            .expect("ManagedStandardIo is in an invalid state")
        {
            state @ State::Live => {
                *current = Some(state);
                Err(disabled_stdin_error())
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
                Ok(result)
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
        }
    }

    pub async fn readable(&self) -> anyhow::Result<()> {
        match &*self.current.lock().await {
            None => Err(disabled_stdin_error()),
            Some(State::Live) => Err(disabled_stdin_error()),
            Some(State::SingleCall { .. }) => Ok(()),
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

impl<Ctx: WorkerCtx> Clone for ManagedStandardIo<Ctx> {
    fn clone(&self) -> Self {
        Self {
            current: self.current.clone(),
            worker_id: self.worker_id.clone(),
            invocation_queue: self.invocation_queue.clone(),
        }
    }
}

pub fn disabled_stdin_error() -> anyhow::Error {
    anyhow!("standard input is disabled")
}
