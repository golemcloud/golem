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

use super::{ConfigError, ConfigOrigin, parse_catalog_path, valid_catalog_name};
use crate::base_model::quota::{EnforcementAction, ResourceLimit};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigCatalogKind {
    Static,
    Shared,
    Secret,
    Resource,
}

/// Golem declarations are typed; user data is checked against its consumer's schema later.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigCatalogEntry {
    Group(BTreeMap<String, ConfigCatalogEntry>),
    Data(Value),
    SecretReference(String),
    Declaration(ConfigDeclaration),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "$kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ConfigDeclaration {
    Shared {
        #[serde(
            rename = "$init",
            default,
            deserialize_with = "supplied_value",
            skip_serializing_if = "Option::is_none"
        )]
        initializer: Option<Value>,
    },
    Secret(SecretDeclaration),
    Resource {
        #[serde(
            rename = "$init",
            default,
            deserialize_with = "supplied_value",
            skip_serializing_if = "Option::is_none"
        )]
        initializer: Option<ResourceInitializer>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawSecretDeclaration", into = "RawSecretDeclaration")]
pub struct SecretDeclaration {
    pub source: SecretDefinition,
    pub destinations: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SecretDefinition {
    Value { initializer: Option<Value> },
    OAuth(OAuthDefinition),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuthDefinition {
    pub authorization_server: String,
    pub client_id: String,
    #[serde(
        default,
        deserialize_with = "supplied_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub client_secret: Option<SecretSelector>,
    pub scopes: Vec<String>,
    pub resource: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSelector {
    #[serde(rename = "$secret")]
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceInitializer {
    pub limit: ResourceLimit,
    pub enforcement_action: EnforcementAction,
    pub unit: String,
    pub units: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSecretDeclaration {
    #[serde(
        rename = "$init",
        default,
        deserialize_with = "supplied_value",
        skip_serializing_if = "Option::is_none"
    )]
    initializer: Option<Value>,
    #[serde(
        rename = "$oauth",
        default,
        deserialize_with = "supplied_value",
        skip_serializing_if = "Option::is_none"
    )]
    oauth: Option<OAuthDefinition>,
    #[serde(
        rename = "$destinations",
        default,
        deserialize_with = "supplied_value",
        skip_serializing_if = "Option::is_none"
    )]
    destinations: Option<Vec<String>>,
}

fn supplied_value<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl TryFrom<RawSecretDeclaration> for SecretDeclaration {
    type Error = String;

    fn try_from(raw: RawSecretDeclaration) -> Result<Self, Self::Error> {
        let source = match (raw.initializer, raw.oauth) {
            (Some(_), Some(_)) => {
                return Err("OAuth-backed Secrets use '$oauth', never '$init'".into());
            }
            (initializer, None) => SecretDefinition::Value { initializer },
            (None, Some(oauth)) => SecretDefinition::OAuth(oauth),
        };
        Ok(Self {
            source,
            destinations: raw.destinations,
        })
    }
}

impl From<SecretDeclaration> for RawSecretDeclaration {
    fn from(declaration: SecretDeclaration) -> Self {
        let (initializer, oauth) = match declaration.source {
            SecretDefinition::Value { initializer } => (initializer, None),
            SecretDefinition::OAuth(oauth) => (None, Some(oauth)),
        };
        Self {
            initializer,
            oauth,
            destinations: declaration.destinations,
        }
    }
}

impl Serialize for ConfigCatalogEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Group(entries) => entries.serialize(serializer),
            Self::Data(value) => value.serialize(serializer),
            Self::SecretReference(path) => {
                SecretSelector { path: path.clone() }.serialize(serializer)
            }
            Self::Declaration(declaration) => declaration.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ConfigCatalogEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match &value {
            Value::Object(object) if object.contains_key("$kind") => {
                let declaration: ConfigDeclaration =
                    serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
                if matches!(declaration, ConfigDeclaration::Resource { .. })
                    && serde_json::to_value(&declaration).map_err(serde::de::Error::custom)?
                        != value
                {
                    return Err(serde::de::Error::custom(
                        "resource initializer must match the resource definition grammar exactly",
                    ));
                }
                Ok(Self::Declaration(declaration))
            }
            Value::Object(object) if object.contains_key("$secret") => {
                let selector: SecretSelector =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(Self::SecretReference(selector.path))
            }
            Value::Object(_) => serde_json::from_value(value)
                .map(Self::Group)
                .map_err(serde::de::Error::custom),
            _ => Ok(Self::Data(value)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ConfigCatalog(BTreeMap<String, ConfigCatalogEntry>);

impl<'de> Deserialize<'de> for ConfigCatalog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let catalog = Self(BTreeMap::deserialize(deserializer)?);
        catalog
            .validate(&ConfigOrigin {
                file: "<manifest>".into(),
                path: vec!["config".into()],
                layer: "catalog".into(),
                package: None,
            })
            .map_err(serde::de::Error::custom)?;
        Ok(catalog)
    }
}

impl ConfigCatalog {
    pub fn entries(&self) -> &BTreeMap<String, ConfigCatalogEntry> {
        &self.0
    }

    pub fn validate(&self, origin: &ConfigOrigin) -> Result<(), ConfigError> {
        validate_entries(&self.0, origin)
    }
}

fn validate_entries(
    entries: &BTreeMap<String, ConfigCatalogEntry>,
    origin: &ConfigOrigin,
) -> Result<(), ConfigError> {
    for (name, entry) in entries {
        if !valid_catalog_name(name) {
            return Err(origin.child(name).error("invalid catalog name; use nested declarations and [A-Za-z_-][A-Za-z0-9_-]* segments"));
        }
        let origin = origin.child(name);
        match entry {
            ConfigCatalogEntry::Group(entries) => validate_entries(entries, &origin)?,
            ConfigCatalogEntry::Data(value) => validate_payload(value, true, &origin)?,
            ConfigCatalogEntry::SecretReference(path) => {
                parse_catalog_path(path).map_err(|error| origin.error(error))?;
            }
            ConfigCatalogEntry::Declaration(ConfigDeclaration::Shared { initializer }) => {
                if let Some(value) = initializer {
                    validate_payload(value, true, &origin.child("$init"))?;
                }
            }
            ConfigCatalogEntry::Declaration(ConfigDeclaration::Secret(secret)) => {
                match &secret.source {
                    SecretDefinition::Value { initializer } => {
                        if let Some(value) = initializer {
                            validate_payload(value, false, &origin.child("$init"))?;
                        }
                    }
                    SecretDefinition::OAuth(oauth) => {
                        if oauth.authorization_server.is_empty()
                            || oauth.client_id.is_empty()
                            || oauth.resource.is_empty()
                            || oauth.scopes.iter().any(String::is_empty)
                        {
                            return Err(origin
                                .child("$oauth")
                                .error("OAuth names, resource and scopes must not be empty"));
                        }
                        if let Some(selector) = &oauth.client_secret {
                            parse_catalog_path(&selector.path)
                                .map_err(|error| origin.child("$oauth").error(error))?;
                        }
                    }
                }
            }
            ConfigCatalogEntry::Declaration(ConfigDeclaration::Resource { .. }) => {}
        }
    }
    Ok(())
}

fn validate_payload(
    value: &Value,
    secret_selectors: bool,
    origin: &ConfigOrigin,
) -> Result<(), ConfigError> {
    if let Some(object) = value.as_object() {
        if secret_selectors && let Some(path) = super::selector(value, "$secret", origin)? {
            parse_catalog_path(path).map_err(|error| origin.error(error))?;
            return Ok(());
        }
        for (name, child) in object {
            if name.starts_with('$') {
                return Err(origin.child(name).error(
                    "payloads cannot declare managed roots or reference public catalog entries",
                ));
            }
            validate_payload(child, secret_selectors, &origin.child(name))?;
        }
    } else if let Some(elements) = value.as_array() {
        for (index, child) in elements.iter().enumerate() {
            validate_payload(child, secret_selectors, &origin.child(&index.to_string()))?;
        }
    }
    Ok(())
}
