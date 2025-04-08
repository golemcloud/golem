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
use crate::model::app::AppComponentName;
use crate::wasm_rpc_stubgen::wit_resolve::ExportedFunction;
use anyhow::{anyhow, Context};
use heck::ToLowerCamelCase;
use std::collections::HashMap;

pub fn componentize(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    log_action("Building", "components");
    let _indent = LogIndent::new();

    for component_name in ctx.selected_component_names() {
        let component_properties = ctx
            .application
            .component_properties(component_name, ctx.profile());

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

        let env_vars = build_step_env_vars(ctx, component_name)
            .context("Failed to get env vars for build step")?;

        for build_step in &component_properties.build {
            execute_external_command(
                ctx,
                ctx.application.component_source_dir(component_name),
                build_step,
                env_vars.clone(),
            )?;
        }
    }

    Ok(())
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
