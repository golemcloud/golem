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
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::wasm_metadata::{AddMetadata, AddMetadataField};
use anyhow::Context;
use golem_common::model::component::ComponentName;
use std::path::Path;
use wit_parser::PackageName;

fn component_name_to_package_name(component_name: &ComponentName) -> PackageName {
    let name_str = component_name.as_str();
    let (namespace, name) = if let Some((ns, n)) = name_str.split_once(':') {
        (ns.to_string(), n.to_string())
    } else {
        ("component".to_string(), name_str.to_string())
    };
    PackageName {
        namespace,
        name,
        version: None,
    }
}

fn add_metadata(
    source: &impl AsRef<Path>,
    root_package_name: PackageName,
    target: &impl AsRef<Path>,
) -> anyhow::Result<()> {
    let wasm = fs::read(source).context("Failed reading linked WASM")?;

    let mut metadata = AddMetadata::default();
    metadata.name = AddMetadataField::Set(format!(
        "{}:{}",
        root_package_name.namespace, root_package_name.name
    ));
    metadata.version = match &root_package_name.version {
        None => AddMetadataField::Clear,
        Some(v) => AddMetadataField::Set(crate::wasm_metadata::Version::new(v.to_string())),
    };

    let updated_wasm = metadata
        .to_wasm(&wasm)
        .context("Adding name and version metadata to the linked WASM")?;

    fs::create_dir_all(fs::parent_or_err(target.as_ref())?)?;

    fs::write(target, &updated_wasm).context("Failed writing final linked WASM")?;
    Ok(())
}

pub async fn add_metadata_to_selected_components(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    log_action("Adding", "metadata to components");
    let _indent = LogIndent::new();

    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        let temp_linked_wasm = component.temp_linked_wasm();
        let final_linked_wasm = component.final_linked_wasm();

        let root_package_name = component_name_to_package_name(component_name);

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
