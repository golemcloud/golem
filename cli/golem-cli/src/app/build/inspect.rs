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

use super::command::{CommonDependencies, common_dependencies_for_tool};
use super::plan::{
    BuildAction, BuildEffect, BuildOperation, BuildPlan, BuildPlanLimits, BuildRequirement,
    BuildStage,
};
use crate::app::context::BuildContext;
use crate::log::{log_action, logln};
use crate::model::app::AppBuildStep;
use anyhow::{Context, ensure};
use golem_common::model::component::ComponentName;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn inspect_build(
    ctx: &BuildContext<'_>,
    with_dependencies: bool,
) -> anyhow::Result<BuildPlan> {
    let mut components = ctx.application_context().selected_component_names().clone();
    if with_dependencies {
        super::expand_build_component_dependencies(
            ctx,
            &mut components,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )?;
        // Component metadata can select a local middleware provider later. Check
        // its declared candidates now, before either project or grant writes.
        if super::needs_dependency_guest_bridges(
            ctx,
            &components.iter().cloned().collect::<Vec<_>>(),
        ) {
            for declaration in ctx.application().tool_middleware_declarations().values() {
                if declaration.value.release.is_none() {
                    if let Some(name) = &declaration.value.component {
                        components.insert(name.clone());
                    } else {
                        components.extend(ctx.application().component_names().cloned());
                    }
                }
            }
        }
        super::expand_build_component_dependencies(
            ctx,
            &mut components,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )?;
    }
    let stages = inspect_stages(ctx, components.iter(), with_dependencies)?;
    BuildPlan::new(stages, BuildPlanLimits::default())
}

pub(super) fn inspect_stages<'a>(
    ctx: &BuildContext<'_>,
    components: impl Iterator<Item = &'a ComponentName>,
    with_dependencies: bool,
) -> anyhow::Result<Vec<BuildStage>> {
    let limits = BuildPlanLimits::default();
    let mut stages = Vec::new();
    let mut actions = 0;
    let mut edges = 0;
    for name in components {
        ensure!(
            stages.len() < limits.components,
            "Build plan component limit exceeded"
        );
        let stage = inspect_stage(ctx, name, with_dependencies)?;
        actions += stage.actions.len();
        edges += stage.dependencies.len();
        ensure!(
            actions <= limits.actions,
            "Build plan action limit exceeded"
        );
        ensure!(edges <= limits.edges, "Build plan edge limit exceeded");
        stages.push(stage);
    }
    Ok(stages)
}

fn inspect_stage(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
    with_dependencies: bool,
) -> anyhow::Result<BuildStage> {
    let component = ctx.application().component(component_name);
    let mut actions = Vec::new();
    let go_component =
        component.guest_language() == Some(crate::model::language::GuestLanguage::Go);
    let inspect_commands =
        ctx.should_run_step(AppBuildStep::Build) || ctx.should_run_step(AppBuildStep::Check);
    for command in component
        .build_commands()
        .iter()
        .filter(|_| inspect_commands)
    {
        let mut action = BuildAction::command(component.component_dir(), command)?;
        for requirement in action.requirements.clone() {
            let BuildRequirement::HostProgram { program, directory } = &requirement else {
                continue;
            };
            if go_component && crate::process::normalized_program_name(program) == "go" {
                action.requirements.remove(&requirement);
                action
                    .requirements
                    .insert(BuildRequirement::ManagedGoToolchain);
            }
            let preparation = common_dependencies_for_tool(ctx, program, directory, go_component)?;
            let Some(preparation) = preparation else {
                continue;
            };
            let (program, directory, inputs, outputs, managed_go) = match preparation {
                CommonDependencies::Npm(root) => (
                    "npm",
                    root,
                    vec!["package.json", "package-lock.json"],
                    vec!["node_modules", "package-lock.json"],
                    false,
                ),
                CommonDependencies::Moon(root) => (
                    "moon",
                    root,
                    vec!["moon.mod", "moon.work"],
                    vec![".mooncakes"],
                    false,
                ),
                CommonDependencies::Go(root) => (
                    "go",
                    root,
                    vec!["go.mod", "go.sum"],
                    vec!["go.mod", "go.sum"],
                    true,
                ),
            };
            actions.push(BuildAction {
                inputs: inputs.iter().map(|path| directory.join(path)).collect(),
                outputs: outputs.iter().map(|path| directory.join(path)).collect(),
                requirements: BTreeSet::from([if managed_go {
                    BuildRequirement::ManagedGoToolchain
                } else {
                    BuildRequirement::HostProgram {
                        program: program.into(),
                        directory: directory.clone(),
                    }
                }]),
                effects: BTreeSet::from([
                    BuildEffect::ReadFiles,
                    BuildEffect::WriteFiles,
                    BuildEffect::FetchDependencies,
                    BuildEffect::HostProcess,
                ]),
                operation: BuildOperation::Dependencies {
                    program: program.into(),
                    directory,
                },
            });
        }
        actions.push(action);
        ensure!(
            actions.len() <= BuildPlanLimits::default().actions,
            "Build plan action limit exceeded"
        );
    }
    if !inspect_commands || component.build_commands().is_empty() {
        let wasm = component.wasm();
        actions.push(BuildAction {
            inputs: vec![wasm.clone()],
            outputs: Vec::new(),
            requirements: BTreeSet::from([BuildRequirement::ExistingFile { path: wasm }]),
            effects: BTreeSet::from([BuildEffect::ReadFiles]),
            operation: BuildOperation::Prebuilt,
        });
    }
    let dependencies = if with_dependencies {
        super::component_guest_bridge_requirements(ctx, component_name)
            .iter()
            .filter_map(|dependency| dependency.component_name().cloned())
            .collect()
    } else {
        BTreeSet::new()
    };
    let mut stage = BuildStage {
        component: component_name.clone(),
        dependencies,
        actions,
    };
    if with_dependencies {
        let wasm = component.wasm();
        stage.actions.push(BuildAction::metadata(
            wasm.clone(),
            component.extracted_component_metadata(&wasm),
        ));
    }
    Ok(stage)
}

/// Host resolution stays at the Standalone boundary. Tool execution must use
/// the shared program resolver, not reinterpret these host observations.
pub(super) fn check_host_requirements(plan: &BuildPlan) -> anyhow::Result<()> {
    for requirement in plan.requirements() {
        if let BuildRequirement::ExistingFile { path } = requirement {
            ensure!(
                path.is_file(),
                "Build requires existing file {}; supply the artifact or declare its build commands",
                path.display()
            );
        }
        if matches!(requirement, BuildRequirement::ManagedGoToolchain) {
            super::go_toolchain::installed_go_toolchain()?;
        }
        if let BuildRequirement::HostProgram { program, directory } = requirement {
            let resolved = crate::process::resolve_command_for_execution(program, Some(directory))
                .with_context(|| format!("Build requires program '{program}' in {}; install it or repair the build command", directory.display()))?;
            let path = if resolved.program.is_absolute() {
                resolved.program
            } else {
                directory.join(resolved.program)
            };
            ensure!(
                path.is_file(),
                "Build requires program '{program}', but {} is not a file; install it or repair the build command",
                path.display()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                ensure!(
                    std::fs::metadata(&path)?.permissions().mode() & 0o111 != 0,
                    "Build program {} is not executable",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

pub(super) fn log_build_plan(plan: &BuildPlan) {
    log_action("Planned", "checked host build stages (no files changed)");
    for stage in plan.stages() {
        logln(format!(
            "  {}: {} actions; dependencies: {}",
            stage.component,
            stage.actions.len(),
            stage
                .dependencies
                .iter()
                .map(ComponentName::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for action in &stage.actions {
            for requirement in &action.requirements {
                logln(format!("    requires {requirement:?}"));
            }
        }
    }
    logln(
        "Preflight checks declared requirements, not whether arbitrary user build code will succeed.",
    );
}
