use crate::app::build::extract_agent_type::extract_and_store_agent_types;
use crate::app::build::task_result_marker::GenerateBridgeSdkMarkerHash;
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::BuildContext;
use crate::bridge_gen::moonbit::MoonBitBridgeGenerator;
use crate::bridge_gen::rust::{RustBridgeGenerator, RustBridgeMode};
use crate::bridge_gen::scala::ScalaBridgeGenerator;
use crate::bridge_gen::typescript::TypeScriptBridgeGenerator;
use crate::bridge_gen::{BridgeGenerator, BridgeMode, bridge_client_directory_name};
use crate::command::GolemCliCommand;
use crate::error::NonSuccessfulExit;
use crate::fs;
use crate::log::log_error;
use crate::log::{LogColorize, LogIndent, log_action, log_skipping_up_to_date, logln};
use crate::model::GuestLanguage;
use crate::model::app::{BridgeSdkTarget, CustomBridgeSdkTarget};
use crate::model::repl::{ReplAgentMetadata, ReplMetadata};
use anyhow::bail;
use camino::Utf8PathBuf;
use golem_common::model::component::ComponentName;
use itertools::Itertools;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(crate) struct AgentMetadataCache {
    agent_types_by_component: BTreeMap<ComponentName, Vec<golem_common::schema::AgentTypeSchema>>,
}

impl AgentMetadataCache {
    pub(crate) async fn get(
        &mut self,
        ctx: &BuildContext<'_>,
        component_name: &ComponentName,
    ) -> anyhow::Result<Vec<golem_common::schema::AgentTypeSchema>> {
        if let Some(agent_types) = self.agent_types_by_component.get(component_name) {
            Ok(agent_types.clone())
        } else {
            let agent_types = extract_and_store_agent_types(ctx, component_name).await?;
            self.agent_types_by_component
                .insert(component_name.clone(), agent_types.clone());
            Ok(agent_types)
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct BridgeGenerationPlan {
    pub(crate) targets: Vec<BridgeSdkTarget>,
    pub(crate) repl_metadata_by_language: BTreeMap<GuestLanguage, ReplMetadata>,
}

pub async fn gen_bridge(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    gen_bridge_with_manifest_mode_filter(ctx, None).await
}

pub async fn gen_external_bridge(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    gen_external_bridge_with_additional_collision_targets(ctx, &[]).await
}

pub async fn gen_external_bridge_with_additional_collision_targets(
    ctx: &BuildContext<'_>,
    additional_collision_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    gen_bridge_with_manifest_mode_filter_and_additional_collision_targets(
        ctx,
        Some(BridgeMode::External),
        additional_collision_targets,
    )
    .await
}

async fn gen_bridge_with_manifest_mode_filter(
    ctx: &BuildContext<'_>,
    manifest_bridge_mode_filter: Option<BridgeMode>,
) -> anyhow::Result<()> {
    gen_bridge_with_manifest_mode_filter_and_additional_collision_targets(
        ctx,
        manifest_bridge_mode_filter,
        &[],
    )
    .await
}

async fn gen_bridge_with_manifest_mode_filter_and_additional_collision_targets(
    ctx: &BuildContext<'_>,
    manifest_bridge_mode_filter: Option<BridgeMode>,
    additional_collision_targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    let plan = plan_bridge_generation(ctx, manifest_bridge_mode_filter).await?;

    if plan.targets.is_empty() {
        if !additional_collision_targets.is_empty() {
            validate_no_output_dir_collisions(additional_collision_targets)?;
        }
        return Ok(());
    }

    let mut collision_targets = additional_collision_targets.to_vec();
    collision_targets.extend(plan.targets.iter().cloned());
    validate_supported_bridge_targets(&collision_targets)?;
    validate_no_output_dir_collisions(&collision_targets)?;

    write_repl_metadata(ctx, &plan).await?;

    log_action("Generating", "bridge SDKs");
    let _indent = LogIndent::new();

    gen_bridge_sdk_targets(ctx, plan.targets).await?;

    Ok(())
}

pub(crate) async fn plan_bridge_generation(
    ctx: &BuildContext<'_>,
    manifest_bridge_mode_filter: Option<BridgeMode>,
) -> anyhow::Result<BridgeGenerationPlan> {
    let mut agent_metadata_cache = AgentMetadataCache::default();
    plan_bridge_generation_with_metadata_cache(
        ctx,
        manifest_bridge_mode_filter,
        &mut agent_metadata_cache,
    )
    .await
}

pub(crate) async fn plan_bridge_generation_with_metadata_cache(
    ctx: &BuildContext<'_>,
    manifest_bridge_mode_filter: Option<BridgeMode>,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    let mut plan = BridgeGenerationPlan::default();

    plan.targets = match &ctx.custom_bridge_sdk_target() {
        Some(custom_target) => {
            collect_custom_targets(ctx, custom_target, agent_metadata_cache).await?
        }
        None => {
            collect_manifest_targets(ctx, manifest_bridge_mode_filter, agent_metadata_cache).await?
        }
    };

    if let Some(target) = ctx.repl_bridge_sdk_target() {
        let repl_targets = collect_custom_targets(ctx, target, agent_metadata_cache).await?;

        for target in &repl_targets {
            plan.repl_metadata_by_language
                .entry(target.target_language)
                .or_default()
                .agents
                .insert(
                    target.agent_type.type_name.clone(),
                    ReplAgentMetadata {
                        client_dir: target.output_dir.clone(),
                        mode: target.agent_type.mode,
                    },
                );
        }

        plan.targets.extend(repl_targets);
    }

    Ok(plan)
}

pub(crate) async fn write_repl_metadata(
    ctx: &BuildContext<'_>,
    plan: &BridgeGenerationPlan,
) -> anyhow::Result<()> {
    for (language, repl_meta) in &plan.repl_metadata_by_language {
        fs::write_str(
            ctx.application().repl_metadata_json(*language),
            &serde_json::to_string(repl_meta)?,
        )?;
        // TODO: from golden file, with "auto-exported static asset" support
        fs::write_str(
            ctx.application().repl_cli_commands_metadata_json(*language),
            &serde_json::to_string(&GolemCliCommand::collect_metadata_for_repl())?,
        )?;
    }

    Ok(())
}

pub(crate) async fn plan_manifest_guest_bridge_generation_for_components_lenient(
    ctx: &BuildContext<'_>,
    component_names: &[ComponentName],
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_manifest_targets_for_components_and_mode(
            ctx,
            component_names,
            Some(BridgeMode::Guest),
            true,
            false,
            agent_metadata_cache,
        )
        .await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_manifest_external_bridge_generation_for_components_lenient(
    ctx: &BuildContext<'_>,
    component_names: &[ComponentName],
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_manifest_external_bridge_targets_for_components_lenient(
            ctx,
            component_names,
            agent_metadata_cache,
        )
        .await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_custom_bridge_generation_lenient(
    ctx: &BuildContext<'_>,
    custom_target: &CustomBridgeSdkTarget,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_custom_targets_lenient(ctx, custom_target, agent_metadata_cache).await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_repl_bridge_generation_lenient(
    ctx: &BuildContext<'_>,
    repl_target: &CustomBridgeSdkTarget,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<BridgeGenerationPlan> {
    let targets = collect_custom_targets_lenient(ctx, repl_target, agent_metadata_cache).await?;
    let mut repl_metadata_by_language = BTreeMap::<GuestLanguage, ReplMetadata>::new();

    for target in &targets {
        repl_metadata_by_language
            .entry(target.target_language)
            .or_default()
            .agents
            .insert(
                target.agent_type.type_name.clone(),
                ReplAgentMetadata {
                    client_dir: target.output_dir.clone(),
                    mode: target.agent_type.mode,
                },
            );
    }

    Ok(BridgeGenerationPlan {
        targets,
        repl_metadata_by_language,
    })
}

pub(crate) async fn collect_manifest_external_bridge_targets_for_components_lenient(
    ctx: &BuildContext<'_>,
    component_names: &[ComponentName],
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    collect_manifest_targets_for_components_and_mode(
        ctx,
        component_names,
        Some(BridgeMode::External),
        true,
        false,
        agent_metadata_cache,
    )
    .await
}

pub(crate) async fn collect_custom_targets_lenient(
    ctx: &BuildContext<'_>,
    custom_target: &CustomBridgeSdkTarget,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    let mut targets = vec![];

    let should_filter_by_agent_type_name = !custom_target.agent_type_names.is_empty();
    let mut agent_type_names = custom_target.agent_type_names.clone();
    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        if !component.agent_type_extraction_source_wasm().exists() {
            continue;
        }

        let target_language = custom_target
            .target_language
            .or_else(|| component.guess_language())
            .unwrap_or(GuestLanguage::TypeScript);

        let mut agent_types = agent_metadata_cache.get(ctx, component_name).await?;
        if should_filter_by_agent_type_name {
            agent_types.retain(|agent_type| agent_type_names.remove(&agent_type.type_name));
        }

        for agent_type in agent_types {
            let output_dir = custom_target
                .output_dir
                .as_ref()
                .map(|output_dir| {
                    output_dir.join(bridge_client_directory_name(&agent_type.type_name))
                })
                .unwrap_or_else(|| {
                    ctx.application().bridge_sdk_dir(
                        &agent_type.type_name,
                        target_language,
                        BridgeMode::External,
                    )
                });

            targets.push(BridgeSdkTarget {
                component_name: component_name.clone(),
                agent_type,
                target_language,
                bridge_mode: BridgeMode::External,
                output_dir,
            });
        }
    }

    Ok(targets)
}

pub(crate) async fn gen_bridge_sdk_targets(
    ctx: &BuildContext<'_>,
    targets: Vec<BridgeSdkTarget>,
) -> anyhow::Result<()> {
    for target in targets {
        gen_bridge_sdk_target(ctx, target).await?;
    }

    Ok(())
}

async fn collect_manifest_targets(
    ctx: &BuildContext<'_>,
    bridge_mode_filter: Option<BridgeMode>,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    let component_names = ctx
        .application_context()
        .selected_component_names()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    collect_manifest_targets_for_components_and_mode(
        ctx,
        &component_names,
        bridge_mode_filter,
        false,
        false,
        agent_metadata_cache,
    )
    .await
}

async fn collect_manifest_targets_for_components_and_mode(
    ctx: &BuildContext<'_>,
    component_names: &[ComponentName],
    bridge_mode_filter: Option<BridgeMode>,
    ignore_unmatched_matchers: bool,
    skip_missing_sources: bool,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    let mut targets = vec![];
    let application_component_names = ctx
        .application()
        .component_names()
        .map(|component_name| component_name.as_str().to_string())
        .collect::<std::collections::BTreeSet<_>>();

    for (target_language, bridge_mode, sdk_targets) in
        ctx.application().bridge_sdks().for_all_used_modes()
    {
        if bridge_mode_filter.is_some_and(|bridge_mode_filter| bridge_mode_filter != bridge_mode) {
            continue;
        }

        let mut matchers = sdk_targets.agents.clone().into_set();

        if matchers.is_empty() {
            continue;
        }

        let is_matching_all = matchers.remove("*");

        for component_name in component_names {
            if skip_missing_sources
                && !ctx
                    .application()
                    .component(component_name)
                    .agent_type_extraction_source_wasm()
                    .exists()
            {
                continue;
            }

            let is_matching_component = matchers.remove(component_name.as_str());

            if !is_matching_all
                && !is_matching_component
                && matchers
                    .iter()
                    .all(|matcher| application_component_names.contains(matcher.as_str()))
            {
                continue;
            }

            let mut agent_types = agent_metadata_cache.get(ctx, component_name).await?;

            if !is_matching_all && !is_matching_component {
                agent_types.retain(|agent_type| matchers.contains(agent_type.type_name.as_str()));
            }

            for agent_type in agent_types {
                matchers.remove(agent_type.type_name.as_str());

                let output_dir = ctx.application().bridge_sdk_dir(
                    &agent_type.type_name,
                    target_language,
                    bridge_mode,
                );
                targets.push(BridgeSdkTarget {
                    component_name: component_name.clone(),
                    agent_type,
                    target_language,
                    bridge_mode,
                    output_dir,
                });
            }
        }

        if !ignore_unmatched_matchers && !matchers.is_empty() {
            // Remove "non-selected" components
            for component_name in ctx.application().component_names() {
                if !ctx
                    .application_context()
                    .selected_component_names()
                    .contains(component_name)
                {
                    matchers.remove(component_name.as_str());
                }
            }
        }

        if !ignore_unmatched_matchers && !matchers.is_empty() {
            logln("");
            log_error(format!(
                "The following agent matchers were not found during {} bridge SDK generation: {}",
                bridge_sdk_target_name(target_language, bridge_mode).log_color_highlight(),
                matchers
                    .iter()
                    .map(|at| at.as_str().log_color_highlight().to_string())
                    .join(", ")
            ));
            bail!(NonSuccessfulExit)
        }
    }

    Ok(targets)
}

async fn collect_custom_targets(
    ctx: &BuildContext<'_>,
    custom_target: &CustomBridgeSdkTarget,
    agent_metadata_cache: &mut AgentMetadataCache,
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    let mut targets = vec![];

    let should_filter_by_agent_type_name = !custom_target.agent_type_names.is_empty();
    let mut agent_type_names = custom_target.agent_type_names.clone();
    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        let target_language = custom_target
            .target_language
            .or_else(|| component.guess_language())
            .unwrap_or(GuestLanguage::TypeScript);

        let agent_types = {
            let mut agent_types = agent_metadata_cache.get(ctx, component_name).await?;

            if should_filter_by_agent_type_name {
                agent_types.retain(|agent_type| agent_type_names.remove(&agent_type.type_name));
            }

            agent_types
        };

        for agent_type in agent_types {
            let output_dir = custom_target
                .output_dir
                .as_ref()
                .map(|output_dir| {
                    output_dir.join(bridge_client_directory_name(&agent_type.type_name))
                })
                .unwrap_or_else(|| {
                    ctx.application().bridge_sdk_dir(
                        &agent_type.type_name,
                        target_language,
                        BridgeMode::External,
                    )
                });

            targets.push(BridgeSdkTarget {
                component_name: component_name.clone(),
                agent_type,
                target_language,
                bridge_mode: BridgeMode::External,
                output_dir,
            });
        }
    }

    if !agent_type_names.is_empty() {
        logln("");
        log_error(format!(
            "The following agent type names were not found: {}",
            agent_type_names
                .iter()
                .map(|at| at.as_str().log_color_highlight().to_string())
                .join(", ")
        ));
        bail!(NonSuccessfulExit)
    }

    Ok(targets)
}

async fn gen_bridge_sdk_target(
    ctx: &BuildContext<'_>,
    target: BridgeSdkTarget,
) -> anyhow::Result<()> {
    let component = ctx.application().component(&target.component_name);
    let final_wasm = component.final_wasm();
    let agent_type_name = target.agent_type.type_name.clone();
    let output_dir = Utf8PathBuf::try_from(target.output_dir)?;

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(GenerateBridgeSdkMarkerHash {
            component_name: &target.component_name,
            agent_type_name: &target.agent_type.type_name,
            language: &target.target_language,
            bridge_mode: target.bridge_mode,
        })?
        .with_sources(|| vec![&final_wasm])
        .with_targets(|| vec![&output_dir])
        .run_async_or_skip(
            || async {
                log_action(
                    "Generating",
                    format!(
                        "{} bridge SDK for {} to {}",
                        bridge_sdk_target_name(target.target_language, target.bridge_mode)
                            .log_color_highlight(),
                        agent_type_name.as_str().log_color_highlight(),
                        output_dir.log_color_highlight(),
                    ),
                );
                let _indent = LogIndent::new();

                let mut generator: Box<dyn BridgeGenerator> =
                    match (target.target_language, target.bridge_mode) {
                        (GuestLanguage::Rust, BridgeMode::External) => {
                            Box::new(RustBridgeGenerator::new_with_mode(
                                target.agent_type,
                                &output_dir,
                                false,
                                RustBridgeMode::ExternalRest,
                            )?)
                        }
                        (GuestLanguage::Rust, BridgeMode::Guest) => {
                            Box::new(RustBridgeGenerator::new_with_mode(
                                target.agent_type,
                                &output_dir,
                                false,
                                RustBridgeMode::GuestWasmRpc,
                            )?)
                        }
                        (GuestLanguage::TypeScript, BridgeMode::External) => Box::new(
                            TypeScriptBridgeGenerator::new(target.agent_type, &output_dir, false)?,
                        ),
                        (GuestLanguage::Scala, BridgeMode::External) => Box::new(
                            ScalaBridgeGenerator::new(target.agent_type, &output_dir, false)?,
                        ),
                        (GuestLanguage::MoonBit, BridgeMode::External) => Box::new(
                            MoonBitBridgeGenerator::new(target.agent_type, &output_dir, false)?,
                        ),
                        (language, BridgeMode::Guest) => bail!(
                            "guest bridge mode is not supported for {} yet",
                            language.to_string().log_color_highlight()
                        ),
                    };

                fs::remove(&output_dir)?;
                generator.generate()
            },
            || {
                log_skipping_up_to_date(format!(
                    "generating {} bridge SDK for {} to {}",
                    bridge_sdk_target_name(target.target_language, target.bridge_mode)
                        .log_color_highlight(),
                    agent_type_name.as_str().log_color_highlight(),
                    output_dir.log_color_highlight()
                ));
            },
        )
        .await
}

fn bridge_sdk_target_name(language: GuestLanguage, bridge_mode: BridgeMode) -> String {
    match bridge_mode {
        BridgeMode::External => language.to_string(),
        BridgeMode::Guest => format!("{} guest", language),
    }
}

pub(crate) fn validate_no_output_dir_collisions(targets: &[BridgeSdkTarget]) -> anyhow::Result<()> {
    let mut resolved_targets = Vec::new();

    for target in targets {
        let output_dir = fs::absolute_lexical_path(&target.output_dir)?;
        resolved_targets.push((output_dir, target));
    }

    let mut collisions = Vec::new();
    for (index, (left_output_dir, left_target)) in resolved_targets.iter().enumerate() {
        for (right_output_dir, right_target) in resolved_targets.iter().skip(index + 1) {
            if left_output_dir == right_output_dir
                || left_output_dir.starts_with(right_output_dir)
                || right_output_dir.starts_with(left_output_dir)
            {
                collisions.push((left_output_dir, left_target, right_output_dir, right_target));
            }
        }
    }

    if !collisions.is_empty() {
        for (left_output_dir, left_target, right_output_dir, right_target) in collisions {
            logln("");
            log_error(format!(
                "Bridge SDK target output directories overlap: {} for {} resolves to {}, {} for {} resolves to {}",
                bridge_sdk_target_name(left_target.target_language, left_target.bridge_mode),
                left_target.agent_type.type_name.as_str(),
                left_output_dir.log_color_highlight(),
                bridge_sdk_target_name(right_target.target_language, right_target.bridge_mode),
                right_target.agent_type.type_name.as_str(),
                right_output_dir.log_color_highlight(),
            ));
        }
        bail!(NonSuccessfulExit)
    }

    Ok(())
}

pub(crate) fn validate_supported_bridge_targets(targets: &[BridgeSdkTarget]) -> anyhow::Result<()> {
    for target in targets {
        if target.bridge_mode == BridgeMode::Guest && target.target_language != GuestLanguage::Rust
        {
            bail!(
                "guest bridge mode is not supported for {} yet",
                target.target_language.to_string().log_color_highlight()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::{AgentMode, AgentTypeName, Snapshotting};
    use golem_common::model::component::ComponentName;
    use golem_common::schema::{AgentConstructorSchema, AgentTypeSchema, InputSchema, SchemaGraph};
    use tempfile::tempdir;
    use test_r::test;

    #[test]
    fn validate_no_output_dir_collisions_rejects_nested_output_dirs() {
        let temp_dir = tempdir().unwrap();
        let parent_output_dir = temp_dir.path().join("bridge/alpha-client");
        let nested_output_dir = parent_output_dir.join("beta-client");

        let targets = vec![
            bridge_sdk_target("AlphaAgent", GuestLanguage::Rust, parent_output_dir),
            bridge_sdk_target("BetaAgent", GuestLanguage::TypeScript, nested_output_dir),
        ];

        assert!(validate_no_output_dir_collisions(&targets).is_err());
    }

    #[test]
    fn validate_no_output_dir_collisions_rejects_duplicate_output_dirs() {
        let temp_dir = tempdir().unwrap();
        let output_dir = temp_dir.path().join("bridge/alpha-client");

        let targets = vec![
            bridge_sdk_target_with_mode(
                "AlphaAgent",
                GuestLanguage::Rust,
                BridgeMode::External,
                output_dir.clone(),
            ),
            bridge_sdk_target_with_mode(
                "AlphaAgent",
                GuestLanguage::Rust,
                BridgeMode::Guest,
                output_dir,
            ),
        ];

        assert!(validate_no_output_dir_collisions(&targets).is_err());
    }

    fn bridge_sdk_target(
        agent_type_name: &str,
        target_language: GuestLanguage,
        output_dir: impl Into<std::path::PathBuf>,
    ) -> BridgeSdkTarget {
        bridge_sdk_target_with_mode(
            agent_type_name,
            target_language,
            BridgeMode::External,
            output_dir,
        )
    }

    fn bridge_sdk_target_with_mode(
        agent_type_name: &str,
        target_language: GuestLanguage,
        bridge_mode: BridgeMode,
        output_dir: impl Into<std::path::PathBuf>,
    ) -> BridgeSdkTarget {
        BridgeSdkTarget {
            component_name: ComponentName("component".to_string()),
            agent_type: agent_type(agent_type_name),
            target_language,
            bridge_mode,
            output_dir: output_dir.into(),
        }
    }

    fn agent_type(type_name: &str) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName(type_name.to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![]),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Ephemeral,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }
}
