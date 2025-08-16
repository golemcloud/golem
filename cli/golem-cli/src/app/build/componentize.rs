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

use crate::app::build::command::execute_build_command;
use crate::app::context::ApplicationContext;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::app::{AppComponentName, DependencyType};
use std::collections::{BTreeSet, HashMap};

pub async fn componentize(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    log_action("Building", "components");
    let _indent = LogIndent::new();

    let components_to_build = components_to_build(ctx);
    for component_name in components_to_build {
        let component_properties = ctx
            .application
            .component_properties(&component_name, ctx.build_profile());

        if component_properties.build.is_empty() {
            log_warn_action(
                "Skipping",
                format!(
                    "building {}, no build steps",
                    component_name.as_str().log_color_highlight(),
                ),
            );
            continue;
        }

        log_action(
            "Building",
            format!("{}", component_name.as_str().log_color_highlight()),
        );
        let _indent = LogIndent::new();

        let env_vars = HashMap::new();
        for build_step in component_properties.build.clone() {
            execute_build_command(ctx, &component_name, &build_step, env_vars.clone()).await?;
        }
    }

    Ok(())
}

fn components_to_build(ctx: &ApplicationContext) -> BTreeSet<AppComponentName> {
    let mut components_to_build = BTreeSet::new();
    let mut remaining: Vec<_> = ctx.selected_component_names().iter().cloned().collect();

    while let Some(component_name) = remaining.pop() {
        components_to_build.insert(component_name.clone());

        for dep in ctx.application.component_dependencies(&component_name) {
            if dep.dep_type == DependencyType::Wasm {
                if let Some(dep) = dep.as_dependent_app_component() {
                    if !components_to_build.contains(&dep.name) {
                        components_to_build.insert(dep.name.clone());
                        remaining.push(dep.name.clone());
                    }
                }
            }
        }
    }
    components_to_build
}
