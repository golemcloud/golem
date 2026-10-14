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

use crate::bridge_gen::rust::RustBridgeGeneratorConfig;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::app::{ApplicationComponentSelectMode, BuildConfig, CustomBridgeSdkTarget};
use crate::model::language::GuestLanguage;
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
        derive_rules: Vec<String>,
        rust_dependencies: Vec<String>,
    ) -> anyhow::Result<()> {
        validate_rust_generator_options(language, &derive_rules, &rust_dependencies)?;
        let rust_config = RustBridgeGeneratorConfig::from_cli(
            &derive_rules,
            &rust_dependencies,
            &crate::fs::current_dir_lexical()?,
        )?;
        self.ctx
            .app_handler()
            .build(
                &BuildConfig::new().with_custom_bridge_sdk_target(CustomBridgeSdkTarget {
                    agent_type_names: agent_type_names.into_iter().collect(),
                    target_language: language,
                    output_dir,
                    rust_config,
                }),
                component_names,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await?;

        self.ctx
            .log_handler()
            .log_output(crate::app::build::gen_bridge::GenerateBridgeResult { generated: true })?;

        Ok(())
    }
}

fn validate_rust_generator_options(
    language: Option<GuestLanguage>,
    derive_rules: &[String],
    rust_dependencies: &[String],
) -> anyhow::Result<()> {
    if (!derive_rules.is_empty() || !rust_dependencies.is_empty())
        && language.is_some_and(|language| language != GuestLanguage::Rust)
    {
        anyhow::bail!(
            "--derive-rule and --rust-dependency are only valid for Rust bridge generation"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn explicitly_selected_non_rust_language_rejects_rust_generator_options() {
        assert!(
            validate_rust_generator_options(
                Some(GuestLanguage::TypeScript),
                &[".*=Clone".into()],
                &[],
            )
            .is_err()
        );
        assert!(
            validate_rust_generator_options(
                Some(GuestLanguage::Scala),
                &[],
                &["anyhow = \"1\"".into()],
            )
            .is_err()
        );
        assert!(validate_rust_generator_options(None, &[".*=Clone".into()], &[]).is_ok());
    }
}
