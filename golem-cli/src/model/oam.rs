use crate::model::validation::{ValidatedResult, ValidationBuilder};
use anyhow::Context;
use itertools::Itertools;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::GolemError;

pub const API_VERSION_V1BETA1: &str = "core.oam.dev/v1beta1";
pub const KIND_APPLICATION: &str = "Application";

#[derive(Clone, Debug)]
pub struct ApplicationWithSource {
    pub source: PathBuf,
    pub application: Application,
}

impl ApplicationWithSource {
    pub fn from_yaml_file(file: PathBuf) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(file.as_path())
            .with_context(|| format!("Failed to load file: {}", file.to_string_lossy()))?;

        Ok(Self::from_yaml_string(file, content)?)
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

    // NOTE: unlike the wasm_rpc model, here validation is optional separate step, so we can access the "raw" data
    pub fn validate(self) -> ValidatedResult<Self> {
        let mut validation = ValidationBuilder::new();
        validation.push_context("source", self.source_as_string());

        if self.application.api_version != API_VERSION_V1BETA1 {
            validation.add_warn(format!("Expected apiVersion: {}", API_VERSION_V1BETA1))
        }

        if self.application.kind != KIND_APPLICATION {
            validation.add_error(format!("Expected kind: {}", KIND_APPLICATION))
        }

        self.application
            .spec
            .components
            .iter()
            .map(|component| &component.name)
            .counts()
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .for_each(|(component_name, count)| {
                validation.add_warn(format!(
                    "Component specified multiple times component: {}, count: {}",
                    component_name, count
                ));
            });

        validation.pop_context();
        validation.build(self)
    }
}

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

    pub fn from_yaml_file<P: AsRef<Path>>(yaml_path: P) -> Result<Self, GolemError> {
        let file = std::fs::File::open(yaml_path)
            .map_err(GolemError::from_error)?;
        serde_yaml::from_reader(file)
            .map_err(GolemError::from_error)
    }

    pub fn to_yaml_file<P: AsRef<Path>>(&self, yaml_path: P) -> Result<(), GolemError> {
        let file = std::fs::File::create(yaml_path)
            .map_err(GolemError::from_error)?;
        serde_yaml::to_writer(file, self)
            .map_err(GolemError::from_error)
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
    pub components: Vec<Component>,
}

impl Spec {
    pub fn extract_components_by_type(
        &mut self,
        component_types: &BTreeSet<&'static str>,
    ) -> BTreeMap<&'static str, Vec<Component>> {
        let mut components = Vec::<Component>::new();

        std::mem::swap(&mut components, &mut self.components);

        let mut matching_components = BTreeMap::<&'static str, Vec<Component>>::new();
        let mut remaining_components = Vec::<Component>::new();

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
pub struct Component {
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

impl Component {
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
