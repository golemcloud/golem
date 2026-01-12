use crate::app::build::new_task_up_to_date_check;
use crate::app::build::task_result_marker::GenerateBridgeSdkMarkerHash;
use crate::app::context::ApplicationContext;
use crate::bridge_gen::typescript::TypeScriptBridgeGenerator;
use crate::bridge_gen::BridgeGenerator;
use crate::error::NonSuccessfulExit;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::model::app::BridgeSdkTarget;
use crate::model::text::fmt::log_error;
use anyhow::bail;
use camino::Utf8PathBuf;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_templates::model::GuestLanguage;
use itertools::Itertools;

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
                let final_linked_wasm = component.final_linked_wasm();
                let target_language = custom_target
                    .target_language
                    .or_else(|| component.guess_language())
                    .unwrap_or(GuestLanguage::TypeScript);

                let agent_types = {
                    let mut agent_types = ctx
                        .wit
                        .get_extracted_agent_types(component_name, &final_linked_wasm)
                        .await?;

                    if should_filter_by_agent_type_name {
                        agent_types.retain(|agent_type| {
                            agent_type_names.contains(&agent_type.type_name)
                                || agent_type_names.contains(&agent_type.type_name.to_wit_naming())
                        });
                    }

                    agent_types.iter().for_each(|agent_type| {
                        agent_type_names.remove(&agent_type.type_name);
                        agent_type_names.remove(&agent_type.type_name.to_wit_naming());
                    });

                    agent_types
                };

                for agent_type in agent_types {
                    targets.push(BridgeSdkTarget {
                        component_name: component_name.clone(),
                        agent_type,
                        target_language,
                        output_dir: None,
                    });
                }
            }

            if agent_type_names.len() != 0 {
                log_error(format!(
                    "The following agent type names were not found: {}",
                    agent_type_names
                        .iter()
                        .map(|at| at.as_str().log_color_highlight().to_string())
                        .join(", ")
                ));
                bail!(NonSuccessfulExit)
            }

            if custom_target.agent_type_names.len() == 1 {
                targets
                    .iter_mut()
                    .for_each(|target| target.output_dir = custom_target.output_dir.clone());
            } else {
                targets.iter_mut().for_each(|target| {
                    target.output_dir = custom_target
                        .output_dir
                        .as_ref()
                        .map(|output_dir| output_dir.join(target.agent_type.wrapper_type_name()));
                });
            }
        }
        None => {
            // TODO: select targets based on component selection AND manifest
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
    let output_dir = match target.output_dir {
        Some(output_dir) => output_dir,
        None => ctx
            .application
            .default_bridge_sdk_dir(&agent_type_name, target.target_language),
    };
    let output_dir = Utf8PathBuf::try_from(output_dir)?;

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
