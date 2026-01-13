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

use crate::app::build::add_metadata::add_metadata_to_selected_components;
use crate::app::build::componentize::componentize;
use crate::app::build::gen_bridge::gen_bridge;
use crate::app::build::gen_rpc::gen_rpc;
use crate::app::build::link::link;
use crate::app::context::ApplicationContext;
use crate::fs;
use crate::log::{log_warn_action, LogColorize};
use crate::model::app::AppBuildStep;
use anyhow::{anyhow, Context};
use std::path::Path;

pub mod add_metadata;
pub mod clean;
pub mod command;
pub mod componentize;
pub mod extract_agent_type;
pub mod gen_bridge;
pub mod gen_rpc;
pub mod link;
pub mod task_result_marker;
pub mod up_to_date_check;

pub async fn build_app(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    if ctx.config.should_run_step(AppBuildStep::GenRpc) {
        gen_rpc(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::Componentize) {
        componentize(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::Link) {
        link(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::AddMetadata) {
        add_metadata_to_selected_components(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::GenBridge) {
        gen_bridge(ctx).await?;
    }

    Ok(())
}

fn env_var_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|flag| {
            let flag = flag.to_lowercase();
            flag.starts_with("t") || flag == "1"
        })
        .unwrap_or_default()
}

fn delete_path_logged(context: &str, path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        log_warn_action(
            "Deleting",
            format!("{} {}", context, path.log_color_highlight()),
        );
        fs::remove(path).with_context(|| {
            anyhow!(
                "Failed to delete {}, path: {}",
                context.log_color_highlight(),
                path.log_color_highlight()
            )
        })?;
    }
    Ok(())
}
