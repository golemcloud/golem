use std::io::Read;

use async_trait::async_trait;
use golem_client::model::Export;
use golem_client::model::ExportFunction;
use golem_client::model::ExportInstance;
use golem_client::model::FunctionParameter;
use golem_client::model::FunctionResult;
use golem_client::model::NameOptionTypePair;
use golem_client::model::NameTypePair;
use golem_client::model::Template;
use golem_client::model::TemplateQuery;
use golem_client::model::Type;
use golem_client::model::TypeEnum;
use golem_client::model::TypeFlags;
use golem_client::model::TypeRecord;
use golem_client::model::TypeTuple;
use golem_client::model::TypeVariant;
use serde::Serialize;
use tokio::fs::File;
use tracing::info;

use crate::model::{GolemError, PathBufOrStdin, TemplateName};
use crate::{ProjectId, RawTemplateId};

#[async_trait]
pub trait TemplateClient {
    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<TemplateName>,
    ) -> Result<Vec<TemplateView>, GolemError>;
    async fn add(
        &self,
        project_id: Option<ProjectId>,
        name: TemplateName,
        file: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError>;
    async fn update(
        &self,
        id: RawTemplateId,
        file: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError>;
}

#[derive(Clone)]
pub struct TemplateClientLive<C: golem_client::api::TemplateClient + Sync + Send> {
    pub client: C,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateView {
    pub template_id: String,
    pub template_version: i32,
    pub template_name: String,
    pub template_size: i32,
    pub exports: Vec<String>,
}

impl From<&Template> for TemplateView {
    fn from(value: &Template) -> Self {
        TemplateView {
            template_id: value.versioned_template_id.template_id.to_string(),
            template_version: value.versioned_template_id.version,
            template_name: value.template_name.to_string(),
            template_size: value.template_size,
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
impl<C: golem_client::api::TemplateClient + Sync + Send> TemplateClient for TemplateClientLive<C> {
    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<TemplateName>,
    ) -> Result<Vec<TemplateView>, GolemError> {
        info!("Getting templates");

        let project_id = project_id.map(|p| p.0);
        let name = name.map(|n| n.0);

        let templates: Vec<Template> = self
            .client
            .get(project_id.as_ref(), name.as_deref())
            .await?;
        let views = templates.iter().map(|c| c.into()).collect();
        Ok(views)
    }

    async fn add(
        &self,
        project_id: Option<ProjectId>,
        name: TemplateName,
        path: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError> {
        info!("Adding template {name:?} from {path:?}");

        let query = TemplateQuery {
            project_id: project_id.map(|ProjectId(id)| id),
            template_name: name.0,
        };

        let template = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                self.client.post(&query, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.post(&query, bytes).await?
            }
        };

        Ok((&template).into())
    }

    async fn update(
        &self,
        id: RawTemplateId,
        path: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError> {
        info!("Updating template {id:?} from {path:?}");

        let template = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                self.client.template_id_upload_put(&id.0, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.template_id_upload_put(&id.0, bytes).await?
            }
        };

        Ok((&template).into())
    }
}
