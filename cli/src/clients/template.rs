use std::io::Read;

use async_trait::async_trait;
use golem_client::model::{
    Export, FunctionParameter, FunctionResult, Template, TemplateQuery, Type,
};
use serde::Serialize;
use tokio::fs::File;
use tracing::info;

use crate::clients::CloudAuthentication;
use crate::model::{GolemError, PathBufOrStdin, TemplateName};
use crate::{ProjectId, RawTemplateId};

#[async_trait]
pub trait TemplateClient {
    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<TemplateName>,
        auth: &CloudAuthentication,
    ) -> Result<Vec<TemplateView>, GolemError>;
    async fn add(
        &self,
        project_id: Option<ProjectId>,
        name: TemplateName,
        file: PathBufOrStdin,
        auth: &CloudAuthentication,
    ) -> Result<TemplateView, GolemError>;
    async fn update(
        &self,
        id: RawTemplateId,
        file: PathBufOrStdin,
        auth: &CloudAuthentication,
    ) -> Result<TemplateView, GolemError>;
}

#[derive(Clone)]
pub struct TemplateClientLive<C: golem_client::template::Template + Sync + Send> {
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
            template_id: value.versioned_template_id.raw_template_id.to_string(),
            template_version: value.versioned_template_id.version,
            template_name: value.template_name.value.to_string(),
            template_size: value.template_size,
            exports: value
                .metadata
                .exports
                .iter()
                .flat_map(|exp| match exp {
                    Export::Instance { name, functions } => {
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
                    Export::Function {
                        name,
                        parameters,
                        results,
                    } => {
                        vec![show_exported_function("", name, parameters, results)]
                    }
                })
                .collect(),
        }
    }
}

fn render_type(tpe: &Type) -> String {
    match tpe {
        Type::Variant(cases) => {
            let cases_str = cases
                .iter()
                .map(|(name, tpe)| {
                    format!(
                        "{name}: {}",
                        tpe.clone()
                            .map(|tpe| render_type(&tpe))
                            .unwrap_or("()".to_string())
                    )
                })
                .collect::<Vec<String>>()
                .join(", ");
            format!("variant({cases_str})")
        }
        Type::Result((ok, err)) => format!(
            "result({}, {})",
            ok.clone().map_or("()".to_string(), |tpe| render_type(&tpe)),
            err.clone()
                .map_or("()".to_string(), |tpe| render_type(&tpe))
        ),
        Type::Option(elem) => format!("{}?", render_type(elem)),
        Type::Enum(names) => format!("enum({})", names.join(", ")),
        Type::Flags(names) => format!("flags({})", names.join(", ")),
        Type::Record(fields) => {
            let pairs: Vec<String> = fields
                .iter()
                .map(|(name, tpe)| format!("{name}: {}", render_type(tpe)))
                .collect();

            format!("{{{}}}", pairs.join(", "))
        }
        Type::Tuple(elems) => {
            let tpes: Vec<String> = elems.iter().map(|tpe| render_type(tpe)).collect();
            format!("({})", tpes.join(", "))
        }
        Type::List(elem) => format!("[{}]", render_type(elem)),
        Type::Str {} => "str".to_string(),
        Type::Chr {} => "chr".to_string(),
        Type::F64 {} => "f64".to_string(),
        Type::F32 {} => "f32".to_string(),
        Type::U64 {} => "u64".to_string(),
        Type::S64 {} => "s64".to_string(),
        Type::U32 {} => "u32".to_string(),
        Type::S32 {} => "s32".to_string(),
        Type::U16 {} => "u16".to_string(),
        Type::S16 {} => "s16".to_string(),
        Type::U8 {} => "u8".to_string(),
        Type::S8 {} => "s8".to_string(),
        Type::Bool {} => "bool".to_string(),
    }
}

fn render_result(r: &FunctionResult) -> String {
    match &r.name {
        None => render_type(&r.tpe),
        Some(name) => format!("{name}: {}", render_type(&r.tpe)),
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
        .map(|p| format!("{}: {}", p.name, render_type(&p.tpe)))
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
impl<C: golem_client::template::Template + Sync + Send> TemplateClient for TemplateClientLive<C> {
    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<TemplateName>,
        auth: &CloudAuthentication,
    ) -> Result<Vec<TemplateView>, GolemError> {
        info!("Getting templates");

        let templates = self
            .client
            .get_all_templates(
                project_id.map(|ProjectId(id)| id.to_string()).as_deref(),
                name.map(|TemplateName(s)| s).as_deref(),
                &auth.header(),
            )
            .await?;

        let views = templates.iter().map(|c| c.into()).collect();
        Ok(views)
    }

    async fn add(
        &self,
        project_id: Option<ProjectId>,
        name: TemplateName,
        path: PathBufOrStdin,
        auth: &CloudAuthentication,
    ) -> Result<TemplateView, GolemError> {
        info!("Adding template {name:?} from {path:?}");

        let template_name = golem_client::model::TemplateName { value: name.0 };
        let query = TemplateQuery {
            project_id: project_id.map(|ProjectId(id)| id),
            component_name: template_name,
        };

        let template = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                self.client
                    .upload_template(query, file, &auth.header())
                    .await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .upload_template(query, bytes, &auth.header())
                    .await?
            }
        };

        Ok((&template).into())
    }

    async fn update(
        &self,
        id: RawTemplateId,
        path: PathBufOrStdin,
        auth: &CloudAuthentication,
    ) -> Result<TemplateView, GolemError> {
        info!("Updating template {id:?} from {path:?}");

        let template = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;
                self.client
                    .update_template(&id.0.to_string(), file, &auth.header())
                    .await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .update_template(&id.0.to_string(), bytes, &auth.header())
                    .await?
            }
        };

        Ok((&template).into())
    }
}
