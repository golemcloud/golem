use crate::fs;
use crate::log::LogColorize;
use crate::model::app::app_builder::build_application;
use crate::model::app_raw;
use crate::model::component::AppComponentType;
use crate::model::template::Template;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::naming::wit::package_dep_dir_name_from_parser;
use crate::wasm_rpc_stubgen::stub::RustDependencyOverride;
use golem_common::model::{ComponentFilePathWithPermissions, ComponentFilePermissions};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Formatter;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::Url;
use wit_parser::PackageName;

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

#[derive(Clone, Debug)]
pub struct ApplicationConfig {
    pub app_source_mode: ApplicationSourceMode,
    pub skip_up_to_date_checks: bool,
    pub profile: Option<BuildProfileName>,
    pub offline: bool,
    pub steps_filter: HashSet<AppBuildStep>,
    pub golem_rust_override: RustDependencyOverride,
}

impl ApplicationConfig {
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
    Explicit(PathBuf),
    None,
}

#[derive(Debug, Clone)]
pub enum ApplicationComponentSelectMode {
    CurrentDir,
    All,
    Explicit(Vec<AppComponentName>),
}

impl ApplicationComponentSelectMode {
    pub fn all_or_explicit(component_names: Vec<AppComponentName>) -> Self {
        if component_names.is_empty() {
            ApplicationComponentSelectMode::All
        } else {
            ApplicationComponentSelectMode::Explicit(component_names)
        }
    }

    pub fn current_dir_or_explicit(component_names: Vec<AppComponentName>) -> Self {
        if component_names.is_empty() {
            ApplicationComponentSelectMode::CurrentDir
        } else {
            ApplicationComponentSelectMode::Explicit(component_names)
        }
    }
}

#[derive(Debug, Clone)]
pub struct DynamicHelpSections {
    pub components: bool,
    pub custom_commands: bool,
    pub builtin_commands: BTreeSet<String>,
}

#[derive(Debug)]
pub struct ComponentStubInterfaces {
    pub stub_interface_name: String,
    pub component_name: AppComponentName,
    pub is_ephemeral: bool,
    pub exported_interfaces_per_stub_resource: BTreeMap<String, String>,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[clap(rename_all = "kebab_case")]
pub enum AppBuildStep {
    GenRpc,
    Componentize,
    Link,
    AddMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppComponentName(String);

impl AppComponentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for AppComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for AppComponentName {
    fn from(value: String) -> Self {
        AppComponentName(value)
    }
}

impl From<&str> for AppComponentName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BuildProfileName(String);

impl BuildProfileName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for BuildProfileName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for BuildProfileName {
    fn from(value: String) -> Self {
        BuildProfileName(value)
    }
}

impl From<&str> for BuildProfileName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TemplateName(String);

impl TemplateName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for TemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TemplateName {
    fn from(value: String) -> Self {
        TemplateName(value)
    }
}

impl From<&str> for TemplateName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

pub fn includes_from_yaml_file(source: &Path) -> Vec<String> {
    fs::read_to_string(source)
        .ok()
        .and_then(|source| app_raw::Application::from_yaml_str(source.as_str()).ok())
        .map(|app| {
            if app.includes.is_empty() {
                vec!["**/golem.yaml".to_string()]
            } else {
                app.includes
            }
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub enum ResolvedComponentProperties {
    Properties {
        template_name: Option<TemplateName>,
        any_template_overrides: bool,
        properties: ComponentProperties,
    },
    Profiles {
        template_name: Option<TemplateName>,
        any_template_overrides: HashMap<BuildProfileName, bool>,
        default_profile: BuildProfileName,
        profiles: HashMap<BuildProfileName, ComponentProperties>,
    },
}

pub struct ComponentEffectivePropertySource<'a> {
    pub template_name: Option<&'a TemplateName>,
    pub profile: Option<&'a BuildProfileName>,
    pub is_requested_profile: bool,
    pub any_template_overrides: bool,
}

#[derive(Clone, Debug)]
pub struct WithSource<T> {
    pub source: PathBuf,
    pub value: T,
}

impl<T> WithSource<T> {
    pub fn new(source: PathBuf, value: T) -> Self {
        Self { source, value }
    }
}

impl<T: Default> Default for WithSource<T> {
    fn default() -> Self {
        Self {
            source: Default::default(),
            value: T::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DependencyType {
    /// Dynamic ("stubless") wasm-rpc
    DynamicWasmRpc,
    /// Static (composed with compiled stub) wasm-rpc
    StaticWasmRpc,
    /// Composes the two WASM components together
    Wasm,
}

impl DependencyType {
    pub const STATIC_WASM_RPC: &'static str = "static-wasm-rpc";
    pub const WASM_RPC: &'static str = "wasm-rpc";
    pub const WASM: &'static str = "wasm";

    pub fn as_str(&self) -> &'static str {
        match self {
            DependencyType::DynamicWasmRpc => Self::WASM_RPC,
            DependencyType::StaticWasmRpc => Self::STATIC_WASM_RPC,
            DependencyType::Wasm => Self::WASM,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            DependencyType::DynamicWasmRpc => "WASM RPC dependency",
            DependencyType::StaticWasmRpc => "Statically composed WASM RPC dependency",
            DependencyType::Wasm => "WASM component dependency",
        }
    }

    pub fn is_wasm_rpc(&self) -> bool {
        matches!(
            self,
            DependencyType::DynamicWasmRpc | DependencyType::StaticWasmRpc
        )
    }
}

impl FromStr for DependencyType {
    type Err = ();

    fn from_str(str: &str) -> Result<Self, Self::Err> {
        match str {
            Self::WASM_RPC => Ok(Self::DynamicWasmRpc),
            Self::STATIC_WASM_RPC => Ok(Self::StaticWasmRpc),
            Self::WASM => Ok(Self::Wasm),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DependentComponent {
    pub name: AppComponentName,
    pub dep_type: DependencyType,
}

impl PartialOrd for DependentComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DependentComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Clone, Debug)]
pub struct Application {
    temp_dir: Option<WithSource<String>>,
    wit_deps: WithSource<Vec<String>>,
    components: BTreeMap<AppComponentName, Component>,
    dependencies: BTreeMap<AppComponentName, BTreeSet<DependentComponent>>,
    no_dependencies: BTreeSet<DependentComponent>,
    custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
    clean: Vec<WithSource<String>>,
}

impl Application {
    pub fn from_raw_apps(apps: Vec<app_raw::ApplicationWithSource>) -> ValidatedResult<Self> {
        build_application(apps)
    }

    pub fn component_names(&self) -> impl Iterator<Item = &AppComponentName> {
        self.components.keys()
    }

    pub fn has_any_component(&self) -> bool {
        !self.components.is_empty()
    }

    pub fn contains_component(&self, component_name: &AppComponentName) -> bool {
        self.components.contains_key(component_name)
    }

    pub fn common_custom_commands(
        &self,
    ) -> &HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>> {
        &self.custom_commands
    }

    pub fn common_clean(&self) -> &Vec<WithSource<String>> {
        &self.clean
    }

    pub fn wit_deps(&self) -> &WithSource<Vec<String>> {
        &self.wit_deps
    }

    pub fn all_dependencies(&self) -> BTreeSet<DependentComponent> {
        self.dependencies.values().flatten().cloned().collect()
    }

    pub fn all_profiles(&self) -> BTreeSet<BuildProfileName> {
        self.component_names()
            .flat_map(|component_name| self.component_profiles(component_name))
            .collect()
    }

    pub fn all_option_profiles(&self) -> BTreeSet<Option<BuildProfileName>> {
        let mut profiles = self
            .component_names()
            .flat_map(|component_name| self.component_profiles(component_name))
            .map(Some)
            .collect::<BTreeSet<_>>();
        profiles.insert(None);
        profiles
    }

    pub fn all_custom_commands(&self, profile: Option<&BuildProfileName>) -> BTreeSet<String> {
        let mut custom_commands = BTreeSet::new();
        custom_commands.extend(self.component_names().flat_map(|component_name| {
            self.component_properties(component_name, profile)
                .custom_commands
                .keys()
                .cloned()
        }));
        custom_commands.extend(self.custom_commands.keys().cloned());
        custom_commands
    }

    pub fn all_custom_commands_for_all_profiles(
        &self,
    ) -> BTreeMap<Option<BuildProfileName>, BTreeSet<String>> {
        let mut custom_commands = BTreeMap::<Option<BuildProfileName>, BTreeSet<String>>::new();

        custom_commands
            .entry(None)
            .or_default()
            .extend(self.custom_commands.keys().cloned());

        for profile in self.all_option_profiles() {
            let profile_commands: &mut BTreeSet<String> = {
                if custom_commands.contains_key(&profile) {
                    custom_commands.get_mut(&profile).unwrap()
                } else {
                    custom_commands.entry(profile.clone()).or_default()
                }
            };

            profile_commands.extend(self.component_names().flat_map(|component_name| {
                self.component_properties(component_name, profile.as_ref())
                    .custom_commands
                    .keys()
                    .cloned()
            }));
        }

        custom_commands
    }

    pub fn temp_dir(&self) -> PathBuf {
        match self.temp_dir.as_ref() {
            Some(temp_dir) => temp_dir.source.as_path().join(&temp_dir.value),
            None => Path::new("golem-temp").to_path_buf(),
        }
    }

    pub fn task_result_marker_dir(&self) -> PathBuf {
        self.temp_dir().join("task-results")
    }

    fn component(&self, component_name: &AppComponentName) -> &Component {
        self.components
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {}", component_name))
    }

    pub fn component_source(&self, component_name: &AppComponentName) -> &Path {
        &self.component(component_name).source
    }

    pub fn component_source_dir(&self, component_name: &AppComponentName) -> &Path {
        self.component(component_name).source_dir()
    }

    pub fn component_dependencies(
        &self,
        component_name: &AppComponentName,
    ) -> &BTreeSet<DependentComponent> {
        self.dependencies
            .get(component_name)
            .unwrap_or(&self.no_dependencies)
    }

    pub fn component_profiles(
        &self,
        component_name: &AppComponentName,
    ) -> BTreeSet<BuildProfileName> {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::Properties { .. } => BTreeSet::new(),
            ResolvedComponentProperties::Profiles { profiles, .. } => {
                profiles.keys().cloned().collect()
            }
        }
    }

    pub fn component_effective_property_source<'a>(
        &'a self,
        component_name: &AppComponentName,
        profile: Option<&'a BuildProfileName>,
    ) -> ComponentEffectivePropertySource<'a> {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::Properties {
                template_name,
                any_template_overrides,
                ..
            } => ComponentEffectivePropertySource {
                template_name: template_name.as_ref(),
                profile: None,
                is_requested_profile: false,
                any_template_overrides: *any_template_overrides,
            },
            ResolvedComponentProperties::Profiles {
                template_name,
                any_template_overrides,
                default_profile,
                profiles,
            } => {
                let effective_profile = profile
                    .map(|profile| {
                        if profiles.contains_key(profile) {
                            profile
                        } else {
                            default_profile
                        }
                    })
                    .unwrap_or_else(|| default_profile);

                let is_requested_profile = Some(&effective_profile) == profile.as_ref();

                let any_template_overrides = any_template_overrides
                    .get(effective_profile)
                    .cloned()
                    .unwrap_or_default();
                ComponentEffectivePropertySource {
                    template_name: template_name.as_ref(),
                    profile: Some(effective_profile),
                    is_requested_profile,
                    any_template_overrides,
                }
            }
        }
    }

    pub fn component_properties(
        &self,
        component_name: &AppComponentName,
        profile: Option<&BuildProfileName>,
    ) -> &ComponentProperties {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::Properties { properties, .. } => properties,
            ResolvedComponentProperties::Profiles {
                default_profile,
                profiles,
                ..
            } => profiles
                .get(
                    profile
                        .map(|profile| {
                            if profiles.contains_key(profile) {
                                profile
                            } else {
                                default_profile
                            }
                        })
                        .unwrap_or_else(|| default_profile),
                )
                .unwrap(),
        }
    }

    pub fn component_name_as_safe_path_elem(&self, component_name: &AppComponentName) -> String {
        component_name.as_str().replace(":", "_")
    }

    pub fn component_source_wit(
        &self,
        component_name: &AppComponentName,
        profile: Option<&BuildProfileName>,
    ) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name, profile)
                .source_wit
                .clone(),
        )
    }

    pub fn component_generated_base_wit(&self, component_name: &AppComponentName) -> PathBuf {
        self.temp_dir()
            .join("generated-base-wit")
            .join(self.component_name_as_safe_path_elem(component_name))
    }

    pub fn component_generated_base_wit_exports_package_dir(
        &self,
        component_name: &AppComponentName,
        exports_package_name: &PackageName,
    ) -> PathBuf {
        self.component_generated_base_wit(component_name)
            .join(naming::wit::DEPS_DIR)
            .join(package_dep_dir_name_from_parser(exports_package_name))
            .join(naming::wit::EXPORTS_WIT_FILE_NAME)
    }

    pub fn component_generated_wit(
        &self,
        component_name: &AppComponentName,
        profile: Option<&BuildProfileName>,
    ) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name, profile)
                .generated_wit
                .clone(),
        )
    }

    pub fn component_wasm(
        &self,
        component_name: &AppComponentName,
        profile: Option<&BuildProfileName>,
    ) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name, profile)
                .component_wasm
                .clone(),
        )
    }

    /// The final linked component WASM
    pub fn component_final_linked_wasm(
        &self,
        component_name: &AppComponentName,
        profile: Option<&BuildProfileName>,
    ) -> PathBuf {
        self.component_source_dir(component_name).join(
            self.component_properties(component_name, profile)
                .linked_wasm
                .as_ref()
                .cloned()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    self.temp_dir()
                        .join("final-linked-wasm")
                        .join(format!("{}.wasm", component_name.as_str()))
                }),
        )
    }

    /// Temporary target of the component composition (linking) step
    pub fn component_linked_wasm(&self, component_name: &AppComponentName) -> PathBuf {
        self.component_source_dir(component_name).join(
            self.temp_dir()
                .join("linked-wasm")
                .join(format!("{}.wasm", component_name.as_str())),
        )
    }

    fn client_build_dir(&self) -> PathBuf {
        self.temp_dir().join("client")
    }

    pub fn client_temp_build_dir(&self, component_name: &AppComponentName) -> PathBuf {
        self.client_build_dir()
            .join(self.component_name_as_safe_path_elem(component_name))
            .join("temp-build")
    }

    pub fn client_wasm(&self, component_name: &AppComponentName) -> PathBuf {
        self.client_build_dir()
            .join(self.component_name_as_safe_path_elem(component_name))
            .join("client.wasm")
    }

    pub fn client_wit(&self, component_name: &AppComponentName) -> PathBuf {
        self.client_build_dir()
            .join(self.component_name_as_safe_path_elem(component_name))
            .join(naming::wit::WIT_DIR)
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    pub source: PathBuf,
    pub properties: ResolvedComponentProperties,
}

impl Component {
    pub fn source_dir(&self) -> &Path {
        let parent = self.source.parent().unwrap_or_else(|| {
            panic!(
                "Failed to get parent for component, source: {}",
                self.source.display()
            )
        });
        if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComponentProperties {
    pub source_wit: String,
    pub generated_wit: String,
    pub component_wasm: String,
    pub linked_wasm: Option<String>,
    pub build: Vec<app_raw::ExternalCommand>,
    pub custom_commands: HashMap<String, Vec<app_raw::ExternalCommand>>,
    pub clean: Vec<String>,
    pub component_type: AppComponentType,
    pub files: Vec<InitialComponentFile>,
}

impl ComponentProperties {
    fn from_raw(
        validation: &mut ValidationBuilder,
        source: &Path,
        raw: app_raw::ComponentProperties,
    ) -> Option<Self> {
        let files = InitialComponentFile::from_raw_vec(validation, source, raw.files)?;

        Some(Self {
            source_wit: raw.source_wit.unwrap_or_default(),
            generated_wit: raw.generated_wit.unwrap_or_default(),
            component_wasm: raw.component_wasm.unwrap_or_default(),
            linked_wasm: raw.linked_wasm,
            build: raw.build,
            custom_commands: raw.custom_commands,
            clean: raw.clean,
            component_type: raw.component_type.unwrap_or_default(),
            files,
        })
    }

    fn from_raw_template<C: Serialize>(
        validation: &mut ValidationBuilder,
        source: &Path,
        template_env: &minijinja::Environment,
        template_ctx: &C,
        template_properties: &app_raw::ComponentProperties,
    ) -> anyhow::Result<Option<Self>> {
        Ok(ComponentProperties::from_raw(
            validation,
            source,
            template_properties.render(template_env, template_ctx)?,
        ))
    }

    fn merge_with_overrides(
        mut self,
        validation: &mut ValidationBuilder,
        source: &Path,
        overrides: app_raw::ComponentProperties,
    ) -> anyhow::Result<Option<(Self, bool)>> {
        let mut any_overrides = false;
        let mut any_errors = false;

        if let Some(source_wit) = overrides.source_wit {
            self.source_wit = source_wit;
            any_overrides = true;
        }

        if let Some(generated_wit) = overrides.generated_wit {
            self.generated_wit = generated_wit;
            any_overrides = true;
        }

        if let Some(component_wasm) = overrides.component_wasm {
            self.component_wasm = component_wasm;
            any_overrides = true;
        }

        if overrides.linked_wasm.is_some() {
            self.linked_wasm = overrides.linked_wasm;
            any_overrides = true;
        }

        if !overrides.build.is_empty() {
            self.build = overrides.build;
            any_overrides = true;
        }

        if !overrides.custom_commands.is_empty() {
            any_overrides = true;
            self.custom_commands.extend(overrides.custom_commands)
        }

        if let Some(component_type) = overrides.component_type {
            self.component_type = component_type;
            any_overrides = true;
        }

        if !overrides.files.is_empty() {
            any_overrides = true;
            match InitialComponentFile::from_raw_vec(validation, source, overrides.files) {
                Some(files) => {
                    self.files.extend(files);
                }
                None => {
                    any_errors = true;
                }
            }
        }

        Ok((!any_errors).then_some((self, any_overrides)))
    }

    pub fn is_ephemeral(&self) -> bool {
        self.component_type == AppComponentType::Ephemeral
    }

    pub fn is_durable(&self) -> bool {
        self.component_type == AppComponentType::Durable
    }

    pub fn is_deployable(&self) -> bool {
        self.component_type.as_deployable_component_type().is_some()
    }
}

#[derive(Clone, Debug)]
pub struct InitialComponentFile {
    pub source: InitialComponentFileSource,
    pub target: ComponentFilePathWithPermissions,
}

impl InitialComponentFile {
    pub fn from_raw(
        validation: &mut ValidationBuilder,
        source: &Path,
        file: app_raw::InitialComponentFile,
    ) -> Option<InitialComponentFile> {
        let source = InitialComponentFileSource::new(&file.source_path, source)
            .map_err(|err| {
                validation.push_context("source path", file.source_path.to_string());
                validation.add_error(err);
                validation.pop_context();
            })
            .ok()?;

        Some(InitialComponentFile {
            source,
            target: ComponentFilePathWithPermissions {
                path: file.target_path,
                permissions: file
                    .permissions
                    .unwrap_or(ComponentFilePermissions::ReadOnly),
            },
        })
    }

    pub fn from_raw_vec(
        validation: &mut ValidationBuilder,
        source: &Path,
        files: Vec<app_raw::InitialComponentFile>,
    ) -> Option<Vec<Self>> {
        let source_count = files.len();

        let files = files
            .into_iter()
            .filter_map(|file| InitialComponentFile::from_raw(validation, source, file))
            .collect::<Vec<_>>();

        (files.len() == source_count).then_some(files)
    }
}

#[derive(Clone, Debug)]
pub struct InitialComponentFileSource(Url);

impl InitialComponentFileSource {
    pub fn new(url_string: &str, relative_to: &Path) -> Result<Self, String> {
        // Try to parse the URL as an absolute URL
        let url = Url::parse(url_string).or_else(|_| {
            // If that fails, try to parse it as a relative path
            let canonical_relative_to = relative_to
                .parent()
                .expect("Failed to get parent")
                .canonicalize()
                .map_err(|_| {
                    format!(
                        "Failed to canonicalize relative path: {}",
                        relative_to.log_color_highlight()
                    )
                })?;

            let source = canonical_relative_to.join(PathBuf::from(url_string));
            Url::from_file_path(&source).map_err(|_| {
                format!(
                    "Failed to convert source ({}) to URL",
                    source.log_color_highlight(),
                )
            })
        })?;

        let source_path_scheme = url.scheme();
        let supported_schemes = ["http", "https", "file", ""];
        if !supported_schemes.contains(&source_path_scheme) {
            return Err(format!(
                "Unsupported source path scheme: {}, supported schemes {}:",
                source_path_scheme.log_color_highlight(),
                supported_schemes.join(", ")
            ));
        }
        Ok(Self(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn into_url(self) -> Url {
        self.0
    }
}

mod app_builder {
    use crate::fs::PathExtra;
    use crate::log::LogColorize;
    use crate::model::app::{
        AppComponentName, Application, BuildProfileName, Component, ComponentProperties,
        DependencyType, DependentComponent, ResolvedComponentProperties, TemplateName, WithSource,
    };
    use crate::model::app_raw;
    use crate::validation::{ValidatedResult, ValidationBuilder};
    use heck::{
        ToKebabCase, ToLowerCamelCase, ToPascalCase, ToShoutyKebabCase, ToShoutySnakeCase,
        ToSnakeCase, ToTitleCase, ToTrainCase, ToUpperCamelCase,
    };
    use itertools::Itertools;
    use serde::Serialize;
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    pub fn build_application(
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Application> {
        AppBuilder::build(apps)
    }

    #[derive(Debug, PartialEq, Eq, Hash)]
    enum UniqueSourceCheckedEntityKey {
        Include,
        TempDir,
        WitDeps,
        CustomCommand(String),
        Template(TemplateName),
        WasmRpcDependency((AppComponentName, DependentComponent)),
        Component(AppComponentName),
    }

    impl UniqueSourceCheckedEntityKey {
        fn entity_kind(&self) -> &'static str {
            let property = "Property";
            match self {
                UniqueSourceCheckedEntityKey::Include => property,
                UniqueSourceCheckedEntityKey::TempDir => property,
                UniqueSourceCheckedEntityKey::WitDeps => property,
                UniqueSourceCheckedEntityKey::CustomCommand(_) => "Custom command",
                UniqueSourceCheckedEntityKey::Template(_) => "Template",
                UniqueSourceCheckedEntityKey::WasmRpcDependency(_) => "WASM RPC dependency",
                UniqueSourceCheckedEntityKey::Component(_) => "Component",
            }
        }

        fn entity_name(self) -> String {
            match self {
                UniqueSourceCheckedEntityKey::Include => {
                    "include".log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::TempDir => {
                    "tempDir".log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::WitDeps => {
                    "witDeps".log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::CustomCommand(command_name) => {
                    command_name.log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::Template(template_name) => {
                    template_name.as_str().log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::WasmRpcDependency((
                    component_name,
                    dependent_component,
                )) => {
                    format!(
                        "{} - {} - {}",
                        component_name.as_str().log_color_highlight(),
                        dependent_component.name.as_str().log_color_highlight(),
                        dependent_component.dep_type.as_str().log_color_highlight(),
                    )
                }
                UniqueSourceCheckedEntityKey::Component(component_name) => {
                    component_name.as_str().log_color_highlight().to_string()
                }
            }
        }
    }

    #[derive(Default)]
    struct AppBuilder {
        include: Vec<String>,
        temp_dir: Option<WithSource<String>>,
        wit_deps: WithSource<Vec<String>>,
        templates: HashMap<TemplateName, app_raw::ComponentTemplate>,
        dependencies: BTreeMap<AppComponentName, BTreeSet<DependentComponent>>,
        custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
        clean: Vec<WithSource<String>>,
        raw_components: HashMap<AppComponentName, (PathBuf, app_raw::Component)>,
        resolved_components: BTreeMap<AppComponentName, Component>,

        entity_sources: HashMap<UniqueSourceCheckedEntityKey, Vec<PathBuf>>,
    }

    impl AppBuilder {
        fn build(apps: Vec<app_raw::ApplicationWithSource>) -> ValidatedResult<Application> {
            let mut builder = Self::default();
            let mut validation = ValidationBuilder::default();

            builder.add_raw_apps(&mut validation, apps);
            builder.validate_dependency_targets(&mut validation);
            builder.validate_unique_sources(&mut validation);
            builder.resolve_components(&mut validation);

            validation.build(Application {
                temp_dir: builder.temp_dir,
                wit_deps: builder.wit_deps,
                components: builder.resolved_components,
                dependencies: builder.dependencies,
                no_dependencies: BTreeSet::new(),
                custom_commands: builder.custom_commands,
                clean: builder.clean,
            })
        }

        fn add_entity_source(&mut self, key: UniqueSourceCheckedEntityKey, source: &Path) -> bool {
            let sources = self.entity_sources.entry(key).or_default();
            let is_first = sources.is_empty();
            sources.push(source.to_path_buf());
            is_first
        }

        fn add_raw_apps(
            &mut self,
            validation: &mut ValidationBuilder,
            apps: Vec<app_raw::ApplicationWithSource>,
        ) {
            for app in apps {
                self.add_raw_app(validation, app);
            }
        }

        fn add_raw_app(
            &mut self,
            validation: &mut ValidationBuilder,
            app: app_raw::ApplicationWithSource,
        ) {
            validation.with_context(
                vec![("source", app.source.to_string_lossy().to_string())],
                |validation| {
                    let app_source = PathExtra::new(&app.source);
                    let app_source_dir = app_source.parent().unwrap();

                    if let Some(dir) = app.application.temp_dir {
                        if self
                            .add_entity_source(UniqueSourceCheckedEntityKey::TempDir, &app.source)
                        {
                            self.temp_dir =
                                Some(WithSource::new(app_source_dir.to_path_buf(), dir));
                        }
                    }

                    if !app.application.includes.is_empty()
                        && self
                            .add_entity_source(UniqueSourceCheckedEntityKey::Include, &app.source)
                    {
                        self.include = app.application.includes;
                    }

                    if !app.application.wit_deps.is_empty()
                        && self
                            .add_entity_source(UniqueSourceCheckedEntityKey::WitDeps, &app.source)
                    {
                        self.wit_deps =
                            WithSource::new(app_source_dir.to_path_buf(), app.application.wit_deps);
                    }

                    for (template_name, template) in app.application.templates {
                        self.add_raw_template(validation, &app.source, template_name, template);
                    }

                    for (component_name, component) in app.application.components {
                        let component_name = AppComponentName::from(component_name);
                        let unique_key =
                            UniqueSourceCheckedEntityKey::Component(component_name.clone());
                        if self.add_entity_source(unique_key, &app.source) {
                            self.raw_components
                                .insert(component_name, (app.source.to_path_buf(), component));
                        }
                    }

                    for (component_name, component_dependencies) in app.application.dependencies {
                        self.add_component_dependencies(
                            validation,
                            &app.source,
                            component_name,
                            component_dependencies,
                        );
                    }

                    for (command_name, command) in app.application.custom_commands {
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::CustomCommand(command_name.clone()),
                            &app.source,
                        ) {
                            self.custom_commands.insert(
                                command_name,
                                WithSource::new(app_source_dir.to_path_buf(), command),
                            );
                        }
                    }

                    self.clean.extend(
                        app.application
                            .clean
                            .into_iter()
                            .map(|path| WithSource::new(app.source.to_path_buf(), path)),
                    );
                },
            );
        }

        fn add_raw_template(
            &mut self,
            validation: &mut ValidationBuilder,
            source: &Path,
            template_name: String,
            template: app_raw::ComponentTemplate,
        ) {
            let valid =
                validation.with_context(vec![("template", template_name.clone())], |validation| {
                    if template.profiles.is_empty() {
                        if template.default_profile.is_some() {
                            validation.add_error(format!(
                                "When {} is not defined then {} should not be defined",
                                "profiles".log_color_highlight(),
                                "defaultProfile".log_color_highlight()
                            ));
                        }
                    } else {
                        let defined_property_names =
                            template.component_properties.defined_property_names();
                        if !defined_property_names.is_empty() {
                            for property_name in defined_property_names {
                                validation.add_error(format!(
                                    "When {} is defined then {} should not be defined",
                                    "profiles".log_color_highlight(),
                                    property_name.log_color_highlight()
                                ));
                            }
                        }

                        if template.default_profile.is_none() {
                            validation.add_error(format!(
                                "When {} is defined then {} is mandatory",
                                "profiles".log_color_highlight(),
                                "defaultProfile".log_color_highlight()
                            ));
                        }
                    }
                });

            let template_name = TemplateName::from(template_name);
            if self.add_entity_source(
                UniqueSourceCheckedEntityKey::Template(template_name.clone()),
                source,
            ) && valid
            {
                self.templates.insert(template_name, template);
            }
        }

        fn add_component_dependencies(
            &mut self,
            validation: &mut ValidationBuilder,
            source: &Path,
            component_name: String,
            component_dependencies: Vec<app_raw::Dependency>,
        ) {
            validation.with_context(vec![("component", component_name.clone())], |validation| {
                for dependency in component_dependencies {
                    let dep_type = DependencyType::from_str(&dependency.type_);
                    if let Ok(dep_type) = dep_type {
                        match dependency.target {
                            Some(target_name) => {
                                let dependent_component = DependentComponent {
                                    name: target_name.into(),
                                    dep_type,
                                };

                                let unique_key = UniqueSourceCheckedEntityKey::WasmRpcDependency((
                                    component_name.clone().into(),
                                    dependent_component.clone(),
                                ));
                                if self.add_entity_source(unique_key, source) {
                                    self.dependencies
                                        .entry(component_name.clone().into())
                                        .or_default()
                                        .insert(dependent_component);
                                }
                            }
                            None => validation.add_error(format!(
                                "Missing {} field for component wasm-rpc dependency",
                                "target".log_color_error_highlight()
                            )),
                        }
                    } else {
                        validation.add_error(format!(
                            "Unknown component dependency type: {}",
                            dependency.type_.log_color_error_highlight()
                        ));
                    }
                }
            });
        }

        fn validate_unique_sources(&mut self, validation: &mut ValidationBuilder) {
            let entity_sources = std::mem::take(&mut self.entity_sources);
            entity_sources
                .into_iter()
                .filter(|(_, sources)| sources.len() > 1)
                .for_each(|(key, sources)| {
                    validation.add_error(format!(
                        "{} {} is defined in multiple sources: {}",
                        key.entity_kind(),
                        key.entity_name(),
                        sources
                            .into_iter()
                            .map(|s| s.log_color_highlight())
                            .join(", ")
                    ))
                })
        }

        fn validate_dependency_targets(&mut self, validation: &mut ValidationBuilder) {
            for (component, deps) in &self.dependencies {
                for target in deps {
                    let invalid_source = !self.raw_components.contains_key(component);
                    let invalid_target = !self.raw_components.contains_key(&target.name);

                    if invalid_source || invalid_target {
                        let source = self
                            .entity_sources
                            .get(&UniqueSourceCheckedEntityKey::WasmRpcDependency((
                                component.clone(),
                                target.clone(),
                            )))
                            .expect("Missing sources for dependency")
                            .first()
                            .expect("Missing source for dependency");

                        validation.with_context(
                            vec![("source", source.to_string_lossy().to_string())],
                            |validation| {
                                if invalid_source {
                                    validation.add_error(format!(
                                        "{} {} - {} references unknown component",
                                        target.dep_type.describe(),
                                        component.as_str().log_color_error_highlight(),
                                        target.name.as_str().log_color_highlight()
                                    ))
                                }
                                if invalid_target {
                                    validation.add_error(format!(
                                        "{} {} - {} references unknown target component",
                                        target.dep_type.describe(),
                                        component.as_str().log_color_highlight(),
                                        target.name.as_str().log_color_error_highlight()
                                    ))
                                }
                            },
                        );
                    }
                }
            }
        }

        fn template_env<'a>() -> minijinja::Environment<'a> {
            let mut env = minijinja::Environment::new();

            env.add_filter("to_snake_case", |str: &str| str.to_snake_case());

            env.add_filter("to_kebab_case", |str: &str| str.to_kebab_case());
            env.add_filter("to_lower_camel_case", |str: &str| str.to_lower_camel_case());
            env.add_filter("to_pascal_case", |str: &str| str.to_pascal_case());
            env.add_filter("to_shouty_kebab_case", |str: &str| {
                str.to_shouty_kebab_case()
            });
            env.add_filter("to_shouty_snake_case", |str: &str| {
                str.to_shouty_snake_case()
            });
            env.add_filter("to_snake_case", |str: &str| str.to_snake_case());
            env.add_filter("to_title_case", |str: &str| str.to_title_case());
            env.add_filter("to_train_case", |str: &str| str.to_train_case());
            env.add_filter("to_upper_camel_case", |str: &str| str.to_upper_camel_case());

            env
        }

        fn template_context(component_name: &AppComponentName) -> impl Serialize {
            minijinja::context! {
                componentName => component_name.as_str(),
                component_name => component_name.as_str()
            }
        }

        fn resolve_components(&mut self, validation: &mut ValidationBuilder) {
            let template_env = Self::template_env();

            let components = std::mem::take(&mut self.raw_components);

            for (component_name, (source, component)) in components {
                self.resolve_component(
                    validation,
                    &template_env,
                    source,
                    component_name,
                    component,
                );
            }
        }

        fn resolve_component(
            &mut self,
            validation: &mut ValidationBuilder,
            template_env: &minijinja::Environment,
            source: PathBuf,
            component_name: AppComponentName,
            component: app_raw::Component,
        ) {
            validation.with_context(
                vec![
                    ("source", source.to_string_lossy().to_string()),
                    ("component", component_name.to_string()),
                ],
                |validation| {
                    let properties = match &component.template {
                        Some(template_name) => {
                            let template_name = TemplateName::from(template_name.clone());
                            match self.templates.get_mut(&template_name) {
                                Some(template) => Self::resolve_templated_component_properties(
                                    validation,
                                    &source,
                                    template_env,
                                    template_name,
                                    template,
                                    component_name.clone(),
                                    component,
                                ),
                                None => {
                                    validation.add_error(format!(
                                        "Component references unknown template: {}",
                                        template_name.as_str().log_color_error_highlight()
                                    ));
                                    None
                                }
                            }
                        }
                        None => Self::resolve_directly_defined_component_properties(
                            validation, &source, component,
                        ),
                    };
                    if let Some(properties) = properties {
                        self.resolved_components
                            .insert(component_name, Component { source, properties });
                    }
                },
            );
        }

        fn resolve_templated_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: TemplateName,
            template: &mut app_raw::ComponentTemplate,
            component_name: AppComponentName,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let (properties, _) = validation.with_context_returning(
                vec![("template", template_name.to_string())],
                |validation| {
                    let overrides_compatible = validation.with_context(vec![], |validation| {
                        let defined_property_names = component.component_properties.defined_property_names();

                        if !template.profiles.is_empty() && !defined_property_names.is_empty() {
                            for property_name in defined_property_names {
                                validation.add_error(
                                    format!(
                                        "Property {} cannot be used, as the component uses a template with profiles",
                                        property_name.log_color_highlight()
                                    )
                                );
                            }
                        }

                        for profile_name in component.profiles.keys() {
                            if !template.profiles.contains_key(profile_name) {
                                validation.add_error(
                                    format!(
                                        "Profile {} cannot be used, as the component uses template {} with the following profiles: {}",
                                        profile_name.log_color_highlight(),
                                        template_name.as_str().log_color_highlight(),
                                        template.profiles.keys().map(|s| s.log_color_highlight()).join(", ")
                                    )
                                );
                            }
                        }

                        if let Some(default_profile) = &component.default_profile {
                            if !template.profiles.contains_key(default_profile) {
                                validation.add_error(
                                    format!(
                                        "Default profile override {} cannot be used, as the component uses template {} with the following profiles: {}",
                                        default_profile.log_color_highlight(),
                                        template_name.as_str().log_color_highlight(),
                                        template.profiles.keys().map(|s| s.log_color_highlight()).join(", ")
                                    )
                                );
                            }
                        }
                    });

                    overrides_compatible.then(|| {
                        if template.profiles.is_empty() {
                            Self::resolve_templated_non_profiled_component_properties(
                                validation,
                                source,
                                template_env,
                                template_name,
                                template,
                                component_name,
                                component.component_properties,
                            )
                        } else {
                            Self::resolve_templated_profiled_component_properties(
                                validation,
                                source,
                                template_env,
                                template_name,
                                template,
                                component_name,
                                component.profiles,
                                component.default_profile,
                            )
                        }
                    }).flatten()
                },
            );

            properties
        }

        fn resolve_templated_non_profiled_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: TemplateName,
            template: &app_raw::ComponentTemplate,
            component_name: AppComponentName,
            component_properties: app_raw::ComponentProperties,
        ) -> Option<ResolvedComponentProperties> {
            Self::convert_and_validate_templated_component_properties(
                validation,
                source,
                template_env,
                &template_name,
                &template.component_properties,
                &component_name,
                Some(component_properties),
            )
            .map(|(properties, any_template_overrides)| {
                ResolvedComponentProperties::Properties {
                    template_name: Some(template_name),
                    any_template_overrides,
                    properties,
                }
            })
        }

        fn resolve_templated_profiled_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: TemplateName,
            template: &app_raw::ComponentTemplate,
            component_name: AppComponentName,
            mut profiles: HashMap<String, app_raw::ComponentProperties>,
            default_profile: Option<String>,
        ) -> Option<ResolvedComponentProperties> {
            let ((profiles, any_template_overrides), valid) =
                validation.with_context_returning(vec![], |validation| {
                    let mut resolved_overrides = HashMap::<BuildProfileName, bool>::new();
                    let mut resolved_profiles =
                        HashMap::<BuildProfileName, ComponentProperties>::new();

                    for (profile_name, template_component_properties) in &template.profiles {
                        validation.with_context(
                            vec![("profile", profile_name.to_string())],
                            |validation| {
                                let component_properties = profiles.remove(profile_name);
                                Self::convert_and_validate_templated_component_properties(
                                    validation,
                                    source,
                                    template_env,
                                    &template_name,
                                    template_component_properties,
                                    &component_name,
                                    component_properties,
                                )
                                .into_iter()
                                .for_each(
                                    |(component_properties, any_template_overrides)| {
                                        resolved_overrides.insert(
                                            profile_name.clone().into(),
                                            any_template_overrides,
                                        );
                                        resolved_profiles.insert(
                                            profile_name.clone().into(),
                                            component_properties,
                                        );
                                    },
                                );
                            },
                        );
                    }

                    (resolved_profiles, resolved_overrides)
                });

            valid.then(|| ResolvedComponentProperties::Profiles {
                template_name: Some(template_name),
                any_template_overrides,
                default_profile: default_profile
                    .or(template.default_profile.clone())
                    .clone()
                    .expect("Missing template default profile")
                    .into(),
                profiles,
            })
        }

        fn resolve_directly_defined_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            if component.profiles.is_empty() {
                Self::resolve_directly_defined_non_profiled_component_properties(
                    validation, source, component,
                )
            } else {
                Self::resolve_directly_defined_profiled_component_properties(
                    validation, source, component,
                )
            }
        }

        fn resolve_directly_defined_profiled_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let valid =
                validation.with_context(vec![], |validation| match &component.default_profile {
                    Some(default_profile) => {
                        if !component.profiles.contains_key(default_profile) {
                            validation.add_error(format!(
                                "Default profile {} not found in available profiles: {}",
                                default_profile.log_color_highlight(),
                                component
                                    .profiles
                                    .keys()
                                    .map(|s| s.log_color_highlight())
                                    .join(", ")
                            ));
                        }
                    }
                    None => {
                        validation.add_error(format!(
                            "When {} is defined then {} is mandatory",
                            "profiles".log_color_highlight(),
                            "defaultProfile".log_color_highlight()
                        ));
                    }
                });

            valid.then(|| ResolvedComponentProperties::Profiles {
                template_name: None,
                any_template_overrides: Default::default(),
                default_profile: component
                    .default_profile
                    .map(BuildProfileName::from)
                    .unwrap(),
                profiles: {
                    component
                        .profiles
                        .into_iter()
                        .filter_map(|(profile_name, properties)| {
                            let (properties, _) = validation.with_context_returning(
                                vec![("profile", profile_name.to_string())],
                                |validation| {
                                    Self::convert_and_validate_component_properties(
                                        validation, source, properties,
                                    )
                                },
                            );
                            properties.map(|properties| {
                                (BuildProfileName::from(profile_name), properties)
                            })
                        })
                        .collect()
                },
            })
        }

        fn resolve_directly_defined_non_profiled_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let valid = validation.with_context(vec![], |validation| {
                if component.default_profile.is_some() {
                    validation.add_error(format!(
                        "When {} is not defined then {} should not be defined",
                        "profiles".log_color_highlight(),
                        "defaultProfile".log_color_highlight()
                    ));
                }
            });

            valid
                .then(|| {
                    Self::convert_and_validate_component_properties(
                        validation,
                        source,
                        component.component_properties,
                    )
                })
                .flatten()
                .map(|properties| ResolvedComponentProperties::Properties {
                    template_name: None,
                    any_template_overrides: false,
                    properties,
                })
        }

        fn convert_and_validate_templated_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: &TemplateName,
            template_properties: &app_raw::ComponentProperties,
            component_name: &AppComponentName,
            component_properties: Option<app_raw::ComponentProperties>,
        ) -> Option<(ComponentProperties, bool)> {
            ComponentProperties::from_raw_template(
                validation,
                source,
                template_env,
                &Self::template_context(component_name),
                template_properties,
            )
            .inspect_err(|err| {
                validation.add_error(format!(
                    "Failed to render template {}, error: {}",
                    template_name.as_str().log_color_highlight(),
                    err.to_string().log_color_error_highlight()
                ))
            })
            .ok()
            .and_then(
                |rendered_template_properties| match rendered_template_properties {
                    Some(rendered_template_properties) => match component_properties {
                        Some(component_properties) => rendered_template_properties
                            .merge_with_overrides(validation, source, component_properties)
                            .inspect_err(|err| {
                                validation.add_error(format!(
                                    "Failed to override template {}, error: {}",
                                    template_name.as_str().log_color_highlight(),
                                    err.to_string().log_color_error_highlight()
                                ))
                            })
                            .ok()
                            .flatten(),
                        None => Some((rendered_template_properties, false)),
                    },
                    None => None,
                },
            )
            .inspect(|(properties, _)| {
                Self::validate_resolved_component_properties(validation, properties)
            })
        }

        fn convert_and_validate_component_properties(
            validation: &mut ValidationBuilder,
            source: &Path,
            component_properties: app_raw::ComponentProperties,
        ) -> Option<ComponentProperties> {
            ComponentProperties::from_raw(validation, source, component_properties).inspect(
                |properties| Self::validate_resolved_component_properties(validation, properties),
            )
        }

        fn validate_resolved_component_properties(
            validation: &mut ValidationBuilder,
            properties: &ComponentProperties,
        ) {
            for (name, value) in [
                ("sourceWit", &properties.source_wit),
                ("generatedWit", &properties.generated_wit),
                ("componentWasm", &properties.component_wasm),
            ] {
                if value.is_empty() {
                    validation.add_error(format!(
                        "Property {} is empty or undefined",
                        name.log_color_highlight()
                    ));
                }
            }
        }
    }
}
