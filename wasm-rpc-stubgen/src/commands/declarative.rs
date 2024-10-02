use crate::commands::dependencies::UpdateCargoToml;
use crate::commands::log::{
    log_action, log_skipping_up_to_date, log_validated_action_result, log_warn_action,
};
use crate::model::oam;
use crate::model::validation::ValidatedResult;
use crate::model::wasm_rpc::{
    include_glob_patter_from_yaml_file, init_oam_app, Application, DEFAULT_CONFIG_FILE_NAME,
};
use crate::stub::StubDefinition;
use crate::{commands, WasmRpcOverride};
use anyhow::{anyhow, Context};
use colored::Colorize;
use glob::glob;
use itertools::Itertools;
use std::cmp::Ordering;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use tempfile::TempDir;
use walkdir::WalkDir;

pub struct Config {
    pub app_resolve_mode: ApplicationResolveMode,
    pub skip_up_to_date_checks: bool,
}

#[derive(Debug, Clone)]
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
    let app = load_app(&config)?;
    pre_build_app(&config, &app).await
}

async fn pre_build_app(config: &Config, app: &Application) -> anyhow::Result<()> {
    if app.all_wasm_rpc_dependencies().is_empty() {
        log_warn_action("Skipping", "building wasm rpc stubs, no dependency found");
    } else {
        log_action("Building", "wasm rpc stubs");
        for component_name in app.all_wasm_rpc_dependencies() {
            if is_up_to_date(
                config.skip_up_to_date_checks,
                || [app.component_wit(&component_name)],
                || {
                    [
                        app.stub_wasm(&component_name),
                        app.stub_wit(&component_name),
                    ]
                },
            ) {
                log_skipping_up_to_date(format!("building component stub: {}", component_name));
                continue;
            }

            log_action("Building", format!("wasm rpc stub: {}", component_name));

            let target_root = TempDir::new()?;
            let canonical_target_root = target_root.path().canonicalize()?;

            let stub_def = StubDefinition::new(
                &app.component_wit(&component_name),
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
                &app.stub_wasm(&component_name),
                &app.stub_wit(&component_name),
            )
            .await?
        }
    }

    for (component_name, component) in &app.wasm_components_by_name {
        if !component.wasm_rpc_dependencies.is_empty() {
            log_action(
                "Adding",
                format!("stub wit dependencies to {}", component_name),
            )
        }

        for dep_component_name in &component.wasm_rpc_dependencies {
            // TODO: this should check into the wit deps for the specific stubs or do folder diffs
            /*if is_up_to_date(
                config.skip_up_to_date_checks,
                || [app.stub_wit(dep_component_name)],
                || [app.component_wit(component_name)],
            ) {
                log_skipping_up_to_date(format!(
                    "adding {} stub wit dependency to {}",
                    dep_component_name, component_name
                ));
                continue;
            }*/

            log_action(
                "Adding",
                format!(
                    "{} stub wit dependency to {}",
                    dep_component_name, component_name
                ),
            );

            commands::dependencies::add_stub_dependency(
                &app.stub_wit(dep_component_name),
                &app.component_wit(component_name),
                true, // NOTE: in declarative mode we always use overwrite
                UpdateCargoToml::UpdateIfExists,
            )?
        }
    }

    Ok(())
}

pub fn component_build(config: Config) -> anyhow::Result<()> {
    let app = load_app(&config)?;
    component_build_app(&config, &app)
}

pub fn component_build_app(_config: &Config, app: &Application) -> anyhow::Result<()> {
    let components_with_build_steps = app
        .wasm_components_by_name
        .values()
        .filter(|component| !component.build_steps.is_empty())
        .collect::<Vec<_>>();

    if components_with_build_steps.is_empty() {
        log_warn_action(
            "Skipping",
            "building components, no components with build steps found",
        );
        return Ok(());
    }

    log_action("Building", "components");

    for component in components_with_build_steps {
        log_action("Building", format!("component: {}", component.name));
        for build_step in &component.build_steps {
            // TODO: handle up-to-date checks optionally based on step.input and step.output
            log_action("Executing", format!("command: {}", build_step.command));

            let command_tokens = build_step.command.split(' ').collect::<Vec<_>>();
            if command_tokens.is_empty() {
                return Err(anyhow!("Empty command!"));
            }

            let result = Command::new(command_tokens[0])
                .args(command_tokens.iter().skip(1))
                .current_dir(
                    build_step
                        .dir
                        .as_ref()
                        .map(|dir| component.source_dir().join(dir))
                        .unwrap_or_else(|| component.source_dir().to_path_buf()),
                )
                .status();

            if let Some(err) = result.err() {
                return Err(anyhow!("Command failed: {}", err));
            }
        }
    }

    Ok(())
}

pub fn post_build(config: Config) -> anyhow::Result<()> {
    let app = load_app(&config)?;
    post_build_app(&config, &app)
}

pub fn post_build_app(config: &Config, app: &Application) -> anyhow::Result<()> {
    for (component_name, component) in &app.wasm_components_by_name {
        let input_wasm = app.component_input_wasm(component_name);
        let output_wasm = app.component_output_wasm(component_name);

        if is_up_to_date(
            config.skip_up_to_date_checks,
            // We also include the component specification source,
            // so it triggers build in case deps are changed
            || [component.source.clone(), input_wasm.clone()],
            || [output_wasm.clone()],
        ) {
            log_skipping_up_to_date(format!(
                "composing wasm rpc dependencies ({}) into {}",
                component.wasm_rpc_dependencies.iter().join(", "),
                component_name,
            ));
            continue;
        }

        if component.wasm_rpc_dependencies.is_empty() {
            log_action(
                "Copying",
                format!(
                    "{} to {} (no wasm rpc dependencies found)",
                    input_wasm.to_string_lossy(),
                    output_wasm.to_string_lossy()
                ),
            );
            std::fs::create_dir_all(
                output_wasm
                    .parent()
                    .expect("Failed to get parent dir of output wasm"),
            )?;
            std::fs::copy(&input_wasm, &output_wasm)?;
        } else {
            log_action(
                "Composing",
                format!(
                    "wasm rpc dependencies ({}) into {}, dependencies",
                    component.wasm_rpc_dependencies.iter().join(", "),
                    component_name,
                ),
            );

            // TODO: "no dependencies of component were found" in not handled yet
            let stub_wasms = component
                .wasm_rpc_dependencies
                .iter()
                .map(|dep| app.stub_wasm(dep))
                .collect::<Vec<_>>();

            commands::composition::compose(
                app.component_input_wasm(component_name).as_path(),
                &stub_wasms,
                app.component_output_wasm(component_name).as_path(),
            )?;
        }
    }

    Ok(())
}

pub async fn build(config: Config) -> anyhow::Result<()> {
    let app = load_app(&config)?;

    pre_build_app(&config, &app).await?;
    component_build_app(&config, &app)?;
    post_build_app(&config, &app)?;

    Ok(())
}

fn load_app(config: &Config) -> anyhow::Result<Application> {
    to_anyhow(
        "Failed to load application manifest(s), see problems above".to_string(),
        load_app_validated(&config),
    )
}

fn load_app_validated(config: &Config) -> ValidatedResult<Application> {
    let sources = collect_sources(&config.app_resolve_mode);
    let oam_apps = sources.and_then(|sources| {
        sources
            .into_iter()
            .map(|source| {
                ValidatedResult::from_result(oam::ApplicationWithSource::from_yaml_file(source))
                    .and_then(oam::ApplicationWithSource::validate)
            })
            .collect::<ValidatedResult<Vec<_>>>()
    });

    log_action("Collecting", "components");

    let app = oam_apps.and_then(Application::from_oam_apps);

    log_validated_action_result("Found", &app, |app| {
        if app.wasm_components_by_name.is_empty() {
            "no components".to_string()
        } else {
            format!(
                "components: {}",
                app.wasm_components_by_name.keys().join(", ")
            )
        }
    });

    app
}

fn collect_sources(mode: &ApplicationResolveMode) -> ValidatedResult<Vec<PathBuf>> {
    log_action("Collecting", "sources");

    let sources = match mode {
        ApplicationResolveMode::Automatic => match find_main_source() {
            Some(source) => {
                std::env::set_current_dir(source.parent().expect("Failed ot get config parent"))
                    .expect("Failed to set current dir for config parent");

                match include_glob_patter_from_yaml_file(source.as_path()) {
                    Some(pattern) => ValidatedResult::from_result(
                        glob(pattern.as_str())
                            .map_err(|err| {
                                format!(
                                    "Failed to compile glob pattern: {}, source: {}, error: {}",
                                    pattern,
                                    source.to_string_lossy(),
                                    err
                                )
                            })
                            .and_then(|matches| {
                                matches.collect::<Result<Vec<_>, _>>().map_err(|err| {
                                    format!(
                                        "Failed to resolve glob pattern: {}, source: {}, error: {}",
                                        pattern,
                                        source.to_string_lossy(),
                                        err
                                    )
                                })
                            }),
                    )
                    .map(|mut sources| {
                        sources.insert(0, source);
                        sources
                    }),
                    None => ValidatedResult::Ok(vec![source]),
                }
            }
            None => ValidatedResult::from_error("No config file found!".to_string()),
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

            ValidatedResult::from_value_and_warns(sources.clone(), non_unique_source_warns)
        }
    };

    log_validated_action_result("Found", &sources, |sources| {
        if sources.is_empty() {
            "no sources".to_string()
        } else {
            format!(
                "sources: {}",
                sources
                    .iter()
                    .map(|source| source.to_string_lossy())
                    .join(", ")
            )
        }
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

fn is_up_to_date<S, T, FS, FT>(skip_check: bool, sources: FS, targets: FT) -> bool
where
    S: IntoIterator<Item = PathBuf>,
    T: IntoIterator<Item = PathBuf>,
    FS: FnOnce() -> S,
    FT: FnOnce() -> T,
{
    if skip_check {
        return true;
    }

    fn max_modified(path: &Path) -> Option<SystemTime> {
        let mut max_modified: Option<SystemTime> = None;
        let mut update_max_modified = |modified: SystemTime| {
            if max_modified.map_or(true, |max_mod| max_mod.cmp(&modified) == Ordering::Less) {
                max_modified = Some(modified)
            }
        };

        if let Ok(metadata) = std::fs::metadata(path) {
            if metadata.is_dir() {
                WalkDir::new(path)
                    .into_iter()
                    .filter_map(|entry| entry.ok().and_then(|entry| entry.metadata().ok()))
                    .filter(|metadata| !metadata.is_dir())
                    .filter_map(|metadata| metadata.modified().ok())
                    .for_each(update_max_modified)
            } else if let Ok(modified) = metadata.modified() {
                update_max_modified(modified)
            }
        }

        max_modified
    }

    fn max_modified_short_circuit_on_missing<I: IntoIterator<Item = PathBuf>>(
        paths: I,
    ) -> Option<SystemTime> {
        // Using Result and collect for short-circuit on any missing mod time
        paths
            .into_iter()
            .map(|path| max_modified(path.as_path()).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .and_then(|mod_times| mod_times.into_iter().max().ok_or(()))
            .ok()
    }

    let targets = targets();

    let max_target_modified = max_modified_short_circuit_on_missing(targets);

    let max_target_modified = match max_target_modified {
        Some(modified) => modified,
        None => return false,
    };

    let sources = sources();

    let max_source_modified = max_modified_short_circuit_on_missing(sources);

    match max_source_modified {
        Some(max_source_modified) => {
            max_source_modified.cmp(&max_target_modified) == Ordering::Less
        }
        None => false,
    }
}
