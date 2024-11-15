use crate::cargo::regenerate_cargo_package_component;
use crate::fs;
use crate::fs::PathExtra;
use crate::log::{
    log_action, log_skipping_up_to_date, log_validated_action_result, log_warn_action, LogColorize,
    LogIndent,
};
use crate::model::oam;
use crate::model::wasm_rpc::{
    include_glob_patter_from_yaml_file, init_oam_app, Application, ComponentName,
    DEFAULT_CONFIG_FILE_NAME,
};
use crate::stub::{StubConfig, StubDefinition};
use crate::validation::ValidatedResult;
use crate::wit_generate::{
    add_stub_as_dependency_to_wit_dir, extract_main_interface_as_wit_dep, AddStubAsDepConfig,
    UpdateCargoToml,
};
use crate::wit_resolve::{ResolvedWitApplication, WitDepsResolver};
use crate::{commands, naming, WasmRpcOverride};
use anyhow::{anyhow, bail, Context, Error};
use colored::Colorize;
use glob::glob;
use itertools::Itertools;
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
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
    common_wit_deps: OnceCell<anyhow::Result<WitDepsResolver>>,
    component_base_output_wit_deps: HashMap<ComponentName, WitDepsResolver>,
}

impl ApplicationContext {
    fn new(config: Config) -> anyhow::Result<ApplicationContext> {
        to_anyhow(
            "Failed to create application context, see problems above",
            load_app_validated(&config).and_then(|application| {
                ResolvedWitApplication::new(&application).map(|wit| ApplicationContext {
                    config,
                    application,
                    wit,
                    common_wit_deps: OnceCell::new(),
                    component_base_output_wit_deps: HashMap::new(),
                })
            }),
        )
    }

    fn update_wit_context(&mut self) -> anyhow::Result<()> {
        to_anyhow(
            "Failed to update application wit context, see problems above",
            ResolvedWitApplication::new(&self.application).map(|wit| {
                self.wit = wit;
            }),
        )
    }

    fn common_wit_deps(&self) -> anyhow::Result<&WitDepsResolver> {
        match self
            .common_wit_deps
            .get_or_init(|| {
                let sources = self.application.wit_deps();
                if sources.is_empty() {
                    bail!("No common witDeps were defined in the application manifest")
                }
                WitDepsResolver::new(sources)
            })
            .as_ref()
        {
            Ok(wit_deps) => Ok(wit_deps),
            Err(err) => Err(anyhow!("Failed to init wit dependency resolver: {}", err)),
        }
    }

    fn component_base_output_wit_deps(
        &mut self,
        component_name: &ComponentName,
    ) -> anyhow::Result<&WitDepsResolver> {
        // Not using the entry API, so we can skip copying the component name
        if !self
            .component_base_output_wit_deps
            .contains_key(component_name)
        {
            self.component_base_output_wit_deps.insert(
                component_name.clone(),
                WitDepsResolver::new(vec![self
                    .application
                    .component_base_output_wit(component_name)
                    .join(naming::wit::DEPS_DIR)])?,
            );
        }
        Ok(self
            .component_base_output_wit_deps
            .get(component_name)
            .unwrap())
    }
}

pub fn init(component_name: ComponentName) -> anyhow::Result<()> {
    fs::write_str(
        DEFAULT_CONFIG_FILE_NAME,
        init_oam_app(component_name).to_yaml_string(),
    )
    .context("Failed to init component application manifest")
}

pub async fn pre_component_build(config: Config) -> anyhow::Result<()> {
    let mut ctx = ApplicationContext::new(config)?;
    pre_component_build_ctx(&mut ctx).await
}

async fn pre_component_build_ctx(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    log_action("Executing", "pre-component-build steps");
    let _indent = LogIndent::new();

    {
        for component_name in ctx.wit.component_order_cloned() {
            create_base_output_wit(ctx, &component_name)?;
        }
        for component_name in &ctx.application.all_wasm_rpc_dependencies() {
            build_stub(ctx, component_name).await?;
        }
    }

    {
        let mut any_changed = false;
        for component_name in ctx.application.wasm_components_by_name.keys() {
            let changed = create_output_wit(ctx, component_name)?;
            if changed {
                update_cargo_toml(ctx, component_name)?;
            }
            any_changed |= changed;
        }
        if any_changed {
            ctx.update_wit_context()?;
        }
    }

    Ok(())
}

pub fn component_build(config: Config) -> anyhow::Result<()> {
    let ctx = ApplicationContext::new(config)?;
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
    let ctx = ApplicationContext::new(config)?;
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
            fs::copy(&input_wasm, &output_wasm)?;
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
            let _indent = LogIndent::new();

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
    let mut ctx = ApplicationContext::new(config)?;

    pre_component_build_ctx(&mut ctx).await?;
    component_build_ctx(&ctx)?;
    post_component_build_ctx(&ctx).await?;

    Ok(())
}

pub fn clean(config: Config) -> anyhow::Result<()> {
    let app = to_anyhow(
        "Failed to load application manifest(s), see problems above",
        load_app_validated(&config),
    )?;

    {
        log_action("Cleaning", "components");
        let _indent = LogIndent::new();

        for (component_name, component) in &app.wasm_components_by_name {
            log_action(
                "Cleaning",
                format!("component {}", component_name.log_color_highlight()),
            );
            let _indent = LogIndent::new();

            delete_path("wit output dir", &app.component_output_wit(component_name))?;
            delete_path("wasm input", &app.component_input_wasm(component_name))?;
            delete_path("wasm output", &app.component_output_wasm(component_name))?;

            for build_step in &component.build_steps {
                let build_dir = build_step
                    .dir
                    .as_ref()
                    .map(|dir| component.source_dir().join(dir))
                    .unwrap_or_else(|| component.source_dir().to_path_buf());

                let outputs = compile_and_collect_globs(&build_dir, &build_step.outputs)?;
                for output in outputs {
                    delete_path("build step output", &output)?;
                }
            }
        }
    }

    {
        log_action("Cleaning", "component stubs");
        let _indent = LogIndent::new();

        for component_name in app.all_wasm_rpc_dependencies() {
            log_action(
                "Cleaning",
                format!(
                    "component stub {}",
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            delete_path("stub wit", &app.stub_wit(&component_name))?;
            delete_path("stub wasm", &app.stub_wasm(&component_name))?;
        }
    }

    {
        log_action("Cleaning", "application build dir");
        let _indent = LogIndent::new();

        delete_path("application build dir", &app.build_dir())?;
    }

    Ok(())
}

fn delete_path(context: &str, path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        log_warn_action(
            "Deleting",
            format!("{} {}", context, path.log_color_highlight()),
        );
        fs::remove(path)
            .with_context(|| anyhow!("Failed to delete {}, path: {}", context, path.display()))?;
    }
    Ok(())
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
                app.wasm_components_by_name
                    .keys()
                    .map(|s| s.log_color_highlight())
                    .join(", ")
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
                std::env::set_current_dir(PathExtra::new(&source).parent().unwrap())
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
                sources
                    .iter()
                    .map(|source| source.log_color_highlight())
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

        if let Ok(metadata) = fs::metadata(path) {
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

fn create_base_output_wit(
    ctx: &mut ApplicationContext,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let component_input_wit = ctx.application.component_input_wit(component_name);
    let component_base_output_wit = ctx.application.component_base_output_wit(component_name);
    let gen_dir_done_marker = GeneratedDirDoneMarker::new(&component_base_output_wit);

    if is_up_to_date(
        ctx.config.skip_up_to_date_checks
            || !gen_dir_done_marker.is_done()
            || !ctx.wit.is_dep_graph_up_to_date(component_name)?,
        || [component_input_wit.clone()],
        || [component_base_output_wit.clone()],
    ) {
        log_skipping_up_to_date(format!(
            "creating base output wit directory for {}",
            component_name.log_color_highlight()
        ));
        Ok(false)
    } else {
        log_action(
            "Creating",
            format!(
                "base output wit directory for {}",
                component_name.log_color_highlight(),
            ),
        );
        let _indent = LogIndent::new();

        delete_path("base output wit directory", &component_base_output_wit)?;
        copy_wit_sources(&component_input_wit, &component_base_output_wit)?;

        {
            let missing_package_deps =
                ctx.wit.missing_generic_input_package_deps(component_name)?;

            if !missing_package_deps.is_empty() {
                log_action("Adding", "package deps");
                let _indent = LogIndent::new();

                ctx.common_wit_deps()?
                    .add_packages_with_transitive_deps_to_wit_dir(
                        &missing_package_deps,
                        &component_base_output_wit,
                    )?;
            }
        }

        {
            let component_interface_package_deps =
                ctx.wit.component_interface_package_deps(component_name)?;
            if !component_interface_package_deps.is_empty() {
                log_action("Adding", "component interface package dependencies");
                let _indent = LogIndent::new();

                for (dep_interface_package_name, dep_component_name) in
                    &component_interface_package_deps
                {
                    ctx.component_base_output_wit_deps(dep_component_name)?
                        .add_packages_with_transitive_deps_to_wit_dir(
                            &[dep_interface_package_name.clone()],
                            &component_base_output_wit,
                        )?;
                }
            }
        }

        {
            log_action(
                "Extracting",
                format!(
                    "main interface package from {} to {}",
                    component_input_wit.log_color_highlight(),
                    component_base_output_wit.log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            extract_main_interface_as_wit_dep(&component_base_output_wit)?;
        }

        gen_dir_done_marker.mark_as_done()?;

        Ok(true)
    }
}

fn create_output_wit(
    ctx: &ApplicationContext,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let component_base_output_wit = ctx.application.component_base_output_wit(component_name);
    let component_output_wit = ctx.application.component_output_wit(component_name);
    let gen_dir_done_marker = GeneratedDirDoneMarker::new(&component_output_wit);

    if is_up_to_date(
        ctx.config.skip_up_to_date_checks
            || !gen_dir_done_marker.is_done()
            || !ctx.wit.is_dep_graph_up_to_date(component_name)?,
        || [component_base_output_wit.clone()],
        || [component_output_wit.clone()],
    ) {
        log_skipping_up_to_date(format!(
            "creating output wit directory for {}",
            component_name.log_color_highlight()
        ));
        Ok(false)
    } else {
        log_action(
            "Creating",
            format!(
                "output wit directory for {}",
                component_name.log_color_highlight(),
            ),
        );
        let _indent = LogIndent::new();

        delete_path("output wit directory", &component_output_wit)?;
        copy_wit_sources(&component_base_output_wit, &component_output_wit)?;
        add_stub_deps(ctx, component_name)?;

        gen_dir_done_marker.mark_as_done()?;

        Ok(true)
    }
}

fn update_cargo_toml(
    ctx: &ApplicationContext,
    component_name: &ComponentName,
) -> anyhow::Result<()> {
    let component_input_wit = PathExtra::new(ctx.application.component_input_wit(component_name));
    let component_input_wit_parent = component_input_wit
        .parent()
        .with_context(|| anyhow!("Failed to get parent for component {}", component_name))?;
    let cargo_toml = component_input_wit_parent.join("Cargo.toml");

    if cargo_toml.exists() {
        regenerate_cargo_package_component(
            &cargo_toml,
            &ctx.application.component_output_wit(component_name),
            ctx.application.stub_world(component_name),
        )?
    }

    Ok(())
}

async fn build_stub(
    ctx: &ApplicationContext,
    component_name: &ComponentName,
) -> anyhow::Result<bool> {
    let target_root = ctx.application.stub_temp_build_dir(component_name);

    let stub_def = StubDefinition::new(StubConfig {
        source_wit_root: ctx.application.component_base_output_wit(component_name),
        target_root: target_root.clone(),
        selected_world: ctx.application.stub_world(component_name),
        stub_crate_version: ctx.application.stub_crate_version(component_name),
        wasm_rpc_override: WasmRpcOverride {
            wasm_rpc_path_override: ctx.application.stub_wasm_rpc_path(component_name),
            wasm_rpc_version_override: ctx.application.stub_wasm_rpc_version(component_name),
        },
        extract_source_interface_package: false,
        seal_cargo_workspace: true,
    })
    .context("Failed to gather information for the stub generator")?;

    let stub_dep_package_ids = stub_def.stub_dep_package_ids();
    let stub_inputs: Vec<PathBuf> = stub_def
        .packages_with_wit_sources()
        .flat_map(|(package_id, _, sources)| {
            (stub_dep_package_ids.contains(&package_id) || package_id == stub_def.source_package_id)
                .then(|| sources.files.iter().cloned())
                .unwrap_or_default()
        })
        .collect();

    let stub_wasm = ctx.application.stub_wasm(component_name);
    let stub_wit = ctx.application.stub_wit(component_name);
    let gen_dir_done_marker = GeneratedDirDoneMarker::new(&stub_wit);

    if is_up_to_date(
        ctx.config.skip_up_to_date_checks || !gen_dir_done_marker.is_done(),
        || stub_inputs,
        || [stub_wit.clone(), stub_wasm.clone()],
    ) {
        log_skipping_up_to_date(format!(
            "building wasm rpc stub for {}",
            component_name.log_color_highlight()
        ));
        Ok(false)
    } else {
        log_action(
            "Building",
            format!("wasm rpc stub for {}", component_name.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        delete_path("stub temp build dir", &target_root)?;
        delete_path("stub wit", &stub_wit)?;
        delete_path("stub wasm", &stub_wasm)?;

        log_action(
            "Creating",
            format!("stub temp build dir {}", target_root.log_color_highlight()),
        );
        fs::create_dir_all(&target_root)?;

        commands::generate::build(&stub_def, &stub_wasm, &stub_wit).await?;

        gen_dir_done_marker.mark_as_done()?;

        delete_path("stub temp build dir", &target_root)?;

        Ok(true)
    }
}

fn add_stub_deps(ctx: &ApplicationContext, component_name: &ComponentName) -> Result<bool, Error> {
    let component = ctx.application.component(component_name);
    if component.wasm_rpc_dependencies.is_empty() {
        Ok(false)
    } else {
        log_action(
            "Adding",
            format!(
                "stub wit dependencies to {}",
                component_name.log_color_highlight()
            ),
        );

        let _indent = LogIndent::new();

        for dep_component_name in &component.wasm_rpc_dependencies {
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
                update_cargo_toml: UpdateCargoToml::NoUpdate,
            })?
        }

        Ok(true)
    }
}

fn copy_wit_sources(source: &Path, target: &Path) -> anyhow::Result<()> {
    log_action(
        "Copying",
        format!(
            "wit sources from {} to {}",
            source.log_color_highlight(),
            target.log_color_highlight()
        ),
    );
    let _indent = LogIndent::new();

    let dir_content = fs_extra::dir::get_dir_content(source).with_context(|| {
        anyhow!(
            "Failed to read component input wit directory entries for {}",
            source.display()
        )
    })?;

    for file in dir_content.files {
        let from = PathBuf::from(&file);
        let to = target.join(
            from.strip_prefix(source)
                .with_context(|| anyhow!("Failed to strip prefix for source {}", &file))?,
        );

        log_action(
            "Copying",
            format!(
                "wit source {} to {}",
                from.log_color_highlight(),
                to.log_color_highlight()
            ),
        );
        fs::copy(from, to)?;
    }

    Ok(())
}

static GENERATED_DIR_DONE_MARKER_FILE_NAME: &str = &".done";

struct GeneratedDirDoneMarker<'a> {
    dir: &'a Path,
}

impl<'a> GeneratedDirDoneMarker<'a> {
    fn new(dir: &'a Path) -> Self {
        Self { dir }
    }

    fn is_done(&self) -> bool {
        self.dir.join(GENERATED_DIR_DONE_MARKER_FILE_NAME).exists()
    }

    fn mark_as_done(&self) -> anyhow::Result<()> {
        fs::write_str(self.dir.join(GENERATED_DIR_DONE_MARKER_FILE_NAME), "")
    }
}
