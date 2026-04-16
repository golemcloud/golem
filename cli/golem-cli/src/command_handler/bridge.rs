// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::bridge_gen::DeriveRule;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::GuestLanguage;
use crate::model::app::{ApplicationComponentSelectMode, BuildConfig, CustomBridgeSdkTarget};
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
        derive_rules_raw: Vec<String>,
    ) -> anyhow::Result<()> {
        let derive_rules = parse_derive_rules(derive_rules_raw)?;
        self.ctx
            .app_handler()
            .build(
                &BuildConfig::new().with_custom_bridge_sdk_target(CustomBridgeSdkTarget {
                    agent_type_names: agent_type_names.into_iter().collect(),
                    target_language: language,
                    output_dir,
                    derive_rules,
                }),
                component_names,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await
    }
}

/// Parses CLI `--derive-rule` values in the format `REGEX=Derive1,Derive2`.
fn parse_derive_rules(raw: Vec<String>) -> anyhow::Result<Vec<DeriveRule>> {
    raw.into_iter()
        .map(|s| {
            let (pattern, derives_str) = s.split_once('=').ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid derive rule format: '{}'. Expected REGEX=Derive1,Derive2",
                    s
                )
            })?;
            Ok(DeriveRule {
                pattern: pattern.to_string(),
                derives: derives_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect(),
            })
        })
        .collect()
}
