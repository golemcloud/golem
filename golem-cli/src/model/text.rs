use crate::model::component::ComponentView;
use crate::model::deploy::TryUpdateAllWorkersResult;
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::{
    ApiDeployment, ExampleDescription, IdempotencyKey, WorkerMetadataView,
    WorkersMetadataResponseView,
};
use chrono::{DateTime, Utc};
use cli_table::{format::Justify, print_stdout, Table, WithTitle};
use colored::Colorize;
use golem_client::model::{
    HttpApiDefinitionWithTypeInfo, RouteWithTypeInfo, ScanCursor, WorkerStatus,
};
use golem_common::model::ComponentId;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier};
use indoc::{eprintdoc, printdoc};
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub trait TextFormat {
    fn print(&self);
}

pub trait MessageWithFieldsTextFormat {
    fn message(&self) -> String;
    fn fields(&self) -> Vec<(&'static str, String)>;
}

impl<T: MessageWithFieldsTextFormat> TextFormat for T {
    fn print(&self) {
        println!("{}\n", self.message());

        let fields = self.fields();
        let max_field_len = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;

        for (name, value) in self.fields() {
            let lines: Vec<_> = value.lines().collect();
            if lines.len() == 1 {
                println!("{: <max_field_len$} {}", format!("{}:", name), lines[0]);
            } else {
                println!("{}:", name);
                for line in lines {
                    println!("  {}", line)
                }
            }
        }
    }
}

pub struct FieldsBuilder(Vec<(&'static str, String)>);

impl FieldsBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(vec![])
    }

    pub fn field<T: ToString>(&mut self, name: &'static str, value: &T) -> &mut Self {
        self.0.push((name, value.to_string()));
        self
    }

    pub fn fmt_field<T>(
        &mut self,
        name: &'static str,
        value: &T,
        format: impl Fn(&T) -> String,
    ) -> &mut Self {
        self.0.push((name, format(value)));
        self
    }

    pub fn fmt_field_optional<T>(
        &mut self,
        name: &'static str,
        value: &T,
        cond: bool,
        format: impl Fn(&T) -> String,
    ) -> &mut Self {
        if cond {
            self.0.push((name, format(value)));
        }
        self
    }

    pub fn fmt_field_option<T>(
        &mut self,
        name: &'static str,
        value: &Option<T>,
        format: impl Fn(&T) -> String,
    ) -> &mut Self {
        if let Some(value) = &value {
            self.0.push((name, format(value)));
        }
        self
    }

    pub fn build(self) -> Vec<(&'static str, String)> {
        self.0
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

impl From<&HttpApiDefinitionWithTypeInfo> for HttpApiDefinitionView {
    fn from(value: &HttpApiDefinitionWithTypeInfo) -> Self {
        Self {
            id: value.id.to_string(),
            version: value.version.to_string(),
            n_routes: value.routes.len(),
        }
    }
}

impl TextFormat for Vec<HttpApiDefinitionWithTypeInfo> {
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
pub struct ApiDefinitionGetRes(pub HttpApiDefinitionWithTypeInfo);

impl TextFormat for ApiDefinitionGetRes {
    fn print(&self) {
        print_api_definition(&self.0, "")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionAddRes(pub HttpApiDefinitionWithTypeInfo);

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

impl From<&RouteWithTypeInfo> for RouteView {
    fn from(value: &RouteWithTypeInfo) -> Self {
        let component_urn = ComponentUrn {
            id: ComponentId(value.binding.component_id.component_id),
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

fn print_api_definition(def: &HttpApiDefinitionWithTypeInfo, action: &str) {
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
pub struct ApiDefinitionUpdateRes(pub HttpApiDefinitionWithTypeInfo);

impl TextFormat for ApiDefinitionUpdateRes {
    fn print(&self) {
        print_api_definition(&self.0, "updated ");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionImportRes(pub HttpApiDefinitionWithTypeInfo);

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

impl MessageWithFieldsTextFormat for ComponentGetView {
    fn message(&self) -> String {
        format!("Metadata for component: {}", self.0.component_name.bold())
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Component URN", &self.0.component_urn, format_main_id)
            .fmt_field("Component name", &self.0.component_name, format_id)
            .fmt_field("Component version", &self.0.component_version, format_id)
            .fmt_field_option("Project ID", &self.0.project_id, format_id)
            .fmt_field("Component size", &self.0.component_size, format_binary_size)
            .fmt_field_option("Created at", &self.0.created_at, |d| d.to_string())
            .fmt_field("Exports", &self.0.exports, |e| format_exports(e.as_slice()));

        fields.build()
    }
}

/*impl TextFormat for ComponentGetView {
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
}*/

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
            New worker created for component {}, with name {}.
            Worker URN: {}
            ",
            format_id(&component_urn),
            format_id(&self.0.id.worker_name),
            format_main_id(&self.0),
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

impl MessageWithFieldsTextFormat for WorkerMetadataView {
    fn message(&self) -> String {
        format!(
            "Metadata for worker: {}",
            self.worker_urn.id.worker_name.bold()
        )
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Worker URN", &self.worker_urn, format_main_id)
            .fmt_field("Component URN", &self.worker_urn.id.component_id, |id| {
                format_id(&ComponentUrn { id: id.clone() })
            })
            .fmt_field("Worker name", &self.worker_urn.id.worker_name, format_id)
            .fmt_field("Component version", &self.component_version, format_id)
            .field("Created at", &self.created_at)
            .fmt_field("Component size", &self.component_size, format_binary_size)
            .fmt_field(
                "Total linear memory size",
                &self.total_linear_memory_size,
                format_binary_size,
            )
            .fmt_field_optional("Arguments", &self.args, !self.args.is_empty(), |args| {
                args.join(" ")
            })
            .fmt_field_optional(
                "Environment variables",
                &self.env,
                !self.env.is_empty(),
                |env| {
                    env.iter()
                        .map(|(k, v)| format!("{}={}", k, v.bold()))
                        .join(";")
                },
            )
            .fmt_field("Status", &self.status, format_status)
            .fmt_field("Retry count", &self.retry_count, format_retry_count)
            .fmt_field_optional(
                "Pending invocation count",
                &self.pending_invocation_count,
                self.pending_invocation_count > 0,
                |n| n.to_string(),
            )
            .fmt_field_option("Last error", &self.last_error, |err| {
                format_stack(err.as_ref())
            });

        fields.build()
    }
}

#[derive(Table)]
struct WorkerMetadataListView {
    #[table(title = "Component URN")]
    pub component_urn: ComponentUrn,
    #[table(title = "Name")]
    pub worker_name: String,
    #[table(title = "Component\nversion", justify = "Justify::Right")]
    pub component_version: u64,
    #[table(title = "Status", justify = "Justify::Right")]
    pub status: String,
    #[table(title = "Created at")]
    pub created_at: DateTime<Utc>,
}

impl From<&WorkerMetadataView> for WorkerMetadataListView {
    fn from(value: &WorkerMetadataView) -> Self {
        Self {
            component_urn: ComponentUrn {
                id: value.worker_urn.id.component_id.clone(),
            },
            worker_name: value.worker_urn.id.worker_name.to_string(),
            status: format_status(&value.status),
            component_version: value.component_version,
            created_at: value.created_at,
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

pub fn format_main_id<T: ToString>(id: &T) -> String {
    id.to_string().bold().underline().to_string()
}

pub fn format_id<T: ToString>(id: &T) -> String {
    id.to_string().bold().to_string()
}

pub fn format_warn<T: ToString>(s: &T) -> String {
    s.to_string().yellow().to_string()
}

pub fn format_stack(stack: &str) -> String {
    stack
        .lines()
        .map(|line| {
            if line.contains("<unknown>!<wasm function") {
                line.bright_black().to_string()
            } else {
                line.yellow().to_string()
            }
        })
        .join("\n")
}

pub fn format_error(error: &str) -> String {
    if error.starts_with("error while executing at wasm backtrace") {
        format_stack(error)
    } else {
        error.yellow().to_string()
    }
}

pub fn format_binary_size(size: &u64) -> String {
    humansize::format_size(*size, humansize::BINARY)
}

pub fn format_status(status: &WorkerStatus) -> String {
    let status_name = status.to_string();
    match status {
        WorkerStatus::Running => status_name.green(),
        WorkerStatus::Idle => status_name.cyan(),
        WorkerStatus::Suspended => status_name.yellow(),
        WorkerStatus::Interrupted => status_name.red(),
        WorkerStatus::Retrying => status_name.yellow(),
        WorkerStatus::Failed => status_name.bright_red(),
        WorkerStatus::Exited => status_name.white(),
    }
    .to_string()
}

pub fn format_retry_count(retry_count: &u64) -> String {
    if *retry_count == 0 {
        retry_count.to_string()
    } else {
        format_warn(&retry_count.to_string())
    }
}

static BUILTIN_TYPES: phf::Set<&'static str> = phf::phf_set! {
    "bool",
    "s8", "s16", "s32", "s64",
    "u8", "u16", "u32", "u64",
    "f32", "f64",
    "char",
    "string",
    "list",
    "option",
    "result",
    "tuple"
};

// A very naive highlighter
pub fn format_export(export: &str) -> String {
    let separator =
        Regex::new(r"[ :/.{}()<>]").expect("Failed to compile export separator pattern");
    let mut formatted = String::with_capacity(export.len());

    fn format_token(target: &mut String, token: &str) {
        let trimmed_token = token.trim_ascii_start();
        let starts_with_ascii = trimmed_token
            .chars()
            .next()
            .map(|c| c.is_ascii())
            .unwrap_or(false);
        if starts_with_ascii {
            if BUILTIN_TYPES.contains(trimmed_token) {
                target.push_str(&token.green().to_string());
            } else {
                target.push_str(&token.cyan().to_string());
            }
        } else {
            target.push_str(token);
        }
    }

    let mut last_end = 0;
    for separator in separator.find_iter(export) {
        if separator.start() != last_end {
            format_token(&mut formatted, &export[last_end..separator.start()]);
        }
        formatted.push_str(separator.as_str());
        last_end = separator.end();
    }
    if last_end != export.len() {
        format_token(&mut formatted, &export[last_end..])
    }

    formatted
}

pub fn format_exports(exports: &[String]) -> String {
    exports.iter().map(|e| format_export(e.as_str())).join("\n")
}
