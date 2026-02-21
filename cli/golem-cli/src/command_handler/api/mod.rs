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

use crate::command::api::ApiSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use std::sync::Arc;

pub mod deployment;
pub mod domain;
pub mod security_scheme;

pub struct ApiCommandHandler {
    ctx: Arc<Context>,
}

impl ApiCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiSubcommand) -> anyhow::Result<()> {
        match command {
            ApiSubcommand::Deployment { subcommand } => {
                self.ctx
                    .api_deployment_handler()
                    .handle_command(subcommand)
                    .await
            }
            ApiSubcommand::SecurityScheme { subcommand } => {
                self.ctx
                    .api_security_scheme_handler()
                    .handle_command(subcommand)
                    .await
            }
            ApiSubcommand::Domain { subcommand } => {
                self.ctx
                    .api_domain_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }
}
