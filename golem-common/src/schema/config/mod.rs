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
pub use catalog::{
    ConfigCatalog, ConfigCatalogEntry, ConfigCatalogKind, ConfigDeclaration, OAuthDefinition,
    ResourceInitializer, SecretDeclaration, SecretDefinition, SecretSelector,
};
pub use descriptor::ConfigDescriptorKind;

use crate::base_model::agent::AgentConfigSource;
use crate::base_model::worker::TypedAgentConfigEntry;
use crate::cascade::error::StoreGetValueError;
use crate::cascade::layer::Layer;
use crate::cascade::store::Store;
use crate::schema::agent::{
    AgentConfigDeclarationSchema, agent_secret_value_schema, typed_schema_value_with_projected_defs,
};
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

#[derive(Clone, Debug, Default, Serialize)]
struct ConfigFill {
    effective: Option<Value>,
    origins: BTreeMap<Vec<String>, ConfigOrigin>,
    catalog_origin: Option<ConfigOrigin>,
}

struct ConfigCascadeContext<'a> {
    schema: &'a ConfigSchema,
    field: &'a ConfigFieldSchema,
    catalogs: &'a ConfigCatalogSources,
    code_origin: &'a ConfigOrigin,
    evaluate: &'a dyn Fn(&Value, &ConfigOrigin) -> Result<Value, ConfigError>,
}

#[derive(Debug)]
struct ConfigCascadeLayer<'a> {
    id: usize,
    parents: Vec<usize>,
    input: Option<&'a ConfigLayer>,
}

impl<'a> Layer for ConfigCascadeLayer<'a> {
    type Id = usize;
    type Value = ConfigFill;
    type Selector = ();
    type AppliedSelection = ();
    type ApplyContext = ConfigCascadeContext<'a>;
    type ApplyError = ConfigError;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn parent_layers(&self) -> &[Self::Id] {
        &self.parents
    }

    fn apply_onto_parent(
        &self,
        ctx: &Self::ApplyContext,
        _selector: &Self::Selector,
        fill: &mut Self::Value,
    ) -> Result<(), Self::ApplyError> {
        let ConfigCascadeContext {
            schema,
            field,
            catalogs,
            code_origin,
            evaluate,
        } = ctx;
        let Some(layer) = self.input else {
            if let Some(default) = &field.default {
                let value =
                    to_json_value(&schema.graph, &field.value_type, default).map_err(|error| {
                        code_origin.error(format!("cannot export static code default: {error}"))
                    })?;
                let origin_value = schema.descriptor_json(
                    &field.value_type,
                    value.clone(),
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
                        .fold((*code_origin).clone(), |origin, name| origin.child(name)),
                    &mut fill.origins,
                );
                fill.effective = Some(value);
            }
            return Ok(());
        };
        for length in 0..field.path.len() {
            if let Some((value, origin)) = field_fill(
                &layer.value,
                &field.path[..length],
                &layer.origin,
                catalogs,
                None,
            )? && selector(&value, "$config", &origin)?.is_some()
            {
                fill.effective = None;
                fill.origins.clear();
                fill.catalog_origin = None;
                break;
            }
        }
        let Some((value, origin)) =
            field_fill(&layer.value, &field.path, &layer.origin, catalogs, None)?
        else {
            return Ok(());
        };
        if field.kind == ConfigFieldKind::Secret && selector(&value, "$config", &origin)?.is_some()
        {
            return Err(origin.error("Secret fields require '$secret', not '$config'"));
        }
        let binding_path = selector(&value, "$config", &origin)?.map(str::to_owned);
        let (value, selected_origin) = match selector(&value, "$config", &origin)? {
            Some(path) => {
                let (selected, source) = catalogs.select(path, &origin)?;
                fill.catalog_origin = Some(source.clone());
                (selected, Some(source))
            }
            None => (value, None),
        };
        if field.kind == ConfigFieldKind::Static {
            if catalog_kind(&value) != ConfigCatalogKind::Static {
                return Err(origin.error("a static field cannot snapshot a managed catalog entry"));
            }
            let previous = if selected_origin.is_some() {
                fill.origins.clear();
                None
            } else {
                fill.catalog_origin = None;
                fill.effective.take()
            };
            fill.effective = Some(merge_config(
                schema,
                &field.value_type,
                previous,
                value,
                &field.path,
                selected_origin.as_ref().unwrap_or(&origin),
                &mut fill.origins,
                catalogs,
                evaluate,
            )?);
        } else {
            fill.origins.clear();
            fill.origins.insert(field.path.clone(), origin.clone());
            fill.effective = Some(match field.kind {
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
                    let path = selector(&value, "$secret", &origin)?.ok_or_else(|| {
                        origin.error("Secret fields require an explicit '$secret' binding")
                    })?;
                    let (entry, source) = catalogs.select(path, &origin)?;
                    if catalog_kind(&entry) != ConfigCatalogKind::Secret {
                        return Err(origin.error(format!("'{path}' is not a Secret declaration")));
                    }
                    fill.catalog_origin = Some(source);
                    Value::String(path.to_owned())
                }
                ConfigFieldKind::Static => unreachable!(),
            });
        }
        Ok(())
    }
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

    fn validate_dependencies(
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

    /// Capture code defaults below explicitly supplied values. A child default
    /// does not make an omitted required group present.
    pub fn capture_missing_defaults(&self, captures: &mut Vec<TypedAgentConfigEntry>) {
        let defaults = self
            .fields
            .iter()
            .filter_map(|field| {
                let value = field.default.as_ref()?;
                if captures.iter().any(|entry| entry.path == field.path)
                    || (1..field.path.len()).any(|length| {
                        !captures
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
        captures.extend(defaults);
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

    pub fn resolve(
        &self,
        catalogs: &ConfigCatalogSources,
        layers: &[ConfigLayer],
        code_origin: &ConfigOrigin,
    ) -> Result<Vec<ConfigFieldPlan>, ConfigError> {
        self.resolve_with(catalogs, layers, code_origin, &|value, _| Ok(value.clone()))
    }

    /// Evaluate supplied static leaves before validation. Code defaults are already
    /// canonical values and never pass through the manifest evaluator.
    pub fn resolve_with(
        &self,
        catalogs: &ConfigCatalogSources,
        layers: &[ConfigLayer],
        code_origin: &ConfigOrigin,
        evaluate: &impl Fn(&Value, &ConfigOrigin) -> Result<Value, ConfigError>,
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
            let mut store = Store::new();
            for (id, input) in std::iter::once(None)
                .chain(layers.iter().map(Some))
                .enumerate()
            {
                store
                    .add_layer(ConfigCascadeLayer {
                        id,
                        parents: id.checked_sub(1).into_iter().collect(),
                        input,
                    })
                    .expect("config cascade layer indices are unique");
            }
            let ConfigFill {
                effective,
                origins,
                catalog_origin,
            } = store
                .value(
                    &layers.len(),
                    &(),
                    &ConfigCascadeContext {
                        schema: self,
                        field,
                        catalogs,
                        code_origin,
                        evaluate,
                    },
                )
                .map_err(|error| match error {
                    StoreGetValueError::LayerApplyError(_, error) => error,
                    error => code_origin.error(error.to_string()),
                })?;
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
        if catalog_kind(&group) != ConfigCatalogKind::Static {
            return Err(origin.error("a whole-group selection must refer to an ordinary group"));
        }
        return field_fill(&group, path, &source, catalogs, Some(selected.to_owned()));
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
        if catalog_kind(&value) != ConfigCatalogKind::Static {
            return Err(origin.error("a whole-group selection must refer to an ordinary group"));
        }
        return validate_fills(&value, path, fields, &source, catalogs);
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
    evaluate: &impl Fn(&Value, &ConfigOrigin) -> Result<Value, ConfigError>,
) -> Result<Value, ConfigError> {
    let graph = &schema.graph;
    if let Some(selected) = selector(&layer, "$config", origin)? {
        if matches!(deref_type(graph, ty), SchemaType::Secret { .. }) {
            return Err(origin.error("Secret fields require '$secret', not '$config'"));
        }
        let (value, source) = catalogs.select(selected, origin)?;
        if catalog_kind(&value) != ConfigCatalogKind::Static {
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
            evaluate,
        );
    }
    if matches!(deref_type(graph, ty), SchemaType::Secret { .. }) {
        let selected = selector(&layer, "$secret", origin)?
            .ok_or_else(|| origin.error("Secret fields require an explicit '$secret' binding"))?;
        let (entry, _) = catalogs.select(selected, origin)?;
        if catalog_kind(&entry) != ConfigCatalogKind::Secret {
            return Err(origin.error(format!("'{selected}' is not a Secret declaration")));
        }
        origins.retain(|key, _| !key.starts_with(path));
        record_origins(&layer, path, origin, origins);
        return Ok(layer);
    }
    if schema.descriptor_kind(ty).is_some() {
        origins.retain(|key, _| !key.starts_with(path));
        record_origins(&layer, path, origin, origins);
        let layer = evaluate(&layer, origin)?;
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
            evaluate,
        );
    }
    if let SchemaType::Union { spec, .. } = ty {
        let mut evaluated = layer.clone();
        if let Some(object) = evaluated.as_object_mut() {
            for branch in &spec.branches {
                if let crate::schema::DiscriminatorRule::FieldEquals(rule) = &branch.discriminator
                    && let Some(value) = layer.get(&rule.field_name)
                {
                    let origin = origin.child(&rule.field_name);
                    let (value, source) = match selector(value, "$config", &origin)? {
                        Some(selected) => {
                            let (value, source) = catalogs.select(selected, &origin)?;
                            (value, source)
                        }
                        None => (value.clone(), origin),
                    };
                    object.insert(rule.field_name.clone(), evaluate(&value, &source)?);
                }
            }
        } else {
            evaluated = evaluate(&layer, origin)?;
        }
        // A partial record overlay inherits the discriminator, not the old case's fields.
        let mut discriminated = current.clone().unwrap_or(Value::Null);
        if let (Some(previous), Some(overlay)) =
            (discriminated.as_object_mut(), evaluated.as_object())
        {
            previous.extend(overlay.clone());
        } else {
            discriminated = evaluated;
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
                evaluate,
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
                evaluate,
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
                    evaluate,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    origins.retain(|key, _| !key.starts_with(path));
    record_origins(&layer, path, origin, origins);
    let layer = evaluate(&layer, origin)?;
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
