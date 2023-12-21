use std::io::Read;

use async_trait::async_trait;
use golem_client::apis::configuration::Configuration;
use golem_client::models::TemplateQuery;
use reqwest::Url;
use serde::Serialize;
use tokio::fs::File;
use tracing::info;
use uuid::Uuid;

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
pub struct TemplateClientLive {
    pub configuration: Configuration,
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
        Type::Result(TypeResult { ok, err }) => format!(
            "result({}, {})",
            ok.clone().map_or("()".to_string(), |typ| render_type(&typ)),
            err.clone()
                .map_or("()".to_string(), |typ| render_type(&typ))
        ),
        Type::Option(TypeOption { inner }) => format!("{}?", render_type(&inner)),
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
        Type::List(TypeList { inner }) => format!("[{}]", render_type(inner)),
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
impl TemplateClient for TemplateClientLive {
    async fn find(
        &self,
        project_id: Option<ProjectId>,
        name: Option<TemplateName>,
    ) -> Result<Vec<TemplateView>, GolemError> {
        info!("Getting templates");

        let url = format!("{}/v2/templates", &self.configuration.base_path);

        let mut request = self
            .configuration
            .client
            .request(reqwest::Method::GET, url.as_str());

        if let Some(project_id) = project_id {
            request = request.query(&[("project-id", &project_id.0.to_string())])
        }

        if let Some(name) = name {
            request = request.query(&[("template-name", &name.0)])
        }

        if let Some(ref token) = &self.configuration.bearer_access_token {
            request = request.bearer_auth(token.to_string());
        };

        let request = request.build()?;

        let templates_resp = self.configuration.client.execute(request).await?;

        let status = templates_resp.status();
        let content = templates_resp.text().await?;

        if status.is_success() {
            let templates: Vec<Template> = serde_json::from_str(&content).map_err(|err| {
                GolemError(format!(
                    "Failed to parse response as json: {err:?}, content: {content}"
                ))
            })?;
            let views = templates.iter().map(|c| c.into()).collect();
            Ok(views)
        } else {
            Err(GolemError(format!(
                "Templates list request failed with status {status} and content {content}"
            )))
        }
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

        let url = format!("{}/v2/templates", &self.configuration.base_path);

        let mut request = self
            .configuration
            .client
            .request(reqwest::Method::POST, url.as_str());

        if let Some(ref token) = &self.configuration.bearer_access_token {
            request = request.bearer_auth(token.to_string());
        };
        let mut form = reqwest::multipart::Form::new();
        form = form.part(
            "query",
            reqwest::multipart::Part::text(
                serde_json::to_string(&query).expect("Failed to serialize TemplateQuery to json"),
            )
            .mime_str("application/json")?,
        );

        match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                form = form.part(
                    "template",
                    reqwest::multipart::Part::stream(file).mime_str("application/octet-stream")?,
                );
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                form = form.part(
                    "template",
                    reqwest::multipart::Part::stream(bytes).mime_str("application/octet-stream")?,
                );
            }
        };

        request = request.multipart(form);

        let resp = self.configuration.client.execute(request.build()?).await?;

        let status = resp.status();
        let content = resp.text().await?;

        if status.is_success() {
            let template: Template = serde_json::from_str(&content).map_err(|err| {
                GolemError(format!(
                    "Failed to parse response as json: {err:?}, content: {content}"
                ))
            })?;
            Ok((&template).into())
        } else {
            Err(GolemError(format!(
                "Templates add request failed with status {status} and content {content}"
            )))
        }
    }

    async fn update(
        &self,
        id: RawTemplateId,
        path: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError> {
        info!("Updating template {id:?} from {path:?}");

        let mut url = Url::parse(&self.configuration.base_path).unwrap();

        url.path_segments_mut()
            .unwrap()
            .push("v2")
            .push("templates")
            .push(&id.0.to_string())
            .push("upload");

        let mut request = self.configuration.client.request(reqwest::Method::PUT, url);

        if let Some(local_var_token) = &self.configuration.bearer_access_token {
            request = request.bearer_auth(local_var_token.to_owned());
        };

        match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                request = request.body(file);
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                request = request.body(bytes);
            }
        };

        request = request.header(reqwest::header::CONTENT_TYPE, "application/octet-stream");

        let resp = self.configuration.client.execute(request.build()?).await?;

        let status = resp.status();
        let content = resp.text().await?;

        if status.is_success() {
            let template: Template = serde_json::from_str(&content).map_err(|err| {
                GolemError(format!(
                    "Failed to parse response as json: {err:?}, content: {content}"
                ))
            })?;
            Ok((&template).into())
        } else {
            Err(GolemError(format!(
                "Templates update request failed with status {status} and content {content}"
            )))
        }
    }
}

// Temporary copy of data classes:

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Template {
    pub versioned_template_id: VersionedTemplateId,
    pub template_name: String,
    pub template_size: i32,
    pub metadata: TemplateMetadata,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
struct VersionedTemplateId {
    pub template_id: Uuid,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TemplateMetadata {
    pub exports: Vec<Export>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
enum Export {
    Instance(ExportInstance),
    Function(ExportFunction),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ExportInstance {
    pub name: String,
    pub functions: Vec<ExportFunction>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ExportFunction {
    pub name: String,
    pub parameters: Vec<FunctionParameter>,
    pub results: Vec<FunctionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FunctionParameter {
    pub name: String,
    pub typ: Type,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FunctionResult {
    pub name: Option<String>,
    pub typ: Type,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
enum Type {
    Variant(TypeVariant),
    Result(TypeResult),
    Option(TypeOption),
    Enum(TypeEnum),
    Flags(TypeFlags),
    Record(TypeRecord),
    Tuple(TypeTuple),
    List(TypeList),
    Str(TypeStr),
    Chr(TypeChr),
    F64(TypeF64),
    F32(TypeF32),
    U64(TypeU64),
    S64(TypeS64),
    U32(TypeU32),
    S32(TypeS32),
    U16(TypeU16),
    S16(TypeS16),
    U8(TypeU8),
    S8(TypeS8),
    Bool(TypeBool),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeVariant {
    pub cases: Vec<NameOptionTypePair>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct NameOptionTypePair {
    pub name: String,
    pub typ: Option<Box<Type>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeResult {
    ok: Option<Box<Type>>,
    err: Option<Box<Type>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeOption {
    pub inner: Box<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeEnum {
    pub cases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeFlags {
    pub cases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeRecord {
    pub cases: Vec<NameTypePair>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct NameTypePair {
    pub name: String,
    pub typ: Box<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeTuple {
    pub items: Vec<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeList {
    pub inner: Box<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeStr;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeChr;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeF64;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeF32;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeU64;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeS64;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeU32;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeS32;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeU16;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeS16;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeU8;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeS8;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TypeBool;
