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

use super::super::ComponentCommandHandler;
use super::{ComponentStager, extract_local_component, resolve_ifs_entry, resolve_tool_ifs_entry};
use crate::command_handler::component::ifs::expand_package_files;
use crate::model::app::{InitialComponentFile, ResolvedToolProvision};
use crate::package::{
    self, PackageComponent, PackageConfig, PackageFileDefault, PackageInitialFile,
    PackageInitialFiles, PackageProvision,
};
use anyhow::{Context, ensure};
use golem_common::model::component::AgentFilePath;
use golem_common::model::tool::ToolName;
use golem_common::model::tool_middleware::ToolMiddlewareName;
use golem_common::schema::tool::validation::{validate_tool, validate_tool_middleware};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tokio::io::AsyncReadExt;

impl ComponentCommandHandler {
    pub(crate) async fn pack_application(&self) -> anyhow::Result<(String, Vec<u8>)> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        let app_ctx = app_ctx.some_or_err_mut()?;
        let app = app_ctx.application();
        let mut config = PackageConfig {
            application: app.application_name().to_string(),
            package: app_ctx.publisher_package.clone().context(
                "Declare package: { name, version } in the root golem.yaml before packing",
            )?,
            components: BTreeMap::new(),
        };
        let mut stage = PackageStage {
            blobs: BTreeMap::new(),
            root: &tokio::fs::canonicalize(app.app_root_dir()).await?,
        };
        let mut staged_tools = BTreeSet::new();
        let mut staged_middlewares = BTreeSet::new();
        let mut staged_agents = BTreeSet::new();
        for name in app.component_names() {
            let component = app.component(name);
            let wasm = ComponentStager::open_local_wasm(&component.wasm())
                .await
                .with_context(|| format!("Cannot pack {name}; run golem build first"))?;
            let wasm = read_local(wasm).await?;
            // Extract from precisely the bytes written to the layer, not a build marker.
            let (metadata, agents) = extract_local_component(app_ctx, name, Some(&wasm))
                .await
                .with_context(|| format!("Failed to stage metadata for {name}"))?;
            let local =
                self.local_component_properties(app_ctx, name, metadata.clone(), &agents, false)?;
            let wasm = package::add_blob(&mut stage.blobs, package::WASM, wasm).digest;
            let config_schema = local.config_schema;
            let defaults = component.config().clone().unwrap_or_else(|| json!({}));
            package::reject_secret_defaults(
                &defaults,
                &config_schema.schema,
                &config_schema.declarations,
            )
            .with_context(|| format!("Invalid default in component {name}"))?;
            let mut provision = PackageProvision {
                config: defaults,
                env: local.component_env,
                initial_card: component.initial_card().cloned(),
                plugins: local.component_plugins,
                files: None,
                file_defaults: Vec::new(),
            };
            stage
                .file_defaults(
                    &mut provision,
                    &config_schema.schema,
                    &config_schema.declarations,
                    |path| component.layer_properties().config.source_at(path),
                )
                .await?;
            stage
                .provision(&mut provision, &local.component_files)
                .await
                .with_context(|| format!("Failed to stage defaults and files for {name}"))?;

            let mut agent_provisions = BTreeMap::new();
            for agent_type in &metadata.agent_types {
                staged_agents.insert(agent_type.type_name.clone());
                if let (Some(agent), Some(staged)) = (
                    agents.agent(&agent_type.type_name),
                    local.agent_type_configs.get(&agent_type.type_name),
                ) {
                    let defaults = agent.config().cloned().unwrap_or_else(|| json!({}));
                    package::reject_secret_defaults(
                        &defaults,
                        &agent_type.schema,
                        &agent_type.config,
                    )
                    .with_context(|| {
                        format!(
                            "Invalid default in agent {} of {name}",
                            agent_type.type_name
                        )
                    })?;
                    let mut provision = PackageProvision {
                        config: defaults,
                        env: staged.env.clone(),
                        initial_card: agent.initial_card().cloned(),
                        plugins: staged.plugins.clone(),
                        files: None,
                        file_defaults: Vec::new(),
                    };
                    stage
                        .file_defaults(
                            &mut provision,
                            &agent_type.schema,
                            &agent_type.config,
                            |path| agent.config_source_at(path),
                        )
                        .await?;
                    let files = staged
                        .files
                        .iter()
                        .map(|file| resolve_ifs_entry(file, &staged.files_source))
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    stage
                        .provision(&mut provision, &files)
                        .await
                        .with_context(|| {
                            format!("Failed to stage agent {} in {name}", agent_type.type_name)
                        })?;
                    agent_provisions.insert(agent_type.type_name.clone(), provision);
                }
            }

            let mut tools = BTreeMap::new();
            for tool in &metadata.tools {
                let tool_name = ToolName::try_from(tool.name().context("Tool has no root name")?)
                    .map_err(anyhow::Error::msg)?;
                let declaration = app
                    .tool_declarations()
                    .get(&tool_name)
                    .with_context(|| format!("Tool {tool_name} has no top-level declaration"))?;
                if declaration
                    .value
                    .component
                    .as_ref()
                    .is_some_and(|owner| owner != name)
                {
                    continue;
                }
                ensure!(
                    declaration.value.release.is_none(),
                    "Tool {tool_name} selects both a local export and a release"
                );
                validate_tool(tool).map_err(|errors| {
                    anyhow::anyhow!("Invalid tool {tool_name} in {name}: {errors:?}")
                })?;
                ensure!(
                    staged_tools.insert(tool_name.clone()),
                    "Tool {tool_name} is exported by multiple components"
                );
                tools.insert(
                    tool_name.clone(),
                    stage
                        .tool_provision(app.resolve_tool_provision(&tool_name, name)?)
                        .await
                        .with_context(|| format!("Failed to stage tool {tool_name} in {name}"))?,
                );
            }
            let mut tool_middlewares = BTreeMap::new();
            for middleware in &metadata.tool_middlewares {
                let middleware_name = ToolMiddlewareName::try_from(middleware.name.as_str())
                    .map_err(anyhow::Error::msg)?;
                validate_tool_middleware(middleware).map_err(|errors| {
                    anyhow::anyhow!(
                        "Invalid tool middleware {middleware_name} in {name}: {errors:?}"
                    )
                })?;
                ensure!(
                    staged_middlewares.insert(middleware_name.clone()),
                    "Tool middleware {middleware_name} is exported by multiple components"
                );
                if let Some(declaration) = app.tool_middleware_declarations().get(&middleware_name)
                    && declaration.value.release.is_none()
                {
                    ensure!(
                        declaration
                            .value
                            .component
                            .as_ref()
                            .is_none_or(|owner| owner == name),
                        "Tool middleware {middleware_name} is exported by {name} but declares a different component"
                    );
                    tool_middlewares.insert(
                        middleware_name.clone(),
                        stage
                            .tool_provision(
                                app.resolve_tool_middleware_provision(
                                    &middleware_name,
                                    Some(name),
                                )?,
                            )
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to stage tool middleware {middleware_name} in {name}"
                                )
                            })?,
                    );
                }
            }
            config.components.insert(
                name.clone(),
                PackageComponent {
                    wasm,
                    metadata,
                    config_schema,
                    provision,
                    agents: agent_provisions,
                    tools,
                    tool_middlewares,
                },
            );
        }
        for agent in app.agent_ids() {
            ensure!(
                staged_agents.contains(agent),
                "Agent {agent} is declared but not exported by any component"
            );
        }
        for (tool, declaration) in app.tool_declarations() {
            ensure!(
                declaration.value.release.is_some() || staged_tools.contains(tool),
                "Tool {tool} is declared but not exported by any component"
            );
        }
        for (middleware, declaration) in app.tool_middleware_declarations() {
            ensure!(
                declaration.value.release.is_some() || staged_middlewares.contains(middleware),
                "Tool middleware {middleware} is declared but not exported by any component"
            );
        }
        package::pack(&config, &stage.blobs)
    }
}

struct PackageStage<'a> {
    blobs: BTreeMap<String, Vec<u8>>,
    root: &'a Path,
}

impl PackageStage<'_> {
    async fn file_defaults<'a>(
        &mut self,
        provision: &mut PackageProvision,
        schema: &golem_common::schema::SchemaGraph,
        declarations: &[golem_common::schema::agent::AgentConfigDeclarationSchema],
        source: impl Fn(&[String]) -> Option<&'a Path>,
    ) -> anyhow::Result<()> {
        package::validate_default(&provision.config)?;
        for path in package::file_default_paths(&provision.config, schema, declarations)? {
            let value = package::config_value(&provision.config, &path)
                .and_then(serde_json::Value::as_str)
                .context("Expected a File path")?;
            let manifest = source(&path).context("Missing declaring manifest for File default")?;
            ensure!(
                !value.is_empty() && !value.contains([':', '\\']) && Path::new(value).is_relative(),
                "File default at {} must be a relative path",
                path.join(".")
            );
            let input = tokio::fs::canonicalize(
                manifest
                    .parent()
                    .context("Missing manifest directory")?
                    .join(value),
            )
            .await
            .with_context(|| format!("Cannot resolve File default at {}", path.join(".")))?;
            ensure!(
                input.starts_with(self.root),
                "File default at {} leaves the application root",
                path.join(".")
            );
            let directory = input.is_dir();
            let (kind, bytes) = if directory {
                ensure!(
                    input != self.root,
                    "File default at {} cannot capture the application root directory",
                    path.join(".")
                );
                let url = url::Url::from_directory_path(&input)
                    .map_err(|_| anyhow::anyhow!("Invalid File directory source"))?;
                let files = [InitialComponentFile {
                    source: crate::model::app::InitialComponentFileSource::new(
                        url.as_str(),
                        manifest,
                    )
                    .map_err(anyhow::Error::msg)?,
                    target: crate::model::app::CanonicalFilePathWithPermissions {
                        path: AgentFilePath::from_abs_str("/file")
                            .map_err(anyhow::Error::msg)?
                            .0,
                        permissions: golem_common::model::component::AgentFilePermissions::ReadOnly,
                    },
                }];
                let mut entries = BTreeMap::new();
                for entry in expand_package_files(&files, self.root).await? {
                    let relative = entry
                        .target
                        .path
                        .as_abs_str()
                        .trim_start_matches("/file/")
                        .to_owned();
                    let file = entry
                        .source
                        .as_url()
                        .to_file_path()
                        .map_err(|_| anyhow::anyhow!("Invalid File source"))?;
                    entries.insert(relative, read_local_path(&file).await?);
                }
                (package::FILE_DIRECTORY, package::write_tar(&entries)?)
            } else {
                (package::FILE, read_local_path(&input).await?)
            };
            let digest = package::add_blob(&mut self.blobs, kind, bytes).digest;
            package::replace_config_value(&mut provision.config, &path, json!(&digest))?;
            provision.file_defaults.push(PackageFileDefault {
                path,
                digest,
                directory,
            });
        }
        Ok(())
    }

    async fn tool_provision(
        &mut self,
        resolved: ResolvedToolProvision,
    ) -> anyhow::Result<PackageProvision> {
        let mut provision = PackageProvision {
            config: resolved.properties.config.unwrap_or_else(|| json!({})),
            env: resolved.properties.env,
            plugins: resolved.properties.plugins,
            ..Default::default()
        };
        let files = resolved
            .properties
            .files
            .iter()
            .map(resolve_tool_ifs_entry)
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.provision(&mut provision, &files).await?;
        Ok(provision)
    }

    async fn provision(
        &mut self,
        provision: &mut PackageProvision,
        files: &[InitialComponentFile],
    ) -> anyhow::Result<()> {
        package::validate_default(&provision.config)?;
        if !files.is_empty() {
            let mut loaded = expand_package_files(files, self.root).await?;
            loaded.sort_by(|a, b| a.target.path.as_abs_str().cmp(b.target.path.as_abs_str()));
            let mut archive = BTreeMap::new();
            let mut entries = BTreeMap::new();
            let mut previous = None;
            for (index, file) in loaded.into_iter().enumerate() {
                let path = file.target.path.as_abs_str();
                ensure!(
                    previous.as_deref() != Some(path),
                    "Duplicate initial file target {path}"
                );
                previous = Some(path.to_string());
                let entry = format!("{index:08}");
                archive.insert(
                    entry.clone(),
                    read_local(
                        tokio::fs::File::open(
                            file.source
                                .as_url()
                                .to_file_path()
                                .map_err(|_| anyhow::anyhow!("Invalid local initial-file URL"))?,
                        )
                        .await?,
                    )
                    .await?,
                );
                entries.insert(
                    entry,
                    PackageInitialFile {
                        target: AgentFilePath(file.target.path),
                        permissions: file.target.permissions,
                    },
                );
            }
            if !entries.is_empty() {
                let layer = package::add_blob(
                    &mut self.blobs,
                    package::INITIAL_FILES,
                    package::write_tar(&archive)?,
                )
                .digest;
                provision.files = Some(PackageInitialFiles { layer, entries });
            }
        }
        Ok(())
    }
}

async fn read_local(file: tokio::fs::File) -> anyhow::Result<Vec<u8>> {
    ensure!(
        file.metadata().await?.is_file(),
        "Package input must be a regular file"
    );
    let mut data = Vec::new();
    file.take(package::MAX_PACKAGE_BYTES + 1)
        .read_to_end(&mut data)
        .await?;
    ensure!(
        data.len() as u64 <= package::MAX_PACKAGE_BYTES,
        "Package input exceeds the size limit"
    );
    Ok(data)
}

async fn read_local_path(path: &Path) -> anyhow::Result<Vec<u8>> {
    ensure!(
        tokio::fs::metadata(path).await?.is_file(),
        "Package input must be a regular file"
    );
    read_local(tokio::fs::File::open(path).await?).await
}
