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

use crate::log::{log_action, LogColorize};
use crate::model::text::fmt::format_stderr;
use anyhow::anyhow;
use golem_common::model::agent::AgentType;
use itertools::Itertools;
use std::path::Path;
use wasmtime_wasi::p2::pipe;

/// Extracts the implemented agent types from the given WASM component, assuming it implements the `golem:agent/guest` interface.
/// If it does not, it fails.
pub async fn extract_agent_types(
    wasm_path: &Path,
    enable_wasmtime_fs_cache: bool,
) -> anyhow::Result<Vec<AgentType>> {
    log_action(
        "Extracting",
        format!(
            "agent types from {}",
            wasm_path
                .to_string_lossy()
                .to_string()
                .log_color_highlight()
        ),
    );

    let stdout = pipe::MemoryOutputPipe::new(usize::MAX);
    let stderr = pipe::MemoryOutputPipe::new(usize::MAX);

    golem_common::model::agent::extraction::extract_agent_types_with_streams(
        wasm_path,
        Some(stdout.clone()),
        Some(stderr.clone()),
        true,
        enable_wasmtime_fs_cache,
    )
    .await
    .map_err(|err| {
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
    })
}
