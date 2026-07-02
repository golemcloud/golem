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
use crate::bridge_gen::BridgeMode;
use crate::error::NonSuccessfulExit;
use crate::log::{LogColorize, log_error, logln};
use crate::model::GuestLanguage;
use crate::model::app::{AppBuildStep, BridgeSdkTarget, CustomBridgeSdkTarget};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentName;
use golem_common::schema::AgentTypeSchema;
use itertools::Itertools;
use std::collections::BTreeSet;
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
    let effective_component_names = selected_component_names_with_dependencies(ctx);
    let guest_source_component_names = effective_component_names
        .iter()
        .filter(|component_name| {
            component_guest_bridge_requirements(ctx, component_name).is_empty()
        })
        .cloned()
        .collect::<Vec<_>>();
    let requests = if ctx.should_run_step(AppBuildStep::GenBridge) {
        bridge_requests(ctx, &effective_component_names)
    } else {
        Vec::new()
    };
    let mut agent_metadata_cache = AgentMetadataCache::default();
    let mut validated_targets = Vec::<(BridgeRequestId, BridgeSdkTarget)>::new();
    let claims = bridge_output_dir_claims(ctx, &effective_component_names);
    let mut pending_components = effective_component_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut built_components = BTreeSet::<ComponentName>::new();
    let mut available_guest_bridge_components = BTreeSet::<ComponentName>::new();
    let mut generated_guest_target_keys = BTreeSet::<BridgeSdkTargetKey>::new();

    while !pending_components.is_empty() {
        let buildable_components = pending_components
            .iter()
            .filter(|component_name| {
                component_guest_bridge_requirements(ctx, component_name)
                    .is_subset(&available_guest_bridge_components)
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
            if !requests.contains(&BridgeRequestId::ManifestGuest) {
                available_guest_bridge_components.insert(component_name);
            }
            made_progress = true;
        }

        if requests.contains(&BridgeRequestId::ManifestGuest) {
            let has_explicit_manifest_guest_request =
                has_explicit_manifest_guest_bridge_request(ctx, &effective_component_names);
            let dependency_source_components =
                selected_guest_bridge_dependency_sources(ctx, &effective_component_names);
            let built_component_names = built_components
                .iter()
                .filter(|component_name| {
                    has_explicit_manifest_guest_request
                        || dependency_source_components.contains(*component_name)
                })
                .cloned()
                .collect::<Vec<_>>();
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
            let plan = plan_manifest_guest_bridge_generation_for_components_lenient(
                ctx,
                &source_component_names,
                &effective_component_names,
                &mut agent_metadata_cache,
            )
            .await?;

            let new_targets = validate_and_filter_new_bridge_targets(
                ctx,
                BridgeRequestId::ManifestGuest,
                plan.targets,
                &mut validated_targets,
                &claims,
                &mut generated_guest_target_keys,
            )?;

            if !new_targets.is_empty() {
                gen_bridge_sdk_targets(ctx, new_targets.clone()).await?;
            }

            let available_before = available_guest_bridge_components.len();
            available_guest_bridge_components.extend(built_component_names);
            if available_guest_bridge_components.len() != available_before {
                made_progress = true;
            }
        }

        if !made_progress {
            report_guest_bridge_build_cycle(
                ctx,
                &pending_components,
                &available_guest_bridge_components,
            )?;
        }
    }

    if requests.contains(&BridgeRequestId::ManifestGuest) {
        let exact_targets = validated_targets
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>();
        validate_manifest_guest_matchers_resolved(ctx, &exact_targets)?;
    }

    for request_id in [BridgeRequestId::ManifestExternal, BridgeRequestId::Repl] {
        if !requests.contains(&request_id) {
            continue;
        }

        let plan = plan_bridge_request(
            ctx,
            request_id,
            &effective_component_names,
            &guest_source_component_names,
            &mut agent_metadata_cache,
        )
        .await?;

        let targets = validate_and_filter_new_bridge_targets(
            ctx,
            request_id,
            plan.targets.clone(),
            &mut validated_targets,
            &claims,
            &mut BTreeSet::new(),
        )?;

        let exact_targets = validated_targets
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>();
        match request_id {
            BridgeRequestId::ManifestExternal => {
                validate_manifest_external_matchers_resolved(ctx, &exact_targets)?;
            }
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

fn report_guest_bridge_build_cycle(
    ctx: &BuildContext<'_>,
    pending_components: &BTreeSet<ComponentName>,
    available_guest_bridge_components: &BTreeSet<ComponentName>,
) -> anyhow::Result<()> {
    logln("");
    log_error("Build graph has a cycle involving guest bridge generation:".to_string());
    for component_name in pending_components {
        let missing = component_guest_bridge_requirements(ctx, component_name)
            .difference(available_guest_bridge_components)
            .map(|component| component.as_str().to_string())
            .collect::<Vec<_>>();
        log_error(format!(
            "  component {} waits for guest bridge generation from components: {}",
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
    agent_type_name: AgentTypeName,
    target_language: GuestLanguage,
    bridge_mode: BridgeMode,
    output_dir: PathBuf,
}

impl BridgeSdkTargetKey {
    fn from_bridge_target(ctx: &BuildContext<'_>, target: &BridgeSdkTarget) -> Self {
        Self {
            component_name: target.component_name.clone(),
            agent_type_name: target.agent_type.type_name.clone(),
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

fn component_guest_bridge_requirements(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> BTreeSet<ComponentName> {
    ctx.application()
        .component(component_name)
        .properties()
        .dependencies
        .iter()
        .cloned()
        .collect()
}

fn selected_guest_bridge_dependency_sources(
    ctx: &BuildContext<'_>,
    selected_component_names: &[ComponentName],
) -> BTreeSet<ComponentName> {
    selected_component_names
        .iter()
        .flat_map(|component_name| component_guest_bridge_requirements(ctx, component_name))
        .collect()
}

fn selected_component_names_with_dependencies(ctx: &BuildContext<'_>) -> Vec<ComponentName> {
    let mut selected = BTreeSet::<ComponentName>::new();
    let mut pending = ctx
        .application_context()
        .selected_component_names()
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    while let Some(component_name) = pending.pop() {
        if !selected.insert(component_name.clone()) {
            continue;
        }

        pending.extend(component_guest_bridge_requirements(ctx, &component_name));
    }

    selected.into_iter().collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum BridgeRequestId {
    ManifestGuest,
    ManifestExternal,
    Custom,
    Repl,
}

async fn plan_bridge_request(
    ctx: &BuildContext<'_>,
    request_id: BridgeRequestId,
    selected_component_names: &[ComponentName],
    guest_source_component_names: &[ComponentName],
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    match request_id {
        BridgeRequestId::ManifestGuest => {
            plan_manifest_guest_bridge_generation_for_components_lenient(
                ctx,
                guest_source_component_names,
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
            if claim.request_id == BridgeRequestId::ManifestGuest
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
        if has_explicit_manifest_guest_bridge_request(ctx, selected_component_names)
            || selected_component_names.iter().any(|component_name| {
                !component_guest_bridge_requirements(ctx, component_name).is_empty()
            })
        {
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
                && manifest_bridge_request_may_match_selected_components(
                    ctx,
                    sdk_targets.agents,
                    selected_component_names,
                )
        })
}

fn should_run_bridge_scheduler(ctx: &BuildContext<'_>) -> bool {
    if !ctx.should_run_step(AppBuildStep::Build) || ctx.custom_bridge_sdk_target().is_some() {
        return false;
    }

    let effective_component_names = selected_component_names_with_dependencies(ctx);

    if effective_component_names
        .iter()
        .any(|component_name| !component_guest_bridge_requirements(ctx, component_name).is_empty())
    {
        return true;
    }

    if !ctx.should_run_step(AppBuildStep::GenBridge) {
        return false;
    }

    bridge_requests(ctx, &effective_component_names).contains(&BridgeRequestId::ManifestGuest)
}
