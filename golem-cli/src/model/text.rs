use crate::model::component::ComponentView;
use crate::model::deploy::TryUpdateAllWorkersResult;
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::{
    ApiDeployment, ExampleDescription, IdempotencyKey, WorkerMetadataView,
    WorkersMetadataResponseView,
};
use cli_table::{format::Justify, print_stdout, Table, WithTitle};
use golem_client::model::{HttpApiDefinition, Route, ScanCursor};
use golem_common::model::ComponentId;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier};
use indoc::{eprintdoc, printdoc};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

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
    #[table(title = "Component URN", justify = "Justify::Right")]
    pub component_urn: String,
    #[table(title = "Worker Name")]
    pub worker_name: String,
}

impl From<&Route> for RouteView {
    fn from(value: &Route) -> Self {
        let component_urn = ComponentUrn {
            id: ComponentId(value.binding.component_id),
        };
        let component_str = component_urn.to_string();
        let component_end = &component_str[component_str.len() - 12..];
        RouteView {
            method: value.method.to_string(),
            path: value.path.to_string(),
            component_urn: format!("*{component_end}"),
            worker_name: value.binding.worker_name.to_string(),
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
            New component created with URN {}, version {}, and size of {} bytes.
            Component name: {}.
            Exports:
            ",
            self.0.component_urn,
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
            Updated component with URN {}. New version: {}. Component size is {} bytes.
            Component name: {}.
            Exports:
            ",
            self.0.component_urn,
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
pub struct ComponentGetView(pub ComponentView);

impl TextFormat for ComponentGetView {
    fn print(&self) {
        printdoc!(
            "
            Component with URN {}. Version: {}. Component size is {} bytes.
            Component name: {}.
            Exports:
            ",
            self.0.component_urn,
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
    #[table(title = "URN")]
    pub component_urn: String,
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
            component_urn: value.component_urn.to_string(),
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
pub struct WorkerAddView(pub WorkerUrn);

impl TextFormat for WorkerAddView {
    fn print(&self) {
        let component_urn = ComponentUrn {
            id: self.0.id.component_id.clone(),
        };

        printdoc!(
            "
            New worker created for component {component_urn}, with name {}.
            Worker URN: {}
            ",
            self.0.id.worker_name,
            self.0,
        )
    }
}

impl TextFormat for IdempotencyKey {
    fn print(&self) {
        printdoc!(
            "
            Invocation key: {}
            You can use it in invoke-and-await command this way:
            invoke-and-await --idempotency-key {} ...
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

impl TextFormat for WorkerMetadataView {
    fn print(&self) {
        printdoc!(
            r#"
            Worker "{}" of component {} with component version {}.
            URN: {}.
            Status: {}.
            Startup arguments: {}.
            Environment variables: {}.
            Retry count: {}.
            "#,
            self.worker_urn.id.worker_name,
            ComponentUrn {
                id: self.worker_urn.id.component_id.clone()
            },
            self.component_version,
            self.worker_urn,
            self.status.to_string(),
            self.args.join(", "),
            self.env.iter().map(|(k, v)| format!("{k}={v}")).join(", "),
            self.retry_count,
        )
    }
}

#[derive(Table)]
struct WorkerMetadataListView {
    #[table(title = "Component")]
    pub component_urn: ComponentUrn,
    #[table(title = "Name")]
    pub worker_name: String,
    #[table(title = "Status", justify = "Justify::Right")]
    pub status: String,
    #[table(title = "Component version", justify = "Justify::Right")]
    pub component_version: u64,
}

impl From<&WorkerMetadataView> for WorkerMetadataListView {
    fn from(value: &WorkerMetadataView) -> Self {
        Self {
            component_urn: ComponentUrn {
                id: value.worker_urn.id.component_id.clone(),
            },
            worker_name: value.worker_urn.id.worker_name.to_string(),
            status: value.status.to_string(),
            component_version: value.component_version,
        }
    }
}

impl TextFormat for WorkersMetadataResponseView {
    fn print(&self) {
        print_stdout(
            self.workers
                .iter()
                .map(WorkerMetadataListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap();

        if let Some(cursor) = &self.cursor {
            let layer = cursor.layer;
            let cursor = cursor.cursor;
            printdoc!(
                "
                There are more workers to display.
                To fetch next page use cursor {layer}/{cursor} this way:
                worker list --cursor {layer}/{cursor} ...
                "
            )
        }
    }
}

impl TextFormat for ScanCursor {
    fn print(&self) {
        let layer = self.layer;
        let cursor = self.cursor;
        printdoc!("{layer}/{cursor}")
    }
}

impl TextFormat for ApiDeployment {
    fn print(&self) {
        for api_defs in &self.api_definitions {
            printdoc!(
                "
                API deployment on {} with definition {}/{}
                ",
                match &self.site.subdomain {
                    Some(subdomain) => format!("{}.{}", subdomain, self.site.host),
                    None => self.site.host.to_string(),
                },
                api_defs.id,
                api_defs.version,
            );
        }
    }
}

#[derive(Table)]
struct ApiDeploymentView {
    #[table(title = "Site")]
    pub site: String,
    #[table(title = "Definition ID")]
    pub id: String,
    #[table(title = "Version")]
    pub version: String,
}

impl TextFormat for Vec<ApiDeployment> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .flat_map(|deployment| {
                    deployment
                        .api_definitions
                        .iter()
                        .map(|def| ApiDeploymentView {
                            site: match &deployment.site.subdomain {
                                Some(subdomain) => {
                                    format!("{}.{}", subdomain, deployment.site.host)
                                }
                                None => deployment.site.host.to_string(),
                            },
                            id: def.id.to_string(),
                            version: def.version.to_string(),
                        })
                })
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Table)]
struct WorkerUrnTableEntry {
    #[table(title = "Worker URN")]
    pub worker_urn: WorkerUrn,

    #[table(title = "Name")]
    pub worker_name: String,
}

impl TextFormat for TryUpdateAllWorkersResult {
    fn print(&self) {
        if !self.triggered.is_empty() {
            println!("Triggered update for the following workers:");
            print_stdout(
                self.triggered
                    .iter()
                    .map(|worker_urn| WorkerUrnTableEntry {
                        worker_urn: worker_urn.clone(),
                        worker_name: worker_urn.id.worker_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .with_title(),
            )
            .unwrap();
        }

        if !self.failed.is_empty() {
            println!("Failed to trigger update for the following workers:");
            print_stdout(
                self.failed
                    .iter()
                    .map(|worker_urn| WorkerUrnTableEntry {
                        worker_urn: worker_urn.clone(),
                        worker_name: worker_urn.id.worker_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .with_title(),
            )
            .unwrap();
        }
    }
}
