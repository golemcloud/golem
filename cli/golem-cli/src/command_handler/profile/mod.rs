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

pub mod config;

use crate::command::profile::ProfileSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use std::sync::Arc;

pub struct ProfileCommandHandler {
    ctx: Arc<Context>,
}

impl ProfileCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&mut self, subcommand: ProfileSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ProfileSubcommand::New { .. } => {
                todo!()
            }
            ProfileSubcommand::List { .. } => todo!(),
            ProfileSubcommand::Switch { .. } => todo!(),
            ProfileSubcommand::Get { .. } => todo!(),
            ProfileSubcommand::Delete { .. } => todo!(),
            ProfileSubcommand::Config {
                profile_name,
                subcommand,
            } => {
                self.ctx
                    .profile_config_handler()
                    .handler_subcommand(profile_name, subcommand)
                    .await
            }
        }
    }
}
