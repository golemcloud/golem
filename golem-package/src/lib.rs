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

use anyhow::{Context, bail, ensure};
use golem_common::model::agent::{
    AgentConfigSource, AgentTypeName, extraction::ExtractedComponentMetadata,
};
use golem_common::model::component::{AgentFilePath, AgentFilePermissions, ComponentName};
use golem_common::model::tool::ToolName;
use golem_common::model::tool_middleware::ToolMiddlewareName;
use golem_common::schema::agent::{AgentConfigDeclarationSchema, ComponentConfigSchema};
use golem_common::schema::{SchemaGraph, SchemaType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

mod expand;
pub use expand::*;

#[cfg(test)]
mod tests;
#[cfg(test)]
test_r::enable!();

const ARTIFACT: &str = "application/vnd.golem.package.v1";
const CONFIG: &str = "application/vnd.golem.package.config.v1+json";
const INDEX: &str = "application/vnd.oci.image.index.v1+json";
const MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const WASM: &str = "application/wasm";
pub const INITIAL_FILES: &str = "application/vnd.golem.package.initial-files.v1.tar";
pub const FILE: &str = "application/vnd.golem.package.file.v1";
pub const FILE_DIRECTORY: &str = "application/vnd.golem.package.file-directory.v1.tar";
pub const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 16_384;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub calls: BTreeSet<AgentTypeName>,
}

pub fn validate_identity(identity: &PackageIdentity) -> anyhow::Result<()> {
    ensure!(
        valid_name(&identity.name),
        "Invalid package name {:?}",
        identity.name
    );
    ensure!(
        identity
            .calls
            .iter()
            .all(|name| !name.as_str().trim().is_empty()),
        "Package {:?} lists an empty agent type name in calls",
        identity.name
    );
    validate_version(&identity.version)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn validate_version(version: &str) -> anyhow::Result<()> {
    ensure!(
        version.len() <= 128 && semver::Version::parse(version).is_ok_and(|v| v.build.is_empty()),
        "Invalid exact package version {version:?} (expected SemVer without build metadata)"
    );
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageConfig {
    pub application: String,
    pub package: PackageIdentity,
    pub components: BTreeMap<ComponentName, PackageComponent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageComponent {
    pub wasm: String,
    pub metadata: ExtractedComponentMetadata,
    pub config_schema: ComponentConfigSchema,
    pub provision: PackageProvision,
    pub agents: BTreeMap<AgentTypeName, PackageProvision>,
    pub tools: BTreeMap<ToolName, PackageProvision>,
    pub tool_middlewares: BTreeMap<ToolMiddlewareName, PackageProvision>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageProvision {
    pub config: Value,
    pub file_defaults: Vec<PackageFileDefault>,
    pub env: BTreeMap<String, String>,
    pub initial_card: Option<ManifestInitialCard>,
    pub plugins: Vec<PluginInstallation>,
    pub files: Option<PackageInitialFiles>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestInitialCard {
    #[serde(default)]
    pub lower_bound: ManifestInitialCardBound,
    #[serde(default)]
    pub upper_bound: ManifestInitialCardBound,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestInitialCardBound {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positive: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub negative: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub parameters: std::collections::HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageFileDefault {
    pub path: Vec<String>,
    pub digest: String,
    pub directory: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageInitialFiles {
    pub layer: String,
    pub entries: BTreeMap<String, PackageInitialFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageInitialFile {
    pub target: AgentFilePath,
    pub permissions: AgentFilePermissions,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    media_type: String,
    pub digest: String,
    size: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OciIndex {
    schema_version: u32,
    media_type: String,
    manifests: Vec<Descriptor>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OciManifest {
    schema_version: u32,
    media_type: String,
    artifact_type: String,
    config: Descriptor,
    layers: Vec<Descriptor>,
}

#[derive(Debug)]
pub struct ValidatedPackage {
    digest: String,
    config: PackageConfig,
    entries: BTreeMap<String, Vec<u8>>,
}

impl ValidatedPackage {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn config(&self) -> &PackageConfig {
        &self.config
    }

    pub fn blob(&self, digest: &str) -> Option<&[u8]> {
        self.entries
            .get(&blob_path(digest).ok()?)
            .map(Vec::as_slice)
    }
}

pub fn add_blob(
    blobs: &mut BTreeMap<String, Vec<u8>>,
    media_type: &str,
    bytes: Vec<u8>,
) -> Descriptor {
    let descriptor = describe(media_type, &bytes);
    blobs.entry(descriptor.digest.clone()).or_insert(bytes);
    descriptor
}

fn describe(media_type: &str, bytes: &[u8]) -> Descriptor {
    Descriptor {
        media_type: media_type.into(),
        digest: format!("sha256:{:x}", Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

fn blob_path(digest: &str) -> anyhow::Result<String> {
    let hex = digest.strip_prefix("sha256:").unwrap_or_default();
    ensure!(
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid OCI digest {digest:?}"
    );
    Ok(format!("blobs/sha256/{hex}"))
}

fn canonical_json(value: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

pub fn validate_default(value: &Value) -> anyhow::Result<()> {
    match value {
        Value::Object(fields) => {
            for key in ["$secret", "$config"] {
                ensure!(
                    !fields.contains_key(key),
                    "Package defaults cannot contain {key} references"
                );
            }
            for value in fields.values() {
                validate_default(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_default(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn reject_secret_defaults(
    config: &Value,
    graph: &SchemaGraph,
    declarations: &[AgentConfigDeclarationSchema],
) -> anyhow::Result<()> {
    for declaration in declarations {
        if let Some(value) = object_value(config, &declaration.path) {
            ensure!(
                declaration.source != AgentConfigSource::Secret
                    && !contains_secret_default(graph, &declaration.value_type, value, 0)
                        .with_context(|| format!(
                            "Invalid default at {}",
                            declaration.path.join(".")
                        ))?,
                "Package default supplies a secret at {}",
                declaration.path.join(".")
            );
        }
    }
    Ok(())
}

pub fn file_default_paths(
    config: &Value,
    graph: &SchemaGraph,
    declarations: &[AgentConfigDeclarationSchema],
) -> anyhow::Result<BTreeSet<Vec<String>>> {
    fn collect(
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &Value,
        path: &mut Vec<String>,
        result: &mut BTreeSet<Vec<String>>,
        depth: usize,
    ) -> anyhow::Result<()> {
        ensure!(
            depth < 128,
            "Package default exceeds the schema nesting limit"
        );
        let ty = graph.resolve_ref(ty)?;
        if golem_schema::schema::File::is_declared_type(ty) {
            ensure!(
                value.is_string(),
                "File default at {} must be a string",
                path.join(".")
            );
            result.insert(path.clone());
            return Ok(());
        }
        let mut visit = |key: String, ty: &SchemaType, value: &Value| {
            path.push(key);
            let done = collect(graph, ty, value, path, result, depth + 1);
            path.pop();
            done
        };
        match ty {
            SchemaType::Record { fields, .. } => {
                for field in fields {
                    if let Some(value) = value.get(&field.name) {
                        visit(field.name.clone(), &field.body, value)?;
                    }
                }
            }
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                for (index, item) in value.as_array().into_iter().flatten().enumerate() {
                    visit(index.to_string(), element, item)?;
                }
            }
            SchemaType::Tuple { elements, .. } => {
                for (index, (ty, item)) in elements
                    .iter()
                    .zip(value.as_array().into_iter().flatten())
                    .enumerate()
                {
                    visit(index.to_string(), ty, item)?;
                }
            }
            SchemaType::Option { inner, .. } if !value.is_null() => {
                collect(graph, inner, value, path, result, depth + 1)?
            }
            SchemaType::Union { spec, .. } => {
                let branch =
                    golem_schema::schema::render::json_value::select_union_branch(spec, value)
                        .with_context(|| format!("Invalid union default at {}", path.join(".")))?;
                collect(graph, &branch.body, value, path, result, depth + 1)?;
            }
            SchemaType::Variant { cases, .. } => {
                for case in cases {
                    if let (Some(ty), Some(item)) = (&case.payload, value.get(&case.name)) {
                        visit(case.name.clone(), ty, item)?;
                    }
                }
            }
            SchemaType::Result { spec, .. } => {
                for (key, ty) in [("ok", &spec.ok), ("err", &spec.err)] {
                    if let (Some(ty), Some(item)) = (ty, value.get(key)) {
                        visit(key.into(), ty, item)?;
                    }
                }
            }
            SchemaType::Map {
                key: key_type,
                value: item_type,
                ..
            } => {
                for (key, item) in value.as_object().into_iter().flatten() {
                    ensure!(
                        !golem_schema::schema::File::is_declared_type(graph.resolve_ref(key_type)?),
                        "Package defaults do not support File map keys"
                    );
                    visit(key.clone(), item_type, item)?;
                }
                for (index, pair) in value.as_array().into_iter().flatten().enumerate() {
                    if let Some(key) = pair.get(0) {
                        let mut key_paths = BTreeSet::new();
                        collect(
                            graph,
                            key_type,
                            key,
                            &mut Vec::new(),
                            &mut key_paths,
                            depth + 1,
                        )?;
                        ensure!(
                            key_paths.is_empty(),
                            "Package defaults do not support File map keys"
                        );
                    }
                    if let Some(item) = pair.get(1) {
                        path.push(index.to_string());
                        path.push("1".into());
                        collect(graph, item_type, item, path, result, depth + 1)?;
                        path.truncate(path.len() - 2);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut result = BTreeSet::new();
    for declaration in declarations
        .iter()
        .filter(|decl| decl.source == AgentConfigSource::Local)
    {
        if let Some(value) = object_value(config, &declaration.path) {
            collect(
                graph,
                &declaration.value_type,
                value,
                &mut declaration.path.clone(),
                &mut result,
                0,
            )?;
        }
    }
    Ok(result)
}

fn file_layers(
    provision: &PackageProvision,
    paths: BTreeSet<Vec<String>>,
) -> anyhow::Result<Vec<(&'static str, String)>> {
    ensure!(
        provision
            .file_defaults
            .iter()
            .map(|file| &file.path)
            .eq(paths.iter()),
        "Package File layers do not match config declarations"
    );
    provision
        .file_defaults
        .iter()
        .map(|file| {
            ensure!(
                config_value(&provision.config, &file.path).and_then(Value::as_str)
                    == Some(file.digest.as_str()),
                "Package File default does not match its layer digest"
            );
            Ok((
                if file.directory { FILE_DIRECTORY } else { FILE },
                file.digest.clone(),
            ))
        })
        .collect()
}

fn object_value<'a>(config: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter()
        .try_fold(config, |value, key| value.as_object()?.get(key))
}

pub fn config_value<'a>(config: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(config, |value, key| match value {
        Value::Array(items) => items.get(key.parse::<usize>().ok()?),
        _ => value.get(key),
    })
}

pub fn replace_config_value(
    config: &mut Value,
    path: &[String],
    replacement: Value,
) -> anyhow::Result<()> {
    let mut value = config;
    for key in path {
        value = match value {
            Value::Array(items) => key
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get_mut(index)),
            _ => value.get_mut(key),
        }
        .context("Missing File default")?;
    }
    *value = replacement;
    Ok(())
}

fn contains_secret_default(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &Value,
    depth: usize,
) -> anyhow::Result<bool> {
    ensure!(
        depth < 128,
        "Package default exceeds the schema nesting limit"
    );
    let child = |ty, value| contains_secret_default(graph, ty, value, depth + 1);
    match graph.resolve_ref(ty)? {
        SchemaType::Secret { .. } => return Ok(true),
        SchemaType::Record { fields, .. } => {
            for field in fields {
                if let Some(field_value) = value.get(&field.name)
                    && child(&field.body, field_value)?
                {
                    return Ok(true);
                }
            }
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            for item in value.as_array().into_iter().flatten() {
                if child(element, item)? {
                    return Ok(true);
                }
            }
        }
        SchemaType::Tuple { elements, .. } => {
            for (ty, item) in elements.iter().zip(value.as_array().into_iter().flatten()) {
                if child(ty, item)? {
                    return Ok(true);
                }
            }
        }
        SchemaType::Option { inner, .. } if !value.is_null() => {
            return child(inner, value);
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let (Some(ty), Some(item)) = (&case.payload, value.get(&case.name))
                    && child(ty, item)?
                {
                    return Ok(true);
                }
            }
        }
        SchemaType::Result { spec, .. } => {
            for (key, ty) in [("ok", &spec.ok), ("err", &spec.err)] {
                if let (Some(ty), Some(item)) = (ty, value.get(key))
                    && child(ty, item)?
                {
                    return Ok(true);
                }
            }
        }
        SchemaType::Map {
            key,
            value: item_type,
            ..
        } => {
            for (name, item) in value.as_object().into_iter().flatten() {
                if contains_secret_default(graph, key, &Value::String(name.clone()), depth + 1)?
                    || child(item_type, item)?
                {
                    return Ok(true);
                }
            }
            for pair in value.as_array().into_iter().flatten() {
                if let (Some(name), Some(item)) = (pair.get(0), pair.get(1))
                    && (child(key, name)? || child(item_type, item)?)
                {
                    return Ok(true);
                }
            }
        }
        SchemaType::Union { spec, .. } => {
            let branch =
                golem_schema::schema::render::json_value::select_union_branch(spec, value)?;
            return child(&branch.body, value);
        }
        _ => {}
    }
    Ok(false)
}

fn layers_for(config: &PackageConfig) -> anyhow::Result<Vec<(&str, String)>> {
    validate_identity(&config.package)?;
    ensure!(!config.components.is_empty(), "Package has no components");
    let mut layers = Vec::new();
    let mut agents = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut middlewares = BTreeSet::new();
    for (name, component) in &config.components {
        layers.push((WASM, component.wasm.clone()));
        layers.extend(file_layers(
            &component.provision,
            file_default_paths(
                &component.provision.config,
                &component.config_schema.schema,
                &component.config_schema.declarations,
            )?,
        )?);
        reject_secret_defaults(
            &component.provision.config,
            &component.config_schema.schema,
            &component.config_schema.declarations,
        )
        .with_context(|| format!("Invalid default in component {name}"))?;
        for agent in &component.metadata.agent_types {
            ensure!(
                agents.insert(&agent.type_name),
                "Duplicate agent type {} in component {name}",
                agent.type_name
            );
            if let Some(provision) = component.agents.get(&agent.type_name) {
                layers.extend(file_layers(
                    provision,
                    file_default_paths(&provision.config, &agent.schema, &agent.config)?,
                )?);
                reject_secret_defaults(&provision.config, &agent.schema, &agent.config)
                    .with_context(|| {
                        format!("Invalid default in agent {} of {name}", agent.type_name)
                    })?;
            }
        }
        // Deploy can select one of multiple exports with the same tool name.
        // Only the selected export has a provision under that name.
        for tool in component.tools.keys() {
            ensure!(
                tools.insert(tool),
                "Duplicate tool provision {tool} in component {name}"
            );
        }
        for middleware in &component.metadata.tool_middlewares {
            for middleware_name in std::iter::once(&middleware.name).chain(&middleware.aliases) {
                ensure!(
                    middlewares.insert(middleware_name),
                    "Duplicate tool middleware name or alias {middleware_name} in component {name}"
                );
            }
        }
        for name in component.agents.keys() {
            ensure!(
                component
                    .metadata
                    .agent_types
                    .iter()
                    .any(|agent| &agent.type_name == name),
                "Agent provision {name} has no matching exported agent type"
            );
        }
        for provision in component
            .tools
            .values()
            .chain(component.tool_middlewares.values())
        {
            ensure!(
                provision.file_defaults.is_empty(),
                "File defaults require a supported config declaration"
            );
        }
        for provision in std::iter::once(&component.provision)
            .chain(component.agents.values())
            .chain(component.tools.values())
            .chain(component.tool_middlewares.values())
        {
            if let Some(files) = &provision.files {
                layers.push((INITIAL_FILES, files.layer.clone()));
            }
            validate_default(&provision.config)
                .with_context(|| format!("Invalid default in component {name}"))?;
        }
    }
    Ok(layers)
}

pub fn pack(
    config: &PackageConfig,
    blobs: &BTreeMap<String, Vec<u8>>,
) -> anyhow::Result<(String, Vec<u8>)> {
    let layers = layers_for(config)?
        .into_iter()
        .map(|(media_type, digest)| {
            let blob = blobs
                .get(&digest)
                .with_context(|| format!("Missing layer {digest}"))?;
            let descriptor = describe(media_type, blob);
            ensure!(descriptor.digest == digest, "Invalid layer {digest}");
            Ok(descriptor)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut entries = BTreeMap::new();
    for layer in &layers {
        entries
            .entry(blob_path(&layer.digest)?)
            .or_insert_with(|| std::borrow::Cow::Borrowed(blobs[&layer.digest].as_slice()));
    }
    let contents = canonical_json(config)?;
    let config = describe(CONFIG, &contents);
    entries.insert(blob_path(&config.digest)?, contents.into());
    let contents = canonical_json(&OciManifest {
        schema_version: 2,
        media_type: MANIFEST.into(),
        artifact_type: ARTIFACT.into(),
        config,
        layers,
    })?;
    let manifest = describe(MANIFEST, &contents);
    let digest = manifest.digest.clone();
    entries.insert(blob_path(&digest)?, contents.into());
    entries.insert(
        "index.json".into(),
        canonical_json(&OciIndex {
            schema_version: 2,
            media_type: INDEX.into(),
            manifests: vec![manifest],
        })?
        .into(),
    );
    entries.insert(
        "oci-layout".into(),
        std::borrow::Cow::Borrowed(br#"{"imageLayoutVersion":"1.0.0"}"#),
    );
    let bytes = write_tar(&entries)?;
    let validated = read(bytes.as_slice())?;
    Ok((validated.digest, bytes))
}

pub fn write_tar(entries: &BTreeMap<String, impl AsRef<[u8]>>) -> anyhow::Result<Vec<u8>> {
    let mut archive = tar::Builder::new(Vec::new());
    for (path, contents) in entries {
        let contents = contents.as_ref();
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(contents.len() as u64);
        archive.append_data(&mut header, path, contents)?;
    }
    Ok(archive.into_inner()?)
}

fn archive_entries(input: impl Read) -> anyhow::Result<BTreeMap<String, Vec<u8>>> {
    archive_entries_with_limit(input, MAX_ENTRIES)
}

fn portable_tar_component(part: &str) -> bool {
    if part.is_empty()
        || part.ends_with(['.', ' '])
        || part.contains(['<', '>', '"', '|', '?', '*'])
        || part.chars().any(char::is_control)
    {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered_device = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    !matches!(
        stem.as_str(),
        "CON" | "NUL" | "AUX" | "PRN" | "CONIN$" | "CONOUT$"
    ) && !numbered_device.is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

fn archive_entries_with_limit(
    input: impl Read,
    max_entries: usize,
) -> anyhow::Result<BTreeMap<String, Vec<u8>>> {
    let mut archive = tar::Archive::new(input.take(MAX_PACKAGE_BYTES + 1));
    let mut entries = BTreeMap::new();
    // Reject extension headers rather than allowing them to override raw entry paths or sizes.
    for entry in archive.entries()?.raw(true) {
        let mut entry = entry.context("Invalid package tar entry")?;
        let path = std::str::from_utf8(&entry.path_bytes())
            .context("Invalid UTF-8 in package entry path")?
            .to_owned();
        ensure!(
            !path.contains(['\\', ':']) && path.split('/').all(portable_tar_component),
            "Unsafe tar entry {path:?}"
        );
        ensure!(
            entry.header().entry_type() == tar::EntryType::Regular,
            "Non-regular tar entry {path:?}"
        );
        ensure!(!entries.contains_key(&path), "Duplicate tar entry {path:?}");
        ensure!(
            entries.len() < max_entries && entry.size() <= MAX_PACKAGE_BYTES,
            "Package limit exceeded at {path:?}"
        );
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .with_context(|| format!("Invalid tar entry {path:?}"))?;
        ensure!(
            contents.len() as u64 == entry.size(),
            "Truncated tar entry {path:?}"
        );
        entries.insert(path, contents);
    }
    let mut remaining = archive.into_inner();
    let mut trailer = [0; 512];
    remaining
        .read_exact(&mut trailer)
        .context("Missing package tar terminator")?;
    loop {
        ensure!(
            trailer == [0; 512],
            "Unexpected data after package tar terminator"
        );
        if remaining.read(&mut trailer[..1])? == 0 {
            break;
        }
        remaining
            .read_exact(&mut trailer[1..])
            .context("Truncated package tar trailer")?;
    }
    ensure!(remaining.limit() != 0, "Package size limit exceeded");
    Ok(entries)
}

fn entry<'a>(entries: &'a BTreeMap<String, Vec<u8>>, path: &str) -> anyhow::Result<&'a [u8]> {
    entries
        .get(path)
        .map(Vec::as_slice)
        .with_context(|| format!("Missing package entry {path}"))
}

fn parse<T: DeserializeOwned>(bytes: &[u8], path: &str) -> anyhow::Result<T> {
    let value: Value =
        serde_json::from_slice(bytes).with_context(|| format!("Invalid package entry {path}"))?;
    ensure!(
        canonical_json(&value)? == bytes,
        "Non-canonical JSON in package entry {path}"
    );
    serde_json::from_value(value).with_context(|| format!("Invalid package entry {path}"))
}

fn verify_blob<'a>(
    entries: &'a BTreeMap<String, Vec<u8>>,
    descriptor: &Descriptor,
    used: &mut BTreeSet<String>,
) -> anyhow::Result<&'a [u8]> {
    let path = blob_path(&descriptor.digest)?;
    let bytes = entry(entries, &path)?;
    if used.insert(path.clone()) {
        ensure!(
            describe(&descriptor.media_type, bytes).digest == descriptor.digest,
            "Digest mismatch for {path}"
        );
    }
    ensure!(
        bytes.len() as u64 == descriptor.size,
        "Size mismatch for {path}"
    );
    Ok(bytes)
}

/// Checks the complete OCI layout, including every referenced blob, without unpacking or executing it.
pub fn read(input: impl Read) -> anyhow::Result<ValidatedPackage> {
    let entries = archive_entries(input)?;
    let index: OciIndex = parse(entry(&entries, "index.json")?, "index.json")?;
    let layout: Value = parse(entry(&entries, "oci-layout")?, "oci-layout")?;
    ensure!(
        layout == json!({"imageLayoutVersion":"1.0.0"}),
        "Unsupported package oci-layout"
    );
    ensure!(
        index.schema_version == 2 && index.media_type == INDEX,
        "Invalid package index.json"
    );
    let [root] = index.manifests.as_slice() else {
        bail!("index.json must reference exactly one manifest")
    };
    ensure!(
        root.media_type == MANIFEST,
        "Invalid manifest media type in index.json"
    );
    let mut used = BTreeSet::from(["index.json".into(), "oci-layout".into()]);
    let manifest_path = blob_path(&root.digest)?;
    let manifest: OciManifest = parse(verify_blob(&entries, root, &mut used)?, &manifest_path)?;
    ensure!(
        manifest.schema_version == 2
            && manifest.media_type == MANIFEST
            && manifest.artifact_type == ARTIFACT
            && manifest.config.media_type == CONFIG,
        "Invalid Golem manifest {manifest_path}"
    );
    let config_path = blob_path(&manifest.config.digest)?;
    let config: PackageConfig = parse(
        verify_blob(&entries, &manifest.config, &mut used)?,
        &config_path,
    )?;
    let expected =
        layers_for(&config).with_context(|| format!("Invalid package config {config_path}"))?;
    ensure!(
        manifest.layers.len() == expected.len(),
        "Layer count mismatch in {manifest_path}"
    );
    for (layer, (media_type, digest)) in manifest.layers.iter().zip(expected) {
        ensure!(
            layer.media_type == media_type && layer.digest == digest,
            "Invalid layer {} in {manifest_path}",
            layer.digest
        );
        let contents = verify_blob(&entries, layer, &mut used)?;
        if layer.media_type == FILE_DIRECTORY {
            archive_entries(contents)
                .with_context(|| format!("Invalid File directory layer {}", layer.digest))?;
        }
    }
    for component in config.components.values() {
        for files in std::iter::once(&component.provision)
            .chain(component.agents.values())
            .chain(component.tools.values())
            .chain(component.tool_middlewares.values())
            .filter_map(|provision| provision.files.as_ref())
        {
            let path = blob_path(&files.layer)?;
            let mut targets = BTreeSet::new();
            for file in files.entries.values() {
                ensure!(
                    targets.insert(file.target.as_abs_str()),
                    "Duplicate initial-file target {} in {path}",
                    file.target.as_abs_str()
                );
            }
            let contents = archive_entries(entry(&entries, &path)?)
                .with_context(|| format!("Invalid initial-file layer {path}"))?;
            ensure!(
                contents.keys().eq(files.entries.keys())
                    && contents
                        .keys()
                        .enumerate()
                        .all(|(index, key)| *key == format!("{index:08}")),
                "Initial-file entries do not match config in {path}"
            );
        }
    }
    if let Some(path) = entries.keys().find(|path| !used.contains(*path)) {
        bail!("Unexpected package entry {path:?}");
    }
    Ok(ValidatedPackage {
        digest: root.digest.clone(),
        config,
        entries,
    })
}
