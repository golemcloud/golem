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

use super::command::possible_common_dependencies_for_tool;
use super::plan::{
    BuildAction, BuildEffect, BuildOperation, BuildPlan, BuildPlanLimits, BuildRequirement,
    BuildSelectionRule, BuildStage, BuildValue, CommonDependencies,
};
use crate::app::context::BuildContext;
use crate::log::{log_action, logln};
use crate::model::app::{AppBuildStep, ApplicationConfig};
use anyhow::{Context, ensure};
use golem_common::model::component::ComponentName;
use std::collections::{BTreeMap, BTreeSet};

fn inspect_selection(
    ctx: &BuildContext<'_>,
    with_dependencies: bool,
) -> anyhow::Result<(BTreeSet<ComponentName>, Vec<BuildSelectionRule>)> {
    let selected = ctx.application_context().selected_component_names().clone();
    let mut components = selected.clone();
    let mut rules = Vec::new();
    if with_dependencies {
        rules = super::manifest_middleware_selection_rules(ctx)?;
        components.extend(
            rules
                .iter()
                .flat_map(|rule| rule.components.iter().cloned()),
        );
        let owners = super::build_owner_components(ctx);
        let mut inspected = BTreeSet::new();
        loop {
            ensure!(
                components.len() <= BuildPlanLimits::default().components,
                "Build plan component limit exceeded"
            );
            let pending = components
                .difference(&inspected)
                .cloned()
                .collect::<Vec<_>>();
            if pending.is_empty() {
                break;
            }
            for name in pending {
                ensure!(
                    ctx.application()
                        .component_names()
                        .any(|candidate| candidate == &name),
                    "Unknown build component '{name}'"
                );
                components.extend(
                    super::component_guest_bridge_requirements(ctx, &name)
                        .iter()
                        .filter_map(|dependency| dependency.component_name().cloned()),
                );
                let candidates = super::component_environment_tool_middleware_requirements(
                    ctx,
                    &name,
                    &BTreeMap::new(),
                )?;
                if !candidates.is_empty() {
                    rules.push(BuildSelectionRule {
                        owner: name.clone(),
                        after_metadata: false,
                        subject: None,
                        components: candidates,
                    });
                }
                if owners.contains(&name) {
                    rules.extend(super::owner_middleware_selection_rules(ctx, &name)?);
                }
                components.extend(
                    rules
                        .iter()
                        .flat_map(|rule| rule.components.iter().cloned()),
                );
                inspected.insert(name);
            }
        }
    }
    Ok((components, rules))
}

pub(super) fn inspect_build(
    ctx: &BuildContext<'_>,
    with_dependencies: bool,
) -> anyhow::Result<BuildPlan> {
    let selected = ctx.application_context().selected_component_names().clone();
    let (components, rules) = inspect_selection(ctx, with_dependencies)?;
    let stages = inspect_stages(ctx, components.iter(), with_dependencies)?;
    let mut bridges = BTreeMap::new();
    let explicit = selected.iter().cloned().collect::<Vec<_>>();
    let claims = if ctx.should_run_step(AppBuildStep::GenBridge) {
        let mut requests = super::bridge_requests(ctx, &explicit);
        if super::needs_dependency_guest_bridges(ctx, &explicit) {
            requests.push(super::BridgeRequestId::DependencyGuest);
        }
        let claims = super::bridge_output_dir_claims(ctx, &explicit);
        for request in requests {
            bridges.insert(
                request,
                BuildAction {
                    inputs: stages
                        .iter()
                        .flat_map(|stage| &stage.actions)
                        .filter(|action| matches!(action.operation, BuildOperation::Metadata))
                        .flat_map(|action| action.outputs.iter().cloned())
                        .collect(),
                    outputs: if request == super::BridgeRequestId::DependencyGuest {
                        stages
                            .iter()
                            .flat_map(|stage| &stage.inputs)
                            .filter_map(|input| match input {
                                BuildValue::GuestBridge {
                                    dependency,
                                    language,
                                } => Some(match dependency {
                                    crate::model::app::ComponentDependency::Agent {
                                        agent_type_name,
                                        ..
                                    } => ctx
                                        .application()
                                        .dependency_bridge_sdk_dir(agent_type_name, *language),
                                    crate::model::app::ComponentDependency::Tool {
                                        tool_name,
                                        ..
                                    } => ctx.application().dependency_tool_bridge_sdk_dir(
                                        tool_name.as_str(),
                                        *language,
                                    ),
                                }),
                                _ => None,
                            })
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect()
                    } else {
                        claims
                            .iter()
                            .filter(|claim| claim.request_id == request)
                            .map(|claim| claim.base_dir.clone())
                            .collect()
                    },
                    requirements: BTreeSet::from([BuildRequirement::Native {
                        backend: super::plan::NativeBuildBackend::BridgeGeneration,
                    }]),
                    effects: BTreeSet::from([BuildEffect::ReadFiles, BuildEffect::WriteFiles]),
                    operation: BuildOperation::Bridge,
                },
            );
        }
        claims
    } else {
        Vec::new()
    };
    let preparations = if ctx.should_run_step(AppBuildStep::GenBridge)
        || ctx.should_run_step(AppBuildStep::Check)
    {
        super::preparation::BuildPreparations::inspect(ctx, stages.iter())?
    } else {
        Default::default()
    };
    BuildPlan::new(stages, BuildPlanLimits::default())?
        .with_selection(selected, rules)?
        .with_bridges(bridges, claims)?
        .with_preparations(preparations)
}

pub(super) fn inspect_stages<'a>(
    ctx: &BuildContext<'_>,
    components: impl Iterator<Item = &'a ComponentName>,
    with_dependencies: bool,
) -> anyhow::Result<Vec<BuildStage>> {
    let limits = BuildPlanLimits::default();
    let components = components.collect::<Vec<_>>();
    let metadata_sources = components
        .iter()
        .flat_map(|name| super::component_guest_bridge_requirements(ctx, name))
        .filter_map(|dependency| dependency.component_name().cloned())
        .collect::<BTreeSet<_>>();
    let mut stages = Vec::new();
    let mut actions = 0;
    let mut edges = 0;
    for name in components {
        ensure!(
            stages.len() < limits.components,
            "Build plan component limit exceeded"
        );
        let requires_artifact = ctx.should_run_step(AppBuildStep::Build)
            || ctx.should_run_step(AppBuildStep::Check)
            || metadata_sources.contains(name)
            || !super::bridge_requests(ctx, std::slice::from_ref(name)).is_empty();
        let stage = inspect_stage(ctx, name, with_dependencies, requires_artifact)?;
        actions += stage.actions.len();
        edges += stage.dependencies.len() + stage.inputs.len();
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
    requires_artifact: bool,
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
            for preparation in
                possible_common_dependencies_for_tool(ctx, program, directory, go_component)?
            {
                let (directory, inputs, outputs) = match &preparation {
                    CommonDependencies::Npm(root) => (
                        root,
                        vec!["package.json", "package-lock.json"],
                        vec!["node_modules", "package-lock.json"],
                    ),
                    CommonDependencies::Moon(root) => {
                        (root, vec!["moon.mod", "moon.work"], vec![".mooncakes"])
                    }
                    CommonDependencies::Go(root) => {
                        (root, vec!["go.mod", "go.sum"], vec!["go.mod", "go.sum"])
                    }
                };
                actions.push(BuildAction {
                    inputs: inputs.iter().map(|path| directory.join(path)).collect(),
                    outputs: outputs.iter().map(|path| directory.join(path)).collect(),
                    requirements: BTreeSet::from([BuildRequirement::Preparation {
                        preparation: preparation.clone(),
                    }]),
                    effects: BTreeSet::from([
                        BuildEffect::ReadFiles,
                        BuildEffect::WriteFiles,
                        BuildEffect::FetchDependencies,
                        BuildEffect::HostProcess,
                    ]),
                    operation: BuildOperation::Dependencies(preparation),
                });
            }
        }
        actions.push(action);
        ensure!(
            actions.len() <= BuildPlanLimits::default().actions,
            "Build plan action limit exceeded"
        );
    }
    let installed = ctx
        .application_context()
        .installed_components
        .get(component_name);
    if installed.is_none()
        && requires_artifact
        && (!inspect_commands || component.build_commands().is_empty())
    {
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
        inputs: if ctx.should_run_step(AppBuildStep::GenBridge)
            && super::needs_dependency_guest_bridges(
                ctx,
                &ctx.application_context()
                    .selected_component_names()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>(),
            ) {
            super::component_guest_bridge_requirements(ctx, component_name)
                .into_iter()
                .filter_map(|dependency| match component.guest_language() {
                    Some(language) => Some(BuildValue::GuestBridge {
                        dependency,
                        language,
                    }),
                    None if dependency.component_name().is_some() => {
                        Some(BuildValue::SubjectMetadata { dependency })
                    }
                    None => None,
                })
                .collect()
        } else {
            BTreeSet::new()
        },
        actions,
    };
    if with_dependencies && (requires_artifact || component.wasm().exists()) {
        if let Some(installed) = installed {
            stage.actions.push(BuildAction {
                inputs: Vec::new(),
                outputs: Vec::new(),
                requirements: BTreeSet::new(),
                effects: BTreeSet::new(),
                operation: BuildOperation::InstalledMetadata {
                    package_digest: installed.package_metadata.package_digest.clone(),
                    layer_digest: installed.package_metadata.layer_digest.clone(),
                },
            });
        } else {
            let wasm = component.wasm();
            stage.actions.push(BuildAction::metadata(
                wasm.clone(),
                component.extracted_component_metadata(&wasm),
            ));
        }
    }
    Ok(stage)
}

/// Host resolution stays at the Standalone boundary. Tool execution must use
/// the shared program resolver, not reinterpret these host observations.
pub(super) fn check_host_requirements(
    plan: &BuildPlan,
    config: &ApplicationConfig,
) -> anyhow::Result<()> {
    let selected = plan.selected_components(&BTreeMap::new());
    for stage in plan.stages() {
        for requirement in stage.actions.iter().flat_map(|action| &action.requirements) {
            if selected.contains(&stage.component)
                || !matches!(requirement, BuildRequirement::ExistingFile { .. })
            {
                check_requirement(requirement, config)?;
            }
        }
    }
    Ok(())
}

pub(super) fn check_requirement(
    requirement: &BuildRequirement,
    config: &ApplicationConfig,
) -> anyhow::Result<()> {
    if let BuildRequirement::Preparation { preparation } = requirement {
        if !preparation.manifest().exists() {
            return Ok(());
        }
        let requirement = match preparation {
            CommonDependencies::Npm(directory) => BuildRequirement::HostProgram {
                program: "npm".into(),
                directory: directory.clone(),
            },
            CommonDependencies::Moon(directory) => BuildRequirement::HostProgram {
                program: "moon".into(),
                directory: directory.clone(),
            },
            CommonDependencies::Go(_) => BuildRequirement::ManagedGoToolchain,
        };
        return check_requirement(&requirement, config);
    }
    if let BuildRequirement::ExistingFile { path } = requirement {
        ensure!(
            path.is_file(),
            "Build requires existing file {}; supply the artifact or declare its build commands",
            path.display()
        );
    }
    if matches!(requirement, BuildRequirement::ManagedGoToolchain) {
        super::go_toolchain::inspect_go_toolchain(config)?;
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
    Ok(())
}

pub(super) fn log_build_plan(plan: &BuildPlan) {
    log_action("Planned", "checked host build stages (no files changed)");
    plan.preparations.log();
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
        for input in &stage.inputs {
            logln(format!("    awaits {input:?}"));
        }
        for action in &stage.actions {
            match &action.operation {
                BuildOperation::Command(crate::model::app_raw::BuildCommand::External(command)) => {
                    logln(format!(
                        "    operation: external command {:?}",
                        command.command
                    ));
                }
                BuildOperation::Command(crate::model::app_raw::BuildCommand::QuickJSCrate(
                    command,
                )) => {
                    logln(format!(
                        "    operation: generate QuickJS crate {:?}; WIT: {:?}; world: {:?}; JS modules: {:?}",
                        command.generate_quickjs_crate,
                        command.wit,
                        command.world,
                        command.js_modules.iter().collect::<BTreeMap<_, _>>()
                    ));
                }
                operation => logln(format!("    operation: {operation:?}")),
            }
            logln(format!(
                "    inputs: {:?}; outputs: {:?}; effects: {:?}",
                action.inputs, action.outputs, action.effects
            ));
            for requirement in &action.requirements {
                logln(format!("    requires {requirement:?}"));
            }
        }
    }
    for rule in plan.selection_rules() {
        logln(format!(
            "  candidate scope: {} after metadata={}; subject={:?}; candidates={:?}",
            rule.owner, rule.after_metadata, rule.subject, rule.components
        ));
    }
    for (request, action) in &plan.bridges {
        logln(format!(
            "  bridge {request:?}: inputs={:?}; output scopes={:?}; effects={:?}",
            action.inputs, action.outputs, action.effects
        ));
    }
    logln(
        "Preflight checks declared requirements, not whether arbitrary user build code will succeed.",
    );
}
