use crate::commands::log::{log_action, log_validated_action_result};
use crate::model::oam::ApplicationWithSource;
use crate::model::validation::ValidatedResult;
use crate::model::wasm_rpc::{init_oam_app, Application, DEFAULT_CONFIG_FILE_NAME};
use crate::stub::StubDefinition;
use crate::{commands, WasmRpcOverride};
use anyhow::{anyhow, Context};
use colored::Colorize;
use itertools::Itertools;
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

pub struct Config {
    pub app_resolve_mode: ApplicationResolveMode,
    pub skip_up_to_date_checks: bool,
}

pub enum ApplicationResolveMode {
    Automatic,
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

pub async fn pre_build(config: Config) -> anyhow::Result<()> {
    let app = to_anyhow(
        "Failed to load application manifest(s), see problems above".to_string(),
        load_app(config.app_resolve_mode),
    )?;

    for component_name in app.all_wasm_rpc_dependencies() {
        log_action("Building", format!("component stub: {}", component_name));

        let target_root = TempDir::new()?;
        let canonical_target_root = target_root.path().canonicalize()?;

        let stub_def = StubDefinition::new(
            &app.stub_source_wit_root(&component_name),
            &canonical_target_root,
            &app.stub_world(&component_name),
            &app.stub_crate_version(&component_name),
            &WasmRpcOverride {
                wasm_rpc_path_override: app.stub_wasm_rpc_path(&component_name),
                wasm_rpc_version_override: app.stub_wasm_rpc_version(&component_name),
            },
            app.stub_always_inline_types(&component_name),
        )
        .context("Failed to gather information for the stub generator")?;

        commands::generate::build(
            &stub_def,
            &app.stub_dest_wasm(&component_name),
            &app.stub_dest_wit(&component_name),
        )
        .await?
    }

    for (component_name, component) in &app.components_by_name {
        if !component.wasm_rpc_dependencies.is_empty() {
            log_action(
                "Adding",
                format!("stub wit dependencies to {}", component_name),
            )
        }

        for dep_component_name in &component.wasm_rpc_dependencies {
            log_action(
                "Adding",
                format!(
                    "{} stub wit dependency to {}",
                    dep_component_name, component_name
                ),
            );

            commands::dependencies::add_stub_dependency(
                &app.stub_dest_wit(dep_component_name),
                &app.component_wit(component_name),
                true,  // TODO: is overwrite ever not needed?
                false, // TODO: should update cargo toml be exposed?
            )?
        }
    }

    Ok(())
}

pub fn post_build(_config: Config) -> anyhow::Result<()> {
    todo!()
}

fn load_app(mode: ApplicationResolveMode) -> ValidatedResult<Application> {
    let sources = collect_sources(mode);
    let oam_apps = sources.and_then(|sources| {
        sources
            .into_iter()
            .map(|source| {
                ValidatedResult::from_result(ApplicationWithSource::from_yaml_file(source))
                    .and_then(ApplicationWithSource::validate)
            })
            .collect::<ValidatedResult<Vec<_>>>()
    });

    log_action("Collecting", "components");

    let app = oam_apps.and_then(Application::from_oam_apps);

    log_validated_action_result("Found", &app, |app| {
        format!("components: {}", app.components_by_name.keys().join(", "))
    });

    app
}

fn collect_sources(mode: ApplicationResolveMode) -> ValidatedResult<Vec<PathBuf>> {
    log_action("Collecting", "sources");

    let sources = match mode {
        ApplicationResolveMode::Automatic => match find_main_source() {
            Some(source) => {
                std::env::set_current_dir(source.parent().expect("Failed ot get config parent"))
                    .expect("Failed to set current dir for config parent");

                ValidatedResult::Ok(vec![source])
            }
            None => {
                ValidatedResult::WarnsAndErrors(vec![], vec!["No config file found!".to_string()])
            }
        },
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

    log_validated_action_result("Found", &sources, |sources| {
        format!(
            "sources: {}",
            sources
                .iter()
                .map(|source| source.to_string_lossy())
                .join(", ")
        )
    });

    sources
}

fn find_main_source() -> Option<PathBuf> {
    let mut current_dir = std::env::current_dir().expect("Failed to get current dir");
    let mut last_source: Option<PathBuf> = None;

    loop {
        let file = current_dir.join(DEFAULT_CONFIG_FILE_NAME);
        if current_dir.join(DEFAULT_CONFIG_FILE_NAME).exists() {
            last_source = Some(file);
        }
        match current_dir.parent() {
            Some(parent_dir) => current_dir = parent_dir.to_path_buf(),
            None => {
                break;
            }
        }
    }

    last_source
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
