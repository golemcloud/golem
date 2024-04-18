use crate::model::invoke_result_view::InvokeResultView;
use crate::model::template::TemplateView;
use crate::model::{ExampleDescription, InvocationKey};
use cli_table::{format::Justify, print_stdout, Table, WithTitle};
use golem_client::model::{
    HttpApiDefinition, Route, VersionedWorkerId, WorkerMetadata, WorkersMetadataResponse,
};
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier};
use indoc::{eprintdoc, printdoc};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use uuid::Uuid;

pub trait TextFormat {
    fn print(&self);
}

#[derive(Table)]
struct ApiDefinitionDetailedView {
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Version")]
    pub version: String,
    #[table(title = "Routes")]
    pub routes: String,
}

impl From<&HttpApiDefinition> for ApiDefinitionDetailedView {
    fn from(value: &HttpApiDefinition) -> Self {
        Self {
            id: value.id.to_string(),
            version: value.version.to_string(),
            routes: value
                .routes
                .iter()
                .map(|r| format!("{} {}", r.method.to_string(), r.path))
                .join("\n"),
        }
    }
}

#[derive(Table)]
struct HttpApiDefinitionView {
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Version")]
    pub version: String,
    #[table(title = "Routes count", justify = "Justify::Right")]
    pub n_routes: usize,
}

impl From<&HttpApiDefinition> for HttpApiDefinitionView {
    fn from(value: &HttpApiDefinition) -> Self {
        Self {
            id: value.id.to_string(),
            version: value.version.to_string(),
            n_routes: value.routes.len(),
        }
    }
}

impl TextFormat for Vec<HttpApiDefinition> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(HttpApiDefinitionView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionGetRes(pub Vec<HttpApiDefinition>);

impl TextFormat for ApiDefinitionGetRes {
    fn print(&self) {
        print_stdout(
            self.0
                .iter()
                .map(ApiDefinitionDetailedView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionPostRes(pub HttpApiDefinition);

#[derive(Table)]
struct RouteView {
    #[table(title = "Method")]
    pub method: String,
    #[table(title = "Path")]
    pub path: String,
    #[table(title = "Template")]
    pub template: Uuid,
    #[table(title = "Worker")]
    pub worker_id: String,
    #[table(title = "Function")]
    pub function_name: String,
}

impl From<&Route> for RouteView {
    fn from(value: &Route) -> Self {
        RouteView {
            method: value.method.to_string(),
            path: value.path.to_string(),
            template: value.binding.template,
            worker_id: value.binding.worker_id.to_string(),
            function_name: value.binding.function_name.to_string(),
        }
    }
}

impl TextFormat for ApiDefinitionPostRes {
    fn print(&self) {
        printdoc!(
            "
            New API Definition created with ID {} and version {}.
            Routes:
            ",
            self.0.id,
            self.0.version
        );

        print_stdout(
            self.0
                .routes
                .iter()
                .map(RouteView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Table)]
pub struct ExampleDescriptionView {
    #[table(title = "Name")]
    pub name: ExampleName,
    #[table(title = "Language")]
    pub language: GuestLanguage,
    #[table(title = "Tier")]
    pub tier: GuestLanguageTier,
    #[table(title = "Description")]
    pub description: String,
}

impl From<&ExampleDescription> for ExampleDescriptionView {
    fn from(value: &ExampleDescription) -> Self {
        Self {
            name: value.name.clone(),
            language: value.language.clone(),
            tier: value.tier.clone(),
            description: textwrap::wrap(&value.description, 20).join("\n"),
        }
    }
}

impl TextFormat for Vec<ExampleDescription> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(ExampleDescriptionView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateAddView(pub TemplateView);

impl TextFormat for TemplateAddView {
    fn print(&self) {
        printdoc!(
            "
            New template created with ID {}, version {}, and size of {} bytes.
            Template name: {}.
            Exports:
            ",
            self.0.template_id,
            self.0.template_version,
            self.0.template_size,
            self.0.template_name
        );

        for export in &self.0.exports {
            println!("\t{export}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateUpdateView(pub TemplateView);

impl TextFormat for TemplateUpdateView {
    fn print(&self) {
        printdoc!(
            "
            Updated template with ID {}. New version: {}. Template size is {} bytes.
            Template name: {}.
            Exports:
            ",
            self.0.template_id,
            self.0.template_version,
            self.0.template_size,
            self.0.template_name
        );

        for export in &self.0.exports {
            println!("\t{export}")
        }
    }
}

#[derive(Table)]
struct TemplateListView {
    #[table(title = "ID")]
    pub template_id: String,
    #[table(title = "Name")]
    pub template_name: String,
    #[table(title = "Version", justify = "Justify::Right")]
    pub template_version: u64,
    #[table(title = "Size", justify = "Justify::Right")]
    pub template_size: i32,
    #[table(title = "Exports count", justify = "Justify::Right")]
    pub n_exports: usize,
}

impl From<&TemplateView> for TemplateListView {
    fn from(value: &TemplateView) -> Self {
        Self {
            template_id: value.template_id.to_string(),
            template_name: value.template_name.to_string(),
            template_version: value.template_version,
            template_size: value.template_size,
            n_exports: value.exports.len(),
        }
    }
}

impl TextFormat for Vec<TemplateView> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(TemplateListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAddView(pub VersionedWorkerId);

impl TextFormat for WorkerAddView {
    fn print(&self) {
        printdoc!(
            "
            New worker created for template {}, with name {}, using template version {}.
            ",
            self.0.worker_id.template_id,
            self.0.worker_id.worker_name,
            self.0.template_version_used,
        )
    }
}

impl TextFormat for InvocationKey {
    fn print(&self) {
        printdoc!(
            "
            Invocation key: {}
            You can use it in invoke-and-await command this way:
            invoke-and-await --invocation-key {} ...
            ",
            self.0,
            self.0
        )
    }
}

impl TextFormat for InvokeResultView {
    fn print(&self) {
        match self {
            InvokeResultView::Wave(wave) => {
                if wave.is_empty() {
                    println!("Empty result.")
                } else {
                    println!("Invocation results in WAVE format:");
                    println!("{}", serde_yaml::to_string(wave).unwrap());
                }
            }
            InvokeResultView::Json(json) => {
                eprintdoc!(
                    "
                    Failed to convert invocation result to WAVE format.
                    At the moment WAVE does not support Handle (aka Resource) data type.

                    Use -vvv flags to get detailed logs.
                    "
                );

                println!("Invocation result in JSON format:");
                println!("{}", serde_json::to_string_pretty(json).unwrap());
            }
        }
    }
}

impl TextFormat for WorkerMetadata {
    fn print(&self) {
        printdoc!(
            r#"
            Worker "{}" of template {} with template version {}.
            Status: {}.
            Startup arguments: {}.
            Environment variables: {}.
            Retry count: {}.
            "#,
            self.worker_id.worker_name,
            self.worker_id.template_id,
            self.template_version,
            self.status.to_string(),
            self.args.join(", "),
            self.env.iter().map(|(k, v)| format!("{k}={v}")).join(", "),
            self.retry_count,
        )
    }
}

#[derive(Table)]
struct WorkerMetadataView {
    #[table(title = "Template")]
    pub template_id: Uuid,
    #[table(title = "Name")]
    pub worker_name: String,
    #[table(title = "Status", justify = "Justify::Right")]
    pub status: String,
    #[table(title = "Template version", justify = "Justify::Right")]
    pub template_version: u64,
}

impl From<&WorkerMetadata> for WorkerMetadataView {
    fn from(value: &WorkerMetadata) -> Self {
        Self {
            template_id: value.worker_id.template_id,
            worker_name: value.worker_id.worker_name.to_string(),
            status: value.status.to_string(),
            template_version: value.template_version,
        }
    }
}

impl TextFormat for WorkersMetadataResponse {
    fn print(&self) {
        print_stdout(
            self.workers
                .iter()
                .map(WorkerMetadataView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap();

        if let Some(cursor) = self.cursor {
            printdoc!(
                "
                There are more workers to display.
                To fetch next page use cursor {cursor} this way:
                worker list --cursor {cursor} ...
                "
            )
        }
    }
}
