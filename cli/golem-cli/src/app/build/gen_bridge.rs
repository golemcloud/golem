use crate::app::build::extract_component_metadata::extract_and_store_component_metadata;
use crate::app::build::task_result_marker::GenerateBridgeSdkMarkerHash;
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::BuildContext;
use crate::bridge_gen::moonbit::tool::MoonBitToolBridgeGenerator;
use crate::bridge_gen::moonbit::{MoonBitBridgeGenerator, MoonBitBridgeMode};
use crate::bridge_gen::rust::tool::RustToolBridgeGenerator;
use crate::bridge_gen::rust::{RustBridgeGenerator, RustBridgeMode};
use crate::bridge_gen::scala::tool::ScalaToolBridgeGenerator;
use crate::bridge_gen::scala::{ScalaBridgeGenerator, ScalaBridgeMode};
use crate::bridge_gen::typescript::tool::TypeScriptToolBridgeGenerator;
use crate::bridge_gen::typescript::{TypeScriptBridgeGenerator, TypeScriptBridgeMode};
use crate::bridge_gen::{
    BridgeGenerator, BridgeMode, bridge_client_directory_name,
    validate_host_managed_agent_bridge_policy,
};
use crate::command::GolemCliCommand;
use crate::error::NonSuccessfulExit;
use crate::fs;
use crate::log::log_error;
use crate::log::{LogColorize, LogIndent, log_action, log_skipping_up_to_date, logln};
use crate::model::app::{
    BridgeSdkTarget, BridgeSdkTargetKind, BridgeSdkTargetSource, BridgeSdkTargetSubject,
    ComponentDependency, CustomBridgeSdkTarget,
};
use crate::model::cli_output::StructuredOutput;
use crate::model::language::GuestLanguage;
use crate::model::repl::{ReplAgentMetadata, ReplMetadata};
use crate::model::text_format::{NoTextOutput, TextOutput};
use crate::model::tool_deployment::{
    ToolEntityPath, ToolValidationCode, ToolValidationIssue, ToolValidationPhase,
};
use anyhow::bail;
use camino::Utf8PathBuf;
use golem_common::model::component::ComponentName;
use golem_common::model::tool::ToolName;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

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

    let mut collision_targets = additional_collision_targets.to_vec();
    collision_targets.extend(plan.targets.iter().cloned());
    validate_supported_bridge_targets(&collision_targets)?;
    validate_host_managed_bridge_targets(&collision_targets)?;

    if plan.targets.is_empty() {
        if !additional_collision_targets.is_empty() {
            validate_no_output_dir_collisions(additional_collision_targets)?;
        }
        return Ok(());
    }

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
    let mut plan = BridgeGenerationPlan {
        targets: match &ctx.custom_bridge_sdk_target() {
            Some(custom_target) => collect_custom_targets(ctx, custom_target).await?,
            None => collect_manifest_targets(ctx, manifest_bridge_mode_filter).await?,
        },
        ..Default::default()
    };

    if let Some(target) = ctx.repl_bridge_sdk_target() {
        let repl_targets = collect_custom_targets(ctx, target).await?;

        for target in &repl_targets {
            let Some(agent_type) = target.subject.as_agent() else {
                continue;
            };
            plan.repl_metadata_by_language
                .entry(target.target_language)
                .or_default()
                .agents
                .insert(
                    agent_type.type_name.clone(),
                    ReplAgentMetadata {
                        client_dir: target.output_dir.clone(),
                        mode: agent_type.mode,
                    },
                );
        }

        plan.targets.extend(repl_targets);
    }

    deduplicate_bridge_targets(&mut plan.targets);

    Ok(plan)
}

fn deduplicate_bridge_targets(targets: &mut Vec<BridgeSdkTarget>) {
    let mut seen = HashSet::new();
    targets.retain(|target| {
        seen.insert((
            target.source.clone(),
            target.subject.kind(),
            target.subject.display_name().to_string(),
            target.target_language,
            target.bridge_mode,
            target.output_dir.clone(),
        ))
    });
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

pub(crate) async fn plan_explicit_manifest_guest_bridge_generation_for_components_lenient(
    ctx: &BuildContext<'_>,
    component_names: &[ComponentName],
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_manifest_targets_for_components_and_mode(
            ctx,
            component_names,
            component_names,
            Some(BridgeMode::Guest),
            true,
            true,
            false,
        )
        .await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_dependency_guest_bridge_generation_for_components_lenient(
    ctx: &BuildContext<'_>,
    source_component_names: &[ComponentName],
    selection_scope_component_names: &[ComponentName],
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_dependency_guest_bridge_targets(
            ctx,
            source_component_names,
            selection_scope_component_names,
        )
        .await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_manifest_external_bridge_generation_for_components_lenient(
    ctx: &BuildContext<'_>,
    component_names: &[ComponentName],
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_manifest_external_bridge_targets_for_components_lenient(
            ctx,
            component_names,
        )
        .await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_custom_bridge_generation(
    ctx: &BuildContext<'_>,
    custom_target: &CustomBridgeSdkTarget,
) -> anyhow::Result<BridgeGenerationPlan> {
    Ok(BridgeGenerationPlan {
        targets: collect_custom_targets(ctx, custom_target).await?,
        repl_metadata_by_language: BTreeMap::new(),
    })
}

pub(crate) async fn plan_repl_bridge_generation_lenient(
    ctx: &BuildContext<'_>,
    repl_target: &CustomBridgeSdkTarget,
) -> anyhow::Result<BridgeGenerationPlan> {
    let targets = collect_custom_targets_lenient(ctx, repl_target).await?;
    let mut repl_metadata_by_language = BTreeMap::<GuestLanguage, ReplMetadata>::new();

    for target in &targets {
        let Some(agent_type) = target.subject.as_agent() else {
            continue;
        };
        repl_metadata_by_language
            .entry(target.target_language)
            .or_default()
            .agents
            .insert(
                agent_type.type_name.clone(),
                ReplAgentMetadata {
                    client_dir: target.output_dir.clone(),
                    mode: agent_type.mode,
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
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    collect_manifest_targets_for_components_and_mode(
        ctx,
        component_names,
        component_names,
        Some(BridgeMode::External),
        true,
        false,
        false,
    )
    .await
}

pub(crate) async fn collect_custom_targets_lenient(
    ctx: &BuildContext<'_>,
    custom_target: &CustomBridgeSdkTarget,
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

        let mut agent_types = extract_and_store_component_metadata(ctx, component_name)
            .await?
            .agent_types;
        if should_filter_by_agent_type_name {
            agent_types.retain(|agent_type| agent_type_names.remove(&agent_type.type_name));
        }

        for agent_type in agent_types {
            let output_dir = custom_target
                .output_dir
                .as_ref()
                .map(|output_dir| {
                    output_dir.join(bridge_client_directory_name(
                        &agent_type.type_name,
                        BridgeMode::External,
                    ))
                })
                .unwrap_or_else(|| {
                    ctx.application().bridge_sdk_dir(
                        &agent_type.type_name,
                        target_language,
                        BridgeMode::External,
                    )
                });

            targets.push(BridgeSdkTarget {
                source: BridgeSdkTargetSource::local(component_name.clone()),
                subject: BridgeSdkTargetSubject::Agent(agent_type),
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
    validate_host_managed_bridge_targets(&targets)?;

    for target in targets {
        gen_bridge_sdk_target(ctx, target).await?;
    }

    Ok(())
}

pub(crate) fn validate_host_managed_bridge_targets(
    targets: &[BridgeSdkTarget],
) -> anyhow::Result<()> {
    for target in targets {
        let BridgeSdkTargetSubject::Agent(agent) = &target.subject else {
            continue;
        };
        validate_host_managed_agent_bridge_policy(agent, target.bridge_mode)?;
    }

    Ok(())
}

async fn collect_manifest_targets(
    ctx: &BuildContext<'_>,
    bridge_mode_filter: Option<BridgeMode>,
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
        &component_names,
        bridge_mode_filter,
        false,
        false,
        true,
    )
    .await
}

async fn collect_manifest_targets_for_components_and_mode(
    ctx: &BuildContext<'_>,
    source_component_names: &[ComponentName],
    selection_scope_component_names: &[ComponentName],
    bridge_mode_filter: Option<BridgeMode>,
    ignore_unmatched_matchers: bool,
    skip_missing_sources: bool,
    include_dependency_targets: bool,
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

        collect_agent_manifest_targets_for_entry(
            ctx,
            source_component_names,
            selection_scope_component_names,
            bridge_mode,
            target_language,
            sdk_targets.agents.clone().into_set(),
            &application_component_names,
            ignore_unmatched_matchers,
            skip_missing_sources,
            &mut targets,
        )
        .await?;

        collect_tool_manifest_targets_for_entry(
            ctx,
            source_component_names,
            selection_scope_component_names,
            bridge_mode,
            target_language,
            sdk_targets
                .tools
                .map(|tools| tools.clone().into_set())
                .unwrap_or_default(),
            &application_component_names,
            ignore_unmatched_matchers,
            skip_missing_sources,
            &mut targets,
        )
        .await?;
    }

    if include_dependency_targets
        && bridge_mode_filter.is_none_or(|bridge_mode| bridge_mode == BridgeMode::Guest)
    {
        targets.extend(
            collect_dependency_guest_bridge_targets(
                ctx,
                source_component_names,
                selection_scope_component_names,
            )
            .await?,
        );
    }

    Ok(targets)
}

#[allow(clippy::too_many_arguments)]
async fn collect_agent_manifest_targets_for_entry(
    ctx: &BuildContext<'_>,
    source_component_names: &[ComponentName],
    selection_scope_component_names: &[ComponentName],
    bridge_mode: BridgeMode,
    target_language: GuestLanguage,
    mut matchers: BTreeSet<String>,
    application_component_names: &BTreeSet<String>,
    ignore_unmatched_matchers: bool,
    skip_missing_sources: bool,
    targets: &mut Vec<BridgeSdkTarget>,
) -> anyhow::Result<()> {
    if matchers.is_empty() {
        return Ok(());
    }

    let is_matching_all = matchers.remove("*");

    for component_name in source_component_names {
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

        let mut agent_types = extract_and_store_component_metadata(ctx, component_name)
            .await?
            .agent_types;

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
                source: BridgeSdkTargetSource::local(component_name.clone()),
                subject: BridgeSdkTargetSubject::Agent(agent_type),
                target_language,
                bridge_mode,
                output_dir,
            });
        }
    }

    if !ignore_unmatched_matchers && !matchers.is_empty() {
        for component_name in ctx.application().component_names() {
            if !selection_scope_component_names.contains(component_name) {
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

    Ok(())
}

fn collect_remote_tool_manifest_targets_for_entry(
    ctx: &BuildContext<'_>,
    bridge_mode: BridgeMode,
    target_language: GuestLanguage,
    matchers: &mut BTreeSet<String>,
    is_matching_all: bool,
    targets: &mut Vec<BridgeSdkTarget>,
) -> anyhow::Result<()> {
    for (name, _) in ctx.application().remote_release_references() {
        if !is_matching_all && !matchers.remove(name.as_str()) {
            continue;
        }
        let grant = ctx.release_grant_by_name(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Remote tool '{}' is not granted to the selected environment",
                name
            )
        })?;
        targets.push(BridgeSdkTarget {
            source: BridgeSdkTargetSource::RemoteRelease {
                release_id: grant.release.id,
                version: grant.release.version.clone(),
                metadata_version: grant.release.metadata_version.clone(),
                metadata_digest: grant.release.metadata_digest,
                source_digest: grant.release.source_digest,
                manifest_source: ctx.application().tool_declarations()[name].source.clone(),
            },
            subject: BridgeSdkTargetSubject::Tool(grant.release.definition.clone()),
            target_language,
            bridge_mode,
            output_dir: ctx
                .application()
                .tool_bridge_sdk_dir(name.as_str(), target_language),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn collect_tool_manifest_targets_for_entry(
    ctx: &BuildContext<'_>,
    source_component_names: &[ComponentName],
    selection_scope_component_names: &[ComponentName],
    bridge_mode: BridgeMode,
    target_language: GuestLanguage,
    mut matchers: BTreeSet<String>,
    application_component_names: &BTreeSet<String>,
    ignore_unmatched_matchers: bool,
    skip_missing_sources: bool,
    targets: &mut Vec<BridgeSdkTarget>,
) -> anyhow::Result<()> {
    if matchers.is_empty() {
        return Ok(());
    }

    if let Some(error) = BridgeSdkTargetKind::Tool.support_error(bridge_mode, target_language) {
        logln("");
        log_error(error);
        bail!(NonSuccessfulExit)
    }

    let is_matching_all = matchers.remove("*");
    collect_remote_tool_manifest_targets_for_entry(
        ctx,
        bridge_mode,
        target_language,
        &mut matchers,
        is_matching_all,
        targets,
    )?;

    for component_name in source_component_names {
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

        let mut tools = extract_and_store_component_metadata(ctx, component_name)
            .await?
            .tools;

        collect_local_tool_manifest_targets_for_component(
            ctx.application(),
            component_name,
            bridge_mode,
            target_language,
            &mut matchers,
            is_matching_all,
            is_matching_component,
            &mut tools,
            targets,
        )?;
    }

    if !ignore_unmatched_matchers && !matchers.is_empty() {
        for component_name in ctx.application().component_names() {
            if !selection_scope_component_names.contains(component_name) {
                matchers.remove(component_name.as_str());
            }
        }
    }

    if !ignore_unmatched_matchers && !matchers.is_empty() {
        logln("");
        log_error(format!(
            "The following tool matchers were not found during {} bridge SDK generation: {}",
            bridge_sdk_target_name(target_language, bridge_mode).log_color_highlight(),
            matchers
                .iter()
                .map(|at| at.as_str().log_color_highlight().to_string())
                .join(", ")
        ));
        bail!(NonSuccessfulExit)
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_local_tool_manifest_targets_for_component(
    application: &crate::model::app::Application,
    component_name: &ComponentName,
    bridge_mode: BridgeMode,
    target_language: GuestLanguage,
    matchers: &mut BTreeSet<String>,
    is_matching_all: bool,
    is_matching_component: bool,
    tools: &mut Vec<golem_common::schema::tool::Tool>,
    targets: &mut Vec<BridgeSdkTarget>,
) -> anyhow::Result<()> {
    tools.retain(|tool| {
        let Some(name) = tool.name() else {
            return true;
        };
        let Ok(name) = ToolName::try_from(name) else {
            return true;
        };
        let Some(declaration) = application.tool_declarations().get(&name) else {
            return true;
        };

        is_matching_component
            || declaration
                .value
                .component
                .as_ref()
                .is_none_or(|selected| selected == component_name)
    });

    for tool in tools.iter() {
        let Some(name) = tool.name() else {
            continue;
        };
        let Ok(name) = ToolName::try_from(name) else {
            continue;
        };
        let Some(declaration) = application.tool_declarations().get(&name) else {
            continue;
        };
        let selected_as_remote = targets.iter().any(|target| {
            matches!(&target.source, BridgeSdkTargetSource::RemoteRelease { .. })
                && target.subject.display_name() == name.as_str()
        });

        if declaration.value.release.is_some()
            && (is_matching_all
                || is_matching_component
                || matchers.contains(name.as_str())
                || selected_as_remote)
        {
            let issue = ToolValidationIssue::error(
                ToolValidationPhase::DeclarationDiscoveryIdentity,
                ToolValidationCode::DuplicateImplementation,
                ToolEntityPath::tool(&name, "definition.name"),
                Some(declaration.source.clone()),
                format!(
                    "Tool '{name}' is provided both by the remote release declared in '{}' and component '{component_name}'",
                    declaration.source.display()
                ),
            );
            bail!(issue.render());
        }
    }

    if !is_matching_all && !is_matching_component {
        tools.retain(|tool| tool.name().is_some_and(|name| matchers.contains(name)));
    }

    for tool in tools.drain(..) {
        let Some(name) = tool.name() else {
            continue;
        };
        matchers.remove(name);

        let output_dir = application.tool_bridge_sdk_dir(name, target_language);
        targets.push(BridgeSdkTarget {
            source: BridgeSdkTargetSource::local(component_name.clone()),
            subject: BridgeSdkTargetSubject::Tool(tool),
            target_language,
            bridge_mode,
            output_dir,
        });
    }

    Ok(())
}

async fn collect_dependency_guest_bridge_targets(
    ctx: &BuildContext<'_>,
    source_component_names: &[ComponentName],
    selection_scope_component_names: &[ComponentName],
) -> anyhow::Result<Vec<BridgeSdkTarget>> {
    let mut targets = Vec::new();

    for component_name in source_component_names {
        if !ctx
            .application()
            .component(component_name)
            .agent_type_extraction_source_wasm()
            .exists()
        {
            continue;
        }

        let metadata = extract_and_store_component_metadata(ctx, component_name).await?;
        for agent_type in &metadata.agent_types {
            let dependency = ComponentDependency::Agent {
                component_name: component_name.clone(),
                agent_type_name: agent_type.type_name.clone(),
            };
            let target_languages = dependency_guest_bridge_target_languages(
                ctx,
                &dependency,
                selection_scope_component_names,
            );

            for target_language in target_languages {
                let output_dir = ctx
                    .application()
                    .dependency_bridge_sdk_dir(&agent_type.type_name, target_language);
                targets.push(BridgeSdkTarget {
                    source: BridgeSdkTargetSource::local(component_name.clone()),
                    subject: BridgeSdkTargetSubject::Agent(agent_type.clone()),
                    target_language,
                    bridge_mode: BridgeMode::Guest,
                    output_dir,
                });
            }
        }

        for tool in &metadata.tools {
            let Some(tool_name) = tool.name() else {
                continue;
            };
            let Ok(tool_dependency_name) = ToolName::try_from(tool_name) else {
                continue;
            };
            let dependency = ComponentDependency::Tool {
                source: crate::model::app::SubjectSource::Local {
                    component_name: component_name.clone(),
                },
                tool_name: tool_dependency_name,
            };
            let target_languages = dependency_guest_bridge_target_languages(
                ctx,
                &dependency,
                selection_scope_component_names,
            );

            for target_language in target_languages {
                let output_dir = ctx
                    .application()
                    .dependency_tool_bridge_sdk_dir(tool_name, target_language);
                targets.push(BridgeSdkTarget {
                    source: BridgeSdkTargetSource::local(component_name.clone()),
                    subject: BridgeSdkTargetSubject::Tool(tool.clone()),
                    target_language,
                    bridge_mode: BridgeMode::Guest,
                    output_dir,
                });
            }
        }
    }

    let remote_tool_dependencies = selection_scope_component_names
        .iter()
        .flat_map(|component_name| {
            ctx.application()
                .component(component_name)
                .properties()
                .dependencies
                .clone()
                .into_iter()
        })
        .filter(|dependency| {
            matches!(
                dependency,
                ComponentDependency::Tool {
                    source: crate::model::app::SubjectSource::RemoteRelease,
                    ..
                }
            )
        })
        .collect::<BTreeSet<_>>();

    for dependency in remote_tool_dependencies {
        let ComponentDependency::Tool {
            source: crate::model::app::SubjectSource::RemoteRelease,
            tool_name,
        } = &dependency
        else {
            unreachable!()
        };
        let grant = ctx.release_grant_by_name(tool_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Remote tool dependency '{}' is not granted to the selected environment",
                tool_name
            )
        })?;
        for target_language in dependency_guest_bridge_target_languages(
            ctx,
            &dependency,
            selection_scope_component_names,
        ) {
            targets.push(BridgeSdkTarget {
                source: BridgeSdkTargetSource::RemoteRelease {
                    release_id: grant.release.id,
                    version: grant.release.version.clone(),
                    metadata_version: grant.release.metadata_version.clone(),
                    metadata_digest: grant.release.metadata_digest,
                    source_digest: grant.release.source_digest,
                    manifest_source: ctx.application().tool_declarations()[tool_name]
                        .source
                        .clone(),
                },
                subject: BridgeSdkTargetSubject::Tool(grant.release.definition.clone()),
                target_language,
                bridge_mode: BridgeMode::Guest,
                output_dir: ctx
                    .application()
                    .dependency_tool_bridge_sdk_dir(tool_name.as_str(), target_language),
            });
        }
    }

    Ok(targets)
}

fn dependency_guest_bridge_target_languages(
    ctx: &BuildContext<'_>,
    dependency: &ComponentDependency,
    selection_scope_component_names: &[ComponentName],
) -> BTreeSet<GuestLanguage> {
    selection_scope_component_names
        .iter()
        .filter(|consumer_component_name| {
            ctx.application()
                .component(consumer_component_name)
                .properties()
                .dependencies
                .contains(dependency)
        })
        .filter_map(|consumer_component_name| {
            ctx.application()
                .component(consumer_component_name)
                .guess_language()
        })
        .filter(|language| supported_dependency_guest_bridge_target_language(dependency, *language))
        .collect()
}

fn supported_dependency_guest_bridge_target_language(
    dependency: &ComponentDependency,
    language: GuestLanguage,
) -> bool {
    match dependency {
        ComponentDependency::Agent { .. } => {
            BridgeSdkTargetKind::Agent.supports(BridgeMode::Guest, language)
        }
        ComponentDependency::Tool { .. } => {
            BridgeSdkTargetKind::Tool.supports(BridgeMode::Guest, language)
        }
    }
}

async fn collect_custom_targets(
    ctx: &BuildContext<'_>,
    custom_target: &CustomBridgeSdkTarget,
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
            let mut agent_types = extract_and_store_component_metadata(ctx, component_name)
                .await?
                .agent_types;

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
                    output_dir.join(bridge_client_directory_name(
                        &agent_type.type_name,
                        BridgeMode::External,
                    ))
                })
                .unwrap_or_else(|| {
                    ctx.application().bridge_sdk_dir(
                        &agent_type.type_name,
                        target_language,
                        BridgeMode::External,
                    )
                });

            targets.push(BridgeSdkTarget {
                source: BridgeSdkTargetSource::local(component_name.clone()),
                subject: BridgeSdkTargetSubject::Agent(agent_type),
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
    let freshness_source = match &target.source {
        BridgeSdkTargetSource::Local { component_name } => {
            ctx.application().component(component_name).final_wasm()
        }
        BridgeSdkTargetSource::RemoteRelease {
            manifest_source, ..
        } => manifest_source.clone(),
    };
    let target_name = target.subject.display_name().to_string();
    let target_kind = target.subject.kind().as_str();
    let output_dir = Utf8PathBuf::try_from(target.output_dir)?;

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(GenerateBridgeSdkMarkerHash {
            source: &target.source,
            target_name: &target_name,
            kind: target_kind,
            language: &target.target_language,
            bridge_mode: target.bridge_mode,
        })?
        .with_sources(|| vec![&freshness_source])
        .with_targets(|| vec![&output_dir])
        .run_async_or_skip(
            || async {
                log_action(
                    "Generating",
                    format!(
                        "{} bridge SDK for {} to {}",
                        bridge_sdk_target_name(target.target_language, target.bridge_mode)
                            .log_color_highlight(),
                        target_name.as_str().log_color_highlight(),
                        output_dir.log_color_highlight(),
                    ),
                );
                let _indent = LogIndent::new();

                match target.subject {
                    BridgeSdkTargetSubject::Agent(agent_type) => {
                        let mut generator: Box<dyn BridgeGenerator> = match (target.target_language, target.bridge_mode) {
                        (GuestLanguage::Rust, BridgeMode::External) => {
                            Box::new(RustBridgeGenerator::new_with_mode(
                                agent_type,
                                &output_dir,
                                false,
                                RustBridgeMode::ExternalRest,
                            )?)
                        }
                        (GuestLanguage::Rust, BridgeMode::Guest) => {
                            Box::new(RustBridgeGenerator::new_with_mode(
                                agent_type,
                                &output_dir,
                                false,
                                RustBridgeMode::GuestWasmRpc,
                            )?)
                        }
                        (GuestLanguage::TypeScript, BridgeMode::External) => Box::new(
                            TypeScriptBridgeGenerator::new(agent_type, &output_dir, false)?,
                        ),
                        (GuestLanguage::TypeScript, BridgeMode::Guest) => {
                            Box::new(TypeScriptBridgeGenerator::new_with_mode(
                                agent_type,
                                &output_dir,
                                false,
                                TypeScriptBridgeMode::GuestWasmRpc,
                            )?)
                        }
                        (GuestLanguage::Scala, BridgeMode::External) => {
                            Box::new(ScalaBridgeGenerator::new_with_mode(
                                agent_type,
                                &output_dir,
                                false,
                                ScalaBridgeMode::ExternalRest,
                            )?)
                        }
                        (GuestLanguage::Scala, BridgeMode::Guest) => {
                            Box::new(ScalaBridgeGenerator::new_with_mode(
                                agent_type,
                                &output_dir,
                                false,
                                ScalaBridgeMode::GuestWasmRpc,
                            )?)
                        }
                        (GuestLanguage::MoonBit, BridgeMode::External) => {
                            Box::new(MoonBitBridgeGenerator::new(agent_type, &output_dir, false)?)
                        }
                        (GuestLanguage::MoonBit, BridgeMode::Guest) => {
                            Box::new(MoonBitBridgeGenerator::new_with_mode(
                                agent_type,
                                &output_dir,
                                false,
                                MoonBitBridgeMode::GuestWasmRpc,
                            )?)
                        }
                    };

                        fs::remove(&output_dir)?;
                        generator.generate()
                    }
                    BridgeSdkTargetSubject::Tool(tool) => match (target.target_language, target.bridge_mode) {
                        (GuestLanguage::Rust, BridgeMode::Guest) => {
                            fs::remove(&output_dir)?;
                            RustToolBridgeGenerator::new(tool, &output_dir, false)?.generate()
                        }
                        (GuestLanguage::Scala, BridgeMode::Guest) => {
                            fs::remove(&output_dir)?;
                            ScalaToolBridgeGenerator::new(tool, &output_dir, false)?.generate()
                        }
                        (GuestLanguage::TypeScript, BridgeMode::Guest) => {
                            fs::remove(&output_dir)?;
                            TypeScriptToolBridgeGenerator::new(tool, &output_dir, false)?.generate()
                        }
                        (GuestLanguage::MoonBit, BridgeMode::Guest) => {
                            fs::remove(&output_dir)?;
                            MoonBitToolBridgeGenerator::new(tool, &output_dir, false)?.generate()
                        }
                        _ => bail!("tool guest bridge generation is only implemented for Rust, TypeScript, Scala and MoonBit guest bridges"),
                    },
                }
            },
            || {
                log_skipping_up_to_date(format!(
                    "generating {} bridge SDK for {} to {}",
                    bridge_sdk_target_name(target.target_language, target.bridge_mode)
                        .log_color_highlight(),
                    target_name.as_str().log_color_highlight(),
                    output_dir.log_color_highlight()
                ));
            },
        )
        .await
}

fn bridge_sdk_target_name(language: GuestLanguage, bridge_mode: BridgeMode) -> String {
    match bridge_mode {
        BridgeMode::External => language.to_string(),
        BridgeMode::Guest => format!("{} internal", language),
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
                left_target.subject.display_name(),
                left_output_dir.log_color_highlight(),
                bridge_sdk_target_name(right_target.target_language, right_target.bridge_mode),
                right_target.subject.display_name(),
                right_output_dir.log_color_highlight(),
            ));
        }
        bail!(NonSuccessfulExit)
    }

    Ok(())
}

pub(crate) fn validate_supported_bridge_targets(targets: &[BridgeSdkTarget]) -> anyhow::Result<()> {
    for target in targets {
        let target_kind = target.subject.kind();
        if let Some(error) = target_kind.support_error(target.bridge_mode, target.target_language) {
            bail!(error);
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBridgeResult {
    pub generated: bool,
}

impl NoTextOutput for GenerateBridgeResult {}
impl TextOutput for GenerateBridgeResult {}

impl StructuredOutput for GenerateBridgeResult {
    const KIND: &'static str = "generate-bridge";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::app::{Application, ApplicationPreload, ComponentPresetSelector};
    use crate::model::app_raw;
    use golem_common::model::Empty;
    use golem_common::model::agent::{AgentConfigSource, AgentMode, AgentTypeName, Snapshotting};
    use golem_common::model::component::ComponentName;
    use golem_common::schema::agent::AgentConfigDeclarationSchema;
    use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use golem_common::schema::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, AutoInjectedKind, InputSchema,
        NamedField, OutputSchema, SchemaGraph, SchemaType,
    };
    use indoc::indoc;
    use strum::IntoEnumIterator;
    use tempfile::{TempDir, tempdir};
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

    #[test]
    fn deduplicate_bridge_targets_preserves_distinct_output_directories() {
        let temp_dir = tempdir().unwrap();
        let mut targets = vec![
            bridge_sdk_target_with_mode(
                "AlphaAgent",
                GuestLanguage::Rust,
                BridgeMode::Guest,
                temp_dir.path().join("manifest/alpha-agent-guest-client"),
            ),
            bridge_sdk_target_with_mode(
                "AlphaAgent",
                GuestLanguage::Rust,
                BridgeMode::Guest,
                temp_dir.path().join("repl/alpha-agent-guest-client"),
            ),
        ];

        deduplicate_bridge_targets(&mut targets);

        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn deduplicate_bridge_targets_preserves_distinct_sources_for_collision_validation() {
        let output_dir = tempdir().unwrap().path().join("bridge/alpha-client");
        let mut targets = vec![
            bridge_sdk_target_with_mode(
                "AlphaAgent",
                GuestLanguage::Rust,
                BridgeMode::Guest,
                output_dir.clone(),
            ),
            BridgeSdkTarget {
                source: BridgeSdkTargetSource::local(ComponentName("other-component".to_string())),
                subject: BridgeSdkTargetSubject::Agent(agent_type("AlphaAgent")),
                target_language: GuestLanguage::Rust,
                bridge_mode: BridgeMode::Guest,
                output_dir,
            },
        ];

        deduplicate_bridge_targets(&mut targets);

        assert!(validate_no_output_dir_collisions(&targets).is_err());
    }

    #[test]
    fn validate_supported_bridge_targets_accepts_guest_targets_for_all_current_languages() {
        for language in GuestLanguage::iter() {
            let agent_target = bridge_sdk_target_with_mode(
                "AlphaAgent",
                language,
                BridgeMode::Guest,
                tempdir().unwrap().path().join("bridge/alpha-guest-client"),
            );
            let tool_target = BridgeSdkTarget {
                source: BridgeSdkTargetSource::local(ComponentName("component".to_string())),
                subject: BridgeSdkTargetSubject::Tool(tool("MyTool")),
                target_language: language,
                bridge_mode: BridgeMode::Guest,
                output_dir: tempdir()
                    .unwrap()
                    .path()
                    .join("bridge/my-tool-guest-client"),
            };

            validate_supported_bridge_targets(&[agent_target, tool_target]).unwrap();
        }
    }

    #[test]
    fn bridge_sdk_support_matrix_matches_current_capabilities() {
        let capabilities = [
            (BridgeSdkTargetKind::Agent, BridgeMode::External, true),
            (BridgeSdkTargetKind::Agent, BridgeMode::Guest, true),
            (BridgeSdkTargetKind::Tool, BridgeMode::External, false),
            (BridgeSdkTargetKind::Tool, BridgeMode::Guest, true),
        ];

        for (kind, mode, expected) in capabilities {
            for language in GuestLanguage::iter() {
                assert_eq!(
                    kind.supports(mode, language),
                    expected,
                    "{kind} {mode} bridge support for {language}"
                );
            }
        }
    }

    #[test]
    fn validate_supported_bridge_targets_reports_external_tool_mode_separately() {
        let target = BridgeSdkTarget {
            source: BridgeSdkTargetSource::local(ComponentName("component".to_string())),
            subject: BridgeSdkTargetSubject::Tool(tool("MyTool")),
            target_language: GuestLanguage::Rust,
            bridge_mode: BridgeMode::External,
            output_dir: tempdir().unwrap().path().join("bridge/my-tool-client"),
        };

        assert_eq!(
            validate_supported_bridge_targets(&[target])
                .unwrap_err()
                .to_string(),
            "external tool bridge SDKs are not supported yet"
        );
    }

    #[test]
    fn external_bridge_rejects_host_managed_method_types_before_touching_output() {
        let temp_dir = tempdir().unwrap();
        let output_dir = temp_dir.path().join("bridge/agent-client");
        std::fs::create_dir_all(&output_dir).unwrap();
        let sentinel = output_dir.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();

        let mut target = bridge_sdk_target_with_mode(
            "Agent",
            GuestLanguage::Rust,
            BridgeMode::External,
            output_dir,
        );
        let agent = match &mut target.subject {
            BridgeSdkTargetSubject::Agent(agent) => agent,
            BridgeSdkTargetSubject::Tool(_) => unreachable!(),
        };
        agent.methods.push(AgentMethodSchema {
            name: "forward".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters([NamedField::user_supplied(
                "credentials",
                SchemaType::list(SchemaType::secret(Default::default())),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });

        let error = validate_host_managed_bridge_targets(&[target])
            .unwrap_err()
            .to_string();
        assert!(error.contains("method `forward` input parameter `credentials`"));
        assert!(error.contains("host-managed capability `secret`"));
        assert!(sentinel.exists(), "preflight must not modify bridge output");
    }

    #[test]
    fn guest_bridge_allows_host_managed_method_inputs_and_outputs() {
        let mut target = bridge_sdk_target_with_mode(
            "Agent",
            GuestLanguage::Rust,
            BridgeMode::Guest,
            tempdir().unwrap().path().join("bridge/agent-client"),
        );
        let agent = match &mut target.subject {
            BridgeSdkTargetSubject::Agent(agent) => agent,
            BridgeSdkTargetSubject::Tool(_) => unreachable!(),
        };
        agent.methods.push(AgentMethodSchema {
            name: "forward".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters([NamedField::user_supplied(
                "credentials",
                SchemaType::secret(Default::default()),
            )]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::permission_card(
                Default::default(),
            ))),
            http_endpoint: vec![],
            read_only: None,
        });

        validate_host_managed_bridge_targets(&[target]).unwrap();
    }

    #[test]
    fn guest_bridge_rejects_host_managed_constructor_and_configuration_types() {
        let mut constructor_target = bridge_sdk_target_with_mode(
            "Agent",
            GuestLanguage::Rust,
            BridgeMode::Guest,
            tempdir().unwrap().path().join("bridge/constructor-client"),
        );
        let constructor_agent = match &mut constructor_target.subject {
            BridgeSdkTargetSubject::Agent(agent) => agent,
            BridgeSdkTargetSubject::Tool(_) => unreachable!(),
        };
        constructor_agent.constructor.input_schema = InputSchema::parameters([
            NamedField::user_supplied(
                "authorization",
                SchemaType::permission_card(Default::default()),
            ),
            NamedField::auto_injected(
                "host-secret",
                AutoInjectedKind::Principal,
                SchemaType::secret(Default::default()),
            ),
        ]);

        let error = validate_host_managed_bridge_targets(&[constructor_target])
            .unwrap_err()
            .to_string();
        assert!(error.contains("constructor parameter `authorization`"));
        assert!(error.contains("host-managed capability `permission-card`"));

        let mut config_target = bridge_sdk_target_with_mode(
            "Agent",
            GuestLanguage::Rust,
            BridgeMode::Guest,
            tempdir().unwrap().path().join("bridge/config-client"),
        );
        let config_agent = match &mut config_target.subject {
            BridgeSdkTargetSubject::Agent(agent) => agent,
            BridgeSdkTargetSubject::Tool(_) => unreachable!(),
        };
        config_agent.config.push(AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["limits".to_string()],
            value_type: SchemaType::quota_token(Default::default()),
        });

        let error = validate_host_managed_bridge_targets(&[config_target])
            .unwrap_err()
            .to_string();
        assert!(error.contains("configuration `limits`"));
        assert!(error.contains("host-managed capability `quota-token`"));
    }

    #[test]
    fn bridge_preflight_allows_host_supplied_capabilities() {
        let mut target = bridge_sdk_target_with_mode(
            "Agent",
            GuestLanguage::Rust,
            BridgeMode::External,
            tempdir().unwrap().path().join("bridge/agent-client"),
        );
        let agent = match &mut target.subject {
            BridgeSdkTargetSubject::Agent(agent) => agent,
            BridgeSdkTargetSubject::Tool(_) => unreachable!(),
        };
        agent.config.push(AgentConfigDeclarationSchema {
            source: AgentConfigSource::Secret,
            path: vec!["credentials".to_string()],
            value_type: SchemaType::secret(Default::default()),
        });
        agent.methods.push(AgentMethodSchema {
            name: "inspect".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters([NamedField::auto_injected(
                "authority",
                AutoInjectedKind::Principal,
                SchemaType::permission_card(Default::default()),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });

        validate_host_managed_bridge_targets(&[target]).unwrap();
    }

    #[test]
    fn dependency_guest_bridge_support_accepts_all_current_languages_for_agents_and_tools() {
        let component_name = ComponentName("component".to_string());
        let agent_dependency = ComponentDependency::Agent {
            component_name: component_name.clone(),
            agent_type_name: AgentTypeName("Agent".to_string()),
        };
        let tool_dependency = ComponentDependency::Tool {
            source: crate::model::app::SubjectSource::Local { component_name },
            tool_name: ToolName::try_from("tool").unwrap(),
        };

        for language in GuestLanguage::iter() {
            assert!(supported_dependency_guest_bridge_target_language(
                &agent_dependency,
                language
            ));
            assert!(supported_dependency_guest_bridge_target_language(
                &tool_dependency,
                language
            ));
        }
    }

    #[test]
    fn tool_guest_bridge_supported_language_names_match_all_current_languages() {
        assert_eq!(
            BridgeSdkTargetKind::Tool.supported_language_names(BridgeMode::Guest),
            GuestLanguage::iter()
                .map(|language| language.to_string())
                .join(", ")
        );
    }

    #[test]
    fn manifest_tool_matching_uses_declared_component_for_logical_name_and_wildcard() {
        let (application, _temp_dir) = application_with_duplicate_tool_exporters();
        let first = ComponentName("app:first".to_string());
        let second = ComponentName("app:second".to_string());

        for mut matchers in [BTreeSet::from(["echo".to_string()]), BTreeSet::new()] {
            let is_matching_all = matchers.is_empty();
            let mut targets = Vec::new();
            for component_name in [&first, &second] {
                collect_local_tool_manifest_targets_for_component(
                    &application,
                    component_name,
                    BridgeMode::Guest,
                    GuestLanguage::Rust,
                    &mut matchers,
                    is_matching_all,
                    false,
                    &mut vec![tool("echo")],
                    &mut targets,
                )
                .unwrap();
            }

            assert_eq!(
                targets
                    .iter()
                    .filter_map(|target| target.source.component_name())
                    .collect::<Vec<_>>(),
                vec![&second]
            );
        }

        let mut explicit_component_targets = Vec::new();
        collect_local_tool_manifest_targets_for_component(
            &application,
            &first,
            BridgeMode::Guest,
            GuestLanguage::Rust,
            &mut BTreeSet::new(),
            false,
            true,
            &mut vec![tool("echo")],
            &mut explicit_component_targets,
        )
        .unwrap();
        assert_eq!(
            explicit_component_targets[0].source.component_name(),
            Some(&first)
        );
    }

    #[test]
    fn manifest_tool_matching_rejects_local_and_remote_release_collision() {
        let (application, _temp_dir) = application_from_manifest(indoc! {r#"
            app: bridge-test

            environments:
              local:
                server: local

            components:
              app:provider:
                componentWasm: provider.wasm

            tools:
              echo:
                release:
                  releaseId: 00000000-0000-0000-0000-000000000001
        "#});
        let component_name = ComponentName("app:provider".to_string());

        let error = collect_local_tool_manifest_targets_for_component(
            &application,
            &component_name,
            BridgeMode::Guest,
            GuestLanguage::Rust,
            &mut BTreeSet::new(),
            true,
            false,
            &mut vec![tool("echo")],
            &mut Vec::new(),
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("DuplicateImplementation"), "{message}");
        assert!(message.contains("app:provider"), "{message}");
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
            source: BridgeSdkTargetSource::local(ComponentName("component".to_string())),
            subject: BridgeSdkTargetSubject::Agent(agent_type(agent_type_name)),
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

    fn tool(name: &str) -> Tool {
        Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: vec![],
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        }
    }

    fn application_with_duplicate_tool_exporters() -> (Application, TempDir) {
        application_from_manifest(indoc! {r#"
            app: bridge-test

            environments:
              local:
                server: local

            components:
              app:first:
                componentWasm: first.wasm
              app:second:
                componentWasm: second.wasm

            tools:
              echo:
                component: app:second
        "#})
    }

    fn application_from_manifest(contents: &str) -> (Application, TempDir) {
        let temp_dir = tempdir().unwrap();
        let manifest = temp_dir.path().join("golem.yaml");
        crate::fs::write(&manifest, contents).unwrap();

        let raw_apps = vec![app_raw::ApplicationWithSource::from_yaml_file(&manifest).unwrap()];
        let (preload, warns, errors) = Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "{}", warns.join("\n"));
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let ApplicationPreload {
            application_name,
            environments,
            local_server,
            version: _,
        } = preload.unwrap();

        let (application, warns, errors) = Application::from_raw_apps(
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
        assert!(warns.is_empty(), "{}", warns.join("\n"));
        assert!(errors.is_empty(), "{}", errors.join("\n"));

        (application.unwrap(), temp_dir)
    }
}
