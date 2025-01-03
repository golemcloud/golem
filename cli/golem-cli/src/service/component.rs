// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::clients::component::ComponentClient;
use crate::clients::file_download::FileDownloadClient;
use crate::model::app_ext::InitialComponentFile;
use crate::model::component::{Component, ComponentView};
use crate::model::text::component::{ComponentAddView, ComponentGetView, ComponentUpdateView};
use crate::model::{ComponentName, Format, GolemError, GolemResult, PathBufOrStdin};
use async_trait::async_trait;
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use golem_client::model::ComponentType;
use golem_common::model::{
    ComponentFilePath, ComponentFilePathWithPermissions, ComponentFilePathWithPermissionsList,
};
use golem_common::model::{ComponentId, PluginInstallationId};
use golem_common::uri::oss::uri::ComponentUri;
use golem_common::uri::oss::url::ComponentUrl;
use golem_common::uri::oss::urn::ComponentUrn;
use indoc::formatdoc;
use itertools::Itertools;
use std::collections::HashMap;
use std::collections::{HashSet, VecDeque};
use std::fmt::Display;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs::File;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

#[async_trait]
pub trait ComponentService {
    type ProjectContext: Send + Sync;

    async fn add(
        &self,
        component_name: ComponentName,
        component_file: PathBufOrStdin,
        component_type: ComponentType,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
        files: Vec<InitialComponentFile>,
    ) -> Result<Component, GolemError>;

    async fn update(
        &self,
        component_uri: ComponentUri,
        component_file: PathBufOrStdin,
        component_type: Option<ComponentType>,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
        files: Vec<InitialComponentFile>,
    ) -> Result<GolemResult, GolemError>;

    async fn list(
        &self,
        component_name: Option<ComponentName>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn get(
        &self,
        component_uri: ComponentUri,
        version: Option<u64>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn resolve_uri(
        &self,
        uri: ComponentUri,
        project: &Option<Self::ProjectContext>,
    ) -> Result<ComponentUrn, GolemError>;

    async fn get_metadata(
        &self,
        component_urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError>;

    async fn get_latest_metadata(
        &self,
        component_urn: &ComponentUrn,
    ) -> Result<Component, GolemError>;

    async fn resolve_component_name(&self, uri: &ComponentUri) -> Result<String, GolemError> {
        match uri {
            ComponentUri::URN(urn) => {
                let component = self.get_metadata(urn, 0).await?;
                Ok(component.component_name)
            }
            ComponentUri::URL(ComponentUrl { name }) => Ok(name.clone()),
        }
    }

    async fn install_plugin(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> Result<GolemResult, GolemError>;

    async fn get_installations(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        version: Option<u64>,
    ) -> Result<GolemResult, GolemError>;

    async fn uninstall_plugin(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        installation_id: &PluginInstallationId,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ComponentServiceLive<ProjectContext> {
    pub client: Box<dyn ComponentClient<ProjectContext = ProjectContext> + Send + Sync>,
    pub file_download_client: Box<dyn FileDownloadClient + Send + Sync>,
}

impl<ProjectContext> ComponentServiceLive<ProjectContext> {
    async fn load_file(
        &self,
        component_file: InitialComponentFile,
    ) -> Result<Vec<LoadedFile>, GolemError> {
        let scheme = component_file.source_path.as_url().scheme();
        match scheme {
            "file" | "" => self.load_local_file(component_file).await,
            "http" | "https" => self
                .download_remote_file(component_file)
                .await
                .map(|f| vec![f]),
            _ => Err(GolemError(format!("Unsupported scheme: {}", scheme))),
        }
    }

    async fn load_local_file(
        &self,
        component_file: InitialComponentFile,
    ) -> Result<Vec<LoadedFile>, GolemError> {
        // if it's a directory, we need to recursively load all files and combine them with their target paths and permissions.
        let source_path = PathBuf::from(component_file.source_path.as_url().path());

        let mut results: Vec<LoadedFile> = vec![];
        let mut queue: VecDeque<(PathBuf, ComponentFilePathWithPermissions)> =
            vec![(source_path, component_file.target)].into();

        while let Some((path, target)) = queue.pop_front() {
            if path.is_dir() {
                let read_dir = tokio::fs::read_dir(&path).await.map_err(|err| {
                    GolemError(format!("Error reading directory {}: {err}", path.display()))
                })?;
                let mut read_dir_stream = ReadDirStream::new(read_dir);

                while let Some(entry) = read_dir_stream.next().await {
                    let entry = entry.map_err(|err| {
                        GolemError(format!("Error reading directory entry: {}", err))
                    })?;
                    let next_path = entry.path();

                    let file_name = entry.file_name().into_string().map_err(|_| {
                        GolemError(
                            "Error converting file name to string: Contains non-unicode data"
                                .to_string(),
                        )
                    })?;

                    let mut new_target = target.clone();
                    new_target
                        .extend_path(file_name.as_str())
                        .map_err(|err| GolemError(format!("Error extending path: {err}")))?;

                    queue.push_back((next_path, new_target));
                }
            } else {
                let content = tokio::fs::read(&path).await.map_err(|err| {
                    GolemError(format!(
                        "Error reading component file {}: {err}",
                        path.display()
                    ))
                })?;
                results.push(LoadedFile { content, target });
            }
        }
        Ok(results)
    }

    async fn download_remote_file(
        &self,
        component_file: InitialComponentFile,
    ) -> Result<LoadedFile, GolemError> {
        let content = self
            .file_download_client
            .download_file(component_file.source_path.into_url())
            .await
            .expect("request failed");
        Ok(LoadedFile {
            content,
            target: component_file.target,
        })
    }

    async fn build_files_archive(
        &self,
        component_files: Vec<InitialComponentFile>,
    ) -> Result<ComponentFilesArchive, GolemError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("golem-cli-zip")
            .tempdir()
            .map_err(|err| GolemError(format!("Error creating temporary dir: {}", err)))?;
        let zip_file_path = temp_dir.path().join("data.zip");
        let zip_file = File::create(&zip_file_path)
            .await
            .map_err(|err| GolemError(format!("Error creating temporary file: {}", err)))?;

        let mut zip_writer = ZipFileWriter::with_tokio(zip_file);

        let mut seen_paths: HashSet<ComponentFilePath> = HashSet::new();
        let mut successfully_added: Vec<ComponentFilePathWithPermissions> = vec![];

        for component_file in component_files {
            for LoadedFile { content, target } in self.load_file(component_file).await? {
                if !seen_paths.insert(target.path.clone()) {
                    return Err(GolemError(format!(
                        "Conflicting paths in component files: {}",
                        target.path
                    )));
                }

                // zip files do not like absolute paths. Convert the absolute component path to relative path from the root of the zip file
                let zip_entry_name = target.path.to_string();
                let builder = ZipEntryBuilder::new(zip_entry_name.into(), Compression::Deflate);

                zip_writer
                    .write_entry_whole(builder, &content)
                    .await
                    .map_err(|err| GolemError(format!("Error writing to archive: {}", err)))?;

                successfully_added.push(target);
            }
        }
        zip_writer
            .close()
            .await
            .map_err(|err| GolemError(format!("Error finishing archive: {}", err)))?;

        let properties = ComponentFilePathWithPermissionsList {
            values: successfully_added,
        };

        Ok(ComponentFilesArchive {
            _temp_dir: temp_dir,
            archive_path: zip_file_path,
            properties,
        })
    }
}

#[async_trait]
impl<ProjectContext: Display + Send + Sync> ComponentService
    for ComponentServiceLive<ProjectContext>
{
    type ProjectContext = ProjectContext;

    async fn add(
        &self,
        component_name: ComponentName,
        component_file: PathBufOrStdin,
        component_type: ComponentType,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
        files: Vec<InitialComponentFile>,
    ) -> Result<Component, GolemError> {
        let files_archive = if !files.is_empty() {
            Some(self.build_files_archive(files).await?)
        } else {
            None
        };

        let files_archive_path = files_archive.as_ref().map(|fa| fa.archive_path.as_path());
        let files_archive_properties = files_archive.as_ref().map(|fa| &fa.properties);

        let result = self
            .client
            .add(
                component_name.clone(),
                component_file.clone(),
                &project,
                component_type,
                files_archive_path,
                files_archive_properties,
            )
            .await;

        let can_fallback = format == Format::Text;
        let result = match result {
            Err(GolemError(message))
                if message.starts_with("Component already exists") && can_fallback =>
            {
                let answer = {
                    if non_interactive {
                        Ok(true)
                    } else {
                        inquire::Confirm::new("Would you like to update the existing component?")
                            .with_default(false)
                            .with_help_message(&message)
                            .prompt()
                    }
                };

                match answer {
                    Ok(true) => {
                        let component_uri = ComponentUri::URL(ComponentUrl {
                            name: component_name.0.clone(),
                        });
                        let urn = self.resolve_uri(component_uri, &project).await?;
                        self.client.update(urn, component_file, Some(component_type), files_archive_path,
                                           files_archive_properties).await

                    }
                    Ok(false) => Err(GolemError(message)),
                    Err(error) => Err(GolemError(format!("Error while asking for confirmation: {}; Use the --non-interactive (-y) flag to bypass it.", error))),
                }
            }
            Err(other) => Err(other),
            Ok(component) => Ok(component),
        }?;

        // We need to keep the files archive open until the client is done uploading it
        drop(files_archive);

        Ok(result)
    }

    async fn update(
        &self,
        component_uri: ComponentUri,
        component_file: PathBufOrStdin,
        component_type: Option<ComponentType>,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
        files: Vec<InitialComponentFile>,
    ) -> Result<GolemResult, GolemError> {
        let result = self.resolve_uri(component_uri.clone(), &project).await;

        let files_archive = if !files.is_empty() {
            Some(self.build_files_archive(files).await?)
        } else {
            None
        };

        let files_archive_path = files_archive.as_ref().map(|fa| fa.archive_path.as_path());
        let files_archive_properties = files_archive.as_ref().map(|fa| &fa.properties);

        let can_fallback =
            format == Format::Text && matches!(component_uri, ComponentUri::URL { .. });
        let result = match result {
            Err(GolemError(message))
                if message.starts_with("Can't find component") && can_fallback =>
            {
                let answer = {
                    if non_interactive {
                        Ok(true)
                    } else {
                        inquire::Confirm::new("Would you like to create a new component?")
                            .with_default(false)
                            .with_help_message(&message)
                            .prompt()
                    }
                };

                match answer {
                        Ok(true) => {
                            let component_name = match &component_uri {
                                ComponentUri::URL(ComponentUrl { name }) => ComponentName(name.clone()),
                                _ => unreachable!(),
                            };
                            self.client.add(component_name, component_file, &project, component_type.unwrap_or(ComponentType::Durable), files_archive_path, files_archive_properties).await.map(|component| {
                                GolemResult::Ok(Box::new(ComponentAddView(component.into())))
                            })

                        }
                        Ok(false) => Err(GolemError(message)),
                        Err(error) => Err(GolemError(format!("Error while asking for confirmation: {}; Use the --non-interactive (-y) flag to bypass it.", error))),
                    }
            }
            Err(other) => Err(other),
            Ok(urn) => self
                .client
                .update(
                    urn,
                    component_file.clone(),
                    component_type,
                    files_archive_path,
                    files_archive_properties,
                )
                .await
                .map(|component| GolemResult::Ok(Box::new(ComponentUpdateView(component.into())))),
        }?;

        // We need to keep the files archive open until the client is done uploading it
        drop(files_archive);

        Ok(result)
    }

    async fn list(
        &self,
        component_name: Option<ComponentName>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let components = self.client.find(component_name, &project).await?;
        let views: Vec<ComponentView> = components.into_iter().map(|t| t.into()).collect();

        Ok(GolemResult::Ok(Box::new(views)))
    }

    async fn get(
        &self,
        component_uri: ComponentUri,
        version: Option<u64>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let urn = self.resolve_uri(component_uri, &project).await?;
        let component = match version {
            Some(v) => self.get_metadata(&urn, v).await?,
            None => self.get_latest_metadata(&urn).await?,
        };
        let view: ComponentView = component.into();
        Ok(GolemResult::Ok(Box::new(ComponentGetView(view))))
    }

    async fn resolve_uri(
        &self,
        uri: ComponentUri,
        project_context: &Option<Self::ProjectContext>,
    ) -> Result<ComponentUrn, GolemError> {
        match uri {
            ComponentUri::URN(urn) => Ok(urn),
            ComponentUri::URL(ComponentUrl { name }) => {
                let components = self
                    .client
                    .find(Some(ComponentName(name.clone())), project_context)
                    .await?;
                let components: Vec<Component> = components
                    .into_iter()
                    .chunk_by(|c| c.versioned_component_id.component_id)
                    .into_iter()
                    .map(|(_, group)| {
                        group
                            .max_by_key(|c| c.versioned_component_id.version)
                            .unwrap()
                    })
                    .collect();

                if components.len() > 1 {
                    let project_msg = match project_context {
                        None => "".to_string(),
                        Some(project) => format!(" in project {project}"),
                    };
                    let ids: Vec<String> = components
                        .into_iter()
                        .map(|c| c.versioned_component_id.component_id.to_string())
                        .collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple components found for name {name}{project_msg}:
                        {}
                        Use explicit --component-id
                    ",
                        ids.join(", ")
                    )))
                } else {
                    match components.first() {
                        None => Err(GolemError(format!("Can't find component {name}"))),
                        Some(component) => Ok(ComponentUrn {
                            id: ComponentId(component.versioned_component_id.component_id),
                        }),
                    }
                }
            }
        }
    }

    async fn get_metadata(
        &self,
        urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError> {
        self.client.get_metadata(urn, version).await
    }

    async fn get_latest_metadata(&self, urn: &ComponentUrn) -> Result<Component, GolemError> {
        self.client.get_latest_metadata(urn).await
    }

    async fn install_plugin(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> Result<GolemResult, GolemError> {
        let urn = self.resolve_uri(component_uri, &project).await?;
        self.client
            .install_plugin(&urn, plugin_name, plugin_version, priority, parameters)
            .await
            .map(|installation| GolemResult::Ok(Box::new(installation)))
    }

    async fn get_installations(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        version: Option<u64>,
    ) -> Result<GolemResult, GolemError> {
        let urn = self.resolve_uri(component_uri, &project).await?;

        let version = match version {
            Some(v) => v,
            None => {
                let component = self.get_latest_metadata(&urn).await?;
                component.versioned_component_id.version
            }
        };

        let installations = self.client.get_installations(&urn, version).await?;
        Ok(GolemResult::Ok(Box::new(installations)))
    }

    async fn uninstall_plugin(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        installation_id: &PluginInstallationId,
    ) -> Result<GolemResult, GolemError> {
        let urn = self.resolve_uri(component_uri, &project).await?;
        self.client
            .uninstall_plugin(&urn, &installation_id.0)
            .await?;
        Ok(GolemResult::Str("Plugin uninstalled".to_string()))
    }
}

#[derive(Debug, Clone)]
struct LoadedFile {
    content: Vec<u8>,
    target: ComponentFilePathWithPermissions,
}

#[derive(Debug)]
struct ComponentFilesArchive {
    pub archive_path: PathBuf,
    pub properties: ComponentFilePathWithPermissionsList,
    _temp_dir: TempDir, // archive_path is only valid as long as this is alive
}
