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

use crate::context::Context;
use crate::model::repl::BridgeReplArgs;
use crate::process::ExitStatusExt;
use crate::{binary_path_to_string, GOLEM_EVCXR_REPL};
use std::sync::Arc;
use tokio::process::Command;

pub struct RustRepl {
    _ctx: Arc<Context>,
}

impl RustRepl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { _ctx: ctx }
    }

    pub async fn run(&self, args: BridgeReplArgs) -> anyhow::Result<()> {
        let result = Command::new(binary_path_to_string()?)
            .current_dir(args.repl_root_dir)
            .env(GOLEM_EVCXR_REPL, "1")
            .spawn()?
            .wait()
            .await?;

        result.check_exit_status()
    }
}
