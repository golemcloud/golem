use crate::config::ProfileName;
use crate::fs;
use crate::log::LogColorize;
use crate::model::component::AppComponentType;
use crate::model::Format;
use anyhow::{anyhow, Context};
use golem_common::model::{ComponentFilePath, ComponentFilePermissions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Debug)]
pub struct ApplicationWithSource {
    pub source: PathBuf,
    pub application: Application,
}

impl ApplicationWithSource {
    pub fn from_yaml_file(file: PathBuf) -> anyhow::Result<Self> {
        Self::from_yaml_string(file.clone(), fs::read_to_string(file.clone())?)
            .with_context(|| anyhow!("Failed to load source {}", file.log_color_highlight()))
    }

    pub fn from_yaml_string(source: PathBuf, string: String) -> serde_yaml::Result<Self> {
        Ok(Self {
            source,
            application: Application::from_yaml_str(string.as_str())?,
        })
    }

    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Application {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wit_deps: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub templates: HashMap<String, ComponentTemplate>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub components: HashMap<String, Component>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dependencies: HashMap<String, Vec<Dependency>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_commands: HashMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api: Option<HttpApi>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<ProfileName, Profile>,
}

impl Application {
    pub fn from_yaml_str(yaml: &str) -> serde_yaml::Result<Self> {
        serde_yaml::from_str(yaml)
    }

    pub fn to_yaml_string(&self) -> String {
        serde_yaml::to_string(self).expect("Failed to serialize Application as YAML")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentTemplate {
    #[serde(flatten)]
    pub component_properties: ComponentProperties,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ComponentProperties>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

impl ComponentTemplate {
    pub fn merge_common_properties_into_profiles(self) -> Self {
        Self {
            component_properties: self.component_properties.clone(),
            profiles: self
                .profiles
                .into_iter()
                .map(|(name, profile)| {
                    (
                        name,
                        self.component_properties
                            .clone()
                            .merge_with_overrides(profile),
                    )
                })
                .collect(),
            default_profile: self.default_profile,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(flatten)]
    pub component_properties: ComponentProperties,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ComponentProperties>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

impl Component {
    pub fn merge_common_properties_into_profiles<'a, I: IntoIterator<Item = &'a String>>(
        mut self,
        profile_names: I,
    ) -> Self {
        Self {
            template: self.template.clone(),
            component_properties: self.component_properties.clone(),
            profiles: {
                profile_names
                    .into_iter()
                    .map(|name| {
                        (
                            name.clone(),
                            match self.profiles.remove(name) {
                                Some(profile) => self
                                    .component_properties
                                    .clone()
                                    .merge_with_overrides(profile),
                                None => self.component_properties.clone(),
                            },
                        )
                    })
                    .collect()
            },
            default_profile: self.default_profile,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApi {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub definitions: HashMap<String, HttpApiDefinition>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub deployments: HashMap<ProfileName, Vec<HttpApiDeployment>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDefinition {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<HttpApiDefinitionRoute>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDefinitionRoute {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    pub binding: HttpApiDefinitionBinding,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HttpApiDefinitionBindingType {
    #[default]
    Default,
    CorsPreflight,
    FileServer,
    HttpHandler,
    SwaggerUi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDefinitionBinding {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<HttpApiDefinitionBindingType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDeployment {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub format: Option<Format>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub build_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auto_confirm: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub redeploy_workers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub redeploy_http_api: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub redeploy_all: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialComponentFile {
    pub source_path: String,
    pub target_path: ComponentFilePath,
    pub permissions: Option<ComponentFilePermissions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentProperties {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_wit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_wit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildCommand>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_commands: HashMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_type: Option<AppComponentType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<InitialComponentFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginInstallation>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
}

impl ComponentProperties {
    pub fn merge_with_overrides(mut self, overrides: ComponentProperties) -> Self {
        if overrides.source_wit.is_some() {
            self.source_wit = overrides.source_wit;
        }

        if overrides.generated_wit.is_some() {
            self.generated_wit = overrides.generated_wit;
        }

        if overrides.component_wasm.is_some() {
            self.component_wasm = overrides.component_wasm;
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

        if !overrides.files.is_empty() {
            self.files.extend(overrides.files);
        }

        if !overrides.plugins.is_empty() {
            self.plugins.extend(overrides.plugins);
        }

        if !overrides.env.is_empty() {
            self.env.extend(overrides.env);
        }

        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, untagged)]
pub enum BuildCommand {
    External(ExternalCommand),
    QuickJSCrate(GenerateQuickJSCrate),
    QuickJSDTS(GenerateQuickJSDTS),
    AgentWrapper(GenerateAgentWrapper),
    ComposeAgentWrapper(ComposeAgentWrapper),
    InjectToPrebuiltQuickJs(InjectToPrebuiltQuickJs),
}

impl BuildCommand {
    pub fn dir(&self) -> Option<&str> {
        match self {
            BuildCommand::External(cmd) => cmd.dir.as_deref(),
            BuildCommand::QuickJSCrate(_) => None,
            BuildCommand::QuickJSDTS(_) => None,
            BuildCommand::AgentWrapper(_) => None,
            BuildCommand::ComposeAgentWrapper(_) => None,
            BuildCommand::InjectToPrebuiltQuickJs(_) => None,
        }
    }

    pub fn targets(&self) -> Vec<String> {
        match self {
            BuildCommand::External(cmd) => cmd.targets.clone(),
            BuildCommand::QuickJSCrate(cmd) => vec![cmd.generate_quickjs_crate.clone()],
            BuildCommand::QuickJSDTS(cmd) => vec![cmd.generate_quickjs_dts.clone()],
            BuildCommand::AgentWrapper(cmd) => vec![cmd.generate_agent_wrapper.clone()],
            BuildCommand::ComposeAgentWrapper(cmd) => vec![cmd.to.clone()],
            BuildCommand::InjectToPrebuiltQuickJs(cmd) => vec![cmd.into.clone()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rmdirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mkdirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateQuickJSCrate {
    pub generate_quickjs_crate: String,
    pub wit: String,
    pub js_modules: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateQuickJSDTS {
    pub generate_quickjs_dts: String,
    pub wit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateAgentWrapper {
    /// The target path of the generated wrapper component
    pub generate_agent_wrapper: String,
    /// The path of the compiled WASM component containing the dynamic golem:agent implementation
    pub based_on_compiled_wasm: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposeAgentWrapper {
    /// The target path of the generated wrapper component
    pub compose_agent_wrapper: String,
    /// The path of the compiled WASM component implementing golem:agent
    pub with_agent: String,
    /// The path of the resulting composed WASM component
    pub to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InjectToPrebuiltQuickJs {
    /// The path to the prebuilt QuickJS WASM file that loads a JS module through a get-script import
    pub inject_to_prebuilt_quickjs: String,
    /// The path to the JS module
    pub module: String,
    /// The path to the intermediate WASM containing the JS module
    pub module_wasm: String,
    /// The path to the output WASM component containing the injected JS module
    pub into: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dependency {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallation {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, String>,
}
