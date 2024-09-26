use crate::model::oam::ApplicationWithSource;
use crate::model::validation::ValidatedResult;
use crate::model::wasm_rpc::{init_oam_app, Application, Component, DEFAULT_CONFIG_FILE_NAME};
use anyhow::{anyhow, Context};
use colored::Colorize;
use itertools::Itertools;
use std::io::Write;
use std::path::PathBuf;

pub enum ApplicationResolveMode {
    Explicit(Vec<PathBuf>),
}

pub fn init(component_name: String) -> anyhow::Result<()> {
    let file_name = DEFAULT_CONFIG_FILE_NAME;

    let mut file = std::fs::File::create_new(file_name)
        .with_context(|| format!("Failed to create {}", file_name))?;

    let app = init_oam_app(component_name);
    file.write_all(app.to_yaml_string().as_bytes())
        .with_context(|| format!("Failed to write {}", file_name))?;

    Ok(())
}

pub fn pre_build(application_resolve_mode: ApplicationResolveMode) -> anyhow::Result<()> {
    let app = collect_component_specifications(application_resolve_mode)?;

    println!("Component dependencies:");
    for (component_name, component) in &app.components_by_name {
        println!("  - {}", component_name);
        for dep_component_name in &component.wasm_rpc_dependencies {
            println!("    - {}", dep_component_name);
        }
    }

    Ok(())
}

pub fn post_build() -> anyhow::Result<()> {
    todo!()
}

fn collect_components(mode: ApplicationResolveMode) -> ValidatedResult<Vec<Component>> {
    let resolved_sources = match mode {
        ApplicationResolveMode::Explicit(sources) => {
            let non_unique_source_warns: Vec<_> = sources
                .iter()
                .counts()
                .into_iter()
                .filter(|(_, count)| *count > 1)
                .map(|(source, count)| {
                    format!(
                        "Source added multiple times, source: {}, count: {}",
                        source.to_string_lossy(),
                        count
                    )
                })
                .collect();

            ValidatedResult::from_value_and_warns(
                sources.into_iter().unique().collect::<Vec<_>>(),
                non_unique_source_warns,
            )
        }
    };

    let apps = resolved_sources.and_then(|sources| {
        sources
            .into_iter()
            .map(|source| {
                ValidatedResult::from_result(ApplicationWithSource::from_yaml_file(source))
                    .and_then(ApplicationWithSource::validate)
            })
            .collect::<ValidatedResult<Vec<_>>>()
    });

    apps.and_then(Component::from_oam_applications)
}

fn collect_component_specifications(mode: ApplicationResolveMode) -> anyhow::Result<Application> {
    to_anyhow(
        "Failed to load component specification, see problems above".to_string(),
        collect_components(mode).and_then(Application::from_components),
    )
}

fn to_anyhow<T>(message: String, result: ValidatedResult<T>) -> anyhow::Result<T> {
    fn print_warns(warns: Vec<String>) {
        let label = "Warning".yellow();
        for warn in warns {
            eprintln!("{}: {}", label, warn);
        }
    }

    fn print_errors(errors: Vec<String>) {
        let label = "Error".red();
        for error in errors {
            eprintln!("{}: {}", label, error);
        }
    }

    match result {
        ValidatedResult::Ok(value) => Ok(value),
        ValidatedResult::OkWithWarns(components, warns) => {
            print_warns(warns);
            println!();
            Ok(components)
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => {
            print_warns(warns);
            print_errors(errors);
            println!();

            Err(anyhow!(message))
        }
    }
}
