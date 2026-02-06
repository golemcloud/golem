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

use crate::app::build::task_result_marker::ExtractAgentTypeMarkerHash;
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize};
use anyhow::Context;
use golem_common::model::agent::AgentType;
use golem_common::model::component::ComponentName;

pub async fn extract_and_store_agent_types(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> anyhow::Result<Vec<AgentType>> {
    let component = ctx.application().component(component_name);
    let wasm = component.agent_type_extraction_source_wasm();
    let extracted_agent_types = component.extracted_agent_types(&wasm);

    let agent_types = new_task_up_to_date_check(ctx)
        .with_task_result_marker(ExtractAgentTypeMarkerHash { component_name })?
        .with_sources(|| vec![&wasm])
        .with_targets(|| vec![&extracted_agent_types])
        .run_async_or_skip_returning(
            || async {
                log_action(
                    "Extracting",
                    format!(
                        "{} agent types from {}",
                        component_name.as_str().log_color_highlight(),
                        wasm.log_color_highlight()
                    ),
                );

                let agent_types = ctx
                    .wit()
                    .await
                    .get_or_extract_component_agent_types(component_name, &wasm)
                    .await?;

                fs::write_str(
                    &extracted_agent_types,
                    serde_json::to_string(&agent_types)
                        .context("Failed to serialize agent types")?,
                )?;

                Ok(agent_types)
            },
            || {
                log_skipping_up_to_date(format!(
                    "extracting {} agent types",
                    component_name.as_str().log_color_highlight(),
                ));
            },
        )
        .await?;

    match agent_types {
        Some(agent_types) => Ok(agent_types),
        None => {
            let agent_types = serde_json::from_str(&fs::read_to_string(&extracted_agent_types)?)
                .context("Failed to deserialize agent types")?;
            ctx.wit()
                .await
                .add_cached_component_agent_types(component_name, agent_types)
                .await
        }
    }
}
