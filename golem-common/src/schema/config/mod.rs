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

//! Pure catalog resolution. Binding plans contain names, never fabricated runtime handles.
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
//! Resolution checks exact names and kinds; `ports.missing` is an error, not a default.
//! An omitted effective Shared optional fill produces [`ConfigSourcePlan::CapturedAbsence`],
//! whereas a named Shared binding remains a binding even if its initializer is `null`.
//! An omitted layer preserves an inherited fill. Direct Secrets and Shared lists require
//! explicit bindings. Defaults fill only missing static settings.
//!
//! Records merge by field; lists and selectors replace their whole value. A union overlay
//! inherits its discriminator when omitted, but changing it discards the previous case.
//! Origins retain the declaring file, field and layer for captures and descriptor members.
//! Initializers stay uninterpreted: no resource creation, private value access, authorization,
//! readiness check or activation occurs here.
//!
//! [`ConfigSchema::descriptors`] explicitly marks File, Tool, RetryPolicy and Quota definitions.
//! Their canonical defaults use the existing JSON string codec; resolved plans contain checked
//! authoring data, not resource handles. Retry fields reuse the policy DSL parser. File root
//! containment/content capture and Tool compatibility require their runtime integrations.

mod catalog;
mod descriptor;
pub use catalog::{ConfigCatalog, ConfigCatalogKind};
pub use descriptor::ConfigDescriptorKind;

use crate::schema::render::{from_json_value, to_json_value};
use crate::schema::validation::{is_equivalent_cross_graph, validate_root_type, validate_value};
use crate::schema::{NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypeId};
use golem_schema::schema::render::json_value::matches_discriminator;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
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
    pub(super) fn error(&self, message: impl Into<String>) -> ConfigError {
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

/// A named managed binding or an immutable static capture, before runtime materialization.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigSourcePlan {
    Static(SchemaValue),
    /// A checked descriptor with explicit Secret selectors, before handle materialization.
    Descriptor(Value),
    Shared {
        catalog_path: Vec<String>,
    },
    Secret {
        catalog_path: Vec<String>,
    },
    CapturedAbsence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigFieldPlan {
    pub path: Vec<String>,
    pub source: ConfigSourcePlan,
    /// Includes each effective static member, including members supplied by code defaults.
    pub origins: BTreeMap<Vec<String>, ConfigOrigin>,
    pub catalog_origin: Option<ConfigOrigin>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigLayer {
    pub origin: ConfigOrigin,
    pub value: Value,
}

/// Disjoint catalog roots from manifest files. Combining sources does not imply layering.
#[derive(Clone, Debug, Default)]
pub struct ConfigCatalogSources {
    roots: BTreeMap<String, (Value, ConfigOrigin)>,
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
    ) -> Result<(&Value, ConfigOrigin), ConfigError> {
        let segments = parse_catalog_path(path).map_err(|message| origin.error(message))?;
        let (mut value, mut source) = self
            .roots
            .get(&segments[0])
            .map(|(value, origin)| (value, origin.clone()))
            .ok_or_else(|| origin.error(format!("unknown catalog path '{path}'")))?;
        for segment in &segments[1..] {
            if catalog_kind(value) != ConfigCatalogKind::Static {
                return Err(origin.error(format!(
                    "catalog path '{path}' traverses a managed declaration at {}",
                    source.path.join(".")
                )));
            }
            value = value
                .as_object()
                .and_then(|object| object.get(segment))
                .ok_or_else(|| origin.error(format!("unknown catalog path '{path}'")))?;
            source = source.child(segment);
        }
        Ok((value, source))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        for (name, (value, origin)) in &self.roots {
            self.validate_dependencies(value, origin, &mut vec![name.clone()])?;
        }
        Ok(())
    }

    fn validate_dependencies(
        &self,
        value: &Value,
        origin: &ConfigOrigin,
        active: &mut Vec<String>,
    ) -> Result<(), ConfigError> {
        if let Some(path) = selector(value, "$secret", origin)? {
            let (target, target_origin) = self.select(path, origin)?;
            if catalog_kind(target) != ConfigCatalogKind::Secret {
                return Err(origin.error(format!("'{path}' is not a Secret declaration")));
            }
            if active.iter().any(|ancestor| ancestor == path) {
                return Err(origin.error(format!("Secret definition cycle through '{path}'")));
            }
            active.push(path.to_owned());
            self.validate_dependencies(target, &target_origin, active)?;
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
    let segments = path.split('.').map(str::to_owned).collect::<Vec<_>>();
    if segments.iter().all(|segment| valid_catalog_name(segment)) {
        Ok(segments)
    } else {
        Err(format!(
            "invalid catalog path '{path}'; use exact root-based segments matching [A-Za-z_-][A-Za-z0-9_-]*"
        ))
    }
}

pub fn valid_catalog_name(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_' || c == b'-')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

pub(super) fn catalog_kind(value: &Value) -> ConfigCatalogKind {
    match value.get("$kind").and_then(Value::as_str) {
        Some("shared") => ConfigCatalogKind::Shared,
        Some("secret") => ConfigCatalogKind::Secret,
        Some("resource") => ConfigCatalogKind::Resource,
        _ => ConfigCatalogKind::Static,
    }
}

fn selector<'a>(
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
                    .any(|name| name.is_empty() || name.starts_with('$'))
            {
                return Err(origin
                    .error("config fields must have nonempty names that do not start with '$'"));
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

    pub fn resolve(
        &self,
        catalogs: &ConfigCatalogSources,
        layers: &[ConfigLayer],
        code_origin: &ConfigOrigin,
    ) -> Result<Vec<ConfigFieldPlan>, ConfigError> {
        self.validate(code_origin)?;
        catalogs.validate()?;
        for layer in layers {
            validate_fills(&layer.value, &[], &self.fields, &layer.origin, catalogs)?;
        }
        for field in &self.fields {
            for length in 1..field.path.len() {
                let group = &field.path[..length];
                let mut present = false;
                for layer in layers {
                    for prefix_length in 0..group.len() {
                        if let Some((value, origin)) = field_fill(
                            &layer.value,
                            &group[..prefix_length],
                            &layer.origin,
                            catalogs,
                            None,
                        )? && selector(&value, "$config", &origin)?.is_some()
                        {
                            present = false;
                            break;
                        }
                    }
                    present |=
                        field_fill(&layer.value, group, &layer.origin, catalogs, None)?.is_some();
                }
                if !present {
                    return Err(code_origin.error(format!(
                        "missing required config group '{}'",
                        group.join(".")
                    )));
                }
            }
        }
        let mut plans: Vec<ConfigFieldPlan> = Vec::with_capacity(self.fields.len());
        let mut binding_graph = self.graph.clone();
        for def in &mut binding_graph.defs {
            if let Some(kind) = self.descriptors.get(&def.id) {
                def.body = SchemaType::Enum {
                    cases: vec![format!("{kind:?}")],
                    metadata: Default::default(),
                };
            }
        }
        for field in &self.fields {
            let mut effective = None;
            let mut origins = BTreeMap::new();
            if let Some(default) = &field.default {
                effective = Some(
                    to_json_value(&self.graph, &field.value_type, default).map_err(|error| {
                        code_origin.error(format!("cannot export static code default: {error}"))
                    })?,
                );
                let origin_value = self.descriptor_json(
                    &field.value_type,
                    effective.as_ref().unwrap().clone(),
                    false,
                    code_origin,
                    Some(catalogs),
                )?;
                record_origins(
                    &origin_value,
                    &field.path,
                    &field
                        .path
                        .iter()
                        .fold(code_origin.clone(), |origin, name| origin.child(name)),
                    &mut origins,
                );
            }
            let mut catalog_origin = None;
            for layer in layers {
                for length in 0..field.path.len() {
                    if let Some((value, origin)) = field_fill(
                        &layer.value,
                        &field.path[..length],
                        &layer.origin,
                        catalogs,
                        None,
                    )? && selector(&value, "$config", &origin)?.is_some()
                    {
                        effective = None;
                        origins.clear();
                        catalog_origin = None;
                        break;
                    }
                }
                if let Some((value, origin)) =
                    field_fill(&layer.value, &field.path, &layer.origin, catalogs, None)?
                {
                    if field.kind == ConfigFieldKind::Secret
                        && selector(&value, "$config", &origin)?.is_some()
                    {
                        return Err(origin.error("Secret fields require '$secret', not '$config'"));
                    }
                    let binding_path = selector(&value, "$config", &origin)?.map(str::to_owned);
                    let (value, selected_origin) = match selector(&value, "$config", &origin)? {
                        Some(path) => {
                            let (selected, source) = catalogs.select(path, &origin)?;
                            catalog_origin = Some(source.clone());
                            (selected.clone(), Some(source))
                        }
                        None => (value, None),
                    };
                    if field.kind == ConfigFieldKind::Static {
                        if catalog_kind(&value) != ConfigCatalogKind::Static {
                            return Err(origin
                                .error("a static field cannot snapshot a managed catalog entry"));
                        }
                        if selected_origin.is_some() {
                            origins.clear();
                        } else {
                            catalog_origin = None;
                        }
                        effective = Some(merge_config(
                            self,
                            &field.value_type,
                            if selected_origin.is_some() {
                                None
                            } else {
                                effective
                            },
                            value,
                            &field.path,
                            selected_origin.as_ref().unwrap_or(&origin),
                            &mut origins,
                            catalogs,
                        )?);
                    } else {
                        origins.clear();
                        origins.insert(field.path.clone(), origin.clone());
                        effective = Some(match field.kind {
                            ConfigFieldKind::Shared => {
                                if selected_origin.is_none()
                                    || catalog_kind(&value) != ConfigCatalogKind::Shared
                                {
                                    return Err(origin.error("Shared fields require a named '$config' binding to a Shared declaration"));
                                }
                                // Keep the selector, not initializer bytes, in an owner capture.
                                Value::String(binding_path.unwrap())
                            }
                            ConfigFieldKind::Secret => {
                                let path =
                                    selector(&value, "$secret", &origin)?.ok_or_else(|| {
                                        origin.error(
                                            "Secret fields require an explicit '$secret' binding",
                                        )
                                    })?;
                                let (entry, source) = catalogs.select(path, &origin)?;
                                if catalog_kind(entry) != ConfigCatalogKind::Secret {
                                    return Err(origin
                                        .error(format!("'{path}' is not a Secret declaration")));
                                }
                                catalog_origin = Some(source);
                                Value::String(path.to_owned())
                            }
                            ConfigFieldKind::Static => unreachable!(),
                        });
                    }
                }
            }
            let value_origin = origins.get(&field.path).unwrap_or(code_origin);
            let source = match (field.kind, effective) {
                (ConfigFieldKind::Shared, None)
                    if matches!(
                        deref_type(&self.graph, &field.value_type),
                        SchemaType::Option { .. }
                    ) =>
                {
                    ConfigSourcePlan::CapturedAbsence
                }
                (ConfigFieldKind::Static, None)
                    if matches!(
                        deref_type(&self.graph, &field.value_type),
                        SchemaType::Option { .. }
                    ) =>
                {
                    ConfigSourcePlan::Static(SchemaValue::Option { inner: None })
                }
                (_, None) => {
                    return Err(code_origin.error(format!(
                        "missing config binding or value for '{}'",
                        field.path.join(".")
                    )));
                }
                (ConfigFieldKind::Static, Some(value))
                    if contains_secret(&self.graph, &field.value_type, &mut HashSet::new())
                        || self.contains_descriptor(&field.value_type, &mut HashSet::new()) =>
                {
                    let mut graph = self.graph.clone();
                    for def in &mut graph.defs {
                        descriptor_type(&mut def.body);
                    }
                    let mut ty = field.value_type.clone();
                    descriptor_type(&mut ty);
                    let decoded = from_json_value(&graph, &ty, &value).map_err(|error| {
                        value_origin.error(format!(
                            "invalid descriptor at {}: {error}",
                            field.path.join(".")
                        ))
                    })?;
                    validate_value(&graph, &ty, &decoded).map_err(|errors| {
                        value_origin.error(format!(
                            "invalid descriptor at {}: {errors:?}",
                            field.path.join(".")
                        ))
                    })?;
                    let value = self.descriptor_json(
                        &field.value_type,
                        value,
                        false,
                        value_origin,
                        Some(catalogs),
                    )?;
                    catalogs.validate_dependencies(&value, value_origin, &mut vec![])?;
                    ConfigSourcePlan::Descriptor(value)
                }
                (ConfigFieldKind::Static, Some(value)) => {
                    let value = from_json_value(&self.graph, &field.value_type, &value).map_err(
                        |error| {
                            value_origin.error(format!(
                                "invalid config value at {}: {error}",
                                field.path.join(".")
                            ))
                        },
                    )?;
                    validate_value(&self.graph, &field.value_type, &value).map_err(|errors| {
                        value_origin.error(format!(
                            "invalid config value at {}: {errors:?}",
                            field.path.join(".")
                        ))
                    })?;
                    ConfigSourcePlan::Static(value)
                }
                (ConfigFieldKind::Shared, Some(Value::String(path))) => ConfigSourcePlan::Shared {
                    catalog_path: parse_catalog_path(&path)
                        .map_err(|error| code_origin.error(error))?,
                },
                (ConfigFieldKind::Secret, Some(Value::String(path))) => ConfigSourcePlan::Secret {
                    catalog_path: parse_catalog_path(&path)
                        .map_err(|error| code_origin.error(error))?,
                },
                _ => unreachable!(),
            };
            for (previous_field, previous_plan) in self.fields.iter().zip(&plans) {
                let same_binding = match (&source, &previous_plan.source) {
                    (
                        ConfigSourcePlan::Shared { catalog_path: left },
                        ConfigSourcePlan::Shared {
                            catalog_path: right,
                        },
                    )
                    | (
                        ConfigSourcePlan::Secret { catalog_path: left },
                        ConfigSourcePlan::Secret {
                            catalog_path: right,
                        },
                    ) => left == right,
                    _ => false,
                };
                if same_binding
                    && !is_equivalent_cross_graph(
                        &binding_graph,
                        &field.value_type,
                        &binding_graph,
                        &previous_field.value_type,
                    )
                {
                    return Err(value_origin.error(format!(
                        "incompatible schemas select the same managed entry at '{}' and '{}'",
                        previous_field.path.join("."),
                        field.path.join(".")
                    )));
                }
            }
            plans.push(ConfigFieldPlan {
                path: field.path.clone(),
                source,
                origins,
                catalog_origin,
            });
        }
        Ok(plans)
    }
}

fn field_fill(
    value: &Value,
    path: &[String],
    origin: &ConfigOrigin,
    catalogs: &ConfigCatalogSources,
    catalog_path: Option<String>,
) -> Result<Option<(Value, ConfigOrigin)>, ConfigError> {
    if path.is_empty() {
        if let Some(catalog_path) = catalog_path
            && selector(value, "$secret", origin)?.is_none()
        {
            let key = if catalog_kind(value) == ConfigCatalogKind::Secret {
                "$secret"
            } else {
                "$config"
            };
            return Ok(Some((
                Value::Object(Map::from_iter([(key.into(), Value::String(catalog_path))])),
                origin.clone(),
            )));
        }
        return Ok(Some((value.clone(), origin.clone())));
    }
    if let Some(selected) = selector(value, "$config", origin)? {
        let (group, source) = catalogs.select(selected, origin)?;
        if catalog_kind(group) != ConfigCatalogKind::Static {
            return Err(origin.error("a whole-group selection must refer to an ordinary group"));
        }
        return field_fill(group, path, &source, catalogs, Some(selected.to_owned()));
    }
    let object = value
        .as_object()
        .ok_or_else(|| origin.error("config group must be an object"))?;
    match object.get(&path[0]) {
        Some(value) => field_fill(
            value,
            &path[1..],
            &origin.child(&path[0]),
            catalogs,
            catalog_path.map(|prefix| format!("{prefix}.{}", path[0])),
        ),
        None => Ok(None),
    }
}

fn validate_fills(
    value: &Value,
    path: &[String],
    fields: &[ConfigFieldSchema],
    origin: &ConfigOrigin,
    catalogs: &ConfigCatalogSources,
) -> Result<(), ConfigError> {
    if fields.iter().any(|field| field.path == path) {
        return Ok(());
    }
    if let Some(selected) = selector(value, "$config", origin)? {
        let (value, source) = catalogs.select(selected, origin)?;
        if catalog_kind(value) != ConfigCatalogKind::Static {
            return Err(origin.error("a whole-group selection must refer to an ordinary group"));
        }
        return validate_fills(value, path, fields, &source, catalogs);
    }
    let object = value
        .as_object()
        .ok_or_else(|| origin.error("config group must be an object"))?;
    for (key, child) in object {
        let mut child_path = path.to_vec();
        child_path.push(key.clone());
        if !fields
            .iter()
            .any(|field| field.path.starts_with(&child_path))
        {
            return Err(origin.child(key).error("unknown config field"));
        }
        validate_fills(child, &child_path, fields, &origin.child(key), catalogs)?;
    }
    Ok(())
}

fn descriptor_type(ty: &mut SchemaType) {
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

fn deref_type<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> &'a SchemaType {
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

fn contains_secret(graph: &SchemaGraph, ty: &SchemaType, seen: &mut HashSet<TypeId>) -> bool {
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

fn record_origins(
    value: &Value,
    path: &[String],
    origin: &ConfigOrigin,
    origins: &mut BTreeMap<Vec<String>, ConfigOrigin>,
) {
    origins.insert(path.to_vec(), origin.clone());
    if let Some(object) = value.as_object() {
        for (name, child) in object {
            let mut path = path.to_vec();
            path.push(name.clone());
            record_origins(child, &path, &origin.child(name), origins);
        }
    } else if let Some(array) = value.as_array() {
        for (index, child) in array.iter().enumerate() {
            let index = index.to_string();
            let mut path = path.to_vec();
            path.push(index.clone());
            record_origins(child, &path, &origin.child(&index), origins);
        }
    }
}

fn union_branch<'a>(ty: &'a SchemaType, value: &Value) -> Option<&'a SchemaType> {
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

fn merge_config(
    schema: &ConfigSchema,
    ty: &SchemaType,
    current: Option<Value>,
    layer: Value,
    path: &[String],
    origin: &ConfigOrigin,
    origins: &mut BTreeMap<Vec<String>, ConfigOrigin>,
    catalogs: &ConfigCatalogSources,
) -> Result<Value, ConfigError> {
    let graph = &schema.graph;
    if let Some(selected) = selector(&layer, "$config", origin)? {
        if matches!(deref_type(graph, ty), SchemaType::Secret { .. }) {
            return Err(origin.error("Secret fields require '$secret', not '$config'"));
        }
        let (value, source) = catalogs.select(selected, origin)?;
        if catalog_kind(value) != ConfigCatalogKind::Static {
            return Err(origin.error("a static field cannot snapshot a managed entry"));
        }
        origins.retain(|key, _| !key.starts_with(path));
        return merge_config(
            schema,
            ty,
            None,
            value.clone(),
            path,
            &source,
            origins,
            catalogs,
        );
    }
    if matches!(deref_type(graph, ty), SchemaType::Secret { .. }) {
        let selected = selector(&layer, "$secret", origin)?
            .ok_or_else(|| origin.error("Secret fields require an explicit '$secret' binding"))?;
        let (entry, _) = catalogs.select(selected, origin)?;
        if catalog_kind(entry) != ConfigCatalogKind::Secret {
            return Err(origin.error(format!("'{selected}' is not a Secret declaration")));
        }
        origins.retain(|key, _| !key.starts_with(path));
        record_origins(&layer, path, origin, origins);
        return Ok(layer);
    }
    if schema.descriptor_kind(ty).is_some() {
        origins.retain(|key, _| !key.starts_with(path));
        record_origins(&layer, path, origin, origins);
        return schema.descriptor_json(ty, layer, true, origin, Some(catalogs));
    }
    let ty = deref_type(graph, ty);
    if let SchemaType::Option { inner, .. } = ty
        && !layer.is_null()
    {
        return merge_config(
            schema,
            inner,
            current.filter(|value| !value.is_null()),
            layer,
            path,
            origin,
            origins,
            catalogs,
        );
    }
    if matches!(ty, SchemaType::Union { .. }) {
        // A partial record overlay inherits the discriminator, not the old case's fields.
        let mut discriminated = current.clone().unwrap_or(Value::Null);
        if let (Some(previous), Some(overlay)) = (discriminated.as_object_mut(), layer.as_object())
        {
            previous.extend(overlay.clone());
        } else {
            discriminated = layer.clone();
        }
        if let Some(branch) = union_branch(ty, &discriminated) {
            let same_case = current
                .as_ref()
                .and_then(|value| union_branch(ty, value))
                .is_some_and(|previous| std::ptr::eq(previous, branch));
            if !same_case {
                origins.retain(|key, _| !key.starts_with(path));
            }
            return merge_config(
                schema,
                branch,
                current.filter(|_| same_case),
                layer,
                path,
                origin,
                origins,
                catalogs,
            );
        }
    } else if let SchemaType::Record { fields, .. } = ty
        && let Some(object) = layer.as_object()
    {
        let mut merged = current
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        for (name, value) in object {
            let field = fields
                .iter()
                .find(|field| &field.name == name)
                .ok_or_else(|| origin.child(name).error("unknown config field"))?;
            let mut child_path = path.to_vec();
            child_path.push(name.clone());
            let value = merge_config(
                schema,
                &field.body,
                merged.remove(name),
                value.clone(),
                &child_path,
                &origin.child(name),
                origins,
                catalogs,
            )?;
            merged.insert(name.clone(), value);
        }
        origins.insert(path.to_vec(), origin.clone());
        return Ok(Value::Object(merged));
    }
    if let SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } = ty
        && let Some(array) = layer.as_array()
    {
        origins.retain(|key, _| !key.starts_with(path));
        origins.insert(path.to_vec(), origin.clone());
        return array
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let index = index.to_string();
                let mut child_path = path.to_vec();
                child_path.push(index.clone());
                merge_config(
                    schema,
                    element,
                    None,
                    value.clone(),
                    &child_path,
                    &origin.child(&index),
                    origins,
                    catalogs,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    origins.retain(|key, _| !key.starts_with(path));
    record_origins(&layer, path, origin, origins);
    let value = schema.descriptor_json(ty, layer, true, origin, Some(catalogs))?;
    if !contains_secret(graph, ty, &mut HashSet::new()) {
        let decoded = from_json_value(graph, ty, &value)
            .map_err(|error| origin.error(format!("invalid supplied config value: {error}")))?;
        validate_value(graph, ty, &decoded)
            .map_err(|errors| origin.error(format!("invalid supplied config value: {errors:?}")))?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
