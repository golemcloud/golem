// Copyright 2024-2025 Golem Cloud
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

pub mod fmt {
    use cli_table::{print_stdout, Row, Title, WithTitle};
    use colored::control::SHOULD_COLORIZE;
    use colored::Colorize;
    use golem_client::model::WorkerStatus;
    use itertools::Itertools;
    use regex::Regex;

    pub trait TextFormat {
        fn print(&self);
    }

    pub trait TableWrapper: Sized {
        type Table: TextFormat;
        fn from_vec(vec: &[Self]) -> Self::Table;
    }

    impl<T: TableWrapper> TextFormat for Vec<T> {
        fn print(&self) {
            let table = T::from_vec(self);
            table.print();
        }
    }

    pub trait MessageWithFields {
        fn message(&self) -> String;
        fn fields(&self) -> Vec<(String, String)>;
    }

    impl<T: MessageWithFields> TextFormat for T {
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

    pub struct FieldsBuilder(Vec<(String, String)>);

    impl FieldsBuilder {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Self(vec![])
        }

        pub fn field<T: ToString>(&mut self, name: &str, value: &T) -> &mut Self {
            self.0.push((name.to_string(), value.to_string()));
            self
        }

        pub fn fmt_field<T: ?Sized>(
            &mut self,
            name: &str,
            value: &T,
            format: impl Fn(&T) -> String,
        ) -> &mut Self {
            self.0.push((name.to_string(), format(value)));
            self
        }

        pub fn fmt_field_optional<T: ?Sized>(
            &mut self,
            name: &str,
            value: &T,
            cond: bool,
            format: impl Fn(&T) -> String,
        ) -> &mut Self {
            if cond {
                self.0.push((name.to_string(), format(value)));
            }
            self
        }

        pub fn fmt_field_option<T>(
            &mut self,
            name: &str,
            value: &Option<T>,
            format: impl Fn(&T) -> String,
        ) -> &mut Self {
            if let Some(value) = &value {
                self.0.push((name.to_string(), format(value)));
            }
            self
        }

        pub fn build(self) -> Vec<(String, String)> {
            self.0
        }
    }

    pub fn format_main_id<T: ToString + ?Sized>(id: &T) -> String {
        id.to_string().bold().underline().to_string()
    }

    pub fn format_id<T: ToString + ?Sized>(id: &T) -> String {
        id.to_string().bold().to_string()
    }

    pub fn format_warn<T: ToString + ?Sized>(s: &T) -> String {
        s.to_string().yellow().to_string()
    }

    pub fn format_message_highlight<T: ToString + ?Sized>(s: &T) -> String {
        s.to_string().green().bold().to_string()
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
        if error.contains("error while executing at wasm backtrace") {
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

    // A very naive highlighter for basic coloring of builtin types and user defined names
    pub fn format_export(export: &str) -> String {
        if !SHOULD_COLORIZE.should_colorize() {
            return export.to_string();
        }

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

    pub fn format_table<E, R>(table: &[E]) -> String
    where
        R: Title + 'static + for<'b> From<&'b E>,
        for<'a> &'a R: Row,
    {
        let rows: Vec<R> = table.iter().map(R::from).collect();
        let rows = &rows;

        format!(
            "{}",
            rows.with_title()
                .display()
                .expect("Failed to display table")
        )
    }

    pub fn print_table<E, R>(table: &[E])
    where
        R: Title + 'static + for<'b> From<&'b E>,
        for<'a> &'a R: Row,
    {
        let rows: Vec<R> = table.iter().map(R::from).collect();
        let rows = &rows;

        print_stdout(rows.with_title()).expect("Failed to print table");
    }
}

pub mod api_security {
    use crate::model::text::fmt::*;
    use crate::model::ApiSecurityScheme;
    use cli_table::Table;
    use golem_client::model::SecuritySchemeData;
    use indoc::printdoc;

    impl TextFormat for ApiSecurityScheme {
        fn print(&self) {
            printdoc!(
                    "
                    API Security Scheme: ID: {}, scopes: {}, client ID: {}, client secret: {}, redirect URL: {}
                    ",
                    format_message_highlight(&self.scheme_identifier),
                    &self.scopes.join(", "),
                    format_message_highlight(&self.client_id),
                    format_message_highlight(&self.client_secret),
                    format_message_highlight(&self.redirect_url),
                );
        }
    }

    #[derive(Table)]
    struct ApiSecuritySchemeTableView {
        #[table(title = "ID")]
        pub id: String,
        #[table(title = "Provider")]
        pub provider: String,
        #[table(title = "Client ID")]
        pub client_id: String,
        #[table(title = "Client Secret")]
        pub client_secret: String,
        #[table(title = "Redirect URL")]
        pub redirect_url: String,
    }

    impl From<&SecuritySchemeData> for ApiSecuritySchemeTableView {
        fn from(value: &SecuritySchemeData) -> Self {
            Self {
                id: value.scheme_identifier.clone(),
                provider: value.provider_type.to_string(),
                client_id: value.client_id.clone(),
                client_secret: value.client_secret.clone(),
                redirect_url: value.redirect_url.clone(),
            }
        }
    }
}

pub mod api_definition {
    use crate::model::text::fmt::*;
    use cli_table::{format::Justify, Table};
    use golem_client::model::{HttpApiDefinitionResponseData, RouteResponseData};
    use golem_common::model::ComponentId;
    use golem_common::uri::oss::urn::ComponentUrn;
    use serde::{Deserialize, Serialize};

    #[derive(Table)]
    struct RouteTableView {
        #[table(title = "Method")]
        pub method: String,
        #[table(title = "Path")]
        pub path: String,
        #[table(title = "Component URN", justify = "Justify::Right")]
        pub component_urn: String,
        #[table(title = "Worker Name")]
        pub worker_name: String,
    }

    impl From<&RouteResponseData> for RouteTableView {
        fn from(value: &RouteResponseData) -> Self {
            Self {
                method: value.method.to_string(),
                path: value.path.to_string(),
                component_urn: value
                    .binding
                    .clone()
                    .component_id
                    .map(|id| {
                        ComponentUrn {
                            id: ComponentId(id.component_id),
                        }
                        .to_string()
                    })
                    .unwrap_or("NA".to_string())
                    .to_string(),

                worker_name: value
                    .binding
                    .worker_name
                    .clone()
                    .unwrap_or("<NA/ephemeral>".to_string()),
            }
        }
    }

    fn api_definition_fields(def: &HttpApiDefinitionResponseData) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("ID", &def.id, format_main_id)
            .fmt_field("Version", &def.version, format_main_id)
            .fmt_field_option("Created at", &def.created_at, |d| d.to_string())
            .fmt_field_optional("Draft", &def.draft, def.draft, |d| d.to_string())
            .fmt_field_optional(
                "Routes",
                def.routes.as_slice(),
                !def.routes.is_empty(),
                format_table::<_, RouteTableView>,
            );

        fields.build()
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ApiDefinitionGetView(pub HttpApiDefinitionResponseData);

    impl MessageWithFields for ApiDefinitionGetView {
        fn message(&self) -> String {
            format!(
                "Got metadata for API definition {} version {}",
                format_message_highlight(&self.0.id),
                format_message_highlight(&self.0.version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            api_definition_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ApiDefinitionAddView(pub HttpApiDefinitionResponseData);

    impl MessageWithFields for ApiDefinitionAddView {
        fn message(&self) -> String {
            format!(
                "Added API definition {} with version {}",
                format_message_highlight(&self.0.id),
                format_message_highlight(&self.0.version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            api_definition_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ApiDefinitionUpdateView(pub HttpApiDefinitionResponseData);

    impl MessageWithFields for ApiDefinitionUpdateView {
        fn message(&self) -> String {
            format!(
                "Updated API definition {} with version {}",
                format_message_highlight(&self.0.id),
                format_message_highlight(&self.0.version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            api_definition_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ApiDefinitionImportView(pub HttpApiDefinitionResponseData);

    impl MessageWithFields for ApiDefinitionImportView {
        fn message(&self) -> String {
            format!(
                "Imported API definition {} with version {}",
                format_message_highlight(&self.0.id),
                format_message_highlight(&self.0.version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            api_definition_fields(&self.0)
        }
    }

    #[derive(Table)]
    struct HttpApiDefinitionTableView {
        #[table(title = "ID")]
        pub id: String,
        #[table(title = "Version")]
        pub version: String,
        #[table(title = "Route count", justify = "Justify::Right")]
        pub route_count: usize,
    }

    impl From<&HttpApiDefinitionResponseData> for HttpApiDefinitionTableView {
        fn from(value: &HttpApiDefinitionResponseData) -> Self {
            Self {
                id: value.id.to_string(),
                version: value.version.to_string(),
                route_count: value.routes.len(),
            }
        }
    }

    impl TextFormat for Vec<HttpApiDefinitionResponseData> {
        fn print(&self) {
            print_table::<_, HttpApiDefinitionTableView>(self);
        }
    }
}

pub mod api_deployment {
    use crate::model::text::fmt::*;
    use crate::model::ApiDeployment;
    use cli_table::{print_stdout, Table, WithTitle};
    use indoc::printdoc;

    pub fn format_site(api_deployment: &ApiDeployment) -> String {
        match &api_deployment.site.subdomain {
            Some(subdomain) => format!("{}.{}", subdomain, api_deployment.site.host),
            None => api_deployment.site.host.to_string(),
        }
    }

    impl TextFormat for ApiDeployment {
        fn print(&self) {
            for api_defs in &self.api_definitions {
                printdoc!(
                    "
                    API deployment on {} with definition {}/{}
                    ",
                    format_message_highlight(&format_site(self)),
                    format_message_highlight(&api_defs.id),
                    format_message_highlight(&api_defs.version),
                );
            }
        }
    }

    #[derive(Table)]
    struct ApiDeploymentTableView {
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
                            .map(move |def| ApiDeploymentTableView {
                                site: format_site(deployment),
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
}

pub mod component {
    use crate::model::component::ComponentView;
    use crate::model::text::fmt::*;
    use cli_table::{format::Justify, print_stdout, Table, WithTitle};
    use serde::{Deserialize, Serialize};

    #[derive(Table)]
    struct ComponentTableView {
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

    impl From<&ComponentView> for ComponentTableView {
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
                    .map(ComponentTableView::from)
                    .collect::<Vec<_>>()
                    .with_title(),
            )
            .unwrap()
        }
    }

    fn component_view_fields(view: &ComponentView) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Component URN", &view.component_urn, format_main_id)
            .fmt_field("Component name", &view.component_name, format_id)
            .fmt_field("Component version", &view.component_version, format_id)
            .fmt_field_option("Project ID", &view.project_id, format_id)
            .fmt_field("Component size", &view.component_size, format_binary_size)
            .fmt_field_option("Created at", &view.created_at, |d| d.to_string())
            .fmt_field("Exports", &view.exports, |e| format_exports(e.as_slice()));

        fields.build()
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ComponentAddView(pub ComponentView);

    impl MessageWithFields for ComponentAddView {
        fn message(&self) -> String {
            format!(
                "Added new component {}",
                format_message_highlight(&self.0.component_name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            component_view_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ComponentUpdateView(pub ComponentView);

    impl MessageWithFields for ComponentUpdateView {
        fn message(&self) -> String {
            format!(
                "Updated component {} to version {}",
                format_message_highlight(&self.0.component_name),
                format_message_highlight(&self.0.component_version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            component_view_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ComponentGetView(pub ComponentView);

    impl MessageWithFields for ComponentGetView {
        fn message(&self) -> String {
            format!(
                "Got metadata for component {}",
                format_message_highlight(&self.0.component_name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            component_view_fields(&self.0)
        }
    }
}

pub mod example {
    use crate::model::text::fmt::*;
    use crate::model::ExampleDescription;
    use cli_table::Table;
    use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier};

    #[derive(Table)]
    pub struct ExampleDescriptionTableView {
        #[table(title = "Name")]
        pub name: ExampleName,
        #[table(title = "Language")]
        pub language: GuestLanguage,
        #[table(title = "Tier")]
        pub tier: GuestLanguageTier,
        #[table(title = "Description")]
        pub description: String,
    }

    impl From<&ExampleDescription> for ExampleDescriptionTableView {
        fn from(value: &ExampleDescription) -> Self {
            Self {
                name: value.name.clone(),
                language: value.language.clone(),
                tier: value.tier.clone(),
                description: textwrap::wrap(&value.description, 30).join("\n"),
            }
        }
    }

    impl TextFormat for Vec<ExampleDescription> {
        fn print(&self) {
            print_table::<_, ExampleDescriptionTableView>(self);
        }
    }
}

pub mod profile {
    use crate::command::profile::{ProfileType, ProfileView};
    use crate::config::ProfileConfig;
    use crate::model::text::fmt::*;
    use colored::Colorize;
    use itertools::Itertools;

    impl TextFormat for Vec<ProfileView> {
        fn print(&self) {
            let res = self
                .iter()
                .map(|p| {
                    if p.is_active {
                        format!(" * {}", format_id(&p.name))
                    } else {
                        format!("   {}", p.name)
                    }
                })
                .join("\n");

            println!("{}", res)
        }
    }

    impl MessageWithFields for ProfileView {
        fn message(&self) -> String {
            match self.typ {
                ProfileType::Golem => {
                    format!("Golem profile {}", format_message_highlight(&self.name))
                }
                ProfileType::GolemCloud => format!(
                    "Golem Cloud profile {}'",
                    format_message_highlight(&self.name)
                ),
            }
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field_optional("Active", &self.is_active, self.is_active, |b| {
                    b.to_string().green().to_string()
                })
                .fmt_field_optional(
                    "Allow insecure",
                    &self.allow_insecure,
                    self.allow_insecure,
                    |b| b.to_string().red().to_string(),
                )
                .field("Default output format", &self.config.default_format);

            if let Some(url) = &self.url {
                if let Some(worker_url) = &self.worker_url {
                    fields
                        .field("Component service URL", url)
                        .field("Worker service URL", worker_url);
                } else {
                    fields.field("Service URL", url);
                }
            } else {
                fields.field("Using default URLs", &true);
            }

            fields.build()
        }
    }

    impl TextFormat for ProfileConfig {
        fn print(&self) {
            println!(
                "Default output format: {}",
                format_message_highlight(&self.default_format)
            )
        }
    }
}

pub mod worker {
    use crate::model::deploy::TryUpdateAllWorkersResult;
    use crate::model::invoke_result_view::InvokeResultView;
    use crate::model::text::fmt::*;
    use crate::model::{
        IdempotencyKey, WorkerMetadata, WorkerMetadataView, WorkersMetadataResponseView,
    };
    use base64::prelude::BASE64_STANDARD;
    use base64::Engine;
    use chrono::{DateTime, Utc};
    use cli_table::{format::Justify, Table};
    use colored::Colorize;
    use golem_client::model::PublicOplogEntry;
    use golem_common::model::public_oplog::{
        PluginInstallationDescription, PublicUpdateDescription, PublicWorkerInvocation,
    };
    use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::{print_type_annotated_value, ValueAndType};
    use indoc::{formatdoc, indoc, printdoc};
    use itertools::Itertools;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorkerAddView(pub WorkerUrn);

    impl MessageWithFields for WorkerAddView {
        fn message(&self) -> String {
            if let Some(worker_name) = &self.0.id.worker_name {
                format!("Added worker {}", format_message_highlight(&worker_name))
            } else {
                format!(
                    "Added worker with a {}",
                    format_message_highlight("random generated name")
                )
            }
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Worker URN", &self.0, format_main_id)
                .fmt_field("Component URN", &self.0.id.component_id, |id| {
                    format_id(&ComponentUrn { id: id.clone() })
                })
                .fmt_field_option("Worker name", &(self.0.id.worker_name.as_ref()), format_id);

            fields.build()
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorkerGetView(pub WorkerMetadataView);

    impl From<WorkerMetadata> for WorkerGetView {
        fn from(value: WorkerMetadata) -> Self {
            WorkerMetadataView::from(value).into()
        }
    }

    impl From<WorkerMetadataView> for WorkerGetView {
        fn from(value: WorkerMetadataView) -> Self {
            Self(value)
        }
    }

    impl MessageWithFields for WorkerGetView {
        fn message(&self) -> String {
            if let Some(worker_name) = &self.0.worker_urn.id.worker_name {
                format!(
                    "Got metadata for worker {}",
                    format_message_highlight(worker_name)
                )
            } else {
                "Got metadata for worker".to_string()
            }
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Worker URN", &self.0.worker_urn, format_main_id)
                .fmt_field("Component URN", &self.0.worker_urn.id.component_id, |id| {
                    format_id(&ComponentUrn { id: id.clone() })
                })
                .fmt_field_option("Worker name", &self.0.worker_urn.id.worker_name, format_id)
                .fmt_field("Component version", &self.0.component_version, format_id)
                .field("Created at", &self.0.created_at)
                .fmt_field("Component size", &self.0.component_size, format_binary_size)
                .fmt_field(
                    "Total linear memory size",
                    &self.0.total_linear_memory_size,
                    format_binary_size,
                )
                .fmt_field_optional("Arguments", &self.0.args, !self.0.args.is_empty(), |args| {
                    args.join(" ")
                })
                .fmt_field_optional(
                    "Environment variables",
                    &self.0.env,
                    !self.0.env.is_empty(),
                    |env| {
                        env.iter()
                            .map(|(k, v)| format!("{}={}", k, v.bold()))
                            .join(";")
                    },
                )
                .fmt_field("Status", &self.0.status, format_status)
                .fmt_field("Retry count", &self.0.retry_count, format_retry_count)
                .fmt_field_optional(
                    "Pending invocation count",
                    &self.0.pending_invocation_count,
                    self.0.pending_invocation_count > 0,
                    |n| n.to_string(),
                )
                .fmt_field_option("Last error", &self.0.last_error, |err| {
                    format_stack(err.as_ref())
                });

            fields.build()
        }
    }

    #[derive(Table)]
    struct WorkerMetadataTableView {
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

    impl From<&WorkerMetadataView> for WorkerMetadataTableView {
        fn from(value: &WorkerMetadataView) -> Self {
            Self {
                component_urn: ComponentUrn {
                    id: value.worker_urn.id.component_id.clone(),
                },
                worker_name: value.worker_urn.id.worker_name.clone().unwrap_or_default(),
                status: format_status(&value.status),
                component_version: value.component_version,
                created_at: value.created_at,
            }
        }
    }

    impl TextFormat for WorkersMetadataResponseView {
        fn print(&self) {
            print_table::<_, WorkerMetadataTableView>(&self.workers);

            if let Some(cursor) = &self.cursor {
                let layer = cursor.layer;
                let cursor = cursor.cursor;

                println!(
                    "{}",
                    formatdoc!(
                        "

                        There are more workers to display.
                        To fetch next page use cursor {layer}/{cursor} this way:
                        worker list --cursor {layer}/{cursor} ...
                        "
                    )
                    .yellow()
                );
            }
        }
    }

    impl TextFormat for IdempotencyKey {
        fn print(&self) {
            printdoc!(
                "
                Idempotency key: {}

                You can use it in invoke-and-await command this way:
                {}

                ",
                format_main_id(&self.0),
                format!("invoke-and-await --idempotency-key {} ...", self.0).cyan()
            )
        }
    }

    #[derive(Table)]
    struct WorkerUrnTableView {
        #[table(title = "Worker URN")]
        pub worker_urn: WorkerUrn,

        #[table(title = "Name")]
        pub worker_name: String,
    }

    impl From<&WorkerUrn> for WorkerUrnTableView {
        fn from(value: &WorkerUrn) -> Self {
            WorkerUrnTableView {
                worker_urn: value.clone(),
                worker_name: value.id.worker_name.clone().unwrap_or_default(),
            }
        }
    }

    impl TextFormat for TryUpdateAllWorkersResult {
        fn print(&self) {
            if !self.triggered.is_empty() {
                println!("Triggered update for the following workers:");
                print_table::<_, WorkerUrnTableView>(&self.triggered);
            }

            if !self.failed.is_empty() {
                println!(
                    "{}",
                    format_warn("Failed to trigger update for the following workers:")
                );
                print_table::<_, WorkerUrnTableView>(&self.failed);
            }
        }
    }

    impl TextFormat for InvokeResultView {
        fn print(&self) {
            fn print_results_format(format: &str) {
                println!(
                    "Invocation results in {} format:",
                    format_message_highlight(format)
                )
            }

            match self {
                InvokeResultView::Wave(wave) => {
                    if wave.is_empty() {
                        println!("Empty result.")
                    } else {
                        print_results_format("WAVE");
                        println!("{}", serde_yaml::to_string(wave).unwrap());
                    }
                }
                InvokeResultView::Json(json) => {
                    eprintln!(
                        "{}",
                        format_warn(indoc!(
                            "
                            Failed to convert invocation result to WAVE format.
                            At the moment WAVE does not support Handle (aka Resource) data type.

                            Use -vvv flags to get detailed logs.

                            "
                        ))
                    );

                    print_results_format("JSON");
                    println!("{}", serde_json::to_string_pretty(json).unwrap());
                }
            }
        }
    }

    impl TextFormat for Vec<(u64, PublicOplogEntry)> {
        fn print(&self) {
            for (idx, entry) in self {
                print!("{}: ", format_main_id(&format!("#{idx:0>5}")));
                entry.print()
            }
        }
    }

    impl TextFormat for PublicOplogEntry {
        fn print(&self) {
            let pad = "          ";
            match self {
                PublicOplogEntry::Create(params) => {
                    println!("{}", format_message_highlight("CREATE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}component version: {}",
                        format_id(&params.component_version)
                    );
                    println!(
                        "{pad}args:              {}",
                        format_id(&params.args.join(", "))
                    );
                    println!("{pad}env:");
                    for (k, v) in &params.env {
                        println!("{pad}  - {}: {}", k, format_id(&v));
                    }
                    if let Some(parent) = params.parent.as_ref() {
                        println!("{pad}parent:            {}", format_id(parent));
                    }
                    println!("{pad}initial active plugins:");
                    for plugin in &params.initial_active_plugins {
                        println!(
                            "{pad}  - installation id: {}",
                            format_id(&plugin.installation_id)
                        );
                        let inner_pad = format!("{pad}    ");
                        print_plugin_description(&inner_pad, plugin);
                    }
                }
                PublicOplogEntry::ImportedFunctionInvoked(params) => {
                    println!(
                        "{} {}",
                        format_message_highlight("CALL"),
                        format_id(&params.function_name)
                    );
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}input:             {}", print_value(&params.request));
                    println!("{pad}result:            {}", print_value(&params.response));
                }
                PublicOplogEntry::ExportedFunctionInvoked(params) => {
                    println!(
                        "{} {}",
                        format_message_highlight("INVOKE"),
                        format_id(&params.function_name)
                    );
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}idempotency key:   {}",
                        format_id(&params.idempotency_key)
                    );
                    println!("{pad}input:");
                    for param in &params.request {
                        println!("{pad}  - {}", print_value(param));
                    }
                }
                PublicOplogEntry::ExportedFunctionCompleted(params) => {
                    println!("{}", format_message_highlight("INVOKE COMPLETED"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}consumed fuel:     {}",
                        format_id(&params.consumed_fuel)
                    );
                    println!("{pad}result:            {}", print_value(&params.response));
                }
                PublicOplogEntry::Suspend(params) => {
                    println!("{}", format_message_highlight("SUSPEND"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::Error(params) => {
                    println!("{}", format_message_highlight("ERROR"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}error:             {}", format_error(&params.error));
                }
                PublicOplogEntry::NoOp(params) => {
                    println!("{}", format_message_highlight("NOP"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::Jump(params) => {
                    println!("{}", format_message_highlight("JUMP"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}from:              {}", format_id(&params.jump.start));
                    println!("{pad}to:                {}", format_id(&params.jump.end));
                }
                PublicOplogEntry::Interrupted(params) => {
                    println!("{}", format_message_highlight("INTERRUPTED"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::Exited(params) => {
                    println!("{}", format_message_highlight("EXITED"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::ChangeRetryPolicy(params) => {
                    println!("{}", format_message_highlight("CHANGE RETRY POLICY"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}max attempts:      {}",
                        format_id(&params.new_policy.max_attempts)
                    );
                    println!(
                        "{pad}min delay:         {} ms",
                        format_id(&params.new_policy.min_delay.as_millis())
                    );
                    println!(
                        "{pad}max delay:         {} ms",
                        format_id(&params.new_policy.max_delay.as_millis())
                    );
                    println!(
                        "{pad}multiplier:        {}",
                        format_id(&params.new_policy.multiplier)
                    );
                    println!(
                        "{pad}max jitter factor: {}",
                        format_id(
                            &params
                                .new_policy
                                .max_jitter_factor
                                .map(|x| x.to_string())
                                .unwrap_or("-".to_string())
                        )
                    );
                }
                PublicOplogEntry::BeginAtomicRegion(params) => {
                    println!("{}", format_message_highlight("BEGIN ATOMIC REGION"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::EndAtomicRegion(params) => {
                    println!("{}", format_message_highlight("END ATOMIC REGION"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}begin index:       {}", format_id(&params.begin_index));
                }
                PublicOplogEntry::BeginRemoteWrite(params) => {
                    println!("{}", format_message_highlight("BEGIN REMOTE WRITE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::EndRemoteWrite(params) => {
                    println!("{}", format_message_highlight("END REMOTE WRITE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}begin index:       {}", format_id(&params.begin_index));
                }
                PublicOplogEntry::PendingWorkerInvocation(params) => match &params.invocation {
                    PublicWorkerInvocation::ExportedFunction(inner_params) => {
                        println!(
                            "{} {}",
                            format_message_highlight("ENQUEUED INVOCATION"),
                            format_id(&inner_params.full_function_name)
                        );
                        println!("{pad}at:                {}", format_id(&params.timestamp));
                        println!(
                            "{pad}idempotency key:   {}",
                            format_id(&inner_params.idempotency_key)
                        );
                        if let Some(input) = &inner_params.function_input {
                            println!("{pad}input:");
                            for param in input {
                                println!("{pad}  - {}", print_value(param));
                            }
                        }
                    }
                    PublicWorkerInvocation::ManualUpdate(inner_params) => {
                        println!("{}", format_message_highlight("ENQUEUED MANUAL UPDATE"));
                        println!("{pad}at:                {}", format_id(&params.timestamp));
                        println!(
                            "{pad}target version: {}",
                            format_id(&inner_params.target_version)
                        );
                    }
                },
                PublicOplogEntry::PendingUpdate(params) => {
                    println!("{}", format_message_highlight("ENQUEUED UPDATE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}target version:    {}",
                        format_id(&params.target_version)
                    );
                    match &params.description {
                        PublicUpdateDescription::Automatic(_) => {
                            println!("{pad}type:              {}", format_id("automatic"));
                        }
                        PublicUpdateDescription::SnapshotBased(inner_params) => {
                            println!("{pad}type:              {}", format_id("snapshot based"));
                            println!(
                                "{pad}snapshot:          {}",
                                BASE64_STANDARD.encode(&inner_params.payload)
                            );
                        }
                    }
                }
                PublicOplogEntry::SuccessfulUpdate(params) => {
                    println!("{}", format_message_highlight("SUCCESSFUL UPDATE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}target version:    {}",
                        format_id(&params.target_version)
                    );
                    println!("{pad}new active plugins:");
                    for plugin in &params.new_active_plugins {
                        println!(
                            "{pad}  - installation id: {}",
                            format_id(&plugin.installation_id)
                        );
                        let inner_pad = format!("{pad}    ");
                        print_plugin_description(&inner_pad, plugin);
                    }
                }
                PublicOplogEntry::FailedUpdate(params) => {
                    println!("{}", format_message_highlight("FAILED UPDATE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}target version:    {}",
                        format_id(&params.target_version)
                    );
                    if let Some(details) = &params.details {
                        println!("{pad}error:             {}", format_error(details));
                    }
                }
                PublicOplogEntry::GrowMemory(params) => {
                    println!("{}", format_message_highlight("GROW MEMORY"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}increase:          {}",
                        format_id(&format_binary_size(&params.delta))
                    );
                }
                PublicOplogEntry::CreateResource(params) => {
                    println!("{}", format_message_highlight("CREATE RESOURCE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}resource id:       {}", format_id(&params.id));
                }
                PublicOplogEntry::DropResource(params) => {
                    println!("{}", format_message_highlight("DROP RESOURCE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}resource id:       {}", format_id(&params.id));
                }
                PublicOplogEntry::DescribeResource(params) => {
                    println!("{}", format_message_highlight("DESCRIBE RESOURCE"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!("{pad}resource id:       {}", format_id(&params.id));
                    println!(
                        "{pad}resource name:     {}",
                        format_id(&params.resource_name)
                    );
                    println!("{pad}resource parameters:");
                    for value in &params.resource_params {
                        println!("{pad}  - {}", print_value(value));
                    }
                }
                PublicOplogEntry::Log(params) => {
                    println!("{}", format_message_highlight("LOG"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}level:             {}",
                        format_id(&format!("{:?}", params.level))
                    );
                    println!("{pad}message:           {}", params.message);
                }
                PublicOplogEntry::Restart(params) => {
                    println!("{}", format_message_highlight("RESTART"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                }
                PublicOplogEntry::ActivatePlugin(params) => {
                    println!("{}", format_message_highlight("ACTIVATE PLUGIN"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}installation id:   {}",
                        format_id(&params.plugin.installation_id)
                    );
                    print_plugin_description(pad, &params.plugin);
                }
                PublicOplogEntry::DeactivatePlugin(params) => {
                    println!("{}", format_message_highlight("DEACTIVATE PLUGIN"));
                    println!("{pad}at:                {}", format_id(&params.timestamp));
                    println!(
                        "{pad}installation id:   {}",
                        format_id(&params.plugin.installation_id)
                    );
                    print_plugin_description(pad, &params.plugin);
                }
            }
        }
    }

    fn print_plugin_description(pad: &str, value: &PluginInstallationDescription) {
        println!("{pad}plugin name:       {}", format_id(&value.plugin_name));
        println!(
            "{pad}plugin version:    {}",
            format_id(&value.plugin_version)
        );
        println!(
            "{pad}plugin parameters:    {}",
            format_id(&value.plugin_version)
        );
        for (k, v) in &value.parameters {
            println!("{pad}  - {}: {}", k, format_id(&v));
        }
    }

    fn print_value(value: &ValueAndType) -> String {
        let tav: TypeAnnotatedValue = value.try_into().expect("Failed to convert value to string");
        print_type_annotated_value(&tav).expect("Failed to convert value to string")
    }
}

pub mod plugin {
    use crate::model::text::fmt::{
        format_id, format_main_id, format_message_highlight, FieldsBuilder, MessageWithFields,
        TableWrapper, TextFormat,
    };
    use cli_table::{print_stdout, Table, WithTitle};
    use golem_client::model::{
        DefaultPluginScope, PluginDefinitionDefaultPluginOwnerDefaultPluginScope,
        PluginInstallation, PluginTypeSpecificDefinition,
    };
    use itertools::Itertools;

    #[derive(Table)]
    struct PluginDefinitionTableView {
        #[table(title = "Plugin name")]
        pub name: String,
        #[table(title = "Plugin version")]
        pub version: String,
        #[table(title = "Description")]
        pub description: String,
        #[table(title = "Homepage")]
        pub homepage: String,
        #[table(title = "Type")]
        pub typ: String,
        #[table(title = "Scope")]
        pub scope: String,
    }

    impl From<&PluginDefinitionDefaultPluginOwnerDefaultPluginScope> for PluginDefinitionTableView {
        fn from(value: &PluginDefinitionDefaultPluginOwnerDefaultPluginScope) -> Self {
            Self {
                name: value.name.clone(),
                version: value.version.clone(),
                description: value.description.clone(),
                homepage: value.homepage.clone(),
                typ: match &value.specs {
                    PluginTypeSpecificDefinition::ComponentTransformer(_) => {
                        "Component Transformer".to_string()
                    }
                    PluginTypeSpecificDefinition::OplogProcessor(_) => {
                        "Oplog Processor".to_string()
                    }
                },
                scope: match &value.scope {
                    DefaultPluginScope::Global(_) => "Global".to_string(),
                    DefaultPluginScope::Component(component_scope) => {
                        format!("Component {}", component_scope.component_id)
                    }
                },
            }
        }
    }

    pub struct PluginDefinitionTable(Vec<PluginDefinitionDefaultPluginOwnerDefaultPluginScope>);

    impl TableWrapper for PluginDefinitionDefaultPluginOwnerDefaultPluginScope {
        type Table = PluginDefinitionTable;

        fn from_vec(vec: &[Self]) -> Self::Table {
            PluginDefinitionTable(vec.to_vec())
        }
    }

    impl TextFormat for PluginDefinitionTable {
        fn print(&self) {
            print_stdout(
                self.0
                    .iter()
                    .map(PluginDefinitionTableView::from)
                    .collect::<Vec<_>>()
                    .with_title(),
            )
            .unwrap()
        }
    }

    impl MessageWithFields for PluginDefinitionDefaultPluginOwnerDefaultPluginScope {
        fn message(&self) -> String {
            format!(
                "Got metadata for plugin {} version {}",
                format_message_highlight(&self.name),
                format_message_highlight(&self.version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Name", &self.name, format_main_id)
                .fmt_field("Version", &self.version, format_main_id)
                .fmt_field("Description", &self.description, format_id)
                .fmt_field("Homepage", &self.homepage, format_id)
                .fmt_field("Scope", &self.scope, format_id);

            match &self.specs {
                PluginTypeSpecificDefinition::ComponentTransformer(specs) => {
                    fields.fmt_field("Type", &"Component Transformer".to_string(), format_id);
                    fields.fmt_field("Validate URL", &specs.validate_url, format_id);
                    fields.fmt_field("Transform URL", &specs.transform_url, format_id);
                }
                PluginTypeSpecificDefinition::OplogProcessor(specs) => {
                    fields.fmt_field("Type", &"Oplog Processor".to_string(), format_id);
                    fields.fmt_field("Component ID", &specs.component_id, format_id);
                    fields.fmt_field("Component Version", &specs.component_version, format_id);
                }
            }

            fields.build()
        }
    }

    impl MessageWithFields for PluginInstallation {
        fn message(&self) -> String {
            format!(
                "Installed plugin {} version {}",
                format_message_highlight(&self.name),
                format_message_highlight(&self.version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("ID", &self.id, format_main_id)
                .fmt_field("Plugin name", &self.version, format_id)
                .fmt_field("Plugin version", &self.version, format_id)
                .fmt_field("Priority", &self.priority, format_id);

            for (k, v) in &self.parameters {
                fields.fmt_field(k, v, format_id);
            }

            fields.build()
        }
    }

    #[derive(Table)]
    struct PluginInstallationTableView {
        #[table(title = "Installation ID")]
        pub id: String,
        #[table(title = "Plugin name")]
        pub name: String,
        #[table(title = "Plugin version")]
        pub version: String,
        #[table(title = "Priority")]
        pub priority: String,
        #[table(title = "Parameters")]
        pub parameters: String,
    }

    impl From<&PluginInstallation> for PluginInstallationTableView {
        fn from(value: &PluginInstallation) -> Self {
            Self {
                id: value.id.to_string(),
                name: value.name.clone(),
                version: value.version.clone(),
                priority: value.priority.to_string(),
                parameters: value
                    .parameters
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .join(", "),
            }
        }
    }

    impl TextFormat for Vec<PluginInstallation> {
        fn print(&self) {
            print_stdout(
                self.iter()
                    .map(PluginInstallationTableView::from)
                    .collect::<Vec<_>>()
                    .with_title(),
            )
            .unwrap()
        }
    }
}
