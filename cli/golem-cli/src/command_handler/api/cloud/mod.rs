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

pub mod certificate;
pub mod domain;

use crate::command::api::cloud::ApiCloudSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use std::sync::Arc;

pub struct ApiCloudCommandHandler {
    ctx: Arc<Context>,
}

impl ApiCloudCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiCloudSubcommand) -> anyhow::Result<()> {
        match command {
            ApiCloudSubcommand::Domain { subcommand } => {
                self.ctx
                    .api_cloud_domain_handler()
                    .handle_command(subcommand)
                    .await
            }
            ApiCloudSubcommand::Certificate { subcommand } => {
                self.ctx
                    .api_cloud_certificate_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }
}
