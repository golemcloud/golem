use crate::cargo::regenerate_cargo_package_component;
use crate::fs;
use crate::fs::PathExtra;
use crate::log::{
    log_action, log_skipping_up_to_date, log_warn_action, set_log_output, LogColorize, LogIndent,
    Output,
};
use crate::model::app::{
    includes_from_yaml_file, AppBuildStep, Application, ComponentName,
    ComponentPropertiesExtensions, DependencyType, DependentComponent, ProfileName,
    DEFAULT_CONFIG_FILE_NAME,
};
use crate::model::app_raw;
use crate::stub::{StubConfig, StubDefinition};
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::wit_generate::{
    add_client_as_dependency_to_wit_dir, extract_exports_as_wit_dep, AddClientAsDepConfig,
    UpdateCargoToml,
};
use crate::wit_resolve::{ResolvedWitApplication, WitDepsResolver};
use crate::{commands, naming, WasmRpcOverride};
use anyhow::{anyhow, bail, Context, Error};
use colored::control::SHOULD_COLORIZE;
use colored::Colorize;
use glob::{glob_with, MatchOptions};
use golem_wasm_rpc::WASM_RPC_VERSION;
use itertools::Itertools;
use serde::Serialize;
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use walkdir::WalkDir;

pub struct Config<CPE: ComponentPropertiesExtensions> {
    pub app_source_mode: ApplicationSourceMode,
    pub component_select_mode: ComponentSelectMode,
    pub skip_up_to_date_checks: bool,
    pub profile: Option<ProfileName>,
    pub offline: bool,
    pub extensions: PhantomData<CPE>,
    pub log_output: Output,
    pub steps_filter: HashSet<AppBuildStep>,
}

impl<CPE: ComponentPropertiesExtensions> Config<CPE> {
    pub fn should_run_step(&self, step: AppBuildStep) -> bool {
        if self.steps_filter.is_empty() {
            true
        } else {
            self.steps_filter.contains(&step)
        }
    }
}

#[derive(Debug, Clone)]
pub enum ApplicationSourceMode {
    Automatic,
    Explicit(Vec<PathBuf>),
}

#[derive(Debug, Clone)]
pub enum ComponentSelectMode {
    CurrentDir,
    Explicit(Vec<ComponentName>),
}

#[derive(Debug)]
pub struct ComponentStubInterfaces {
    pub stub_interface_name: String,
    pub exported_interfaces_per_stub_resource: BTreeMap<String, String>,
}

pub struct ApplicationContext<CPE: ComponentPropertiesExtensions> {
    pub config: Config<CPE>,
    pub application: Application<CPE>,
    pub wit: ResolvedWitApplication,
    component_stub_defs: HashMap<ComponentName, StubDefinition>,
    common_wit_deps: OnceCell<anyhow::Result<WitDepsResolver>>,
    component_generated_base_wit_deps: HashMap<ComponentName, WitDepsResolver>,
    selected_component_names: BTreeSet<ComponentName>,
}

impl<CPE: ComponentPropertiesExtensions> ApplicationContext<CPE> {
    pub fn new(config: Config<CPE>) -> anyhow::Result<ApplicationContext<CPE>> {
        set_log_output(config.log_output);

        let ctx = to_anyhow(
            config.log_output,
            "Failed to create application context, see problems above",
            load_app(&config).and_then(|(application, selected_component_names)| {
                ResolvedWitApplication::new(&application, config.profile.as_ref()).map(|wit| {
                    let selected_component_names = if selected_component_names.is_empty() {
                        application.component_names().cloned().collect()
                    } else {
                        selected_component_names
                    };
                    ApplicationContext {
                        config,
                        application,
                        wit,
                        component_stub_defs: HashMap::new(),
                        common_wit_deps: OnceCell::new(),
                        component_generated_base_wit_deps: HashMap::new(),
                        selected_component_names,
                    }
                })
            }),
        )?;

        select_and_validate_profiles(&ctx)?;

        if ctx.config.offline {
            log_action("Selected", "offline mode");
        }

        Ok(ctx)
    }

    fn profile(&self) -> Option<&ProfileName> {
        self.config.profile.as_ref()
    }

    fn update_wit_context(&mut self) -> anyhow::Result<()> {
        to_anyhow(
            self.config.log_output,
            "Failed to update application wit context, see problems above",
            ResolvedWitApplication::new(&self.application, self.profile()).map(|wit| {
                self.wit = wit;
            }),
        )
    }

    fn component_stub_def(
        &mut self,
        component_name: &ComponentName,
    ) -> anyhow::Result<&StubDefinition> {
        if !self.component_stub_defs.contains_key(component_name) {
            self.component_stub_defs.insert(
                component_name.clone(),
                StubDefinition::new(StubConfig {
                    source_wit_root: self
                        .application
                        .component_generated_base_wit(component_name),
                    client_root: self.application.client_temp_build_dir(component_name),
                    selected_world: None,
                    stub_crate_version: WASM_RPC_VERSION.to_string(),
                    // NOTE: these overrides are deliberately not part of cli flags or the app manifest, at least for now
                    wasm_rpc_override: WasmRpcOverride {
                        wasm_rpc_path_override: std::env::var("WASM_RPC_PATH_OVERRIDE").ok(),
                        wasm_rpc_version_override: std::env::var("WASM_RPC_VERSION_OVERRIDE").ok(),
                    },
                    extract_source_exports_package: false,
                    seal_cargo_workspace: true,
                })
                .context("Failed to gather information for the stub generator")?,
            );
        }
        Ok(self.component_stub_defs.get(component_name).unwrap())
    }

    pub fn component_stub_interfaces(
        &mut self,
        component_name: &ComponentName,
    ) -> anyhow::Result<ComponentStubInterfaces> {
        let stub_def = self.component_stub_def(component_name)?;
        let client_package_name = stub_def.client_parser_package_name();
        let result = ComponentStubInterfaces {
            stub_interface_name: client_package_name
                .interface_id(&stub_def.client_interface_name()),
            exported_interfaces_per_stub_resource: BTreeMap::from_iter(
                stub_def
                    .stub_imported_interfaces()
                    .iter()
                    .filter_map(|interface| {
                        interface
                            .owner_interface
                            .clone()
                            .map(|owner| (interface.name.clone(), owner))
                    }),
            ),
        };
        Ok(result)
    }

    fn common_wit_deps(&self) -> anyhow::Result<&WitDepsResolver> {
        match self
            .common_wit_deps
            .get_or_init(|| {
                let sources = self.application.wit_deps();
                if sources.value.is_empty() {
                    bail!("No common witDeps were defined in the application manifest")
                }
                WitDepsResolver::new(
                    sources
                        .value
                        .iter()
                        .cloned()
                        .map(|path| sources.source.join(path))
                        .collect(),
                )
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
            .component_generated_base_wit_deps
            .contains_key(component_name)
        {
            self.component_generated_base_wit_deps.insert(
                component_name.clone(),
                WitDepsResolver::new(vec![self
                    .application
                    .component_generated_base_wit(component_name)
                    .join(naming::wit::DEPS_DIR)])?,
            );
        }
        Ok(self
            .component_generated_base_wit_deps
            .get(component_name)
            .unwrap())
    }

    pub fn selected_component_names(&self) -> &BTreeSet<ComponentName> {
        &self.selected_component_names
    }
}

// TODO: this step is not selected_component_names aware yet, for that we have to build / filter
//         - based on wit deps and / or
//         - based on rpc deps
//       depending on the sub-step
async fn gen_rpc<CPE: ComponentPropertiesExtensions>(
    ctx: &mut ApplicationContext<CPE>,
) -> anyhow::Result<()> {
    log_action("Generating", "RPC artifacts");
    let _indent = LogIndent::new();

    {
        for component_name in ctx.wit.component_order_cloned() {
            create_generated_base_wit(ctx, &component_name)?;
        }

        for dep in &ctx.application.all_wasm_rpc_dependencies() {
            build_client(ctx, dep).await?;
        }
    }

    {
        let mut any_changed = false;
        for component_name in ctx.application.component_names() {
            let changed = create_generated_wit(ctx, component_name)?;
            update_cargo_toml(ctx, changed, component_name)?;
            any_changed |= changed;
        }
        if any_changed {
            ctx.update_wit_context()?;
        }
    }

    Ok(())
}

fn componentize<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
) -> anyhow::Result<()> {
    log_action("Building", "components");
    let _indent = LogIndent::new();

    for component_name in ctx.selected_component_names() {
        let component_properties = ctx
            .application
            .component_properties(component_name, ctx.profile());

        if component_properties.build.is_empty() {
            log_warn_action(
                "Skipping",
                format!(
                    "building {}, no build steps",
                    component_name.as_str().log_color_highlight(),
                ),
            );
            continue;
        }

        log_action(
            "Building",
            format!("{}", component_name.as_str().log_color_highlight()),
        );
        let _indent = LogIndent::new();

        for build_step in &component_properties.build {
            execute_external_command(
                ctx,
                ctx.application.component_source_dir(component_name),
                build_step,
            )?;
        }
    }

    Ok(())
}

async fn link_rpc<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
) -> anyhow::Result<()> {
    log_action("Linking", "RPC");
    let _indent = LogIndent::new();

    for component_name in ctx.selected_component_names() {
        let static_dependencies = ctx
            .application
            .component_wasm_rpc_dependencies(component_name)
            .iter()
            .filter(|dep| dep.dep_type == DependencyType::StaticWasmRpc)
            .collect::<BTreeSet<_>>();
        let dynamic_dependencies = ctx
            .application
            .component_wasm_rpc_dependencies(component_name)
            .iter()
            .filter(|dep| dep.dep_type == DependencyType::DynamicWasmRpc)
            .collect::<BTreeSet<_>>();
        let client_wasms = static_dependencies
            .iter()
            .map(|dep| ctx.application.client_wasm(&dep.name))
            .collect::<Vec<_>>();
        let component_wasm = ctx
            .application
            .component_wasm(component_name, ctx.profile());
        let linked_wasm = ctx
            .application
            .component_linked_wasm(component_name, ctx.profile());

        let task_result_marker = TaskResultMarker::new(
            &ctx.application.task_result_marker_dir(),
            LinkRpcMarkerHash {
                component_name,
                dependencies: &static_dependencies,
            },
        )?;

        if !dynamic_dependencies.is_empty() {
            log_action(
                "Found",
                format!(
                    "dynamic WASM RPC dependencies ({}) for {}",
                    dynamic_dependencies
                        .iter()
                        .map(|s| s.name.as_str().log_color_highlight())
                        .join(", "),
                    component_name.as_str().log_color_highlight(),
                ),
            );
        }

        if !static_dependencies.is_empty() {
            log_action(
                "Found",
                format!(
                    "static WASM RPC dependencies ({}) for {}",
                    static_dependencies
                        .iter()
                        .map(|s| s.name.as_str().log_color_highlight())
                        .join(", "),
                    component_name.as_str().log_color_highlight(),
                ),
            );
        }

        if is_up_to_date(
            ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date(),
            || {
                let mut inputs = client_wasms.clone();
                inputs.push(component_wasm.clone());
                inputs
            },
            || [linked_wasm.clone()],
        ) {
            log_skipping_up_to_date(format!(
                "linking RPC for {}",
                component_name.as_str().log_color_highlight(),
            ));
            continue;
        }

        task_result_marker.result(
            async {
                if static_dependencies.is_empty() {
                    log_action(
                        "Copying",
                        format!(
                            "{} without linking, no static WASM RPC dependencies were found",
                            component_name.as_str().log_color_highlight(),
                        ),
                    );
                    fs::copy(&component_wasm, &linked_wasm).map(|_| ())
                } else {
                    log_action(
                        "Linking",
                        format!(
                            "static WASM RPC dependencies ({}) into {}",
                            static_dependencies
                                .iter()
                                .map(|s| s.name.as_str().log_color_highlight())
                                .join(", "),
                            component_name.as_str().log_color_highlight(),
                        ),
                    );
                    let _indent = LogIndent::new();

                    commands::composition::compose(
                        ctx.application
                            .component_wasm(component_name, ctx.profile())
                            .as_path(),
                        &client_wasms,
                        ctx.application
                            .component_linked_wasm(component_name, ctx.profile())
                            .as_path(),
                    )
                    .await
                }
            }
            .await,
        )?;
    }

    Ok(())
}

pub async fn build<CPE: ComponentPropertiesExtensions>(config: Config<CPE>) -> anyhow::Result<()> {
    let mut ctx = ApplicationContext::<CPE>::new(config)?;

    if ctx.config.should_run_step(AppBuildStep::GenRpc) {
        gen_rpc(&mut ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::Componentize) {
        componentize(&ctx)?;
    }
    if ctx.config.should_run_step(AppBuildStep::LinkRpc) {
        link_rpc(&ctx).await?;
    }

    Ok(())
}

// TODO: clean is not selected_component_names aware yet
pub fn clean<CPE: ComponentPropertiesExtensions>(config: Config<CPE>) -> anyhow::Result<()> {
    let (app, _selected_component_names) = to_anyhow(
        config.log_output,
        "Failed to load application manifest(s), see problems above",
        load_app(&config),
    )?;

    {
        log_action("Cleaning", "components");
        let _indent = LogIndent::new();

        let all_profiles = app.all_option_profiles();
        let paths = {
            let mut paths = BTreeSet::<(&'static str, PathBuf)>::new();
            for component_name in app.component_names() {
                for profile in &all_profiles {
                    paths.insert((
                        "generated wit",
                        app.component_generated_wit(component_name, profile.as_ref()),
                    ));
                    paths.insert((
                        "component wasm",
                        app.component_wasm(component_name, profile.as_ref()),
                    ));
                    paths.insert((
                        "linked wasm",
                        app.component_linked_wasm(component_name, profile.as_ref()),
                    ));

                    let properties = &app.component_properties(component_name, profile.as_ref());

                    for build_step in &properties.build {
                        let build_dir = build_step
                            .dir
                            .as_ref()
                            .map(|dir| app.component_source_dir(component_name).join(dir))
                            .unwrap_or_else(|| {
                                app.component_source_dir(component_name).to_path_buf()
                            });

                        paths.extend(
                            compile_and_collect_globs(&build_dir, &build_step.targets)?
                                .into_iter()
                                .map(|path| ("build output", path)),
                        );
                    }

                    paths.extend(properties.clean.iter().map(|path| {
                        (
                            "clean target",
                            app.component_source_dir(component_name).join(path),
                        )
                    }));
                }
            }
            paths
        };

        for (context, path) in paths {
            delete_path(context, &path)?;
        }
    }

    {
        log_action("Cleaning", "component clients");
        let _indent = LogIndent::new();

        for dep in app.all_wasm_rpc_dependencies() {
            log_action(
                "Cleaning",
                format!(
                    "component client {}",
                    dep.name.as_str().log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            delete_path("client wit", &app.client_wit(&dep.name))?;
            if dep.dep_type == DependencyType::StaticWasmRpc {
                delete_path("client wasm", &app.client_wasm(&dep.name))?;
            }
        }
    }

    {
        log_action("Cleaning", "common clean targets");
        let _indent = LogIndent::new();

        for clean in app.common_clean() {
            delete_path("common clean target", &clean.source.join(&clean.value))?;
        }
    }

    {
        log_action("Cleaning", "application build dir");
        let _indent = LogIndent::new();

        delete_path("temp dir", &app.temp_dir())?;
    }

    Ok(())
}

pub fn print_dynamic_help<CPE: ComponentPropertiesExtensions>(
    config: Config<CPE>,
) -> anyhow::Result<()> {
    static LABEL_SOURCE: &str = "Source";
    static LABEL_SELECTED: &str = "Selected";
    static LABEL_TEMPLATE: &str = "Template";
    static LABEL_PROFILES: &str = "Profiles";
    static LABEL_DEPENDENCIES: &str = "Dependencies";

    let label_padding = {
        [
            &LABEL_SOURCE,
            &LABEL_SELECTED,
            &LABEL_TEMPLATE,
            &LABEL_PROFILES,
            &LABEL_DEPENDENCIES,
        ]
        .map(|label| label.len())
        .into_iter()
        .max()
        .unwrap_or(0)
            + 1
    };

    let print_field = |label: &'static str, value: String| {
        println!("    {:<label_padding$} {}", format!("{}:", label), value)
    };

    let ctx = ApplicationContext::new(config)?;
    let should_colorize = SHOULD_COLORIZE.should_colorize();

    if ctx.application.has_any_component() {
        println!("{}", "Components:".log_color_help_group());
        for component_name in ctx.application.component_names() {
            let selected = ctx.selected_component_names.contains(component_name);
            let effective_property_source = ctx
                .application
                .component_effective_property_source(component_name, ctx.profile());
            println!("  {}", component_name.as_str().bold());
            print_field(
                LABEL_SELECTED,
                if selected {
                    "yes".green().bold().to_string()
                } else {
                    "no".red().bold().to_string()
                },
            );
            print_field(
                LABEL_SOURCE,
                ctx.application
                    .component_source(component_name)
                    .to_string_lossy()
                    .underline()
                    .to_string(),
            );
            if let Some(template_name) = effective_property_source.template_name {
                print_field(LABEL_TEMPLATE, template_name.as_str().bold().to_string());
            }
            if let Some(selected_profile) = effective_property_source.profile {
                print_field(
                    LABEL_PROFILES,
                    ctx.application
                        .component_profiles(component_name)
                        .iter()
                        .map(|profile| {
                            if selected_profile == profile {
                                if should_colorize {
                                    profile.as_str().bold().underline().to_string()
                                } else {
                                    format!("*{}", profile.as_str())
                                }
                            } else {
                                profile.to_string()
                            }
                        })
                        .join(", "),
                );
            }
            let dependencies = ctx
                .application
                .component_wasm_rpc_dependencies(component_name);
            if !dependencies.is_empty() {
                println!("    {}:", LABEL_DEPENDENCIES);
                for dependency in dependencies {
                    println!(
                        "      - {} ({})",
                        dependency.name.as_str().bold(),
                        dependency.dep_type.as_str()
                    )
                }
            }
        }
        println!()
    } else {
        println!("No components found\n");
    }

    for (profile, commands) in ctx.application.all_custom_commands_for_all_profiles() {
        if commands.is_empty() {
            continue;
        }

        match profile {
            None => {
                println!("{}", "Custom commands:".log_color_help_group())
            }
            Some(profile) => {
                println!(
                    "{}{}{}",
                    "Custom commands for ".log_color_help_group(),
                    profile.as_str().log_color_help_group(),
                    " profile:".log_color_help_group()
                )
            }
        }
        for command in commands {
            println!("  {}", command.bold())
        }
        println!()
    }

    Ok(())
}

pub fn custom_command<CPE: ComponentPropertiesExtensions>(
    config: Config<CPE>,
    args: Vec<String>,
) -> anyhow::Result<()> {
    if args.len() != 1 {
        bail!("Invalid number of arguments for custom command, expected exactly one argument");
    }
    let command_name = &args[0];

    let ctx = ApplicationContext::new(config)?;

    let all_custom_commands = ctx.application.all_custom_commands(ctx.profile());
    if !all_custom_commands.contains(command_name) {
        if all_custom_commands.is_empty() {
            bail!(
                "Custom command {} not found, no custom command is available",
                command_name.log_color_error_highlight(),
            );
        } else {
            bail!(
                "Custom command {} not found, available custom commands: {}",
                command_name.log_color_error_highlight(),
                all_custom_commands
                    .iter()
                    .map(|s| s.log_color_highlight())
                    .join(", ")
            );
        }
    }

    log_action(
        "Executing",
        format!("custom command {}", command_name.log_color_highlight()),
    );
    let _indent = LogIndent::new();

    let common_custom_commands = ctx.application.common_custom_commands();
    if let Some(command) = common_custom_commands.get(command_name) {
        log_action(
            "Executing",
            format!(
                "common custom command {}",
                command_name.log_color_highlight(),
            ),
        );
        let _indent = LogIndent::new();

        for step in &command.value {
            execute_external_command(&ctx, &command.source, step)?;
        }
    }

    for component_name in ctx.application.component_names() {
        let properties = &ctx
            .application
            .component_properties(component_name, ctx.profile());
        if let Some(custom_command) = properties.custom_commands.get(command_name) {
            log_action(
                "Executing",
                format!(
                    "custom command {} for component {}",
                    command_name.log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            for step in custom_command {
                execute_external_command(
                    &ctx,
                    ctx.application.component_source_dir(component_name),
                    step,
                )?;
            }
        }
    }

    Ok(())
}

fn delete_path(context: &str, path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        log_warn_action(
            "Deleting",
            format!("{} {}", context, path.log_color_highlight()),
        );
        fs::remove(path).with_context(|| {
            anyhow!(
                "Failed to delete {}, path: {}",
                context.log_color_highlight(),
                path.log_color_highlight()
            )
        })?;
    }
    Ok(())
}

fn load_app<CPE: ComponentPropertiesExtensions>(
    config: &Config<CPE>,
) -> ValidatedResult<(Application<CPE>, BTreeSet<ComponentName>)> {
    collect_sources(&config.app_source_mode).and_then(|(sources, calling_working_dir)| {
        sources
            .into_iter()
            .map(|source| {
                ValidatedResult::from_result(app_raw::ApplicationWithSource::from_yaml_file(source))
            })
            .collect::<ValidatedResult<Vec<_>>>()
            .map(|source_apps| (source_apps, calling_working_dir))
    }).and_then(|(source_raw_apps, calling_working_dir)| {
        let current_dir = std::env::current_dir().expect("Failed to get current working directory");

        log_action("Collecting", "components");
        let _indent = LogIndent::new();

        Application::from_raw_apps(source_raw_apps)
            .and_then(|application| {
                let component_names: ValidatedResult<BTreeSet<ComponentName>> =
                    match &config.component_select_mode {
                        ComponentSelectMode::CurrentDir => match &config.app_source_mode {
                            ApplicationSourceMode::Automatic => {
                                let called_from_project_root = calling_working_dir == current_dir;
                                if called_from_project_root {
                                    ValidatedResult::Ok(BTreeSet::new())
                                } else {
                                    ValidatedResult::Ok(
                                        application
                                            .component_names()
                                            .filter(|component_name| application.component_source_dir(component_name).starts_with(calling_working_dir.as_path()))
                                            .cloned()
                                            .collect()
                                    )
                                }
                            }
                            ApplicationSourceMode::Explicit(_) => ValidatedResult::Ok(BTreeSet::new()),
                        },
                        ComponentSelectMode::Explicit(component_names) => {
                            let mut validation = ValidationBuilder::new();
                            for component_name in component_names {
                                if !application.contains_component(component_name) {
                                    validation.add_error(format!(
                                        "Requested component {} not found, available components: {}",
                                        component_name.as_str().log_color_error_highlight(),
                                        application
                                            .component_names()
                                            .map(|s| s.as_str().log_color_highlight())
                                            .join(", ")
                                    ));
                                }
                            }
                            validation.build(BTreeSet::from_iter(component_names.iter().cloned()))
                        }
                    };

                component_names.map(|component_names| (application, component_names))
            })
            .inspect(|(application, component_names)| {
                if application.component_names().next().is_none() {
                    log_action("Found", "no components")
                } else {
                    log_action(
                        "Found",
                        format!(
                            "components: {}",
                            application
                                .component_names()
                                .map(|s| s.as_str().log_color_highlight())
                                .join(", ")
                        ),
                    )
                }

                if !component_names.is_empty() {
                    match config.component_select_mode {
                        ComponentSelectMode::CurrentDir => log_action(
                            "Selected",
                            format!(
                                "components based on current dir: {} ",
                                component_names
                                    .iter()
                                    .map(|s| s.as_str().log_color_highlight())
                                    .join(", ")
                            ),
                        ),
                        ComponentSelectMode::Explicit(_) => log_action(
                            "Selected",
                            format!(
                                "components based on request: {} ",
                                component_names
                                    .iter()
                                    .map(|s| s.as_str().log_color_highlight())
                                    .join(", ")
                            ),
                        ),
                    }
                }
            })
    })
}

fn collect_sources(mode: &ApplicationSourceMode) -> ValidatedResult<(BTreeSet<PathBuf>, PathBuf)> {
    let calling_working_dir =
        std::env::current_dir().expect("Failed to get current working directory");

    log_action("Collecting", "sources");
    let _indent = LogIndent::new();

    match mode {
        ApplicationSourceMode::Automatic => match find_main_source() {
            Some(source) => {
                let source_ext = PathExtra::new(&source);
                let source_dir = source_ext.parent().unwrap();
                std::env::set_current_dir(source_dir)
                    .expect("Failed to set current dir for config parent");

                let includes = includes_from_yaml_file(source.as_path());
                if includes.is_empty() {
                    ValidatedResult::Ok(BTreeSet::from([source]))
                } else {
                    ValidatedResult::from_result(compile_and_collect_globs(source_dir, &includes))
                        .map(|mut sources| {
                            sources.insert(0, source);
                            sources.into_iter().collect()
                        })
                }
            }
            None => ValidatedResult::from_error("No application manifest found!".to_string()),
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

            ValidatedResult::from_value_and_warns(
                sources.iter().cloned().collect(),
                non_unique_source_warns,
            )
        }
    }
    .inspect(|sources| {
        if sources.is_empty() {
            log_action("Found", "no sources");
        } else {
            log_action(
                "Found",
                format!(
                    "sources: {}",
                    sources
                        .iter()
                        .map(|source| source.log_color_highlight())
                        .join(", ")
                ),
            );
        }
    })
    .map(|sources| (sources, calling_working_dir))
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

fn select_and_validate_profiles<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
) -> anyhow::Result<()> {
    match &ctx.config.profile {
        Some(profile) => {
            let all_profiles = ctx.application.all_profiles();
            if all_profiles.is_empty() {
                bail!(
                    "Profile {} not found, no available profiles",
                    profile.as_str().log_color_error_highlight(),
                );
            } else if !all_profiles.contains(profile) {
                bail!(
                    "Profile {} not found, available profiles: {}",
                    profile.as_str().log_color_error_highlight(),
                    all_profiles
                        .into_iter()
                        .map(|s| s.as_str().log_color_highlight())
                        .join(", ")
                );
            }
            log_action(
                "Selecting",
                format!(
                    "profiles, requested profile: {}",
                    profile.as_str().log_color_highlight()
                ),
            );
        }
        None => {
            log_action("Selecting", "profiles, no profile was requested");
        }
    }

    let _indent = LogIndent::new();
    for component_name in ctx.application.component_names() {
        let selection = ctx
            .application
            .component_effective_property_source(component_name, ctx.profile());

        let message = match (
            selection.profile,
            selection.template_name,
            ctx.profile().is_some(),
            selection.is_requested_profile,
        ) {
            (None, None, false, _) => {
                format!(
                    "default build for {}",
                    component_name.as_str().log_color_highlight()
                )
            }
            (None, None, true, _) => {
                format!(
                    "default build for {}, component has no profiles",
                    component_name.as_str().log_color_highlight()
                )
            }
            (None, Some(template), false, _) => {
                format!(
                    "default build for {} using template {}{}",
                    component_name.as_str().log_color_highlight(),
                    template.as_str().log_color_highlight(),
                    if selection.any_template_overrides {
                        " with overrides"
                    } else {
                        ""
                    }
                )
            }
            (None, Some(template), true, _) => {
                format!(
                    "default build for {} using template {}{}, component has no profiles",
                    component_name.as_str().log_color_highlight(),
                    template.as_str().log_color_highlight(),
                    if selection.any_template_overrides {
                        " with overrides"
                    } else {
                        ""
                    }
                )
            }
            (Some(profile), None, false, false) => {
                format!(
                    "default profile {} for {}",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                )
            }
            (Some(profile), None, true, false) => {
                format!(
                    "default profile {} for {}, component has no matching requested profile",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                )
            }
            (Some(profile), Some(template), false, false) => {
                format!(
                    "default profile {} for {} using template {}{}",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight(),
                    template.as_str().log_color_highlight(),
                    if selection.any_template_overrides {
                        " with overrides"
                    } else {
                        ""
                    }
                )
            }
            (Some(profile), Some(template), true, false) => {
                format!(
                    "default profile {} for {} using template {}{}, component has no matching requested profile",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight(),
                    template.as_str().log_color_highlight(),
                    if selection.any_template_overrides {
                        " with overrides"
                    } else {
                        ""
                    }
                )
            }
            (Some(profile), None, false, true) => {
                format!(
                    "profile {} for {}",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                )
            }
            (Some(profile), None, true, true) => {
                format!(
                    "requested profile {} for {}",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                )
            }
            (Some(profile), Some(template), false, true) => {
                format!(
                    "profile {} for {} using template {}{}",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight(),
                    template.as_str().log_color_highlight(),
                    if selection.any_template_overrides {
                        " with overrides"
                    } else {
                        ""
                    }
                )
            }
            (Some(profile), Some(template), true, true) => {
                format!(
                    "requested profile {} for {} using template {}{}",
                    profile.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight(),
                    template.as_str().log_color_highlight(),
                    if selection.any_template_overrides {
                        " with overrides"
                    } else {
                        ""
                    }
                )
            }
        };

        log_action("Selected", message);
    }

    Ok(())
}

fn to_anyhow<T>(
    log_output: Output,
    message: &str,
    result: ValidatedResult<T>,
) -> anyhow::Result<T> {
    fn format_warns(warns: Vec<String>) -> String {
        let label = "Warning".yellow();
        warns
            .into_iter()
            .map(|warn| format!("{}: {}", label, warn))
            .join("\n")
    }

    fn format_errors(errors: Vec<String>) -> String {
        let label = "Error".red();
        errors
            .into_iter()
            .map(|error| format!("{}: {}", label, error))
            .join("\n")
    }

    match result {
        ValidatedResult::Ok(value) => Ok(value),
        ValidatedResult::OkWithWarns(components, warns) => {
            match log_output {
                Output::Stdout => {
                    println!("\n{}\n", format_warns(warns));
                }
                Output::Stderr => {
                    eprintln!("\n{}\n", format_warns(warns));
                }
                Output::None => {
                    // NOP
                }
            }

            Ok(components)
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => {
            let message = match log_output {
                Output::Stdout => {
                    println!("\n");
                    println!("{}", format_warns(warns));
                    println!("{}", format_errors(errors));
                    println!("\n");

                    message.to_string()
                }
                Output::Stderr => {
                    eprintln!("\n");
                    eprintln!("{}", format_warns(warns));
                    eprintln!("{}", format_errors(errors));
                    eprintln!("\n");

                    message.to_string()
                }
                Output::None => {
                    fn with_new_line_if_not_empty(mut str: String) -> String {
                        if !str.is_empty() {
                            str.write_char('\n').unwrap()
                        }
                        str
                    }

                    let warns = with_new_line_if_not_empty(format_warns(warns));
                    let errors = with_new_line_if_not_empty(format_errors(errors));

                    format!("\n{}{}\n{}", warns, errors, message)
                }
            };

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
            glob_with(
                &format!("{}/{}", root_dir.to_string_lossy(), pattern),
                MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: false,
                    require_literal_leading_dot: true,
                },
            )
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

fn create_generated_base_wit<CPE: ComponentPropertiesExtensions>(
    ctx: &mut ApplicationContext<CPE>,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let component_source_wit = ctx
        .application
        .component_source_wit(component_name, ctx.profile());
    let component_generated_base_wit = ctx.application.component_generated_base_wit(component_name);
    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        ComponentGeneratorMarkerHash {
            component_name,
            generator_kind: "base_wit",
        },
    )?;

    if is_up_to_date(
        ctx.config.skip_up_to_date_checks
            || !task_result_marker.is_up_to_date()
            || !ctx.wit.is_dep_graph_up_to_date(component_name)?,
        || [component_source_wit.clone()],
        || [component_generated_base_wit.clone()],
    ) {
        log_skipping_up_to_date(format!(
            "creating generated base wit directory for {}",
            component_name.as_str().log_color_highlight()
        ));
        Ok(false)
    } else {
        log_action(
            "Creating",
            format!(
                "generated base wit directory for {}",
                component_name.as_str().log_color_highlight(),
            ),
        );

        task_result_marker.result((|| {
            let _indent = LogIndent::new();

            delete_path(
                "generated base wit directory",
                &component_generated_base_wit,
            )?;
            copy_wit_sources(&component_source_wit, &component_generated_base_wit)?;

            {
                let missing_package_deps = ctx
                    .wit
                    .missing_generic_source_package_deps(component_name)?;

                if !missing_package_deps.is_empty() {
                    log_action("Adding", "package deps");
                    let _indent = LogIndent::new();

                    ctx.common_wit_deps()
                        .with_context(|| {
                            format!(
                                "Failed to add package dependencies: {}",
                                missing_package_deps
                                    .iter()
                                    .map(|s| s.to_string().log_color_highlight())
                                    .join(", ")
                            )
                        })?
                        .add_packages_with_transitive_deps_to_wit_dir(
                            &missing_package_deps,
                            &component_generated_base_wit,
                        )?;
                }
            }

            {
                let component_exports_package_deps =
                    ctx.wit.component_exports_package_deps(component_name)?;
                if !component_exports_package_deps.is_empty() {
                    log_action("Adding", "component exports package dependencies");
                    let _indent = LogIndent::new();

                    for (dep_exports_package_name, dep_component_name) in
                        &component_exports_package_deps
                    {
                        ctx.component_base_output_wit_deps(dep_component_name)?
                            .add_packages_with_transitive_deps_to_wit_dir(
                                &[dep_exports_package_name.clone()],
                                &component_generated_base_wit,
                            )?;
                    }
                }
            }

            {
                log_action(
                    "Extracting",
                    format!(
                        "exports package from {} to {}",
                        component_source_wit.log_color_highlight(),
                        component_generated_base_wit.log_color_highlight()
                    ),
                );
                let _indent = LogIndent::new();
                extract_exports_as_wit_dep(&component_generated_base_wit)?
            }

            Ok(true)
        })())
    }
}

fn create_generated_wit<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let component_generated_base_wit = ctx.application.component_generated_base_wit(component_name);
    let component_generated_wit = ctx
        .application
        .component_generated_wit(component_name, ctx.profile());
    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        ComponentGeneratorMarkerHash {
            component_name,
            generator_kind: "wit",
        },
    )?;

    if is_up_to_date(
        ctx.config.skip_up_to_date_checks
            || !task_result_marker.is_up_to_date()
            || !ctx.wit.is_dep_graph_up_to_date(component_name)?,
        || [component_generated_base_wit.clone()],
        || [component_generated_wit.clone()],
    ) {
        log_skipping_up_to_date(format!(
            "creating generated wit directory for {}",
            component_name.as_str().log_color_highlight()
        ));
        Ok(false)
    } else {
        log_action(
            "Creating",
            format!(
                "generated wit directory for {}",
                component_name.as_str().log_color_highlight(),
            ),
        );

        task_result_marker.result((|| {
            let _indent = LogIndent::new();
            delete_path("generated wit directory", &component_generated_wit)?;
            copy_wit_sources(&component_generated_base_wit, &component_generated_wit)?;
            add_client_deps(ctx, component_name)?;
            Ok(true)
        })())
    }
}

fn update_cargo_toml<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
    mut skip_up_to_date_checks: bool,
    component_name: &ComponentName,
) -> anyhow::Result<()> {
    let component_source_wit = PathExtra::new(
        ctx.application
            .component_source_wit(component_name, ctx.profile()),
    );
    let component_source_wit_parent = component_source_wit.parent().with_context(|| {
        anyhow!(
            "Failed to get parent for component {}",
            component_name.as_str().log_color_highlight()
        )
    })?;
    let cargo_toml = component_source_wit_parent.join("Cargo.toml");

    if !cargo_toml.exists() {
        return Ok(());
    }

    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        ComponentGeneratorMarkerHash {
            component_name,
            generator_kind: "Cargo.toml",
        },
    )?;

    skip_up_to_date_checks |= skip_up_to_date_checks || ctx.config.skip_up_to_date_checks;
    if !skip_up_to_date_checks && task_result_marker.is_up_to_date() {
        log_skipping_up_to_date(format!(
            "updating Cargo.toml for {}",
            component_name.as_str().log_color_highlight()
        ));
        return Ok(());
    }

    task_result_marker.result(regenerate_cargo_package_component(
        &cargo_toml,
        &ctx.application
            .component_generated_wit(component_name, ctx.profile()),
        None,
    ))
}

async fn build_client<CPE: ComponentPropertiesExtensions>(
    ctx: &mut ApplicationContext<CPE>,
    component: &DependentComponent,
) -> anyhow::Result<bool> {
    let stub_def = ctx.component_stub_def(&component.name)?;
    let client_wit_root = stub_def.client_wit_root();

    let client_dep_package_ids = stub_def.stub_dep_package_ids();
    let client_sources: Vec<PathBuf> = stub_def
        .packages_with_wit_sources()
        .flat_map(|(package_id, _, sources)| {
            (client_dep_package_ids.contains(&package_id)
                || package_id == stub_def.source_package_id)
                .then(|| sources.files.iter().cloned())
                .unwrap_or_default()
        })
        .collect();

    let client_wasm = ctx.application.client_wasm(&component.name);
    let client_wit = ctx.application.client_wit(&component.name);
    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        ComponentGeneratorMarkerHash {
            component_name: &component.name,
            generator_kind: "client",
        },
    )?;

    if is_up_to_date(
        ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date(),
        || client_sources,
        || {
            if component.dep_type == DependencyType::StaticWasmRpc {
                vec![client_wit.clone(), client_wasm.clone()]
            } else {
                vec![client_wit.clone()]
            }
        },
    ) {
        // TODO: message based on type
        log_skipping_up_to_date(format!(
            "generating WASM RPC client for {}",
            component.name.as_str().log_color_highlight()
        ));
        Ok(false)
    } else {
        task_result_marker.result(
            async {
                match component.dep_type {
                    DependencyType::StaticWasmRpc => {
                        log_action(
                            "Building",
                            format!(
                                "WASM RPC client for {}",
                                component.name.as_str().log_color_highlight()
                            ),
                        );

                        let _indent = LogIndent::new();

                        delete_path("client temp build dir", &client_wit_root)?;
                        delete_path("client wit", &client_wit)?;
                        delete_path("client wasm", &client_wasm)?;

                        log_action(
                            "Creating",
                            format!(
                                "client temp build dir {}",
                                client_wit_root.log_color_highlight()
                            ),
                        );
                        fs::create_dir_all(&client_wit_root)?;

                        let offline = ctx.config.offline;
                        commands::generate::build(
                            ctx.component_stub_def(&component.name)?,
                            &client_wasm,
                            &client_wit,
                            offline,
                        )
                        .await?;

                        if !env_var_flag("WASM_RPC_KEEP_CLIENT_DIR") {
                            delete_path("client temp build dir", &client_wit_root)?;
                        }

                        Ok(())
                    }
                    DependencyType::DynamicWasmRpc => {
                        log_action(
                            "Generating",
                            format!(
                                "WASM RPC client for {}",
                                component.name.as_str().log_color_highlight()
                            ),
                        );
                        let _indent = LogIndent::new();

                        delete_path("client wit", &client_wit)?;

                        log_action(
                            "Creating",
                            format!(
                                "client temp build dir {}",
                                client_wit_root.log_color_highlight()
                            ),
                        );
                        fs::create_dir_all(&client_wit_root)?;

                        let stub_def = ctx.component_stub_def(&component.name)?;
                        commands::generate::generate_and_copy_client_wit(stub_def, &client_wit)
                    }
                }
            }
            .await,
        )?;

        Ok(true)
    }
}

fn add_client_deps<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let dependencies = ctx
        .application
        .component_wasm_rpc_dependencies(component_name);
    if dependencies.is_empty() {
        Ok(false)
    } else {
        log_action(
            "Adding",
            format!(
                "client wit dependencies to {}",
                component_name.as_str().log_color_highlight()
            ),
        );

        let _indent = LogIndent::new();

        for dep_component in dependencies {
            log_action(
                "Adding",
                format!(
                    "{} client wit dependency to {}",
                    dep_component.name.as_str().log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
                client_wit_root: ctx.application.client_wit(&dep_component.name),
                dest_wit_root: ctx
                    .application
                    .component_generated_wit(component_name, ctx.profile()),
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
            "Failed to read component source wit directory entries for {}",
            source.log_color_highlight()
        )
    })?;

    for file in dir_content.files {
        let from = PathBuf::from(&file);
        let to = target.join(from.strip_prefix(source).with_context(|| {
            anyhow!(
                "Failed to strip prefix for source {}",
                &file.log_color_highlight()
            )
        })?);

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

fn execute_external_command<CPE: ComponentPropertiesExtensions>(
    ctx: &ApplicationContext<CPE>,
    base_build_dir: &Path,
    command: &app_raw::ExternalCommand,
) -> anyhow::Result<()> {
    let build_dir = command
        .dir
        .as_ref()
        .map(|dir| base_build_dir.join(dir))
        .unwrap_or_else(|| base_build_dir.to_path_buf());

    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        ResolvedExternalCommandMarkerHash {
            build_dir: &build_dir,
            command,
        },
    )?;

    let skip_up_to_date_checks =
        ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date();

    if !command.sources.is_empty() && !command.targets.is_empty() {
        let sources = compile_and_collect_globs(&build_dir, &command.sources)?;
        let targets = compile_and_collect_globs(&build_dir, &command.targets)?;

        if is_up_to_date(skip_up_to_date_checks, || sources, || targets) {
            log_skipping_up_to_date(format!(
                "executing external command '{}' in directory {}",
                command.command.log_color_highlight(),
                build_dir.log_color_highlight()
            ));
            return Ok(());
        }
    }

    log_action(
        "Executing",
        format!(
            "external command '{}' in directory {}",
            command.command.log_color_highlight(),
            build_dir.log_color_highlight()
        ),
    );

    task_result_marker.result((|| {
        if !command.rmdirs.is_empty() {
            let _ident = LogIndent::new();
            for dir in &command.rmdirs {
                let dir = build_dir.join(dir);
                delete_path("directory", &dir)?;
            }
        }

        if !command.mkdirs.is_empty() {
            let _ident = LogIndent::new();
            for dir in &command.mkdirs {
                let dir = build_dir.join(dir);
                if !std::fs::exists(&dir)? {
                    log_action(
                        "Creating",
                        format!("directory {}", dir.log_color_highlight()),
                    );
                    std::fs::create_dir_all(dir)?
                }
            }
        }

        let command_tokens = shlex::split(&command.command).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse external command: {}", command.command)
        })?;
        if command_tokens.is_empty() {
            return Err(anyhow!("Empty command!"));
        }

        let result = Command::new(command_tokens[0].clone())
            .args(command_tokens.iter().skip(1))
            .current_dir(build_dir)
            .status()
            .with_context(|| "Failed to execute command".to_string())?;

        if result.success() {
            Ok(())
        } else {
            Err(anyhow!(format!(
                "Command failed with exit code: {}",
                result
                    .code()
                    .map(|code| code.to_string().log_color_error_highlight().to_string())
                    .unwrap_or_else(|| "?".to_string())
            )))
        }
    })())
}

trait TaskResultMarkerHashInput {
    fn task_kind() -> &'static str;

    fn hash_input(&self) -> anyhow::Result<Vec<u8>>;
}

#[derive(Serialize)]
struct ResolvedExternalCommandMarkerHash<'a> {
    build_dir: &'a Path,
    command: &'a app_raw::ExternalCommand,
}

impl TaskResultMarkerHashInput for ResolvedExternalCommandMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "ResolvedExternalCommandMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_yaml::to_string(self)?.into_bytes())
    }
}

struct ComponentGeneratorMarkerHash<'a> {
    component_name: &'a ComponentName,
    generator_kind: &'a str,
}

impl TaskResultMarkerHashInput for ComponentGeneratorMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "ComponentGeneratorMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(format!("{}-{}", self.component_name, self.generator_kind).into_bytes())
    }
}

struct LinkRpcMarkerHash<'a> {
    component_name: &'a ComponentName,
    dependencies: &'a BTreeSet<&'a DependentComponent>,
}

impl TaskResultMarkerHashInput for LinkRpcMarkerHash<'_> {
    fn task_kind() -> &'static str {
        "RpcLinkMarkerHash"
    }

    fn hash_input(&self) -> anyhow::Result<Vec<u8>> {
        Ok(format!(
            "{}#{}",
            self.component_name,
            self.dependencies
                .iter()
                .map(|s| format!("{}#{}", s.name.as_str(), s.dep_type.as_str()))
                .join(",")
        )
        .into_bytes())
    }
}

struct TaskResultMarker {
    success_marker_file_path: PathBuf,
    failure_marker_file_path: PathBuf,
    success_before: bool,
    failure_before: bool,
}

static TASK_RESULT_MARKER_SUCCESS_SUFFIX: &str = "-success";
static TASK_RESULT_MARKER_FAILURE_SUFFIX: &str = "-failure";

impl TaskResultMarker {
    fn new<T: TaskResultMarkerHashInput>(dir: &Path, task: T) -> anyhow::Result<Self> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(T::task_kind().as_bytes());
        hasher.update(&task.hash_input()?);
        let hex_hash = hasher.finalize().to_hex().to_string();

        let success_marker_file_path = dir.join(format!(
            "{}{}",
            &hex_hash, TASK_RESULT_MARKER_SUCCESS_SUFFIX
        ));
        let failure_marker_file_path = dir.join(format!(
            "{}{}",
            &hex_hash, TASK_RESULT_MARKER_FAILURE_SUFFIX
        ));

        let success_marker_exists = success_marker_file_path.exists();
        let failure_marker_exists = failure_marker_file_path.exists();

        let (success_before, failure_before) = match (success_marker_exists, failure_marker_exists)
        {
            (true, false) => (true, false),
            (false, false) => (false, false),
            (_, true) => (false, true),
        };

        if failure_marker_exists || !success_marker_exists {
            if success_marker_exists {
                fs::remove(&success_marker_file_path)?
            }
            if failure_marker_exists {
                fs::remove(&failure_marker_file_path)?
            }
        }

        Ok(Self {
            success_marker_file_path,
            failure_marker_file_path,
            success_before,
            failure_before,
        })
    }

    fn is_up_to_date(&self) -> bool {
        !self.failure_before && self.success_before
    }

    fn success(&self) -> anyhow::Result<()> {
        fs::write_str(&self.success_marker_file_path, "")
    }

    fn failure(&self) -> anyhow::Result<()> {
        fs::write_str(&self.failure_marker_file_path, "")
    }

    fn result<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        match result {
            Ok(result) => {
                self.success()?;
                Ok(result)
            }
            Err(source_err) => {
                self.failure().with_context(|| {
                    anyhow!(
                        "Failed to save failure marker for source error: {:?}",
                        source_err,
                    )
                })?;
                Err(source_err)
            }
        }
    }
}

fn env_var_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|flag| {
            let flag = flag.to_lowercase();
            flag.starts_with("t") || flag == "1"
        })
        .unwrap_or_default()
}
