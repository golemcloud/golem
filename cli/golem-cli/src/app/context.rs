// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::app::build::build_app;
use crate::app::build::clean::clean_app;
use crate::app::build::command::execute_custom_command;
use crate::app::error::{AppValidationError, CustomCommandError, format_warns};
use crate::app::manifest_upgrade::plan_manifest_upgrade_steps;
use crate::app::manifest_version::validate_manifest_versions;
use crate::app::template::AppTemplateRepo;
use crate::command_handler::ResolvedToolMiddlewareGrants;
use crate::command_handler::interactive::InteractiveHandler;
use crate::error::NonSuccessfulExit;
use crate::fs;
use crate::log::{LogColorize, LogIndent, LogOutput, Output, log_action, logln};
use crate::model::app::{
    AppBuildStep, Application, ApplicationComponentSelectMode, ApplicationConfig,
    ApplicationPreload, ApplicationSourceMode, BuildConfig, CleanMode, ComponentPresetSelector,
    CustomBridgeSdkTarget, DynamicHelpSections, LoadedRawApps, ResolvedLocalServer, WithSource,
    includes_from_yaml_file,
};
use crate::model::app_raw;
use crate::model::component::format_component_applied_layers;
use crate::model::config::server::ToFormattedServerContext;
use crate::model::deploy::log_unified_diff_for_path;
use crate::model::format::Format;
use crate::model::language::GuestLanguage;
use crate::model::text_format::DecoratedIndent;
use crate::model::tool_release::ResolvedToolGrants;
use crate::validation::{ValidatedResult, ValidationBuilder};
use anyhow::{Context, anyhow, bail, ensure};
use colored::Colorize;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentPlanAmbientToolEntry;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::environment_tool_grant::EnvironmentToolGrantWithDetails;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

#[derive(Clone, Debug)]
pub struct ResolvedMcpDiagnostic {
    pub canonical_name: String,
    pub import_index: u32,
    pub upstream_name: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedEnvironmentTools {
    pub environment_id: EnvironmentId,
    pub ambient_tools: Vec<DeploymentPlanAmbientToolEntry>,
    pub mcp_tools: Vec<golem_client::model::McpResolvedTool>,
    pub mcp_diagnostics: Vec<ResolvedMcpDiagnostic>,
}

#[derive(Clone, Copy, Debug)]
pub enum ResolvedEnvironmentTool<'a> {
    Ambient(&'a DeploymentPlanAmbientToolEntry),
    Mcp(&'a golem_client::model::McpResolvedTool),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildMetadataRequired {
    #[error("Remote tool '{0}' is not granted to the selected environment")]
    Tool(golem_common::model::tool::ToolName),
    #[error("Tool middleware '{0}' has no environment release grant")]
    Middleware(golem_common::model::tool_middleware::ToolMiddlewareName),
    #[error(
        "Environment tool dependency '{0}' was not found among the selected environment's ambient tools or MCP imports"
    )]
    EnvironmentTool(golem_common::model::tool::ToolName),
}

pub struct BuildContext<'a> {
    application_context: &'a ApplicationContext,
    build_config: &'a BuildConfig,
    resolved_tool_grants: Option<&'a ResolvedToolGrants>,
    resolved_tool_middleware_grants: Option<&'a ResolvedToolMiddlewareGrants>,
    environment_tools: Option<&'a ResolvedEnvironmentTools>,
    pub(crate) extracted_metadata: std::sync::Mutex<
        BTreeMap<ComponentName, golem_common::model::agent::extraction::ExtractedComponentMetadata>,
    >,
}

impl<'a> BuildContext<'a> {
    pub fn new(application_context: &'a ApplicationContext, build_config: &'a BuildConfig) -> Self {
        Self {
            application_context,
            build_config,
            resolved_tool_grants: None,
            resolved_tool_middleware_grants: None,
            environment_tools: None,
            extracted_metadata: Default::default(),
        }
    }

    pub fn new_with_resolved_tool_grants(
        application_context: &'a ApplicationContext,
        build_config: &'a BuildConfig,
        resolved_tool_grants: &'a ResolvedToolGrants,
    ) -> Self {
        Self {
            application_context,
            build_config,
            resolved_tool_grants: Some(resolved_tool_grants),
            resolved_tool_middleware_grants: None,
            environment_tools: None,
            extracted_metadata: Default::default(),
        }
    }

    pub fn with_tool_middleware_grants(mut self, grants: &'a ResolvedToolMiddlewareGrants) -> Self {
        self.resolved_tool_middleware_grants = Some(grants);
        self
    }

    pub(crate) fn remote_middleware_definition(
        &self,
        name: &golem_common::model::tool_middleware::ToolMiddlewareName,
    ) -> anyhow::Result<golem_common::schema::tool::ToolMiddleware> {
        let declaration = self
            .application()
            .tool_middleware_declarations()
            .get(name)
            .ok_or_else(|| anyhow!("Tool middleware '{name}' is not declared"))?;
        let reference = declaration
            .value
            .release
            .as_ref()
            .ok_or_else(|| anyhow!("Tool middleware '{name}' has no release"))?
            .to_release_reference()
            .map_err(anyhow::Error::msg)?;
        self.resolved_tool_middleware_grants
            .and_then(|grants| grants.get(&reference))
            .map(|grant| grant.release.definition.clone())
            .ok_or_else(|| BuildMetadataRequired::Middleware(name.clone()).into())
    }

    pub fn with_environment_tools(mut self, tools: &'a ResolvedEnvironmentTools) -> Self {
        self.environment_tools = Some(tools);
        self
    }

    pub fn mcp_tools(&self) -> &[golem_client::model::McpResolvedTool] {
        self.environment_tools
            .map(|tools| tools.mcp_tools.as_slice())
            .unwrap_or_default()
    }

    pub fn ambient_tools(&self) -> &[DeploymentPlanAmbientToolEntry] {
        self.environment_tools
            .map(|tools| tools.ambient_tools.as_slice())
            .unwrap_or_default()
    }

    pub fn environment_tools_id(&self) -> Option<EnvironmentId> {
        self.environment_tools.map(|tools| tools.environment_id)
    }

    pub fn environment_tool(
        &self,
        name: &golem_common::model::tool::ToolName,
    ) -> anyhow::Result<ResolvedEnvironmentTool<'_>> {
        if let Some(tool) = self.ambient_tools().iter().find(|tool| tool.name == *name) {
            return Ok(ResolvedEnvironmentTool::Ambient(tool));
        }
        if let Some(tool) = self
            .mcp_tools()
            .iter()
            .find(|tool| tool.definition.name() == Some(name.as_str()))
        {
            return Ok(ResolvedEnvironmentTool::Mcp(tool));
        }
        if let Some(diagnostic) = self.environment_tool_diagnostic(name) {
            bail!(
                "Environment tool '{name}' was rejected by MCP import {} (upstream '{}'): {}",
                diagnostic.import_index,
                diagnostic.upstream_name,
                diagnostic.reason
            );
        }
        Err(BuildMetadataRequired::EnvironmentTool(name.clone()).into())
    }

    pub fn environment_tool_diagnostic(
        &self,
        name: &golem_common::model::tool::ToolName,
    ) -> Option<&ResolvedMcpDiagnostic> {
        self.environment_tools
            .into_iter()
            .flat_map(|tools| &tools.mcp_diagnostics)
            .find(|diagnostic| diagnostic.canonical_name == name.as_str())
    }

    pub fn application_context(&self) -> &ApplicationContext {
        self.application_context
    }

    pub fn application(&self) -> &Application {
        &self.application_context.application
    }

    pub fn application_config(&self) -> &ApplicationConfig {
        &self.application_context.config
    }

    pub fn component_metadata(&self) -> &crate::app::component_metadata::ComponentMetadataRegistry {
        self.application_context.component_metadata()
    }

    pub fn build_config(&self) -> &BuildConfig {
        self.build_config
    }

    pub fn should_run_step(&self, step: AppBuildStep) -> bool {
        self.build_config().should_run_step(step)
    }

    pub fn custom_bridge_sdk_target(&self) -> Option<&CustomBridgeSdkTarget> {
        self.build_config.custom_bridge_sdk_target.as_ref()
    }

    pub fn repl_bridge_sdk_target(&self) -> Option<&CustomBridgeSdkTarget> {
        self.build_config.repl_bridge_sdk_target.as_ref()
    }

    pub fn skip_up_to_date_checks(&self) -> bool {
        self.build_config.skip_up_to_date_checks
    }

    pub fn should_colorize(&self) -> bool {
        self.application_config().should_colorize
    }

    pub fn tools_with_ensured_common_deps(&self) -> &ToolsWithEnsuredCommonDeps {
        &self.application_context.tools_with_ensured_common_deps
    }

    pub fn release_grant(
        &self,
        reference: &app_raw::RegistrySubject,
    ) -> Option<&EnvironmentToolGrantWithDetails> {
        self.resolved_tool_grants?
            .get(&reference.to_release_reference().ok()?)
    }

    pub fn release_grant_by_name(
        &self,
        name: &golem_common::model::tool::ToolName,
    ) -> Option<&EnvironmentToolGrantWithDetails> {
        self.application()
            .remote_release_reference(name)
            .and_then(|reference| self.release_grant(reference))
    }
}

pub struct ToolsWithEnsuredCommonDeps {
    tools_with_ensured_common_deps: tokio::sync::RwLock<HashSet<String>>,
}

impl Default for ToolsWithEnsuredCommonDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolsWithEnsuredCommonDeps {
    pub fn new() -> ToolsWithEnsuredCommonDeps {
        ToolsWithEnsuredCommonDeps {
            tools_with_ensured_common_deps: tokio::sync::RwLock::new(HashSet::new()),
        }
    }

    pub async fn ensure_common_deps_for_tool_once(
        &self,
        tool: &str,
        ensure: impl AsyncFnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        if self
            .tools_with_ensured_common_deps
            .read()
            .await
            .contains(tool)
        {
            return Ok(());
        }

        let mut lock = self.tools_with_ensured_common_deps.write().await;

        let result = ensure().await;

        if result.is_ok() {
            lock.insert(tool.to_string());
        }

        result
    }
}

pub struct ApplicationPreloadResult {
    pub source_mode: ApplicationSourceMode,
    pub loaded_with_warnings: bool,
    pub application_preload: Option<ApplicationPreload>,
    pub resolved_local_server: Option<ResolvedLocalServer>,
}

pub struct ApplicationContext {
    loaded_with_warnings: bool,
    config: ApplicationConfig,
    application: Application,
    component_metadata: crate::app::component_metadata::ComponentMetadataRegistry,
    pub(crate) publisher_package: Option<golem_package::PackageIdentity>,
    pub(crate) has_development_packages: bool,
    pub(crate) installed_components: BTreeMap<ComponentName, golem_package::InstalledComponent>,
    pub(crate) installed_package_calls: BTreeMap<String, BTreeSet<AgentTypeName>>,
    _package_dir: Option<crate::package::UnpackRoot>,
    calling_working_dir: PathBuf,
    selected_component_names: BTreeSet<ComponentName>,
    builtin_local_url: url::Url,
    tools_with_ensured_common_deps: ToolsWithEnsuredCommonDeps,
}

impl ApplicationContext {
    #[cfg(test)]
    pub(crate) fn for_test(application: Application) -> Self {
        Self {
            calling_working_dir: application.app_root_dir().to_path_buf(),
            selected_component_names: application.component_names().cloned().collect(),
            application,
            loaded_with_warnings: false,
            config: ApplicationConfig {
                offline: true,
                dev_mode: false,
                should_colorize: false,
                enable_wasmtime_fs_cache: false,
            },
            component_metadata: crate::app::component_metadata::ComponentMetadataRegistry::new(
                false,
            ),
            publisher_package: None,
            has_development_packages: false,
            installed_components: BTreeMap::new(),
            installed_package_calls: BTreeMap::new(),
            _package_dir: None,
            builtin_local_url: "http://localhost:9881".parse().unwrap(),
            tools_with_ensured_common_deps: ToolsWithEnsuredCommonDeps::new(),
        }
    }

    pub fn plan_and_apply_manifest_upgrades_before_load(
        source_mode: ApplicationSourceMode,
        yes: bool,
    ) -> anyhow::Result<()> {
        Self::plan_and_apply_manifest_upgrades_before_load_from(
            source_mode,
            yes,
            &fs::current_dir_lexical()?,
        )
    }

    fn plan_and_apply_manifest_upgrades_before_load_from(
        source_mode: ApplicationSourceMode,
        yes: bool,
        calling_working_dir: &Path,
    ) -> anyhow::Result<()> {
        let collected_sources = {
            let _output = LogOutput::new(Output::None);
            match &source_mode {
                ApplicationSourceMode::Automatic => collect_sources_from(calling_working_dir, None),
                ApplicationSourceMode::ByRootManifest(root_manifest) => {
                    collect_sources_from(calling_working_dir, Some(root_manifest))
                }
                ApplicationSourceMode::Preloaded(_) | ApplicationSourceMode::None => None,
            }
        };

        let Some(collected_sources) = collected_sources else {
            return Ok(());
        };
        let collected_sources = validated_to_anyhow(
            "Failed to collect application manifests for automatic upgrade",
            collected_sources,
            None,
        )?;
        let steps = plan_manifest_upgrade_steps(&collected_sources.sources)?;
        if steps.is_empty() {
            return Ok(());
        }

        let _output = LogOutput::new(Output::Stderr);
        logln("");
        log_action("Planned", "automatic application manifest upgrade changes");
        let _indent = DecoratedIndent::new_primary(Format::Text);
        for step in &steps {
            logln(format!(
                "- {} {}",
                "update".green(),
                step.path.display().to_string().log_color_highlight()
            ));
            let _indent = LogIndent::new();
            let _indent = DecoratedIndent::new_secondary(Format::Text);
            log_unified_diff_for_path(
                &step.path,
                &diff::unified_diff(step.current.as_str(), step.new.as_str()),
            );
        }
        if !InteractiveHandler::confirm_manifest_upgrade_plan_apply(yes)? {
            bail!(NonSuccessfulExit);
        }

        logln("");
        log_action("Applying", "application manifest upgrades");
        let _indent = LogIndent::new();
        for step in &steps {
            log_action(
                "Updating",
                format!("{}", step.path.display().to_string().log_color_highlight()),
            );
            fs::write_str(&step.path, step.new.as_str())?;
        }
        Ok(())
    }

    /// Finds and reads the application's manifests by `source_mode`, searching from
    /// `calling_working_dir`, and makes the application's root the current directory.
    pub fn preload_application(
        source_mode: ApplicationSourceMode,
        calling_working_dir: &Path,
        dev_mode: bool,
    ) -> anyhow::Result<ApplicationPreloadResult> {
        let _output = LogOutput::new(Output::None);

        match preload_app(source_mode, calling_working_dir, dev_mode) {
            Some(environments) => validated_to_anyhow(
                "Failed to load application manifest environments, see problems above",
                environments,
                Some(|mut preload_result| {
                    preload_result.loaded_with_warnings = true;
                    preload_result
                }),
            ),
            None => Ok(ApplicationPreloadResult {
                source_mode: ApplicationSourceMode::None,
                loaded_with_warnings: false,
                application_preload: None,
                resolved_local_server: None,
            }),
        }
    }

    pub async fn new(
        source_mode: ApplicationSourceMode,
        config: ApplicationConfig,
        application_name: WithSource<ApplicationName>,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        local_server: Option<WithSource<app_raw::LocalServer>>,
        component_presets: ComponentPresetSelector,
        builtin_local_url: url::Url,
        config_dir: &Path,
    ) -> anyhow::Result<Option<ApplicationContext>> {
        let mut source_mode = source_mode;
        let mut has_development_packages = false;
        let mut package_agent_defaults = BTreeMap::new();
        let (publisher_package, expanded, package_dir) = match &mut source_mode {
            ApplicationSourceMode::Preloaded(loaded) => {
                let (publisher_package, pins, lock) = package_pins(loaded)?;
                has_development_packages = package_root(loaded).is_some_and(|root| {
                    root.packages
                        .values()
                        .any(|entry| matches!(entry, crate::package::PackageEntry::Path { .. }))
                });
                let selected_tools = package_tool_selections(loaded, &pins)?;
                if pins.is_empty() {
                    (publisher_package, Default::default(), None)
                } else {
                    let dir = crate::package::UnpackRoot::new(&loaded.app_root_dir)?;
                    let mut effects = crate::package::LocalEffects {
                        app_root: &loaded.app_root_dir,
                        cache_root: config_dir.join("package-cache"),
                    };
                    let mut expanded = golem_package::expand(
                        &pins,
                        &lock,
                        &selected_tools,
                        dir.path(),
                        &mut effects,
                    )
                    .await?;
                    check_package_tool_collisions(&loaded.raw_apps, &expanded.declarations)?;
                    for installed in expanded.components.values_mut() {
                        if let golem_package::PackageSource::File(path) =
                            golem_package::PackageSource::parse(&installed.package_metadata.source)
                        {
                            // Do not send absolute local paths to the registry.
                            let source = fs::absolute_lexical_path_from_base_dir(
                                Path::new(path),
                                &loaded.app_root_dir,
                            );
                            installed.package_metadata.source =
                                match source.strip_prefix(&loaded.app_root_dir) {
                                    Ok(relative) => fs::path_to_unix_str(relative)?,
                                    Err(_) => fs::file_name_to_str(&source)?.to_owned(),
                                };
                        }
                    }
                    let mut package_app = package_manifest(dir.path(), &expanded.declarations)?;
                    for app in &loaded.raw_apps {
                        for (name, agent) in &app.application.agents {
                            if agent.package.is_some()
                                && let Some(defaults) =
                                    package_app.application.agents.shift_remove(name)
                            {
                                package_agent_defaults.insert(
                                    name.clone(),
                                    WithSource::new(package_app.source.clone(), defaults),
                                );
                            }
                        }
                    }
                    loaded.raw_apps.push(package_app);
                    (publisher_package, expanded, Some(dir))
                }
            }
            _ => (None, Default::default(), None),
        };
        if let ApplicationSourceMode::Preloaded(loaded) = &source_mode {
            validate_agent_package_selectors(&loaded.raw_apps, &expanded.agent_packages)?;
        }
        let Some(app_and_calling_working_dir) = load_app(
            application_name,
            environments,
            local_server,
            component_presets,
            source_mode,
        ) else {
            return Ok(None);
        };

        let mut ctx = validated_to_anyhow(
            "Failed to load application manifest, see problems above",
            Self::create_context(app_and_calling_working_dir, config, builtin_local_url).await,
            Some(|mut app_ctx| {
                app_ctx.loaded_with_warnings = true;
                app_ctx
            }),
        )?;
        ctx.publisher_package = publisher_package;
        ctx.has_development_packages = has_development_packages;
        ctx.installed_components = expanded.components;
        ctx.installed_package_calls = expanded.calls;
        ctx.application.package_agent_defaults = package_agent_defaults;
        ctx._package_dir = package_dir;

        if ctx.config.offline {
            log_action("Using", "offline mode");
        }

        Ok(Some(ctx))
    }

    pub fn loaded_with_warnings(&self) -> bool {
        self.loaded_with_warnings
    }

    pub fn application(&self) -> &Application {
        &self.application
    }

    pub fn component_metadata(&self) -> &crate::app::component_metadata::ComponentMetadataRegistry {
        &self.component_metadata
    }

    pub fn new_repl_bridge_sdk_target(&self, language: GuestLanguage) -> CustomBridgeSdkTarget {
        let repl_root_bridge_sdk_dir = self.application.repl_root_bridge_sdk_dir(language);
        CustomBridgeSdkTarget {
            agent_type_names: Default::default(),
            target_language: Some(language),
            output_dir: Some(repl_root_bridge_sdk_dir.clone()),
            rust_config: Default::default(),
        }
    }

    async fn create_context(
        app_and_calling_working_dir: ValidatedResult<(Application, PathBuf)>,
        config: ApplicationConfig,
        builtin_local_url: url::Url,
    ) -> ValidatedResult<ApplicationContext> {
        let tools_with_ensured_common_deps = ToolsWithEnsuredCommonDeps::new();
        app_and_calling_working_dir.map(|(application, calling_working_dir)| {
            let component_metadata = crate::app::component_metadata::ComponentMetadataRegistry::new(
                config.enable_wasmtime_fs_cache,
            );
            ApplicationContext {
                loaded_with_warnings: false,
                config,
                application,
                component_metadata,
                publisher_package: None,
                has_development_packages: false,
                installed_components: BTreeMap::new(),
                installed_package_calls: BTreeMap::new(),
                _package_dir: None,
                calling_working_dir,
                selected_component_names: BTreeSet::new(),
                builtin_local_url,
                tools_with_ensured_common_deps,
            }
        })
    }

    pub fn select_components(
        &mut self,
        component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        log_action("Selecting", "components");
        let _indent = LogIndent::new();

        let current_dir = fs::current_dir_lexical()?;

        let selected_component_names: ValidatedResult<BTreeSet<ComponentName>> =
            match component_select_mode {
                ApplicationComponentSelectMode::CurrentDir => {
                    let all_components = || {
                        ValidatedResult::Ok(
                            self.application
                                .component_names()
                                .map(|cn| cn.to_owned())
                                .collect(),
                        )
                    };

                    let called_from_project_root =
                        fs::path_eq_normalized(&self.calling_working_dir, &current_dir);
                    if called_from_project_root {
                        all_components()
                    } else {
                        let components_by_dir = self
                            .application
                            .component_names()
                            .filter(|component_name| {
                                let component = self.application.component(component_name);
                                let component_dir = component.component_dir();
                                fs::path_starts_with_normalized(
                                    component_dir,
                                    self.calling_working_dir.as_path(),
                                )
                            })
                            .cloned()
                            .collect::<BTreeSet<_>>();

                        if components_by_dir.is_empty() {
                            all_components()
                        } else {
                            ValidatedResult::Ok(components_by_dir)
                        }
                    }
                }
                ApplicationComponentSelectMode::All => ValidatedResult::Ok(
                    self.application
                        .component_names()
                        .map(|cn| cn.to_owned())
                        .collect(),
                ),
                ApplicationComponentSelectMode::Explicit(component_names) => {
                    let mut validation = ValidationBuilder::new();
                    for component_name in component_names {
                        if !self.application.contains_component(component_name) {
                            validation.add_error(format!(
                                "Requested component {} not found, available components: {}",
                                component_name.as_str().log_color_error_highlight(),
                                self.application
                                    .component_names()
                                    .map(|s| s.as_str().log_color_highlight())
                                    .join(", ")
                            ));
                        }
                    }
                    validation.build(BTreeSet::from_iter(component_names.iter().cloned()))
                }
            };

        let selected_component_names = validated_to_anyhow(
            "Failed to select requested components",
            selected_component_names,
            None,
        )?;

        if self.application.component_names().next().is_none() {
            log_action("Found", "no components")
        } else {
            log_action(
                "Found",
                format!(
                    "components: {}",
                    self.application
                        .component_names()
                        .map(|s| s.as_str().log_color_highlight())
                        .join(", ")
                ),
            )
        }

        {
            log_action("Selected", "components and layers:");
            let _indent = LogIndent::new();

            let padding = selected_component_names
                .iter()
                .map(|name| name.as_str().len())
                .max()
                .unwrap_or(0)
                + 1;

            for component_name in &selected_component_names {
                let component = self.application.component(component_name);
                let applied_layers = component.applied_layers();

                if applied_layers.is_empty() {
                    logln(component_name.as_str().blue().to_string());
                } else {
                    logln(format!(
                        "{} {}",
                        format!("{:<padding$}", format!("{}:", component_name.as_str())).blue(),
                        format_component_applied_layers(applied_layers)
                    ))
                }
            }
        }

        self.selected_component_names = selected_component_names;

        Ok(())
    }

    pub fn selected_component_names(&self) -> &BTreeSet<ComponentName> {
        &self.selected_component_names
    }

    pub(crate) fn ensure_component_npm_common(&self, name: &ComponentName) -> anyhow::Result<()> {
        let root = self.application.component_build_root(name);
        if root != self.application.app_root_dir()
            && let Some(language @ (GuestLanguage::TypeScript | GuestLanguage::Effect)) =
                self.application.component(name).guest_language()
            && let Some(template) =
                AppTemplateRepo::get(self.config.dev_mode)?.common_on_demand_template(language)?
        {
            // Node resolves the Rollup config's imports from its file location.
            template.generate(
                self.application.application_name(),
                root,
                &root.join(Application::on_demand_common_dir_for_language(language)),
            )?;
        }
        Ok(())
    }

    pub fn component_names(&self) -> BTreeSet<ComponentName> {
        self.application.component_names().cloned().collect()
    }

    pub async fn build(
        &self,
        build_config: &BuildConfig,
        resolved_tool_grants: &ResolvedToolGrants,
        resolved_tool_middleware_grants: &ResolvedToolMiddlewareGrants,
        environment_tools: Option<&ResolvedEnvironmentTools>,
    ) -> anyhow::Result<()> {
        let ctx =
            BuildContext::new_with_resolved_tool_grants(self, build_config, resolved_tool_grants)
                .with_tool_middleware_grants(resolved_tool_middleware_grants);
        match environment_tools {
            Some(environment_tools) => {
                build_app(&ctx.with_environment_tools(environment_tools)).await
            }
            None => build_app(&ctx).await,
        }
    }

    pub async fn custom_command(
        &self,
        build_config: &BuildConfig,
        command_name: &str,
    ) -> Result<(), CustomCommandError> {
        execute_custom_command(&BuildContext::new(self, build_config), command_name).await
    }

    pub fn clean(&self, build_config: &BuildConfig, mode: CleanMode) -> anyhow::Result<()> {
        clean_app(&BuildContext::new(self, build_config), mode)
    }

    pub fn log_dynamic_help(&self, config: &DynamicHelpSections) -> anyhow::Result<()> {
        fn print_field(label_padding: usize, label: &'static str, value: String) {
            logln(format!(
                "    {:<label_padding$} {}",
                format!("{}:", label),
                value
            ));
        }

        fn padding(labels: &[&str]) -> usize {
            labels.iter().map(|label| label.len()).max().unwrap_or(0) + 1
        }

        if config.environments() {
            static LABEL_NAME: &str = "Name";
            static LABEL_SELECTED: &str = "Selected";
            static LABEL_SERVER: &str = "Server";
            static LABEL_PRESETS: &str = "Presets";

            let label_padding = padding(&[LABEL_NAME, LABEL_SELECTED, LABEL_SERVER, LABEL_PRESETS]);
            let selected_environment_name = self.application.environment_name();

            logln(format!(
                "{}",
                "Application environments:".log_color_help_group()
            ));
            for (environment_name, environment) in self.application.environments() {
                logln(format!("  {}", environment_name.0.bold()));
                print_field(
                    label_padding,
                    LABEL_SELECTED,
                    if environment_name == selected_environment_name {
                        "yes".green().bold().to_string()
                    } else {
                        "no".red().bold().to_string()
                    },
                );
                print_field(
                    label_padding,
                    LABEL_SERVER,
                    environment
                        .to_formatted_server_context(&self.builtin_local_url)
                        .bold()
                        .to_string(),
                );
                print_field(
                    label_padding,
                    LABEL_PRESETS,
                    environment
                        .component_presets
                        .clone()
                        .into_vec()
                        .join(", ")
                        .bold()
                        .to_string(),
                );
                logln("")
            }
        }

        if config.components() {
            if self.application.has_any_component() {
                static LABEL_SOURCE: &str = "Source";
                static LABEL_SELECTED: &str = "Selected";
                static LABEL_LAYERS: &str = "Layers";
                let label_padding = padding(&[LABEL_SOURCE, LABEL_LAYERS]);

                logln(format!(
                    "{}",
                    "Application components:".log_color_help_group()
                ));
                for component_name in self.application.component_names() {
                    let component = self.application.component(component_name);
                    let selected = self.selected_component_names.contains(component_name);
                    logln(format!("  {}", component_name.as_str().bold()));
                    print_field(
                        label_padding,
                        LABEL_SELECTED,
                        if selected {
                            "yes".green().bold().to_string()
                        } else {
                            "no".red().bold().to_string()
                        },
                    );
                    print_field(
                        label_padding,
                        LABEL_SOURCE,
                        component
                            .source()
                            .display()
                            .to_string()
                            .underline()
                            .to_string(),
                    );
                    print_field(
                        label_padding,
                        LABEL_LAYERS,
                        format_component_applied_layers(component.applied_layers()),
                    );

                    logln("")
                }
            } else {
                logln("No components found in the application.\n");
            }
        }

        if config.api_deployments() {
            let environment_name = self.application.environment_name();
            let http_api_deployments = self
                .application
                .http_api_deployments(self.application.environment_name());
            match http_api_deployments {
                Some(http_api_deployments) => {
                    logln(format!(
                        "{}",
                        format!(
                            "Application API deployments for environment {}:",
                            environment_name.0
                        )
                        .log_color_help_group()
                    ));

                    for (site, deployment) in http_api_deployments {
                        logln(format!("  {}", site.to_string().log_color_highlight(),));
                        for agent_id in deployment.value.agents.keys() {
                            logln(format!("    {}", agent_id.as_str().log_color_highlight(),));
                        }
                    }
                    logln("");
                }
                None => {
                    logln(format!(
                        "No API deployments found in the application for environment {}.\n",
                        environment_name.0.log_color_highlight()
                    ));
                }
            }

            let mcp_deployments = self
                .application
                .mcp_deployments(self.application.environment_name());
            if let Some(mcp_deployments) = mcp_deployments {
                logln(format!(
                    "{}",
                    format!(
                        "Application MCP deployments for environment {}:",
                        environment_name.0
                    )
                    .log_color_help_group()
                ));

                for (site, deployment) in mcp_deployments {
                    logln(format!("  {}", site.to_string().log_color_highlight(),));
                    for agent_id in deployment.value.agents.keys() {
                        logln(format!("    {}", agent_id.as_str().log_color_highlight(),));
                    }
                }
                logln("");
            }
        }

        if config.custom_commands() {
            let commands = self.application.all_custom_commands();
            if !commands.is_empty() {
                logln(format!(
                    "{}",
                    "Application custom commands:".log_color_help_group()
                ));
                for command in commands {
                    logln(format!(
                        "  {}",
                        format!(
                            "{}{}",
                            if config.builtin_commands().contains(&command)
                                || command.starts_with(':')
                            {
                                ":"
                            } else {
                                ""
                            },
                            command
                        )
                        .bold()
                    ));
                }
                logln("")
            }
        }

        Ok(())
    }
}

fn load_app(
    application_name: WithSource<ApplicationName>,
    environments: BTreeMap<EnvironmentName, app_raw::Environment>,
    local_server: Option<WithSource<app_raw::LocalServer>>,
    component_presets: ComponentPresetSelector,
    source_mode: ApplicationSourceMode,
) -> Option<ValidatedResult<(Application, PathBuf)>> {
    // The context loads the result of a preload, which needs no search. A mode that still needs
    // one searches from the current directory, the application's root after the preload.
    let loaded_raw_apps = match source_mode {
        ApplicationSourceMode::Preloaded(loaded_raw_apps) => {
            Some(ValidatedResult::Ok(loaded_raw_apps))
        }
        ApplicationSourceMode::None => None,
        source_mode => load_raw_apps(source_mode, &fs::current_dir_lexical().unwrap()),
    };
    loaded_raw_apps.map(|loaded_raw_apps| {
        loaded_raw_apps.and_then(|loaded_raw_apps| {
            Application::from_raw_apps(
                loaded_raw_apps.app_root_dir,
                application_name,
                environments,
                local_server,
                component_presets,
                loaded_raw_apps.raw_apps,
            )
            .map(|app| (app, loaded_raw_apps.calling_working_dir))
        })
    })
}

fn preload_app(
    source_mode: ApplicationSourceMode,
    calling_working_dir: &Path,
    dev_mode: bool,
) -> Option<ValidatedResult<ApplicationPreloadResult>> {
    load_raw_apps(source_mode, calling_working_dir).map(|loaded_raw_apps| {
        loaded_raw_apps.and_then(|mut loaded_raw_apps| {
            ValidatedResult::from_result((|| -> anyhow::Result<()> {
                package_pins(&loaded_raw_apps)?;
                let development = development_manifests(&loaded_raw_apps)?;
                loaded_raw_apps.raw_apps.extend(development);
                Ok(())
            })())
            .and_then(|_| {
                let referenced_template_names =
                    Application::referenced_template_names_from_raw_apps(&loaded_raw_apps.raw_apps);

                Application::preload_from_raw_apps(loaded_raw_apps.raw_apps.as_slice())
                    .and_then(|application_preload| {
                        ValidatedResult::from_result(ensure_on_demand_commons(
                            &application_preload.application_name.value,
                            &referenced_template_names,
                            dev_mode,
                        ))
                        .map(|on_demand_common_raw_apps| {
                            (on_demand_common_raw_apps, application_preload)
                        })
                    })
                    .map(|(on_demand_common_raw_apps, application_preload)| {
                        let resolved_local_server =
                            application_preload
                                .local_server
                                .as_ref()
                                .map(|local_server| {
                                    ResolvedLocalServer::from_raw_with_source(
                                        local_server,
                                        &loaded_raw_apps.app_root_dir,
                                    )
                                });
                        let raw_apps = {
                            let mut raw_apps = loaded_raw_apps.raw_apps;
                            raw_apps.extend(on_demand_common_raw_apps);
                            raw_apps
                        };

                        ApplicationPreloadResult {
                            source_mode: ApplicationSourceMode::Preloaded(LoadedRawApps {
                                app_root_dir: loaded_raw_apps.app_root_dir,
                                root_manifest: loaded_raw_apps.root_manifest,
                                calling_working_dir: loaded_raw_apps.calling_working_dir,
                                raw_apps,
                            }),
                            loaded_with_warnings: false,
                            application_preload: Some(application_preload),
                            resolved_local_server,
                        }
                    })
            })
        })
    })
}

fn package_manifest(
    root: &Path,
    declarations: &golem_package::Declarations,
) -> anyhow::Result<app_raw::ApplicationWithSource> {
    let source = root.join("package-manifest.json");
    let declarations = serde_json::to_string(declarations)?;
    std::fs::write(&source, &declarations)?;
    Ok(app_raw::ApplicationWithSource {
        source,
        application: serde_json::from_str(&declarations)?,
        development_root: None,
    })
}

fn check_package_tool_collisions(
    apps: &[app_raw::ApplicationWithSource],
    declarations: &golem_package::Declarations,
) -> anyhow::Result<()> {
    for app in apps {
        let (tools, _) = app.application.tools.clone().into_tools_and_middleware();
        for name in tools.keys() {
            if let Some(package) = declarations.tool_package(name) {
                bail!(
                    "Tool {name:?} from package {package:?} conflicts with the local declaration tools.{name} in {}",
                    app.source.display()
                );
            }
        }
    }
    Ok(())
}

fn validate_agent_package_selectors(
    apps: &[app_raw::ApplicationWithSource],
    agent_packages: &BTreeMap<AgentTypeName, String>,
) -> anyhow::Result<()> {
    for app in apps {
        for (name, agent) in &app.application.agents {
            if let Some(package) = &agent.package {
                anyhow::ensure!(
                    agent_packages.get(name) == Some(package),
                    "Agent type {name} in {} is not exported by installed package {package:?}",
                    app.source.display()
                );
            }
        }
    }
    Ok(())
}

fn package_tool_selections(
    loaded: &mut LoadedRawApps,
    pins: &golem_package::Pins,
) -> anyhow::Result<BTreeMap<String, Vec<golem_package::ToolSelection>>> {
    use golem_package::ToolSelection;

    let path_packages = package_root(loaded)
        .into_iter()
        .flat_map(|root| &root.packages)
        .filter(|(_, entry)| matches!(entry, crate::package::PackageEntry::Path { .. }))
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    let mut selections = BTreeMap::<String, Vec<ToolSelection>>::new();
    let mut declarations = BTreeMap::new();
    for app in &mut loaded.raw_apps {
        for (name, value) in app.application.tools.extract_package_tools() {
            let package = value["package"].as_str().with_context(|| {
                format!(
                    "tools.{name}.package in {} must name a file package",
                    app.source.display()
                )
            })?;
            ensure!(
                !path_packages.contains(package),
                "tools.{name} cannot select a tool from development package {package:?}; declare and bind its tools by their local names"
            );
            ensure!(
                pins.contains_key(package),
                "tools.{name} in {} selects unknown file package {package:?}",
                app.source.display()
            );
            ensure!(
                value.as_object().is_some_and(|fields| fields
                    .keys()
                    .all(|key| matches!(key.as_str(), "package" | "tool"))),
                "tools.{name} in {} must use only package and tool selectors",
                app.source.display()
            );
            if let Some((previous_package, source)) =
                declarations.insert(name.clone(), (package.to_owned(), app.source.as_path()))
            {
                bail!(
                    "Tool {name:?} from package {previous_package:?} in {} conflicts with package {package:?} in {}",
                    source.display(),
                    app.source.display()
                );
            }
            let published = value
                .get("tool")
                .map(|tool| {
                    tool.as_str()
                        .context("Expected a published tool name")?
                        .parse()
                        .map_err(anyhow::Error::msg)
                })
                .transpose()
                .with_context(|| format!("tools.{name}.tool in {}", app.source.display()))?;
            selections
                .entry(package.to_owned())
                .or_default()
                .push(ToolSelection {
                    published,
                    local: name.parse().map_err(anyhow::Error::msg)?,
                });
        }
    }
    Ok(selections)
}

fn package_pins(
    loaded: &LoadedRawApps,
) -> anyhow::Result<(
    Option<golem_package::PackageIdentity>,
    golem_package::Pins,
    golem_package::Pins,
)> {
    let mut publisher = None;
    let mut pins = golem_package::Pins::new();
    for app in &loaded.raw_apps {
        let raw = &app.application;
        if app.source == loaded.root_manifest {
            publisher = raw.package.clone();
            pins.extend(raw.packages.iter().filter_map(|(name, entry)| match entry {
                crate::package::PackageEntry::File {
                    source,
                    version,
                    digest,
                    ..
                } => Some((
                    name.clone(),
                    golem_package::FilePin {
                        source: source.clone(),
                        version: version.clone(),
                        digest: digest.clone(),
                    },
                )),
                crate::package::PackageEntry::Path { .. } => None,
            }));
        } else if raw.package.is_some() || !raw.packages.is_empty() {
            anyhow::bail!(
                "Package declarations are only allowed in the root manifest, not {}",
                app.source.display()
            );
        }
    }
    if let Some(identity) = &publisher {
        golem_package::validate_identity(identity)?;
    }
    let lock = crate::package::load_lock(&loaded.app_root_dir)?;
    if let Some(root) = package_root(loaded) {
        for (name, entry) in &root.packages {
            ensure!(
                !matches!(entry, crate::package::PackageEntry::Path { .. })
                    || !lock.contains_key(name),
                "Development package {name:?} must not have a lock entry; remove packages.{name} from golem.lock"
            );
        }
    }
    golem_package::check_lock(&pins, &lock)?;
    Ok((publisher, pins, lock))
}

fn package_root(loaded: &LoadedRawApps) -> Option<&app_raw::Application> {
    loaded
        .raw_apps
        .iter()
        .find(|app| app.source == loaded.root_manifest)
        .map(|app| &app.application)
}

fn development_manifests(
    loaded: &LoadedRawApps,
) -> anyhow::Result<Vec<app_raw::ApplicationWithSource>> {
    let Some(root) = package_root(loaded) else {
        return Ok(Vec::new());
    };
    if !root
        .packages
        .values()
        .any(|entry| matches!(entry, crate::package::PackageEntry::Path { .. }))
    {
        return Ok(Vec::new());
    }
    let mut sources = loaded
        .raw_apps
        .iter()
        .map(|app| std::fs::canonicalize(&app.source))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    let mut manifests = Vec::new();
    for (name, entry) in &root.packages {
        let crate::package::PackageEntry::Path { path } = entry else {
            continue;
        };
        let directory = loaded.app_root_dir.join(path);
        let manifest = directory.join(DEFAULT_CONFIG_FILE_NAME);
        (|| -> anyhow::Result<()> {
            ensure!(
                !path.as_os_str().is_empty() && manifest.is_file(),
                "Expected a directory containing golem.yaml"
            );
            let collected = validated_to_anyhow(
                "Failed to collect development package manifests",
                collect_sources_from(&directory, Some(&manifest)).context("No package manifest")?,
                None,
            )?;
            validated_to_anyhow(
                "Invalid development package manifest version",
                validate_manifest_versions(&collected.sources),
                None,
            )?;
            let producer_name =
                app_raw::ApplicationWithSource::from_yaml_file(&collected.root_manifest)?
                    .application
                    .app;
            for source in collected.sources {
                ensure!(
                    sources.insert(std::fs::canonicalize(&source)?),
                    "Manifest {} is already loaded",
                    source.display()
                );
                let mut raw = app_raw::ApplicationWithSource::from_yaml_file(&source)?.application;
                ensure!(
                    raw.packages.is_empty(),
                    "Development package dependencies are not supported ({})",
                    source.display()
                );
                if let Some((tool, _)) = raw.tools.extract_package_tools().next() {
                    bail!(
                        "Development package tool {tool} cannot select a file package ({})",
                        source.display()
                    );
                }
                if let Some((agent, _)) =
                    raw.agents.iter().find(|(_, agent)| agent.package.is_some())
                {
                    bail!(
                        "Development package agent {agent} cannot select a file package ({})",
                        source.display()
                    );
                }
                if source == collected.root_manifest {
                    let identity = raw
                        .package
                        .as_ref()
                        .context("Declare package identity in golem.yaml")?;
                    golem_package::validate_identity(identity)?;
                    ensure!(
                        identity.name == *name,
                        "Package declares name {:?}, not {name:?}",
                        identity.name
                    );
                }
                manifests.push(app_raw::ApplicationWithSource {
                    source,
                    development_root: Some((
                        fs::normalize_path_lexically(&directory),
                        producer_name.clone(),
                    )),
                    application: app_raw::Application {
                        component_templates: raw.component_templates,
                        components: raw.components,
                        agents: raw.agents,
                        tools: raw.tools,
                        ..Default::default()
                    },
                });
            }
            Ok(())
        })()
        .with_context(|| format!("Development package {name:?} at {}", directory.display()))?;
    }
    Ok(manifests)
}

fn ensure_on_demand_commons(
    application_name: &ApplicationName,
    referenced_template_names: &BTreeSet<String>,
    dev_mode: bool,
) -> anyhow::Result<Vec<app_raw::ApplicationWithSource>> {
    let app_template_repo = AppTemplateRepo::get(dev_mode)?;

    let mut on_demand_raw_apps = Vec::new();

    for language in app_template_repo.builtin_template_languages(referenced_template_names) {
        if let Some(template) = app_template_repo.common_on_demand_template(language)? {
            let app_dir = std::env::current_dir()?;
            let target_dir = Application::on_demand_common_dir_for_language(template.0.language);
            template.generate(application_name, &app_dir, &target_dir)?;
            let golem_yaml_path = target_dir.join("golem.yaml");
            if golem_yaml_path.exists() {
                on_demand_raw_apps.push(app_raw::ApplicationWithSource::from_yaml_file(
                    &golem_yaml_path,
                )?);
            }
        }
    }

    Ok(on_demand_raw_apps)
}

fn load_raw_apps(
    source_mode: ApplicationSourceMode,
    calling_working_dir: &Path,
) -> Option<ValidatedResult<LoadedRawApps>> {
    let load = |root_source: Option<&Path>| -> Option<ValidatedResult<LoadedRawApps>> {
        collect_sources_and_switch_to_app_root(calling_working_dir, root_source).map(
            |collected_sources| {
                collected_sources.and_then(|collected_sources| {
                    validate_manifest_versions(&collected_sources.sources).and_then(|()| {
                        collected_sources
                            .sources
                            .into_iter()
                            .map(|source| {
                                ValidatedResult::from_result(
                                    app_raw::ApplicationWithSource::from_yaml_file(&source),
                                )
                            })
                            .collect::<ValidatedResult<Vec<_>>>()
                            .map(|raw_apps| LoadedRawApps {
                                app_root_dir: collected_sources.app_root_dir,
                                root_manifest: collected_sources.root_manifest,
                                calling_working_dir: collected_sources.calling_working_dir,
                                raw_apps,
                            })
                    })
                })
            },
        )
    };

    match source_mode {
        ApplicationSourceMode::Automatic => load(None),
        ApplicationSourceMode::ByRootManifest(root_manifest) => load(Some(&root_manifest)),
        ApplicationSourceMode::Preloaded(loaded_raw_apps) => {
            Some(ValidatedResult::Ok(loaded_raw_apps))
        }
        ApplicationSourceMode::None => None,
    }
}

struct CollectedSources {
    app_root_dir: PathBuf,
    root_manifest: PathBuf,
    calling_working_dir: PathBuf,
    sources: BTreeSet<PathBuf>,
}

fn collect_sources_and_switch_to_app_root(
    calling_working_dir: &Path,
    root_manifest: Option<&Path>,
) -> Option<ValidatedResult<CollectedSources>> {
    collect_sources_from(calling_working_dir, root_manifest).map(|collected_sources| {
        collected_sources.inspect(|collected_sources| {
            std::env::set_current_dir(&collected_sources.app_root_dir)
                .expect("Failed to set current dir for config parent");
        })
    })
}

fn collect_sources_from(
    calling_working_dir: &Path,
    root_manifest: Option<&Path>,
) -> Option<ValidatedResult<CollectedSources>> {
    log_action("Collecting", "application manifests");
    let _indent = LogIndent::new();

    fn collect_by_main_source(
        source: &Path,
    ) -> Option<ValidatedResult<(BTreeSet<PathBuf>, PathBuf, PathBuf)>> {
        let source_dir =
            fs::parent_or_err(source).expect("Failed to get parent dir for config parent");

        let includes = includes_from_yaml_file(source);
        if includes.is_empty() {
            Some(ValidatedResult::Ok((
                BTreeSet::from([source.to_path_buf()]),
                source_dir.to_path_buf(),
                source.to_path_buf(),
            )))
        } else {
            Some(
                ValidatedResult::from_result(fs::compile_and_collect_globs(
                    source_dir, source_dir, &includes,
                ))
                .map(|mut sources| {
                    sources.insert(0, source.to_path_buf());
                    (
                        sources.into_iter().collect(),
                        source_dir.to_path_buf(),
                        source.to_path_buf(),
                    )
                }),
            )
        }
    }

    let sources_and_root_dir = match root_manifest {
        None => match find_main_source_from(calling_working_dir) {
            Some(source) => collect_by_main_source(&source),
            None => None,
        },
        Some(source) => {
            let source = fs::absolute_lexical_path_from_base_dir(source, calling_working_dir);
            collect_by_main_source(&source)
        }
    };

    sources_and_root_dir.map(|sources_and_root_dir| {
        sources_and_root_dir
            .inspect(|sources_and_root_dir| {
                let sources = &sources_and_root_dir.0;
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
            .map(|sources_and_root_dir| CollectedSources {
                app_root_dir: sources_and_root_dir.1,
                root_manifest: sources_and_root_dir.2,
                calling_working_dir: calling_working_dir.to_path_buf(),
                sources: sources_and_root_dir.0,
            })
    })
}

pub(crate) fn find_main_source_from(start_dir: &Path) -> Option<PathBuf> {
    let mut current_dir = start_dir.to_path_buf();
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

pub fn validated_to_anyhow<T>(
    message: &str,
    result: ValidatedResult<T>,
    mark_had_warns: Option<fn(T) -> T>,
) -> anyhow::Result<T> {
    match result {
        ValidatedResult::Ok(value) => Ok(value),
        ValidatedResult::OkWithWarns(value, warns) => {
            logln("");
            for line in format_warns(&warns).lines() {
                logln(line);
            }
            Ok(match mark_had_warns {
                Some(mark_had_warns) => mark_had_warns(value),
                None => value,
            })
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => Err(anyhow!(AppValidationError {
            message: message.to_string(),
            warns,
            errors,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::ApplicationContext;
    use crate::model::app::{Application, ApplicationSourceMode, ResolvedLocalServer};
    use test_r::test;

    #[test]
    fn package_selection_is_manifest_only() {
        use crate::model::app_raw::Application;

        let pin = format!(
            "source: tools.tar, version: 1.0.0, digest: sha256:{}",
            "a".repeat(64)
        );
        for entry in [format!("{{{pin}}}"), "{path: ../tools}".into()] {
            let manifest = format!("packages: {{tools: {entry}}}");
            assert!(
                Application::from_yaml_str(&manifest).is_ok_and(|app| app.packages.len() == 1),
                "{manifest}"
            );
        }
        for entry in [
            format!("{{{pin}, path: ../tools}}"),
            format!("{{{pin}, tool: echo}}"),
            "{path: ../tools, digest: invalid}".into(),
        ] {
            let manifest = format!("packages: {{tools: {entry}}}");
            assert!(Application::from_yaml_str(&manifest).is_err(), "{manifest}");
        }
    }

    #[test]
    async fn package_selection_reports_conflicting_and_unknown_tools() {
        use crate::model::app_raw::ApplicationWithSource;
        use golem_package::*;
        use std::collections::BTreeMap;
        use std::fs;

        let app = tempfile::tempdir().unwrap();
        let unpack = app.path().join("unpack");
        let mut pins = Pins::new();
        for (name, tools) in [
            ("first", vec!["shared", "other"]),
            ("second", vec!["shared"]),
        ] {
            let mut blobs = BTreeMap::new();
            let wasm = add_blob(&mut blobs, WASM, vec![0]).digest;
            let spec = PackageConfig {
                application: name.into(),
                package: PackageIdentity {
                    name: name.into(),
                    version: "1.0.0".into(),
                    calls: Default::default(),
                },
                components: BTreeMap::from([(
                    format!("{name}:tool").parse().unwrap(),
                    PackageComponent {
                        wasm,
                        metadata:
                            golem_common::model::agent::extraction::ExtractedComponentMetadata {
                                agent_types: vec![],
                                tools: vec![],
                                tool_middlewares: vec![],
                            },
                        config_schema: Default::default(),
                        provision: Default::default(),
                        agents: BTreeMap::new(),
                        tools: tools
                            .into_iter()
                            .map(|tool| (tool.parse().unwrap(), PackageProvision::default()))
                            .collect(),
                        tool_middlewares: BTreeMap::new(),
                    },
                )]),
            };
            let (digest, bytes) = pack(&spec, &blobs).unwrap();
            let source = format!("{name}.tar");
            fs::write(app.path().join(&source), bytes).unwrap();
            pins.insert(
                name.to_owned(),
                FilePin {
                    source,
                    version: "1.0.0".into(),
                    digest,
                },
            );
        }
        let mut effects = crate::package::LocalEffects {
            app_root: app.path(),
            cache_root: app.path().join("cache"),
        };
        let mut expected = BTreeMap::new();
        for (first, fragments) in [
            ("shared", ["shared", "first", "second"]),
            ("missing", ["missing", "shared", "other"]),
        ] {
            expected.insert(
                "first".into(),
                vec![ToolSelection {
                    published: Some(first.parse().unwrap()),
                    local: first.parse().unwrap(),
                }],
            );
            let unpack = app.path().join(first);
            let error = expand(&pins, &pins, &expected, &unpack, &mut effects)
                .await
                .unwrap_err()
                .to_string();
            assert!(
                fragments.iter().all(|fragment| error.contains(fragment)),
                "{error}"
            );
        }
        expected.insert(
            "first".into(),
            vec![ToolSelection {
                published: Some("other".parse().unwrap()),
                local: "other".parse().unwrap(),
            }],
        );
        let expanded = expand(&pins, &pins, &expected, &unpack, &mut effects)
            .await
            .unwrap();
        let manifest = ApplicationWithSource::from_yaml_string(
            app.path().join("local.yaml"),
            "tools: {shared: {}}",
        )
        .unwrap();
        let error = super::check_package_tool_collisions(
            std::slice::from_ref(&manifest),
            &expanded.declarations,
        )
        .unwrap_err()
        .to_string();
        assert!(
            ["shared", "second", "local.yaml"]
                .iter()
                .all(|fragment| error.contains(fragment)),
            "{error}"
        );
        expected.insert(
            "second".into(),
            vec![ToolSelection {
                published: None,
                local: "local-shared".parse().unwrap(),
            }],
        );
        let expanded = expand(
            &pins,
            &pins,
            &expected,
            &app.path().join("renamed"),
            &mut effects,
        )
        .await
        .unwrap();
        super::check_package_tool_collisions(&[manifest], &expanded.declarations).unwrap();
        assert_eq!(
            expanded.declarations.tool_package("local-shared"),
            Some("second")
        );
        assert_eq!(expanded.declarations.tool_package("shared"), None);
    }

    #[test]
    fn agent_package_selector_requires_the_published_type_in_that_package() {
        use super::{AgentTypeName, BTreeMap, app_raw, validate_agent_package_selectors};

        let packages = BTreeMap::from([(AgentTypeName("SupportCall".into()), "support".into())]);
        for (agent, package, valid) in [
            ("SupportCall", "support", true),
            ("SupportCall", "unknown", false),
            ("RenamedSupport", "support", false),
        ] {
            let app = app_raw::ApplicationWithSource {
                source: "golem.yaml".into(),
                application: app_raw::Application::from_yaml_str(&format!(
                    "agents:\n  {agent}:\n    package: {package}\n"
                ))
                .unwrap(),
                development_root: None,
            };
            let result = validate_agent_package_selectors(&[app], &packages);
            if valid {
                result.unwrap();
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains(agent) && error.contains(package), "{error}");
            }
        }
    }

    #[test]
    async fn installed_declarations_resolve_like_local_declarations() {
        use crate::model::app::ComponentPresetSelector;
        use crate::model::app_raw::ApplicationWithSource;
        use golem_common::model::agent::{AgentMode, Snapshotting};
        use golem_common::schema::agent::{
            AgentConstructorSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
        };
        use golem_package::*;
        use serde_json::json;
        use std::collections::{BTreeMap, BTreeSet};
        use std::fs;

        let app = tempfile::tempdir().unwrap();
        let unpack = app.path().join("unpack");
        let mut blobs = BTreeMap::new();
        let wasm = add_blob(&mut blobs, WASM, vec![0]).digest;
        let file = add_blob(
            &mut blobs,
            INITIAL_FILES,
            write_tar(&BTreeMap::from([("00000000".into(), b"sample")])).unwrap(),
        )
        .digest;
        let component_name = "sample:shared".parse().unwrap();
        let agent_name: super::AgentTypeName = "SampleAgent".parse().unwrap();
        let tool_name = "example".parse().unwrap();
        let middleware_name = "policy".parse().unwrap();
        let spec = PackageConfig {
            application: "publisher".into(),
            package: PackageIdentity { name: "sample".into(), version: "1.0.0".into(), calls: BTreeSet::new() },
            components: BTreeMap::from([(component_name, PackageComponent {
                wasm: wasm.clone(),
                metadata: golem_common::model::agent::extraction::ExtractedComponentMetadata {
                    agent_types: vec![AgentTypeSchema {
                        kind: AgentTypeKind::Regular,
                        type_name: agent_name.clone(),
                        description: String::new(),
                        source_language: "rust".into(),
                        schema: golem_common::schema::SchemaGraph::empty(),
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
                        snapshotting: Snapshotting::Disabled(golem_schema::Empty {}),
                        config: vec![],
                    }],
                    tools: vec![],
                    tool_middlewares: vec![],
                },
                config_schema: Default::default(),
                provision: PackageProvision {
                    env: BTreeMap::from([("FROM_COMPONENT".into(), "default".into())]),
                    files: Some(PackageInitialFiles {
                        layer: file.clone(),
                        entries: BTreeMap::from([("00000000".into(), PackageInitialFile {
                            target: serde_json::from_value(json!("/hello.txt")).unwrap(),
                            permissions: golem_common::model::component::AgentFilePermissions::ReadOnly,
                        })]),
                    }),
                    ..Default::default()
                },
                agents: BTreeMap::from([(agent_name, PackageProvision { config: json!({"agentValue": 7}), ..Default::default() })]),
                tools: BTreeMap::from([(tool_name, PackageProvision { config: json!({"toolValue": true}), ..Default::default() })]),
                tool_middlewares: BTreeMap::from([(middleware_name, PackageProvision { config: json!({"middlewareValue": "strict"}), ..Default::default() })]),
            })]),
        };
        let (digest, bytes) = pack(&spec, &blobs).unwrap();
        fs::write(app.path().join("sample.tar"), bytes).unwrap();
        let pins = Pins::from([(
            "sample".into(),
            FilePin {
                source: "sample.tar".into(),
                version: "1.0.0".into(),
                digest,
            },
        )]);
        let mut effects = crate::package::LocalEffects {
            app_root: app.path(),
            cache_root: app.path().join("cache"),
        };
        let expanded = expand(&pins, &pins, &BTreeMap::new(), &unpack, &mut effects)
            .await
            .unwrap();
        let installed = super::package_manifest(&unpack, &expanded.declarations).unwrap();
        let local_source = unpack.join("golem.yaml");
        fs::write(
            &local_source,
            format!(
                r#"
components:
  sample:shared:
    componentWasm: sample/wasm/{}
    env: {{ FROM_COMPONENT: default }}
    files:
      - sourcePath: sample/initial-files/{}/00000000
        targetPath: /hello.txt
agents:
  SampleAgent:
    config: {{ agentValue: 7 }}
tools:
  example:
    component: sample:shared
    config: {{ toolValue: true }}
  middleware:
    policy:
      component: sample:shared
      config: {{ middlewareValue: strict }}
"#,
                &wasm[7..],
                &file[7..]
            ),
        )
        .unwrap();
        let local = ApplicationWithSource::from_yaml_file(&local_source).unwrap();
        let root = app.path().join("golem.yaml");
        fs::write(
            &root,
            "app: installer\nenvironments:\n  local: { server: local }\n",
        )
        .unwrap();
        let resolve = |declarations| {
            let apps = vec![
                ApplicationWithSource::from_yaml_file(&root).unwrap(),
                declarations,
            ];
            let (preload, _, errors) = Application::preload_from_raw_apps(&apps).into_product();
            assert!(errors.is_empty(), "{errors:?}");
            let preload = preload.unwrap();
            let (app, _, errors) = Application::from_raw_apps(
                app.path().into(),
                preload.application_name,
                preload.environments,
                preload.local_server,
                ComponentPresetSelector {
                    environment: "local".parse().unwrap(),
                    presets: vec![],
                },
                apps,
            )
            .into_product();
            assert!(errors.is_empty(), "{errors:?}");
            app.unwrap()
        };
        let installed = resolve(installed);
        let local = resolve(local);
        let component_name: golem_common::model::component::ComponentName =
            "sample:shared".parse().unwrap();
        let agent_name: golem_common::model::agent::AgentTypeName = "SampleAgent".parse().unwrap();
        let snapshot = |app: Application| {
            let component = app.component(&component_name);
            let agents = app
                .resolve_agents(&BTreeMap::from([(
                    agent_name.clone(),
                    component_name.clone(),
                )]))
                .unwrap();
            json!({
                "wasm": component.wasm(), "env": component.env(), "files": component.files(),
                "agent": agents.agent(&agent_name).unwrap().config(),
                "tools": app.tool_declarations().iter().map(|(name, decl)| (name, &decl.value)).collect::<BTreeMap<_, _>>(),
                "middleware": app.tool_middleware_declarations().iter().map(|(name, decl)| (name, &decl.value)).collect::<BTreeMap<_, _>>(),
            })
        };
        assert_eq!(snapshot(installed), snapshot(local));
    }

    #[test]
    fn automatic_manifest_upgrade_applies_before_load_without_changing_cwd() {
        let original_dir = std::env::current_dir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let app_dir = temp_dir.path().join("app");
        std::fs::create_dir(&app_dir).unwrap();
        let manifest = app_dir.join("golem.yaml");
        std::fs::write(
            &manifest,
            r#"# $schema: https://schema.golem.cloud/app/golem/1.5.0/golem.schema.json
manifestVersion: 1.5.0
app: demo
"#,
        )
        .unwrap();

        let nested_dir = app_dir.join("src");
        std::fs::create_dir(&nested_dir).unwrap();

        let result = ApplicationContext::plan_and_apply_manifest_upgrades_before_load_from(
            ApplicationSourceMode::Automatic,
            true,
            &nested_dir,
        );

        let current_dir = std::env::current_dir().unwrap();
        result.unwrap();
        assert_eq!(
            std::fs::canonicalize(current_dir).unwrap(),
            std::fs::canonicalize(original_dir).unwrap()
        );

        let upgraded = std::fs::read_to_string(manifest).unwrap();
        assert!(upgraded.contains("manifestVersion: 2.0.0"));
        assert!(upgraded.contains(&format!(
            "/{}/golem.schema.json",
            crate::manifest_schema_version!()
        )));
    }

    #[test]
    fn preload_resolves_local_server_paths_against_declaring_manifest_dir() {
        let original_dir = std::env::current_dir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let app_dir = temp_dir.path().join("app");
        let config_dir = app_dir.join("config");
        std::fs::create_dir(&app_dir).unwrap();
        std::fs::create_dir(&config_dir).unwrap();
        let manifest = app_dir.join("golem.yaml");
        let local_server_manifest = config_dir.join("local-server.yaml");
        std::fs::write(
            &manifest,
            r#"
manifestVersion: 2.0.0
app: demo
includes:
  - config/local-server.yaml
environments:
  local:
    server: local
"#,
        )
        .unwrap();
        std::fs::write(
            &local_server_manifest,
            r#"
manifestVersion: 2.0.0
localServer:
  portsFile: .golem/ports.json
  dataDir: .golem/data
  agentFilesystemRoot: .golem/agents
"#,
        )
        .unwrap();

        let result = super::collect_sources_from(&original_dir, Some(&manifest))
            .expect("manifest should be found");
        let (collected_sources, warns, errors) = result.into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));

        let collected_sources = collected_sources.unwrap();
        let raw_apps = collected_sources
            .sources
            .iter()
            .map(|source| crate::model::app_raw::ApplicationWithSource::from_yaml_file(source))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let (application_preload, warns, errors) =
            Application::preload_from_raw_apps(raw_apps.as_slice()).into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));

        let application_preload = application_preload.unwrap();
        let raw_local_server = application_preload
            .local_server
            .as_ref()
            .unwrap()
            .value
            .clone();

        assert_eq!(
            raw_local_server.ports_file,
            Some(std::path::PathBuf::from(".golem/ports.json"))
        );
        assert_eq!(
            raw_local_server.data_dir,
            Some(std::path::PathBuf::from(".golem/data"))
        );
        assert_eq!(
            raw_local_server.agent_filesystem_root,
            Some(std::path::PathBuf::from(".golem/agents"))
        );

        let resolved_local_server = ResolvedLocalServer::from_raw_with_source(
            application_preload.local_server.as_ref().unwrap(),
            &collected_sources.app_root_dir,
        );

        assert_eq!(
            resolved_local_server.ports_file,
            Some(config_dir.join(".golem/ports.json"))
        );
        assert_eq!(
            resolved_local_server.data_dir,
            Some(config_dir.join(".golem/data"))
        );
        assert_eq!(
            resolved_local_server.agent_filesystem_root,
            Some(config_dir.join(".golem/agents"))
        );
    }
}
