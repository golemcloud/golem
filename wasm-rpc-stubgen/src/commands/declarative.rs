use crate::model::oam::ApplicationWithSource;
use crate::model::validation::ValidatedResult;
use crate::model::wasm_rpc::{init_oam_app, Application, Component, DEFAULT_CONFIG_FILE_NAME};
use crate::stub::StubDefinition;
use crate::{commands, WasmRpcOverride};
use anyhow::{anyhow, Context};
use colored::Colorize;
use itertools::Itertools;
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

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

pub async fn pre_build(application_resolve_mode: ApplicationResolveMode) -> anyhow::Result<()> {
    let app = collect_components_into_app(application_resolve_mode)?;

    for (component_name, component) in &app.components_by_name {
        println!("  - {}", component_name.green());
        for dep_component_name in &component.wasm_rpc_dependencies {
            println!("    - {}", dep_component_name);
        }
    }
    println!("\n");

    let build_dir = PathBuf::from("./build");

    for dep_component_name in app.all_wasm_rpc_dependencies() {
        log_action(
            "Building",
            format!("component stub: {}", dep_component_name),
        );

        let component_build_dir = build_dir.join(&dep_component_name);

        let component = app
            .components_by_name
            .get(&dep_component_name)
            .expect("Failed to lookup component dependency by name");

        let source_wit_root = component.resolve_wit_path();
        let world = None;
        let always_inline_types = true;

        let target_root = TempDir::new()?;
        let canonical_target_root = target_root.path().canonicalize()?;

        let dest_wasm = component_build_dir.join("stub.wasm");
        let dest_wit_root = component_build_dir.join("wit");

        let stub_def = StubDefinition::new(
            &source_wit_root,
            &canonical_target_root,
            &world,
            "0.0.1",
            &WasmRpcOverride::default(),
            always_inline_types,
        )
        .context("Failed to gather information for the stub generator")?;

        commands::generate::build(&stub_def, &dest_wasm, &dest_wit_root).await?
    }

    Ok(())
}

pub fn post_build() -> anyhow::Result<()> {
    todo!()
}

fn collect_components(mode: ApplicationResolveMode) -> ValidatedResult<Vec<Component>> {
    log_action("Collecting", "sources");

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

    log_validated_action_result("Found", &resolved_sources, |sources| {
        format!(
            "sources: {}",
            sources
                .iter()
                .map(|source| source.to_string_lossy())
                .join(", ")
        )
    });

    let apps = resolved_sources.and_then(|sources| {
        sources
            .into_iter()
            .map(|source| {
                ValidatedResult::from_result(ApplicationWithSource::from_yaml_file(source))
                    .and_then(ApplicationWithSource::validate)
            })
            .collect::<ValidatedResult<Vec<_>>>()
    });

    log_action("Collecting", "components");

    let components = apps.and_then(Component::from_oam_applications);

    log_validated_action_result("Found", &components, |components| {
        format!(
            "components: {}",
            components
                .iter()
                .map(|component| component.name.as_str())
                .join(", ")
        )
    });

    components
}

fn collect_components_into_app(mode: ApplicationResolveMode) -> anyhow::Result<Application> {
    log_action("Collecting and validating", "components and dependencies");

    to_anyhow(
        "Failed to load component specification(s), see problems above".to_string(),
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

fn log_action<T: AsRef<str>>(action: &str, subject: T) {
    println!("{} {}", action.green(), subject.as_ref())
}

fn log_validated_action_result<T, F>(action: &str, result: &ValidatedResult<T>, to_log: F)
where
    F: Fn(&T) -> String,
{
    result
        .as_ok_ref()
        .iter()
        .for_each(|value| log_action(action, to_log(value)));
}
