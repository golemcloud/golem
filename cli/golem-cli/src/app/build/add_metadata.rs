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

use crate::app::build::task_result_marker::{AddMetadataMarkerHash, TaskResultMarker};
use crate::app::build::up_to_date_check::is_up_to_date;
use crate::app::context::BuildContext;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::wasm_rpc_stubgen::commands::metadata::add_metadata;

pub async fn add_metadata_to_selected_components(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    log_action("Adding", "metadata to components");
    let _indent = LogIndent::new();

    let wit = ctx.wit().await;
    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        let temp_linked_wasm = component.temp_linked_wasm();
        let final_linked_wasm = component.final_linked_wasm();

        let root_package_name = wit.root_package_name(component_name)?;

        let task_result_marker = TaskResultMarker::new(
            &ctx.application().task_result_marker_dir(),
            AddMetadataMarkerHash {
                component_name,
                root_package_name: root_package_name.clone(),
            },
        )?;

        if is_up_to_date(
            ctx.skip_up_to_date_checks() || !task_result_marker.is_up_to_date(),
            || [&temp_linked_wasm],
            || [&final_linked_wasm],
        ) {
            log_skipping_up_to_date(format!(
                "adding metadata to {}",
                component_name.as_str().log_color_highlight(),
            ));
            continue;
        }

        task_result_marker.result(
            async {
                log_action(
                    "Adding",
                    format!(
                        "metadata to {}",
                        component_name.as_str().log_color_highlight()
                    ),
                );
                add_metadata(&temp_linked_wasm, root_package_name, &final_linked_wasm)
            }
            .await,
        )?;
    }

    Ok(())
}
