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

use crate::command::profile::config::ProfileConfigSubcommand;
use crate::config::{Config, ProfileName};
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::model::text::fmt::log_error;
use crate::model::Format;
use anyhow::bail;
use golem_wasm_rpc_stubgen::log::log_action;
use std::sync::Arc;

pub struct ProfileConfigCommandHandler {
    ctx: Arc<Context>,
}

impl ProfileConfigCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handler_subcommand(
        &mut self,
        profile_name: ProfileName,
        subcommand: ProfileConfigSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            ProfileConfigSubcommand::SetFormat { format } => {
                self.cmd_set_format(profile_name, format)
            }
        }
    }

    fn cmd_set_format(&mut self, profile_name: ProfileName, format: Format) -> anyhow::Result<()> {
        match Config::get_profile(&profile_name, self.ctx.config_dir())? {
            Some(mut profile) => {
                profile.get_config_mut().default_format = format;

                log_action(
                    "Updating",
                    format!(
                        "profile's default format for {} to {}",
                        &profile_name, format
                    ),
                );
                Config::set_profile(profile_name, profile, self.ctx.config_dir())?;
                log_action("Updated", "");

                Ok(())
            }
            None => {
                log_error(format!("Profile {} not found", profile_name));
                // TODO: show available profiles
                bail!(NonSuccessfulExit);
            }
        }
    }
}
