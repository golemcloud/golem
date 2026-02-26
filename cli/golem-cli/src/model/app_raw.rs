// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::log::LogColorize;
use crate::model::cascade::property::map::MapMergeMode;
use crate::model::cascade::property::vec::VecMergeMode;
use crate::model::component::AppComponentType;
use crate::model::format::Format;
use crate::model::GuestLanguage;
use crate::{fs, APP_MANIFEST_JSON_SCHEMA};
use anyhow::{anyhow, Context};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentFilePath, ComponentFilePermissions};
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::security_scheme::SecuritySchemeName;
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::LazyLock;
use strum::IntoEnumIterator;
use url::Url;

struct NoopRetriever;

impl jsonschema::Retrieve for NoopRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(format!("External schema retrieval is disabled: {uri}").into())
    }
}

static JSON_SCHEMA_VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str::<serde_json::Value>(APP_MANIFEST_JSON_SCHEMA)
        .expect("Invalid Application manifest JSON schema: cannot parse as JSON");
    jsonschema::options()
        .with_retriever(NoopRetriever)
        .build(&schema)
        .expect("Invalid Application manifest JSON schema: cannot create validator")
});

#[derive(Clone, Debug)]
pub struct ApplicationWithSource {
    pub source: PathBuf,
    pub application: Application,
}

impl ApplicationWithSource {
    pub fn from_yaml_file(file: PathBuf) -> anyhow::Result<Self> {
        Self::from_yaml_string(file.clone(), &fs::read_to_string(file.clone())?)
            .with_context(|| anyhow!("Failed to load source {}", file.log_color_highlight()))
    }

    pub fn from_yaml_string(source: PathBuf, string: &str) -> anyhow::Result<Self> {
        Ok(Self {
            source,
            application: Application::from_yaml_str(string)?,
        })
    }

    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Application {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub component_templates: IndexMap<String, ComponentTemplate>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub components: IndexMap<String, Component>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api: Option<HttpApi>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environments: IndexMap<String, Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<BridgeSdks>,
}

#[derive(Debug)]
struct JsonSchemaValidationError {
    schema: String,
    error: String,
}

#[derive(Debug)]
pub struct DeserializationError {
    serde_yaml_error: serde_yaml::Error,
    json_schema_validation_errors_by_path: BTreeMap<String, Vec<JsonSchemaValidationError>>,
}

impl DeserializationError {
    fn new(
        serde_yaml_error: serde_yaml::Error,
        json_schema_evaluation: Option<jsonschema::Evaluation>,
    ) -> Self {
        match json_schema_evaluation {
            Some(evaluation) => {
                if evaluation.flag().valid {
                    serde_yaml_error.into()
                } else {
                    let mut schema_errors =
                        BTreeMap::<String, Vec<JsonSchemaValidationError>>::new();

                    for error in evaluation.iter_errors() {
                        let path = format!(".{}", error.instance_location);
                        schema_errors
                            .entry(path)
                            .or_default()
                            .push(JsonSchemaValidationError {
                                schema: error.schema_location.to_string(),
                                error: error.error.to_string(),
                            })
                    }

                    Self {
                        serde_yaml_error,
                        json_schema_validation_errors_by_path: schema_errors,
                    }
                }
            }
            None => serde_yaml_error.into(),
        }
    }
}

impl From<serde_yaml::Error> for DeserializationError {
    fn from(value: serde_yaml::Error) -> Self {
        Self {
            serde_yaml_error: value,
            json_schema_validation_errors_by_path: BTreeMap::new(),
        }
    }
}

impl Display for DeserializationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.json_schema_validation_errors_by_path.is_empty() {
            write!(f, "{}", self.serde_yaml_error)
        } else {
            writeln!(f, "Failed to deserialize application manifest:")?;
            writeln!(
                f,
                "  {}",
                "YAML deserialization error:".log_color_help_group()
            )?;
            writeln!(
                f,
                "    {}",
                self.serde_yaml_error
                    .to_string()
                    .log_color_error_highlight()
            )?;
            writeln!(
                f,
                "  {}",
                "Schema validation hint(s):".log_color_help_group()
            )?;
            for (path, errors) in &self.json_schema_validation_errors_by_path {
                writeln!(f, "    path: {}", path.log_color_highlight())?;
                for error in errors {
                    writeln!(f, "      - schema: {}", error.schema)?;
                    writeln!(f, "        error: {}", error.error.log_color_warn())?;
                }
            }
            Ok(())
        }
    }
}

impl Error for DeserializationError {}

impl Application {
    pub fn from_yaml_str(yaml: &str) -> Result<Self, DeserializationError> {
        match serde_yaml::from_str(yaml) {
            Ok(app) => Ok(app),
            Err(err) => Err(DeserializationError::new(
                err,
                serde_yaml::from_str::<serde_json::Value>(yaml)
                    .ok()
                    .map(json_value_without_null_fields)
                    .map(|app| JSON_SCHEMA_VALIDATOR.evaluate(&app)),
            )),
        }
    }

    pub fn to_yaml_string(&self) -> String {
        serde_yaml::to_string(self).expect("Failed to serialize Application as YAML")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentTemplate {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
    #[serde(flatten)]
    pub component_properties: ComponentLayerProperties,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentLayerProperties>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
    #[serde(flatten)]
    pub component_properties: ComponentLayerProperties,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentLayerProperties>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApi {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub deployments: IndexMap<EnvironmentName, Vec<HttpApiDeployment>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDeployment {
    pub domain: Domain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<AgentTypeName, HttpApiDeploymentAgentOptions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub server: Option<Server>,
    #[serde(skip_serializing_if = "LenientTokenList::is_empty", default)]
    pub component_presets: LenientTokenList,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cli: Option<CliOptions>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deployment: Option<DeploymentOptions>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum Server {
    Builtin(BuiltinServer),
    Custom(Box<CustomServer>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum BuiltinServer {
    Local,
    Cloud,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomServer {
    pub url: Url,
    pub worker_url: Option<Url>,
    pub allow_insecure: Option<bool>,
    pub auth: CustomServerAuth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum CustomServerAuth {
    OAuth2 {
        oauth2: Marker,
    },
    #[serde[rename_all = "camelCase"]]
    Static {
        static_token: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliOptions {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub format: Option<Format>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auto_confirm: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub redeploy_agents: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset: Option<Marker>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentOptions {
    pub compatibility_check: Option<bool>,
    pub version_check: Option<bool>,
    pub security_overrides: Option<bool>,
}

impl DeploymentOptions {
    pub fn new_local() -> Self {
        Self {
            compatibility_check: Some(false),
            version_check: Some(false),
            security_overrides: Some(true),
        }
    }

    pub fn new_cloud() -> Self {
        Self {
            compatibility_check: None,
            version_check: None,
            security_overrides: None,
        }
    }

    pub fn with_defaults_from(mut self, other: Self) -> Self {
        if self.compatibility_check.is_none() {
            self.compatibility_check = other.compatibility_check;
        }
        if self.version_check.is_none() {
            self.version_check = other.version_check;
        }
        if self.security_overrides.is_none() {
            self.security_overrides = other.security_overrides;
        }
        self
    }

    pub fn compatibility_check(&self) -> bool {
        self.compatibility_check.unwrap_or(true)
    }

    pub fn version_check(&self) -> bool {
        // TODO: atomic: switch to true, once versioning is implemented
        self.version_check.unwrap_or(false)
    }

    pub fn security_overrides(&self) -> bool {
        self.security_overrides.unwrap_or(false)
    }

    pub fn to_diffable(&self) -> diff::Environment {
        diff::Environment {
            compatibility_check: self.compatibility_check(),
            version_check: self.version_check(),
            security_overrides: self.security_overrides(),
        }
    }
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
pub struct ComponentLayerProperties {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_wit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_wit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildCommand>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_type: Option<AppComponentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_vars_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_vars: Option<IndexMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDeploymentAgentOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<SecuritySchemeName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_session_header_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, untagged)]
pub enum BuildCommand {
    External(ExternalCommand),
    QuickJSCrate(GenerateQuickJSCrate),
    QuickJSDTS(GenerateQuickJSDTS),
    InjectToPrebuiltQuickJs(InjectToPrebuiltQuickJs),
}

impl BuildCommand {
    pub fn dir(&self) -> Option<&str> {
        match self {
            BuildCommand::External(cmd) => cmd.dir.as_deref(),
            BuildCommand::QuickJSCrate(_) => None,
            BuildCommand::QuickJSDTS(_) => None,
            BuildCommand::InjectToPrebuiltQuickJs(_) => None,
        }
    }

    pub fn targets(&self) -> Vec<String> {
        match self {
            BuildCommand::External(cmd) => cmd.targets.clone(),
            BuildCommand::QuickJSCrate(cmd) => vec![cmd.generate_quickjs_crate.clone()],
            BuildCommand::QuickJSDTS(cmd) => vec![cmd.generate_quickjs_dts.clone()],
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
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
pub struct PluginInstallation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<BridgeSdkLanguageTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<BridgeSdkLanguageTargets>,
}

impl BridgeSdks {
    pub fn for_language(&self, language: GuestLanguage) -> Option<&BridgeSdkLanguageTargets> {
        match language {
            GuestLanguage::Rust => self.rust.as_ref(),
            GuestLanguage::TypeScript => self.ts.as_ref(),
        }
    }

    pub fn for_all_languages(
        &self,
    ) -> impl Iterator<Item = (GuestLanguage, Option<&BridgeSdkLanguageTargets>)> {
        GuestLanguage::iter().map(|lang| (lang, self.for_language(lang)))
    }

    pub fn for_all_used_languages(
        &self,
    ) -> impl Iterator<Item = (GuestLanguage, &BridgeSdkLanguageTargets)> {
        self.for_all_languages().filter_map(|(lang, targets)| {
            targets.and_then(|targets| (!targets.agents.is_empty()).then_some((lang, targets)))
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdkLanguageTargets {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub agents: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker;

impl Serialize for Marker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for Marker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match bool::deserialize(deserializer)? {
            true => Ok(Marker),
            false => Err(serde::de::Error::custom(
                "value must be `true`, `false` is not allowed",
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
#[derive(Default)]
pub enum LenientTokenList {
    #[default]
    None,
    String(String),
    List(Vec<String>),
}

impl LenientTokenList {
    pub fn is_empty(&self) -> bool {
        match self {
            LenientTokenList::None => true,
            LenientTokenList::String(s) => Self::parse(s).next().is_none(),
            LenientTokenList::List(l) => l.is_empty(),
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::None => vec![],
            Self::String(s) => Self::parse(&s).collect(),
            Self::List(l) => l,
        }
    }

    pub fn into_set(self) -> BTreeSet<String> {
        match self {
            Self::None => BTreeSet::new(),
            Self::String(s) => Self::parse(&s).collect(),
            Self::List(l) => l.into_iter().collect(),
        }
    }

    fn parse(s: &str) -> impl Iterator<Item = String> + use<'_> {
        s.split([',', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

fn json_value_without_null_fields(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => value,
        serde_json::Value::Array(array) => array
            .into_iter()
            .map(json_value_without_null_fields)
            .collect::<Vec<_>>()
            .into(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .filter_map(|(k, v)| {
                if v != serde_json::Value::Null {
                    Some((k, json_value_without_null_fields(v)))
                } else {
                    None
                }
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
    }
}

#[cfg(test)]
mod test {
    mod app_manifest_json_schema_validation {
        use crate::model::app_raw::{Application, JSON_SCHEMA_VALIDATOR};
        use test_r::test;

        #[test]
        fn schema_is_loadable_and_validates_empty_app() {
            let app = Application {
                app: Some("app-name".to_string()),
                ..Default::default()
            };

            assert!(JSON_SCHEMA_VALIDATOR.is_valid(&serde_json::to_value(&app).unwrap()));
        }
    }
}
