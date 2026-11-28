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

use crate::model::cascade::error::StoreGetValueError;
use crate::model::cascade::layer::Layer;
use crate::model::cascade::store::Store;
use golem_common::schema::config::{
    ConfigCatalogKind, ConfigCatalogSources, ConfigError, ConfigFieldKind, ConfigFieldSchema,
    ConfigOrigin, ConfigSchema, catalog_kind, contains_secret, deref_type, descriptor_type,
    parse_catalog_path, selector, union_branch,
};
use golem_common::schema::render::{from_json_value, to_json_value};
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
use golem_common::schema::{DiscriminatorRule, SchemaType, SchemaValue};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};

/// A named managed binding or an immutable static value, before runtime materialization.
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
    SharedAbsence,
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
    /// Component/template inputs can supply fields for several owners.
    pub project_to_owner: bool,
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
                    // Keep the selector, not initializer bytes, in a managed binding.
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

/// Manifest inheritance is CLI-owned; schema and value validation remain shared.
pub trait ConfigSchemaResolution {
    fn resolve(
        &self,
        catalogs: &ConfigCatalogSources,
        layers: &[ConfigLayer],
        code_origin: &ConfigOrigin,
    ) -> Result<Vec<ConfigFieldPlan>, ConfigError> {
        self.resolve_with(catalogs, layers, code_origin, &|value, _| Ok(value.clone()))
    }

    /// Evaluate manifest leaves, not already canonical code defaults.
    fn resolve_with(
        &self,
        catalogs: &ConfigCatalogSources,
        layers: &[ConfigLayer],
        code_origin: &ConfigOrigin,
        evaluate: &impl Fn(&Value, &ConfigOrigin) -> Result<Value, ConfigError>,
    ) -> Result<Vec<ConfigFieldPlan>, ConfigError>;
}

impl ConfigSchemaResolution for ConfigSchema {
    fn resolve_with(
        &self,
        catalogs: &ConfigCatalogSources,
        layers: &[ConfigLayer],
        code_origin: &ConfigOrigin,
        evaluate: &impl Fn(&Value, &ConfigOrigin) -> Result<Value, ConfigError>,
    ) -> Result<Vec<ConfigFieldPlan>, ConfigError> {
        let schema = self;
        schema.validate(code_origin)?;
        catalogs.validate()?;
        for layer in layers {
            validate_fills(
                &layer.value,
                &[],
                &schema.fields,
                &layer.origin,
                catalogs,
                layer.project_to_owner,
            )?;
        }
        for field in &schema.fields {
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
        let mut plans: Vec<ConfigFieldPlan> = Vec::with_capacity(schema.fields.len());
        let mut binding_graph = schema.graph.clone();
        for def in &mut binding_graph.defs {
            if let Some(kind) = schema.descriptors.get(&def.id) {
                def.body = SchemaType::Enum {
                    cases: vec![format!("{kind:?}")],
                    metadata: Default::default(),
                };
            }
        }
        for field in &schema.fields {
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
                        schema,
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
                        deref_type(&schema.graph, &field.value_type),
                        SchemaType::Option { .. }
                    ) =>
                {
                    ConfigSourcePlan::SharedAbsence
                }
                (ConfigFieldKind::Static, None)
                    if matches!(
                        deref_type(&schema.graph, &field.value_type),
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
                    if contains_secret(&schema.graph, &field.value_type, &mut HashSet::new())
                        || schema.contains_descriptor(&field.value_type, &mut HashSet::new()) =>
                {
                    let mut graph = schema.graph.clone();
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
                    let value = schema.descriptor_json(
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
                    let value = from_json_value(&schema.graph, &field.value_type, &value).map_err(
                        |error| {
                            value_origin.error(format!(
                                "invalid config value at {}: {error}",
                                field.path.join(".")
                            ))
                        },
                    )?;
                    validate_value(&schema.graph, &field.value_type, &value).map_err(|errors| {
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
            for (previous_field, previous_plan) in schema.fields.iter().zip(&plans) {
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

pub(crate) fn validate_fills(
    value: &Value,
    path: &[String],
    fields: &[ConfigFieldSchema],
    origin: &ConfigOrigin,
    catalogs: &ConfigCatalogSources,
    project_to_owner: bool,
) -> Result<(), ConfigError> {
    if fields.iter().any(|field| field.path == path) {
        return Ok(());
    }
    if let Some(selected) = selector(value, "$config", origin)? {
        let (value, source) = catalogs.select(selected, origin)?;
        if catalog_kind(&value) != ConfigCatalogKind::Static {
            return Err(origin.error("a whole-group selection must refer to an ordinary group"));
        }
        return validate_fills(&value, path, fields, &source, catalogs, project_to_owner);
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
            if project_to_owner {
                continue;
            }
            return Err(origin.child(key).error("unknown config field"));
        }
        validate_fills(
            child,
            &child_path,
            fields,
            &origin.child(key),
            catalogs,
            project_to_owner,
        )?;
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
                if let DiscriminatorRule::FieldEquals(rule) = &branch.discriminator
                    && let Some(value) = layer.get(&rule.field_name)
                {
                    let origin = origin.child(&rule.field_name);
                    let (value, source) = match selector(value, "$config", &origin)? {
                        Some(selected) => catalogs.select(selected, &origin)?,
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
    let mut value = layer;
    let mut resolve_member = |ty: &SchemaType, member: &mut Value, segments: &[&str]| {
        let mut child_path = path.to_vec();
        child_path.extend(segments.iter().map(|segment| (*segment).to_string()));
        let child_origin = segments
            .iter()
            .fold(origin.clone(), |origin, segment| origin.child(segment));
        *member = merge_config(
            schema,
            ty,
            None,
            member.take(),
            &child_path,
            &child_origin,
            origins,
            catalogs,
            evaluate,
        )?;
        Ok::<_, ConfigError>(())
    };
    match (ty, &mut value) {
        (SchemaType::Tuple { elements, .. }, Value::Array(array)) => {
            for (index, (ty, member)) in elements.iter().zip(array).enumerate() {
                resolve_member(ty, member, &[&index.to_string()])?;
            }
        }
        (
            SchemaType::Map {
                key,
                value: member_type,
                ..
            },
            Value::Array(entries),
        ) => {
            for (index, entry) in entries.iter_mut().enumerate() {
                if let Some(pair) = entry.as_array_mut() {
                    for (position, (ty, member)) in [key.as_ref(), member_type.as_ref()]
                        .into_iter()
                        .zip(pair)
                        .enumerate()
                    {
                        resolve_member(ty, member, &[&index.to_string(), &position.to_string()])?;
                    }
                }
            }
        }
        (SchemaType::Variant { cases, .. }, Value::Object(object)) => {
            for case in cases {
                if let (Some(ty), Some(member)) = (&case.payload, object.get_mut(&case.name)) {
                    resolve_member(ty, member, &[&case.name])?;
                }
            }
        }
        (SchemaType::Result { spec, .. }, Value::Object(object)) => {
            for (key, ty) in [("ok", &spec.ok), ("err", &spec.err)] {
                if let (Some(ty), Some(member)) = (ty, object.get_mut(key)) {
                    resolve_member(ty, member, &[key])?;
                }
            }
        }
        _ => {
            value = schema.descriptor_json(
                ty,
                evaluate(&value, origin)?,
                true,
                origin,
                Some(catalogs),
            )?;
        }
    }
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
