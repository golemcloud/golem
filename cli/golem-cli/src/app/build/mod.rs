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

use self::check::check_build_tool_requirements;
use self::extract_component_metadata::extract_and_store_component_metadata;
use crate::app::build::componentize::build_components;
use crate::app::build::gen_bridge::{
    BridgeGenerationPlan, build_owner_components, component_tool_dependency_components,
    gen_bridge_sdk_targets, local_tool_middleware_components, plan_custom_bridge_generation,
    plan_explicit_manifest_guest_bridge_generation_for_components_lenient,
    plan_manifest_external_bridge_generation_for_components_lenient,
    plan_repl_bridge_generation_lenient, resolve_dependency_guest_bridge_inputs,
    validate_dependency_tool_bridge_presentations, validate_host_managed_bridge_targets,
    validate_no_ambient_tool_collisions, validate_no_output_dir_collisions,
    validate_supported_bridge_targets, write_repl_metadata,
};
use crate::app::context::BuildContext;
use crate::bridge_gen::BridgeMode;
use crate::error::NonSuccessfulExit;
use crate::log::{LogColorize, LogIndent, log_action, log_error, logln};
use crate::model::app::{
    AppBuildStep, BridgeSdkTarget, ComponentDependency, CustomBridgeSdkTarget,
};
use crate::model::cascade::property::Property;
use crate::model::language::GuestLanguage;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::component::ComponentName;
use golem_common::model::tool::ToolName;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
pub mod check;
pub mod clean;
pub mod command;
pub mod componentize;
pub mod extract_component_metadata;
pub mod gen_bridge;
pub mod go_inputs;
pub mod go_toolchain;
mod inspect;
pub mod plan;
pub mod preparation;
pub mod task_result_marker;
pub mod up_to_date_check;

#[derive(Default)]
pub struct BuildExecution {
    progress: plan::BuildProgress,
    extracted: BTreeMap<ComponentName, ExtractedComponentMetadata>,
    built_components: BTreeSet<ComponentName>,
    validated_targets: Vec<(BridgeRequestId, BridgeSdkTarget)>,
    available_guest_bridge_dependencies: BTreeSet<ComponentDependency>,
    generated_guest_target_keys: BTreeSet<BridgeSdkTargetKey>,
    completed_bridge_requests: BTreeSet<BridgeRequestId>,
    pub(crate) resolved_tool_grants: crate::model::tool_release::ResolvedToolGrants,
    pub(crate) resolved_middleware_grants: crate::command_handler::ResolvedToolMiddlewareGrants,
    pub(crate) environment_tools: Option<crate::app::context::ResolvedEnvironmentTools>,
    pub(crate) completed_preparations: BTreeSet<preparation::BuildPreparationId>,
}

pub enum BuildAdvance {
    Complete,
    Prepare(BTreeSet<preparation::BuildPreparationId>),
}

fn pending_preparations(
    ctx: &BuildContext<'_>,
    checked_plan: &plan::BuildPlan,
    extracted: &BTreeMap<ComponentName, ExtractedComponentMetadata>,
    environment_tools: Option<&crate::app::context::ResolvedEnvironmentTools>,
    completed: &BTreeSet<preparation::BuildPreparationId>,
) -> anyhow::Result<BTreeSet<preparation::BuildPreparationId>> {
    if !ctx.should_run_step(AppBuildStep::GenBridge) {
        return Ok(BTreeSet::new());
    }
    Ok(checked_plan
        .preparations
        .selected(
            &checked_plan.selected_components(extracted),
            extracted,
            environment_tools,
            checked_plan.preparation_limits(),
        )?
        .difference(completed)
        .cloned()
        .collect())
}

/// Inspect admission requirements before authorizing project preparation. This
/// does not construct an executable plan from unpersisted loading changes.
pub async fn inspect_build_requirements(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    let requirements = inspect::inspect_build(
        ctx,
        ctx.should_run_step(AppBuildStep::GenBridge) || ctx.should_run_step(AppBuildStep::Check),
    )?;
    let selected = requirements.selected_components(&BTreeMap::new());
    requirements.preparations.selected(
        &selected,
        &BTreeMap::new(),
        None,
        requirements.preparation_limits(),
    )?;
    // Conditional candidates reserve their programs now, but their artifacts are
    // required only when metadata activates the corresponding selection rule.
    for stage in requirements.stages() {
        for requirement in stage.actions.iter().flat_map(|action| &action.requirements) {
            if selected.contains(&stage.component)
                || !matches!(requirement, plan::BuildRequirement::ExistingFile { .. })
            {
                inspect::check_requirement(requirement, ctx.application_config())?;
            }
        }
    }
    if ctx.should_run_step(AppBuildStep::Check) {
        check_build_tool_requirements(
            ctx,
            requirements
                .stages()
                .filter(|stage| {
                    stage
                        .actions
                        .iter()
                        .any(|action| matches!(action.operation, plan::BuildOperation::Command(_)))
                })
                .map(|stage| &stage.component),
        )
        .await?;
    }
    Ok(())
}

pub async fn preflight_build(ctx: &BuildContext<'_>) -> anyhow::Result<plan::BuildPlan> {
    let plan = inspect::inspect_build(
        ctx,
        ctx.should_run_step(AppBuildStep::GenBridge) || ctx.should_run_step(AppBuildStep::Check),
    )?;
    inspect::check_host_requirements(&plan, ctx.application_config())?;
    if ctx.should_run_step(AppBuildStep::Check) {
        check_build_tool_requirements(
            ctx,
            plan.stages()
                .filter(|stage| {
                    stage
                        .actions
                        .iter()
                        .any(|action| matches!(action.operation, plan::BuildOperation::Command(_)))
                })
                .map(|stage| &stage.component),
        )
        .await?;
    }
    inspect::log_build_plan(&plan);
    Ok(plan)
}

pub async fn build_app(
    ctx: &BuildContext<'_>,
    checked_plan: &plan::BuildPlan,
    execution: &mut BuildExecution,
) -> anyhow::Result<BuildAdvance> {
    ctx.extracted_metadata
        .lock()
        .unwrap()
        .extend(execution.extracted.clone());
    for (component, metadata) in &execution.extracted {
        validate_no_ambient_tool_collisions(ctx, component, &metadata.tools)?;
    }
    if let BuildAdvance::Prepare(inputs) =
        execute_component_stages(ctx, checked_plan, execution).await?
    {
        return Ok(BuildAdvance::Prepare(inputs));
    }
    let explicit_bridge_component_names = checked_plan
        .explicit_components()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let has_dependency_guest_request = checked_plan
        .bridges
        .contains_key(&BridgeRequestId::DependencyGuest);
    let BuildExecution {
        validated_targets,
        generated_guest_target_keys,
        completed_bridge_requests,
        ..
    } = execution;
    let claims = &checked_plan.output_claims;

    if has_dependency_guest_request {
        let extracted = ctx.extracted_metadata.lock().unwrap().clone();
        let targets = validated_targets
            .iter()
            .filter(|(request, _)| *request == BridgeRequestId::DependencyGuest)
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>();
        validate_dependency_tool_bridge_presentations(ctx, &extracted, &targets)?;
    }

    for request_id in [
        BridgeRequestId::Custom,
        BridgeRequestId::ManifestExternal,
        BridgeRequestId::ManifestGuest,
        BridgeRequestId::Repl,
    ] {
        if !checked_plan.bridges.contains_key(&request_id)
            || completed_bridge_requests.contains(&request_id)
        {
            continue;
        }

        let plan = plan_bridge_request(ctx, request_id, &explicit_bridge_component_names).await?;

        let mut request_target_keys = BTreeSet::new();
        let generated_target_keys = if request_id == BridgeRequestId::ManifestGuest {
            &mut *generated_guest_target_keys
        } else {
            &mut request_target_keys
        };
        let targets = validate_and_filter_new_bridge_targets(
            ctx,
            request_id,
            plan.targets.clone(),
            validated_targets,
            claims,
            generated_target_keys,
        )?;

        let exact_targets = plan.targets.clone();
        match request_id {
            BridgeRequestId::ManifestGuest => {
                validate_manifest_guest_matchers_resolved(
                    ctx,
                    &explicit_bridge_component_names,
                    &exact_targets,
                )?;
            }
            BridgeRequestId::ManifestExternal => {
                validate_manifest_external_matchers_resolved(
                    ctx,
                    &explicit_bridge_component_names,
                    &exact_targets,
                )?;
            }
            BridgeRequestId::Custom => {}
            BridgeRequestId::Repl => {}
            _ => unreachable!(),
        }

        if !targets.is_empty() {
            write_repl_metadata(
                ctx,
                &BridgeGenerationPlan {
                    targets: targets.clone(),
                    repl_metadata_by_language: plan.repl_metadata_by_language,
                },
            )
            .await?;
            gen_bridge_sdk_targets(ctx, targets.clone()).await?;
            generated_target_keys.extend(
                targets
                    .iter()
                    .map(|target| BridgeSdkTargetKey::from_bridge_target(ctx, target)),
            );
        } else if !plan.repl_metadata_by_language.is_empty() {
            write_repl_metadata(
                ctx,
                &BridgeGenerationPlan {
                    targets: Vec::new(),
                    repl_metadata_by_language: plan.repl_metadata_by_language,
                },
            )
            .await?;
        }
        completed_bridge_requests.insert(request_id);
    }

    Ok(BuildAdvance::Complete)
}

fn target_dependency(target: &BridgeSdkTarget) -> ComponentDependency {
    use crate::model::app::{BridgeSdkTargetSource, BridgeSdkTargetSubject, SubjectSource};
    match &target.subject {
        BridgeSdkTargetSubject::Agent(agent) => {
            let BridgeSdkTargetSource::Local { component_name } = &target.source else {
                unreachable!("agent clients have a local source")
            };
            ComponentDependency::Agent {
                component_name: component_name.clone(),
                agent_type_name: agent.type_name.clone(),
            }
        }
        BridgeSdkTargetSubject::Tool { rpc_name, .. } => ComponentDependency::Tool {
            tool_name: rpc_name.as_str().try_into().expect("validated tool name"),
            source: match &target.source {
                BridgeSdkTargetSource::Local { component_name } => SubjectSource::Local {
                    component_name: component_name.clone(),
                },
                BridgeSdkTargetSource::RemoteRelease { .. } => SubjectSource::RemoteRelease,
                _ => SubjectSource::EnvironmentTool,
            },
        },
    }
}

async fn execute_component_stages(
    ctx: &BuildContext<'_>,
    checked_plan: &plan::BuildPlan,
    execution: &mut BuildExecution,
) -> anyhow::Result<BuildAdvance> {
    let BuildExecution {
        progress,
        extracted,
        built_components,
        validated_targets,
        available_guest_bridge_dependencies,
        generated_guest_target_keys,
        completed_preparations,
        environment_tools,
        ..
    } = execution;
    let has_dependency_guest_request = checked_plan
        .bridges
        .contains_key(&BridgeRequestId::DependencyGuest);
    let mut pending_components = checked_plan
        .selected_components(extracted)
        .difference(&progress.completed)
        .cloned()
        .collect::<BTreeSet<_>>();
    let _indent = if ctx.should_run_step(AppBuildStep::Build) {
        log_action("Building", "components");
        Some(LogIndent::new())
    } else {
        None
    };

    loop {
        for name in pending_components.iter() {
            for requirement in checked_plan
                .stage(name)?
                .actions
                .iter()
                .flat_map(|action| &action.requirements)
            {
                inspect::check_requirement(requirement, ctx.application_config())?;
            }
        }
        let inputs = pending_preparations(
            ctx,
            checked_plan,
            extracted,
            environment_tools.as_ref(),
            completed_preparations,
        )?;
        if !inputs.is_empty() {
            return Ok(BuildAdvance::Prepare(inputs));
        }
        if pending_components.is_empty() {
            return Ok(BuildAdvance::Complete);
        }
        if has_dependency_guest_request && !ctx.should_run_step(AppBuildStep::Build) {
            for component_name in pending_components
                .iter()
                .filter(|name| !extracted.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>()
            {
                let wasm = ctx.application().component(&component_name).wasm();
                if !wasm.exists() {
                    continue;
                }
                let metadata = extract_and_store_component_metadata(ctx, &component_name).await?;
                extracted.insert(component_name.clone(), metadata);
                built_components.insert(component_name);
            }
            let inputs = pending_preparations(
                ctx,
                checked_plan,
                extracted,
                environment_tools.as_ref(),
                completed_preparations,
            )?;
            if !inputs.is_empty() {
                return Ok(BuildAdvance::Prepare(inputs));
            }
        }
        let mut made_progress = false;

        if has_dependency_guest_request
            && generate_available_dependency_guest_bridges(
                ctx,
                &checked_plan.output_claims,
                validated_targets,
                generated_guest_target_keys,
                &pending_components,
                built_components,
                extracted,
                available_guest_bridge_dependencies,
            )
            .await?
        {
            made_progress = true;
        }

        let mut values = checked_plan.metadata_values(extracted);
        values.extend(
            validated_targets
                .iter()
                .filter(|(_, target)| {
                    generated_guest_target_keys
                        .contains(&BridgeSdkTargetKey::from_bridge_target(ctx, target))
                })
                .map(|(_, target)| plan::BuildValue::GuestBridge {
                    dependency: target_dependency(target),
                    language: target.target_language,
                }),
        );
        if let Some(component_name) = checked_plan
            .ready(progress, &values)
            .into_iter()
            .find(|name| pending_components.contains(name))
        {
            if ctx.should_run_step(AppBuildStep::Build) {
                ctx.extracted_metadata
                    .lock()
                    .unwrap()
                    .remove(&component_name);
                build_components(ctx, checked_plan.stage(&component_name)?, progress).await?;
            }
            checked_plan.complete(progress, &component_name, &values)?;

            pending_components.remove(&component_name);
            built_components.insert(component_name.clone());
            if has_dependency_guest_request {
                let component = ctx.application().component(&component_name);
                if component.wasm().exists() {
                    let metadata =
                        extract_and_store_component_metadata(ctx, &component_name).await?;
                    validate_no_ambient_tool_collisions(ctx, &component_name, &metadata.tools)?;
                    extracted.insert(component_name.clone(), metadata);
                }
                pending_components.extend(
                    checked_plan
                        .selected_components(extracted)
                        .difference(built_components)
                        .cloned(),
                );
            }
            made_progress = true;
        }

        if !made_progress {
            report_guest_bridge_dependency_ordering_cycle(
                ctx,
                &pending_components,
                available_guest_bridge_dependencies,
                extracted,
            )?;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn generate_available_dependency_guest_bridges(
    ctx: &BuildContext<'_>,
    claims: &[OutputDirClaim],
    validated_targets: &mut Vec<(BridgeRequestId, BridgeSdkTarget)>,
    generated_guest_target_keys: &mut BTreeSet<BridgeSdkTargetKey>,
    pending_components: &BTreeSet<ComponentName>,
    built_components: &BTreeSet<ComponentName>,
    extracted: &BTreeMap<ComponentName, ExtractedComponentMetadata>,
    available_guest_bridge_dependencies: &mut BTreeSet<ComponentDependency>,
) -> anyhow::Result<bool> {
    let scope_component_names = built_components
        .iter()
        .chain(pending_components.iter())
        .cloned()
        .collect::<Vec<_>>();
    let source_component_names = built_components
        .iter()
        .filter(|component_name| extracted.contains_key(*component_name))
        .cloned()
        .collect::<Vec<_>>();
    let targets = resolve_dependency_guest_bridge_inputs(
        ctx,
        &source_component_names,
        &scope_component_names,
        extracted,
    )?;

    let new_targets = validate_and_filter_new_bridge_targets(
        ctx,
        BridgeRequestId::DependencyGuest,
        targets,
        validated_targets,
        claims,
        generated_guest_target_keys,
    )?;

    if !new_targets.is_empty() {
        gen_bridge_sdk_targets(ctx, new_targets.clone()).await?;
    }

    generated_guest_target_keys.extend(
        new_targets
            .iter()
            .map(|target| BridgeSdkTargetKey::from_bridge_target(ctx, target)),
    );
    available_guest_bridge_dependencies.extend(new_targets.iter().map(target_dependency));
    Ok(!new_targets.is_empty())
}

fn validate_and_filter_new_bridge_targets(
    ctx: &BuildContext<'_>,
    request_id: BridgeRequestId,
    targets: Vec<BridgeSdkTarget>,
    validated_targets: &mut Vec<(BridgeRequestId, BridgeSdkTarget)>,
    claims: &[OutputDirClaim],
    generated_target_keys: &mut BTreeSet<BridgeSdkTargetKey>,
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    let mut new_targets = Vec::new();
    let mut new_target_keys = BTreeSet::new();
    for target in targets {
        let key = BridgeSdkTargetKey::from_bridge_target(ctx, &target);
        if generated_target_keys.contains(&key) {
            continue;
        }
        if new_target_keys.insert(key) {
            new_targets.push(target);
        }
    }

    let mut tagged_targets = validated_targets.clone();
    tagged_targets.extend(
        new_targets
            .iter()
            .cloned()
            .map(|target| (request_id, target)),
    );

    let exact_targets = tagged_targets
        .iter()
        .map(|(_, target)| target.clone())
        .collect::<Vec<_>>();
    validate_supported_bridge_targets(&exact_targets)?;
    validate_host_managed_bridge_targets(&exact_targets)?;
    validate_no_output_dir_collisions(&exact_targets)?;
    validate_exact_targets_against_claims(&tagged_targets, claims)?;

    for target in &new_targets {
        validated_targets.push((request_id, target.clone()));
    }

    Ok(new_targets)
}

fn report_guest_bridge_dependency_ordering_cycle(
    ctx: &BuildContext<'_>,
    pending_components: &BTreeSet<ComponentName>,
    available_guest_bridge_dependencies: &BTreeSet<ComponentDependency>,
    extracted: &BTreeMap<ComponentName, ExtractedComponentMetadata>,
) -> anyhow::Result<()> {
    logln("");
    log_error(
        "Build cannot resolve guest bridge generation: missing metadata or an unresolved build dependency cycle:",
    );
    for component_name in pending_components {
        let missing = component_guest_bridge_requirements(ctx, component_name)
            .difference(available_guest_bridge_dependencies)
            .map(|dependency| match dependency {
                ComponentDependency::Agent {
                    component_name,
                    agent_type_name,
                } => format!("agent {}/{}", component_name, agent_type_name.as_str()),
                ComponentDependency::Tool { source, tool_name } => format!(
                    "tool {}/{}",
                    format_subject_source(source),
                    tool_name.as_str()
                ),
            })
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            log_error(format!(
                "  component {} waits for guest bridge generation for: {}",
                component_name.as_str().log_color_highlight(),
                missing
                    .iter()
                    .map(|component| component.log_color_highlight())
                    .join(", "),
            ));
        }
        let bindings = ctx.application().environment_tool_bindings();
        for dependency in component_guest_bridge_requirements(ctx, component_name) {
            let ComponentDependency::Tool { tool_name, .. } = dependency else {
                continue;
            };
            let Some(binding) = bindings.get(tool_name.as_str()) else {
                continue;
            };
            for installation in binding
                .middleware_installations()
                .map_err(anyhow::Error::msg)?
                .unwrap_or_default()
            {
                let Some(declaration) = ctx
                    .application()
                    .tool_middleware_declarations()
                    .get(&installation.name)
                else {
                    continue;
                };
                if declaration.value.release.is_some() {
                    continue;
                }
                if let Some(provider) = &declaration.value.component {
                    if !extracted.contains_key(provider) {
                        log_error(format!(
                            "  component {component_name} needs middleware '{}' metadata from declared provider '{provider}'",
                            installation.name
                        ));
                    }
                } else {
                    let candidates = ctx
                        .application()
                        .component_names()
                        .filter(|name| !extracted.contains_key(*name))
                        .map(ComponentName::as_str)
                        .join(", ");
                    if !candidates.is_empty() {
                        log_error(format!(
                            "  component {component_name} needs discovery of middleware '{}'; uninspected candidates: {candidates}",
                            installation.name
                        ));
                    }
                }
            }
        }
    }
    anyhow::bail!(NonSuccessfulExit)
}

fn format_subject_source(source: &crate::model::app::SubjectSource) -> String {
    match source {
        crate::model::app::SubjectSource::Local { component_name } => component_name.to_string(),
        crate::model::app::SubjectSource::RemoteRelease => "remote release".to_string(),
        crate::model::app::SubjectSource::EnvironmentTool => "environment tool".to_string(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct BridgeSdkTargetKey {
    source: String,
    target_name: String,
    kind: &'static str,
    target_language: GuestLanguage,
    bridge_mode: BridgeMode,
    output_dir: PathBuf,
    rust_config: String,
}

impl BridgeSdkTargetKey {
    fn from_bridge_target(ctx: &BuildContext<'_>, target: &BridgeSdkTarget) -> Self {
        Self {
            source: serde_json::to_string(&target.source)
                .expect("bridge SDK target sources are serializable"),
            target_name: target.subject.display_name().to_string(),
            kind: target.subject.kind().as_str(),
            target_language: target.target_language,
            bridge_mode: target.bridge_mode,
            output_dir: crate::fs::absolute_lexical_path(&target.output_dir).unwrap_or_else(|_| {
                ctx.application()
                    .bridge_sdks_source()
                    .join(&target.output_dir)
            }),
            rust_config: serde_json::to_string(&target.rust_config)
                .expect("validated Rust config is serializable"),
        }
    }
}

fn component_guest_bridge_requirements(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> BTreeSet<ComponentDependency> {
    ctx.application()
        .component(component_name)
        .properties()
        .dependencies
        .iter()
        .cloned()
        .collect()
}

fn component_environment_tool_middleware_requirements(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
    extracted: &BTreeMap<ComponentName, ExtractedComponentMetadata>,
) -> anyhow::Result<BTreeSet<ComponentName>> {
    let bindings = ctx.application().environment_tool_bindings();
    let mut components = BTreeSet::new();
    for dependency in &ctx
        .application()
        .component(component_name)
        .properties()
        .dependencies
    {
        if let ComponentDependency::Tool { tool_name, .. } = dependency {
            components.extend(local_tool_middleware_components(
                ctx,
                bindings.get(tool_name.as_str()),
                extracted,
            )?);
        }
    }
    Ok(components)
}

fn selected_component_names_have_guest_bridge_dependencies(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> bool {
    selected_component_names
        .iter()
        .any(|component_name| !component_guest_bridge_requirements(ctx, component_name).is_empty())
}

fn needs_dependency_guest_bridges(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> bool {
    selected_component_names_have_guest_bridge_dependencies(ctx, selected_component_names)
        || ctx.custom_bridge_sdk_target().is_none()
            && ctx
                .application()
                .bridge_sdks()
                .for_all_used_modes()
                .any(|(_, mode, targets)| {
                    mode == BridgeMode::Guest
                        && targets.tools.as_ref().is_some_and(|tools| {
                            manifest_tool_bridge_request_may_match_selected_components(
                                ctx,
                                tools,
                                selected_component_names,
                            )
                        })
                })
}

fn owner_middleware_selection_rules(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> anyhow::Result<Vec<plan::BuildSelectionRule>> {
    let tool_names = component_tool_dependency_components(ctx, component_name)
        .iter()
        .flat_map(|name| {
            ctx.application()
                .component(name)
                .properties()
                .dependencies
                .clone()
        })
        .filter_map(|dependency| match dependency {
            ComponentDependency::Tool { tool_name, .. } => Some(tool_name.to_string()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if tool_names.is_empty() {
        return Ok(Vec::new());
    }
    let agent_names = ctx
        .application()
        .agent_ids()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mapping = agent_names
        .iter()
        .map(|name| (name.clone(), component_name.clone()))
        .collect();
    let agents = ctx.application().resolve_agents(&mapping)?;
    let component = ctx.application().component(component_name);
    let bindings = component
        .layer_properties()
        .tool_bindings
        .value()
        .iter()
        .filter(|(name, _)| tool_names.contains(*name))
        .map(|(_, binding)| binding)
        .collect::<Vec<_>>();
    let mut rules = Vec::new();
    for binding in bindings {
        rules.push(plan::BuildSelectionRule {
            owner: component_name.clone(),
            after_metadata: true,
            subject: None,
            components: local_tool_middleware_components(ctx, Some(binding), &BTreeMap::new())?,
        });
    }
    for agent in &agent_names {
        for (_, binding) in agents
            .agent(agent)
            .expect("declared agent has resolved bindings")
            .tool_bindings()
            .iter()
            .filter(|(name, _)| tool_names.contains(*name))
        {
            rules.push(plan::BuildSelectionRule {
                owner: component_name.clone(),
                after_metadata: true,
                subject: Some(ComponentDependency::Agent {
                    component_name: component_name.clone(),
                    agent_type_name: agent.clone(),
                }),
                components: local_tool_middleware_components(ctx, Some(binding), &BTreeMap::new())?,
            });
        }
    }
    rules.retain(|rule| !rule.components.is_empty());
    Ok(rules)
}

fn manifest_middleware_selection_rules(
    ctx: &BuildContext<'_>,
) -> anyhow::Result<Vec<plan::BuildSelectionRule>> {
    let mut rules = Vec::new();
    if ctx.should_run_step(AppBuildStep::GenBridge) && ctx.custom_bridge_sdk_target().is_none() {
        let bindings = ctx.application().environment_tool_bindings();
        for (_, mode, targets) in ctx.application().bridge_sdks().for_all_used_modes() {
            if mode != BridgeMode::Guest {
                continue;
            }
            let matchers = targets
                .tools
                .as_ref()
                .map(|tools| tools.clone().into_set())
                .unwrap_or_default();
            for (name, binding) in &bindings {
                if !matchers.contains("*")
                    && !matchers.contains(name)
                    && !ctx
                        .application_context()
                        .selected_component_names()
                        .iter()
                        .any(|owner| matchers.contains(owner.as_str()))
                {
                    continue;
                }
                let components =
                    local_tool_middleware_components(ctx, Some(binding), &BTreeMap::new())?;
                if components.is_empty() {
                    continue;
                }
                for owner in ctx.application_context().selected_component_names() {
                    let subject = if matchers.contains("*") || matchers.contains(name) {
                        None
                    } else if matchers.contains(owner.as_str()) {
                        Some(ComponentDependency::Tool {
                            tool_name: name.as_str().try_into().map_err(anyhow::Error::msg)?,
                            source: crate::model::app::SubjectSource::Local {
                                component_name: owner.clone(),
                            },
                        })
                    } else {
                        continue;
                    };
                    rules.push(plan::BuildSelectionRule {
                        owner: owner.clone(),
                        after_metadata: subject.is_some(),
                        subject,
                        components: components.clone(),
                    });
                }
            }
        }
    }
    Ok(rules)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum BridgeRequestId {
    DependencyGuest,
    ManifestGuest,
    ManifestExternal,
    Custom,
    Repl,
}

async fn plan_bridge_request(
    ctx: &BuildContext<'_>,
    request_id: BridgeRequestId,
    selected_component_names: &[ComponentName],
) -> anyhow::Result<BridgeGenerationPlan> {
    match request_id {
        BridgeRequestId::DependencyGuest => {
            unreachable!(
                "dependency guest bridges are planned during component dependency ordering"
            )
        }
        BridgeRequestId::ManifestGuest => {
            plan_explicit_manifest_guest_bridge_generation_for_components_lenient(
                ctx,
                selected_component_names,
            )
            .await
        }
        BridgeRequestId::ManifestExternal => {
            plan_manifest_external_bridge_generation_for_components_lenient(
                ctx,
                selected_component_names,
            )
            .await
        }
        BridgeRequestId::Custom => {
            plan_custom_bridge_generation(
                ctx,
                ctx.custom_bridge_sdk_target()
                    .expect("custom bridge request requires a custom target"),
            )
            .await
        }
        BridgeRequestId::Repl => {
            plan_repl_bridge_generation_lenient(
                ctx,
                ctx.repl_bridge_sdk_target()
                    .expect("REPL bridge request requires a REPL target"),
            )
            .await
        }
    }
}

#[derive(Clone, Debug)]
struct OutputDirClaim {
    request_id: BridgeRequestId,
    base_dir: PathBuf,
    description: String,
}

fn bridge_output_dir_claims(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> Vec<OutputDirClaim> {
    let mut claims = Vec::new();

    if ctx.custom_bridge_sdk_target().is_none() {
        for (language, mode, sdk_targets) in ctx.application().bridge_sdks().for_all_used_modes() {
            if manifest_bridge_request_may_match_selected_components(
                ctx,
                &sdk_targets.agents,
                selected_component_names,
            ) {
                add_manifest_bridge_output_dir_claims(
                    ctx,
                    &mut claims,
                    language,
                    mode,
                    &sdk_targets.agents,
                    sdk_targets.output_dir.as_deref(),
                    selected_component_names,
                );
            }

            if mode == BridgeMode::Guest
                && let Some(tools) = sdk_targets.tools.as_ref()
                && manifest_tool_bridge_request_may_match_selected_components(
                    ctx,
                    tools,
                    selected_component_names,
                )
            {
                add_manifest_tool_bridge_output_dir_claims(
                    ctx,
                    &mut claims,
                    language,
                    mode,
                    tools,
                    sdk_targets.output_dir.as_deref(),
                    selected_component_names,
                );
            }
        }
    }

    if let Some(custom_target) = ctx.custom_bridge_sdk_target() {
        add_custom_bridge_output_dir_claim(
            &mut claims,
            BridgeRequestId::Custom,
            "custom bridge target",
            custom_target,
        );
    }
    if let Some(repl_target) = ctx.repl_bridge_sdk_target() {
        add_custom_bridge_output_dir_claim(
            &mut claims,
            BridgeRequestId::Repl,
            "REPL bridge target",
            repl_target,
        );
    }

    claims
}

fn add_manifest_bridge_output_dir_claims(
    ctx: &BuildContext<'_>,
    claims: &mut Vec<OutputDirClaim>,
    language: GuestLanguage,
    mode: BridgeMode,
    agents: &crate::model::app_raw::LenientTokenList,
    output_dir: Option<&Path>,
    selected_component_names: &[ComponentName],
) {
    let request_id = match mode {
        BridgeMode::Guest => BridgeRequestId::ManifestGuest,
        BridgeMode::External => BridgeRequestId::ManifestExternal,
    };
    let base_dir = manifest_bridge_claim_base(ctx, language, mode, output_dir);
    let component_names = ctx
        .application()
        .component_names()
        .cloned()
        .collect::<BTreeSet<_>>();

    for matcher in agents.clone().into_set() {
        let is_component_matcher = component_names
            .iter()
            .any(|component_name| component_name.as_str() == matcher.as_str());
        let is_selected_component_matcher = selected_component_names
            .iter()
            .any(|component_name| component_name.as_str() == matcher.as_str());

        if matcher == "*" || is_component_matcher {
            if matcher != "*" && !is_selected_component_matcher {
                continue;
            }

            if matcher == "*"
                || !add_existing_component_manifest_bridge_output_dir_claims(
                    ctx,
                    claims,
                    request_id,
                    language,
                    mode,
                    &matcher,
                    BridgeTargetNameKind::Agent,
                )
            {
                claims.push(OutputDirClaim {
                    request_id,
                    base_dir: base_dir.clone(),
                    description: format!("{language} {mode:?} manifest bridge outputDir"),
                });
            }
        } else {
            let agent_type_name = AgentTypeName(matcher.clone());
            claims.push(OutputDirClaim {
                request_id,
                base_dir: ctx
                    .application()
                    .bridge_sdk_dir(&agent_type_name, language, mode),
                description: format!(
                    "{} manifest bridge target for {}",
                    bridge_sdk_target_name(language, mode),
                    matcher
                ),
            });
        }
    }
}

fn add_manifest_tool_bridge_output_dir_claims(
    ctx: &BuildContext<'_>,
    claims: &mut Vec<OutputDirClaim>,
    language: GuestLanguage,
    mode: BridgeMode,
    tools: &crate::model::app_raw::LenientTokenList,
    output_dir: Option<&Path>,
    selected_component_names: &[ComponentName],
) {
    let request_id = match mode {
        BridgeMode::Guest => BridgeRequestId::ManifestGuest,
        BridgeMode::External => BridgeRequestId::ManifestExternal,
    };
    let base_dir = manifest_bridge_claim_base(ctx, language, mode, output_dir);
    let component_names = ctx
        .application()
        .component_names()
        .cloned()
        .collect::<BTreeSet<_>>();

    for matcher in tools.clone().into_set() {
        let is_component_matcher = component_names
            .iter()
            .any(|component_name| component_name.as_str() == matcher.as_str());
        let is_selected_component_matcher = selected_component_names
            .iter()
            .any(|component_name| component_name.as_str() == matcher.as_str());

        if matcher == "*" || is_component_matcher {
            if matcher != "*" && !is_selected_component_matcher {
                continue;
            }

            if matcher == "*"
                || !add_existing_component_manifest_bridge_output_dir_claims(
                    ctx,
                    claims,
                    request_id,
                    language,
                    mode,
                    &matcher,
                    BridgeTargetNameKind::Tool,
                )
            {
                claims.push(OutputDirClaim {
                    request_id,
                    base_dir: base_dir.clone(),
                    description: format!("{language} {mode:?} manifest tool bridge outputDir"),
                });
            }
        } else {
            claims.push(OutputDirClaim {
                request_id,
                base_dir: ctx.application().tool_bridge_sdk_dir(&matcher, language),
                description: format!(
                    "{} manifest tool bridge target for {}",
                    bridge_sdk_target_name(language, mode),
                    matcher
                ),
            });
        }
    }
}

fn manifest_tool_bridge_request_may_match_selected_components(
    ctx: &BuildContext<'_>,
    tools: &crate::model::app_raw::LenientTokenList,
    selected_component_names: &[ComponentName],
) -> bool {
    let matchers = tools.clone().into_set();
    let matches_remote_release = (matchers.contains("*")
        && ctx
            .application()
            .remote_release_references()
            .next()
            .is_some())
        || ctx
            .application()
            .remote_release_references()
            .any(|(name, _)| matchers.contains(name.as_str()));
    matches_remote_release
        || manifest_matchers_may_match_selected_components(ctx, matchers, selected_component_names)
}

/// Which kind of manifest bridge matchers a component-based output-dir claim
/// is collected for: agent matchers claim the component's agent type client
/// directories, tool matchers its tool client directories.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BridgeTargetNameKind {
    Agent,
    Tool,
}

fn add_existing_component_manifest_bridge_output_dir_claims(
    ctx: &BuildContext<'_>,
    claims: &mut Vec<OutputDirClaim>,
    request_id: BridgeRequestId,
    language: GuestLanguage,
    mode: BridgeMode,
    component_name: &str,
    kind: BridgeTargetNameKind,
) -> bool {
    let component_name = ComponentName(component_name.to_string());
    let component = ctx.application().component(&component_name);
    if ctx.should_run_step(AppBuildStep::Build) {
        return false;
    }

    let wasm = component.wasm();
    let extracted_component_metadata = component.extracted_component_metadata(&wasm);
    if !extracted_component_metadata.exists() {
        return false;
    }
    if !path_is_fresh_against(&extracted_component_metadata, &wasm) {
        return false;
    }

    let Ok(metadata) =
        crate::fs::read_to_string(&extracted_component_metadata).and_then(|contents| {
            serde_json::from_str::<ExtractedComponentMetadata>(&contents).map_err(Into::into)
        })
    else {
        return false;
    };

    match kind {
        BridgeTargetNameKind::Agent => {
            for agent_type in metadata.agent_types {
                claims.push(OutputDirClaim {
                    request_id,
                    base_dir: ctx.application().bridge_sdk_dir(
                        &agent_type.type_name,
                        language,
                        mode,
                    ),
                    description: format!(
                        "{} manifest bridge target for {} from {}",
                        bridge_sdk_target_name(language, mode),
                        agent_type.type_name.as_str(),
                        component_name.as_str(),
                    ),
                });
            }
        }
        BridgeTargetNameKind::Tool => {
            for tool in metadata.tools {
                let Some(name) = tool.name() else {
                    continue;
                };
                claims.push(OutputDirClaim {
                    request_id,
                    base_dir: ctx.application().tool_bridge_sdk_dir(name, language),
                    description: format!(
                        "{} manifest tool bridge target for {} from {}",
                        bridge_sdk_target_name(language, mode),
                        name,
                        component_name.as_str(),
                    ),
                });
            }
        }
    }

    true
}

fn path_is_fresh_against(target: &Path, source: &Path) -> bool {
    let Ok(target_modified) = std::fs::metadata(target).and_then(|metadata| metadata.modified())
    else {
        return false;
    };
    let Ok(source_modified) = std::fs::metadata(source).and_then(|metadata| metadata.modified())
    else {
        return false;
    };

    target_modified >= source_modified
}

fn manifest_bridge_claim_base(
    ctx: &BuildContext<'_>,
    language: GuestLanguage,
    mode: BridgeMode,
    output_dir: Option<&Path>,
) -> PathBuf {
    match output_dir {
        Some(output_dir) => output_dir.to_path_buf(),
        None => match mode {
            BridgeMode::External => ctx
                .application()
                .temp_dir()
                .join("bridge-sdk")
                .join(language.id()),
            BridgeMode::Guest => ctx
                .application()
                .temp_dir()
                .join("bridge-sdk")
                .join(language.id())
                .join(mode.id()),
        },
    }
}

fn manifest_bridge_request_may_match_selected_components(
    ctx: &BuildContext<'_>,
    agents: &crate::model::app_raw::LenientTokenList,
    selected_component_names: &[ComponentName],
) -> bool {
    manifest_matchers_may_match_selected_components(
        ctx,
        agents.clone().into_set(),
        selected_component_names,
    )
}

fn manifest_matchers_may_match_selected_components(
    ctx: &BuildContext<'_>,
    matchers: BTreeSet<String>,
    selected_component_names: &[ComponentName],
) -> bool {
    if matchers.contains("*") {
        return true;
    }

    matchers.iter().any(|matcher| {
        !ctx.application()
            .component_names()
            .any(|component_name| component_name.as_str() == matcher.as_str())
            || selected_component_names
                .iter()
                .any(|component_name| component_name.as_str() == matcher.as_str())
    })
}

fn add_custom_bridge_output_dir_claim(
    claims: &mut Vec<OutputDirClaim>,
    request_id: BridgeRequestId,
    description: &str,
    target: &CustomBridgeSdkTarget,
) {
    if let Some(output_dir) = &target.output_dir {
        claims.push(OutputDirClaim {
            request_id,
            base_dir: output_dir.clone(),
            description: description.to_string(),
        });
    }
}

fn validate_exact_targets_against_claims(
    exact_targets: &[(BridgeRequestId, BridgeSdkTarget)],
    claims: &[OutputDirClaim],
) -> anyhow::Result<()> {
    let resolved_request_ids = exact_targets
        .iter()
        .map(|(request_id, _)| *request_id)
        .collect::<BTreeSet<_>>();
    for (target_request_id, target) in exact_targets {
        let target_output_dir = crate::fs::absolute_lexical_path(&target.output_dir)?;
        for claim in claims {
            if resolved_request_ids.contains(&claim.request_id) {
                continue;
            }
            if claim.request_id == BridgeRequestId::ManifestGuest
                && *target_request_id == BridgeRequestId::ManifestExternal
            {
                continue;
            }
            if claim_matches_request(claim.request_id, *target_request_id) {
                continue;
            }
            let claim_base_dir = crate::fs::absolute_lexical_path(&claim.base_dir)?;
            // External clients are direct `<base>/<name>-client` children;
            // guest clients use distinct `<base>/<name>-guest-client` children.
            if claim.request_id == BridgeRequestId::ManifestExternal
                && matches!(
                    *target_request_id,
                    BridgeRequestId::DependencyGuest | BridgeRequestId::ManifestGuest
                )
                && target_output_dir.starts_with(&claim_base_dir)
            {
                continue;
            }
            if claim.request_id == BridgeRequestId::ManifestGuest
                && *target_request_id == BridgeRequestId::DependencyGuest
                && target_output_dir.starts_with(&claim_base_dir)
            {
                continue;
            }
            if claim.request_id == BridgeRequestId::Custom
                && *target_request_id == BridgeRequestId::DependencyGuest
                && target_output_dir.starts_with(&claim_base_dir)
            {
                continue;
            }
            if target_output_dir == claim_base_dir
                || target_output_dir.starts_with(&claim_base_dir)
                || claim_base_dir.starts_with(&target_output_dir)
            {
                logln("");
                log_error(format!(
                    "Bridge SDK target output directory {} for {} may overlap unresolved {} at {}",
                    target_output_dir.log_color_highlight(),
                    target.subject.display_name(),
                    claim.description,
                    claim_base_dir.log_color_highlight(),
                ));
                anyhow::bail!(NonSuccessfulExit)
            }
        }
    }

    Ok(())
}

fn validate_manifest_guest_matchers_resolved(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
    exact_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    validate_manifest_matchers_resolved(
        ctx,
        BridgeMode::Guest,
        selected_component_names,
        exact_targets,
    )
}

fn validate_manifest_external_matchers_resolved(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
    exact_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    validate_manifest_matchers_resolved(
        ctx,
        BridgeMode::External,
        selected_component_names,
        exact_targets,
    )
}

fn validate_manifest_matchers_resolved(
    ctx: &BuildContext<'_>,
    bridge_mode_filter: BridgeMode,
    selected_component_names: &[ComponentName],
    exact_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    for (target_language, bridge_mode, sdk_targets) in
        ctx.application().bridge_sdks().for_all_used_modes()
    {
        if bridge_mode != bridge_mode_filter {
            continue;
        }

        let mut agent_matchers = sdk_targets.agents.clone().into_set();
        if agent_matchers.remove("*") {
            validate_wildcard_manifest_sources_resolved(
                ctx,
                target_language,
                bridge_mode,
                selected_component_names,
            )?;
        } else {
            for component_name in ctx.application().component_names() {
                if component_matcher_is_not_expected_to_resolve(
                    ctx,
                    selected_component_names,
                    component_name,
                ) {
                    agent_matchers.remove(component_name.as_str());
                }
            }

            for target in exact_targets {
                if target.target_language == target_language
                    && target.bridge_mode == bridge_mode
                    && let Some(agent_type) = target.subject.as_agent()
                {
                    agent_matchers.remove(agent_type.type_name.as_str());
                }
            }

            if !agent_matchers.is_empty() {
                logln("");
                log_error(format!(
                    "The following agent matchers were not found during {} bridge SDK generation: {}",
                    bridge_sdk_target_name(target_language, bridge_mode).log_color_highlight(),
                    agent_matchers
                        .iter()
                        .map(|at| at.as_str().log_color_highlight().to_string())
                        .join(", ")
                ));
                anyhow::bail!(NonSuccessfulExit)
            }
        }

        let mut tool_matchers = sdk_targets
            .tools
            .as_ref()
            .map(|tools| tools.clone().into_set())
            .unwrap_or_default();
        if tool_matchers.remove("*") {
            validate_wildcard_manifest_sources_resolved(
                ctx,
                target_language,
                bridge_mode,
                selected_component_names,
            )?;
        } else {
            for component_name in ctx.application().component_names() {
                if component_matcher_is_not_expected_to_resolve(
                    ctx,
                    selected_component_names,
                    component_name,
                ) {
                    tool_matchers.remove(component_name.as_str());
                }
            }

            for target in exact_targets {
                if target.target_language == target_language
                    && target.bridge_mode == bridge_mode
                    && matches!(
                        target.subject,
                        crate::model::app::BridgeSdkTargetSubject::Tool { .. }
                    )
                {
                    tool_matchers.remove(target.subject.display_name());
                }
            }

            if !tool_matchers.is_empty() {
                for matcher in &tool_matchers {
                    let Ok(name) = ToolName::try_from(matcher.as_str()) else {
                        continue;
                    };
                    if ctx.environment_tool_diagnostic(&name).is_some() {
                        ctx.environment_tool(&name)?;
                    }
                }
                logln("");
                log_error(format!(
                    "The following tool matchers were not found during {} bridge SDK generation: {}",
                    bridge_sdk_target_name(target_language, bridge_mode).log_color_highlight(),
                    tool_matchers
                        .iter()
                        .map(|at| at.as_str().log_color_highlight().to_string())
                        .join(", ")
                ));
                anyhow::bail!(NonSuccessfulExit)
            }
        }
    }

    Ok(())
}

fn validate_wildcard_manifest_sources_resolved(
    ctx: &BuildContext<'_>,
    target_language: GuestLanguage,
    bridge_mode: BridgeMode,
    selected_component_names: &[ComponentName],
) -> anyhow::Result<()> {
    let missing_component_names = selected_component_names
        .iter()
        .filter(|component_name| !ctx.application().component(component_name).wasm().exists())
        .collect::<Vec<_>>();

    if !missing_component_names.is_empty() {
        logln("");
        log_error(format!(
            "The following selected components could not be inspected during {} bridge SDK generation: {}",
            bridge_sdk_target_name(target_language, bridge_mode).log_color_highlight(),
            missing_component_names
                .iter()
                .map(|component_name| component_name.as_str().log_color_highlight().to_string())
                .join(", ")
        ));
        anyhow::bail!(NonSuccessfulExit)
    }

    Ok(())
}

fn component_matcher_is_not_expected_to_resolve(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
    component_name: &ComponentName,
) -> bool {
    let selected = selected_component_names
        .iter()
        .any(|selected_component_name| selected_component_name == component_name);
    let wasm_exists = ctx.application().component(component_name).wasm().exists();

    !selected || wasm_exists
}

fn bridge_sdk_target_name(language: GuestLanguage, bridge_mode: BridgeMode) -> String {
    match bridge_mode {
        BridgeMode::External => language.to_string(),
        BridgeMode::Guest => format!("{} guest", language),
    }
}

fn claim_matches_request(
    claim_request_id: BridgeRequestId,
    target_request_id: BridgeRequestId,
) -> bool {
    claim_request_id == target_request_id
}

fn bridge_requests(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> Vec<BridgeRequestId> {
    let mut requests = Vec::new();

    if ctx.custom_bridge_sdk_target().is_some() {
        requests.push(BridgeRequestId::Custom);
    } else {
        if has_explicit_manifest_guest_bridge_request(ctx, selected_component_names) {
            requests.push(BridgeRequestId::ManifestGuest);
        }
        if ctx
            .application()
            .bridge_sdks()
            .for_all_used_modes()
            .into_iter()
            .any(|(_, mode, sdk_targets)| {
                mode == BridgeMode::External
                    && manifest_bridge_request_may_match_selected_components(
                        ctx,
                        &sdk_targets.agents,
                        selected_component_names,
                    )
            })
        {
            requests.push(BridgeRequestId::ManifestExternal);
        }
    }

    if ctx.repl_bridge_sdk_target().is_some() {
        requests.push(BridgeRequestId::Repl);
    }

    requests
}

fn has_explicit_manifest_guest_bridge_request(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> bool {
    ctx.application()
        .bridge_sdks()
        .for_all_used_modes()
        .into_iter()
        .any(|(_, mode, sdk_targets)| {
            mode == BridgeMode::Guest
                && (manifest_bridge_request_may_match_selected_components(
                    ctx,
                    &sdk_targets.agents,
                    selected_component_names,
                ) || sdk_targets.tools.as_ref().is_some_and(|tools| {
                    manifest_tool_bridge_request_may_match_selected_components(
                        ctx,
                        tools,
                        selected_component_names,
                    )
                }))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::app::{
        Application, ApplicationPreload, BridgeSdkTargetSubject, ComponentPresetSelector,
    };
    use crate::model::app_raw;
    use tempfile::tempdir;
    use test_r::test;

    #[test]
    fn final_manifest_validation_reports_mcp_projection_diagnostic() {
        let temp_dir = tempdir().unwrap();
        let manifest = temp_dir.path().join("golem.yaml");
        crate::fs::write(
            &manifest,
            r#"
app: rejected-import
environments:
  local:
    server: local
mcp:
  imports:
    local:
      - url: https://tools.example/mcp
bridge:
  rust:
    internal:
      tools: [acme-search-files]
"#,
        )
        .unwrap();
        let raw_apps = vec![app_raw::ApplicationWithSource::from_yaml_file(&manifest).unwrap()];
        let (preload, _, errors) = Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let ApplicationPreload {
            application_name,
            environments,
            local_server,
            ..
        } = preload.unwrap();
        let (application, _, errors) = Application::from_raw_apps(
            temp_dir.path().to_path_buf(),
            application_name,
            environments,
            local_server,
            ComponentPresetSelector {
                environment: "local".parse().unwrap(),
                presets: Vec::new(),
            },
            raw_apps,
        )
        .into_product();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let app_ctx = crate::app::context::ApplicationContext::for_test(application.unwrap());
        let build_config = crate::model::app::BuildConfig::default();
        let environment_tools = crate::app::context::ResolvedEnvironmentTools {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            ambient_tools: Vec::new(),
            mcp_tools: Vec::new(),
            mcp_diagnostics: vec![crate::app::context::ResolvedMcpDiagnostic {
                canonical_name: "acme-search-files".into(),
                import_index: 2,
                upstream_name: "Search Files".into(),
                reason: "unsupported schema".into(),
            }],
        };
        let ctx =
            BuildContext::new(&app_ctx, &build_config).with_environment_tools(&environment_tools);

        let error = validate_manifest_guest_matchers_resolved(&ctx, &[], &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("acme-search-files"));
        assert!(error.contains("MCP import 2"));
        assert!(error.contains("Search Files"));
        assert!(error.contains("unsupported schema"));
    }

    #[test]
    fn dependency_guest_target_under_custom_claim_base_is_allowed() {
        let base_dir =
            std::env::temp_dir().join(format!("golem-cli-build-plan-{}", std::process::id()));
        let dependency_guest_target = BridgeSdkTarget {
            source: crate::model::app::BridgeSdkTargetSource::local(
                ComponentName::try_from("app:producer").unwrap(),
            ),
            subject: BridgeSdkTargetSubject::Agent(bar_agent_type()),
            target_language: GuestLanguage::Rust,
            bridge_mode: BridgeMode::Guest,
            output_dir: base_dir.join("golem-temp/bridge-sdk/rust/internal/bar-agent-guest-client"),
            rust_config: Default::default(),
        };
        let custom_claim = OutputDirClaim {
            request_id: BridgeRequestId::Custom,
            base_dir,
            description: "custom bridge target".to_string(),
        };

        assert!(
            validate_exact_targets_against_claims(
                &[(BridgeRequestId::DependencyGuest, dependency_guest_target)],
                &[custom_claim],
            )
            .is_ok()
        );
    }

    fn bar_agent_type() -> golem_common::schema::AgentTypeSchema {
        let agent_types =
            serde_json::from_str::<Vec<golem_common::schema::AgentTypeSchema>>(include_str!(
                "../../../test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"
            ))
            .unwrap();
        agent_types
            .into_iter()
            .find(|agent_type| agent_type.type_name.as_str() == "BarAgent")
            .unwrap()
    }
}
