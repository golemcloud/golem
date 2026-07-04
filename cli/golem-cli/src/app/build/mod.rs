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
use crate::app::build::add_metadata::add_metadata_to_components;
use crate::app::build::componentize::build_selected_components;
use crate::app::build::gen_bridge::{
    BridgeGenerationPlan, ComponentMetadataCache, gen_bridge_sdk_targets,
    plan_custom_bridge_generation, plan_dependency_guest_bridge_generation_for_components_lenient,
    plan_explicit_manifest_guest_bridge_generation_for_components_lenient,
    plan_manifest_external_bridge_generation_for_components_lenient,
    plan_repl_bridge_generation_lenient, validate_no_output_dir_collisions,
    validate_supported_bridge_targets, write_repl_metadata,
};
use crate::app::component_metadata::tool_name;
use crate::app::context::BuildContext;
use crate::bridge_gen::BridgeMode;
use crate::error::NonSuccessfulExit;
use crate::log::{LogColorize, log_error, logln};
use crate::model::GuestLanguage;
use crate::model::app::{
    AppBuildStep, BridgeSdkTarget, ComponentDependency, CustomBridgeSdkTarget,
};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::component::ComponentName;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
pub mod add_metadata;
pub mod check;
pub mod clean;
pub mod command;
pub mod componentize;
pub mod extract_component_metadata;
pub mod gen_bridge;
pub mod task_result_marker;
pub mod up_to_date_check;

pub async fn build_app(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    if ctx.should_run_step(AppBuildStep::Check) {
        check_build_tool_requirements(ctx).await?;
    }

    build_app_with_build_plan(ctx).await?;

    Ok(())
}

async fn build_app_with_build_plan(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    let mut agent_metadata_cache = ComponentMetadataCache::default();
    let explicit_bridge_component_names = ctx
        .application_context()
        .selected_component_names()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let has_dependency_guest_request = ctx.should_run_step(AppBuildStep::GenBridge)
        && selected_component_names_have_guest_bridge_dependencies(
            ctx,
            &explicit_bridge_component_names,
        );
    let effective_component_names = if has_dependency_guest_request {
        selected_component_names_with_dependencies_for_build(ctx, &mut agent_metadata_cache).await?
    } else {
        explicit_bridge_component_names.clone()
    };
    let requests = if ctx.should_run_step(AppBuildStep::GenBridge) {
        bridge_requests(ctx, &explicit_bridge_component_names)
    } else {
        Vec::new()
    };
    let mut validated_targets = Vec::<(BridgeRequestId, BridgeSdkTarget)>::new();
    let claims = bridge_output_dir_claims(ctx, &explicit_bridge_component_names);
    let mut pending_components = effective_component_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut built_components = BTreeSet::<ComponentName>::new();
    let mut dependency_providers = DependencyProviderMap::default();
    let mut available_guest_bridge_dependencies = BTreeSet::<ComponentDependency>::new();
    let mut generated_guest_target_keys = BTreeSet::<BridgeSdkTargetKey>::new();

    build_components_with_dependency_ordering(
        ctx,
        &effective_component_names,
        has_dependency_guest_request,
        &mut agent_metadata_cache,
        &claims,
        &mut validated_targets,
        &mut generated_guest_target_keys,
        &mut pending_components,
        &mut built_components,
        &mut dependency_providers,
        &mut available_guest_bridge_dependencies,
    )
    .await?;

    for request_id in [
        BridgeRequestId::Custom,
        BridgeRequestId::ManifestGuest,
        BridgeRequestId::ManifestExternal,
        BridgeRequestId::Repl,
    ] {
        if !requests.contains(&request_id) {
            continue;
        }

        let plan = plan_bridge_request(
            ctx,
            request_id,
            &explicit_bridge_component_names,
            &mut agent_metadata_cache,
        )
        .await?;

        let mut request_target_keys = BTreeSet::new();
        let generated_target_keys = if request_id == BridgeRequestId::ManifestGuest {
            &mut generated_guest_target_keys
        } else {
            &mut request_target_keys
        };
        let targets = validate_and_filter_new_bridge_targets(
            ctx,
            request_id,
            plan.targets.clone(),
            &mut validated_targets,
            &claims,
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
            gen_bridge_sdk_targets(ctx, targets).await?;
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
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn build_components_with_dependency_ordering(
    ctx: &BuildContext<'_>,
    effective_component_names: &[ComponentName],
    has_dependency_guest_request: bool,
    agent_metadata_cache: &mut ComponentMetadataCache,
    claims: &[OutputDirClaim],
    validated_targets: &mut Vec<(BridgeRequestId, BridgeSdkTarget)>,
    generated_guest_target_keys: &mut BTreeSet<BridgeSdkTargetKey>,
    pending_components: &mut BTreeSet<ComponentName>,
    built_components: &mut BTreeSet<ComponentName>,
    dependency_providers: &mut DependencyProviderMap,
    available_guest_bridge_dependencies: &mut BTreeSet<ComponentDependency>,
) -> anyhow::Result<()> {
    while !pending_components.is_empty() {
        let buildable_components = pending_components
            .iter()
            .filter(|component_name| {
                !has_dependency_guest_request
                    || component_guest_bridge_requirements(ctx, component_name)
                        .is_subset(available_guest_bridge_dependencies)
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut made_progress = false;

        for component_name in buildable_components {
            let component_names = [component_name.clone()];
            if ctx.should_run_step(AppBuildStep::Build) {
                build_selected_components(ctx, &component_names).await?;
            }
            if ctx.should_run_step(AppBuildStep::AddMetadata) {
                add_metadata_to_components(ctx, &component_names).await?;
            }

            pending_components.remove(&component_name);
            built_components.insert(component_name.clone());
            if has_dependency_guest_request {
                let component = ctx.application().component(&component_name);
                if component.agent_type_extraction_source_wasm().exists() {
                    let metadata = agent_metadata_cache.get(ctx, &component_name).await?;
                    dependency_providers.add_component_metadata(&component_name, &metadata);
                    available_guest_bridge_dependencies.extend(
                        component_guest_bridge_dependencies_provided_by_metadata(&metadata),
                    );
                }
            }
            made_progress = true;
        }

        if has_dependency_guest_request {
            let dependency_guest_requirements =
                selected_guest_bridge_dependency_sources(ctx, effective_component_names);
            for component_name in built_components.iter() {
                if !ctx
                    .application()
                    .component(component_name)
                    .agent_type_extraction_source_wasm()
                    .exists()
                {
                    continue;
                }
                let metadata = agent_metadata_cache.get(ctx, component_name).await?;
                dependency_providers.add_component_metadata(component_name, &metadata);
            }

            let built_component_names = dependency_providers
                .providers_for_any(&dependency_guest_requirements)
                .into_iter()
                .filter(|component_name| built_components.contains(component_name))
                .collect::<BTreeSet<_>>();
            let built_component_names = built_component_names.into_iter().collect::<Vec<_>>();
            let source_component_names = built_component_names
                .iter()
                .filter(|component_name| {
                    ctx.application()
                        .component(component_name)
                        .agent_type_extraction_source_wasm()
                        .exists()
                })
                .cloned()
                .collect::<Vec<_>>();
            let plan = plan_dependency_guest_bridge_generation_for_components_lenient(
                ctx,
                &source_component_names,
                effective_component_names,
                agent_metadata_cache,
            )
            .await?;

            let new_targets = validate_and_filter_new_bridge_targets(
                ctx,
                BridgeRequestId::DependencyGuest,
                plan.targets,
                validated_targets,
                claims,
                generated_guest_target_keys,
            )?;

            if !new_targets.is_empty() {
                gen_bridge_sdk_targets(ctx, new_targets.clone()).await?;
            }

            let available_before = available_guest_bridge_dependencies.len();
            for component_name in built_component_names {
                if !ctx
                    .application()
                    .component(&component_name)
                    .agent_type_extraction_source_wasm()
                    .exists()
                {
                    continue;
                }
                let metadata = agent_metadata_cache.get(ctx, &component_name).await?;
                dependency_providers.add_component_metadata(&component_name, &metadata);
                available_guest_bridge_dependencies.extend(
                    component_guest_bridge_dependencies_provided_by_metadata(&metadata),
                );
            }
            if available_guest_bridge_dependencies.len() != available_before {
                made_progress = true;
            }
        }

        if !made_progress {
            report_guest_bridge_dependency_ordering_cycle(
                ctx,
                &pending_components,
                &available_guest_bridge_dependencies,
            )?;
        }
    }

    Ok(())
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
    validate_no_output_dir_collisions(&exact_targets)?;
    validate_exact_targets_against_claims(&tagged_targets, claims)?;

    for key in new_target_keys {
        generated_target_keys.insert(key);
    }
    for target in &new_targets {
        validated_targets.push((request_id, target.clone()));
    }

    Ok(new_targets)
}

fn report_guest_bridge_dependency_ordering_cycle(
    ctx: &BuildContext<'_>,
    pending_components: &BTreeSet<ComponentName>,
    available_guest_bridge_dependencies: &BTreeSet<ComponentDependency>,
) -> anyhow::Result<()> {
    logln("");
    log_error("Build dependency ordering has a cycle involving guest bridge generation:");
    for component_name in pending_components {
        let missing = component_guest_bridge_requirements(ctx, component_name)
            .difference(available_guest_bridge_dependencies)
            .map(|dependency| match dependency {
                ComponentDependency::Agent(agent_type_name) => {
                    format!("agent {}", agent_type_name.as_str())
                }
                ComponentDependency::Tool(tool_name) => format!("tool {}", tool_name.as_str()),
            })
            .collect::<Vec<_>>();
        log_error(format!(
            "  component {} waits for guest bridge generation for: {}",
            component_name.as_str().log_color_highlight(),
            missing
                .iter()
                .map(|component| component.log_color_highlight())
                .join(", "),
        ));
    }
    anyhow::bail!(NonSuccessfulExit)
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct BridgeSdkTargetKey {
    component_name: ComponentName,
    target_name: String,
    kind: &'static str,
    target_language: GuestLanguage,
    bridge_mode: BridgeMode,
    output_dir: PathBuf,
}

impl BridgeSdkTargetKey {
    fn from_bridge_target(ctx: &BuildContext<'_>, target: &BridgeSdkTarget) -> Self {
        Self {
            component_name: target.component_name.clone(),
            target_name: target.kind.display_name().to_string(),
            kind: match &target.kind {
                crate::model::app::BridgeSdkTargetKind::Agent(_) => "agent",
                crate::model::app::BridgeSdkTargetKind::Tool(_) => "tool",
            },
            target_language: target.target_language,
            bridge_mode: target.bridge_mode,
            output_dir: crate::fs::absolute_lexical_path(&target.output_dir).unwrap_or_else(|_| {
                ctx.application()
                    .bridge_sdks_source()
                    .join(&target.output_dir)
            }),
        }
    }
}

#[derive(Default)]
struct DependencyProviderMap {
    providers_by_dependency: BTreeMap<ComponentDependency, BTreeSet<ComponentName>>,
}

impl DependencyProviderMap {
    fn add_component_metadata(
        &mut self,
        component_name: &ComponentName,
        metadata: &ExtractedComponentMetadata,
    ) {
        for dependency in component_guest_bridge_dependencies_provided_by_metadata(metadata) {
            self.providers_by_dependency
                .entry(dependency)
                .or_default()
                .insert(component_name.clone());
        }
    }

    fn providers_for_any(
        &self,
        dependencies: &BTreeSet<ComponentDependency>,
    ) -> BTreeSet<ComponentName> {
        dependencies
            .iter()
            .filter_map(|dependency| self.providers_by_dependency.get(dependency))
            .flatten()
            .cloned()
            .collect()
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

fn component_guest_bridge_dependencies_provided_by_metadata(
    metadata: &ExtractedComponentMetadata,
) -> BTreeSet<ComponentDependency> {
    let mut dependencies = BTreeSet::new();
    dependencies.extend(
        metadata
            .agent_types
            .iter()
            .map(|agent_type| ComponentDependency::Agent(agent_type.type_name.clone())),
    );
    dependencies.extend(metadata.tools.iter().filter_map(|tool| {
        tool_name(&tool)
            .and_then(|name| crate::model::app::ToolName::try_from(name).ok())
            .map(ComponentDependency::Tool)
    }));
    dependencies
}

fn selected_guest_bridge_dependency_sources(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> BTreeSet<ComponentDependency> {
    selected_component_names
        .iter()
        .flat_map(|component_name| component_guest_bridge_requirements(ctx, component_name))
        .collect()
}

fn selected_component_names_have_guest_bridge_dependencies(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> bool {
    selected_component_names
        .iter()
        .any(|component_name| !component_guest_bridge_requirements(ctx, component_name).is_empty())
}

async fn selected_component_names_with_dependencies_for_build(
    ctx: &BuildContext<'_>,
    agent_metadata_cache: &mut ComponentMetadataCache,
) -> anyhow::Result<Vec<ComponentName>> {
    let selected_component_names = ctx
        .application_context()
        .selected_component_names()
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    let mut selected = BTreeSet::<ComponentName>::new();
    selected.extend(selected_component_names);

    loop {
        let mut provided = BTreeSet::new();
        for component_name in &selected {
            if let Some(metadata) = component_metadata_for_guest_bridge_dependency_for_build(
                ctx,
                component_name,
                agent_metadata_cache,
            )
            .await?
            {
                provided.extend(component_guest_bridge_dependencies_provided_by_metadata(
                    &metadata,
                ));
            }
        }
        let requirements = selected
            .iter()
            .flat_map(|component_name| component_guest_bridge_requirements(ctx, component_name))
            .filter(|dependency| !provided.contains(dependency))
            .collect::<BTreeSet<_>>();

        if requirements.is_empty() {
            break;
        }

        let selected_count_before = selected.len();
        for component_name in ctx.application().component_names() {
            if selected.contains(component_name) {
                continue;
            }
            if component_provides_any_guest_bridge_dependency_for_build(
                ctx,
                component_name,
                &requirements,
                agent_metadata_cache,
            )
            .await?
            {
                selected.insert(component_name.clone());
            }
        }

        if selected.len() == selected_count_before {
            let selected_count_before_fallback = selected.len();
            for component_name in ctx.application().component_names() {
                if selected.contains(component_name) {
                    continue;
                }
                if component_metadata_for_guest_bridge_dependency_for_build(
                    ctx,
                    component_name,
                    agent_metadata_cache,
                )
                .await?
                .is_none()
                {
                    selected.insert(component_name.clone());
                }
            }
            if selected.len() != selected_count_before_fallback {
                continue;
            }
            break;
        }
    }

    Ok(selected.into_iter().collect())
}

async fn component_provides_any_guest_bridge_dependency_for_build(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
    requirements: &BTreeSet<ComponentDependency>,
    agent_metadata_cache: &mut ComponentMetadataCache,
) -> anyhow::Result<bool> {
    let Some(metadata) = component_metadata_for_guest_bridge_dependency_for_build(
        ctx,
        component_name,
        agent_metadata_cache,
    )
    .await?
    else {
        return Ok(false);
    };

    Ok(
        component_guest_bridge_dependencies_provided_by_metadata(&metadata)
            .iter()
            .any(|dependency| requirements.contains(dependency)),
    )
}

async fn component_metadata_for_guest_bridge_dependency_for_build(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
    agent_metadata_cache: &mut ComponentMetadataCache,
) -> anyhow::Result<Option<ExtractedComponentMetadata>> {
    let component = ctx.application().component(component_name);
    let metadata = if let Some(metadata) = stored_component_metadata_if_fresh(ctx, component_name) {
        Some(metadata)
    } else if component.agent_type_extraction_source_wasm().exists() {
        agent_metadata_cache.get(ctx, component_name).await.ok()
    } else {
        None
    };

    Ok(metadata)
}

fn stored_component_metadata_if_fresh(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> Option<ExtractedComponentMetadata> {
    let component = ctx.application().component(component_name);
    let wasm = component.agent_type_extraction_source_wasm();
    let extracted_component_metadata = component.extracted_component_metadata(&wasm);
    if !extracted_component_metadata.exists()
        || (wasm.exists() && !path_is_fresh_against(&extracted_component_metadata, &wasm))
    {
        return None;
    }

    crate::fs::read_to_string(&extracted_component_metadata)
        .ok()
        .and_then(|contents| serde_json::from_str::<ExtractedComponentMetadata>(&contents).ok())
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
    agent_metadata_cache: &mut ComponentMetadataCache,
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
                agent_metadata_cache,
            )
            .await
        }
        BridgeRequestId::ManifestExternal => {
            plan_manifest_external_bridge_generation_for_components_lenient(
                ctx,
                selected_component_names,
                agent_metadata_cache,
            )
            .await
        }
        BridgeRequestId::Custom => {
            plan_custom_bridge_generation(
                ctx,
                ctx.custom_bridge_sdk_target()
                    .expect("custom bridge request requires a custom target"),
                agent_metadata_cache,
            )
            .await
        }
        BridgeRequestId::Repl => {
            plan_repl_bridge_generation_lenient(
                ctx,
                ctx.repl_bridge_sdk_target()
                    .expect("REPL bridge request requires a REPL target"),
                agent_metadata_cache,
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
                sdk_targets.agents,
                selected_component_names,
            ) {
                add_manifest_bridge_output_dir_claims(
                    ctx,
                    &mut claims,
                    language,
                    mode,
                    sdk_targets.agents,
                    sdk_targets.output_dir.map(|output_dir| output_dir.as_str()),
                    selected_component_names,
                );
            }

            if mode == BridgeMode::Guest
                && let Some(tools) = sdk_targets.tools
                && manifest_bridge_request_may_match_selected_components(
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
                    sdk_targets.output_dir.map(|output_dir| output_dir.as_str()),
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
    output_dir: Option<&str>,
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
    output_dir: Option<&str>,
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
    if ctx.should_run_step(AppBuildStep::Build) && !component.build_commands().is_empty() {
        return false;
    }
    if ctx.should_run_step(AppBuildStep::AddMetadata) {
        return false;
    }

    let wasm = component.agent_type_extraction_source_wasm();
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
                let Some(name) = tool_name(&tool) else {
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
    output_dir: Option<&str>,
) -> PathBuf {
    match output_dir {
        Some(output_dir) => ctx.application().bridge_sdks_source().join(output_dir),
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
    let matchers = agents.clone().into_set();
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
                    target.kind.display_name(),
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
                    && let Some(agent_type) = target.kind.as_agent()
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
                    && matches!(target.kind, crate::model::app::BridgeSdkTargetKind::Tool(_))
                {
                    tool_matchers.remove(target.kind.display_name());
                }
            }

            if !tool_matchers.is_empty() {
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
        .filter(|component_name| {
            !ctx.application()
                .component(component_name)
                .agent_type_extraction_source_wasm()
                .exists()
        })
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
    let wasm_exists = ctx
        .application()
        .component(component_name)
        .agent_type_extraction_source_wasm()
        .exists();

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
                        sdk_targets.agents,
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
                    sdk_targets.agents,
                    selected_component_names,
                ) || sdk_targets.tools.is_some_and(|tools| {
                    manifest_bridge_request_may_match_selected_components(
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
    use crate::model::app::BridgeSdkTargetKind;
    use test_r::test;

    #[test]
    fn dependency_guest_target_under_custom_claim_base_is_allowed() {
        let base_dir =
            std::env::temp_dir().join(format!("golem-cli-build-plan-{}", std::process::id()));
        let dependency_guest_target = BridgeSdkTarget {
            component_name: ComponentName::try_from("app:producer").unwrap(),
            kind: BridgeSdkTargetKind::Agent(bar_agent_type()),
            target_language: GuestLanguage::Rust,
            bridge_mode: BridgeMode::Guest,
            output_dir: base_dir.join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client"),
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
