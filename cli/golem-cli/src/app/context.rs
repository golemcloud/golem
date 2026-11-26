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

use crate::app::build::clean::clean_app;
use crate::app::build::command::execute_custom_command;
use crate::app::build::{BuildExecution, build_app};
use crate::app::error::{AppValidationError, CustomCommandError, format_warns};
use crate::app::manifest_upgrade::{ManifestUpgradeStep, plan_manifest_upgrade_steps};
use crate::app::manifest_version::validate_manifest_versions;
use crate::app::template::AppTemplateRepo;
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
use crate::validation::{ValidatedResult, ValidationBuilder};
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentPlanAmbientToolEntry;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::environment_tool_grant::EnvironmentToolGrantWithDetails;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

impl ResolvedEnvironmentTools {
    pub fn result_count(&self) -> usize {
        self.ambient_tools
            .len()
            .saturating_add(self.mcp_tools.len())
            .saturating_add(self.mcp_diagnostics.len())
    }

    pub fn tool(
        &self,
        name: &golem_common::model::tool::ToolName,
    ) -> anyhow::Result<ResolvedEnvironmentTool<'_>> {
        if let Some(tool) = self.ambient_tools.iter().find(|tool| tool.name == *name) {
            return Ok(ResolvedEnvironmentTool::Ambient(tool));
        }
        if let Some(tool) = self
            .mcp_tools
            .iter()
            .find(|tool| tool.definition.name() == Some(name.as_str()))
        {
            return Ok(ResolvedEnvironmentTool::Mcp(tool));
        }
        if let Some(diagnostic) = self
            .mcp_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.canonical_name == name.as_str())
        {
            bail!(
                "Environment tool '{name}' was rejected by MCP import {} (upstream '{}'): {}",
                diagnostic.import_index,
                diagnostic.upstream_name,
                diagnostic.reason
            );
        }
        bail!(
            "Environment tool dependency '{name}' was not found among the selected environment's ambient tools or MCP imports"
        )
    }
}

pub struct BuildContext<'a> {
    application_context: &'a ApplicationContext,
    build_config: &'a BuildConfig,
    execution: &'a BuildExecution,
}

impl<'a> BuildContext<'a> {
    pub(crate) fn execution(&self) -> &BuildExecution {
        self.execution
    }

    pub fn extracted_metadata(&self) -> BTreeMap<ComponentName, ExtractedComponentMetadata> {
        self.execution.extracted_metadata()
    }

    pub fn extracted_component_metadata(
        &self,
        name: &ComponentName,
    ) -> Option<ExtractedComponentMetadata> {
        self.execution.component_metadata(name)
    }

    pub fn store_extracted_component_metadata(
        &self,
        name: ComponentName,
        metadata: ExtractedComponentMetadata,
    ) {
        self.execution.store_component_metadata(name, metadata);
    }

    pub fn remove_extracted_component_metadata(&self, name: &ComponentName) {
        self.execution.remove_component_metadata(name);
    }

    pub fn new(
        application_context: &'a ApplicationContext,
        build_config: &'a BuildConfig,
        execution: &'a BuildExecution,
    ) -> Self {
        Self {
            application_context,
            build_config,
            execution,
        }
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
        self.execution
            .resolved_middleware_grants()
            .get(&reference)
            .map(|grant| grant.release.definition.clone())
            .ok_or_else(|| anyhow!("Tool middleware '{name}' has no resolved build preparation"))
    }

    pub fn mcp_tools(&self) -> &[golem_client::model::McpResolvedTool] {
        self.execution
            .environment_tools()
            .map(|tools| tools.mcp_tools.as_slice())
            .unwrap_or_default()
    }

    pub fn ambient_tools(&self) -> &[DeploymentPlanAmbientToolEntry] {
        self.execution
            .environment_tools()
            .map(|tools| tools.ambient_tools.as_slice())
            .unwrap_or_default()
    }

    pub fn environment_tools_id(&self) -> Option<EnvironmentId> {
        self.execution
            .environment_tools()
            .map(|tools| tools.environment_id)
    }

    pub fn environment_tool(
        &self,
        name: &golem_common::model::tool::ToolName,
    ) -> anyhow::Result<ResolvedEnvironmentTool<'_>> {
        self.execution
            .environment_tools()
            .ok_or_else(|| {
                anyhow!("Environment tool dependency '{name}' has no resolved build preparation")
            })?
            .tool(name)
    }

    pub fn environment_tool_diagnostic(
        &self,
        name: &golem_common::model::tool::ToolName,
    ) -> Option<&ResolvedMcpDiagnostic> {
        self.execution
            .environment_tools()
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
        self.execution
            .resolved_tool_grants()
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
    tools_with_ensured_common_deps: tokio::sync::RwLock<HashMap<String, bool>>,
}

impl Default for ToolsWithEnsuredCommonDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolsWithEnsuredCommonDeps {
    pub fn new() -> ToolsWithEnsuredCommonDeps {
        ToolsWithEnsuredCommonDeps {
            tools_with_ensured_common_deps: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    pub async fn ensure_common_deps_for_tool_once(
        &self,
        tool: &str,
        ensure: impl AsyncFnOnce() -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        if let Some(changed) = self.tools_with_ensured_common_deps.read().await.get(tool) {
            return Ok(*changed);
        }

        let mut lock = self.tools_with_ensured_common_deps.write().await;
        if let Some(changed) = lock.get(tool) {
            return Ok(*changed);
        }

        let result = ensure().await;

        if let Ok(changed) = &result {
            lock.insert(tool.to_string(), *changed);
        }

        result
    }
}

pub struct ApplicationPreloadResult {
    pub source_mode: ApplicationSourceMode,
    pub loaded_with_warnings: bool,
    pub application_preload: Option<ApplicationPreload>,
    pub resolved_local_server: Option<ResolvedLocalServer>,
    pub load_plan: ApplicationLoadPlan,
}

#[derive(Default)]
pub struct ApplicationLoadPlan {
    upgrades: Vec<ManifestUpgradeStep>,
    commons: Vec<(
        crate::app::template::AppTemplateCommonOnDemand,
        ApplicationName,
        PathBuf,
        PathBuf,
    )>,
}

impl ApplicationLoadPlan {
    pub fn is_empty(&self) -> bool {
        self.upgrades.is_empty() && self.commons.is_empty()
    }

    pub fn apply(&self, yes: bool) -> anyhow::Result<()> {
        ApplicationContext::apply_manifest_upgrades(&self.upgrades, yes)?;
        for (template, name, app_dir, target_dir) in &self.commons {
            template.generate(name, app_dir, target_dir)?;
        }
        Ok(())
    }
}

pub struct ApplicationContext {
    loaded_with_warnings: bool,
    config: ApplicationConfig,
    application: Application,
    pub(crate) js_compilation_cache_dir: PathBuf,
    component_metadata: crate::app::component_metadata::ComponentMetadataRegistry,
    publisher_package: Option<golem_package::PackageIdentity>,
    installed_components: BTreeMap<ComponentName, golem_package::InstalledComponent>,
    installed_package_calls: BTreeMap<String, BTreeSet<AgentTypeName>>,
    package_preparation: Option<crate::package::PackagePreparation>,
    prepared_packages: Option<std::sync::Arc<crate::package::PreparedPackages>>,
    calling_working_dir: PathBuf,
    selected_component_names: BTreeSet<ComponentName>,
    builtin_local_url: url::Url,
    tools_with_ensured_common_deps: ToolsWithEnsuredCommonDeps,
}

impl ApplicationContext {
    pub(crate) fn installed_package_calls(&self) -> &BTreeMap<String, BTreeSet<AgentTypeName>> {
        &self.installed_package_calls
    }

    #[cfg(test)]
    pub(crate) fn with_test_packages(
        mut self,
        publisher: golem_package::PackageIdentity,
        calls: BTreeMap<String, BTreeSet<AgentTypeName>>,
        components: BTreeMap<ComponentName, golem_package::InstalledComponent>,
    ) -> Self {
        self.publisher_package = Some(publisher);
        self.installed_package_calls = calls;
        self.installed_components = components;
        self
    }

    #[cfg(test)]
    pub(crate) fn for_test(application: Application) -> Self {
        Self {
            calling_working_dir: application.app_root_dir().to_path_buf(),
            selected_component_names: application.component_names().cloned().collect(),
            js_compilation_cache_dir: application
                .app_root_dir()
                .join(".golem/js-compilation-cache"),
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
            installed_components: BTreeMap::new(),
            installed_package_calls: BTreeMap::new(),
            package_preparation: None,
            prepared_packages: None,
            builtin_local_url: "http://localhost:9881".parse().unwrap(),
            tools_with_ensured_common_deps: ToolsWithEnsuredCommonDeps::new(),
        }
    }

    pub async fn plan_and_apply_manifest_upgrades_before_load(
        source_mode: ApplicationSourceMode,
        yes: bool,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Option<std::fs::File>> {
        Self::plan_and_apply_manifest_upgrades_before_load_from(
            source_mode,
            yes,
            &fs::current_dir_lexical()?,
            cancellation,
        )
        .await
    }

    async fn plan_and_apply_manifest_upgrades_before_load_from(
        source_mode: ApplicationSourceMode,
        yes: bool,
        calling_working_dir: &Path,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Option<std::fs::File>> {
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
            return Ok(None);
        };
        let collected_sources = validated_to_anyhow(
            "Failed to collect application manifests for automatic upgrade",
            collected_sources,
            None,
        )?;
        let steps = plan_manifest_upgrade_steps(&collected_sources.sources)?;
        if steps.is_empty() {
            return Ok(None);
        }
        Self::confirm_manifest_upgrades(&steps, yes)?;
        let operation = fs::lock_file(
            &Application::operation_lock_path(&collected_sources.app_root_dir),
            cancellation,
        )
        .await?;
        Self::apply_confirmed_manifest_upgrades(&steps)?;
        Ok(Some(operation))
    }

    fn apply_manifest_upgrades(steps: &[ManifestUpgradeStep], yes: bool) -> anyhow::Result<()> {
        if steps.is_empty() {
            return Ok(());
        }
        Self::confirm_manifest_upgrades(steps, yes)?;
        Self::apply_confirmed_manifest_upgrades(steps)
    }

    fn confirm_manifest_upgrades(steps: &[ManifestUpgradeStep], yes: bool) -> anyhow::Result<()> {
        let _output = LogOutput::new(Output::Stderr);
        logln("");
        log_action("Planned", "automatic application manifest upgrade changes");
        let _indent = DecoratedIndent::new_primary(Format::Text);
        for step in steps {
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
        Ok(())
    }

    fn apply_confirmed_manifest_upgrades(steps: &[ManifestUpgradeStep]) -> anyhow::Result<()> {
        let _output = LogOutput::new(Output::Stderr);
        // Check all originals after confirmation, before the first write.
        for step in steps {
            anyhow::ensure!(
                fs::read_to_string(&step.path)? == step.current,
                "Manifest {} changed after inspection; run the build again",
                step.path.display()
            );
        }
        logln("");
        log_action("Applying", "application manifest upgrades");
        let _indent = LogIndent::new();
        for step in steps {
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
        defer_writes: bool,
    ) -> anyhow::Result<ApplicationPreloadResult> {
        let _output = LogOutput::new(Output::None);

        let mut load_plan = ApplicationLoadPlan::default();
        let source_mode = if defer_writes {
            prospective_application_source(source_mode, calling_working_dir, &mut load_plan)?
        } else {
            source_mode
        };
        match preload_app(
            source_mode,
            calling_working_dir,
            dev_mode,
            defer_writes,
            load_plan,
        ) {
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
                load_plan: ApplicationLoadPlan::default(),
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
        inspect_packages: bool,
    ) -> anyhow::Result<Option<ApplicationContext>> {
        let mut source_mode = source_mode;
        let mut package_agent_defaults = BTreeMap::new();
        let (publisher_package, expanded, package_preparation) = match &mut source_mode {
            ApplicationSourceMode::Preloaded(loaded) if inspect_packages => {
                let (publisher_package, pins, lock) = package_pins(loaded)?;
                golem_package::check_lock(&pins, &lock)?;
                if pins.is_empty() {
                    (publisher_package, Default::default(), None)
                } else {
                    let plan = crate::package::PackagePreparation::inspect(
                        &loaded.app_root_dir,
                        config_dir.join("package-cache"),
                        &loaded.root_manifest,
                        &pins,
                        &lock,
                    )
                    .await?;
                    let mut expanded = plan.inspection().bind(plan.root());
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
                    let mut package_app = package_manifest(plan.root(), &expanded.declarations)?;
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
                    (publisher_package, expanded, Some(plan))
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
        ctx.installed_components = expanded.components;
        ctx.installed_package_calls = expanded.calls;
        ctx.application.package_agent_defaults = package_agent_defaults;
        ctx.js_compilation_cache_dir = config_dir.join("js-compilation-cache");
        ctx.package_preparation = package_preparation;
        for (name, installed) in &ctx.installed_components {
            ctx.component_metadata
                .add_cached_component_metadata(name, installed.metadata.clone())
                .await?;
        }

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

    pub fn publisher_package(&self) -> Option<&golem_package::PackageIdentity> {
        self.publisher_package.as_ref()
    }

    pub fn installed_component(
        &self,
        name: &ComponentName,
    ) -> Option<&golem_package::InstalledComponent> {
        self.installed_components.get(name)
    }

    pub(crate) fn package_preparation(&self) -> Option<&crate::package::PackagePreparation> {
        self.package_preparation.as_ref()
    }

    pub(crate) fn prepared_packages(&self) -> Option<&crate::package::PreparedPackages> {
        self.prepared_packages.as_deref()
    }

    pub(crate) async fn prepare_packages(
        &mut self,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        let Some(preparation) = &self.package_preparation else {
            return Ok(());
        };
        if let Some(prepared) = &self.prepared_packages {
            return prepared.validate(preparation).await;
        }
        let (prepared, reused) = preparation.execute(cancellation).await?;
        log_action(
            if reused { "Up-to-date" } else { "Prepared" },
            "local file packages",
        );
        self.prepared_packages = Some(prepared);
        Ok(())
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
                js_compilation_cache_dir: application
                    .app_root_dir()
                    .join(".golem/js-compilation-cache"),
                application,
                component_metadata,
                publisher_package: None,
                installed_components: BTreeMap::new(),
                installed_package_calls: BTreeMap::new(),
                package_preparation: None,
                prepared_packages: None,
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

    pub fn component_names(&self) -> BTreeSet<ComponentName> {
        self.application.component_names().cloned().collect()
    }

    pub async fn build(
        &self,
        build_config: &BuildConfig,
        plan: &crate::app::build::plan::BuildPlan,
        execution: &BuildExecution,
    ) -> anyhow::Result<crate::app::build::BuildAdvance> {
        build_app(&BuildContext::new(self, build_config, execution), plan).await
    }

    pub async fn custom_command(
        &self,
        build_config: &BuildConfig,
        command_name: &str,
    ) -> Result<(), CustomCommandError> {
        execute_custom_command(
            &BuildContext::new(self, build_config, &BuildExecution::default()),
            command_name,
        )
        .await
    }

    pub fn clean(&self, build_config: &BuildConfig, mode: CleanMode) -> anyhow::Result<()> {
        clean_app(
            &BuildContext::new(self, build_config, &BuildExecution::default()),
            mode,
        )
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
    defer_writes: bool,
    mut load_plan: ApplicationLoadPlan,
) -> Option<ValidatedResult<ApplicationPreloadResult>> {
    load_raw_apps(source_mode, calling_working_dir).map(|loaded_raw_apps| {
        loaded_raw_apps.and_then(|loaded_raw_apps| {
            ValidatedResult::from_result(package_pins(&loaded_raw_apps)).and_then(|_| {
                let referenced_template_names =
                    Application::referenced_template_names_from_raw_apps(&loaded_raw_apps.raw_apps);

                Application::preload_from_raw_apps(loaded_raw_apps.raw_apps.as_slice())
                    .and_then(|application_preload| {
                        ValidatedResult::from_result(ensure_on_demand_commons(
                            &application_preload.application_name.value,
                            &referenced_template_names,
                            dev_mode,
                            defer_writes,
                            &mut load_plan,
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
                            load_plan,
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
    Ok(app_raw::ApplicationWithSource {
        source,
        application: serde_json::from_value(serde_json::to_value(declarations)?)?,
        package_source: true,
    })
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
            pins.clone_from(&raw.packages);
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
    Ok((publisher, pins, lock))
}

fn ensure_on_demand_commons(
    application_name: &ApplicationName,
    referenced_template_names: &BTreeSet<String>,
    dev_mode: bool,
    defer_writes: bool,
    load_plan: &mut ApplicationLoadPlan,
) -> anyhow::Result<Vec<app_raw::ApplicationWithSource>> {
    let app_template_repo = AppTemplateRepo::get(dev_mode)?;

    let mut on_demand_raw_apps = Vec::new();

    for language in app_template_repo.builtin_template_languages(referenced_template_names) {
        if let Some(template) = app_template_repo.common_on_demand_template(language)? {
            let app_dir = std::env::current_dir()?;
            let target_dir = Application::on_demand_common_dir_for_language(template.0.language);
            let golem_yaml_path = target_dir.join("golem.yaml");
            if defer_writes {
                if template.is_up_to_date(application_name, &target_dir)? {
                    if golem_yaml_path.exists() {
                        on_demand_raw_apps.push(app_raw::ApplicationWithSource::from_yaml_file(
                            &golem_yaml_path,
                        )?);
                    }
                    continue;
                }
                let rendered = template.render(application_name, &app_dir, &target_dir)?;
                if let Some(yaml) = rendered.get(&golem_yaml_path) {
                    on_demand_raw_apps.push(app_raw::ApplicationWithSource::from_yaml_string(
                        golem_yaml_path,
                        yaml,
                    )?);
                }
                load_plan.commons.push((
                    template.clone(),
                    application_name.clone(),
                    app_dir,
                    target_dir,
                ));
                continue;
            }
            template.generate(application_name, &app_dir, &target_dir)?;
            if golem_yaml_path.exists() {
                on_demand_raw_apps.push(app_raw::ApplicationWithSource::from_yaml_file(
                    &golem_yaml_path,
                )?);
            }
        }
    }

    Ok(on_demand_raw_apps)
}

fn prospective_application_source(
    source_mode: ApplicationSourceMode,
    calling_working_dir: &Path,
    load_plan: &mut ApplicationLoadPlan,
) -> anyhow::Result<ApplicationSourceMode> {
    let root = match &source_mode {
        ApplicationSourceMode::Automatic => None,
        ApplicationSourceMode::ByRootManifest(root) => Some(root.as_path()),
        ApplicationSourceMode::Preloaded(_) | ApplicationSourceMode::None => {
            return Ok(source_mode);
        }
    };
    let Some(collected) = collect_sources_and_switch_to_app_root(calling_working_dir, root) else {
        return Ok(ApplicationSourceMode::None);
    };
    let collected =
        validated_to_anyhow("Failed to collect application manifests", collected, None)?;
    load_plan.upgrades = plan_manifest_upgrade_steps(&collected.sources)?;
    if load_plan.upgrades.is_empty() {
        validated_to_anyhow(
            "Invalid manifest versions",
            validate_manifest_versions(&collected.sources),
            None,
        )?;
    }
    let raw_apps = collected
        .sources
        .iter()
        .map(
            |source| match load_plan.upgrades.iter().find(|step| &step.path == source) {
                Some(step) => {
                    app_raw::ApplicationWithSource::from_yaml_string(source.clone(), &step.new)
                }
                None => app_raw::ApplicationWithSource::from_yaml_file(source),
            },
        )
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(ApplicationSourceMode::Preloaded(LoadedRawApps {
        app_root_dir: collected.app_root_dir,
        root_manifest: collected.root_manifest,
        calling_working_dir: collected.calling_working_dir,
        raw_apps,
    }))
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
                package_source: false,
                application: app_raw::Application::from_yaml_str(&format!(
                    "agents:\n  {agent}:\n    package: {package}\n"
                ))
                .unwrap(),
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
        let mut effects = crate::package::LocalEffects::new(app.path(), app.path().join("cache"))
            .with_cache_population(&tokio_util::sync::CancellationToken::new());
        let expanded = expand(&pins, &pins, &unpack, &mut effects).await.unwrap();
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
        assert_eq!(
            installed.all_sources(),
            &std::collections::BTreeSet::from([root.clone()])
        );
        assert!(local.all_sources().contains(&local_source));
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
    async fn automatic_manifest_upgrade_applies_before_load_without_changing_cwd() {
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
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;

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
