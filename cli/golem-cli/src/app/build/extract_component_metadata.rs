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

use crate::app::build::task_result_marker::ExtractComponentMetadataMarkerHash;
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{LogColorize, log_action, log_skipping_up_to_date};
use anyhow::Context;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::component::ComponentName;
use golem_common::schema::AgentTypeSchema;

pub async fn extract_and_store_component_metadata(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> anyhow::Result<ExtractedComponentMetadata> {
    let component = ctx.application().component(component_name);
    let wasm = component.agent_type_extraction_source_wasm();
    let extracted_component_metadata = component.extracted_component_metadata(&wasm);

    let metadata = new_task_up_to_date_check(ctx)
        .with_task_result_marker(ExtractComponentMetadataMarkerHash { component_name })?
        .with_sources(|| vec![&wasm])
        .with_targets(|| vec![&extracted_component_metadata])
        .run_async_or_skip_returning(
            || async {
                log_action(
                    "Extracting",
                    format!(
                        "{} agent types and tools from {}",
                        component_name.as_str().log_color_highlight(),
                        wasm.log_color_highlight()
                    ),
                );

                let metadata = ctx
                    .component_metadata()
                    .get_or_extract_component_metadata(component_name, &wasm)
                    .await?;

                fs::write_str(
                    &extracted_component_metadata,
                    serde_json::to_string(&metadata)
                        .context("Failed to serialize component metadata")?,
                )?;

                Ok(metadata)
            },
            || {
                log_skipping_up_to_date(format!(
                    "extracting {} agent types and tools",
                    component_name.as_str().log_color_highlight(),
                ));
            },
        )
        .await?;

    match metadata {
        Some(metadata) => Ok(metadata),
        None => {
            let metadata =
                serde_json::from_str(&fs::read_to_string(&extracted_component_metadata)?)
                    .context("Failed to deserialize component metadata")?;
            ctx.component_metadata()
                .add_cached_component_metadata(component_name, metadata)
                .await
        }
    }
}

/// Same as [`extract_and_store_component_metadata`], but returns only the
/// extracted agent types.
pub async fn extract_and_store_agent_types(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> anyhow::Result<Vec<AgentTypeSchema>> {
    Ok(extract_and_store_component_metadata(ctx, component_name)
        .await?
        .agent_types)
}
