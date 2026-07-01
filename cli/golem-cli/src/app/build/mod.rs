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
use crate::app::build::add_metadata::{
    add_metadata_to_components, add_metadata_to_selected_components,
};
use crate::app::build::componentize::{build_components, build_selected_components};
use crate::app::build::gen_bridge::{
    AgentMetadataCache, BridgeGenerationPlan, gen_bridge, gen_bridge_sdk_targets,
    plan_custom_bridge_generation_lenient,
    plan_manifest_external_bridge_generation_for_components_lenient,
    plan_manifest_guest_bridge_generation_for_components_lenient,
    plan_repl_bridge_generation_lenient, validate_no_output_dir_collisions,
    validate_supported_bridge_targets, write_repl_metadata,
};
use crate::app::context::BuildContext;
use crate::app::edit::cargo_toml::{self, DependencySpec};
use crate::bridge_gen::BridgeMode;
use crate::error::NonSuccessfulExit;
use crate::log::{LogColorize, log_error, logln};
use crate::model::GuestLanguage;
use crate::model::app::{AppBuildStep, BridgeSdkTarget, CustomBridgeSdkTarget};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentName;
use golem_common::schema::AgentTypeSchema;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
pub mod add_metadata;
pub mod check;
pub mod clean;
pub mod command;
pub mod componentize;
pub mod extract_agent_type;
pub mod gen_bridge;
pub mod task_result_marker;
pub mod up_to_date_check;

pub async fn build_app(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    if ctx.should_run_step(AppBuildStep::Check) {
        check_build_tool_requirements(ctx).await?;
    }

    if should_run_bridge_scheduler(ctx) {
        build_app_with_bridge_scheduler(ctx).await?;
        return Ok(());
    }

    if ctx.should_run_step(AppBuildStep::Build) {
        build_components(ctx).await?;
    }
    if ctx.should_run_step(AppBuildStep::AddMetadata) {
        add_metadata_to_selected_components(ctx).await?;
    }
    if ctx.should_run_step(AppBuildStep::GenBridge) {
        gen_bridge(ctx).await?;
    }

    Ok(())
}

async fn build_app_with_bridge_scheduler(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    let selected_component_names = ctx
        .application_context()
        .selected_component_names()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let guest_source_component_names = selected_component_names
        .iter()
        .filter(|component_name| !component_build_may_need_guest_bridge(ctx, component_name))
        .cloned()
        .collect::<Vec<_>>();
    let guest_consumer_component_names = selected_component_names
        .iter()
        .filter(|component_name| component_build_may_need_guest_bridge(ctx, component_name))
        .cloned()
        .collect::<Vec<_>>();

    let requests = bridge_requests(ctx, &selected_component_names);
    if requests.contains(&BridgeRequestId::ManifestGuestPreBuild)
        && guest_source_component_names.is_empty()
        && !guest_consumer_component_names.is_empty()
    {
        logln("");
        log_error("Build graph has a cycle involving guest bridge generation:".to_string());
        for component_name in &guest_consumer_component_names {
            log_error(format!(
                "  component {} build needs manifest guest bridge generation",
                component_name.as_str().log_color_highlight(),
            ));
        }
        log_error("  manifest guest bridge generation needs agent metadata from a component that can be built before the guest bridge is generated".to_string());
        anyhow::bail!(NonSuccessfulExit)
    }

    let graph = build_bridge_scheduler_graph(
        ctx,
        &selected_component_names,
        &guest_source_component_names,
        &guest_consumer_component_names,
    );
    let sorted_nodes = topological_sort(&graph)?;
    let mut agent_metadata_cache = AgentMetadataCache::default();
    let mut bridge_plans = BTreeMap::<BridgeRequestId, BridgeGenerationPlan>::new();
    let mut validated_targets = Vec::<(BridgeRequestId, BridgeSdkTarget)>::new();
    let claims = bridge_output_dir_claims(ctx, &selected_component_names);

    for node in sorted_nodes {
        match node {
            BuildNode::BuildComponent(component_name) => {
                let component_names = [component_name];
                build_selected_components(ctx, &component_names).await?;
            }
            BuildNode::AddMetadata(component_name) => {
                let component_names = [component_name];
                add_metadata_to_components(ctx, &component_names).await?;
            }
            BuildNode::AgentMetadata(component_name) => {
                agent_metadata_cache.get(ctx, &component_name).await?;
            }
            BuildNode::ValidateBridgeRequest(request_id) => {
                if request_id == BridgeRequestId::ManifestGuestPostBuild {
                    let exact_targets = validated_targets
                        .iter()
                        .map(|(_, target)| target.clone())
                        .collect::<Vec<_>>();
                    if !manifest_guest_needs_post_build_scan(
                        ctx,
                        &exact_targets,
                        &guest_consumer_component_names,
                    ) {
                        validate_manifest_guest_matchers_resolved(ctx, &exact_targets)?;
                        bridge_plans.insert(request_id, BridgeGenerationPlan::default());
                        continue;
                    }
                }

                let plan = plan_bridge_request(
                    ctx,
                    request_id,
                    &selected_component_names,
                    &guest_source_component_names,
                    &mut agent_metadata_cache,
                )
                .await?;

                let mut exact_targets = validated_targets
                    .iter()
                    .map(|(_, target)| target.clone())
                    .collect::<Vec<_>>();
                exact_targets.extend(plan.targets.iter().cloned());
                validate_supported_bridge_targets(&exact_targets)?;
                validate_no_output_dir_collisions(&exact_targets)?;
                let mut tagged_targets = validated_targets.clone();
                tagged_targets.extend(
                    plan.targets
                        .iter()
                        .cloned()
                        .map(|target| (request_id, target)),
                );
                match request_id {
                    BridgeRequestId::ManifestGuestPostBuild => {
                        validate_manifest_guest_matchers_resolved(ctx, &exact_targets)?;
                    }
                    BridgeRequestId::ManifestExternal => {
                        validate_manifest_external_matchers_resolved(ctx, &exact_targets)?;
                    }
                    _ => {}
                }
                validate_exact_targets_against_claims(&tagged_targets, &claims)?;
                validated_targets = tagged_targets;
                bridge_plans.insert(request_id, plan);
            }
            BuildNode::GenerateBridgeRequest(request_id) => {
                let plan = bridge_plans.remove(&request_id).unwrap_or_default();
                if !plan.targets.is_empty() {
                    write_repl_metadata(ctx, &plan).await?;
                    gen_bridge_sdk_targets(ctx, plan.targets).await?;
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum BridgeRequestId {
    ManifestGuestPreBuild,
    ManifestGuestPostBuild,
    ManifestExternal,
    Custom,
    Repl,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum BuildNode {
    BuildComponent(ComponentName),
    AddMetadata(ComponentName),
    AgentMetadata(ComponentName),
    ValidateBridgeRequest(BridgeRequestId),
    GenerateBridgeRequest(BridgeRequestId),
}

fn build_bridge_scheduler_graph(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
    guest_source_component_names: &[ComponentName],
    guest_consumer_component_names: &[ComponentName],
) -> BTreeMap<BuildNode, BTreeSet<BuildNode>> {
    let mut graph = BTreeMap::<BuildNode, BTreeSet<BuildNode>>::new();
    let requests = bridge_requests(ctx, selected_component_names);

    for component_name in selected_component_names {
        let build_component = BuildNode::BuildComponent(component_name.clone());
        let add_metadata = BuildNode::AddMetadata(component_name.clone());

        if ctx.should_run_step(AppBuildStep::Build) {
            graph.entry(build_component.clone()).or_default();
        }
        if ctx.should_run_step(AppBuildStep::AddMetadata) {
            graph.entry(add_metadata.clone()).or_default();
            if ctx.should_run_step(AppBuildStep::Build) {
                graph
                    .entry(add_metadata.clone())
                    .or_default()
                    .insert(build_component.clone());
            }
        }
    }

    for request in &requests {
        let validate = BuildNode::ValidateBridgeRequest(*request);
        let generate = BuildNode::GenerateBridgeRequest(*request);
        graph.entry(validate.clone()).or_default();
        graph.entry(generate).or_default().insert(validate.clone());

        let metadata_components = match request {
            BridgeRequestId::ManifestGuestPreBuild => guest_source_component_names.to_vec(),
            BridgeRequestId::ManifestGuestPostBuild => vec![],
            BridgeRequestId::ManifestExternal => {
                manifest_external_metadata_component_names(ctx, selected_component_names)
            }
            BridgeRequestId::Custom | BridgeRequestId::Repl => selected_component_names.to_vec(),
        };
        for component_name in &metadata_components {
            ensure_agent_metadata_node(ctx, &mut graph, component_name);
            graph
                .entry(validate.clone())
                .or_default()
                .insert(BuildNode::AgentMetadata(component_name.clone()));
        }

        if *request == BridgeRequestId::ManifestGuestPostBuild {
            if requests.contains(&BridgeRequestId::ManifestGuestPreBuild) {
                graph.entry(validate.clone()).or_default().insert(
                    BuildNode::GenerateBridgeRequest(BridgeRequestId::ManifestGuestPreBuild),
                );
            }
            for component_name in guest_consumer_component_names {
                let dependency = if ctx.should_run_step(AppBuildStep::AddMetadata) {
                    BuildNode::AddMetadata(component_name.clone())
                } else {
                    BuildNode::BuildComponent(component_name.clone())
                };
                graph
                    .entry(validate.clone())
                    .or_default()
                    .insert(dependency);
            }
        }
    }

    if requests.contains(&BridgeRequestId::ManifestGuestPreBuild) {
        if requests.contains(&BridgeRequestId::ManifestExternal)
            && guest_consumer_component_names.is_empty()
        {
            graph
                .entry(BuildNode::ValidateBridgeRequest(
                    BridgeRequestId::ManifestGuestPreBuild,
                ))
                .or_default()
                .insert(BuildNode::ValidateBridgeRequest(
                    BridgeRequestId::ManifestExternal,
                ));
            graph
                .entry(BuildNode::GenerateBridgeRequest(
                    BridgeRequestId::ManifestExternal,
                ))
                .or_default()
                .insert(BuildNode::ValidateBridgeRequest(
                    BridgeRequestId::ManifestGuestPreBuild,
                ));
        }

        if !guest_consumer_component_names.is_empty()
            && requests.contains(&BridgeRequestId::ManifestGuestPostBuild)
        {
            for request in [BridgeRequestId::ManifestExternal, BridgeRequestId::Repl] {
                if requests.contains(&request) {
                    graph
                        .entry(BuildNode::ValidateBridgeRequest(request))
                        .or_default()
                        .insert(BuildNode::ValidateBridgeRequest(
                            BridgeRequestId::ManifestGuestPostBuild,
                        ));
                }
            }
        }

        let guest_generate =
            BuildNode::GenerateBridgeRequest(BridgeRequestId::ManifestGuestPreBuild);
        for component_name in guest_consumer_component_names {
            graph
                .entry(BuildNode::BuildComponent(component_name.clone()))
                .or_default()
                .insert(guest_generate.clone());
        }
    }

    graph
}

fn ensure_agent_metadata_node(
    ctx: &BuildContext<'_>,
    graph: &mut BTreeMap<BuildNode, BTreeSet<BuildNode>>,
    component_name: &ComponentName,
) {
    let agent_metadata = BuildNode::AgentMetadata(component_name.clone());
    graph.entry(agent_metadata.clone()).or_default();
    if ctx.should_run_step(AppBuildStep::AddMetadata) {
        graph
            .entry(agent_metadata)
            .or_default()
            .insert(BuildNode::AddMetadata(component_name.clone()));
    } else if ctx.should_run_step(AppBuildStep::Build) {
        graph
            .entry(agent_metadata)
            .or_default()
            .insert(BuildNode::BuildComponent(component_name.clone()));
    }
}

fn topological_sort(
    graph: &BTreeMap<BuildNode, BTreeSet<BuildNode>>,
) -> anyhow::Result<Vec<BuildNode>> {
    let mut remaining = graph.clone();
    let mut result = Vec::new();

    while !remaining.is_empty() {
        let Some(ready) = remaining
            .iter()
            .find_map(|(node, dependencies)| dependencies.is_empty().then_some(node.clone()))
        else {
            logln("");
            log_error("Build graph has a cycle involving:".to_string());
            for (node, dependencies) in &remaining {
                log_error(format!(
                    "  {} needs {}",
                    describe_build_node(node),
                    dependencies.iter().map(describe_build_node).join(", ")
                ));
            }
            anyhow::bail!(NonSuccessfulExit)
        };

        remaining.remove(&ready);
        for dependencies in remaining.values_mut() {
            dependencies.remove(&ready);
        }
        result.push(ready);
    }

    Ok(result)
}

async fn plan_bridge_request(
    ctx: &BuildContext<'_>,
    request_id: BridgeRequestId,
    selected_component_names: &[ComponentName],
    guest_source_component_names: &[ComponentName],
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    match request_id {
        BridgeRequestId::ManifestGuestPreBuild => {
            plan_manifest_guest_bridge_generation_for_components_lenient(
                ctx,
                guest_source_component_names,
                agent_metadata_cache,
            )
            .await
        }
        BridgeRequestId::ManifestGuestPostBuild => {
            let guest_consumer_component_names = selected_component_names
                .iter()
                .filter(|component_name| component_build_may_need_guest_bridge(ctx, component_name))
                .cloned()
                .collect::<Vec<_>>();
            plan_manifest_guest_bridge_generation_for_components_lenient(
                ctx,
                &guest_consumer_component_names,
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
            plan_custom_bridge_generation_lenient(
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
            if !manifest_bridge_request_may_match_selected_components(
                ctx,
                sdk_targets.agents,
                selected_component_names,
            ) {
                continue;
            }
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
        BridgeMode::Guest => BridgeRequestId::ManifestGuestPreBuild,
        BridgeMode::External => BridgeRequestId::ManifestExternal,
    };
    let base_dir = manifest_bridge_claim_base(ctx, language, mode, output_dir);
    let component_names = ctx
        .application()
        .component_names()
        .cloned()
        .collect::<BTreeSet<_>>();

    for matcher in agents.clone().into_set() {
        if matcher == "*"
            || component_names
                .iter()
                .any(|component_name| component_name.as_str() == matcher.as_str())
        {
            if matcher == "*"
                || selected_component_names
                    .iter()
                    .any(|component_name| component_name.as_str() == matcher.as_str())
            {
                if matcher == "*"
                    || !add_existing_component_manifest_bridge_output_dir_claims(
                        ctx, claims, request_id, language, mode, &matcher,
                    )
                {
                    claims.push(OutputDirClaim {
                        request_id,
                        base_dir: base_dir.clone(),
                        description: format!("{language} {mode:?} manifest bridge outputDir"),
                    });
                }
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

fn add_existing_component_manifest_bridge_output_dir_claims(
    ctx: &BuildContext<'_>,
    claims: &mut Vec<OutputDirClaim>,
    request_id: BridgeRequestId,
    language: GuestLanguage,
    mode: BridgeMode,
    component_name: &str,
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
    let extracted_agent_types = component.extracted_agent_types(&wasm);
    if !extracted_agent_types.exists() {
        return false;
    }
    if !path_is_fresh_against(&extracted_agent_types, &wasm) {
        return false;
    }

    let Ok(agent_types) = crate::fs::read_to_string(&extracted_agent_types).and_then(|contents| {
        serde_json::from_str::<Vec<AgentTypeSchema>>(&contents).map_err(Into::into)
    }) else {
        return false;
    };

    for agent_type in agent_types {
        claims.push(OutputDirClaim {
            request_id,
            base_dir: ctx
                .application()
                .bridge_sdk_dir(&agent_type.type_name, language, mode),
            description: format!(
                "{} manifest bridge target for {} from {}",
                bridge_sdk_target_name(language, mode),
                agent_type.type_name.as_str(),
                component_name.as_str(),
            ),
        });
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
            if claim.request_id == BridgeRequestId::ManifestGuestPreBuild
                && *target_request_id == BridgeRequestId::ManifestExternal
            {
                continue;
            }
            if claim_matches_request(claim.request_id, *target_request_id) {
                continue;
            }
            let claim_base_dir = crate::fs::absolute_lexical_path(&claim.base_dir)?;
            if target_output_dir == claim_base_dir
                || target_output_dir.starts_with(&claim_base_dir)
                || claim_base_dir.starts_with(&target_output_dir)
            {
                logln("");
                log_error(format!(
                    "Bridge SDK target output directory {} for {} may overlap unresolved {} at {}",
                    target_output_dir.log_color_highlight(),
                    target.agent_type.type_name.as_str(),
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
    exact_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    validate_manifest_matchers_resolved(ctx, BridgeMode::Guest, exact_targets)
}

fn validate_manifest_external_matchers_resolved(
    ctx: &BuildContext<'_>,
    exact_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    validate_manifest_matchers_resolved(ctx, BridgeMode::External, exact_targets)
}

fn validate_manifest_matchers_resolved(
    ctx: &BuildContext<'_>,
    bridge_mode_filter: BridgeMode,
    exact_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    for (target_language, bridge_mode, sdk_targets) in
        ctx.application().bridge_sdks().for_all_used_modes()
    {
        if bridge_mode != bridge_mode_filter {
            continue;
        }

        let mut matchers = sdk_targets.agents.clone().into_set();
        if matchers.remove("*") {
            continue;
        }

        for component_name in ctx.application().component_names() {
            matchers.remove(component_name.as_str());
        }

        for target in exact_targets {
            if target.target_language == target_language && target.bridge_mode == bridge_mode {
                matchers.remove(target.agent_type.type_name.as_str());
            }
        }

        if !matchers.is_empty() {
            logln("");
            log_error(format!(
                "The following agent matchers were not found during {} bridge SDK generation: {}",
                bridge_sdk_target_name(target_language, bridge_mode).log_color_highlight(),
                matchers
                    .iter()
                    .map(|at| at.as_str().log_color_highlight().to_string())
                    .join(", ")
            ));
            anyhow::bail!(NonSuccessfulExit)
        }
    }

    Ok(())
}

fn bridge_sdk_target_name(language: GuestLanguage, bridge_mode: BridgeMode) -> String {
    match bridge_mode {
        BridgeMode::External => language.to_string(),
        BridgeMode::Guest => format!("{} guest", language),
    }
}

fn manifest_guest_needs_post_build_scan(
    ctx: &BuildContext<'_>,
    exact_targets: &[BridgeSdkTarget],
    guest_consumer_component_names: &[ComponentName],
) -> bool {
    for (target_language, bridge_mode, sdk_targets) in
        ctx.application().bridge_sdks().for_all_used_modes()
    {
        if bridge_mode != BridgeMode::Guest {
            continue;
        }

        if !guest_consumer_component_names.is_empty()
            && exact_targets.iter().any(|target| {
                target.target_language == target_language && target.bridge_mode == bridge_mode
            })
            && guest_consumer_component_names.iter().any(|component_name| {
                let component = ctx.application().component(component_name);
                let wasm = component.agent_type_extraction_source_wasm();
                component.extracted_agent_types(&wasm).exists()
            })
        {
            return true;
        }

        let mut matchers = sdk_targets.agents.clone().into_set();
        if matchers.remove("*") {
            return true;
        }

        for component_name in ctx.application().component_names() {
            if !guest_consumer_component_names.contains(component_name) {
                matchers.remove(component_name.as_str());
            }
        }

        if matchers.iter().any(|matcher| {
            guest_consumer_component_names
                .iter()
                .any(|component_name| component_name.as_str() == matcher.as_str())
        }) {
            return true;
        }

        if guest_consumer_component_names.iter().any(|component_name| {
            ctx.application()
                .component(component_name)
                .agent_type_extraction_source_wasm()
                .exists()
        }) && matchers.iter().any(|matcher| {
            exact_targets.iter().any(|target| {
                target.target_language == target_language
                    && target.bridge_mode == bridge_mode
                    && target.agent_type.type_name.as_str() == matcher.as_str()
            })
        }) {
            return true;
        }

        for target in exact_targets {
            if target.target_language == target_language && target.bridge_mode == bridge_mode {
                matchers.remove(target.agent_type.type_name.as_str());
            }
        }

        if !matchers.is_empty() {
            return true;
        }
    }

    false
}

fn claim_matches_request(
    claim_request_id: BridgeRequestId,
    target_request_id: BridgeRequestId,
) -> bool {
    claim_request_id == target_request_id
        || (claim_request_id == BridgeRequestId::ManifestGuestPreBuild
            && target_request_id == BridgeRequestId::ManifestGuestPostBuild)
        || (claim_request_id == BridgeRequestId::ManifestGuestPostBuild
            && target_request_id == BridgeRequestId::ManifestGuestPreBuild)
}

fn bridge_requests(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> Vec<BridgeRequestId> {
    let mut requests = Vec::new();

    if ctx.custom_bridge_sdk_target().is_some() {
        requests.push(BridgeRequestId::Custom);
    } else {
        if ctx
            .application()
            .bridge_sdks()
            .for_all_used_modes()
            .into_iter()
            .any(|(_, mode, sdk_targets)| {
                mode == BridgeMode::Guest
                    && manifest_bridge_request_may_match_selected_components(
                        ctx,
                        sdk_targets.agents,
                        selected_component_names,
                    )
            })
        {
            requests.push(BridgeRequestId::ManifestGuestPreBuild);
            requests.push(BridgeRequestId::ManifestGuestPostBuild);
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

fn manifest_external_metadata_component_names(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> Vec<ComponentName> {
    let selected_component_names = selected_component_names.iter().collect::<BTreeSet<_>>();
    let all_component_names = ctx.application().component_names().collect::<BTreeSet<_>>();
    let mut metadata_component_names = BTreeSet::<ComponentName>::new();

    for (_, mode, sdk_targets) in ctx.application().bridge_sdks().for_all_used_modes() {
        if mode != BridgeMode::External {
            continue;
        }

        let mut has_concrete_agent_matcher = false;
        for matcher in sdk_targets.agents.clone().into_set() {
            if matcher == "*" {
                return selected_component_names.into_iter().cloned().collect();
            }

            if let Some(component_name) = all_component_names
                .iter()
                .find(|component_name| component_name.as_str() == matcher.as_str())
            {
                if selected_component_names.contains(component_name) {
                    metadata_component_names.insert((*component_name).clone());
                }
            } else {
                has_concrete_agent_matcher = true;
            }
        }

        if has_concrete_agent_matcher {
            metadata_component_names.extend(selected_component_names.iter().cloned().cloned());
        }
    }

    metadata_component_names.into_iter().collect()
}

fn should_run_bridge_scheduler(ctx: &BuildContext<'_>) -> bool {
    ctx.should_run_step(AppBuildStep::Build)
        && ctx.should_run_step(AppBuildStep::GenBridge)
        && ctx.custom_bridge_sdk_target().is_none()
        && ctx
            .application()
            .bridge_sdks()
            .for_all_used_modes()
            .into_iter()
            .any(|(_, mode, _)| mode == BridgeMode::Guest)
}

fn component_build_may_need_guest_bridge(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> bool {
    let component = ctx.application().component(component_name);
    if component.build_commands().is_empty() {
        return false;
    }

    if build_commands_reference_rust_guest_sdk(component.build_commands()) {
        return true;
    }

    let language = component
        .guess_language()
        .or_else(|| infer_component_language_from_dir(component.component_dir()));

    match language {
        Some(GuestLanguage::Rust) => rust_component_cargo_manifests_may_need_guest_bridge(
            component.component_dir(),
            component.build_commands(),
        ),
        Some(_) => false,
        None => false,
    }
}

fn rust_component_cargo_manifests_may_need_guest_bridge(
    component_dir: &Path,
    build_commands: &[crate::model::app_raw::BuildCommand],
) -> bool {
    rust_component_cargo_manifest_paths(component_dir, build_commands)
        .into_iter()
        .any(|cargo_toml_path| cargo_toml_may_need_guest_bridge(&cargo_toml_path))
}

fn cargo_toml_may_need_guest_bridge(cargo_toml_path: &Path) -> bool {
    let Ok(component_cargo_toml) = crate::fs::read_to_string(cargo_toml_path) else {
        return false;
    };

    if cargo_toml::collect_package_dependency_specs(&component_cargo_toml)
        .is_ok_and(|specs| specs.iter().any(dependency_spec_may_need_guest_bridge))
    {
        return true;
    }

    let Ok(workspace_dependency_refs) =
        cargo_toml::collect_workspace_dependency_refs(&component_cargo_toml)
    else {
        return false;
    };
    if workspace_dependency_refs.is_empty() {
        return false;
    }

    let mut current_dir = cargo_toml_path.parent();
    while let Some(dir) = current_dir {
        if let Ok(workspace_cargo_toml) = crate::fs::read_to_string(dir.join("Cargo.toml")) {
            if cargo_toml::collect_workspace_dependency_specs(&workspace_cargo_toml).is_ok_and(
                |specs| {
                    workspace_dependency_refs.iter().any(|name| {
                        specs
                            .get(name)
                            .is_some_and(dependency_spec_may_need_guest_bridge)
                    })
                },
            ) {
                return true;
            }
        }
        current_dir = dir.parent();
    }

    false
}

fn rust_component_cargo_manifest_paths(
    component_dir: &Path,
    build_commands: &[crate::model::app_raw::BuildCommand],
) -> BTreeSet<PathBuf> {
    let mut result = BTreeSet::from([component_dir.join("Cargo.toml")]);

    for build_command in build_commands {
        if let crate::model::app_raw::BuildCommand::External(command) = build_command {
            let command_dir = command
                .dir
                .as_ref()
                .map(|dir| component_dir.join(dir))
                .unwrap_or_else(|| component_dir.to_path_buf());
            let Some(selection) =
                cargo_manifest_selection_from_direct_cargo_command(&command.command)
            else {
                continue;
            };
            match selection {
                CargoManifestSelection::DefaultManifest => {
                    result.insert(command_dir.join("Cargo.toml"));
                }
                CargoManifestSelection::ExplicitManifests(paths) => {
                    result.extend(paths.into_iter().map(|path| {
                        if path.is_absolute() {
                            path
                        } else {
                            command_dir.join(path)
                        }
                    }));
                }
            }
        }
    }

    result
}

enum CargoManifestSelection {
    DefaultManifest,
    ExplicitManifests(Vec<PathBuf>),
}

fn cargo_manifest_selection_from_direct_cargo_command(
    command: &str,
) -> Option<CargoManifestSelection> {
    let Some(words) = shlex::split(command) else {
        return None;
    };
    let program = words.first()?;
    if !Path::new(program)
        .file_name()
        .is_some_and(|file_name| file_name == "cargo")
    {
        return None;
    }

    let mut result = Vec::new();
    let mut index = 1;
    while index < words.len() {
        let word = &words[index];
        if word == "--" {
            break;
        }
        if word == "--manifest-path" {
            if let Some(path) = words.get(index + 1) {
                result.push(PathBuf::from(path));
            }
            index += 2;
            continue;
        }
        if let Some(path) = word.strip_prefix("--manifest-path=") {
            result.push(PathBuf::from(path));
        }
        index += 1;
    }

    Some(if result.is_empty() {
        CargoManifestSelection::DefaultManifest
    } else {
        CargoManifestSelection::ExplicitManifests(result)
    })
}

fn explicit_cargo_manifest_paths_from_direct_cargo_command(command: &str) -> Vec<PathBuf> {
    match cargo_manifest_selection_from_direct_cargo_command(command) {
        Some(CargoManifestSelection::ExplicitManifests(paths)) => paths,
        Some(CargoManifestSelection::DefaultManifest) | None => Vec::new(),
    }
}

fn dependency_spec_may_need_guest_bridge(spec: &DependencySpec) -> bool {
    match spec {
        DependencySpec::Path { path, .. } => text_references_rust_guest_sdk(path),
        DependencySpec::Version { .. } | DependencySpec::Unsupported(_) => false,
    }
}

fn build_commands_reference_rust_guest_sdk(
    build_commands: &[crate::model::app_raw::BuildCommand],
) -> bool {
    build_commands
        .iter()
        .any(|build_command| match build_command {
            crate::model::app_raw::BuildCommand::External(command) => {
                text_references_rust_guest_sdk(&command.command)
                    || command
                        .env
                        .values()
                        .any(|value| text_references_rust_guest_sdk(value))
                    || command
                        .sources
                        .iter()
                        .any(|source| text_references_rust_guest_sdk(source))
            }
            _ => false,
        })
}

fn text_references_rust_guest_sdk(text: &str) -> bool {
    text.contains("-agent-guest-client")
}

fn infer_component_language_from_dir(component_dir: &Path) -> Option<GuestLanguage> {
    if component_dir.join("Cargo.toml").exists() {
        Some(GuestLanguage::Rust)
    } else if component_dir.join("package.json").exists() {
        Some(GuestLanguage::TypeScript)
    } else if component_dir.join("build.sbt").exists() {
        Some(GuestLanguage::Scala)
    } else if component_dir.join("moon.mod.json").exists()
        || component_dir.join("moon.mod").exists()
    {
        Some(GuestLanguage::MoonBit)
    } else {
        None
    }
}

fn describe_build_node(node: &BuildNode) -> String {
    match node {
        BuildNode::BuildComponent(component_name) => {
            format!("component {} build", component_name.as_str())
        }
        BuildNode::AddMetadata(component_name) => {
            format!("component {} metadata", component_name.as_str())
        }
        BuildNode::AgentMetadata(component_name) => {
            format!("component {} agent metadata", component_name.as_str())
        }
        BuildNode::ValidateBridgeRequest(request_id) => {
            format!("{} bridge validation", describe_bridge_request(*request_id))
        }
        BuildNode::GenerateBridgeRequest(request_id) => {
            format!("{} bridge generation", describe_bridge_request(*request_id))
        }
    }
}

fn describe_bridge_request(request_id: BridgeRequestId) -> &'static str {
    match request_id {
        BridgeRequestId::ManifestGuestPreBuild => "manifest guest pre-build",
        BridgeRequestId::ManifestGuestPostBuild => "manifest guest post-build",
        BridgeRequestId::ManifestExternal => "manifest external",
        BridgeRequestId::Custom => "custom",
        BridgeRequestId::Repl => "REPL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use indoc::indoc;
    use test_r::test;

    #[test]
    fn rust_component_workspace_root_cargo_toml_may_need_guest_bridge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            indoc! {r#"
                [package]
                name = "consumer"
                version = "0.1.0"
                edition = "2021"

                [workspace]
                members = ["."]

                [dependencies]
                bar = { workspace = true }

                [workspace.dependencies]
                bar = { path = "bridge/bar-agent-guest-client" }
            "#},
        )
        .unwrap();

        assert!(rust_component_cargo_manifests_may_need_guest_bridge(
            dir.path(),
            &[]
        ));
    }

    #[test]
    fn workspace_dependency_above_golem_yaml_may_need_guest_bridge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/consumer")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            indoc! {r#"
                [workspace]
                members = ["app/consumer"]

                [workspace.dependencies]
                bar = { path = "app/bridge/bar-agent-guest-client" }
            "#},
        )
        .unwrap();
        std::fs::write(dir.path().join("app/golem.yaml"), "app: test\n").unwrap();
        std::fs::write(
            dir.path().join("app/consumer/Cargo.toml"),
            indoc! {r#"
                [package]
                name = "consumer"
                version = "0.1.0"
                edition = "2021"

                [dependencies]
                bar = { workspace = true }
            "#},
        )
        .unwrap();

        assert!(rust_component_cargo_manifests_may_need_guest_bridge(
            &dir.path().join("app/consumer"),
            &[]
        ));
    }

    #[test]
    fn cargo_manifest_path_command_selects_additional_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("component")).unwrap();
        std::fs::write(
            dir.path().join("component/Cargo.toml"),
            indoc! {r#"
                [package]
                name = "component"
                version = "0.1.0"
                edition = "2021"
            "#},
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            indoc! {r#"
                [package]
                name = "workspace-root-package"
                version = "0.1.0"
                edition = "2021"

                [dependencies]
                bar = { path = "bridge/bar-agent-guest-client" }
            "#},
        )
        .unwrap();

        let build_commands = [external_build_command(
            "cargo metadata --manifest-path ../Cargo.toml --format-version=1",
            None,
        )];

        assert!(rust_component_cargo_manifests_may_need_guest_bridge(
            &dir.path().join("component"),
            &build_commands,
        ));
    }

    #[test]
    fn cargo_manifest_path_equals_form_is_supported() {
        assert_eq!(
            explicit_cargo_manifest_paths_from_direct_cargo_command(
                "cargo metadata --manifest-path=../Cargo.toml"
            ),
            vec![PathBuf::from("../Cargo.toml")]
        );
    }

    #[test]
    fn cargo_manifest_path_is_relative_to_external_command_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("component/scripts")).unwrap();
        std::fs::write(
            dir.path().join("component/Cargo.toml"),
            indoc! {r#"
                [package]
                name = "component"
                version = "0.1.0"
                edition = "2021"
            "#},
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            indoc! {r#"
                [package]
                name = "workspace-root-package"
                version = "0.1.0"
                edition = "2021"

                [dependencies]
                bar = { path = "bridge/bar-agent-guest-client" }
            "#},
        )
        .unwrap();

        let build_commands = [external_build_command(
            "cargo metadata --manifest-path=../../Cargo.toml --format-version=1",
            Some("scripts"),
        )];

        assert!(rust_component_cargo_manifests_may_need_guest_bridge(
            &dir.path().join("component"),
            &build_commands,
        ));
    }

    #[test]
    fn cargo_command_without_manifest_path_uses_external_command_dir_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("component/rust/src")).unwrap();
        std::fs::write(
            dir.path().join("component/rust/Cargo.toml"),
            indoc! {r#"
                [package]
                name = "consumer"
                version = "0.1.0"
                edition = "2021"

                [dependencies]
                bar-agent-guest-client = { path = "../../bridge/bar-agent-guest-client" }
            "#},
        )
        .unwrap();

        let build_commands = [external_build_command(
            "cargo metadata --format-version=1",
            Some("rust"),
        )];

        assert!(rust_component_cargo_manifests_may_need_guest_bridge(
            &dir.path().join("component"),
            &build_commands,
        ));
    }

    #[test]
    fn cargo_manifest_path_after_argument_separator_is_not_a_cargo_manifest_path() {
        assert!(
            explicit_cargo_manifest_paths_from_direct_cargo_command(
                "cargo run -- --manifest-path ../Cargo.toml"
            )
            .is_empty()
        );
        assert!(
            explicit_cargo_manifest_paths_from_direct_cargo_command(
                "cargo run -- --manifest-path=../Cargo.toml"
            )
            .is_empty()
        );
    }

    #[test]
    fn non_direct_cargo_commands_are_ignored() {
        assert!(
            explicit_cargo_manifest_paths_from_direct_cargo_command(
                "sh -c 'cargo metadata --manifest-path ../Cargo.toml'"
            )
            .is_empty()
        );
    }

    #[test]
    fn rust_guest_sdk_text_references_are_precise() {
        assert!(text_references_rust_guest_sdk(
            "../bridge/bar-agent-guest-client/Cargo.toml"
        ));
        assert!(!text_references_rust_guest_sdk("echo guest bridge"));
    }

    fn external_build_command(
        command: &str,
        dir: Option<&str>,
    ) -> crate::model::app_raw::BuildCommand {
        crate::model::app_raw::BuildCommand::External(crate::model::app_raw::ExternalCommand {
            command: command.to_string(),
            dir: dir.map(str::to_string),
            sources: Vec::new(),
            targets: Vec::new(),
            env: IndexMap::new(),
            rmdirs: Vec::new(),
            mkdirs: Vec::new(),
        })
    }
}
