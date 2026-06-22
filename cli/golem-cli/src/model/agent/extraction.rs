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
use golem_common::schema::AgentTypeSchema;
use itertools::Itertools;
use std::path::Path;
use wasmtime_wasi::p2::pipe;

/// Maps an extraction error to a user-facing error, draining the captured
/// stdout/stderr pipes and surfacing JavaScript errors cleanly.
fn map_extraction_error(
    stdout: pipe::MemoryOutputPipe,
    stderr: pipe::MemoryOutputPipe,
    err: anyhow::Error,
) -> anyhow::Error {
    let stdout_contents = stdout.contents();
    let stderr_contents = stderr.contents();
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
}

/// Extracts the implemented agent types from the given WASM component, assuming it implements the `golem:agent/guest` interface.
/// If it does not, it fails.
///
/// Returns the schema-native [`AgentTypeSchema`] model (the discover-agent-types
/// wasm export already produces schema-native wire types).
pub async fn extract_agent_type_schemas(
    wasm_path: &Path,
    enable_wasmtime_fs_cache: bool,
) -> anyhow::Result<Vec<AgentTypeSchema>> {
    let stdout = pipe::MemoryOutputPipe::new(usize::MAX);
    let stderr = pipe::MemoryOutputPipe::new(usize::MAX);

    golem_common::model::agent::extraction::extract_agent_type_schemas_with_streams(
        wasm_path,
        Some(stdout.clone()),
        Some(stderr.clone()),
        true,
        enable_wasmtime_fs_cache,
    )
    .await
    .map_err(|err| map_extraction_error(stdout, stderr, err))
}
