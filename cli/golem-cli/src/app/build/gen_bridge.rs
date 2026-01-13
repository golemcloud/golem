use crate::app::build::extract_agent_type::extract_and_cache_agent_types;
use crate::app::build::new_task_up_to_date_check;
use crate::app::build::task_result_marker::GenerateBridgeSdkMarkerHash;
use crate::app::context::ApplicationContext;
use crate::bridge_gen::typescript::TypeScriptBridgeGenerator;
use crate::bridge_gen::BridgeGenerator;
use crate::error::NonSuccessfulExit;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, logln, LogColorize, LogIndent};
use crate::model::app::BridgeSdkTarget;
use crate::model::text::fmt::log_error;
use anyhow::bail;
use camino::Utf8PathBuf;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_templates::model::GuestLanguage;
use heck::ToKebabCase;
use itertools::Itertools;
use std::collections::BTreeSet;
use strum::IntoEnumIterator;

pub async fn gen_bridge(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    let mut targets = vec![];

    match &ctx.config.custom_bridge_sdk_target {
        Some(custom_target) => {
            let should_filter_by_agent_type_name = !custom_target.agent_type_names.is_empty();
            let mut agent_type_names = custom_target.agent_type_names.clone();
            for component_name in ctx.selected_component_names() {
                if !ctx.wit.is_agent(component_name) {
                    continue;
                }

                let component = ctx.application.component(component_name);
                let target_language = custom_target
                    .target_language
                    .or_else(|| component.guess_language())
                    .unwrap_or(GuestLanguage::TypeScript);

                let agent_types = {
                    let mut agent_types =
                        extract_and_cache_agent_types(ctx, component_name).await?;

                    if should_filter_by_agent_type_name {
                        agent_types.retain(|agent_type| {
                            agent_type_names.remove(&agent_type.type_name)
                                || agent_type_names.remove(&agent_type.type_name.to_wit_naming())
                        });
                    }

                    agent_types
                };

                for agent_type in agent_types {
                    let output_dir = custom_target
                        .output_dir
                        .as_ref()
                        .map(|output_dir| {
                            output_dir.join(agent_type.type_name.as_str().to_kebab_case())
                        })
                        .unwrap_or_else(|| {
                            ctx.application
                                .bridge_sdk_dir(&agent_type.type_name, target_language)
                        });

                    targets.push(BridgeSdkTarget {
                        component_name: component_name.clone(),
                        agent_type,
                        target_language,
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
        }
        None => {
            for target_language in GuestLanguage::iter() {
                let Some(lang_sdks) = ctx.application.bridge_sdks().for_language(target_language)
                else {
                    continue;
                };

                let mut matchers = lang_sdks
                    .agents
                    .clone()
                    .into_vec()
                    .into_iter()
                    .collect::<BTreeSet<_>>();

                if matchers.is_empty() {
                    continue;
                }

                let is_matching_all = matchers.remove("*");

                for component_name in ctx.selected_component_names() {
                    if !ctx.wit.is_agent(component_name) {
                        continue;
                    }

                    let is_matching_component = matchers.remove(component_name.as_str());

                    let mut agent_types =
                        extract_and_cache_agent_types(ctx, component_name).await?;

                    if !is_matching_all && !is_matching_component {
                        agent_types
                            .retain(|agent_type| matchers.contains(agent_type.type_name.as_str()));
                    }

                    for agent_type in agent_types {
                        matchers.remove(agent_type.type_name.as_str());

                        let output_dir = ctx
                            .application
                            .bridge_sdk_dir(&agent_type.type_name, target_language);
                        targets.push(BridgeSdkTarget {
                            component_name: component_name.clone(),
                            agent_type,
                            target_language,
                            output_dir,
                        });
                    }
                }

                if !matchers.is_empty() {
                    // Remove "non-selected" components
                    for component_name in ctx.application.component_names() {
                        matchers.remove(component_name.as_str());
                    }
                }

                if !matchers.is_empty() {
                    logln("");
                    log_error(format!(
                        "The following agent matchers were not found during {} bridge SDK generation: {}",
                        target_language.to_string().log_color_highlight(),
                        matchers
                            .iter()
                            .map(|at| at.as_str().log_color_highlight().to_string())
                            .join(", ")
                    ));
                    bail!(NonSuccessfulExit)
                }
            }
        }
    }

    if targets.is_empty() {
        return Ok(());
    }

    log_action("Generating", "bridge SDKs");
    let _indent = LogIndent::new();

    for target in targets {
        gen_bridge_sdk_target(ctx, target).await?;
    }

    Ok(())
}

async fn gen_bridge_sdk_target(
    ctx: &ApplicationContext,
    target: BridgeSdkTarget,
) -> anyhow::Result<()> {
    let component = ctx.application.component(&target.component_name);
    let final_linked_wasm = component.final_linked_wasm();
    let agent_type_name = target.agent_type.type_name.clone();
    let output_dir = Utf8PathBuf::try_from(target.output_dir)?;

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(GenerateBridgeSdkMarkerHash {
            component_name: &target.component_name,
            agent_type_name: &target.agent_type.type_name,
            language: &target.target_language,
        })?
        .with_sources(|| vec![&final_linked_wasm])
        .with_targets(|| vec![&output_dir])
        .run_async_or_skip(
            || async {
                log_action(
                    "Generating",
                    format!(
                        "{} bridge SDK for {} to {}",
                        target.target_language.to_string().log_color_highlight(),
                        agent_type_name.as_str().log_color_highlight(),
                        output_dir.log_color_highlight(),
                    ),
                );
                let _indent = LogIndent::new();

                let generator: Box<dyn BridgeGenerator> = match target.target_language {
                    GuestLanguage::Rust => {
                        todo!("Rust bridge generator not implemented yet")
                    }
                    GuestLanguage::TypeScript => Box::new(TypeScriptBridgeGenerator::new(
                        target.agent_type,
                        &output_dir,
                        false,
                    )),
                };

                fs::remove(&output_dir)?;
                generator.generate()
            },
            || {
                log_skipping_up_to_date(format!(
                    "generating {} bridge SDK for {} to {}",
                    target.target_language.to_string().log_color_highlight(),
                    agent_type_name.as_str().log_color_highlight(),
                    output_dir.log_color_highlight()
                ));
            },
        )
        .await
}
