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
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigCatalogKind {
    Static,
    Shared,
    Secret,
    Resource,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ConfigCatalog(Map<String, Value>);

impl<'de> Deserialize<'de> for ConfigCatalog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = Map::<String, Value>::deserialize(deserializer)?;
        let catalog = Self(entries);
        catalog
            .validate(&ConfigOrigin {
                file: "<manifest>".into(),
                path: vec!["config".into()],
                layer: "catalog".into(),
            })
            .map_err(serde::de::Error::custom)?;
        Ok(catalog)
    }
}

impl ConfigCatalog {
    pub fn entries(&self) -> &Map<String, Value> {
        &self.0
    }

    pub fn validate(&self, origin: &ConfigOrigin) -> Result<(), ConfigError> {
        validate_entries(&self.0, origin)
    }
}

fn validate_entries(
    entries: &Map<String, Value>,
    origin: &ConfigOrigin,
) -> Result<(), ConfigError> {
    for (name, value) in entries {
        if !valid_catalog_name(name) {
            return Err(origin.child(name).error("invalid catalog name; use nested declarations and [A-Za-z_-][A-Za-z0-9_-]* segments"));
        }
        let origin = origin.child(name);
        if let Some(object) = value.as_object() {
            if super::selector(value, "$secret", &origin)?.is_some() {
                // Static descriptors may depend on Secret identities, never public values.
            } else if let Some(kind) = object.get("$kind") {
                validate_declaration(object, kind, &origin)?;
            } else {
                validate_entries(object, &origin)?;
            }
        } else {
            validate_payload(value, true, &origin)?;
        }
    }
    Ok(())
}

fn validate_declaration(
    object: &Map<String, Value>,
    kind: &Value,
    origin: &ConfigOrigin,
) -> Result<(), ConfigError> {
    let allowed = match kind.as_str() {
        Some("shared") | Some("resource") => &["$kind", "$init"][..],
        Some("secret") => &["$kind", "$init", "$oauth", "$destinations"][..],
        _ => return Err(origin.error("'$kind' must be shared, secret or resource")),
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(origin.child(key).error("unknown declaration property"));
        }
    }
    if object.contains_key("$oauth") && object.contains_key("$init") {
        return Err(origin.error("OAuth-backed Secrets use '$oauth', never '$init'"));
    }
    if let Some(destinations) = object.get("$destinations") {
        let destinations = destinations
            .as_array()
            .ok_or_else(|| origin.error("'$destinations' must be a list of strings"))?;
        if !destinations.iter().all(Value::is_string) {
            return Err(origin.error("'$destinations' must be a list of strings"));
        }
    }
    if let Some(oauth) = object.get("$oauth") {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct OAuthDefinition {
            authorization_server: String,
            client_id: String,
            #[serde(default)]
            client_secret: Value,
            scopes: Vec<String>,
            resource: String,
        }
        let definition: OAuthDefinition =
            serde_json::from_value(oauth.clone()).map_err(|error| {
                origin
                    .child("$oauth")
                    .error(format!("invalid OAuth definition: {error}"))
            })?;
        if definition.authorization_server.is_empty()
            || definition.client_id.is_empty()
            || definition.resource.is_empty()
            || definition.scopes.iter().any(String::is_empty)
        {
            return Err(origin
                .child("$oauth")
                .error("OAuth names, resource and scopes must not be empty"));
        }
        if oauth.get("clientSecret").is_some()
            && super::selector(&definition.client_secret, "$secret", origin)?.is_none()
        {
            return Err(origin
                .child("$oauth")
                .error("clientSecret must be an explicit '$secret' selector"));
        }
    }
    if let Some(initializer) = object.get("$init") {
        validate_payload(initializer, kind == "shared", &origin.child("$init"))?;
        if kind == "resource" {
            #[derive(Deserialize, Serialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct ResourceInitializer {
                limit: ResourceLimit,
                enforcement_action: EnforcementAction,
                unit: String,
                units: String,
            }
            let definition: ResourceInitializer = serde_json::from_value(initializer.clone())
                .map_err(|error| {
                    origin
                        .child("$init")
                        .error(format!("invalid resource initializer: {error}"))
                })?;
            let canonical = serde_json::to_value(&definition)
                .map_err(|error| origin.error(error.to_string()))?;
            if &canonical != initializer {
                return Err(origin.child("$init").error(
                    "resource initializer must match the resource definition grammar exactly",
                ));
            }
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
