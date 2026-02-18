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

use crate::evcxr_repl::repl::Repl;
use crate::log::log_anyhow_error;
use std::process::ExitCode;

mod cli_repl_interop;
mod config;
mod log;
mod repl;

pub use config::ReplConfig;
pub use config::REPL_CONFIG_FILE_NAME;

pub fn main() -> ExitCode {
    match Repl::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            log_anyhow_error(&err);
            ExitCode::FAILURE
        }
    }
}
