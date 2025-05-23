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

use crate::command::cloud::CloudSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use std::sync::Arc;

pub mod account;
pub mod project;
pub mod token;

pub struct CloudCommandHandler {
    ctx: Arc<Context>,
}

impl CloudCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: CloudSubcommand) -> anyhow::Result<()> {
        match subcommand {
            CloudSubcommand::Project { subcommand } => {
                self.ctx
                    .cloud_project_handler()
                    .handle_command(subcommand)
                    .await
            }
            CloudSubcommand::Account { subcommand } => {
                self.ctx
                    .cloud_account_handler()
                    .handle_command(subcommand)
                    .await
            }
            CloudSubcommand::Token { subcommand } => {
                self.ctx
                    .cloud_token_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }
}
