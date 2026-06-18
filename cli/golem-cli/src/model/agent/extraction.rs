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

use crate::model::text::fmt::format_stderr;
use anyhow::anyhow;
use golem_common::model::agent::AgentType;
use itertools::Itertools;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use wasmtime_wasi::cli::AsyncStdoutStream;

/// In-memory async writer used to capture the worker's stdout/stderr while
/// running agent type extraction. This intentionally avoids the
/// `wasmtime_wasi::p2::pipe::MemoryOutputPipe` utility because we are
/// removing every direct dependency on the p2 surface.
#[derive(Clone, Default)]
struct MemoryWriter(Arc<Mutex<Vec<u8>>>);

impl MemoryWriter {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn contents(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl AsyncWrite for MemoryWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Extracts the implemented agent types from the given WASM component, assuming it implements the `golem:agent/guest` interface.
/// If it does not, it fails.
pub async fn extract_agent_types(
    wasm_path: &Path,
    enable_wasmtime_fs_cache: bool,
) -> anyhow::Result<Vec<AgentType>> {
    let stdout_buf = MemoryWriter::new();
    let stderr_buf = MemoryWriter::new();
    let stdout_stream = AsyncStdoutStream::new(64 * 1024, stdout_buf.clone());
    let stderr_stream = AsyncStdoutStream::new(64 * 1024, stderr_buf.clone());

    golem_common::model::agent::extraction::extract_agent_types_with_streams(
        wasm_path,
        Some(stdout_stream),
        Some(stderr_stream),
        true,
        enable_wasmtime_fs_cache,
    )
    .await
    .map_err(|err| {
        let stdout_contents = stdout_buf.contents();
        let stderr_contents = stderr_buf.contents();
        let stdout = String::from_utf8_lossy(&stdout_contents);
        let stderr = String::from_utf8_lossy(&stderr_contents);

        if stderr.contains("JavaScript error:") || stderr.contains("JavaScript exception:") {
            let stderr = stderr
                .lines()
                .filter(|line| {
                    !line.starts_with("thread '<unnamed>' panicked at ")
                        && !line.starts_with("stack backtrace:")
                        && !line.starts_with("note: Some details are omitted,")
                })
                .join("\n");
            return anyhow!(format_stderr(&stderr));
        }

        println!("{}", stdout);
        eprintln!("{}", stderr);

        err
    })
}
