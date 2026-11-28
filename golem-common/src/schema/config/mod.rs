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

//! Config schema, catalog and value validation shared by services and the CLI.
//!
//! [`ConfigSchema`] describes normalized owner fields. A disjoint path through required
//! records groups independent fields; `kind` supplies the outer binding kind, and
//! `value_type` supplies its payload schema. Aliases and nested Secret descriptors use
//! the ordinary schema graph. A Secret field's payload schema is data-only.
//!
//! Catalogs are keyed by environment in the application manifest:
//!
//! ```yaml
//! config:
//!   prod:
//!     ports: { main: 8123 }
//!     integration: { $kind: shared, $init: null }
//!     credentials: { api: { $kind: secret } }
//! agents:
//!   Checkout:
//!     config:
//!       port: { $config: ports.main }
//!       integration: { $config: integration }
//!       key: { $secret: credentials.api }
//! ```
//!
//! Catalog validation checks exact names and kinds. Initializers stay uninterpreted:
//! no resource creation, private value access, authorization, readiness check or activation
//! occurs here. Manifest inheritance and source plans belong to the CLI.
//!
//! [`ConfigSchema::descriptors`] explicitly marks File, Tool, RetryPolicy and Quota definitions.
//! Their canonical defaults use the existing JSON string codec; resolved plans contain checked
//! authoring data, not resource handles. Retry fields reuse the policy DSL parser. File root
//! containment/content snapshots and Tool compatibility require their runtime integrations.

mod catalog;
mod descriptor;
pub use catalog::{
    ConfigCatalog, ConfigCatalogEntry, ConfigCatalogKind, ConfigDeclaration, OAuthDefinition,
    ResourceInitializer, SecretDeclaration, SecretDefinition, SecretSelector,
};
pub use descriptor::ConfigDescriptorKind;

use crate::base_model::agent::AgentConfigSource;
use crate::base_model::worker::TypedAgentConfigEntry;
use crate::schema::agent::{
    AgentConfigDeclarationSchema, agent_secret_value_schema, typed_schema_value_with_projected_defs,
};
use crate::schema::render::to_json_value;
use crate::schema::validation::{validate_root_type, validate_value};
use crate::schema::{NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypeId};
use golem_schema::schema::render::json_value::matches_discriminator;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

/// A manifest field or the code metadata that supplied a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOrigin {
    pub file: PathBuf,
    pub path: Vec<String>,
    pub layer: String,
    pub package: Option<crate::base_model::component_metadata::ConfigPackageOrigin>,
}

impl ConfigOrigin {
    pub fn child(&self, segment: &str) -> Self {
        let mut origin = self.clone();
        origin.path.push(segment.to_owned());
        origin
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub origin: ConfigOrigin,
    pub message: String,
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: {}",
            self.origin.file.display(),
            self.origin.path.join("."),
            self.message
        )
    }
}

impl std::error::Error for ConfigError {}

impl ConfigOrigin {
    pub fn error(&self, message: impl Into<String>) -> ConfigError {
        ConfigError {
            origin: self.clone(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFieldKind {
    Static,
    Shared,
    Secret,
}

/// A required field declaration. Optionality describes `value_type`, not the binding.
/// Disjoint paths allow required records to group independent managed fields.
#[derive(Clone, Debug)]
pub struct ConfigFieldSchema {
    pub path: Vec<String>,
    pub kind: ConfigFieldKind,
    pub value_type: SchemaType,
    pub default: Option<SchemaValue>,
}

#[derive(Clone, Debug)]
pub struct ConfigSchema {
    pub graph: SchemaGraph,
    /// Explicit special types. Their graph definitions use the canonical JSON string codec.
    pub descriptors: BTreeMap<TypeId, ConfigDescriptorKind>,
    pub fields: Vec<ConfigFieldSchema>,
}

/// Disjoint catalog roots from manifest files. Combining sources does not imply layering.
#[derive(Clone, Debug, Default)]
pub struct ConfigCatalogSources {
    roots: BTreeMap<String, (ConfigCatalogEntry, ConfigOrigin)>,
}

impl ConfigCatalogSources {
    pub fn add(
        &mut self,
        catalog: &ConfigCatalog,
        origin: &ConfigOrigin,
    ) -> Result<(), ConfigError> {
        for name in catalog.entries().keys() {
            if let Some((_, previous)) = self.roots.get(name) {
                return Err(origin.child(name).error(format!(
                    "catalog entry is also declared in {}:{}",
                    previous.file.display(),
                    previous.path.join(".")
                )));
            }
        }
        for (name, value) in catalog.entries() {
            self.roots
                .insert(name.clone(), (value.clone(), origin.child(name)));
        }
        Ok(())
    }

    pub fn select(
        &self,
        path: &str,
        origin: &ConfigOrigin,
    ) -> Result<(Value, ConfigOrigin), ConfigError> {
        let segments = parse_catalog_path(path).map_err(|message| origin.error(message))?;
        let (mut value, mut source) = self
            .roots
            .get(&segments[0])
            .map(|(value, origin)| (value, origin.clone()))
            .ok_or_else(|| origin.error(format!("unknown catalog path '{path}'")))?;
        for segment in &segments[1..] {
            let ConfigCatalogEntry::Group(entries) = value else {
                return Err(origin.error(format!(
                    "catalog path '{path}' traverses a non-group declaration at {}",
                    source.path.join(".")
                )));
            };
            value = entries
                .get(segment)
                .ok_or_else(|| origin.error(format!("unknown catalog path '{path}'")))?;
            source = source.child(segment);
        }
        let value = serde_json::to_value(value).map_err(|error| origin.error(error.to_string()))?;
        Ok((value, source))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        for (name, (value, origin)) in &self.roots {
            let value =
                serde_json::to_value(value).map_err(|error| origin.error(error.to_string()))?;
            self.validate_dependencies(&value, origin, &mut vec![name.clone()])?;
        }
        Ok(())
    }

    pub fn validate_dependencies(
        &self,
        value: &Value,
        origin: &ConfigOrigin,
        active: &mut Vec<String>,
    ) -> Result<(), ConfigError> {
        if let Some(path) = selector(value, "$secret", origin)? {
            let (target, target_origin) = self.select(path, origin)?;
            if catalog_kind(&target) != ConfigCatalogKind::Secret {
                return Err(origin.error(format!("'{path}' is not a Secret declaration")));
            }
            if active.iter().any(|ancestor| ancestor == path) {
                return Err(origin.error(format!("Secret definition cycle through '{path}'")));
            }
            active.push(path.to_owned());
            self.validate_dependencies(&target, &target_origin, active)?;
            active.pop();
        } else if let Some(object) = value.as_object() {
            for (key, child) in object {
                self.validate_dependencies(child, &origin.child(key), active)?;
            }
        } else if let Some(array) = value.as_array() {
            for (index, child) in array.iter().enumerate() {
                self.validate_dependencies(child, &origin.child(&index.to_string()), active)?;
            }
        }
        Ok(())
    }
}

pub fn parse_catalog_path(path: &str) -> Result<Vec<String>, String> {
    let invalid_path = || {
        format!(
            "invalid catalog path '{path}'; use exact root-based segments matching [A-Za-z_-][A-Za-z0-9_-]*"
        )
    };
    let segments = crate::base_model::split_config_path(path).map_err(|_| invalid_path())?;
    if !segments.iter().all(|segment| valid_catalog_name(segment)) {
        return Err(invalid_path());
    }
    Ok(segments)
}

pub fn valid_catalog_name(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_' || c == b'-')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

pub fn catalog_kind(value: &Value) -> ConfigCatalogKind {
    match value.get("$kind").and_then(Value::as_str) {
        Some("shared") => ConfigCatalogKind::Shared,
        Some("secret") => ConfigCatalogKind::Secret,
        Some("resource") => ConfigCatalogKind::Resource,
        _ => ConfigCatalogKind::Static,
    }
}

pub fn selector<'a>(
    value: &'a Value,
    key: &str,
    origin: &ConfigOrigin,
) -> Result<Option<&'a str>, ConfigError> {
    if let Some(object) = value.as_object()
        && let Some(path) = object.get(key)
    {
        if object.len() != 1 {
            return Err(origin.error(format!(
                "'{key}' replaces the whole field; it cannot have sibling fields"
            )));
        }
        let path = path
            .as_str()
            .ok_or_else(|| origin.error(format!("'{key}' requires a catalog path string")))?;
        parse_catalog_path(path).map_err(|message| origin.error(message))?;
        return Ok(Some(path));
    }
    Ok(None)
}

impl ConfigSchema {
    /// Uses the owner's shared definitions and native defaults emitted by its SDK.
    pub fn from_declarations(
        graph: &SchemaGraph,
        declarations: &[AgentConfigDeclarationSchema],
        origin: &ConfigOrigin,
    ) -> Result<Self, ConfigError> {
        let fields = declarations
            .iter()
            .map(|declaration| {
                let field_origin = declaration
                    .path
                    .iter()
                    .fold(origin.clone(), |origin, name| origin.child(name));
                let (kind, value_type) = match declaration.source {
                    AgentConfigSource::Local => {
                        (ConfigFieldKind::Static, declaration.value_type.clone())
                    }
                    AgentConfigSource::Secret => {
                        if declaration.default_value.is_some() {
                            return Err(
                                field_origin.error("Secret fields cannot declare code defaults")
                            );
                        }
                        let payload = agent_secret_value_schema(graph, &declaration.value_type)
                            .ok_or_else(|| {
                                field_origin.error("Secret fields must declare a Secret type")
                            })?;
                        (ConfigFieldKind::Secret, payload.root)
                    }
                };
                let default = declaration.default_value.clone();
                Ok(ConfigFieldSchema {
                    path: declaration.path.clone(),
                    kind,
                    value_type,
                    default,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let schema = Self {
            graph: graph.clone(),
            descriptors: BTreeMap::new(),
            fields,
        };
        schema.validate(origin)?;
        Ok(schema)
    }

    /// Apply code defaults below explicitly supplied values. A child default
    /// does not make an omitted required group present.
    pub fn apply_missing_defaults(&self, entries: &mut Vec<TypedAgentConfigEntry>) {
        let defaults = self
            .fields
            .iter()
            .filter_map(|field| {
                let value = field.default.as_ref()?;
                if entries.iter().any(|entry| entry.path == field.path)
                    || (1..field.path.len()).any(|length| {
                        !entries
                            .iter()
                            .any(|entry| entry.path.starts_with(&field.path[..length]))
                    })
                {
                    return None;
                }
                Some(TypedAgentConfigEntry {
                    path: field.path.clone(),
                    value: typed_schema_value_with_projected_defs(
                        &self.graph,
                        field.value_type.clone(),
                        value.clone(),
                    ),
                })
            })
            .collect::<Vec<_>>();
        entries.extend(defaults);
    }

    pub fn validate(&self, origin: &ConfigOrigin) -> Result<(), ConfigError> {
        for id in self.descriptors.keys() {
            if !self.graph.lookup(id).is_some_and(|def| {
                matches!(
                    deref_type(&self.graph, &def.body),
                    SchemaType::String { .. }
                )
            }) {
                return Err(origin.error(format!(
                    "descriptor type '{id}' must use the canonical JSON string codec"
                )));
            }
        }
        for (index, field) in self.fields.iter().enumerate() {
            if field.path.is_empty()
                || field
                    .path
                    .iter()
                    .any(|name| name.is_empty() || name.starts_with('$') || name.contains('.'))
            {
                return Err(origin
                    .error("config fields must have nonempty names without '.' or a leading '$'"));
            }
            if self.fields[..index].iter().any(|previous| {
                previous.path.starts_with(&field.path) || field.path.starts_with(&previous.path)
            }) {
                return Err(origin.error(format!(
                    "overlapping config declaration '{}'",
                    field.path.join(".")
                )));
            }
            validate_root_type(&self.graph, &field.value_type).map_err(|errors| {
                origin.error(format!(
                    "invalid config schema at {}: {errors:?}",
                    field.path.join(".")
                ))
            })?;
            let field_origin = field
                .path
                .iter()
                .fold(origin.clone(), |origin, name| origin.child(name));
            validate_config_type(
                self,
                &field.value_type,
                field.kind == ConfigFieldKind::Shared,
                field.kind == ConfigFieldKind::Secret,
                false,
                &field_origin,
                &mut HashSet::new(),
            )?;
            if let Some(default) = &field.default {
                if field.kind != ConfigFieldKind::Static
                    || contains_secret(&self.graph, &field.value_type, &mut HashSet::new())
                {
                    return Err(origin.error(format!(
                        "code defaults cannot fill managed bindings or payloads at '{}'",
                        field.path.join(".")
                    )));
                }
                validate_value(&self.graph, &field.value_type, default).map_err(|errors| {
                    origin.error(format!(
                        "invalid static code default at {}: {errors:?}",
                        field.path.join(".")
                    ))
                })?;
                let value = to_json_value(&self.graph, &field.value_type, default)
                    .map_err(|error| origin.error(format!("invalid canonical default: {error}")))?;
                self.descriptor_json(&field.value_type, value, false, origin, None)?;
            }
        }
        Ok(())
    }
}

pub fn descriptor_type(ty: &mut SchemaType) {
    match ty {
        SchemaType::Secret { .. } => {
            *ty = SchemaType::record(vec![NamedFieldType {
                name: "$secret".into(),
                body: SchemaType::string(),
                metadata: Default::default(),
            }])
        }
        SchemaType::Record { fields, .. } => fields
            .iter_mut()
            .for_each(|field| descriptor_type(&mut field.body)),
        SchemaType::Option { inner, .. } => descriptor_type(inner),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            descriptor_type(element)
        }
        SchemaType::Tuple { elements, .. } => elements.iter_mut().for_each(descriptor_type),
        SchemaType::Map { key, value, .. } => {
            descriptor_type(key);
            descriptor_type(value);
        }
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter_mut()
            .for_each(|branch| descriptor_type(&mut branch.body)),
        SchemaType::Variant { cases, .. } => cases
            .iter_mut()
            .filter_map(|case| case.payload.as_mut())
            .for_each(descriptor_type),
        SchemaType::Result { spec, .. } => spec
            .ok
            .iter_mut()
            .chain(&mut spec.err)
            .for_each(|ty| descriptor_type(ty)),
        _ => {}
    }
}

pub fn deref_type<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> &'a SchemaType {
    let mut seen = HashSet::new();
    while let SchemaType::Ref { id, .. } = ty {
        if !seen.insert(id) {
            break;
        }
        match graph.lookup(id) {
            Some(def) => ty = &def.body,
            None => break,
        }
    }
    ty
}

pub fn contains_secret(graph: &SchemaGraph, ty: &SchemaType, seen: &mut HashSet<TypeId>) -> bool {
    if let SchemaType::Ref { id, .. } = ty {
        return seen.insert(id.clone())
            && graph
                .lookup(id)
                .is_some_and(|def| contains_secret(graph, &def.body, seen));
    }
    matches!(ty, SchemaType::Secret { .. })
        || children(ty)
            .iter()
            .any(|child| contains_secret(graph, child, seen))
}

fn children(ty: &SchemaType) -> Vec<&SchemaType> {
    match ty {
        SchemaType::Record { fields, .. } => fields.iter().map(|field| &field.body).collect(),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => vec![element],
        SchemaType::Option { inner, .. } => vec![inner],
        SchemaType::Tuple { elements, .. } => elements.iter().collect(),
        SchemaType::Map { key, value, .. } => vec![key, value],
        SchemaType::Variant { cases, .. } => cases
            .iter()
            .filter_map(|case| case.payload.as_ref())
            .collect(),
        SchemaType::Union { spec, .. } => spec.branches.iter().map(|branch| &branch.body).collect(),
        SchemaType::Result { spec, .. } => {
            spec.ok.iter().chain(&spec.err).map(Box::as_ref).collect()
        }
        SchemaType::Secret { spec, .. } => vec![&spec.inner],
        _ => vec![],
    }
}

fn validate_config_type(
    schema: &ConfigSchema,
    ty: &SchemaType,
    shared: bool,
    private: bool,
    modified: bool,
    origin: &ConfigOrigin,
    seen: &mut HashSet<(TypeId, bool, bool, bool)>,
) -> Result<(), ConfigError> {
    let graph = &schema.graph;
    if let Some(kind) = schema.descriptor_kind(ty) {
        if private && kind != ConfigDescriptorKind::RetryPolicy {
            return Err(origin
                .error("Secret payloads contain data only, not File, Tool or Quota descriptors"));
        }
        return Ok(());
    }
    if let SchemaType::Ref { id, .. } = ty {
        if seen.insert((id.clone(), shared, private, modified))
            && let Some(def) = graph.lookup(id)
        {
            validate_config_type(schema, &def.body, shared, private, modified, origin, seen)?;
        }
        return Ok(());
    }
    if let SchemaType::Record { fields, .. } = ty {
        for field in fields {
            if field.name.starts_with('$') {
                return Err(origin
                    .child(&field.name)
                    .error("config schema field names cannot start with '$'"));
            }
            validate_config_type(
                schema,
                &field.body,
                shared,
                private,
                modified,
                &origin.child(&field.name),
                seen,
            )?;
        }
        return Ok(());
    }
    if matches!(ty, SchemaType::Secret { .. }) {
        if private || (!shared && modified) {
            return Err(origin.error("Secret handles cannot occur in private payloads or below outer optional/list modifiers"));
        }
        for child in children(ty) {
            validate_config_type(schema, child, false, true, false, origin, seen)?;
        }
        return Ok(());
    }
    if matches!(
        ty,
        SchemaType::PermissionCard { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Stream { .. }
            | SchemaType::Future { .. }
    ) {
        return Err(origin.error(
            "config stores data and descriptors, not authority, tokens, streams or futures",
        ));
    }
    let modified = modified
        || matches!(
            ty,
            SchemaType::Option { .. }
                | SchemaType::List { .. }
                | SchemaType::FixedList { .. }
                | SchemaType::Map { .. }
        );
    for child in children(ty) {
        validate_config_type(schema, child, shared, private, modified, origin, seen)?;
    }
    Ok(())
}

pub fn union_branch<'a>(ty: &'a SchemaType, value: &Value) -> Option<&'a SchemaType> {
    if let SchemaType::Union { spec, .. } = ty {
        let matching = spec
            .branches
            .iter()
            .filter(|branch| matches_discriminator(&branch.discriminator, value).unwrap_or(false))
            .collect::<Vec<_>>();
        if matching.len() == 1 {
            return Some(&matching[0].body);
        }
    }
    None
}
