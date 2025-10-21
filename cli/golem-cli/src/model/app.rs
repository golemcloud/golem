use crate::config::ProfileName;
use crate::fs;
use crate::log::LogColorize;
use crate::model::app::app_builder::{build_application, build_profiles};
use crate::model::app_raw;
use crate::model::component::AppComponentType;
use crate::model::template::Template;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::naming::wit::package_dep_dir_name_from_parser;
use crate::wasm_rpc_stubgen::stub::RustDependencyOverride;
use anyhow::anyhow;
use golem_common::model::{ComponentFilePathWithPermissions, ComponentFilePermissions};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Formatter;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

#[derive(Clone, Debug)]
pub struct ApplicationConfig {
    pub skip_up_to_date_checks: bool,
    pub build_profile: Option<BuildProfileName>,
    pub offline: bool,
    pub steps_filter: HashSet<AppBuildStep>,
    pub golem_rust_override: RustDependencyOverride,
    pub dev_mode: bool,
    pub enable_wasmtime_fs_cache: bool,
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
    ByRootManifest(PathBuf),
    Preloaded {
        raw_apps: Vec<app_raw::ApplicationWithSource>,
        calling_working_dir: PathBuf,
    },
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
    profile: Option<ProfileName>,
    components: bool,
    custom_commands: bool,
    builtin_commands: BTreeSet<String>,
    api_definitions: bool,
    api_deployments: bool,
}

impl DynamicHelpSections {
    pub fn show_all(profile: ProfileName, builtin_commands: BTreeSet<String>) -> Self {
        Self {
            profile: Some(profile),
            components: true,
            custom_commands: true,
            builtin_commands,
            api_definitions: true,
            api_deployments: true,
        }
    }

    pub fn show_components() -> Self {
        Self {
            profile: None,
            components: true,
            custom_commands: false,
            builtin_commands: Default::default(),
            api_definitions: false,
            api_deployments: false,
        }
    }

    pub fn show_custom_commands(builtin_commands: BTreeSet<String>) -> Self {
        Self {
            profile: None,
            components: false,
            custom_commands: true,
            builtin_commands,
            api_definitions: false,
            api_deployments: false,
        }
    }

    pub fn show_api_definitions() -> Self {
        Self {
            profile: None,
            components: false,
            custom_commands: false,
            builtin_commands: Default::default(),
            api_definitions: true,
            api_deployments: false,
        }
    }

    pub fn show_api_deployments(profile: ProfileName) -> Self {
        Self {
            profile: Some(profile),
            components: true,
            custom_commands: false,
            builtin_commands: Default::default(),
            api_definitions: true,
            api_deployments: false,
        }
    }

    pub fn components(&self) -> bool {
        self.components
    }

    pub fn custom_commands(&self) -> bool {
        self.custom_commands
    }

    pub fn builtin_commands(&self) -> &BTreeSet<String> {
        &self.builtin_commands
    }

    pub fn api_definitions(&self) -> bool {
        self.api_definitions
    }

    pub fn api_deployments_profile(&self) -> Option<&ProfileName> {
        (self.api_deployments)
            .then_some(self.profile.as_ref())
            .flatten()
    }
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

impl AppComponentName {
    pub fn to_package_name(&self) -> anyhow::Result<wit_parser::PackageName> {
        let component_name_str = self.as_str();
        let package_name_re =
            regex::Regex::new(r"^(?P<namespace>[^:]+):(?P<name>[^@]+)(?:@(?P<version>.+))?$")?;
        let captures = package_name_re
            .captures(component_name_str)
            .ok_or_else(|| anyhow!("Invalid component name format: {}", component_name_str))?;
        let namespace = captures.name("namespace").unwrap().as_str().to_string();
        let name = captures.name("name").unwrap().as_str().to_string();
        let version = captures
            .name("version")
            .map(|m| m.as_str().to_string())
            .map(|v| semver::Version::parse(&v))
            .transpose()?;

        Ok(wit_parser::PackageName {
            namespace,
            name,
            version,
        })
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HttpApiDefinitionName(String);

impl HttpApiDefinitionName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl Display for HttpApiDefinitionName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for HttpApiDefinitionName {
    fn from(value: String) -> Self {
        HttpApiDefinitionName(value)
    }
}

impl From<&str> for HttpApiDefinitionName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HttpApiDeploymentSite {
    pub host: String,
    pub subdomain: Option<String>,
}

impl Display for HttpApiDeploymentSite {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.subdomain {
            Some(subdomain) => write!(f, "{}.{}", subdomain, self.host),
            None => write!(f, "{}", self.host),
        }
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
#[allow(clippy::large_enum_variant)]
pub enum ResolvedComponentProperties {
    NonProfiled {
        template_name: Option<TemplateName>,
        properties: ComponentProperties,
    },
    Profiled {
        template_name: Option<TemplateName>,
        default_profile: BuildProfileName,
        profiles: HashMap<BuildProfileName, ComponentProperties>,
    },
}

pub struct ComponentEffectivePropertySource<'a> {
    pub template_name: Option<&'a TemplateName>,
    pub profile: Option<&'a BuildProfileName>,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default, EnumIter)]
pub enum DependencyType {
    #[default]
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

    pub fn interactively_selectable_types() -> Vec<Self> {
        Self::iter()
            .filter(|dep_type| dep_type != &DependencyType::StaticWasmRpc)
            .collect()
    }
}

impl FromStr for DependencyType {
    type Err = String;

    fn from_str(str: &str) -> Result<Self, Self::Err> {
        match str {
            Self::WASM_RPC => Ok(Self::DynamicWasmRpc),
            Self::STATIC_WASM_RPC => Ok(Self::StaticWasmRpc),
            Self::WASM => Ok(Self::Wasm),
            _ => {
                let all = DependencyType::iter()
                    .map(|dt| format!("\"{dt}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!(
                    "Unknown dependency type: {str}. Expected one of {all}"
                ))
            }
        }
    }
}

impl Display for DependencyType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinaryComponentSource {
    AppComponent { name: AppComponentName },
    LocalFile { path: PathBuf },
    Url { url: Url },
}

impl Display for BinaryComponentSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryComponentSource::AppComponent { name } => write!(f, "{name}"),
            BinaryComponentSource::LocalFile { path } => write!(f, "{}", path.display()),
            BinaryComponentSource::Url { url } => write!(f, "{url}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DependentComponent {
    pub source: BinaryComponentSource,
    pub dep_type: DependencyType,
}

impl DependentComponent {
    pub fn as_dependent_app_component(&self) -> Option<DependentAppComponent> {
        match &self.source {
            BinaryComponentSource::AppComponent { name } => Some(DependentAppComponent {
                name: name.clone(),
                dep_type: self.dep_type,
            }),
            _ => None,
        }
    }
}

impl PartialOrd for DependentComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DependentComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.source.cmp(&other.source)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DependentAppComponent {
    pub name: AppComponentName,
    pub dep_type: DependencyType,
}

impl PartialOrd for DependentAppComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DependentAppComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

pub type MultiSourceHttpApiDefinitionNames = Vec<WithSource<Vec<HttpApiDefinitionName>>>;

#[derive(Clone, Debug)]
pub struct Application {
    all_sources: BTreeSet<PathBuf>,
    temp_dir: Option<WithSource<String>>,
    wit_deps: WithSource<Vec<String>>,
    components: BTreeMap<AppComponentName, Component>,
    dependencies: BTreeMap<AppComponentName, BTreeSet<DependentComponent>>,
    dependency_sources: BTreeMap<AppComponentName, BTreeMap<AppComponentName, PathBuf>>,
    no_dependencies: BTreeSet<DependentComponent>,
    custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
    clean: Vec<WithSource<String>>,
    http_api_definitions: BTreeMap<HttpApiDefinitionName, WithSource<app_raw::HttpApiDefinition>>,
    http_api_deployments:
        BTreeMap<ProfileName, BTreeMap<HttpApiDeploymentSite, MultiSourceHttpApiDefinitionNames>>,
}

impl Application {
    pub fn profiles_from_raw_apps(
        apps: &[app_raw::ApplicationWithSource],
    ) -> ValidatedResult<BTreeMap<ProfileName, app_raw::Profile>> {
        build_profiles(apps)
    }

    pub fn from_raw_apps(
        available_profiles: &BTreeSet<ProfileName>,
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Self> {
        build_application(available_profiles, apps)
    }

    pub fn all_sources(&self) -> &BTreeSet<PathBuf> {
        &self.all_sources
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

    pub fn wit_deps(&self) -> Vec<PathBuf> {
        self.wit_deps
            .value
            .iter()
            .cloned()
            .map(|path| self.wit_deps.source.join(path))
            .collect()
    }

    pub fn all_dependencies(&self) -> BTreeSet<DependentComponent> {
        self.dependencies.values().flatten().cloned().collect()
    }

    pub fn all_build_profiles(&self) -> BTreeSet<BuildProfileName> {
        self.component_names()
            .flat_map(|component_name| self.component_build_profiles(component_name))
            .collect()
    }

    pub fn all_option_build_profiles(&self) -> BTreeSet<Option<BuildProfileName>> {
        let mut profiles = self
            .component_names()
            .flat_map(|component_name| self.component_build_profiles(component_name))
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

    pub fn all_custom_commands_for_all_build_profiles(
        &self,
    ) -> BTreeMap<Option<BuildProfileName>, BTreeSet<String>> {
        let mut custom_commands = BTreeMap::<Option<BuildProfileName>, BTreeSet<String>>::new();

        custom_commands
            .entry(None)
            .or_default()
            .extend(self.custom_commands.keys().cloned());

        for profile in self.all_option_build_profiles() {
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

    pub fn rib_repl_history_file(&self) -> PathBuf {
        self.temp_dir().join(".rib_repl_history")
    }

    fn component(&self, component_name: &AppComponentName) -> &Component {
        self.components
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {component_name}"))
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

    pub fn dependency_source(
        &self,
        component_name: &AppComponentName,
        dependent_component_name: &AppComponentName,
    ) -> Option<&Path> {
        self.dependency_sources
            .get(component_name)
            .and_then(|sources| {
                sources
                    .get(dependent_component_name)
                    .map(|source| source.as_path())
            })
    }

    pub fn component_build_profiles(
        &self,
        component_name: &AppComponentName,
    ) -> BTreeSet<BuildProfileName> {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::NonProfiled { .. } => BTreeSet::new(),
            ResolvedComponentProperties::Profiled { profiles, .. } => {
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
            ResolvedComponentProperties::NonProfiled { template_name, .. } => {
                ComponentEffectivePropertySource {
                    template_name: template_name.as_ref(),
                    profile: None,
                }
            }
            ResolvedComponentProperties::Profiled {
                template_name,
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

                ComponentEffectivePropertySource {
                    template_name: template_name.as_ref(),
                    profile: Some(effective_profile),
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
            ResolvedComponentProperties::NonProfiled { properties, .. } => properties,
            ResolvedComponentProperties::Profiled {
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
        exports_package_name: &wit_parser::PackageName,
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
    pub fn component_linked_wasm(
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
    pub fn component_temp_linked_wasm(&self, component_name: &AppComponentName) -> PathBuf {
        self.temp_dir()
            .join("temp-linked-wasm")
            .join(format!("{}.wasm", component_name.as_str()))
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

    pub fn http_api_definitions(
        &self,
    ) -> &BTreeMap<HttpApiDefinitionName, WithSource<app_raw::HttpApiDefinition>> {
        &self.http_api_definitions
    }

    pub fn http_api_definition_source(&self, name: &HttpApiDefinitionName) -> PathBuf {
        self.http_api_definitions
            .get(name)
            .unwrap_or_else(|| panic!("HTTP API definition not found: {}", name.as_str()))
            .source
            .clone()
    }

    pub fn used_component_names_for_http_api_definition(
        &self,
        name: &HttpApiDefinitionName,
    ) -> Vec<AppComponentName> {
        self.http_api_definitions
            .get(name)
            .unwrap_or_else(|| panic!("HTTP API definition not found: {}", name.as_str()))
            .value
            .routes
            .iter()
            .filter_map(|route| {
                route
                    .binding
                    .component_name
                    .as_ref()
                    .map(|component_name| AppComponentName::from(component_name.as_str()))
            })
            .collect()
    }

    pub fn used_component_names_for_all_http_api_definition(&self) -> Vec<AppComponentName> {
        self.http_api_definitions
            .values()
            .flat_map(|def| {
                def.value.routes.iter().filter_map(|route| {
                    route
                        .binding
                        .component_name
                        .as_ref()
                        .map(|component_name| AppComponentName::from(component_name.as_str()))
                })
            })
            .collect()
    }

    pub fn http_api_deployments(
        &self,
        profile: &ProfileName,
    ) -> Option<&BTreeMap<HttpApiDeploymentSite, Vec<WithSource<Vec<HttpApiDefinitionName>>>>> {
        self.http_api_deployments.get(profile)
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
    pub build: Vec<app_raw::BuildCommand>,
    pub custom_commands: HashMap<String, Vec<app_raw::ExternalCommand>>,
    pub clean: Vec<String>,
    pub component_type: Option<AppComponentType>,
    pub files: Vec<InitialComponentFile>,
    pub plugins: Vec<PluginInstallation>,
    pub env: HashMap<String, String>,
}

impl ComponentProperties {
    fn from_raw(
        validation: &mut ValidationBuilder,
        source: &Path,
        raw: app_raw::ComponentProperties,
    ) -> Option<Self> {
        let files =
            InitialComponentFile::from_raw_vec(validation, source, raw.files.unwrap_or_default())?;
        let plugins =
            PluginInstallation::from_raw_vec(validation, source, raw.plugins.unwrap_or_default())?;

        Some(Self {
            source_wit: raw.source_wit.unwrap_or_default(),
            generated_wit: raw.generated_wit.unwrap_or_default(),
            component_wasm: raw.component_wasm.unwrap_or_default(),
            linked_wasm: raw.linked_wasm,
            build: raw.build,
            custom_commands: raw.custom_commands,
            clean: raw.clean,
            component_type: raw.component_type,
            files,
            plugins,
            env: Self::validate_and_normalize_env(validation, raw.env.unwrap_or_default()),
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
    ) -> anyhow::Result<Option<Self>> {
        let mut any_errors = false;

        if let Some(source_wit) = overrides.source_wit {
            self.source_wit = source_wit;
        }

        if let Some(generated_wit) = overrides.generated_wit {
            self.generated_wit = generated_wit;
        }

        if let Some(component_wasm) = overrides.component_wasm {
            self.component_wasm = component_wasm;
        }

        if overrides.linked_wasm.is_some() {
            self.linked_wasm = overrides.linked_wasm;
        }

        if !overrides.build.is_empty() {
            self.build = overrides.build;
        }

        if !overrides.custom_commands.is_empty() {
            self.custom_commands.extend(overrides.custom_commands)
        }

        if overrides.component_type.is_some() {
            self.component_type = overrides.component_type;
        }

        let files = overrides.files.unwrap_or_default();
        if !files.is_empty() {
            match InitialComponentFile::from_raw_vec(validation, source, files) {
                Some(files) => {
                    self.files.extend(files);
                }
                None => {
                    any_errors = true;
                }
            }
        }

        let plugins = overrides.plugins.unwrap_or_default();
        if !plugins.is_empty() {
            match PluginInstallation::from_raw_vec(validation, source, plugins) {
                Some(plugins) => {
                    self.plugins.extend(plugins);
                }
                None => {
                    any_errors = true;
                }
            }
        }

        let env = overrides.env.unwrap_or_default();
        if !env.is_empty() {
            self.env
                .extend(Self::validate_and_normalize_env(validation, env));
        }

        Ok((!any_errors).then_some(self))
    }

    pub fn component_type(&self) -> AppComponentType {
        self.component_type.unwrap_or_default()
    }

    pub fn is_ephemeral(&self) -> bool {
        self.component_type() == AppComponentType::Ephemeral
    }

    pub fn is_durable(&self) -> bool {
        self.component_type() == AppComponentType::Durable
    }

    pub fn is_deployable(&self) -> bool {
        self.component_type()
            .as_deployable_component_type()
            .is_some()
    }

    fn validate_and_normalize_env(
        validation: &mut ValidationBuilder,
        env: HashMap<String, String>,
    ) -> HashMap<String, String> {
        env.into_iter()
            .map(|(key, value)| {
                let upper_case_key = key.to_uppercase();
                if upper_case_key != key {
                    validation.add_error(format!(
                        "Only uppercase environment variable names are allowed, found: {}",
                        key.log_color_highlight()
                    ));
                }
                if upper_case_key.starts_with("GOLEM_") {
                    validation.add_warn(format!(
                        concat!(
                        "Using environment names starting with 'GOLEM_' ({}) is not recommended, ",
                        "as those are reserved for variables set by Golem and might be overridden."
                        ),
                        key.log_color_highlight()
                    ));
                }
                (upper_case_key, value)
            })
            .collect::<HashMap<_, _>>()
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

#[derive(Debug, Clone)]
pub struct PluginInstallation {
    pub name: String,
    pub version: String,
    pub parameters: HashMap<String, String>,
}

impl PluginInstallation {
    pub fn from_raw(
        _validation: &mut ValidationBuilder,
        _source: &Path,
        file: app_raw::PluginInstallation,
    ) -> Option<PluginInstallation> {
        Some(PluginInstallation {
            name: file.name,
            version: file.version,
            parameters: file.parameters,
        })
    }

    pub fn from_raw_vec(
        validation: &mut ValidationBuilder,
        source: &Path,
        files: Vec<app_raw::PluginInstallation>,
    ) -> Option<Vec<Self>> {
        let source_count = files.len();

        let files = files
            .into_iter()
            .filter_map(|file| PluginInstallation::from_raw(validation, source, file))
            .collect::<Vec<_>>();

        (files.len() == source_count).then_some(files)
    }
}

mod app_builder {
    use crate::config::ProfileName;
    use crate::fs::PathExtra;
    use crate::fuzzy;
    use crate::fuzzy::FuzzySearch;
    use crate::log::LogColorize;
    use crate::model::api::to_method_pattern;
    use crate::model::app::{
        AppComponentName, Application, BinaryComponentSource, BuildProfileName, Component,
        ComponentProperties, DependencyType, DependentComponent, HttpApiDefinitionName,
        HttpApiDeploymentSite, MultiSourceHttpApiDefinitionNames, ResolvedComponentProperties,
        TemplateName, WithSource,
    };
    use crate::model::app_raw;
    use crate::model::deploy_diff::api_definition::normalize_http_api_binding_path;
    use crate::model::text::fmt::format_rib_source_for_error;
    use crate::validation::{ValidatedResult, ValidationBuilder};
    use colored::Colorize;
    use heck::{
        ToKebabCase, ToLowerCamelCase, ToPascalCase, ToShoutyKebabCase, ToShoutySnakeCase,
        ToSnakeCase, ToTitleCase, ToTrainCase, ToUpperCamelCase,
    };
    use itertools::Itertools;
    use serde::Serialize;
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::fmt::Debug;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use url::Url;

    // Load full manifest EXCEPT profiles
    pub fn build_application(
        available_profiles: &BTreeSet<ProfileName>,
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Application> {
        AppBuilder::build_app(available_profiles, apps)
    }

    // Load only profiles
    pub fn build_profiles(
        apps: &[app_raw::ApplicationWithSource],
    ) -> ValidatedResult<BTreeMap<ProfileName, app_raw::Profile>> {
        AppBuilder::build_profiles(apps)
    }

    #[derive(Debug, PartialEq, Eq, Hash)]
    enum UniqueSourceCheckedEntityKey {
        Include,
        TempDir,
        WitDeps,
        CustomCommand(String),
        Template(TemplateName),
        Dependency((AppComponentName, DependentComponent)),
        Component(AppComponentName),
        HttpApiDefinition(HttpApiDefinitionName),
        HttpApiDefinitionRoute {
            api: HttpApiDefinitionName,
            method: String,
            path: String,
        },
        HttpApiDeployment {
            profile: ProfileName,
            site: HttpApiDeploymentSite,
            definition: HttpApiDefinitionName,
        },
        Profile(ProfileName),
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
                UniqueSourceCheckedEntityKey::Dependency(_) => "Dependency",
                UniqueSourceCheckedEntityKey::Component(_) => "Component",
                UniqueSourceCheckedEntityKey::HttpApiDefinition(_) => "HTTP API Definition",
                UniqueSourceCheckedEntityKey::HttpApiDefinitionRoute { .. } => {
                    "HTTP API Definition Route"
                }
                UniqueSourceCheckedEntityKey::HttpApiDeployment { .. } => "HTTP API Deployment",
                UniqueSourceCheckedEntityKey::Profile(_) => "Profile",
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
                UniqueSourceCheckedEntityKey::Dependency((component_name, dependent_component)) => {
                    format!(
                        "{} - {} - {}",
                        component_name.as_str().log_color_highlight(),
                        dependent_component.source.to_string().log_color_highlight(),
                        dependent_component.dep_type.as_str().log_color_highlight(),
                    )
                }
                UniqueSourceCheckedEntityKey::Component(component_name) => {
                    component_name.as_str().log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::HttpApiDefinition(api_definition_name) => {
                    api_definition_name
                        .as_str()
                        .log_color_highlight()
                        .to_string()
                }
                UniqueSourceCheckedEntityKey::HttpApiDefinitionRoute { api, method, path } => {
                    format!(
                        "{} - {} - {}",
                        api.as_str().log_color_highlight(),
                        method.log_color_highlight(),
                        path.log_color_highlight(),
                    )
                }
                UniqueSourceCheckedEntityKey::HttpApiDeployment {
                    profile,
                    site,
                    definition,
                } => {
                    format!(
                        "{} - {}{} - {}",
                        profile.0.as_str().log_color_highlight(),
                        match site.subdomain {
                            Some(subdomain) => {
                                format!("{}.", subdomain.as_str().log_color_highlight())
                            }
                            None => {
                                "".to_string()
                            }
                        },
                        site.host.as_str().log_color_highlight(),
                        definition.as_str().log_color_highlight(),
                    )
                }
                UniqueSourceCheckedEntityKey::Profile(profile_name) => {
                    profile_name.0.log_color_highlight().to_string()
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
        http_api_definitions:
            BTreeMap<HttpApiDefinitionName, WithSource<app_raw::HttpApiDefinition>>,
        http_api_deployments: BTreeMap<
            ProfileName,
            BTreeMap<HttpApiDeploymentSite, MultiSourceHttpApiDefinitionNames>,
        >,

        // NOTE: raw component names are available (for validation) even after component resolving
        raw_component_names: HashSet<String>,
        // NOTE: raw components are moved into resolved_components during resolving
        raw_components: HashMap<AppComponentName, (PathBuf, app_raw::Component)>,
        resolved_components: BTreeMap<AppComponentName, Component>,

        profiles: BTreeMap<ProfileName, app_raw::Profile>,

        all_sources: BTreeSet<PathBuf>,
        entity_sources: HashMap<UniqueSourceCheckedEntityKey, Vec<PathBuf>>,
    }

    impl AppBuilder {
        // NOTE: build_app DOES NOT include profiles, those are preloaded with build_profiles, so
        //       flows that do not use manifest otherwise won't get blocked by high-level validation errors,
        //       and we do not "steal" manifest loading logs from those which do use the manifest fully.
        fn build_app(
            available_profiles: &BTreeSet<ProfileName>,
            apps: Vec<app_raw::ApplicationWithSource>,
        ) -> ValidatedResult<Application> {
            let mut builder = Self::default();
            let mut validation = ValidationBuilder::default();

            for app in apps {
                builder.add_raw_app(&mut validation, app);
            }
            builder.resolve_components(&mut validation);
            builder.validate_dependency_targets(&mut validation);
            builder.validate_unique_sources(&mut validation);
            builder.validate_http_api_definitions(&mut validation);
            builder.validate_http_api_deployments(&mut validation, available_profiles);

            let dependency_sources = {
                let mut dependency_sources =
                    BTreeMap::<AppComponentName, BTreeMap<AppComponentName, PathBuf>>::new();

                for (key, mut sources) in builder.entity_sources {
                    if let UniqueSourceCheckedEntityKey::Dependency((
                        component,
                        dependent_component,
                    )) = key
                    {
                        if let Some(dependent_component) =
                            dependent_component.as_dependent_app_component()
                        {
                            if !dependency_sources.contains_key(&component) {
                                dependency_sources.insert(component.clone(), BTreeMap::new());
                            }
                            dependency_sources
                                .get_mut(&component)
                                .unwrap()
                                .insert(dependent_component.name, sources.pop().unwrap());
                        }
                    }
                }

                dependency_sources
            };

            validation.build(Application {
                all_sources: builder.all_sources,
                temp_dir: builder.temp_dir,
                wit_deps: builder.wit_deps,
                components: builder.resolved_components,
                dependencies: builder.dependencies,
                dependency_sources,
                no_dependencies: BTreeSet::new(),
                custom_commands: builder.custom_commands,
                clean: builder.clean,
                http_api_definitions: builder.http_api_definitions,
                http_api_deployments: builder.http_api_deployments,
            })
        }

        // NOTE: Unlike build_app, here we do not consume the source apps, so they can be
        //       used for build_app. For more info on this separation, see build_app.
        //
        //       Ironically build_profiles does not build BuildProfiles, only regular ones.
        fn build_profiles(
            apps: &[app_raw::ApplicationWithSource],
        ) -> ValidatedResult<BTreeMap<ProfileName, app_raw::Profile>> {
            let mut builder = Self::default();
            let mut validation = ValidationBuilder::default();

            for app in apps {
                builder.add_raw_app_profiles_only(&mut validation, app);
            }

            validation.build(builder.profiles)
        }

        fn add_entity_source(&mut self, key: UniqueSourceCheckedEntityKey, source: &Path) -> bool {
            let sources = self.entity_sources.entry(key).or_default();
            let is_first = sources.is_empty();
            sources.push(source.to_path_buf());
            is_first
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
                    self.all_sources.insert(app_source.to_path_buf());

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
                        self.add_and_resolve_raw_template(
                            validation,
                            &app.source,
                            template_name,
                            template,
                        );
                    }

                    for (component_name, component) in app.application.components {
                        let app_component_name = AppComponentName::from(component_name.clone());
                        let unique_key =
                            UniqueSourceCheckedEntityKey::Component(app_component_name.clone());
                        if self.add_entity_source(unique_key, &app.source) {
                            self.raw_component_names.insert(component_name);
                            self.raw_components
                                .insert(app_component_name, (app.source.to_path_buf(), component));
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

                    if let Some(http_api) = app.application.http_api {
                        for (api_definition_name, api_definition) in http_api.definitions {
                            let api_definition_name =
                                HttpApiDefinitionName::from(api_definition_name);
                            if self.add_entity_source(
                                UniqueSourceCheckedEntityKey::HttpApiDefinition(
                                    api_definition_name.clone(),
                                ),
                                &app.source,
                            ) {
                                for route in &api_definition.routes {
                                    let Ok(method) = to_method_pattern(&route.method) else {
                                        continue;
                                    };

                                    self.add_entity_source(
                                        UniqueSourceCheckedEntityKey::HttpApiDefinitionRoute {
                                            api: api_definition_name.clone(),
                                            method: method.to_string(),
                                            path: normalize_http_api_binding_path(&route.path),
                                        },
                                        &app.source,
                                    );
                                }

                                self.http_api_definitions.insert(
                                    api_definition_name,
                                    WithSource::new(app.source.to_path_buf(), api_definition),
                                );
                            }
                        }

                        for (profile, deployments) in http_api.deployments {
                            let mut collected_deployments =
                                BTreeMap::<HttpApiDeploymentSite, Vec<HttpApiDefinitionName>>::new(
                                );

                            for api_deployment in deployments {
                                let api_deployment_site = HttpApiDeploymentSite {
                                    host: api_deployment.host.clone(),
                                    subdomain: api_deployment.subdomain.clone(),
                                };

                                let mut unique_definitions =
                                    Vec::with_capacity(api_deployment.definitions.len());
                                for definition in api_deployment.definitions {
                                    let definition = HttpApiDefinitionName::from(definition);
                                    if self.add_entity_source(
                                        UniqueSourceCheckedEntityKey::HttpApiDeployment {
                                            profile: profile.clone(),
                                            site: api_deployment_site.clone(),
                                            definition: definition.clone(),
                                        },
                                        &app.source,
                                    ) {
                                        unique_definitions.push(definition);
                                    }
                                }

                                if !unique_definitions.is_empty() {
                                    collected_deployments
                                        .insert(api_deployment_site, unique_definitions);
                                }
                            }

                            if !collected_deployments.is_empty() {
                                let deployments =
                                    self.http_api_deployments.entry(profile).or_default();

                                for (site, definitions) in collected_deployments {
                                    deployments.entry(site).or_default().push(WithSource::new(
                                        app.source.to_path_buf(),
                                        definitions,
                                    ))
                                }
                            }
                        }
                    }
                },
            );
        }

        fn add_raw_app_profiles_only(
            &mut self,
            validation: &mut ValidationBuilder,
            app: &app_raw::ApplicationWithSource,
        ) {
            fn check_not_allowed_for_profile<T>(
                message_suffix: &str,
                validation: &mut ValidationBuilder,
                property_name: &str,
                value: &Option<T>,
            ) {
                if value.is_some() {
                    validation.add_error(format!(
                        "Property {} is not allowed for {}",
                        property_name.log_color_highlight(),
                        message_suffix
                    ))
                }
            }

            let mut default_profile_names = BTreeSet::<ProfileName>::new();

            validation.with_context(
                vec![("source", app.source.to_string_lossy().to_string())],
                |validation| {
                    for (profile_name, profile) in &app.application.profiles {
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::Profile(profile_name.clone()),
                            &app.source,
                        ) {
                            if profile.default == Some(true) {
                                default_profile_names.insert(profile_name.clone());
                            };

                            self.profiles.insert(profile_name.clone(), profile.clone());
                            validation.with_context(
                                vec![("profile", profile_name.to_string())],
                                |validation| {
                                    let is_builtin_local = profile_name.is_builtin_local();
                                    let is_cloud = profile_name.is_builtin_cloud();
                                    if is_builtin_local || is_cloud {
                                        check_not_allowed_for_profile(
                                            &format!("{} profiles", "Cloud".log_color_highlight()),
                                            validation,
                                            "url",
                                            &profile.url,
                                        );

                                        check_not_allowed_for_profile(
                                            &format!(
                                                "builtin {} profiles",
                                                "local".log_color_highlight()
                                            ),
                                            validation,
                                            "worker_url",
                                            &profile.worker_url,
                                        );
                                    }
                                    if is_builtin_local && is_cloud {
                                        validation.add_error(format!(
                                            "Builtin profile '{}' cannot be used as Cloud profile, using 'cloud:true' or project are not allowed!",
                                            profile_name.0.log_color_highlight(),
                                        ))
                                    }

                                    if profile.reset.unwrap_or_default() {
                                        if profile.redeploy_all.unwrap_or_default() {
                                            validation.add_error(format!(
                                                "Property '{}' and '{}' cannot be set to true at the same time!",
                                                "reset".log_color_highlight(),
                                                "redeploy".log_color_highlight()
                                            ));
                                        }

                                        if profile.redeploy_agents.unwrap_or_default() {
                                            validation.add_error(format!(
                                                "Property '{}' and '{}' cannot be set to true at the same time!",
                                                "reset".log_color_highlight(),
                                                "redeploy_agents".log_color_highlight()
                                            ));
                                        }
                                    }
                                },
                            );
                        }
                    }
                },
            );

            if default_profile_names.len() > 1 {
                validation.add_error(format!(
                    "Only one profile can be used as default! Profiles marked as default: {}",
                    default_profile_names
                        .iter()
                        .map(|pn| pn.0.log_color_highlight())
                        .join(", ")
                ));
            }
        }

        fn add_and_resolve_raw_template(
            &mut self,
            validation: &mut ValidationBuilder,
            source: &Path,
            template_name: String,
            template: app_raw::ComponentTemplate,
        ) {
            let template = template.merge_common_properties_into_profiles();

            validation.with_context(vec![("template", template_name.clone())], |validation| {
                if template.profiles.is_empty() {
                    if template.default_profile.is_some() {
                        validation.add_error(format!(
                            "Property {} is not allowed if no {} are defined",
                            "defaultProfile".log_color_highlight(),
                            "profiles".log_color_highlight(),
                        ));
                    }
                } else {
                    match &template.default_profile {
                        Some(default_profile) => {
                            if !template.profiles.contains_key(default_profile) {
                                validation.add_error(format!(
                                    "Unknown {}: {}\n\n{}",
                                    "defaultProfile".log_color_highlight(),
                                    default_profile.log_color_highlight(),
                                    self.available_profiles(
                                        template.profiles.keys().map(|s| s.as_str()),
                                        default_profile
                                    ),
                                ))
                            }
                        }
                        None => {
                            validation.add_error(format!(
                                "Property {} is mandatory when using {}",
                                "defaultProfile".log_color_highlight(),
                                "profiles".log_color_highlight(),
                            ));
                        }
                    }
                }
            });

            let template_name = TemplateName::from(template_name);
            if self.add_entity_source(
                UniqueSourceCheckedEntityKey::Template(template_name.clone()),
                source,
            ) {
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
                        let binary_component_source = match (dependency.target, dependency.path, dependency.url) {
                            (Some(target_name), None, None) => {
                                Some(BinaryComponentSource::AppComponent {
                                    name: target_name.into(),
                                })
                            }
                            (None, Some(path), None) => {
                                Some(BinaryComponentSource::LocalFile { path: Path::new(&path).to_path_buf() })
                            }
                            (None, None, Some(url)) => {
                                match Url::from_str(&url) {
                                    Ok(url) => {
                                        Some(BinaryComponentSource::Url { url })
                                    }
                                    Err(_) => {
                                        validation.add_error(format!(
                                            "Invalid URL for component dependency: {}",
                                            url.log_color_highlight()
                                        ));
                                        None
                                    }
                                }
                            }
                            (None, None, None) => {
                                validation.add_error(format!(
                                    "Missing one of the {}/{}/{} fields for component dependency",
                                    "target".log_color_error_highlight(),
                                    "path".log_color_error_highlight(),
                                    "url".log_color_error_highlight()
                                ));
                                None
                            }
                            _ => {
                                validation.add_error(format!(
                                    "Only one of the {}/{}/{} fields can be specified for a component dependency",
                                    "target".log_color_error_highlight(),
                                    "path".log_color_error_highlight(),
                                    "url".log_color_error_highlight()
                                ));
                                None
                            }
                        };

                        if let Some(binary_component_source) = binary_component_source {
                            let dependent_component = DependentComponent {
                                source: binary_component_source,
                                dep_type,
                            };

                            let unique_key = UniqueSourceCheckedEntityKey::Dependency((
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
                    let invalid_source = !self.raw_component_names.contains(&component.0);
                    let invalid_target = match &target.source {
                        BinaryComponentSource::AppComponent { name } => {
                            !self.raw_component_names.contains(&name.0)
                        }
                        BinaryComponentSource::LocalFile { path } => {
                            !std::fs::exists(path).unwrap_or(false)
                        }
                        BinaryComponentSource::Url { .. } => false,
                    };
                    let invalid_target_source = match (&target.dep_type, &target.source) {
                        (
                            DependencyType::DynamicWasmRpc,
                            BinaryComponentSource::AppComponent { .. },
                        ) => {
                            false // valid
                        }
                        (
                            DependencyType::StaticWasmRpc,
                            BinaryComponentSource::AppComponent { .. },
                        ) => {
                            false // valid
                        }
                        (DependencyType::Wasm, _) => {
                            false // valid
                        }
                        _ => true,
                    };

                    if invalid_source || invalid_target || invalid_target_source {
                        let source = self
                            .entity_sources
                            .get(&UniqueSourceCheckedEntityKey::Dependency((
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
                                        "{} {} - {} references unknown component: {}\n\n{}",
                                        target.dep_type.describe(),
                                        component.as_str().log_color_highlight(),
                                        target.source.to_string().log_color_highlight(),
                                        component.as_str().log_color_error_highlight(),
                                        self.available_components(component.as_str())
                                    ))
                                }
                                if invalid_target {
                                    validation.add_error(format!(
                                        "{} {} - {} references unknown target component: {}\n\n{}",
                                        target.dep_type.describe(),
                                        component.as_str().log_color_highlight(),
                                        target.source.to_string().log_color_highlight(),
                                        target.source.to_string().log_color_error_highlight(),
                                        self.available_components(&target.source.to_string())
                                    ))
                                }
                                if invalid_target_source {
                                    validation.add_error(format!(
                                        "{} {} - {}: this dependency type only supports local component targets\n",
                                        target.dep_type.describe(),
                                        component.as_str().log_color_highlight(),
                                        target.source.to_string().log_color_highlight(),
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
                            match self.templates.get(&template_name) {
                                Some(template) => self.resolve_templated_component_properties(
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
                                        "Component references unknown template: {}\n\n{}",
                                        template_name.as_str().log_color_error_highlight(),
                                        self.available_templates(template_name.as_str())
                                    ));
                                    None
                                }
                            }
                        }
                        None => self.resolve_directly_defined_component_properties(
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
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: TemplateName,
            template: &app_raw::ComponentTemplate,
            component_name: AppComponentName,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let (properties, _) = validation.with_context_returning(
                vec![("template", template_name.to_string())],
                |validation| {
                    if template.profiles.is_empty() {
                        self.resolve_templated_non_profiled_component_properties(
                            validation,
                            source,
                            template_env,
                            template_name,
                            template,
                            component_name,
                            component.component_properties,
                        )
                    } else {
                        self.resolve_templated_profiled_component_properties(
                            validation,
                            source,
                            template_env,
                            template_name,
                            template,
                            component_name,
                            component,
                        )
                    }
                },
            );

            properties
        }

        fn resolve_templated_non_profiled_component_properties(
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: TemplateName,
            template: &app_raw::ComponentTemplate,
            component_name: AppComponentName,
            component_properties: app_raw::ComponentProperties,
        ) -> Option<ResolvedComponentProperties> {
            self.convert_and_validate_templated_component_properties(
                validation,
                source,
                template_env,
                &template_name,
                &template.component_properties,
                &component_name,
                Some(component_properties),
            )
            .map(|properties| ResolvedComponentProperties::NonProfiled {
                template_name: Some(template_name),
                properties,
            })
        }

        fn resolve_templated_profiled_component_properties(
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: TemplateName,
            template: &app_raw::ComponentTemplate,
            component_name: AppComponentName,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let mut component =
                component.merge_common_properties_into_profiles(template.profiles.keys());

            let (profiles, valid) = validation.with_context_returning(vec![], |validation| {
                let mut resolved_profiles = HashMap::<BuildProfileName, ComponentProperties>::new();

                for (profile_name, template_component_properties) in &template.profiles {
                    validation.with_context(
                        vec![("profile", profile_name.to_string())],
                        |validation| {
                            let component_properties = component.profiles.remove(profile_name);
                            self.convert_and_validate_templated_component_properties(
                                validation,
                                source,
                                template_env,
                                &template_name,
                                template_component_properties,
                                &component_name,
                                component_properties,
                            )
                            .into_iter()
                            .for_each(|component_properties| {
                                resolved_profiles
                                    .insert(profile_name.clone().into(), component_properties);
                            });
                        },
                    );
                }

                if let Some(default_profile) = &component.default_profile {
                    if !resolved_profiles
                        .contains_key(&BuildProfileName::from(default_profile.as_str()))
                    {
                        validation.add_error(format!(
                            "Unknown {}: {}\n\n{}",
                            "defaultProfile".log_color_highlight(),
                            default_profile.log_color_highlight(),
                            self.available_profiles(
                                component.profiles.keys().map(|s| s.as_str()),
                                default_profile
                            ),
                        ))
                    }
                }

                resolved_profiles
            });

            valid.then(|| ResolvedComponentProperties::Profiled {
                template_name: Some(template_name),
                default_profile: component
                    .default_profile
                    .or(template.default_profile.clone())
                    .clone()
                    .expect("Missing template default profile")
                    .into(),
                profiles,
            })
        }

        fn resolve_directly_defined_component_properties(
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            if component.profiles.is_empty() {
                self.resolve_directly_defined_non_profiled_component_properties(
                    validation, source, component,
                )
            } else {
                self.resolve_directly_defined_profiled_component_properties(
                    validation, source, component,
                )
            }
        }

        fn resolve_directly_defined_profiled_component_properties(
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let valid =
                validation.with_context(vec![], |validation| match &component.default_profile {
                    Some(default_profile) => {
                        if !component.profiles.contains_key(default_profile) {
                            validation.add_error(format!(
                                "Unknown {}: {}\n\n{}",
                                "defaultProfile".log_color_highlight(),
                                default_profile.log_color_highlight(),
                                self.available_profiles(
                                    component.profiles.keys().map(|s| s.as_str()),
                                    default_profile
                                ),
                            ))
                        }
                    }
                    None => {
                        validation.add_error(format!(
                            "Property {} is not allowed if no {} are defined",
                            "defaultProfile".log_color_highlight(),
                            "profiles".log_color_highlight(),
                        ));
                    }
                });

            valid.then(|| ResolvedComponentProperties::Profiled {
                template_name: None,
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
                                    self.convert_and_validate_component_properties(
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
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            component: app_raw::Component,
        ) -> Option<ResolvedComponentProperties> {
            let valid = validation.with_context(vec![], |validation| {
                if component.default_profile.is_some() {
                    validation.add_error(format!(
                        "Property {} is not allowed if no {} are defined",
                        "defaultProfile".log_color_highlight(),
                        "profiles".log_color_highlight(),
                    ));
                }
            });

            valid
                .then(|| {
                    self.convert_and_validate_component_properties(
                        validation,
                        source,
                        component.component_properties,
                    )
                })
                .flatten()
                .map(|properties| ResolvedComponentProperties::NonProfiled {
                    template_name: None,
                    properties,
                })
        }

        fn convert_and_validate_templated_component_properties(
            &self,
            validation: &mut ValidationBuilder,
            source: &Path,
            template_env: &minijinja::Environment,
            template_name: &TemplateName,
            template_properties: &app_raw::ComponentProperties,
            component_name: &AppComponentName,
            component_properties: Option<app_raw::ComponentProperties>,
        ) -> Option<ComponentProperties> {
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
                        None => Some(rendered_template_properties),
                    },
                    None => None,
                },
            )
            .inspect(|properties| {
                Self::validate_resolved_component_properties(validation, properties)
            })
        }

        fn convert_and_validate_component_properties(
            &self,
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

        fn validate_http_api_definitions(&self, validation: &mut ValidationBuilder) {
            for (name, api_definition) in &self.http_api_definitions {
                validation.with_context(
                    vec![
                        (
                            "source",
                            api_definition.source.to_string_lossy().to_string(),
                        ),
                        ("HTTP API definition", name.0.to_string()),
                    ],
                    |validation| {
                        let def = &api_definition.value;

                        if let Some(project) = &def.project {
                            check_not_empty(validation, "project", project);
                        }
                        check_not_empty(validation, "version", &def.version);

                        for route in &def.routes {
                            validation.with_context(
                                vec![
                                    ("method", route.method.clone()),
                                    ("path", route.path.clone()),
                                ],
                                |validation| {
                                    if check_not_empty(validation, "method", &route.method) {
                                        if let Err(err) = to_method_pattern(&route.method) {
                                            validation.add_error(err.to_string());
                                        }
                                    }
                                    check_not_empty(validation, "path", &route.path);


                                    let binding_type = route.binding.type_.unwrap_or_default();
                                    let binding_type_as_string = serde_json::to_string(&binding_type).unwrap();

                                    let check_not_allowed = |validation: &mut ValidationBuilder, property_name: &str,
                                                             value: &Option<String>| {
                                        if value.is_some() {
                                            validation.add_error(
                                                format!(
                                                    "Property {} is not allowed with binding type {}",
                                                    property_name.log_color_highlight(),
                                                    binding_type_as_string.log_color_highlight(),
                                                )
                                            );
                                        }
                                    };

                                    let check_component_name_and_version = |validation: &mut ValidationBuilder|
                                        {
                                            match route.binding.component_name.as_deref() {
                                                Some(name) => {
                                                    if !self.raw_component_names.contains(name) {
                                                        validation.add_error(
                                                            format!(
                                                                "Property {} contains unknown component name: {}\n\n{}",
                                                                "component_name".log_color_highlight(),
                                                                name.log_color_error_highlight(),
                                                                self.available_components(name)
                                                            )
                                                        )
                                                    }
                                                }
                                                None => {
                                                    validation.add_error(
                                                        format!(
                                                            "Property {} is required for binding type {}",
                                                            "component_name".log_color_highlight(),
                                                            binding_type_as_string.log_color_highlight(),
                                                        )
                                                    );
                                                }
                                            }
                                        };

                                    let check_rib = |validation: &mut ValidationBuilder, property_name: &str, rib_script: &Option<String>, required: bool| {
                                        match rib_script.as_ref().map(|s| s.as_str()) {
                                            Some(rib) => {
                                                check_not_empty(validation, property_name, rib);
                                                if let Some(err) = rib::from_string(rib).err() {
                                                    validation.add_error(
                                                        format!(
                                                            "Failed to parse property {} as Rib:\n{}\n{}\n{}",
                                                            property_name.log_color_highlight(),
                                                            err.to_string().lines().map(|l| format!("  {l}")).join("\n").log_color_warn(),
                                                            "Rib source:".log_color_highlight(),
                                                            format_rib_source_for_error(rib, &err),
                                                        )
                                                    );
                                                }
                                            }
                                            None => {
                                                if required {
                                                    validation.add_error(
                                                        format!(
                                                            "Property {} is required for binding type {}",
                                                            property_name.log_color_highlight(),
                                                            binding_type_as_string.log_color_highlight(),
                                                        )
                                                    );
                                                }
                                            }
                                        }
                                    };

                                    match route.binding.type_.unwrap_or_default() {
                                        app_raw::HttpApiDefinitionBindingType::Default => {
                                            check_component_name_and_version(validation);
                                            check_rib(validation, "idempotency_key", &route.binding.idempotency_key, false);
                                            check_rib(validation, "invocation_context", &route.binding.invocation_context, false);
                                            check_rib(validation, "response", &route.binding.response, true);
                                        }
                                        app_raw::HttpApiDefinitionBindingType::CorsPreflight => {
                                            check_not_allowed(validation, "component_name", &route.binding.component_name);
                                            check_not_allowed(validation, "idempotency_key", &route.binding.idempotency_key);
                                            check_not_allowed(validation, "invocation_context", &route.binding.invocation_context);
                                            check_rib(validation, "response", &route.binding.response, false);
                                        }
                                        app_raw::HttpApiDefinitionBindingType::FileServer => {
                                            check_component_name_and_version(validation);
                                            check_rib(validation, "idempotency_key", &route.binding.idempotency_key, false);
                                            check_rib(validation, "invocation_context", &route.binding.invocation_context, false);
                                            check_rib(validation, "response", &route.binding.response, true);
                                        }
                                        app_raw::HttpApiDefinitionBindingType::HttpHandler => {
                                            check_component_name_and_version(validation);
                                            check_not_allowed(validation, "idempotency_key", &route.binding.idempotency_key);
                                            check_not_allowed(validation, "invocation_context", &route.binding.invocation_context);
                                            check_not_allowed(validation, "response", &route.binding.response);
                                        }
                                        app_raw::HttpApiDefinitionBindingType::SwaggerUi => {
                                            check_not_allowed(validation, "component_name", &route.binding.component_name);
                                            check_not_allowed(validation, "idempotency_key", &route.binding.idempotency_key);
                                            check_not_allowed(validation, "invocation_context", &route.binding.invocation_context);
                                            check_not_allowed(validation, "response", &route.binding.response);
                                        }
                                    }
                                },
                            );
                        }
                    },
                );
            }
        }

        fn validate_http_api_deployments(
            &self,
            validation: &mut ValidationBuilder,
            available_profiles: &BTreeSet<ProfileName>,
        ) {
            for (profile, api_deployments) in &self.http_api_deployments {
                if !available_profiles.contains(profile) {
                    validation.add_warn(format!(
                        "Unknown profile in manifest: {}\n\n{}",
                        profile.0.log_color_highlight(),
                        self.available_profiles(
                            available_profiles.iter().map(|p| p.0.as_str()),
                            &profile.0
                        )
                    ));
                }

                validation.with_context(
                    vec![("profile", profile.0.clone())],
                    |validation| {
                        for (site, api_definitions_with_source) in api_deployments {
                            for api_definitions in api_definitions_with_source {
                                validation.with_context(
                                    vec![
                                        (
                                            "source",
                                            api_definitions.source.to_string_lossy().to_string(),
                                        ),
                                        ("HTTP API deployment site", site.to_string()),
                                    ],
                                    |validation| {
                                        for def_name in &api_definitions.value {
                                            let parts = def_name.as_str().split("@").collect::<Vec<_>>();
                                            let (name, version) = match parts.len() {
                                                1 => (HttpApiDefinitionName::from(def_name.as_str()), None),
                                                2 => (HttpApiDefinitionName::from(parts[0]), Some(parts[1])),
                                                _ => {
                                                    validation.add_error(
                                                        format!(
                                                            "Invalid definition name: {}, expected 'api-name', or 'api-name@version'",
                                                            def_name.as_str().log_color_error_highlight(),
                                                        ),
                                                    );
                                                    continue;
                                                }
                                            };
                                            if name.0.is_empty() {
                                                validation.add_error(
                                                    format!(
                                                        "Invalid definition name, empty API name part: {}, expected 'api-name', or 'api-name@version'",
                                                        def_name.as_str().log_color_error_highlight(),
                                                    ),
                                                );
                                            } else if !self.http_api_definitions.contains_key(&name) {
                                                validation.add_error(
                                                    format!(
                                                        "Unknown HTTP API definition name: {}\n\n{}",
                                                        def_name.as_str().log_color_error_highlight(),
                                                        self.available_http_api_definitions(def_name.as_str())
                                                    ),
                                                )
                                            }
                                            if let Some(version) = version {
                                                if version.is_empty() {
                                                    validation.add_error(
                                                        format!(
                                                            "Invalid definition name, empty version part: {}, expected 'api-name', or 'api-name@version'",
                                                            def_name.as_str().log_color_error_highlight(),
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                    },
                                );
                            }
                        }
                    });
            }
        }

        fn available_profiles<'a, I: IntoIterator<Item = &'a str>>(
            &self,
            available_profiles: I,
            unknown: &str,
        ) -> String {
            self.available_options_help("profiles", "profile names", unknown, available_profiles)
        }

        fn available_templates(&self, unknown: &str) -> String {
            self.available_options_help(
                "templates",
                "template names",
                unknown,
                self.templates.keys().map(|name| name.as_str()),
            )
        }

        fn available_components(&self, unknown: &str) -> String {
            self.available_options_help(
                "components",
                "component names",
                unknown,
                self.raw_component_names.iter().map(|name| name.as_str()),
            )
        }

        fn available_http_api_definitions(&self, unknown: &str) -> String {
            self.available_options_help(
                "HTTP API definitions",
                "HTTP API definition names",
                unknown,
                self.http_api_definitions.keys().map(|name| name.as_str()),
            )
        }

        fn available_options_help<'a, I: IntoIterator<Item = &'a str>>(
            &self,
            entity_plural: &str,
            entity_name_plural: &str,
            unknown_option: &str,
            name_options: I,
        ) -> String {
            let options = name_options.into_iter().collect::<Vec<_>>();
            if options.is_empty() {
                return format!("No {entity_plural} are defined");
            }

            let fuzzy_search = FuzzySearch::new(options.iter().copied());

            let hint = match fuzzy_search.find(unknown_option) {
                Err(fuzzy::Error::Ambiguous {
                    highlighted_options,
                    ..
                }) => {
                    if highlighted_options.len() == 1 {
                        format!(
                            "Did you mean {}?\n\n",
                            highlighted_options[0].log_color_highlight()
                        )
                    } else {
                        format!(
                            "Did you mean one of {}?\n\n",
                            highlighted_options
                                .iter()
                                .map(|option| option.bold())
                                .join(",")
                        )
                    }
                }
                _ => "".to_string(),
            };

            format!(
                "{}{}\n{}",
                hint,
                format!("Available {entity_name_plural}:").log_color_help_group(),
                options.iter().map(|name| format!("- {name}")).join("\n")
            )
        }
    }

    fn check_not_empty(
        validation: &mut ValidationBuilder,
        property_name: &str,
        value: &str,
    ) -> bool {
        let is_empty = value.is_empty();
        if is_empty {
            validation.add_error(format!(
                "Property {} is empty",
                property_name.log_color_highlight()
            ));
        }
        !is_empty
    }
}

#[cfg(test)]
mod test {
    use crate::model::app::{AppComponentName, Application, BuildProfileName};
    use crate::model::app_raw;
    use crate::model::component::AppComponentType;
    use assert2::{assert, check};
    use indoc::indoc;
    use test_r::test;

    #[test]
    fn component_property_profiles_overrides() {
        let manifest = indoc! {"
            templates:
              template-profiled:
                sourceWit: common-a-source-wit
                generatedWit: common-a-generated-wit
                defaultProfile: debug
                profiles:
                  debug:
                    build:
                    - command: debug-a-build
                    sourceWit: debug-a-source-wit
                    componentWasm: debug-a-component-wasm
                    linkedWasm: debug-a-linked-wasm
                    env:
                      A: debug-a-env-var
                  release:
                    build:
                    - command: release-a-build
                    componentWasm: release-a-component-wasm
                    generatedWit: release-a-generated-wit
                    linkedWasm: release-a-linked-wasm
                    env:
                      A: release-a-env-var
                  release-custom:
                    build:
                    - command: release-custom-a-build
                    componentWasm: release-custom-a-component-wasm
                    generatedWit: release-custom-a-generated-wit
                    linkedWasm: release-custom-a-linked-wasm
                    env:
                      A: release-custom-a-env-var
                    componentType: ephemeral

            components:
              app:comp-profiled-a:
                template: template-profiled
                profiles:
                  release-custom:
                    componentType: durable
                    componentWasm: release-comp-a-component-wasm
                componentType: ephemeral
                componentWasm: comp-a-component-wasm
        "};

        let app = Application::from_raw_apps(
            &Default::default(),
            vec![app_raw::ApplicationWithSource::from_yaml_string(
                "dummy-source".into(),
                manifest.to_string(),
            )
            .unwrap()],
        );

        let (app, warns, errors) = app.into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let app = app.unwrap();

        let component_name_profiled_a = AppComponentName::from("app:comp-profiled-a");
        let debug_profile = BuildProfileName::from("debug");
        let release_profile = BuildProfileName::from("release");
        let release_custom_profile = BuildProfileName::from("release-custom");

        let debug_props =
            app.component_properties(&component_name_profiled_a, Some(&debug_profile));
        let release_props =
            app.component_properties(&component_name_profiled_a, Some(&release_profile));
        let release_custom_props =
            app.component_properties(&component_name_profiled_a, Some(&release_custom_profile));

        check!(debug_props.source_wit == "debug-a-source-wit");
        check!(release_props.source_wit == "common-a-source-wit");
        check!(release_custom_props.source_wit == "common-a-source-wit");

        check!(debug_props.generated_wit == "common-a-generated-wit");
        check!(release_props.generated_wit == "release-a-generated-wit");
        check!(release_custom_props.generated_wit == "release-custom-a-generated-wit");

        check!(debug_props.component_type() == AppComponentType::Ephemeral);
        check!(release_props.component_type() == AppComponentType::Ephemeral);
        check!(release_custom_props.component_type() == AppComponentType::Durable);

        check!(debug_props.component_wasm == "comp-a-component-wasm");
        check!(release_props.component_wasm == "comp-a-component-wasm");
        check!(release_custom_props.component_wasm == "release-comp-a-component-wasm");
    }
}
