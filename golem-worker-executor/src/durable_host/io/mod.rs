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

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite};
use wasmtime_wasi::cli::{IsTerminal, Stderr, StdinStream, Stdout, StdoutStream};
use wasmtime_wasi::{InputStream, OutputStream, Pollable, StreamError, StreamResult};

pub mod error;
pub mod poll;
pub mod streams;

#[derive(Clone)]
pub struct ManagedStdIn;

impl ManagedStdIn {
    pub fn disabled() -> Self {
        Self
    }
}

impl IsTerminal for ManagedStdIn {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdinStream for ManagedStdIn {
    fn async_stream(&self) -> Box<dyn AsyncRead + Send + Sync> {
        Box::new(tokio::io::empty())
    }

    fn p2_stream(&self) -> Box<dyn InputStream> {
        Box::new(self.clone())
    }
}

#[async_trait]
impl Pollable for ManagedStdIn {
    async fn ready(&mut self) {}
}

#[async_trait]
impl InputStream for ManagedStdIn {
    fn read(&mut self, _size: usize) -> StreamResult<Bytes> {
        Err(StreamError::trap("standard input is disabled"))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct ManagedStdOut {
    state: Arc<ManagedStdOutState>,
}

struct ManagedStdOutState {
    stdout: Stdout,
}

impl ManagedStdOut {
    pub fn from_stdout(stdout: Stdout) -> Self {
        Self {
            state: Arc::new(ManagedStdOutState { stdout }),
        }
    }
}

impl IsTerminal for ManagedStdOut {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for ManagedStdOut {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        self.state.stdout.async_stream()
    }

    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }
}

#[async_trait]
impl Pollable for ManagedStdOut {
    async fn ready(&mut self) {
        self.state.stdout.p2_stream().ready().await
    }
}

#[async_trait]
impl OutputStream for ManagedStdOut {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.stdout.p2_stream().write(bytes)
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.state.stdout.p2_stream().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.state.stdout.p2_stream().check_write()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct ManagedStdErr {
    state: Arc<ManagedStdErrState>,
}

struct ManagedStdErrState {
    stderr: Stderr,
}

impl ManagedStdErr {
    pub fn from_stderr(stderr: Stderr) -> Self {
        Self {
            state: Arc::new(ManagedStdErrState { stderr }),
        }
    }
}

impl IsTerminal for ManagedStdErr {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for ManagedStdErr {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        self.state.stderr.async_stream()
    }

    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }
}

#[async_trait]
impl Pollable for ManagedStdErr {
    async fn ready(&mut self) {
        self.state.stderr.p2_stream().ready().await
    }
}

#[async_trait]
impl OutputStream for ManagedStdErr {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.stderr.p2_stream().write(bytes)
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.state.stderr.p2_stream().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.state.stderr.p2_stream().check_write()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
