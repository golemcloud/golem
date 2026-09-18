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

use golem_common::model::agent::http_files::HttpRequestTarget;
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::LazyLock;

pub(super) const PROVIDER_BYTE_LIMIT: usize = 1024 * 1024;
pub(super) const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Category {
    Size,
    Json,
    Structure,
    Unsupported,
    Path,
    Reference,
}

/// Contains locations and host-supplied identity only, never validation messages
/// from a library (which may include provider document values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DocumentError {
    pub router: String,
    pub category: Category,
    pub section: &'static str,
    pub location: String,
}

impl DocumentError {
    fn new(router: &str, category: Category, location: &str) -> Self {
        let section = match location.split('/').nth(1) {
            Some("paths") => "paths",
            Some("components") => "components",
            _ => "document",
        };
        Self {
            router: router.to_owned(),
            category,
            section,
            location: location.to_owned(),
        }
    }
}

pub(super) struct ProviderDocument {
    pub value: Value,
    /// Only semantic references are recorded, so rebasing never changes example data.
    pub references: Vec<LocalReference>,
}

#[derive(Debug)]
pub(super) struct LocalReference {
    pub location: String,
    /// Decoded JSON Pointer, without the URI fragment marker.
    pub target: String,
}

pub(super) fn parse(router: &str, input: &str) -> Result<ProviderDocument, DocumentError> {
    if input.len() > PROVIDER_BYTE_LIMIT {
        return Err(DocumentError::new(router, Category::Size, ""));
    }
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let mut value = StrictJson { depth: 0 }
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value))
        .map_err(|_| DocumentError::new(router, Category::Json, ""))?;
    if value.get("openapi").and_then(Value::as_str) != Some("3.1.0")
        || !value.get("paths").is_some_and(Value::is_object)
    {
        return Err(DocumentError::new(router, Category::Structure, ""));
    }
    if let Err(error) = STRUCTURE.validate(&value) {
        return Err(DocumentError::new(
            router,
            Category::Structure,
            &error.instance_path().to_string(),
        ));
    }
    let mut walker = Walker {
        router,
        targets: BTreeMap::new(),
        references: Vec::new(),
    };
    walker.walk(&value, Kind::Document, "")?;
    for (reference, kind) in &walker.references {
        if walker.targets.get(&reference.target) != Some(kind)
            || (*kind != Kind::Schema
                && resolve_object(&value, value.pointer(&reference.target).unwrap()).is_none())
        {
            return Err(DocumentError::new(
                router,
                Category::Reference,
                &reference.location,
            ));
        }
    }
    validate_parameters(router, &value)?;
    for (location, kind) in walker.targets {
        if kind != Kind::Schema
            && let Some(object) = value.pointer_mut(&location).and_then(Value::as_object_mut)
            && object.contains_key("$ref")
        {
            object.retain(|key, _| matches!(key.as_str(), "$ref" | "summary" | "description"));
        }
    }
    Ok(ProviderDocument {
        value,
        references: walker.references.into_iter().map(|(r, _)| r).collect(),
    })
}

fn resolve_object<'a>(document: &'a Value, mut value: &'a Value) -> Option<&'a Value> {
    let mut visited = BTreeSet::new();
    while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        let pointer = decode_pointer(reference)?;
        if !visited.insert(pointer.clone()) {
            return None;
        }
        value = document.pointer(&pointer)?;
    }
    Some(value)
}

fn validate_parameters(router: &str, document: &Value) -> Result<(), DocumentError> {
    for (path, item) in document["paths"].as_object().unwrap() {
        if path.starts_with("x-") {
            continue;
        }
        let location = pointer("/paths", path);
        let templates: BTreeSet<_> = path
            .split('/')
            .filter_map(|segment| segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
            .collect();
        let inherited = parameter_names(router, document, item, &location)?;
        if inherited
            .iter()
            .any(|(name, position)| *position == "path" && !templates.contains(name))
        {
            return Err(DocumentError::new(
                router,
                Category::Path,
                &pointer(&location, "parameters"),
            ));
        }
        for method in METHODS {
            if let Some(operation) = item.get(method) {
                let location = pointer(&location, method);
                let mut effective = inherited.clone();
                effective.extend(parameter_names(router, document, operation, &location)?);
                let names: BTreeSet<_> = effective
                    .iter()
                    .filter_map(|(name, position)| (*position == "path").then_some(*name))
                    .collect();
                if names != templates {
                    return Err(DocumentError::new(
                        router,
                        Category::Path,
                        &pointer(&location, "parameters"),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn parameter_names<'a>(
    router: &str,
    document: &'a Value,
    item: &'a Value,
    location: &str,
) -> Result<BTreeSet<(&'a str, &'a str)>, DocumentError> {
    let mut names = BTreeSet::new();
    if let Some(parameters) = item.get("parameters").and_then(Value::as_array) {
        for (index, parameter) in parameters.iter().enumerate() {
            let location = pointer(&pointer(location, "parameters"), &index.to_string());
            let parameter = resolve_object(document, parameter)
                .ok_or_else(|| DocumentError::new(router, Category::Reference, &location))?;
            let name = parameter["name"].as_str().unwrap();
            let position = parameter["in"].as_str().unwrap();
            if !names.insert((name, position)) {
                return Err(DocumentError::new(router, Category::Structure, &location));
            }
        }
    }
    Ok(names)
}

struct Offline;

impl jsonschema::Retrieve for Offline {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("OpenAPI schema retrieval is disabled".into())
    }
}

static STRUCTURE: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let mut schema: Value = serde_json::from_str(include_str!("schemas/schema.json")).unwrap();
    // OAS 3.1.0 section 4.8.20 allows literal values of any JSON type as Link
    // parameters, not only the strings accepted by the published schema.
    schema["$defs"]["link"]["properties"]["parameters"] = serde_json::json!({"type":"object"});
    offline_options()
        .build(&schema)
        .expect("offline OpenAPI validator")
});

static SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    offline_options()
        .with_resource(
            "https://spec.openapis.org/oas/3.1/meta/base",
            jsonschema::Resource::from_contents(
                serde_json::from_str(include_str!("schemas/meta.json")).unwrap(),
            ),
        )
        .build(&serde_json::from_str(include_str!("schemas/dialect.json")).unwrap())
        .expect("offline OpenAPI schema-object validator")
});

fn offline_options() -> jsonschema::ValidationOptions {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(false)
        .with_retriever(Offline)
}

/// Counts containers, not scalar values. Duplicate detection also applies in
/// opaque payloads: they are opaque to OpenAPI, but still must be strict JSON.
struct StrictJson {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for StrictJson {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for StrictJson {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("strict JSON")
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        if self.depth == 64 {
            return Err(A::Error::custom("container depth"));
        }
        let mut result = Vec::new();
        while let Some(value) = seq.next_element_seed(StrictJson {
            depth: self.depth + 1,
        })? {
            result.push(value);
        }
        Ok(Value::Array(result))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        if self.depth == 64 {
            return Err(A::Error::custom("container depth"));
        }
        let mut result = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if result.contains_key(&key) {
                return Err(A::Error::custom("duplicate key"));
            }
            let value = map.next_value_seed(StrictJson {
                depth: self.depth + 1,
            })?;
            result.insert(key, value);
        }
        Ok(Value::Object(result))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Document,
    Path,
    Operation,
    Schema,
    Response,
    Parameter,
    Example,
    RequestBody,
    Header,
    SecurityScheme,
    Link,
    Media,
    Encoding,
}

struct Walker<'a> {
    router: &'a str,
    targets: BTreeMap<String, Kind>,
    references: Vec<(LocalReference, Kind)>,
}

impl Walker<'_> {
    fn walk(&mut self, value: &Value, kind: Kind, path: &str) -> Result<(), DocumentError> {
        self.targets.insert(path.to_owned(), kind);
        let Some(object) = value.as_object() else {
            return Ok(()); // Boolean schemas.
        };
        let forbidden: &[&str] = match kind {
            Kind::Document => &["webhooks", "jsonSchemaDialect"],
            Kind::Path => &["$ref", "servers"],
            Kind::Operation => &["servers", "callbacks"],
            Kind::Schema => &[
                "$id",
                "$anchor",
                "$dynamicAnchor",
                "$dynamicRef",
                "$schema",
                "$recursiveRef",
                "$recursiveAnchor",
                "definitions",
                "dependencies",
            ],
            Kind::Example => &["externalValue"],
            _ => &[],
        };
        for name in forbidden {
            if object.contains_key(*name) {
                return Err(self.error(Category::Unsupported, &pointer(path, name)));
            }
        }
        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            self.reference(reference, &pointer(path, "$ref"), kind)?;
            // Unlike Schema Objects, Reference Objects ignore semantic siblings.
            if kind != Kind::Schema {
                return Ok(());
            }
        }
        match kind {
            Kind::Document => {
                for (name, item) in value["paths"].as_object().unwrap() {
                    if name.starts_with("x-") {
                        continue;
                    }
                    let location = pointer("/paths", name);
                    validate_path(name).map_err(|()| self.error(Category::Path, &location))?;
                    self.walk(item, Kind::Path, &location)?;
                }
                if let Some(components) = value.get("components").and_then(Value::as_object) {
                    for (name, items) in components {
                        let location = pointer("/components", name);
                        let kind = match name.as_str() {
                            "schemas" => Kind::Schema,
                            "responses" => Kind::Response,
                            "parameters" => Kind::Parameter,
                            "examples" => Kind::Example,
                            "requestBodies" => Kind::RequestBody,
                            "headers" => Kind::Header,
                            "securitySchemes" => Kind::SecurityScheme,
                            "links" => Kind::Link,
                            name if name.starts_with("x-") => continue,
                            _ => return Err(self.error(Category::Unsupported, &location)),
                        };
                        self.map(items, kind, &location)?;
                    }
                }
            }
            Kind::Path => {
                for method in METHODS {
                    self.child(value, method, Kind::Operation, path)?;
                }
                self.array(value, "parameters", Kind::Parameter, path)?;
            }
            Kind::Operation => {
                self.array(value, "parameters", Kind::Parameter, path)?;
                self.child(value, "requestBody", Kind::RequestBody, path)?;
                self.children(value, "responses", Kind::Response, path)?;
                if value.get("operationId").and_then(Value::as_str) == Some("") {
                    return Err(self.error(Category::Structure, &pointer(path, "operationId")));
                }
            }
            Kind::Response => {
                self.children(value, "headers", Kind::Header, path)?;
                self.children(value, "content", Kind::Media, path)?;
                self.children(value, "links", Kind::Link, path)?;
            }
            Kind::Parameter | Kind::Header => {
                self.child(value, "schema", Kind::Schema, path)?;
                self.children(value, "content", Kind::Media, path)?;
                self.children(value, "examples", Kind::Example, path)?;
            }
            Kind::RequestBody => self.children(value, "content", Kind::Media, path)?,
            Kind::Media => {
                self.child(value, "schema", Kind::Schema, path)?;
                self.children(value, "examples", Kind::Example, path)?;
                self.children(value, "encoding", Kind::Encoding, path)?;
            }
            Kind::Encoding => self.children(value, "headers", Kind::Header, path)?,
            Kind::Link => {
                if let Some(reference) = value.get("operationRef").and_then(Value::as_str) {
                    let location = pointer(path, "operationRef");
                    self.reference(reference, &location, Kind::Operation)?;
                    if !self
                        .references
                        .last()
                        .unwrap()
                        .0
                        .target
                        .starts_with("/paths/")
                    {
                        return Err(self.error(Category::Reference, &location));
                    }
                }
            }
            Kind::Schema => {
                if let Err(error) = SCHEMA.validate(value) {
                    return Err(self.error(
                        Category::Structure,
                        &format!("{path}{}", error.instance_path()),
                    ));
                }
                for name in [
                    "$defs",
                    "properties",
                    "patternProperties",
                    "dependentSchemas",
                ] {
                    self.children(value, name, Kind::Schema, path)?;
                }
                for name in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                    self.array(value, name, Kind::Schema, path)?;
                }
                for name in [
                    "not",
                    "if",
                    "then",
                    "else",
                    "items",
                    "contains",
                    "additionalProperties",
                    "propertyNames",
                    "unevaluatedItems",
                    "unevaluatedProperties",
                    "contentSchema",
                ] {
                    self.child(value, name, Kind::Schema, path)?;
                }
                if let Some(mapping) = value
                    .pointer("/discriminator/mapping")
                    .and_then(Value::as_object)
                {
                    for (name, target) in mapping {
                        self.reference(
                            target.as_str().unwrap(),
                            &pointer(&pointer(&pointer(path, "discriminator"), "mapping"), name),
                            Kind::Schema,
                        )?;
                    }
                }
            }
            Kind::Example | Kind::SecurityScheme => {}
        }
        Ok(())
    }

    fn child(
        &mut self,
        value: &Value,
        key: &str,
        kind: Kind,
        path: &str,
    ) -> Result<(), DocumentError> {
        if let Some(child) = value.get(key) {
            self.walk(child, kind, &pointer(path, key))?;
        }
        Ok(())
    }

    fn children(
        &mut self,
        value: &Value,
        key: &str,
        kind: Kind,
        path: &str,
    ) -> Result<(), DocumentError> {
        if let Some(children) = value.get(key) {
            self.map(children, kind, &pointer(path, key))?;
        }
        Ok(())
    }

    fn map(&mut self, value: &Value, kind: Kind, path: &str) -> Result<(), DocumentError> {
        for (name, child) in value.as_object().unwrap() {
            // Response maps admit extensions; maps of named objects do not reserve x-* names.
            if kind == Kind::Response
                && path.ends_with("/responses")
                && !path.starts_with("/components/")
                && name.starts_with("x-")
            {
                continue;
            }
            self.walk(child, kind, &pointer(path, name))?;
        }
        Ok(())
    }

    fn array(
        &mut self,
        value: &Value,
        key: &str,
        kind: Kind,
        path: &str,
    ) -> Result<(), DocumentError> {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            let path = pointer(path, key);
            for (index, item) in items.iter().enumerate() {
                self.walk(item, kind, &pointer(&path, &index.to_string()))?;
            }
        }
        Ok(())
    }

    fn reference(&mut self, value: &str, path: &str, kind: Kind) -> Result<(), DocumentError> {
        let target = decode_pointer(value).ok_or_else(|| self.error(Category::Reference, path))?;
        self.references.push((
            LocalReference {
                location: path.to_owned(),
                target,
            },
            kind,
        ));
        Ok(())
    }

    fn error(&self, category: Category, path: &str) -> DocumentError {
        DocumentError::new(self.router, category, path)
    }
}

pub(super) fn pointer(parent: &str, token: &str) -> String {
    format!("{parent}/{}", token.replace('~', "~0").replace('/', "~1"))
}

fn decode_pointer(reference: &str) -> Option<String> {
    let fragment = reference.strip_prefix('#')?;
    // urlencoding preserves malformed percent escapes, which are not URI fragments.
    let bytes = fragment.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if !byte.is_ascii_alphanumeric() && !b"-._~!$&'()*+,;=:@/?%".contains(byte) {
            return None;
        }
        if *byte == b'%'
            && !(bytes.get(index + 1)?.is_ascii_hexdigit()
                && bytes.get(index + 2)?.is_ascii_hexdigit())
        {
            return None;
        }
    }
    let decoded = urlencoding::decode(fragment).ok()?;
    if !decoded.starts_with("/components/") && !decoded.starts_with("/paths/") {
        return None;
    }
    let mut result = String::new();
    for token in decoded.strip_prefix('/')?.split('/') {
        let mut chars = token.chars();
        let mut unescaped = String::new();
        while let Some(ch) = chars.next() {
            unescaped.push(if ch == '~' {
                match chars.next()? {
                    '0' => '~',
                    '1' => '/',
                    _ => return None,
                }
            } else {
                ch
            });
        }
        result = pointer(&result, &unescaped);
    }
    Some(result)
}

fn validate_path(path: &str) -> Result<(), ()> {
    if !path.starts_with('/') || path.contains(['?', '#', '*']) {
        return Err(());
    }
    let mut literal = String::new();
    for segment in path.split('/').skip(1) {
        literal.push('/');
        if let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if name.is_empty() || name.contains(['{', '}']) {
                return Err(());
            }
            literal.push_str("parameter");
        } else {
            literal.push_str(segment);
        }
    }
    HttpRequestTarget::parse(&literal)
        .map(|_| ())
        .map_err(|_| ())
}

#[cfg(test)]
mod tests;
