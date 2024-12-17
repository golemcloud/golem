use crate::fs;
use crate::log::LogColorize;
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
#[serde(rename_all = "camelCase")]
pub struct ComponentTemplate {
    #[serde(flatten)]
    pub component_properties: ComponentProperties,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ComponentProperties>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    pub build: Vec<ExternalCommand>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_commands: HashMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(flatten)]
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

impl ComponentProperties {
    pub fn defined_property_names(&self) -> Vec<&str> {
        let mut vec = Vec::<&str>::new();

        if self.source_wit.is_some() {
            vec.push("sourceWit");
        }

        if self.generated_wit.is_some() {
            vec.push("generatedWit");
        }

        if self.component_wasm.is_some() {
            vec.push("componentWasm");
        }

        if self.linked_wasm.is_some() {
            vec.push("linkedWasm");
        }

        if !self.build.is_empty() {
            vec.push("build");
        }

        if !self.custom_commands.is_empty() {
            vec.push("customCommands");
        }

        self.extensions.keys().for_each(|name| vec.push(name));

        vec
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
pub struct Dependency {
    #[serde(rename = "type")]
    pub type_: String,
    pub target: Option<String>,
}
