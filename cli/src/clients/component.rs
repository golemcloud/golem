use std::path::PathBuf;
use tracing::info;
use async_trait::async_trait;
use golem_client::model::{Component, ComponentQuery, Export, FunctionParameter, FunctionResult, Type};
use serde::Serialize;
use tokio::fs::File;
use crate::clients::CloudAuthentication;
use crate::model::{ComponentName, GolemError};
use crate::{ProjectId, RawComponentId};


#[async_trait]
pub trait ComponentClient {
    async fn find(&self, project_id: Option<ProjectId>, name: Option<ComponentName>, auth: &CloudAuthentication) -> Result<Vec<ComponentView>, GolemError>;
    async fn add(&self, project_id: Option<ProjectId>, name: ComponentName, file: PathBuf, auth: &CloudAuthentication) -> Result<ComponentView, GolemError>;
    async fn update(&self, id: RawComponentId, file: PathBuf, auth: &CloudAuthentication) -> Result<ComponentView, GolemError>;
}

pub struct ComponentClientLive<C: golem_client::component::Component + Sync + Send> {
    pub client: C,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct ComponentView {
    pub component_id: String,
    pub component_version: i32,
    pub component_name: String,
    pub component_size: i32,
    pub exports: Vec<String>,
}

impl From<&Component> for ComponentView {
    fn from(value: &Component) -> Self {
        ComponentView {
            component_id: value.versioned_template_id.raw_template_id.to_string(),
            component_version: value.versioned_template_id.version,
            component_name: value.template_name.value.to_string(),
            component_size: value.template_size,
            exports: value.metadata.exports.iter().flat_map(|exp| match exp {
                Export::Instance { name, functions } => {
                    let fs: Vec<String> = functions.iter().map(|f| show_exported_function(&format!("{name}/"), &f.name, &f.parameters, &f.results)).collect();
                    fs
                }
                Export::Function { name, parameters, results } => {
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
                .map(|(name, tpe)| format!("{name}: {}", tpe.clone().map(|tpe| render_type(&tpe)).unwrap_or("()".to_string())))
                .collect::<Vec<String>>()
                .join(", ");
            format!("variant({cases_str})")
        }
        Type::Result((ok, err)) =>
            format!("result({}, {})", ok.clone().map_or("()".to_string(), |tpe| render_type(&tpe)), err.clone().map_or("()".to_string(), |tpe| render_type(&tpe))),
        Type::Option(elem) => format!("{}?", render_type(&elem)),
        Type::Enum(names) => format!("enum({})", names.join(", ")),
        Type::Flags(names) => format!("flags({})", names.join(", ")),
        Type::Record(fields) => {
            let pairs: Vec<String> = fields.iter().map(|(name, tpe)| format!("{name}: {}", render_type(&tpe))).collect();

            format!("{{{}}}", pairs.join(", "))
        }
        Type::Union(types) => {
            let tpes: Vec<String> = types.iter().map(|tpe| render_type(&tpe)).collect();
            format!("{{{}}}", tpes.join(" | "))
        }
        Type::Tuple(elems) => {
            let tpes: Vec<String> = elems.iter().map(|tpe| render_type(&tpe)).collect();
            format!("({})", tpes.join(", "))
        }
        Type::List(elem) =>  format!("[{}]", render_type(&elem)),
        Type::Str => "str".to_string(),
        Type::Chr => "chr".to_string(),
        Type::F64 => "f64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::S64 => "s64".to_string(),
        Type::U32 => "u32".to_string(),
        Type::S32 => "s32".to_string(),
        Type::U16 => "u16".to_string(),
        Type::S16 => "s16".to_string(),
        Type::U8 => "u8".to_string(),
        Type::S8 => "s8".to_string(),
        Type::Bool => "bool".to_string(),
    }
}

fn render_result(r: &FunctionResult) -> String {
    match &r.name {
        None => render_type(&r.tpe),
        Some(name) => format!("{name}: {}", render_type(&r.tpe)),
    }
}

fn show_exported_function(prefix: &str, name: &str, parameters: &Vec<FunctionParameter>, results: &Vec<FunctionResult>) -> String {
    let params = parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.tpe)))
        .collect::<Vec<String>>()
        .join(", ");
    let res_str = results
        .iter()
        .map(|r| render_result(r))
        .collect::<Vec<String>>()
        .join(", ");
    format!("{prefix}{name}({params}) => {res_str}")
}

#[async_trait]
impl<C: golem_client::component::Component + Sync + Send> ComponentClient for ComponentClientLive<C> {
    async fn find(&self, project_id: Option<ProjectId>, name: Option<ComponentName>, auth: &CloudAuthentication) -> Result<Vec<ComponentView>, GolemError> {
        info!("Getting component");

        let components = self.client.get_components(project_id.map(|ProjectId(id)| id.to_string()).as_deref(), name.map(|ComponentName(s)| s).as_deref(), &auth.header()).await?;

        let views = components.iter().map(|c| c.into()).collect();
        Ok(views)
    }

    async fn add(&self, project_id: Option<ProjectId>, name: ComponentName, path: PathBuf, auth: &CloudAuthentication) -> Result<ComponentView, GolemError> {
        info!("Adding component {name:?} from {path:?}");

        let file = File::open(path).await.map_err(|e| GolemError(format!("Can't open component file: {e}")))?;
        let component_name = golem_client::model::ComponentName{value: name.0};

        let component = self.client.post_component(ComponentQuery{project_id: project_id.map(|ProjectId(id)| id), component_name}, file, &auth.header()).await?;

        Ok((&component).into())
    }

    async fn update(&self, id: RawComponentId, path: PathBuf, auth: &CloudAuthentication) -> Result<ComponentView, GolemError> {
        info!("Updating component {id:?} from {path:?}");

        let file = File::open(path).await.map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

        let component = self.client.put_component(&id.0.to_string(), file, &auth.header()).await?;

        Ok((&component).into())
    }
}