// Copyright 2024-2026 Golem Cloud
// SPDX-License-Identifier: LicenseRef-Golem-Source-License-1.1

use super::*;
use golem_common::model::component::ComponentName;
use golem_common::model::tool::ToolName;
use serde_json::Map;
use std::path::{Path, PathBuf};

pub const MAX_PACKAGES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePin {
    pub source: String,
    pub version: String,
    pub digest: String,
}

pub type Pins = BTreeMap<String, FilePin>;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error(
        "Too many packages (maximum {MAX_PACKAGES}); repair the manifest and rerun `golem install <file>`"
    )]
    Limit,
    #[error("Package {0:?} has no golem.lock entry; run `golem install <file>`")]
    Missing(String),
    #[error(
        "golem.lock entry {0:?} has no manifest entry; delete this entry from golem.lock if you no longer use the package, or run `golem install <file>` with the archive for {0:?}"
    )]
    Extra(String),
    #[error("golem.lock entry {0:?} disagrees with the manifest ({1}); run `golem install <file>`")]
    Mismatch(String, &'static str),
    #[error("Invalid golem.lock entry {name:?}; run `golem install <file>`: {cause:#}")]
    Invalid {
        name: String,
        #[source]
        cause: anyhow::Error,
    },
}

pub fn check_lock(manifest: &Pins, lock: &Pins) -> Result<(), LockError> {
    if manifest.len() > MAX_PACKAGES || lock.len() > MAX_PACKAGES {
        return Err(LockError::Limit);
    }
    for (name, expected) in manifest {
        let actual = lock
            .get(name)
            .ok_or_else(|| LockError::Missing(name.clone()))?;
        for (matches, field) in [
            (expected.source == actual.source, "source"),
            (expected.version == actual.version, "version"),
            (expected.digest == actual.digest, "digest"),
        ] {
            if !matches {
                return Err(LockError::Mismatch(name.clone(), field));
            }
        }
        let validate = || {
            ensure!(valid_name(name), "Invalid package name");
            ensure!(
                !actual.source.is_empty()
                    && actual.source.len() <= 4096
                    && !actual.source.contains('\0'),
                "Invalid package source"
            );
            validate_version(&actual.version)?;
            blob_path(&actual.digest)?;
            Ok(())
        };
        validate().map_err(|cause| LockError::Invalid {
            name: name.clone(),
            cause,
        })?;
    }
    if let Some(extra) = lock.keys().find(|key| !manifest.contains_key(*key)) {
        return Err(LockError::Extra(extra.clone()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ExpandError {
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error("golem.lock entry {name:?} ({location:?}, {digest}): {cause:#}")]
    Entry {
        name: String,
        location: String,
        digest: String,
        #[source]
        cause: anyhow::Error,
    },
}

/// The caller controls source authority, including authorization of cache hits. Preflight
/// runs for every pin before any cache, source, or unpack effect. A Golem tool can refuse
/// local file sources here (the only source type so far), without changing expansion.
#[async_trait::async_trait]
pub trait PackageEffects: Send {
    fn authorize_file(&self, name: &str, pin: &FilePin) -> anyhow::Result<()>;
    // Return at most `limit` bytes, or an error. Invalid cache bytes are treated as a miss.
    async fn cached(&mut self, digest: &str, limit: u64) -> anyhow::Result<Option<Vec<u8>>>;
    async fn fetch_file(&mut self, pin: &FilePin, limit: u64) -> anyhow::Result<Vec<u8>>;
    async fn cache(&mut self, digest: &str, verified: &[u8]) -> anyhow::Result<()>;
    // Paths are relative to `root` and contain only normal components. The caller must
    // use a fresh root or a confined filesystem view that does not follow symlinks.
    async fn create_dir(&mut self, root: &Path, relative: &Path) -> anyhow::Result<()>;
    async fn write_file(
        &mut self,
        root: &Path,
        relative: &Path,
        bytes: &[u8],
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct ExpandedPackages {
    pub declarations: Declarations,
    pub components: BTreeMap<ComponentName, InstalledComponent>,
    pub agent_packages: BTreeMap<AgentTypeName, String>,
    pub calls: BTreeMap<String, BTreeSet<AgentTypeName>>,
}

#[derive(Debug)]
pub struct InstalledComponent {
    pub wasm_path: PathBuf,
    pub metadata: ExtractedComponentMetadata,
    pub package_metadata: golem_common::model::component::ComponentPackageMetadata,
}

/// Ordinary manifest declarations, separate from publisher metadata. Build commands,
/// includes, templates, and environment declarations cannot be supplied by a package.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Declarations {
    components: Map<String, Value>,
    agents: Map<String, Value>,
    tools: Map<String, Value>,
    #[serde(skip)]
    tool_packages: BTreeMap<String, String>,
}

impl Declarations {
    pub fn tool_package(&self, name: &str) -> Option<&str> {
        self.tool_packages.get(name).map(String::as_str)
    }
}

#[derive(Debug)]
pub struct ToolSelection {
    pub published: Option<ToolName>,
    pub local: ToolName,
}

pub async fn expand(
    manifest: &Pins,
    lock: &Pins,
    selected_tools: &BTreeMap<String, Vec<ToolSelection>>,
    unpack_root: &Path,
    effects: &mut impl PackageEffects,
) -> Result<ExpandedPackages, ExpandError> {
    check_lock(manifest, lock)?;
    let entry_error = |name: &str, pin: &FilePin, cause| ExpandError::Entry {
        name: name.into(),
        location: pin.source.clone(),
        digest: pin.digest.clone(),
        cause,
    };
    for (name, pin) in lock {
        effects
            .authorize_file(name, pin)
            .map_err(|cause| entry_error(name, pin, cause))?;
    }

    let mut expanded = ExpandedPackages::default();
    for (name, pin) in lock {
        expand_one(
            &mut expanded,
            name,
            pin,
            selected_tools.get(name).map(Vec::as_slice),
            unpack_root,
            effects,
        )
        .await
        .map_err(|cause| entry_error(name, pin, cause))?;
    }
    Ok(expanded)
}

async fn expand_one(
    output: &mut ExpandedPackages,
    name: &str,
    pin: &FilePin,
    selections: Option<&[ToolSelection]>,
    root: &Path,
    effects: &mut impl PackageEffects,
) -> anyhow::Result<()> {
    let verify = |bytes: &[u8]| {
        ensure!(
            bytes.len() as u64 <= MAX_PACKAGE_BYTES,
            "Package size limit exceeded"
        );
        let package = read(bytes)?;
        ensure!(
            package.digest() == pin.digest,
            "Package digest mismatch: expected {}, found {}",
            pin.digest,
            package.digest()
        );
        let identity = &package.config().package;
        ensure!(
            identity.name == name && identity.version == pin.version,
            "Package identity mismatch: expected {name}@{}, found {}@{}",
            pin.version,
            identity.name,
            identity.version
        );
        Ok(package)
    };
    let cached = effects.cached(&pin.digest, MAX_PACKAGE_BYTES).await?;
    let cached = cached.and_then(|bytes| verify(&bytes).ok());
    let package = match cached {
        Some(package) => package,
        None => {
            let bytes = effects.fetch_file(pin, MAX_PACKAGE_BYTES).await?;
            let package = verify(&bytes)
                .context("Could not verify the source package digest and identity")?;
            effects.cache(&pin.digest, &bytes).await?;
            package
        }
    };

    let mut selected = BTreeMap::new();
    if let Some(selections) = selections {
        let available = package
            .config()
            .components
            .values()
            .flat_map(|component| component.tools.keys())
            .collect::<BTreeSet<_>>();
        let list = || {
            available
                .iter()
                .map(|tool| tool.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        for selection in selections {
            let published = if let Some(published) = &selection.published {
                published
            } else {
                ensure!(
                    available.len() == 1,
                    "Set tools.{}.tool to select a member; available tools: {}",
                    selection.local,
                    list()
                );
                available.first().copied().expect("one published tool")
            };
            ensure!(
                available.contains(published),
                "Unknown tool {published:?}; available tools: {}",
                list()
            );
            if let Some((_, previous)) =
                selected.insert(published.as_str(), (published, &selection.local))
            {
                bail!(
                    "Package {name:?} tool {published:?} is selected by both tools.{previous} and tools.{}",
                    selection.local
                );
            }
        }
    }

    let config = package.config();
    for agent in config
        .components
        .values()
        .flat_map(|c| &c.metadata.agent_types)
    {
        if let Some(previous) = output
            .agent_packages
            .insert(agent.type_name.clone(), name.into())
        {
            bail!(
                "Duplicate installed agent type {} in packages {previous:?} and {name:?}",
                agent.type_name
            );
        }
    }
    if !config.package.calls.is_empty() {
        output
            .calls
            .insert(name.into(), config.package.calls.clone());
    }

    let mut unpack = Unpack {
        package: &package,
        root,
        prefix: PathBuf::from(name),
        written: BTreeSet::new(),
        effects,
        remaining_entries: MAX_ENTRIES,
    };
    let mut middleware = Map::new();
    for (component, spec) in &package.config().components {
        let mut metadata = spec.metadata.clone();
        let mut tool_guest_names = BTreeMap::new();
        if selections.is_some() {
            metadata.tools.retain_mut(|tool| {
                let Some(&(published, local)) = tool.name().and_then(|name| selected.get(name))
                else {
                    return false;
                };
                if !spec.tools.contains_key(published) {
                    return false;
                }
                if local != published {
                    tool_guest_names.insert(local.clone(), published.clone());
                    tool.commands.nodes[0].name = local.to_string();
                }
                true
            });
        }
        let component = component.as_str();
        let wasm = unpack.blob(&spec.wasm, "wasm", false).await?;
        let mut declaration = unpack.provision(&spec.provision).await?;
        declaration.insert("componentWasm".into(), json!(wasm));
        declaration.insert(
            "configSchema".into(),
            serde_json::to_value(&spec.config_schema)?,
        );
        insert(&mut output.declarations.components, component, declaration)?;
        output.components.insert(
            ComponentName(component.into()),
            InstalledComponent {
                wasm_path: root.join(wasm),
                metadata,
                package_metadata: golem_common::model::component::ComponentPackageMetadata {
                    source: pin.source.clone(),
                    package_digest: pin.digest.clone(),
                    layer_digest: spec.wasm.clone(),
                    tool_guest_names,
                },
            },
        );
        for (name, provision) in &spec.agents {
            insert(
                &mut output.declarations.agents,
                name.as_str(),
                unpack.provision(provision).await?,
            )?;
        }
        for (tool, provision) in &spec.tools {
            let tool = if selections.is_some() {
                let Some(&(_, local)) = selected.get(tool.as_str()) else {
                    continue;
                };
                local
            } else {
                tool
            };
            let mut declaration = unpack.provision(provision).await?;
            declaration.insert("component".into(), json!(component));
            ensure!(
                tool.as_str() != "middleware",
                "Installed tool name 'middleware' is reserved"
            );
            if let Some(previous) = output.declarations.tool_package(tool.as_str()) {
                bail!(
                    "Tool {tool:?} is declared by both package {previous:?} and package {name:?}"
                );
            }
            insert(&mut output.declarations.tools, tool.as_str(), declaration)?;
            output
                .declarations
                .tool_packages
                .insert(tool.to_string(), name.to_owned());
        }
        for (name, provision) in &spec.tool_middlewares {
            let mut declaration = unpack.provision(provision).await?;
            declaration.insert("component".into(), json!(component));
            insert(&mut middleware, name.as_str(), declaration)?;
        }
    }
    if !middleware.is_empty() {
        let all = output
            .declarations
            .tools
            .entry("middleware")
            .or_insert_with(|| json!({}));
        for (name, declaration) in middleware {
            insert(
                all.as_object_mut().unwrap(),
                &name,
                declaration.as_object().unwrap().clone(),
            )?;
        }
    }
    Ok(())
}

fn insert(
    map: &mut Map<String, Value>,
    name: &str,
    declaration: Map<String, Value>,
) -> anyhow::Result<()> {
    ensure!(
        !map.contains_key(name),
        "Duplicate installed declaration {name:?}"
    );
    map.insert(name.into(), Value::Object(declaration));
    Ok(())
}

struct Unpack<'a, E> {
    package: &'a ValidatedPackage,
    root: &'a Path,
    prefix: PathBuf,
    effects: &'a mut E,
    written: BTreeSet<PathBuf>,
    remaining_entries: usize,
}

impl<E: PackageEffects> Unpack<'_, E> {
    async fn blob(&mut self, digest: &str, kind: &str, archive: bool) -> anyhow::Result<PathBuf> {
        let hex = digest
            .strip_prefix("sha256:")
            .context("Missing SHA-256 digest")?;
        blob_path(digest)?;
        let relative = self.prefix.join(kind).join(hex);
        if self.written.insert(relative.clone()) {
            let bytes = self.package.blob(digest).context("Missing package blob")?;
            if archive {
                let entries = super::archive_entries_with_limit(bytes, self.remaining_entries)?;
                self.remaining_entries -= entries.len();
                self.effects.create_dir(self.root, &relative).await?;
                for (name, contents) in entries {
                    let name = Path::new(&name);
                    ensure!(
                        name.components()
                            .all(|part| matches!(part, std::path::Component::Normal(_))),
                        "Unsafe unpack path {name:?}"
                    );
                    self.effects
                        .write_file(self.root, &relative.join(name), &contents)
                        .await?;
                }
            } else {
                self.remaining_entries = self
                    .remaining_entries
                    .checked_sub(1)
                    .context("Too many unpacked entries")?;
                self.effects.write_file(self.root, &relative, bytes).await?;
            }
        }
        Ok(relative)
    }

    async fn provision(&mut self, spec: &PackageProvision) -> anyhow::Result<Map<String, Value>> {
        let mut config = spec.config.clone();
        for file in &spec.file_defaults {
            let path = self
                .blob(
                    &file.digest,
                    if file.directory {
                        "directories"
                    } else {
                        "files"
                    },
                    file.directory,
                )
                .await?;
            replace_config_value(&mut config, &file.path, json!(path))?;
        }
        let mut result = Map::new();
        if !config.is_null() {
            result.insert("config".into(), config);
        }
        if !spec.env.is_empty() {
            result.insert("env".into(), json!(spec.env));
        }
        if let Some(card) = &spec.initial_card {
            result.insert("initialCard".into(), json!(card));
        }
        if !spec.plugins.is_empty() {
            result.insert("plugins".into(), json!(spec.plugins));
        }
        if let Some(files) = &spec.files {
            let dir = self.blob(&files.layer, "initial-files", true).await?;
            let files = files.entries.iter().map(|(name, file)| json!({
                "sourcePath": dir.join(name), "targetPath": file.target, "permissions": file.permissions,
            })).collect::<Vec<_>>();
            result.insert("files".into(), json!(files));
        }
        Ok(result)
    }
}
