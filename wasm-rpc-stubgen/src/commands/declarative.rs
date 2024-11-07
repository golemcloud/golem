use crate::commands::log::{
    log_action, log_skipping_up_to_date, log_validated_action_result, log_warn_action, LogColorize,
    LogIndent,
};
use crate::fs::copy;
use crate::model::oam;
use crate::model::validation::ValidatedResult;
use crate::model::wasm_rpc::{
    include_glob_patter_from_yaml_file, init_oam_app, Application, DEFAULT_CONFIG_FILE_NAME,
};
use crate::stub::{StubConfig, StubDefinition};
use crate::wit_generate::{
    add_stub_as_dependency_to_wit_dir, extract_main_interface_as_wit_dep, AddStubAsDepConfig,
    UpdateCargoToml,
};
use crate::wit_resolve::ResolvedWitApplication;
use crate::{commands, WasmRpcOverride};
use anyhow::{anyhow, Context, Error};
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
    pub app_resolve_mode: ApplicationSourceMode,
    pub skip_up_to_date_checks: bool,
}

#[derive(Debug, Clone)]
pub enum ApplicationSourceMode {
    Automatic,
    Explicit(Vec<PathBuf>),
}

struct ApplicationContext {
    config: Config,
    application: Application,
    wit: ResolvedWitApplication,
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

pub async fn pre_component_build(config: Config) -> anyhow::Result<()> {
    let mut ctx = create_context(config)?;
    pre_component_build_ctx(&mut ctx).await
}

async fn pre_component_build_ctx(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    log_action("Executing", "pre-component-build steps");
    let _indent = LogIndent::new();

    let changed_any = extract_interface_packages(ctx)?;
    if changed_any {
        update_wit_context(ctx)?;
    }

    if ctx.application.all_wasm_rpc_dependencies().is_empty() {
        log_warn_action("Skipping", "building wasm rpc stubs, no dependency found");
    } else {
        log_action("Building", "wasm rpc stubs");
        let _indent = LogIndent::new();

        for component_name in ctx.application.all_wasm_rpc_dependencies() {
            if is_up_to_date(
                ctx.config.skip_up_to_date_checks,
                || [ctx.application.component_output_wit(&component_name)],
                || {
                    [
                        ctx.application.stub_wasm(&component_name),
                        ctx.application.stub_wit(&component_name),
                    ]
                },
            ) {
                log_skipping_up_to_date(format!(
                    "building wasm rpc stub for {}",
                    component_name.log_color_highlight()
                ));
                continue;
            }

            log_action(
                "Building",
                format!("wasm rpc stub: {}", component_name.log_color_highlight()),
            );
            let _indent = LogIndent::new();

            let target_root = TempDir::new()?;
            let canonical_target_root = target_root.path().canonicalize()?;

            let stub_def = StubDefinition::new(StubConfig {
                source_wit_root: ctx.application.component_output_wit(&component_name),
                target_root: canonical_target_root,
                selected_world: ctx.application.stub_world(&component_name),
                stub_crate_version: ctx.application.stub_crate_version(&component_name),
                wasm_rpc_override: WasmRpcOverride {
                    wasm_rpc_path_override: ctx.application.stub_wasm_rpc_path(&component_name),
                    wasm_rpc_version_override: ctx
                        .application
                        .stub_wasm_rpc_version(&component_name),
                },
                extract_source_interface_package: false,
            })
            .context("Failed to gather information for the stub generator")?;

            commands::generate::build(
                &stub_def,
                &ctx.application.stub_wasm(&component_name),
                &ctx.application.stub_wit(&component_name),
            )
            .await?
        }
    }

    for (component_name, component) in &ctx.application.wasm_components_by_name {
        if !component.wasm_rpc_dependencies.is_empty() {
            log_action(
                "Adding",
                format!(
                    "stub wit dependencies to {}",
                    component_name.log_color_highlight()
                ),
            )
        }
        let _indent = LogIndent::new();

        for dep_component_name in &component.wasm_rpc_dependencies {
            // TODO: this should check into the wit deps for the specific stubs or do folder diffs
            if is_up_to_date(
                ctx.config.skip_up_to_date_checks
                    || !ctx.wit.has_as_wit_dep(component_name, dep_component_name),
                || [ctx.application.stub_wit(dep_component_name)],
                || [ctx.application.component_output_wit(component_name)],
            ) {
                log_skipping_up_to_date(format!(
                    "adding {} stub wit dependency to {}",
                    dep_component_name.log_color_highlight(),
                    component_name.log_color_highlight()
                ));
                continue;
            }

            log_action(
                "Adding",
                format!(
                    "{} stub wit dependency to {}",
                    dep_component_name.log_color_highlight(),
                    component_name.log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            add_stub_as_dependency_to_wit_dir(AddStubAsDepConfig {
                stub_wit_root: ctx.application.stub_wit(dep_component_name),
                dest_wit_root: ctx.application.component_output_wit(component_name),
                update_cargo_toml: UpdateCargoToml::UpdateIfExists,
            })?
        }
    }

    Ok(())
}

fn extract_interface_packages(ctx: &ApplicationContext) -> Result<bool, Error> {
    let mut changed_any = false;
    for component_name in ctx.application.wasm_components_by_name.keys() {
        let component_input_wit = ctx.application.component_input_wit(component_name);
        let component_output_wit = ctx.application.component_output_wit(component_name);

        if is_up_to_date(
            ctx.config.skip_up_to_date_checks || !ctx.wit.is_dep_graph_up_to_date(component_name),
            || [component_input_wit.clone()],
            || [component_output_wit.clone()],
        ) {
            log_skipping_up_to_date(format!(
                "extracting main interface package from {}",
                component_name.log_color_highlight()
            ));
        } else {
            log_action(
                "Extracting",
                format!(
                    "main interface package from {} to {}",
                    component_input_wit.log_color_highlight(),
                    component_output_wit.log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            changed_any = true;

            if component_output_wit.exists() {
                log_warn_action(
                    "Deleting",
                    format!(
                        "output wit directory {}",
                        component_output_wit.log_color_highlight()
                    ),
                );
                std::fs::remove_dir_all(&component_output_wit)?;
            }

            {
                log_action(
                    "Copying",
                    format!(
                        "wit sources from {} to {}",
                        component_input_wit.log_color_highlight(),
                        component_output_wit.log_color_highlight()
                    ),
                );
                let _indent = LogIndent::new();

                let dir_content = fs_extra::dir::get_dir_content(&component_input_wit)
                    .with_context(|| {
                        anyhow!(
                            "Failed to read component input directory entries for {}",
                            component_input_wit.display()
                        )
                    })?;

                for file in dir_content.files {
                    let from = PathBuf::from(&file);
                    let to = component_output_wit.join(
                        from.strip_prefix(&component_input_wit).with_context(|| {
                            anyhow!("Failed to strip prefix for source {}", &file)
                        })?,
                    );

                    log_action(
                        "Copying",
                        format!("wit source {} to {}", from.display(), to.display()),
                    );
                    copy(from, to)?;
                }
            }

            extract_main_interface_as_wit_dep(&component_output_wit)?;
        }
    }
    Ok(changed_any)
}

pub fn component_build(config: Config) -> anyhow::Result<()> {
    let ctx = create_context(config)?;
    component_build_ctx(&ctx)
}

fn component_build_ctx(ctx: &ApplicationContext) -> anyhow::Result<()> {
    log_action("Executing", "component-build steps");
    let _indent = LogIndent::new();

    let components_with_build_steps = ctx
        .application
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
    let _indent = LogIndent::new();

    for component in components_with_build_steps {
        log_action(
            "Building",
            format!(
                "component {} in directory {}",
                component.name.log_color_highlight(),
                component.source_dir().log_color_highlight(),
            ),
        );
        let _indent = LogIndent::new();

        for build_step in &component.build_steps {
            let build_dir = build_step
                .dir
                .as_ref()
                .map(|dir| component.source_dir().join(dir))
                .unwrap_or_else(|| component.source_dir().to_path_buf());

            if !build_step.inputs.is_empty() && !build_step.outputs.is_empty() {
                let inputs = compile_and_collect_globs(&build_dir, &build_step.inputs)?;
                let outputs = compile_and_collect_globs(&build_dir, &build_step.outputs)?;

                if is_up_to_date(ctx.config.skip_up_to_date_checks, || inputs, || outputs) {
                    log_skipping_up_to_date(format!(
                        "executing command: {}",
                        build_step.command.log_color_highlight()
                    ));
                    continue;
                }
            }

            log_action(
                "Executing",
                format!("command: {}", build_step.command.log_color_highlight()),
            );

            let command_tokens = build_step.command.split(' ').collect::<Vec<_>>();
            if command_tokens.is_empty() {
                return Err(anyhow!("Empty command!"));
            }

            let result = Command::new(command_tokens[0])
                .args(command_tokens.iter().skip(1))
                .current_dir(build_dir)
                .status()
                .with_context(|| "Failed to execute command".to_string())?;

            if !result.success() {
                return Err(anyhow!(format!(
                    "Command failed with exit code: {}",
                    result
                        .code()
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "?".to_string())
                )));
            }
        }
    }

    Ok(())
}

pub async fn post_component_build(config: Config) -> anyhow::Result<()> {
    let ctx = create_context(config)?;
    post_component_build_ctx(&ctx).await
}

async fn post_component_build_ctx(ctx: &ApplicationContext) -> anyhow::Result<()> {
    log_action("Executing", "post-component-build steps");
    let _indent = LogIndent::new();

    for (component_name, component) in &ctx.application.wasm_components_by_name {
        let input_wasm = ctx.application.component_input_wasm(component_name);
        let output_wasm = ctx.application.component_output_wasm(component_name);

        if is_up_to_date(
            ctx.config.skip_up_to_date_checks,
            // We also include the component specification source,
            // so it triggers build in case deps are changed
            || [component.source.clone(), input_wasm.clone()],
            || [output_wasm.clone()],
        ) {
            log_skipping_up_to_date(format!(
                "composing wasm rpc dependencies ({}) into {}",
                component
                    .wasm_rpc_dependencies
                    .iter()
                    .map(|s| s.log_color_highlight())
                    .join(", "),
                component_name.log_color_highlight(),
            ));
            continue;
        }

        if component.wasm_rpc_dependencies.is_empty() {
            log_action(
                "Copying",
                format!(
                    "(without composing) {} to {}, no wasm rpc dependencies defined",
                    input_wasm.log_color_highlight(),
                    output_wasm.log_color_highlight(),
                ),
            );
            copy(&input_wasm, &output_wasm)?;
        } else {
            log_action(
                "Composing",
                format!(
                    "wasm rpc dependencies ({}) into {}",
                    component
                        .wasm_rpc_dependencies
                        .iter()
                        .map(|s| s.log_color_highlight())
                        .join(", "),
                    component_name.log_color_highlight(),
                ),
            );

            let stub_wasms = component
                .wasm_rpc_dependencies
                .iter()
                .map(|dep| ctx.application.stub_wasm(dep))
                .collect::<Vec<_>>();

            commands::composition::compose(
                ctx.application
                    .component_input_wasm(component_name)
                    .as_path(),
                &stub_wasms,
                ctx.application
                    .component_output_wasm(component_name)
                    .as_path(),
            )
            .await?;
        }
    }

    Ok(())
}

pub async fn build(config: Config) -> anyhow::Result<()> {
    let mut ctx = create_context(config)?;

    pre_component_build_ctx(&mut ctx).await?;
    component_build_ctx(&ctx)?;
    post_component_build_ctx(&ctx).await?;

    Ok(())
}

fn create_context(config: Config) -> anyhow::Result<ApplicationContext> {
    to_anyhow(
        "Failed to create application context, see problems above",
        load_app_validated(&config).and_then(|application| {
            ResolvedWitApplication::new(&application).map(|wit| ApplicationContext {
                config,
                application,
                wit,
            })
        }),
    )
}

fn update_wit_context(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    to_anyhow(
        "Failed to update application wit context, see problems above",
        ResolvedWitApplication::new(&ctx.application).map(|wit| {
            ctx.wit = wit;
        }),
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
    let _indent = LogIndent::new();

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

fn collect_sources(mode: &ApplicationSourceMode) -> ValidatedResult<Vec<PathBuf>> {
    log_action("Collecting", "sources");
    let _indent = LogIndent::new();

    let sources = match mode {
        ApplicationSourceMode::Automatic => match find_main_source() {
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
                                    source.display(),
                                    err
                                )
                            })
                            .and_then(|matches| {
                                matches.collect::<Result<Vec<_>, _>>().map_err(|err| {
                                    format!(
                                        "Failed to resolve glob pattern: {}, source: {}, error: {}",
                                        pattern,
                                        source.display(),
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
        ApplicationSourceMode::Explicit(sources) => {
            let non_unique_source_warns: Vec<_> = sources
                .iter()
                .counts()
                .into_iter()
                .filter(|(_, count)| *count > 1)
                .map(|(source, count)| {
                    format!(
                        "Source added multiple times, source: {}, count: {}",
                        source.display(),
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
                sources.iter().map(|source| source.display()).join(", ")
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

fn to_anyhow<T>(message: &str, result: ValidatedResult<T>) -> anyhow::Result<T> {
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

            Err(anyhow!(message.to_string()))
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
        return false;
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

fn compile_and_collect_globs(root_dir: &Path, globs: &[String]) -> Result<Vec<PathBuf>, Error> {
    globs
        .iter()
        .map(|pattern| {
            glob(&format!("{}/{}", root_dir.to_string_lossy(), pattern))
                .with_context(|| format!("Failed to compile glob expression: {}", pattern))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow!(err))
        .and_then(|paths| {
            paths
                .into_iter()
                .flatten()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| anyhow!(err))
        })
}
