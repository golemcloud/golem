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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::app::{ApplicationComponentSelectMode, BuildConfig, CustomBridgeSdkTarget};
use crate::model::GuestLanguage;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentName;
use std::path::PathBuf;
use std::sync::Arc;

pub struct BridgeCommandHandler {
    ctx: Arc<Context>,
}

impl BridgeCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn cmd_generate_bridge(
        &self,
        language: Option<GuestLanguage>,
        component_names: Vec<ComponentName>,
        agent_type_names: Vec<AgentTypeName>,
        output_dir: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        self.ctx
            .app_handler()
            .build(
                &BuildConfig::new().with_custom_bridge_sdk_target(CustomBridgeSdkTarget {
                    agent_type_names: agent_type_names.into_iter().collect(),
                    target_language: language,
                    output_dir,
                }),
                component_names,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await
    }
}
