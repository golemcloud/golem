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
use crate::app::build::task_result_marker::{LinkRpcMarkerHash, TaskResultMarker};
use crate::app::context::ApplicationContext;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::model::app::DependencyType;
use crate::wasm_rpc_stubgen::commands;
use itertools::Itertools;
use std::collections::BTreeSet;

pub async fn link(ctx: &ApplicationContext) -> anyhow::Result<()> {
    log_action("Linking", "dependencies");
    let _indent = LogIndent::new();

    for component_name in ctx.selected_component_names() {
        let static_dependencies = ctx
            .application
            .component_dependencies(component_name)
            .iter()
            .filter(|dep| dep.dep_type == DependencyType::StaticWasmRpc)
            .collect::<BTreeSet<_>>();
        let library_dependencies = ctx
            .application
            .component_dependencies(component_name)
            .iter()
            .filter(|dep| dep.dep_type == DependencyType::Wasm)
            .collect::<BTreeSet<_>>();
        let dynamic_dependencies = ctx
            .application
            .component_dependencies(component_name)
            .iter()
            .filter(|dep| dep.dep_type == DependencyType::DynamicWasmRpc)
            .collect::<BTreeSet<_>>();

        let mut wasms_to_compose_with = Vec::new();
        for static_dep in &static_dependencies {
            let path = ctx.resolve_binary_component_source(static_dep).await?;
            wasms_to_compose_with.push(path);
        }
        for library_dep in &library_dependencies {
            let path = ctx.resolve_binary_component_source(library_dep).await?;
            wasms_to_compose_with.push(path);
        }

        let component_wasm = ctx
            .application
            .component_wasm(component_name, ctx.build_profile());
        let linked_wasm = ctx.application.component_temp_linked_wasm(component_name);

        let task_result_marker = TaskResultMarker::new(
            &ctx.application.task_result_marker_dir(),
            LinkRpcMarkerHash {
                component_name,
                dependencies: &static_dependencies,
            },
        )?;

        if !dynamic_dependencies.is_empty() {
            log_action(
                "Found",
                format!(
                    "dynamic WASM RPC dependencies ({}) for {}",
                    dynamic_dependencies
                        .iter()
                        .map(|s| s.source.to_string().log_color_highlight())
                        .join(", "),
                    component_name.as_str().log_color_highlight(),
                ),
            );
        }

        if !static_dependencies.is_empty() {
            log_action(
                "Found",
                format!(
                    "static WASM RPC dependencies ({}) for {}",
                    static_dependencies
                        .iter()
                        .map(|s| s.source.to_string().log_color_highlight())
                        .join(", "),
                    component_name.as_str().log_color_highlight(),
                ),
            );
        }

        if !library_dependencies.is_empty() {
            log_action(
                "Found",
                format!(
                    "static WASM library dependencies ({}) for {}",
                    library_dependencies
                        .iter()
                        .map(|s| s.source.to_string().log_color_highlight())
                        .join(", "),
                    component_name.as_str().log_color_highlight(),
                ),
            );
        }

        if is_up_to_date(
            ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date(),
            || {
                let mut inputs = wasms_to_compose_with.clone();
                inputs.push(component_wasm.clone());
                inputs
            },
            || [linked_wasm.clone()],
        ) {
            log_skipping_up_to_date(format!(
                "linking dependencies for {}",
                component_name.as_str().log_color_highlight(),
            ));
            continue;
        }

        task_result_marker.result(
            async {
                if wasms_to_compose_with.is_empty() {
                    log_action(
                        "Copying",
                        format!(
                            "{} without linking, no static dependencies were found",
                            component_name.as_str().log_color_highlight(),
                        ),
                    );
                    fs::copy(&component_wasm, &linked_wasm).map(|_| ())
                } else {
                    log_action(
                        "Linking",
                        format!(
                            "static dependencies ({}) into {}",
                            static_dependencies
                                .iter()
                                .map(|s| s.source.to_string().log_color_highlight())
                                .chain(
                                    library_dependencies
                                        .iter()
                                        .map(|s| s.source.to_string().log_color_highlight()),
                                )
                                .join(", "),
                            component_name.as_str().log_color_highlight(),
                        ),
                    );
                    let _indent = LogIndent::new();

                    commands::composition::compose(
                        ctx.application
                            .component_wasm(component_name, ctx.build_profile())
                            .as_path(),
                        &wasms_to_compose_with,
                        linked_wasm.as_path(),
                    )
                    .await
                }
            }
            .await,
        )?;
    }

    Ok(())
}
