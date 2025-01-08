// Copyright 2024-2025 Golem Cloud
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

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use wasmtime_wasi::{
    HostInputStream, HostOutputStream, Stderr, StdinStream, Stdout, StdoutStream, StreamError,
    StreamResult, Subscribe,
};

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

impl StdinStream for ManagedStdIn {
    fn stream(&self) -> Box<dyn HostInputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[async_trait]
impl Subscribe for ManagedStdIn {
    async fn ready(&mut self) {}
}

#[async_trait]
impl HostInputStream for ManagedStdIn {
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

impl StdoutStream for ManagedStdOut {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[async_trait]
impl Subscribe for ManagedStdOut {
    async fn ready(&mut self) {
        self.state.stdout.stream().ready().await
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdOut {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.stdout.stream().write(bytes.clone())
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.state.stdout.stream().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.state.stdout.stream().check_write()
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

impl StdoutStream for ManagedStdErr {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[async_trait]
impl Subscribe for ManagedStdErr {
    async fn ready(&mut self) {
        self.state.stderr.stream().ready().await
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdErr {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.stderr.stream().write(bytes.clone())
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.state.stderr.stream().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.state.stderr.stream().check_write()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
