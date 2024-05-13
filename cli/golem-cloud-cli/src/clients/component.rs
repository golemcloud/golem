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

use std::io::Read;

use async_trait::async_trait;
use golem_cloud_client::model::{
    Component, ComponentQuery, Export, ExportFunction, ExportInstance, FunctionParameter,
    FunctionResult, NameOptionTypePair, NameTypePair, ResourceMode, Type, TypeEnum, TypeFlags,
    TypeRecord, TypeTuple, TypeVariant,
};
use serde::Serialize;
use tokio::fs::File;
use tracing::info;

use crate::model::{ComponentName, GolemError, PathBufOrStdin};
use crate::{ProjectId, RawComponentId};

#[async_trait]
pub trait ComponentClient {
    async fn get_metadata(
        &self,
        component_id: &RawComponentId,
        version: u64,
    ) -> Result<ComponentView, GolemError>;
    async fn get_latest_metadata(
        &self,
        component_id: &RawComponentId,
    ) -> Result<ComponentView, GolemError>;
    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<ComponentName>,
    ) -> Result<Vec<ComponentView>, GolemError>;
    async fn add(
        &self,
        project_id: Option<ProjectId>,
        name: ComponentName,
        file: PathBufOrStdin,
    ) -> Result<ComponentView, GolemError>;
    async fn update(
        &self,
        id: RawComponentId,
        file: PathBufOrStdin,
    ) -> Result<ComponentView, GolemError>;
}

#[derive(Clone)]
pub struct ComponentClientLive<C: golem_cloud_client::api::ComponentClient + Sync + Send> {
    pub client: C,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentView {
    pub component_id: String,
    pub component_version: u64,
    pub component_name: String,
    pub component_size: u64,
    pub exports: Vec<String>,
}

impl From<&Component> for ComponentView {
    fn from(value: &Component) -> Self {
        ComponentView {
            component_id: value.versioned_component_id.component_id.to_string(),
            component_version: value.versioned_component_id.version,
            component_name: value.component_name.to_string(),
            component_size: value.component_size,
            exports: value
                .metadata
                .exports
                .iter()
                .flat_map(|exp| match exp {
                    Export::Instance(ExportInstance { name, functions }) => {
                        let fs: Vec<String> = functions
                            .iter()
                            .map(|f| {
                                show_exported_function(
                                    &format!("{name}/"),
                                    &f.name,
                                    &f.parameters,
                                    &f.results,
                                )
                            })
                            .collect();
                        fs
                    }
                    Export::Function(ExportFunction {
                        name,
                        parameters,
                        results,
                    }) => {
                        vec![show_exported_function("", name, parameters, results)]
                    }
                })
                .collect(),
        }
    }
}

fn render_type(typ: &Type) -> String {
    match typ {
        Type::Variant(TypeVariant { cases }) => {
            let cases_str = cases
                .iter()
                .map(|NameOptionTypePair { name, typ }| {
                    format!(
                        "{name}: {}",
                        typ.clone()
                            .map(|typ| render_type(&typ))
                            .unwrap_or("()".to_string())
                    )
                })
                .collect::<Vec<String>>()
                .join(", ");
            format!("variant({cases_str})")
        }
        Type::Result(boxed) => format!(
            "result({}, {})",
            boxed
                .ok
                .clone()
                .map_or("()".to_string(), |typ| render_type(&typ)),
            boxed
                .err
                .clone()
                .map_or("()".to_string(), |typ| render_type(&typ))
        ),
        Type::Option(boxed) => format!("{}?", render_type(&boxed.inner)),
        Type::Enum(TypeEnum { cases }) => format!("enum({})", cases.join(", ")),
        Type::Flags(TypeFlags { cases }) => format!("flags({})", cases.join(", ")),
        Type::Record(TypeRecord { cases }) => {
            let pairs: Vec<String> = cases
                .iter()
                .map(|NameTypePair { name, typ }| format!("{name}: {}", render_type(typ)))
                .collect();

            format!("{{{}}}", pairs.join(", "))
        }
        Type::Tuple(TypeTuple { items }) => {
            let typs: Vec<String> = items.iter().map(render_type).collect();
            format!("({})", typs.join(", "))
        }
        Type::List(boxed) => format!("[{}]", render_type(&boxed.inner)),
        Type::Str { .. } => "str".to_string(),
        Type::Chr { .. } => "chr".to_string(),
        Type::F64 { .. } => "f64".to_string(),
        Type::F32 { .. } => "f32".to_string(),
        Type::U64 { .. } => "u64".to_string(),
        Type::S64 { .. } => "s64".to_string(),
        Type::U32 { .. } => "u32".to_string(),
        Type::S32 { .. } => "s32".to_string(),
        Type::U16 { .. } => "u16".to_string(),
        Type::S16 { .. } => "s16".to_string(),
        Type::U8 { .. } => "u8".to_string(),
        Type::S8 { .. } => "s8".to_string(),
        Type::Bool { .. } => "bool".to_string(),
        Type::Handle(handle) => match handle.mode {
            ResourceMode::Borrowed => format!("&handle<{}>", handle.resource_id),
            ResourceMode::Owned => format!("handle<{}>", handle.resource_id),
        },
    }
}

fn render_result(r: &FunctionResult) -> String {
    match &r.name {
        None => render_type(&r.typ),
        Some(name) => format!("{name}: {}", render_type(&r.typ)),
    }
}

fn show_exported_function(
    prefix: &str,
    name: &str,
    parameters: &[FunctionParameter],
    results: &[FunctionResult],
) -> String {
    let params = parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.typ)))
        .collect::<Vec<String>>()
        .join(", ");
    let res_str = results
        .iter()
        .map(render_result)
        .collect::<Vec<String>>()
        .join(", ");
    format!("{prefix}{name}({params}) => {res_str}")
}

#[async_trait]
impl<C: golem_cloud_client::api::ComponentClient + Sync + Send> ComponentClient
    for ComponentClientLive<C>
{
    async fn get_metadata(
        &self,
        component_id: &RawComponentId,
        version: u64,
    ) -> Result<ComponentView, GolemError> {
        info!("Getting component version");
        let component = self
            .client
            .get_component_metadata(&component_id.0, &version.to_string())
            .await?;
        Ok((&component).into())
    }

    async fn get_latest_metadata(
        &self,
        component_id: &RawComponentId,
    ) -> Result<ComponentView, GolemError> {
        info!("Getting latest component version");

        let component = self
            .client
            .get_latest_component_metadata(&component_id.0)
            .await?;
        Ok((&component).into())
    }

    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<ComponentName>,
    ) -> Result<Vec<ComponentView>, GolemError> {
        info!("Getting components");

        let project_id = project_id.map(|p| p.0);
        let name = name.map(|n| n.0);

        let components: Vec<Component> = self
            .client
            .get_components(project_id.as_ref(), name.as_deref())
            .await?;
        let views = components.iter().map(|c| c.into()).collect();
        Ok(views)
    }

    async fn add(
        &self,
        project_id: Option<ProjectId>,
        name: ComponentName,
        path: PathBufOrStdin,
    ) -> Result<ComponentView, GolemError> {
        info!("Adding component {name:?} from {path:?}");

        let query = ComponentQuery {
            project_id: project_id.map(|ProjectId(id)| id),
            component_name: name.0,
        };

        let component = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client.create_component(&query, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.create_component(&query, bytes).await?
            }
        };

        Ok((&component).into())
    }

    async fn update(
        &self,
        id: RawComponentId,
        path: PathBufOrStdin,
    ) -> Result<ComponentView, GolemError> {
        info!("Updating component {id:?} from {path:?}");

        let component = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client.update_component(&id.0, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.update_component(&id.0, bytes).await?
            }
        };

        Ok((&component).into())
    }
}
