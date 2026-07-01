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
    plan_repl_bridge_generation_lenient, should_auto_enable_guest_bridge,
    validate_no_output_dir_collisions, validate_supported_bridge_targets, write_repl_metadata,
};
use crate::app::context::BuildContext;
use crate::app::edit::cargo_toml::{self, DependencySpec};
use crate::bridge_gen::{BridgeMode, bridge_client_directory_name_for_mode};
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
    let requests = bridge_requests(ctx, &selected_component_names);
    let mut agent_metadata_cache = AgentMetadataCache::default();
    let mut validated_targets = Vec::<(BridgeRequestId, BridgeSdkTarget)>::new();
    let claims = bridge_output_dir_claims(ctx, &selected_component_names);
    let mut pending_components = selected_component_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut built_components = BTreeSet::<ComponentName>::new();
    let mut available_guest_clients = BTreeSet::<GuestBridgeClientId>::new();
    let mut generated_guest_target_keys = BTreeSet::<BridgeSdkTargetKey>::new();

    while !pending_components.is_empty() {
        let buildable_components = pending_components
            .iter()
            .filter(|component_name| {
                component_guest_bridge_requirements(ctx, component_name)
                    .is_subset(&available_guest_clients)
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
            built_components.insert(component_name);
            made_progress = true;
        }

        if requests.contains(&BridgeRequestId::ManifestGuest) {
            let built_component_names = built_components
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
                &built_component_names,
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
                available_guest_clients.extend(
                    new_targets
                        .iter()
                        .map(GuestBridgeClientId::from_bridge_target),
                );
                made_progress = true;
            }
        }

        if !made_progress {
            report_guest_bridge_build_cycle(ctx, &pending_components, &available_guest_clients)?;
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
            &selected_component_names,
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
    available_guest_clients: &BTreeSet<GuestBridgeClientId>,
) -> anyhow::Result<()> {
    logln("");
    log_error("Build graph has a cycle involving guest bridge generation:".to_string());
    for component_name in pending_components {
        let missing = component_guest_bridge_requirements(ctx, component_name)
            .difference(available_guest_clients)
            .map(|client| client.to_string())
            .collect::<Vec<_>>();
        log_error(format!(
            "  component {} build needs guest bridge clients: {}",
            component_name.as_str().log_color_highlight(),
            missing
                .iter()
                .map(|client| client.log_color_highlight())
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

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct GuestBridgeClientId {
    target_language: GuestLanguage,
    client_name: String,
}

impl GuestBridgeClientId {
    fn from_bridge_target(target: &BridgeSdkTarget) -> Self {
        Self {
            target_language: target.target_language,
            client_name: bridge_client_directory_name_for_mode(
                &target.agent_type.type_name,
                target.bridge_mode,
            ),
        }
    }
}

impl std::fmt::Display for GuestBridgeClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.target_language.id(), self.client_name)
    }
}

fn component_guest_bridge_requirements(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> BTreeSet<GuestBridgeClientId> {
    let component = ctx.application().component(component_name);
    component_referenced_guest_clients(
        ctx,
        component.component_dir(),
        component.properties().build.as_slice(),
    )
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
            || should_auto_enable_guest_bridge(ctx, selected_component_names)
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

fn should_run_bridge_scheduler(ctx: &BuildContext<'_>) -> bool {
    if !ctx.should_run_step(AppBuildStep::Build)
        || !ctx.should_run_step(AppBuildStep::GenBridge)
        || ctx.custom_bridge_sdk_target().is_some()
    {
        return false;
    }

    let selected_component_names = ctx
        .application_context()
        .selected_component_names()
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    bridge_requests(ctx, &selected_component_names).contains(&BridgeRequestId::ManifestGuest)
}

fn component_build_may_need_guest_bridge(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> bool {
    let component = ctx.application().component(component_name);
    if component.build_commands().is_empty() {
        return false;
    }

    !component_guest_bridge_requirements(ctx, component_name).is_empty()
}

fn component_referenced_guest_clients(
    ctx: &BuildContext<'_>,
    component_dir: &Path,
    build_commands: &[crate::model::app_raw::BuildCommand],
) -> BTreeSet<GuestBridgeClientId> {
    let output_bases = guest_bridge_output_bases(ctx);
    let mut result = component_cargo_manifest_paths(component_dir, build_commands)
        .into_iter()
        .flat_map(|cargo_toml_path| {
            cargo_toml_referenced_guest_clients(&cargo_toml_path, &output_bases)
        })
        .collect::<BTreeSet<_>>();

    result.extend(build_commands.iter().flat_map(|build_command| {
        build_command_referenced_guest_clients(component_dir, build_command, &output_bases)
    }));

    result
}

fn cargo_toml_referenced_guest_clients(
    cargo_toml_path: &Path,
    output_bases: &[(GuestLanguage, PathBuf)],
) -> BTreeSet<GuestBridgeClientId> {
    let Ok(component_cargo_toml) = crate::fs::read_to_string(cargo_toml_path) else {
        return BTreeSet::new();
    };

    let mut result = BTreeSet::new();
    let source_dir = cargo_toml_path.parent().unwrap_or_else(|| Path::new(""));

    if let Ok(specs) = cargo_toml::collect_package_dependency_specs(&component_cargo_toml) {
        result.extend(
            specs
                .iter()
                .filter_map(|spec| dependency_spec_guest_client(source_dir, spec, output_bases)),
        );
    }

    let Ok(workspace_dependency_refs) =
        cargo_toml::collect_workspace_dependency_refs(&component_cargo_toml)
    else {
        return result;
    };
    if workspace_dependency_refs.is_empty() {
        return result;
    }

    let mut current_dir = cargo_toml_path.parent();
    while let Some(dir) = current_dir {
        if let Ok(workspace_cargo_toml) = crate::fs::read_to_string(dir.join("Cargo.toml"))
            && let Ok(specs) = cargo_toml::collect_workspace_dependency_specs(&workspace_cargo_toml)
        {
            result.extend(workspace_dependency_refs.iter().filter_map(|name| {
                specs
                    .get(name)
                    .and_then(|spec| dependency_spec_guest_client(dir, spec, output_bases))
            }));
        }
        current_dir = dir.parent();
    }

    result
}

fn guest_bridge_output_bases(ctx: &BuildContext<'_>) -> Vec<(GuestLanguage, PathBuf)> {
    let mut result = vec![(
        GuestLanguage::Rust,
        ctx.application()
            .temp_dir()
            .join("bridge-sdk")
            .join(GuestLanguage::Rust.id())
            .join(BridgeMode::Guest.id()),
    )];

    result.extend(
        ctx.application()
            .bridge_sdks()
            .for_all_used_modes()
            .into_iter()
            .filter_map(|(language, mode, sdk_targets)| {
                if mode == BridgeMode::Guest {
                    sdk_targets.output_dir.as_ref().map(|output_dir| {
                        (
                            language,
                            ctx.application().bridge_sdks_source().join(output_dir),
                        )
                    })
                } else {
                    None
                }
            }),
    );

    result
}

#[cfg(test)]
fn component_cargo_manifests_may_need_guest_bridge(
    component_dir: &Path,
    build_commands: &[crate::model::app_raw::BuildCommand],
) -> bool {
    component_cargo_manifest_paths(component_dir, build_commands)
        .into_iter()
        .any(|cargo_toml_path| cargo_toml_may_need_guest_bridge(&cargo_toml_path))
}

#[cfg(test)]
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

fn component_cargo_manifest_paths(
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

#[cfg(test)]
fn explicit_cargo_manifest_paths_from_direct_cargo_command(command: &str) -> Vec<PathBuf> {
    match cargo_manifest_selection_from_direct_cargo_command(command) {
        Some(CargoManifestSelection::ExplicitManifests(paths)) => paths,
        Some(CargoManifestSelection::DefaultManifest) | None => Vec::new(),
    }
}

#[cfg(test)]
fn dependency_spec_may_need_guest_bridge(spec: &DependencySpec) -> bool {
    dependency_spec_guest_client_name(spec).is_some()
}

fn dependency_spec_guest_client(
    source_dir: &Path,
    spec: &DependencySpec,
    output_bases: &[(GuestLanguage, PathBuf)],
) -> Option<GuestBridgeClientId> {
    let DependencySpec::Path { path, .. } = spec else {
        return None;
    };

    generated_guest_client_from_path(source_dir, path, output_bases)
}

#[cfg(test)]
fn dependency_spec_guest_client_name(spec: &DependencySpec) -> Option<String> {
    let DependencySpec::Path { path, .. } = spec else {
        return None;
    };

    guest_client_name_from_path(path)
}

#[cfg(test)]
fn guest_client_name_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .find(|component| text_references_generated_guest_bridge_client(component))
        .map(str::to_string)
}

fn generated_guest_client_from_path(
    source_dir: &Path,
    path: &str,
    output_bases: &[(GuestLanguage, PathBuf)],
) -> Option<GuestBridgeClientId> {
    let path = Path::new(path);
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_dir.join(path)
    };
    let absolute_path = crate::fs::absolute_lexical_path(&absolute_path).ok()?;

    for (target_language, base_dir) in output_bases {
        let base_dir = crate::fs::absolute_lexical_path(base_dir).ok()?;
        let Ok(relative_path) = absolute_path.strip_prefix(&base_dir) else {
            continue;
        };
        if let Some(client_name) = relative_path
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .filter(|component| text_references_generated_guest_bridge_client(component))
        {
            return Some(GuestBridgeClientId {
                target_language: *target_language,
                client_name: client_name.to_string(),
            });
        }
    }

    None
}

fn build_command_referenced_guest_clients(
    component_dir: &Path,
    build_command: &crate::model::app_raw::BuildCommand,
    output_bases: &[(GuestLanguage, PathBuf)],
) -> BTreeSet<GuestBridgeClientId> {
    let crate::model::app_raw::BuildCommand::External(command) = build_command else {
        return BTreeSet::new();
    };
    let command_dir = command
        .dir
        .as_ref()
        .map(|dir| component_dir.join(dir))
        .unwrap_or_else(|| component_dir.to_path_buf());

    shlex::split(&command.command)
        .unwrap_or_else(|| {
            command
                .command
                .split_whitespace()
                .map(str::to_string)
                .collect()
        })
        .into_iter()
        .flat_map(|text| {
            std::iter::once(text.clone())
                .chain(text.split_whitespace().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .chain(command.env.values().cloned())
        .chain(command.sources.iter().cloned())
        .filter_map(|text| generated_guest_client_from_path(&command_dir, &text, output_bases))
        .collect()
}

fn text_references_generated_guest_bridge_client(text: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use indoc::indoc;
    use test_r::test;

    #[test]
    fn component_workspace_root_cargo_toml_may_need_guest_bridge() {
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

        assert!(component_cargo_manifests_may_need_guest_bridge(
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

        assert!(component_cargo_manifests_may_need_guest_bridge(
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

        assert!(component_cargo_manifests_may_need_guest_bridge(
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

        assert!(component_cargo_manifests_may_need_guest_bridge(
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

        assert!(component_cargo_manifests_may_need_guest_bridge(
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
    fn guest_bridge_client_text_references_are_precise() {
        assert!(text_references_generated_guest_bridge_client(
            "../bridge/bar-agent-guest-client/Cargo.toml"
        ));
        assert!(!text_references_generated_guest_bridge_client(
            "echo guest bridge"
        ));
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
