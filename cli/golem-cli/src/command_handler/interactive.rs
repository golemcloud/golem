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

use crate::context::Context;
use crate::model::text::fmt::log_warn;
use crate::model::ComponentName;
use anyhow::anyhow;
use colored::Colorize;
use golem_wasm_rpc_stubgen::log::{log_warn_action, LogColorize};
use inquire::{Confirm, InquireError};
use std::sync::Arc;

pub struct InteractiveHandler {
    ctx: Arc<Context>,
}

impl InteractiveHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn confirm_auto_deploy_component(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Component {} was not found between deployed components, do you want to deploy it, then continue?",
                component_name.0.log_color_highlight()
            ),
        )
    }

    pub fn confirm_redeploy_workers(&self, number_of_workers: usize) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Redeploying will {} then recreate {} worker(s), do you want to continue?",
                "delete".log_color_warn(),
                number_of_workers.to_string().log_color_highlight()
            ),
        )
    }

    fn confirm<M: AsRef<str>>(&self, default: bool, message: M) -> anyhow::Result<bool> {
        const YES_FLAG_HINT: &str = "To automatically confirm such questions use the '--yes' flag.";

        if self.ctx.yes() {
            log_warn_action(
                "Auto confirming",
                format!("question: \"{}\"", message.as_ref().cyan()),
            );
            return Ok(true);
        }

        match Confirm::new(message.as_ref())
            .with_help_message(YES_FLAG_HINT)
            .with_default(default)
            .prompt()
        {
            Ok(result) => Ok(result),
            Err(error) => match error {
                InquireError::NotTTY => {
                    log_warn("The current input device is not a teletype,\ndefaulting to \"false\" as answer for the confirm question.");
                    Ok(false)
                }
                other => Err(anyhow!("{}", other)),
            },
        }
    }
}
