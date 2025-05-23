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

use crate::app::build::is_up_to_date;
use crate::app::build::task_result_marker::{AddMetadataMarkerHash, TaskResultMarker};
use crate::app::context::ApplicationContext;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::wasm_rpc_stubgen::commands::metadata::add_metadata;

pub async fn add_metadata_to_selected_components(
    ctx: &mut ApplicationContext,
) -> anyhow::Result<()> {
    log_action("Adding", "metadata to components");
    let _indent = LogIndent::new();

    for component_name in ctx.selected_component_names() {
        let linked_wasm = ctx.application.component_temp_linked_wasm(component_name);
        let final_linked_wasm = ctx
            .application
            .component_linked_wasm(component_name, ctx.build_profile());

        let root_package_name = ctx.wit.root_package_name(component_name)?;

        let task_result_marker = TaskResultMarker::new(
            &ctx.application.task_result_marker_dir(),
            AddMetadataMarkerHash {
                component_name,
                root_package_name: root_package_name.clone(),
            },
        )?;

        if is_up_to_date(
            ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date(),
            || vec![linked_wasm.clone()],
            || [final_linked_wasm.clone()],
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
                add_metadata(&linked_wasm, root_package_name, &final_linked_wasm)
            }
            .await,
        )?;
    }

    Ok(())
}
