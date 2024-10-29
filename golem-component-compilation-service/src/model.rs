// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Display;
use std::path::Path;
use reqwest::Url;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::DeserializeOwned;
use golem_common::model::ComponentId;
use tokio::sync::mpsc;
use wasmtime::component::Component;
use golem_worker_executor_base::services::ifs::InitialFileSystem;

#[derive(Debug, Clone)]
pub struct ComponentWithVersion {
    pub id: ComponentId,
    pub version: u64,
}

impl Display for ComponentWithVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.id, self.version)
    }
}

#[derive(Debug)]
pub struct CompilationRequest {
    pub component: ComponentWithVersion,
}

pub struct CompiledComponent {
    pub component_and_version: ComponentWithVersion,
    pub component: Component,
}


#[derive(Debug, Clone, thiserror::Error)]
pub enum CompilationError {
    #[error("Component not found: {0}")]
    ComponentNotFound(ComponentWithVersion),
    #[error("Failed to compile component: {0}")]
    CompileFailure(String),
    #[error("Failed to download component: {0}")]
    ComponentDownloadFailed(String),
    #[error("Failed to upload component: {0}")]
    ComponentUploadFailed(String),
    #[error("Unexpected error: {0}")]
    Unexpected(String),
}

pub struct InitialFileSystemToUpload{
    pub component_and_version: ComponentWithVersion,
    pub initial_file_system: Vec<u8>
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum InitialFileSystemError{
    #[error("Unexpected error: {0}")]
    Unexpected(String)
}

impl<T> From<mpsc::error::SendError<T>> for CompilationError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        CompilationError::Unexpected("Failed to send compilation request".to_string())
    }
}

pub const API_VERSION_V1BETA1: &str = "core.oam.dev/v1beta1";
pub const KIND_APPLICATION: &str = "Application";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Application {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

impl Application {
    pub fn new(name: String) -> Self {
        Self {
            api_version: API_VERSION_V1BETA1.to_string(),
            kind: KIND_APPLICATION.to_string(),
            metadata: Metadata {
                name,
                annotations: Default::default(),
                labels: Default::default(),
            },
            spec: Spec { components: vec![] },
        }
    }

    pub fn from_yaml_str(yaml: &str) -> serde_yaml::Result<Self> {
        serde_yaml::from_str(yaml)
    }

    pub fn to_yaml_string(&self) -> String {
        serde_yaml::to_string(self).expect("Failed to serialize Application as YAML")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spec {
    pub components: Vec<ApplicationComponent>,
}

impl Spec {
    pub fn extract_components_by_type(
        &mut self,
        component_types: &BTreeSet<&'static str>,
    ) -> BTreeMap<&'static str, Vec<ApplicationComponent>> {
        let mut components = Vec::<ApplicationComponent>::new();

        std::mem::swap(&mut components, &mut self.components);

        let mut matching_components = BTreeMap::<&'static str, Vec<ApplicationComponent>>::new();
        let mut remaining_components = Vec::<ApplicationComponent>::new();

        for component in components {
            if let Some(component_type) = component_types.get(component.component_type.as_str()) {
                matching_components
                    .entry(component_type)
                    .or_default()
                    .push(component)
            } else {
                remaining_components.push(component)
            }
        }

        std::mem::swap(&mut remaining_components, &mut self.components);

        matching_components
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationComponent {
    pub name: String,
    #[serde(rename = "type")]
    pub component_type: String,
    pub properties: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<Trait>,
}

pub trait TypedComponentProperties: Serialize + DeserializeOwned {
    fn component_type() -> &'static str;
}

impl ApplicationComponent {
    pub fn typed_properties<T: TypedComponentProperties>(&self) -> Result<T, serde_json::Error> {
        if self.component_type != T::component_type() {
            panic!(
                "Component type mismatch in clone_properties_as, self: {}, requested: {}",
                self.component_type,
                T::component_type()
            );
        }
        serde_json::from_value(self.properties.clone())
    }

    pub fn set_typed_properties<T: TypedComponentProperties>(&mut self, properties: T) {
        self.component_type = T::component_type().to_string();
        self.properties = serde_json::to_value(properties).expect("Failed to serialize properties");
    }

    pub fn extract_traits_by_type(
        &mut self,
        trait_types: &BTreeSet<&'static str>,
    ) -> BTreeMap<&'static str, Vec<Trait>> {
        let mut component_traits = Vec::<Trait>::new();

        std::mem::swap(&mut component_traits, &mut self.traits);

        let mut matching_traits = BTreeMap::<&'static str, Vec<Trait>>::new();
        let mut remaining_traits = Vec::<Trait>::new();

        for component_trait in component_traits {
            if let Some(trait_type) = trait_types.get(component_trait.trait_type.as_str()) {
                matching_traits
                    .entry(trait_type)
                    .or_default()
                    .push(component_trait);
            } else {
                remaining_traits.push(component_trait);
            }
        }

        std::mem::swap(&mut remaining_traits, &mut self.traits);

        matching_traits
    }

    pub fn add_typed_trait<T: TypedTraitProperties>(&mut self, properties: T) {
        self.traits.push(Trait {
            trait_type: T::trait_type().to_string(),
            properties: serde_json::to_value(properties).expect("Failed to serialize typed trait"),
        });
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trait {
    #[serde(rename = "type")]
    pub trait_type: String,
    pub properties: serde_json::Value,
}

pub trait TypedTraitProperties: Serialize + DeserializeOwned {
    fn trait_type() -> &'static str;

    fn from_generic_trait(value: Trait) -> Result<Self, serde_json::Error> {
        if value.trait_type != Self::trait_type() {
            panic!(
                "Trait type mismatch in TryFrom<Trait>, value: {}, typed: {}",
                value.trait_type,
                Self::trait_type()
            )
        }
        serde_json::from_value(value.properties)
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Files{
    pub files: Vec<FileProperty>
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permissions{
    #[serde(rename = "read-only")]
    ReadOnly,

    #[serde(rename = "read-write")]
    ReadWrite,

}


#[derive(Debug,Clone, Serialize, Deserialize)]
pub struct FileProperty{
    #[serde(rename = "sourcePath", deserialize_with = "deserialize_source_path")]
    pub source_path: FileSource,
    #[serde(rename = "targetPath")]
    pub target_path: String,
    pub permissions: Permissions
}


#[derive(Debug, Clone)]
pub enum FileSource {
    Path(String),
    Url(String),
}

impl Serialize for FileSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            FileSource::Path(path) => serializer.serialize_str(path),
            FileSource::Url(url) => serializer.serialize_str(url),
        }
    }
}
fn deserialize_source_path<'de, D>(deserializer: D) -> Result<FileSource, D::Error>
where
    D: Deserializer<'de>
{
    let source_str = String::deserialize(deserializer)?;
    if Url::parse(&source_str).is_ok() {
        Ok(FileSource::Url(source_str))
    }
    else {

        Ok(FileSource::Path(source_str))

    }

}

impl Display for FileSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileSource::Path(source_path) => write!(f, "Local Path: {}", source_path),
            FileSource::Url(source_url) => write!(f, "URL: {}", source_url),
        }
    }
}