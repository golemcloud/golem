// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::app::build::external_command::execute_external_command;
use crate::app::context::ApplicationContext;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::app::{AppComponentName, DependencyType};
use crate::wasm_rpc_stubgen::wit_resolve::ExportedFunction;
use anyhow::{anyhow, Context};
use heck::ToLowerCamelCase;
use std::collections::{BTreeSet, HashMap};

pub fn componentize(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    log_action("Building", "components");
    let _indent = LogIndent::new();

    let components_to_build = components_to_build(ctx);
    for component_name in components_to_build {
        let component_properties = ctx
            .application
            .component_properties(&component_name, ctx.profile());

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

        let env_vars = build_step_env_vars(ctx, &component_name)
            .context("Failed to get env vars for build step")?;

        for build_step in &component_properties.build {
            execute_external_command(
                ctx,
                ctx.application.component_source_dir(&component_name),
                build_step,
                env_vars.clone(),
            )?;
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
            if dep.dep_type == DependencyType::Wasm && !components_to_build.contains(&dep.name) {
                components_to_build.insert(dep.name.clone());
                remaining.push(dep.name.clone());
            }
        }
    }
    components_to_build
}

fn build_step_env_vars(
    ctx: &ApplicationContext,
    component_name: &AppComponentName,
) -> anyhow::Result<HashMap<String, String>> {
    let result = HashMap::from_iter(vec![(
        "JCO_ASYNC_EXPORT_ARGS".to_string(),
        jco_async_export_args(ctx, component_name)?.join(" "),
    )]);

    Ok(result)
}

fn jco_async_export_args(
    ctx: &ApplicationContext,
    component_name: &AppComponentName,
) -> anyhow::Result<Vec<String>> {
    let resolved = ctx
        .wit
        .component(component_name)?
        .generated_wit_dir()
        .ok_or(anyhow!("Failed to get generated wit dir"))?;

    let exported_functions = resolved.exported_functions().context(format!(
        "Failed to look up exported_functions for component {component_name}"
    ))?;

    let mut result = Vec::new();

    for function in exported_functions {
        match function {
            ExportedFunction::Interface {
                interface_name,
                function_name,
            } => {
                // This is not a typo, it's a workaround for https://github.com/bytecodealliance/jco/issues/622
                result.push("--async-imports".to_string());
                result.push(format!("{interface_name}#{function_name}"));
            }
            ExportedFunction::InlineInterface {
                export_name,
                function_name,
            } => {
                // This is not a typo, it's a workaround for https://github.com/bytecodealliance/jco/issues/622
                result.push("--async-imports".to_string());
                let transformed = export_name.to_lower_camel_case();
                result.push(format!("{transformed}#{function_name}"));
            }
            ExportedFunction::InlineFunction {
                world_name,
                function_name,
            } => {
                result.push("--async-exports".to_string());
                result.push(format!("{world_name}#{function_name}"));
            }
        }
    }
    Ok(result)
}
