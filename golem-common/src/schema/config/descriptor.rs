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

use super::{
    ConfigCatalogKind, ConfigCatalogSources, ConfigError, ConfigOrigin, ConfigSchema, catalog_kind,
    children, deref_type, parse_catalog_path, union_branch,
};
use crate::model::retry_policy::RetryPolicy;
use crate::model::tool::ToolName;
use crate::model::tool_middleware::{ToolMiddlewareInstallation, ToolMiddlewareName};
use crate::schema::{SchemaType, TypeId};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// Explicit declaration semantics, not a role inferred from a field's name or documentation.
/// The referenced graph definition uses the ordinary `serde_json::Value` string codec.
/// Descriptors contain authoring data, never a live File, Tool or Quota handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigDescriptorKind {
    File,
    Tool,
    RetryPolicy,
    Quota,
}

impl ConfigDescriptorKind {
    fn validate(
        self,
        value: Value,
        origin: &ConfigOrigin,
        catalogs: Option<&ConfigCatalogSources>,
    ) -> Result<Value, ConfigError> {
        match self {
            Self::File => {
                let path = value
                    .as_str()
                    .filter(|path| !path.is_empty() && !Path::new(path).is_absolute())
                    .ok_or_else(|| {
                        origin.error("File descriptors require a relative source path")
                    })?;
                if path.contains('\0') {
                    return Err(origin.error("File source paths cannot contain NUL"));
                }
                // Root containment, symlinks and immutable content capture require the file supplier.
            }
            Self::Tool => {
                if let Some(name) = value.as_str() {
                    ToolName::try_from(name).map_err(|error| origin.error(error))?;
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ToolDescriptor {
                        target: ToolName,
                        #[serde(default)]
                        middleware: Vec<Value>,
                    }
                    let descriptor: ToolDescriptor = serde_json::from_value(value.clone())
                        .map_err(|error| {
                            origin.error(format!("invalid Tool descriptor: {error}"))
                        })?;
                    let _ = descriptor.target;
                    for (index, middleware) in descriptor.middleware.into_iter().enumerate() {
                        let source = origin.child("middleware").child(&index.to_string());
                        if let Some(name) = middleware.as_str() {
                            let name = name.rsplit_once('@').map_or(name, |(name, _)| name);
                            ToolMiddlewareName::try_from(name)
                                .map_err(|error| source.error(error))?;
                        } else {
                            let mut object = middleware.as_object().cloned().ok_or_else(|| {
                                source.error("middleware must be a name or an installation object")
                            })?;
                            for key in object.keys() {
                                if ![
                                    "name",
                                    "version",
                                    "parameters",
                                    "account",
                                    "secretKeysReadable",
                                    "secretKeysRevealable",
                                    "filesystemAccess",
                                ]
                                .contains(&key.as_str())
                                {
                                    return Err(source
                                        .child(key)
                                        .error("unknown middleware property"));
                                }
                            }
                            object
                                .entry("parameters")
                                .or_insert_with(|| serde_json::json!({}));
                            serde_json::from_value::<ToolMiddlewareInstallation>(Value::Object(
                                object,
                            ))
                            .map_err(|error| {
                                source.error(format!("invalid middleware: {error}"))
                            })?;
                        }
                    }
                }
            }
            Self::RetryPolicy => {
                let policy: RetryPolicy = serde_json::from_value(value)
                    .map_err(|error| origin.error(format!("invalid RetryPolicy: {error}")))?;
                return serde_json::to_value(policy)
                    .map_err(|error| origin.error(error.to_string()));
            }
            Self::Quota => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct QuotaDescriptor {
                    resource: String,
                    expected_use: Option<u64>,
                }
                let resource = if let Some(name) = value.as_str() {
                    name.to_owned()
                } else {
                    let descriptor: QuotaDescriptor = serde_json::from_value(value.clone())
                        .map_err(|error| {
                            origin.error(format!("invalid Quota descriptor: {error}"))
                        })?;
                    if value.get("expectedUse").is_some_and(Value::is_null) {
                        return Err(origin
                            .child("expectedUse")
                            .error("expectedUse must be an unsigned integer"));
                    }
                    let _ = descriptor.expected_use;
                    descriptor.resource
                };
                parse_catalog_path(&resource).map_err(|error| origin.error(error))?;
                if let Some(catalogs) = catalogs {
                    let (entry, _) = catalogs.select(&resource, origin)?;
                    if catalog_kind(entry) != ConfigCatalogKind::Resource {
                        return Err(origin
                            .error(format!("'{resource}' is not a quota-resource declaration")));
                    }
                }
            }
        }
        Ok(value)
    }
}

impl ConfigSchema {
    pub(super) fn descriptor_kind<'a>(
        &'a self,
        mut ty: &'a SchemaType,
    ) -> Option<ConfigDescriptorKind> {
        let mut seen = HashSet::new();
        while let SchemaType::Ref { id, .. } = ty {
            if let Some(kind) = self.descriptors.get(id) {
                return Some(*kind);
            }
            if !seen.insert(id) {
                break;
            }
            ty = &self.graph.lookup(id)?.body;
        }
        None
    }

    pub(super) fn contains_descriptor(&self, ty: &SchemaType, seen: &mut HashSet<TypeId>) -> bool {
        if self.descriptor_kind(ty).is_some() {
            return true;
        }
        if let SchemaType::Ref { id, .. } = ty {
            return seen.insert(id.clone())
                && self
                    .graph
                    .lookup(id)
                    .is_some_and(|def| self.contains_descriptor(&def.body, seen));
        }
        children(ty)
            .iter()
            .any(|child| self.contains_descriptor(child, seen))
    }

    /// Project manifest descriptors into the core JSON string codec, or decode them for a plan.
    /// Ordinary data and explicit Secret selectors retain their ordinary JSON representation.
    pub(super) fn descriptor_json(
        &self,
        ty: &SchemaType,
        mut value: Value,
        encode: bool,
        origin: &ConfigOrigin,
        catalogs: Option<&ConfigCatalogSources>,
    ) -> Result<Value, ConfigError> {
        if let Some(kind) = self.descriptor_kind(ty) {
            let value = if encode {
                value
            } else {
                serde_json::from_str(value.as_str().ok_or_else(|| {
                    origin.error("descriptor requires the canonical JSON string codec")
                })?)
                .map_err(|error| origin.error(format!("invalid canonical descriptor: {error}")))?
            };
            let value = kind.validate(value, origin, catalogs)?;
            return if encode {
                serde_json::to_string(&value)
                    .map(Value::String)
                    .map_err(|error| origin.error(error.to_string()))
            } else {
                Ok(value)
            };
        }
        let ty = deref_type(&self.graph, ty);
        match ty {
            SchemaType::Option { inner, .. } if !value.is_null() => {
                return self.descriptor_json(inner, value, encode, origin, catalogs);
            }
            SchemaType::Record { fields, .. } => {
                if let Some(object) = value.as_object_mut() {
                    for field in fields {
                        if let Some(member) = object.get_mut(&field.name) {
                            *member = self.descriptor_json(
                                &field.body,
                                member.take(),
                                encode,
                                &origin.child(&field.name),
                                catalogs,
                            )?;
                        }
                    }
                }
            }
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                if let Some(array) = value.as_array_mut() {
                    for (index, member) in array.iter_mut().enumerate() {
                        *member = self.descriptor_json(
                            element,
                            member.take(),
                            encode,
                            &origin.child(&index.to_string()),
                            catalogs,
                        )?;
                    }
                }
            }
            SchemaType::Tuple { elements, .. } => {
                if let Some(array) = value.as_array_mut() {
                    for (index, (ty, member)) in elements.iter().zip(array).enumerate() {
                        *member = self.descriptor_json(
                            ty,
                            member.take(),
                            encode,
                            &origin.child(&index.to_string()),
                            catalogs,
                        )?;
                    }
                }
            }
            SchemaType::Union { .. } => {
                if let Some(branch) = union_branch(ty, &value) {
                    return self.descriptor_json(branch, value, encode, origin, catalogs);
                }
            }
            SchemaType::Map {
                key,
                value: member_type,
                ..
            } => {
                if let Some(entries) = value.as_array_mut() {
                    for (index, entry) in entries.iter_mut().enumerate() {
                        if let Some(pair) = entry.as_array_mut() {
                            for (position, (ty, member)) in [key.as_ref(), member_type.as_ref()]
                                .into_iter()
                                .zip(pair)
                                .enumerate()
                            {
                                *member = self.descriptor_json(
                                    ty,
                                    member.take(),
                                    encode,
                                    &origin
                                        .child(&index.to_string())
                                        .child(&position.to_string()),
                                    catalogs,
                                )?;
                            }
                        }
                    }
                }
            }
            SchemaType::Variant { cases, .. } => {
                if let Some(object) = value.as_object_mut() {
                    for case in cases {
                        if let (Some(ty), Some(member)) =
                            (&case.payload, object.get_mut(&case.name))
                        {
                            *member = self.descriptor_json(
                                ty,
                                member.take(),
                                encode,
                                &origin.child(&case.name),
                                catalogs,
                            )?;
                        }
                    }
                }
            }
            SchemaType::Result { spec, .. } => {
                if let Some(object) = value.as_object_mut() {
                    for (key, ty) in [("ok", &spec.ok), ("err", &spec.err)] {
                        if let (Some(ty), Some(member)) = (ty, object.get_mut(key)) {
                            *member = self.descriptor_json(
                                ty,
                                member.take(),
                                encode,
                                &origin.child(key),
                                catalogs,
                            )?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(value)
    }
}
