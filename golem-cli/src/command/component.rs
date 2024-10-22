// Copyright 2024 Golem Cloud
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

use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use crate::command::ComponentRefSplit;
use crate::model::{
    ComponentName, Format, GolemError, GolemResult, PathBufOrStdin, WorkerUpdateMode,
};
use crate::service::component::ComponentService;
use crate::service::deploy::DeployService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;
use golem_client::model::ComponentType;
use std::sync::Arc;
use golem_wasm_rpc_stubgen::model::oam::{Application, Component};
use golem_wasm_rpc_stubgen::model::wasm_rpc::DEFAULT_CONFIG_FILE_NAME;
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{error, info};
use zip::write::{FileOptions, SimpleFileOptions};
use zip::ZipWriter;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubCommand<ProjectRef: clap::Args, ComponentRef: clap::Args> {
    /// Create golem.yaml file
    Init {
        #[command(flatten)]
        project_ref: ProjectRef,
    },
    /// Creates a new component with a given name by uploading the component WASM
    #[command(alias = "create")]
    Add {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Name of the newly created component
        #[arg(short, long)]
        component_name: ComponentName,

        /// The WASM file to be used as a Golem component
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBufOrStdin, // TODO: validate exists

        /// The component type. If none specified, the command creates a Durable component.
        #[command(flatten)]
        component_type: ComponentTypeArg,

        /// Do not ask for confirmation for performing an update in case the component already exists
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },

    /// Updates an existing component by uploading a new version of its WASM
    #[command()]
    Update {
        /// The component to update
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The WASM file to be used as a new version of the Golem component
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBufOrStdin, // TODO: validate exists

        /// The updated component's type. If none specified, the previous version's type is used.
        #[command(flatten)]
        component_type: UpdatedComponentTypeArg,

        /// Try to automatically update all existing workers to the new version
        #[arg(long, default_value_t = false)]
        try_update_workers: bool,

        /// Update mode - auto or manual
        #[arg(long, default_value = "auto", requires = "try_update_workers")]
        update_mode: WorkerUpdateMode,

        /// Do not ask for confirmation for creating a new component in case it does not exist
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },

    /// Lists the existing components
    #[command()]
    List {
        /// The project to list components from
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Optionally look for only components matching a given name
        #[arg(short, long)]
        component_name: Option<ComponentName>,
    },
    /// Get component
    #[command()]
    Get {
        /// The Golem component
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The version of the component
        #[arg(short = 't', long)]
        version: Option<u64>,
    },
    /// Try to automatically update all existing workers to the latest version
    #[command()]
    TryUpdateWorkers {
        /// The component to redeploy
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Update mode - auto or manual
        #[arg(long, default_value = "auto")]
        update_mode: WorkerUpdateMode,
    },
    /// Redeploy all workers of a component using the latest version
    #[command()]
    Redeploy {
        /// The component to redeploy
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Do not ask for confirmation
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct ComponentTypeArg {
    /// Create an Ephemeral component. If not specified, the command creates a Durable component.
    #[arg(long, group = "component-type-flag")]
    ephemeral: bool,

    /// Create a Durable component. This is the default.
    #[arg(long, group = "component-type-flag")]
    durable: bool,
}

impl ComponentTypeArg {
    pub fn component_type(&self) -> ComponentType {
        if self.ephemeral {
            ComponentType::Ephemeral
        } else {
            ComponentType::Durable
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct UpdatedComponentTypeArg {
    /// Create an Ephemeral component. If not specified, the previous version's type will be used.
    #[arg(long, group = "component-type-flag")]
    ephemeral: bool,

    /// Create a Durable component. If not specified, the previous version's type will be used.
    #[arg(long, group = "component-type-flag")]
    durable: bool,
}

impl UpdatedComponentTypeArg {
    pub fn optional_component_type(&self) -> Option<ComponentType> {
        if self.ephemeral {
            Some(ComponentType::Ephemeral)
        } else if self.durable {
            Some(ComponentType::Durable)
        } else {
            None
        }
    }
}

impl<
        ProjectRef: clap::Args + Send + Sync + 'static,
        ComponentRef: ComponentRefSplit<ProjectRef> + clap::Args,
    > ComponentSubCommand<ProjectRef, ComponentRef>
{
    pub async fn handle<ProjectContext: Clone + Send + Sync>(
        self,
        format: Format,
        service: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
        deploy_service: Arc<dyn DeployService<ProjectContext = ProjectContext> + Send + Sync>,
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ComponentSubCommand::Init {
                project_ref,
            }=> {
                match init_config_file(){
                    Ok(source) => {
                        info!("Created new config file at {:?}", source.display());
                        Ok(GolemResult::Str("Config file created".to_string()))
                    }
                    Err(error) => {
                        Err(GolemError(error.to_string()))?
                    }
                }
            }
            ComponentSubCommand::Add {
                project_ref,
                component_name,
                component_file,
                component_type,
                non_interactive,
            } => {
                match read_yaml_content() {
                    Ok(config) => {
                        match compress_files(config.clone()).await{
                            Ok(ifs) => {
                                // info!("Compressed file {:?}", ifs.metadata());
                                let project_id = projects.resolve_id_or_default(project_ref).await?;
                                service
                                    .add(
                                        component_name,
                                        component_file,
                                        component_type.component_type(),
                                        Some(project_id),
                                        non_interactive,
                                        format,
                                        ifs,
                                        config
                                    )
                                    .await
                            }
                            Err(error) => {
                                Err(GolemError(error.to_string()))
                            }
                        }
                    }
                    Err(error) => {Err(GolemError(error.to_string()))?}
                }
            }
            ComponentSubCommand::Update {
                component_name_or_uri,
                component_file,
                component_type,
                try_update_workers,
                update_mode,
                non_interactive,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;

                // let mut result = service
                //     .update(
                //         component_name_or_uri.clone(),
                //         component_file,
                //         component_type.optional_component_type(),
                //         project_id.clone(),
                //         non_interactive,
                //         format,
                //     )
                //     .await?;

                match read_yaml_content() {
                    Ok(config) => {

                        match compress_files(config.clone()).await{
                            Ok(ifs) => {
                                let mut result = service
                                    .update(
                                        component_name_or_uri.clone(),
                                        component_file,
                                        component_type.optional_component_type(),
                                        project_id.clone(),
                                        non_interactive,
                                        format,
                                        ifs,
                                        config
                                    )
                                    .await?;
                                if try_update_workers {
                                    let deploy_result = deploy_service
                                        .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                                        .await?;
                                    result = result.merge(deploy_result);
                                }
                                Ok(result)
                            }
                            Err(error) => {Err(GolemError(error.to_string()))?}
                        }

                    }
                    Err(error) => {Err(GolemError(error.to_string()))?}
                }


            }
            ComponentSubCommand::List {
                project_ref,
                component_name,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.list(component_name, Some(project_id)).await
            }
            ComponentSubCommand::Get {
                component_name_or_uri,
                version,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .get(component_name_or_uri, version, project_id)
                    .await
            }
            ComponentSubCommand::TryUpdateWorkers {
                component_name_or_uri,
                update_mode,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                deploy_service
                    .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                    .await
            }
            ComponentSubCommand::Redeploy {
                component_name_or_uri,
                non_interactive,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                deploy_service
                    .redeploy(component_name_or_uri, project_id, non_interactive, format)
                    .await
            }
        }
    }
}

async fn compress_files(application: Application) -> Result<PathBuf, Box<dyn Error>> {
    // Create an in-memory buffer (Vec<u8>)
    let mut buffer = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buffer);

    // Define options for file compression
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored) // You can also use Deflated, Bzip2, etc.
        .unix_permissions(0o755); // Set permissions

    // Add files to the zip with the defined options
    info!("Compressing");
    if let Some(component) = application.spec.components.get(0) {
        info!("Compressed component: {:?}", component);
        if let Some(files_values) = component.properties.get("files") {
            info!("Compressed files values: {:?}", files_values);

            // Deserialize the 'files' into Vec<FileProperty>
            let files: Vec<FileProperty> = match serde_json::from_value(files_values.clone()) {
                Ok(f) => f,
                Err(error) => {
                    error!("Deserialization error: {:?}", error);
                    return Err(Box::from(format!("Source path {:?} does not exist", error)));
                }
            };
            info!("Compressed files: {:?}", files);

            for file in &files {
                info!("Processing file: {:?}", file);
                let source_path = std::path::Path::new(&file.source_path);

                // Handle files based on permissions
                let target_folder = match file.permissions {
                    Permissions::ReadOnly => "read-only",
                    Permissions::ReadWrite => "read-write",
                };

                let target_path = format!("{}/{}", target_folder, file.target_path);

                if source_path.exists() {
                    let mut file_reader = std::fs::File::open(source_path)?;

                    // Add file to the corresponding folder in the ZIP archive
                    zip.start_file(target_path, options)?;
                    std::io::copy(&mut file_reader, &mut zip)?;
                } else {
                    error!("Source path {:?} does not exist", file.source_path);
                }
            }
        } else {
            error!("No 'files' property found in component ");
            return Err(Box::from("No files found in component ".to_string()));
        }
    } else {
        error!("No components found in application");
        return Err(Box::from("No component found in application".to_string()));
    }

    zip.finish()?;

    // Write the buffer to an asynchronous file
    let path = std::path::Path::new("ifs.zip").to_path_buf();
    let mut async_file = File::create(&path).await?;
    async_file.write_all(&buffer.into_inner()).await?;

    Ok(path)
}

fn read_yaml_content() -> Result<Application, Box<dyn Error>> {
    let current_dir = std::env::current_dir()?;
    let source =  current_dir.join(DEFAULT_CONFIG_FILE_NAME);

    if source.exists() {

        load_and_parse_yaml(source)

    }else{
        Err(Box::from("Config file does not exist"))
    }


}
fn load_and_parse_yaml(path: PathBuf) -> Result<Application, Box<dyn Error>> {
    let yaml_content = fs::read_to_string(&path)?;

    // Attempt to deserialize the YAML content into an Application
    match serde_yaml::from_str::<Application>(&yaml_content) {
        Ok(application) => {
            info!("Successfully loaded application: {:?}", application);
            Ok(application)  // Return the parsed application
        }
        Err(error) => {
            error!("Failed to parse YAML content: {}", error);
            Err(Box::new(error))  // Return the error
        }
    }
}

fn init_config_file() -> Result<PathBuf, Box<dyn Error>> {
    match find_main_source() {
        None => {
            info!("No config file found");
            let current_dir = std::env::current_dir()?;
            let source = current_dir.join(DEFAULT_CONFIG_FILE_NAME);

            // Scan the directory first
            scan_directory(current_dir)?;

            // Create the config file if the directory scan succeeds
            fs::File::create(&source).map_err(|error| {
                error!("Failed to create YAML config file: {}", error);
                Box::new(error) as Box<dyn Error>
            })?;

            // Write the YAML content to the newly created file
            write_yaml_content(source.clone())?;

            Ok(source) // Return the path of the newly created config file
        }
        Some(source) => {
            error!("Config file already exists");
            Err(Box::from("Config file already exists"))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Files{
    pub files: Vec<FileProperty>
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permissions{
    #[serde(rename = "read-only")]
    ReadOnly,

    #[serde(rename = "read-write")]
    ReadWrite,

}


#[derive(Debug,Clone, Serialize, Deserialize)]
pub struct FileProperty{
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    #[serde(rename = "targetPath")]
    pub target_path: String,
    pub permissions: Permissions
}


fn write_yaml_content(source: PathBuf) -> Result<(), Box<dyn Error>> {
    let mut application = Application::new("application_name".parse().unwrap());
    let current_dir = std::env::current_dir()?;
    let name = current_dir.file_name().unwrap().to_str().unwrap();

    let read_only_dir = current_dir.join("read-only");
    let mut files_in_dir: Vec<FileProperty> = Vec::new();

    if read_only_dir.exists() {
        for entry in fs::read_dir(&read_only_dir)? {
            let entry = entry?;
            let file_name = entry.file_name().into_string().map_err(|_| "Failed to read file name")?;
            files_in_dir.push(FileProperty{
                source_path: entry.path().as_path().to_str().unwrap().to_string(),
                target_path: format!("/{}", file_name),
                permissions: Permissions::ReadOnly,
            })
        }
    }else{

        return Err(Box::from("'read-only' folder not found in directory"));

    }

    let read_write_dir = current_dir.join("read-write");
    if read_write_dir.exists() {

        if read_write_dir.exists(){

            for entry in fs::read_dir(&read_write_dir)? {

                let entry = entry?;
                let file_name = entry.file_name().into_string().map_err(|_| "Failed to read file name")?;
                files_in_dir.push(FileProperty{
                    source_path: format!("./read-write/{}", file_name),
                    target_path: format!("/{}", file_name),
                    permissions: Permissions::ReadWrite,
                })
            }
        }else{
            return Err(Box::from("'read-write' folder not found in directory"));
        }

    }


    let files = Files{
        files: files_in_dir,
    };
    let properties = serde_json::to_value(files).expect("Failed to serialize Files");
    let component = Component{
        name: name.to_string(),
        component_type: "wasm".to_string(),
        properties,
        traits: vec![]
    };
    application.spec.components.push(component);
    match fs::write(&source, application.to_yaml_string()){
        Ok(result) => {
            Ok(result)
        }
        Err(error) => {
            Err(Box::new(error))
        }
    }
}


fn scan_directory(current_dir: PathBuf) -> Result<(), Box<dyn Error>>{
    //     it will scan the directory of component , it should find read-only and read-only folder
    //     else error shall be thrown
    let read_only_dir = current_dir.join("read-only");
    let read_write_dir = current_dir.join("read-write");

    if !read_only_dir.exists() {
        error!("'read-only' folder not found in directory: {:?}", current_dir.display());
        return Err(Box::from("'read-only' folder not found in directory"))
    }

    if !read_write_dir.exists() {
        error!("'read-write' folder not found in directory: {:?}", current_dir.display());
        return Err(Box::from("'read-write' folder not found"))
    }

    info!("Both 'read-only' and 'read-write' folders exists in the directory: {:?}", current_dir.display());
    Ok(())

}

fn find_main_source() -> Option<PathBuf>{
    let mut current_dir = std::env::current_dir().expect("Failed to get current dir");
    let mut last_source = None;

    loop{
        let file = current_dir.join(DEFAULT_CONFIG_FILE_NAME);
        if current_dir.join(DEFAULT_CONFIG_FILE_NAME).exists(){
            last_source = Some(file);
        }

        match current_dir.parent() {
            None => {break;}
            Some(parent_dir) => {
                current_dir = parent_dir.to_path_buf();
            }
        }
    }

    last_source
}
