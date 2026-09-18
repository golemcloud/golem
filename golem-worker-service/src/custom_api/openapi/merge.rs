// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use super::budget::{Budget, DOCUMENT_BYTE_LIMIT, GENERATION_TIMEOUT};
use super::provider_document::{Category, DocumentError, METHODS, ProviderDocument, pointer};
use golem_common::model::component::ComponentId;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type SecurityRequirements = Vec<BTreeMap<String, Vec<String>>>;

pub(super) struct ProviderContribution {
    pub router: String,
    pub mount: String,
    pub router_type: String,
    pub component_id: ComponentId,
    pub document: ProviderDocument,
    pub mount_security: SecurityRequirements,
}

/// Generated values are host-owned; provider values have already passed strict
/// validation against their own document, before any names can be merged.
pub(super) fn merge(
    generated: Value,
    providers: Vec<ProviderContribution>,
    public_origin: &str,
) -> Result<Value, DocumentError> {
    merge_bounded(
        generated,
        providers,
        public_origin,
        &Budget::new(tokio::time::Instant::now() + GENERATION_TIMEOUT),
    )
}

pub(super) fn merge_bounded(
    mut generated: Value,
    mut providers: Vec<ProviderContribution>,
    public_origin: &str,
    budget: &Budget,
) -> Result<Value, DocumentError> {
    budget.check()?;
    providers.sort_by(|a, b| {
        (&a.mount, &a.router_type, a.component_id.to_string()).cmp(&(
            &b.mount,
            &b.router_type,
            b.component_id.to_string(),
        ))
    });
    let mut paths = Map::new();
    let mut shapes = BTreeMap::new();
    let mut operation_ids = BTreeSet::new();
    let mut links = Vec::new();
    let mut operation_bytes = 0;
    let generated_paths = generated
        .as_object_mut()
        .unwrap()
        .remove("paths")
        .unwrap_or(json!({}));
    merge_paths(
        "generated",
        &mut paths,
        &mut shapes,
        &mut operation_ids,
        generated_paths,
        budget,
        &mut operation_bytes,
    )?;
    generated.as_object_mut().unwrap().remove("security");
    generated["servers"] = json!([{"url":public_origin}]);

    for mut provider in providers {
        budget.check()?;
        let router = &provider.router;
        let document = &mut provider.document;
        let root_security = requirements(document.value.get("security"));
        validate_security(router, &document.value, &root_security, "/security")?;
        for (path, item) in document.value["paths"].as_object().unwrap() {
            if path.starts_with("x-") {
                continue;
            }
            for method in METHODS {
                if let Some(operation) = item.get(method) {
                    let effective = operation
                        .get("security")
                        .map(|value| requirements(Some(value)))
                        .unwrap_or_else(|| root_security.clone());
                    validate_security(
                        router,
                        &document.value,
                        &effective,
                        &pointer(&pointer(&pointer("/paths", path), method), "security"),
                    )?;
                }
            }
        }
        for location in &document.link_operation_ids {
            links.push((
                router.clone(),
                document
                    .value
                    .pointer(location)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
                rebase_pointer(location, &provider.mount),
            ));
        }
        // Rewrite while locations still refer to the original path keys.
        for reference in &document.references {
            budget.check()?;
            let target = rebase_pointer(&reference.target, &provider.mount);
            *document.value.pointer_mut(&reference.location).unwrap() =
                Value::String(fragment(&target));
        }
        let root = document.value.as_object_mut().unwrap();
        let original_paths = root.remove("paths").unwrap();
        let mut rebased = Map::new();
        let Value::Object(original_paths) = original_paths else {
            unreachable!()
        };
        for (path, mut item) in original_paths {
            budget.check()?;
            if path.starts_with("x-") {
                rebased.insert(path, item);
                continue;
            }
            for method in METHODS {
                if let Some(operation) = item.get_mut(method) {
                    let security = operation
                        .get("security")
                        .map(|value| requirements(Some(value)))
                        .unwrap_or_else(|| root_security.clone());
                    operation["security"] =
                        json!(and_security(&provider.mount_security, &security, budget)?);
                    // Each operation survives merging exactly once. Count before retaining
                    // inherited security on further operations, which can multiply input size.
                    add_operation_size(&mut operation_bytes, budget.size(operation)?)?;
                }
            }
            rebased.insert(rebase_path(&path, &provider.mount), item);
        }
        merge_paths(
            router,
            &mut paths,
            &mut shapes,
            &mut operation_ids,
            Value::Object(rebased),
            budget,
            &mut 0,
        )?;
        if let Some(components) = root.remove("components") {
            let target = generated
                .as_object_mut()
                .unwrap()
                .entry("components")
                .or_insert(json!({}))
                .as_object_mut()
                .unwrap();
            for (kind, values) in components.as_object().unwrap() {
                let location = pointer("/components", kind);
                if kind.starts_with("x-") {
                    equal_or_insert(
                        router,
                        target,
                        kind,
                        values.clone(),
                        Category::ComponentConflict,
                        &location,
                    )?;
                } else {
                    let target = target
                        .entry(kind)
                        .or_insert(json!({}))
                        .as_object_mut()
                        .unwrap();
                    for (name, value) in values.as_object().unwrap() {
                        equal_or_insert(
                            router,
                            target,
                            name,
                            value.clone(),
                            Category::ComponentConflict,
                            &pointer(&location, name),
                        )?;
                    }
                }
            }
        }
        if let Some(tags) = root.remove("tags") {
            let target = generated
                .as_object_mut()
                .unwrap()
                .entry("tags")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap();
            for (index, tag) in tags.as_array().unwrap().iter().enumerate() {
                if let Some(previous) = target.iter().find(|t| t["name"] == tag["name"]) {
                    if previous != tag {
                        return Err(DocumentError::new(
                            router,
                            Category::TagConflict,
                            &pointer("/tags", &index.to_string()),
                        ));
                    }
                } else {
                    target.push(tag.clone());
                }
            }
        }
        for (key, value) in root {
            if key == "externalDocs" || key.starts_with("x-") {
                equal_or_insert(
                    router,
                    generated.as_object_mut().unwrap(),
                    key,
                    value.clone(),
                    Category::ExtensionConflict,
                    &pointer("", key),
                )?;
            }
        }
    }
    for (router, id, location) in links {
        if !operation_ids.contains(&id) {
            return Err(DocumentError::new(&router, Category::Reference, &location));
        }
    }
    generated["paths"] = Value::Object(paths);
    budget.size(&generated)?;
    Ok(generated)
}

fn add_operation_size(total: &mut usize, size: usize) -> Result<(), DocumentError> {
    if size > DOCUMENT_BYTE_LIMIT - *total {
        return Err(DocumentError::new("generated", Category::MergedSize, ""));
    }
    *total += size;
    Ok(())
}

fn requirements(value: Option<&Value>) -> SecurityRequirements {
    value
        .map(|value| {
            serde_json::from_value(value.clone()).expect("validated security requirements")
        })
        .unwrap_or_default()
}

fn validate_security(
    router: &str,
    document: &Value,
    security: &SecurityRequirements,
    location: &str,
) -> Result<(), DocumentError> {
    for requirement in security {
        for name in requirement.keys() {
            document
                .pointer(&pointer("/components/securitySchemes", name))
                .ok_or_else(|| DocumentError::new(router, Category::Security, location))?;
        }
    }
    Ok(())
}

fn and_security(
    host: &SecurityRequirements,
    provider: &SecurityRequirements,
    budget: &Budget,
) -> Result<SecurityRequirements, DocumentError> {
    budget.size(host)?;
    budget.size(provider)?;
    if host.is_empty() {
        return Ok(provider.clone());
    }
    if provider.is_empty() {
        return Ok(host.clone());
    }
    let mut combined = Vec::new();
    let mut bytes = 0;
    for host in host {
        for provider in provider {
            budget.check()?;
            let mut requirement = host.clone();
            for (name, scopes) in provider {
                let target = requirement.entry(name.clone()).or_default();
                for scope in scopes {
                    budget.check()?;
                    if !target.contains(scope) {
                        target.push(scope.clone());
                    }
                }
            }
            add_operation_size(&mut bytes, budget.size(&requirement)?)?;
            combined.push(requirement);
        }
    }
    Ok(combined)
}

fn rebase_path(path: &str, mount: &str) -> String {
    if mount == "/" {
        path.to_owned()
    } else if path == "/" {
        mount.to_owned()
    } else {
        format!("{mount}{path}")
    }
}

fn rebase_pointer(path: &str, mount: &str) -> String {
    let Some(tail) = path.strip_prefix("/paths/") else {
        return path.to_owned();
    };
    let (token, suffix) = tail
        .split_once('/')
        .map_or((tail, ""), |(token, suffix)| (token, suffix));
    let unescaped = token.replace("~1", "/").replace("~0", "~");
    let mut result = pointer("/paths", &rebase_path(&unescaped, mount));
    if !suffix.is_empty() {
        result.push('/');
        result.push_str(suffix);
    }
    result
}

fn fragment(pointer: &str) -> String {
    let mut result = String::from("#");
    for byte in pointer.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?".contains(&byte) {
            result.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(result, "%{byte:02X}").unwrap();
        }
    }
    result
}

fn merge_paths(
    router: &str,
    target: &mut Map<String, Value>,
    shapes: &mut BTreeMap<Vec<Option<String>>, String>,
    ids: &mut BTreeSet<String>,
    paths: Value,
    budget: &Budget,
    operation_bytes: &mut usize,
) -> Result<(), DocumentError> {
    for (path, item) in paths.as_object().unwrap() {
        budget.check()?;
        let location = pointer("/paths", path);
        if path.starts_with("x-") {
            equal_or_insert(
                router,
                target,
                path,
                item.clone(),
                Category::ExtensionConflict,
                &location,
            )?;
            continue;
        }
        let shape: Vec<_> = path
            .split('/')
            .map(|part| {
                if part.starts_with('{') && part.ends_with('}') {
                    None
                } else {
                    Some(
                        urlencoding::decode(part)
                            .expect("validated path encoding")
                            .into_owned(),
                    )
                }
            })
            .collect();
        if let Some(previous) = shapes.get(&shape) {
            if previous != path {
                return Err(DocumentError::new(
                    router,
                    Category::OperationConflict,
                    &location,
                ));
            }
        } else {
            shapes.insert(shape, path.clone());
        }
        let incoming = item.as_object().unwrap();
        if let Some(previous) = target.get(path) {
            let empty = json!([]);
            if previous.get("parameters").unwrap_or(&empty)
                != item.get("parameters").unwrap_or(&empty)
            {
                return Err(DocumentError::new(
                    router,
                    Category::PathItemConflict,
                    &pointer(&location, "parameters"),
                ));
            }
        }
        let previous = target
            .entry(path)
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        for (key, value) in incoming {
            let location = pointer(&location, key);
            if METHODS.contains(&key.as_str()) {
                if previous.contains_key(key) {
                    return Err(DocumentError::new(
                        router,
                        Category::OperationConflict,
                        &location,
                    ));
                }
                if let Some(id) = value.get("operationId").and_then(Value::as_str)
                    && (id.is_empty() || !ids.insert(id.to_owned()))
                {
                    return Err(DocumentError::new(
                        router,
                        Category::OperationIdConflict,
                        &pointer(&location, "operationId"),
                    ));
                }
                let mut operation = value.clone();
                operation
                    .as_object_mut()
                    .unwrap()
                    .entry("security")
                    .or_insert(json!([]));
                add_operation_size(operation_bytes, budget.size(&operation)?)?;
                previous.insert(key.clone(), operation);
            } else {
                equal_or_insert(
                    router,
                    previous,
                    key,
                    value.clone(),
                    Category::PathItemConflict,
                    &location,
                )?;
            }
        }
    }
    Ok(())
}

fn equal_or_insert(
    router: &str,
    target: &mut Map<String, Value>,
    key: &str,
    value: Value,
    category: Category,
    location: &str,
) -> Result<(), DocumentError> {
    if let Some(previous) = target.get(key) {
        if previous != &value {
            return Err(DocumentError::new(router, category, location));
        }
    } else {
        target.insert(key.to_owned(), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
