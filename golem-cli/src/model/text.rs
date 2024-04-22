use crate::model::component::ComponentView;
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::{ExampleDescription, InvocationKey};
use cli_table::{format::Justify, print_stdout, Table, WithTitle};
use golem_client::model::{
    HttpApiDefinition, Route, WorkerId, WorkerMetadata, WorkersMetadataResponse,
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
pub struct ApiDefinitionGetRes(pub HttpApiDefinition);

impl TextFormat for ApiDefinitionGetRes {
    fn print(&self) {
        print_api_definition(&self.0, "")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionAddRes(pub HttpApiDefinition);

#[derive(Table)]
struct RouteView {
    #[table(title = "Method")]
    pub method: String,
    #[table(title = "Path")]
    pub path: String,
    #[table(title = "Component", justify = "Justify::Right")]
    pub component: String,
    #[table(title = "Worker")]
    pub worker_id: String,
    #[table(title = "Function")]
    pub function_name: String,
}

impl From<&Route> for RouteView {
    fn from(value: &Route) -> Self {
        let component_str = value.binding.component.to_string();
        let component_end = &component_str[component_str.len() - 7..];
        RouteView {
            method: value.method.to_string(),
            path: value.path.to_string(),
            component: format!("*{component_end}"),
            worker_id: value.binding.worker_id.to_string(),
            function_name: value.binding.function_name.to_string(),
        }
    }
}

fn print_api_definition(def: &HttpApiDefinition, action: &str) {
    printdoc!(
        "
            API Definition {action}with ID {} and version {}.
            Routes:
            ",
        def.id,
        def.version
    );

    print_stdout(
        def.routes
            .iter()
            .map(RouteView::from)
            .collect::<Vec<_>>()
            .with_title(),
    )
    .unwrap()
}

impl TextFormat for ApiDefinitionAddRes {
    fn print(&self) {
        print_api_definition(&self.0, "created ");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionUpdateRes(pub HttpApiDefinition);

impl TextFormat for ApiDefinitionUpdateRes {
    fn print(&self) {
        print_api_definition(&self.0, "updated ");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionImportRes(pub HttpApiDefinition);

impl TextFormat for ApiDefinitionImportRes {
    fn print(&self) {
        print_api_definition(&self.0, "imported ");
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
pub struct ComponentAddView(pub ComponentView);

impl TextFormat for ComponentAddView {
    fn print(&self) {
        printdoc!(
            "
            New component created with ID {}, version {}, and size of {} bytes.
            Component name: {}.
            Exports:
            ",
            self.0.component_id,
            self.0.component_version,
            self.0.component_size,
            self.0.component_name
        );

        for export in &self.0.exports {
            println!("\t{export}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentUpdateView(pub ComponentView);

impl TextFormat for ComponentUpdateView {
    fn print(&self) {
        printdoc!(
            "
            Updated component with ID {}. New version: {}. Component size is {} bytes.
            Component name: {}.
            Exports:
            ",
            self.0.component_id,
            self.0.component_version,
            self.0.component_size,
            self.0.component_name
        );

        for export in &self.0.exports {
            println!("\t{export}")
        }
    }
}

#[derive(Table)]
struct ComponentListView {
    #[table(title = "ID")]
    pub component_id: String,
    #[table(title = "Name")]
    pub component_name: String,
    #[table(title = "Version", justify = "Justify::Right")]
    pub component_version: u64,
    #[table(title = "Size", justify = "Justify::Right")]
    pub component_size: u64,
    #[table(title = "Exports count", justify = "Justify::Right")]
    pub n_exports: usize,
}

impl From<&ComponentView> for ComponentListView {
    fn from(value: &ComponentView) -> Self {
        Self {
            component_id: value.component_id.to_string(),
            component_name: value.component_name.to_string(),
            component_version: value.component_version,
            component_size: value.component_size,
            n_exports: value.exports.len(),
        }
    }
}

impl TextFormat for Vec<ComponentView> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(ComponentListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAddView(pub WorkerId);

impl TextFormat for WorkerAddView {
    fn print(&self) {
        printdoc!(
            "
            New worker created for component {}, with name {}.
            ",
            self.0.component_id,
            self.0.worker_name,
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
            Worker "{}" of component {} with component version {}.
            Status: {}.
            Startup arguments: {}.
            Environment variables: {}.
            Retry count: {}.
            "#,
            self.worker_id.worker_name,
            self.worker_id.component_id,
            self.component_version,
            self.status.to_string(),
            self.args.join(", "),
            self.env.iter().map(|(k, v)| format!("{k}={v}")).join(", "),
            self.retry_count,
        )
    }
}

#[derive(Table)]
struct WorkerMetadataView {
    #[table(title = "Component")]
    pub component_id: Uuid,
    #[table(title = "Name")]
    pub worker_name: String,
    #[table(title = "Status", justify = "Justify::Right")]
    pub status: String,
    #[table(title = "Component version", justify = "Justify::Right")]
    pub component_version: u64,
}

impl From<&WorkerMetadata> for WorkerMetadataView {
    fn from(value: &WorkerMetadata) -> Self {
        Self {
            component_id: value.worker_id.component_id,
            worker_name: value.worker_id.worker_name.to_string(),
            status: value.status.to_string(),
            component_version: value.component_version,
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
