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
    use crate::fuzzy::Match;
    use crate::log::{log_warn_action, logln, LogColorize, LogIndent};
    use crate::model::{Format, WorkerNameMatch};
    use cli_table::{Row, Title, WithTitle};
    use colored::control::SHOULD_COLORIZE;
    use colored::Colorize;
    use golem_client::model::{InitialComponentFile, WorkerStatus};
    use itertools::Itertools;
    use regex::Regex;
    use std::collections::BTreeMap;

    pub trait TextView {
        fn log(&self);
    }

    pub trait MessageWithFields {
        fn message(&self) -> String;
        fn fields(&self) -> Vec<(String, String)>;

        fn indent_fields() -> bool {
            false
        }

        fn nest_ident_fields() -> bool {
            false
        }

        fn format_field_name(name: String) -> String {
            name
        }
    }

    impl<T: MessageWithFields> TextView for T {
        fn log(&self) {
            logln(self.message());
            if !Self::nest_ident_fields() {
                logln("");
            }

            let fields = self.fields();
            let padding = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;

            let _indent = Self::indent_fields().then(LogIndent::new);
            let _nest_indent =
                Self::nest_ident_fields().then(|| NestedTextViewIndent::new(Format::Text));

            for (name, value) in self.fields() {
                let lines: Vec<_> = value.split("\n").collect();
                if lines.len() == 1 {
                    logln(format!(
                        "{:<padding$} {}",
                        format!("{}:", Self::format_field_name(name)),
                        lines[0]
                    ));
                } else {
                    logln(format!("{}:", Self::format_field_name(name)));
                    for line in lines {
                        logln(format!("  {}", line))
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
        "tuple",
        "record",
    };

    // A very naive highlighter for basic coloring of builtin types and user defined names
    pub fn format_export(export: &str) -> String {
        if !SHOULD_COLORIZE.should_colorize() {
            return export.to_string();
        }

        let separator =
            Regex::new(r"[ :/.{}()<>,]").expect("Failed to compile export separator pattern");
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

    pub fn format_dynamic_links(links: &BTreeMap<String, BTreeMap<String, String>>) -> String {
        links
            .iter()
            .map(|(name, link)| {
                let padding = link.keys().map(|name| name.len()).max().unwrap_or_default() + 1;

                format!(
                    "{}:\n{}",
                    name,
                    link.iter()
                        .map(|(resource, interface)| format!(
                            "  {:<padding$} {}",
                            format!("{}:", resource),
                            format_export(interface)
                        ))
                        .join("\n")
                )
            })
            .join("\n")
    }

    pub fn format_ifs_entry(files: &[InitialComponentFile]) -> String {
        files
            .iter()
            .map(|file| {
                format!(
                    "{} {} {}",
                    file.permissions.as_compact_str(),
                    file.path.as_path().as_str().log_color_highlight(),
                    file.key.0.as_str().black()
                )
            })
            .join("\n")
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

    pub fn log_table<E, R>(table: &[E])
    where
        R: Title + 'static + for<'b> From<&'b E>,
        for<'a> &'a R: Row,
    {
        logln(format_table(table));
    }

    pub fn log_text_view<View: TextView>(view: &View) {
        view.log();
    }

    pub fn log_error<S: AsRef<str>>(message: S) {
        logln(format!(
            "{} {}",
            "error:".log_color_error(),
            message.as_ref()
        ));
    }

    pub fn log_warn<S: AsRef<str>>(message: S) {
        logln(format!("{} {}", "warn:".log_color_warn(), message.as_ref()));
    }

    pub fn log_fuzzy_matches(matches: &[Match]) {
        for m in matches {
            if !m.exact_match {
                log_fuzzy_match(m);
            }
        }
    }

    pub fn log_fuzzy_match(m: &Match) {
        log_warn_action(
            "Fuzzy matched",
            format!(
                "pattern {} as {}",
                m.pattern.log_color_highlight(),
                m.option.log_color_ok_highlight()
            ),
        );
    }

    pub struct NestedTextViewIndent {
        format: Format,
        log_indent: Option<LogIndent>,
    }

    impl NestedTextViewIndent {
        pub fn new(format: Format) -> Self {
            match format {
                Format::Json | Format::Yaml => Self {
                    format,
                    log_indent: Some(LogIndent::new()),
                },
                Format::Text => {
                    logln("╔═");
                    Self {
                        format,
                        log_indent: Some(LogIndent::prefix("║ ")),
                    }
                }
            }
        }
    }

    impl Drop for NestedTextViewIndent {
        fn drop(&mut self) {
            if let Some(ident) = self.log_indent.take() {
                drop(ident);
                match self.format {
                    Format::Json | Format::Yaml => {
                        // NOP
                    }
                    Format::Text => logln("╚═"),
                }
            }
        }
    }

    pub fn format_worker_name_match(worker_name_match: &WorkerNameMatch) -> String {
        format!(
            "{}{}{}/{}",
            match &worker_name_match.account_id {
                Some(account_id) => {
                    format!("{}/", account_id.0.blue().bold())
                }
                None => "".to_string(),
            },
            match &worker_name_match.project {
                Some(project) => {
                    format!("{}/", project.project_name.0.blue().bold())
                }
                None => "".to_string(),
            },
            worker_name_match.component_name.0.blue().bold(),
            worker_name_match
                .worker_name
                .as_ref()
                .map(|wn| wn.0.as_str())
                .unwrap_or("-")
                .green()
                .bold(),
        )
    }
}

pub mod api_security {
    use crate::model::text::fmt::*;
    use crate::model::ApiSecurityScheme;
    use cli_table::Table;
    use golem_client::model::SecuritySchemeData;
    use indoc::printdoc;

    impl TextView for ApiSecurityScheme {
        fn log(&self) {
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
    use crate::model::ComponentName;
    use cli_table::{format::Justify, Table};
    use golem_client::model::{HttpApiDefinitionResponseData, RouteResponseData};

    use serde::{Deserialize, Serialize};

    #[derive(Table)]
    struct RouteTableView {
        #[table(title = "Method")]
        pub method: String,
        #[table(title = "Path")]
        pub path: String,
        #[table(title = "Component Name")]
        pub component_name: ComponentName,
    }

    impl From<&RouteResponseData> for RouteTableView {
        fn from(value: &RouteResponseData) -> Self {
            Self {
                method: value.method.to_string(),
                path: value.path.to_string(),
                component_name: value
                    .binding
                    .clone()
                    .component
                    .map(|component| component.name)
                    .unwrap_or("<NA>".to_string())
                    .into(),
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
    pub struct ApiDefinitionNewView(pub HttpApiDefinitionResponseData);

    impl MessageWithFields for ApiDefinitionNewView {
        fn message(&self) -> String {
            format!(
                "Created API definition {} with version {}",
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

    impl TextView for Vec<HttpApiDefinitionResponseData> {
        fn log(&self) {
            log_table::<_, HttpApiDefinitionTableView>(self);
        }
    }
}

pub mod api_deployment {
    use crate::model::text::fmt::*;
    use crate::model::ApiDeployment;
    use cli_table::Table;
    use golem_client::model::ApiDefinitionInfo;

    use indoc::printdoc;

    pub fn format_site(api_deployment: &ApiDeployment) -> String {
        match &api_deployment.site.subdomain {
            Some(subdomain) => format!("{}.{}", subdomain, api_deployment.site.host),
            None => api_deployment.site.host.to_string(),
        }
    }

    impl TextView for ApiDeployment {
        fn log(&self) {
            for api_defs in &self.api_definitions {
                printdoc!(
                    "
                    API {}/{} deployed at {}
                    ",
                    format_message_highlight(&api_defs.id),
                    format_message_highlight(&api_defs.version),
                    format_message_highlight(&format_site(self)),
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

    impl From<&(&ApiDeployment, &ApiDefinitionInfo)> for ApiDeploymentTableView {
        fn from(value: &(&ApiDeployment, &ApiDefinitionInfo)) -> Self {
            let (deployment, def) = value;
            ApiDeploymentTableView {
                site: format_site(deployment),
                id: def.id.to_string(),
                version: def.version.to_string(),
            }
        }
    }

    impl TextView for Vec<ApiDeployment> {
        fn log(&self) {
            log_table::<_, ApiDeploymentTableView>(
                self.iter()
                    .flat_map(|deployment| {
                        deployment
                            .api_definitions
                            .iter()
                            .map(move |def| (deployment, def))
                    })
                    .collect::<Vec<_>>()
                    .as_slice(),
            );
        }
    }
}

pub mod component {
    use crate::model::component::ComponentView;
    use crate::model::text::fmt::*;
    use crate::model::ComponentName;
    use cli_table::{format::Justify, Table};

    use serde::{Deserialize, Serialize};

    // TODO: review columns and formats
    #[derive(Table)]
    struct ComponentTableView {
        #[table(title = "Name")]
        pub component_name: ComponentName,
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
                component_name: value.component_name.clone(),
                component_version: value.component_version,
                component_size: value.component_size,
                n_exports: value.exports.len(),
            }
        }
    }

    impl TextView for Vec<ComponentView> {
        fn log(&self) {
            log_table::<_, ComponentTableView>(self.as_slice())
        }
    }

    fn component_view_fields(view: &ComponentView) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Component name", &view.component_name, format_main_id)
            .fmt_field("Component ID", &view.component_id, format_id)
            .fmt_field("Component type", &view.component_type, |t| t.to_string())
            .fmt_field("Component version", &view.component_version, format_id)
            .fmt_field_option("Project ID", &view.project_id, format_id)
            .fmt_field("Component size", &view.component_size, format_binary_size)
            .fmt_field_option("Created at", &view.created_at, |d| d.to_string())
            .fmt_field("Exports", &view.exports, |e| format_exports(e.as_slice()))
            .fmt_field_optional(
                "Dynamic WASM RPC links",
                &view.dynamic_linking,
                !view.dynamic_linking.is_empty(),
                format_dynamic_links,
            )
            .fmt_field_optional(
                "Initial file system",
                view.files.as_slice(),
                !view.files.is_empty(),
                format_ifs_entry,
            );

        fields.build()
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ComponentCreateView(pub ComponentView);

    impl MessageWithFields for ComponentCreateView {
        fn message(&self) -> String {
            format!(
                "Created new component {}",
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

        fn nest_ident_fields() -> bool {
            true
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

        fn nest_ident_fields() -> bool {
            true
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ComponentReplStartedView(pub ComponentView);

    impl MessageWithFields for ComponentReplStartedView {
        fn message(&self) -> String {
            format!(
                "Started Rib REPL for component {} using version {}",
                format_message_highlight(&self.0.component_name),
                format_message_highlight(&self.0.component_version),
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            component_view_fields(&self.0)
        }

        fn nest_ident_fields() -> bool {
            true
        }
    }
}

pub mod template {
    use crate::model::text::fmt::*;
    use crate::model::TemplateDescription;
    use cli_table::Table;
    use golem_templates::model::{GuestLanguage, GuestLanguageTier, TemplateName};

    #[derive(Table)]
    pub struct TemplateDescriptionTableView {
        #[table(title = "Name")]
        pub name: TemplateName,
        #[table(title = "Language")]
        pub language: GuestLanguage,
        #[table(title = "Tier")]
        pub tier: GuestLanguageTier,
        #[table(title = "Description")]
        pub description: String,
    }

    impl From<&TemplateDescription> for TemplateDescriptionTableView {
        fn from(value: &TemplateDescription) -> Self {
            Self {
                name: value.name.clone(),
                language: value.language,
                tier: value.tier.clone(),
                description: textwrap::wrap(&value.description, 30).join("\n"),
            }
        }
    }

    impl TextView for Vec<TemplateDescription> {
        fn log(&self) {
            log_table::<_, TemplateDescriptionTableView>(self);
        }
    }
}

pub mod profile {
    use crate::config::{ProfileConfig, ProfileKind};
    use crate::log::{logln, LogColorize};
    use crate::model::text::fmt::*;
    use crate::model::ProfileView;
    use colored::Colorize;

    impl TextView for Vec<ProfileView> {
        fn log(&self) {
            logln("Available profiles:".log_color_help_group().to_string());
            for profile in self {
                logln(format!(
                    " {} {}, {}{}",
                    if profile.is_active { "*" } else { " " },
                    format_id(&profile.name),
                    profile.kind,
                    if profile.name.is_builtin() {
                        ", builtin"
                    } else {
                        ""
                    },
                ));
            }
        }
    }

    impl MessageWithFields for ProfileView {
        fn message(&self) -> String {
            match self.kind {
                ProfileKind::Oss => {
                    format!("OSS profile {}", format_message_highlight(&self.name))
                }
                ProfileKind::Cloud => {
                    format!("Cloud profile {}'", format_message_highlight(&self.name))
                }
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

    impl TextView for ProfileConfig {
        fn log(&self) {
            logln(format!(
                "Default output format: {}",
                format_message_highlight(&self.default_format),
            ))
        }
    }
}

pub mod worker {
    use crate::log::{logln, LogColorize};
    use crate::model::deploy::TryUpdateAllWorkersResult;
    use crate::model::invoke_result_view::InvokeResultView;
    use crate::model::text::fmt::*;
    use crate::model::{
        ComponentName, IdempotencyKey, WorkerMetadata, WorkerMetadataView, WorkerName,
        WorkersMetadataResponseView,
    };
    use base64::prelude::BASE64_STANDARD;
    use base64::Engine;
    use chrono::{DateTime, Utc};
    use cli_table::{format::Justify, Table};
    use colored::Colorize;
    use golem_client::model::PublicOplogEntry;
    use golem_common::model::public_oplog::{
        PluginInstallationDescription, PublicAttributeValue, PublicUpdateDescription,
        PublicWorkerInvocation, StringAttributeValue,
    };
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::{print_type_annotated_value, ValueAndType};
    use indoc::{formatdoc, indoc};
    use itertools::Itertools;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorkerCreateView {
        pub component_name: ComponentName,
        pub worker_name: Option<WorkerName>,
    }

    impl MessageWithFields for WorkerCreateView {
        fn message(&self) -> String {
            if let Some(worker_name) = &self.worker_name {
                format!(
                    "Created new worker {}",
                    format_message_highlight(&worker_name)
                )
            } else {
                // TODO: review: do we really want to hide the worker name? it is provided now
                //       in "worker new"
                format!(
                    "Created new worker with a {}",
                    format_message_highlight("random generated name")
                )
            }
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Component name", &self.component_name, format_id)
                .fmt_field_option("Worker name", &self.worker_name, format_main_id);

            fields.build()
        }

        fn nest_ident_fields() -> bool {
            true
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
            format!(
                "Got metadata for worker {}",
                format_message_highlight(&self.0.worker_name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Component name", &self.0.component_name, format_id)
                .fmt_field("Component version", &self.0.component_version, format_id)
                .fmt_field("Worker name", &self.0.worker_name, format_main_id)
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

        fn nest_ident_fields() -> bool {
            true
        }
    }

    #[derive(Table)]
    struct WorkerMetadataTableView {
        #[table(title = "Component name")]
        pub component_name: ComponentName,
        #[table(title = "Worker name")]
        pub worker_name: WorkerName,
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
                component_name: value.component_name.clone(),
                worker_name: value.worker_name.clone(),
                status: format_status(&value.status),
                component_version: value.component_version,
                created_at: value.created_at,
            }
        }
    }

    impl TextView for WorkersMetadataResponseView {
        fn log(&self) {
            log_table::<_, WorkerMetadataTableView>(&self.workers);

            if !self.cursors.is_empty() {
                logln("");
            }
            for (component_name, cursor) in &self.cursors {
                logln(format!(
                    "Cursor for more results for component {}: {}",
                    component_name.log_color_highlight(),
                    cursor.log_color_highlight()
                ));
            }
        }
    }

    impl TextView for IdempotencyKey {
        fn log(&self) {
            logln(formatdoc!(
                "
                Idempotency key: {}

                You can use it in invoke-and-await command this way:
                {}

                ",
                format_main_id(&self.0),
                format!("invoke-and-await --idempotency-key {} ...", self.0).cyan() // TODO: also review for other outdated hints like this
            ))
        }
    }

    impl TextView for TryUpdateAllWorkersResult {
        fn log(&self) {
            // NOP
        }
    }

    impl TextView for InvokeResultView {
        fn log(&self) {
            fn log_results_format(format: &str) {
                logln(format!(
                    "Invocation results in {} format:",
                    format_message_highlight(format),
                ))
            }

            if self.result_wave.is_none() && self.result_json.is_none() {
                return;
            }

            if let Some(wave_values) = &self.result_wave {
                if wave_values.is_empty() {
                    logln("Empty result.")
                } else {
                    log_results_format("WAVE");
                    for wave in wave_values {
                        logln(format!("  - {}", wave));
                    }
                }
            } else if let Some(json) = &self.result_json {
                logln(format_warn(indoc!(
                    "
                    Failed to convert invocation result to WAVE format.
                    At the moment WAVE does not support Handle (aka Resource) data type.

                    "
                )));
                log_results_format("JSON");
                logln(serde_json::to_string_pretty(json).unwrap());
            }
        }
    }

    impl TextView for Vec<(u64, PublicOplogEntry)> {
        fn log(&self) {
            for (idx, entry) in self {
                print!("{}: ", format_main_id(&format!("#{idx:0>5}")));
                entry.log()
            }
        }
    }

    impl TextView for PublicOplogEntry {
        fn log(&self) {
            let pad = "          ";
            match self {
                PublicOplogEntry::Create(params) => {
                    logln(format_message_highlight("CREATE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}component version: {}",
                        format_id(&params.component_version),
                    ));
                    logln(format!(
                        "{pad}args:              {}",
                        format_id(&params.args.join(", ")),
                    ));
                    logln(format!("{pad}env:"));
                    for (k, v) in &params.env {
                        logln(format!("{pad}  - {}: {}", k, format_id(&v)));
                    }
                    if let Some(parent) = params.parent.as_ref() {
                        logln(format!("{pad}parent:            {}", format_id(parent)));
                    }
                    logln(format!("{pad}initial active plugins:"));
                    for plugin in &params.initial_active_plugins {
                        logln(format!(
                            "{pad}  - installation id: {}",
                            format_id(&plugin.installation_id)
                        ));
                        let inner_pad = format!("{pad}    ");
                        log_plugin_description(&inner_pad, plugin);
                    }
                }
                PublicOplogEntry::ImportedFunctionInvoked(params) => {
                    logln(format!(
                        "{} {}",
                        format_message_highlight("CALL"),
                        format_id(&params.function_name),
                    ));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}input:             {}",
                        value_to_string(&params.request)
                    ));
                    logln(format!(
                        "{pad}result:            {}",
                        value_to_string(&params.response)
                    ));
                }
                PublicOplogEntry::ExportedFunctionInvoked(params) => {
                    logln(format!(
                        "{} {}",
                        format_message_highlight("INVOKE"),
                        format_id(&params.function_name),
                    ));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}idempotency key:   {}",
                        format_id(&params.idempotency_key),
                    ));
                    logln(format!("{pad}input:"));
                    for param in &params.request {
                        logln(format!("{pad}  - {}", value_to_string(param)));
                    }
                }
                PublicOplogEntry::ExportedFunctionCompleted(params) => {
                    logln(format_message_highlight("INVOKE COMPLETED"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}consumed fuel:     {}",
                        format_id(&params.consumed_fuel),
                    ));
                    logln(format!(
                        "{pad}result:            {}",
                        value_to_string(&params.response)
                    ));
                }
                PublicOplogEntry::Suspend(params) => {
                    logln(format_message_highlight("SUSPEND"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::Error(params) => {
                    logln(format_message_highlight("ERROR"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}error:             {}",
                        format_error(&params.error)
                    ));
                }
                PublicOplogEntry::NoOp(params) => {
                    logln(format_message_highlight("NOP"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::Jump(params) => {
                    logln(format_message_highlight("JUMP"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}from:              {}",
                        format_id(&params.jump.start)
                    ));
                    logln(format!(
                        "{pad}to:                {}",
                        format_id(&params.jump.end)
                    ));
                }
                PublicOplogEntry::Interrupted(params) => {
                    logln(format_message_highlight("INTERRUPTED"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::Exited(params) => {
                    logln(format_message_highlight("EXITED"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::ChangeRetryPolicy(params) => {
                    logln(format_message_highlight("CHANGE RETRY POLICY"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}max attempts:      {}",
                        format_id(&params.new_policy.max_attempts),
                    ));
                    logln(format!(
                        "{pad}min delay:         {} ms",
                        format_id(&params.new_policy.min_delay.as_millis()),
                    ));
                    logln(format!(
                        "{pad}max delay:         {} ms",
                        format_id(&params.new_policy.max_delay.as_millis()),
                    ));
                    logln(format!(
                        "{pad}multiplier:        {}",
                        format_id(&params.new_policy.multiplier),
                    ));
                    logln(format!(
                        "{pad}max jitter factor: {}",
                        format_id(
                            &params
                                .new_policy
                                .max_jitter_factor
                                .map(|x| x.to_string())
                                .unwrap_or("-".to_string()),
                        ),
                    ));
                }
                PublicOplogEntry::BeginAtomicRegion(params) => {
                    logln(format_message_highlight("BEGIN ATOMIC REGION"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::EndAtomicRegion(params) => {
                    logln(format_message_highlight("END ATOMIC REGION"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}begin index:       {}",
                        format_id(&params.begin_index)
                    ));
                }
                PublicOplogEntry::BeginRemoteWrite(params) => {
                    logln(format_message_highlight("BEGIN REMOTE WRITE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::EndRemoteWrite(params) => {
                    logln(format_message_highlight("END REMOTE WRITE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}begin index:       {}",
                        format_id(&params.begin_index)
                    ));
                }
                PublicOplogEntry::PendingWorkerInvocation(params) => match &params.invocation {
                    PublicWorkerInvocation::ExportedFunction(inner_params) => {
                        logln(format!(
                            "{} {}",
                            format_message_highlight("ENQUEUED INVOCATION"),
                            format_id(&inner_params.full_function_name),
                        ));
                        logln(format!(
                            "{pad}at:                {}",
                            format_id(&params.timestamp)
                        ));
                        logln(format!(
                            "{pad}idempotency key:   {}",
                            format_id(&inner_params.idempotency_key),
                        ));
                        if let Some(input) = &inner_params.function_input {
                            logln(format!("{pad}input:"));
                            for param in input {
                                logln(format!("{pad}  - {}", value_to_string(param)));
                            }
                        }
                    }
                    PublicWorkerInvocation::ManualUpdate(inner_params) => {
                        logln(format_message_highlight("ENQUEUED MANUAL UPDATE"));
                        logln(format!(
                            "{pad}at:                {}",
                            format_id(&params.timestamp)
                        ));
                        logln(format!(
                            "{pad}target version: {}",
                            format_id(&inner_params.target_version),
                        ));
                    }
                },
                PublicOplogEntry::PendingUpdate(params) => {
                    logln(format_message_highlight("ENQUEUED UPDATE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}target version:    {}",
                        format_id(&params.target_version),
                    ));
                    match &params.description {
                        PublicUpdateDescription::Automatic(_) => {
                            logln(format!(
                                "{pad}type:              {}",
                                format_id("automatic")
                            ));
                        }
                        PublicUpdateDescription::SnapshotBased(inner_params) => {
                            logln(format!(
                                "{pad}type:              {}",
                                format_id("snapshot based")
                            ));
                            logln(format!(
                                "{pad}snapshot:          {}",
                                BASE64_STANDARD.encode(&inner_params.payload),
                            ));
                        }
                    }
                }
                PublicOplogEntry::SuccessfulUpdate(params) => {
                    logln(format_message_highlight("SUCCESSFUL UPDATE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}target version:    {}",
                        format_id(&params.target_version),
                    ));
                    logln(format!("{pad}new active plugins:"));
                    for plugin in &params.new_active_plugins {
                        logln(format!(
                            "{pad}  - installation id: {}",
                            format_id(&plugin.installation_id),
                        ));
                        let inner_pad = format!("{pad}    ");
                        log_plugin_description(&inner_pad, plugin);
                    }
                }
                PublicOplogEntry::FailedUpdate(params) => {
                    logln(format_message_highlight("FAILED UPDATE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}target version:    {}",
                        format_id(&params.target_version),
                    ));
                    if let Some(details) = &params.details {
                        logln(format!("{pad}error:             {}", format_error(details)));
                    }
                }
                PublicOplogEntry::GrowMemory(params) => {
                    logln(format_message_highlight("GROW MEMORY"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}increase:          {}",
                        format_id(&format_binary_size(&params.delta)),
                    ));
                }
                PublicOplogEntry::CreateResource(params) => {
                    logln(format_message_highlight("CREATE RESOURCE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!("{pad}resource id:       {}", format_id(&params.id)));
                }
                PublicOplogEntry::DropResource(params) => {
                    logln(format_message_highlight("DROP RESOURCE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!("{pad}resource id:       {}", format_id(&params.id)));
                }
                PublicOplogEntry::DescribeResource(params) => {
                    logln(format_message_highlight("DESCRIBE RESOURCE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!("{pad}resource id:       {}", format_id(&params.id)));
                    logln(format!(
                        "{pad}resource name:     {}",
                        format_id(&params.resource_name),
                    ));
                    logln(format!("{pad}resource parameters:"));
                    for value in &params.resource_params {
                        logln(format!("{pad}  - {}", value_to_string(value)));
                    }
                }
                PublicOplogEntry::Log(params) => {
                    logln(format_message_highlight("LOG"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}level:             {}",
                        format_id(&format!("{:?}", params.level)),
                    ));
                    logln(format!("{pad}message:           {}", params.message));
                }
                PublicOplogEntry::Restart(params) => {
                    logln(format_message_highlight("RESTART"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                }
                PublicOplogEntry::ActivatePlugin(params) => {
                    logln(format_message_highlight("ACTIVATE PLUGIN"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}installation id:   {}",
                        format_id(&params.plugin.installation_id),
                    ));
                    log_plugin_description(pad, &params.plugin);
                }
                PublicOplogEntry::DeactivatePlugin(params) => {
                    logln(format_message_highlight("DEACTIVATE PLUGIN"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}installation id:   {}",
                        format_id(&params.plugin.installation_id),
                    ));
                    log_plugin_description(pad, &params.plugin);
                }
                PublicOplogEntry::Revert(params) => {
                    logln(format_message_highlight("REVERT"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}to oplog index:    {}",
                        format_id(&params.dropped_region.start.previous()),
                    ));
                }
                PublicOplogEntry::CancelInvocation(params) => {
                    logln(format_message_highlight("CANCEL INVOCATION"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}idempotency key:   {}",
                        format_id(&params.idempotency_key),
                    ));
                }
                PublicOplogEntry::StartSpan(params) => {
                    logln(format_message_highlight("START SPAN"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}span id:           {}",
                        format_id(&params.span_id)
                    ));
                    if let Some(parent_id) = &params.parent_id {
                        logln(format!("{pad}parent span:       {}", format_id(&parent_id),));
                    }
                    if let Some(linked_id) = &params.linked_context {
                        logln(format!("{pad}linked span:       {}", format_id(&linked_id),));
                    }
                    logln(format!("{pad}attributes:"));
                    for (k, v) in &params.attributes {
                        logln(format!(
                            "{pad}  - {}: {}",
                            k,
                            match v {
                                PublicAttributeValue::String(StringAttributeValue { value }) =>
                                    format_id(value),
                            }
                        ));
                    }
                }
                PublicOplogEntry::FinishSpan(params) => {
                    logln(format_message_highlight("FINISH SPAN"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}span id:           {}",
                        format_id(&params.span_id)
                    ));
                }
                PublicOplogEntry::SetSpanAttribute(params) => {
                    logln(format_message_highlight("SET SPAN ATTRIBUTE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}span id:           {}",
                        format_id(&params.span_id)
                    ));
                    logln(format!(
                        "{pad}key:               {}",
                        format_id(&params.key)
                    ));
                    logln(format!(
                        "{pad}value:             {}",
                        match &params.value {
                            PublicAttributeValue::String(StringAttributeValue { value }) =>
                                format_id(value),
                        }
                    ));
                }
            }
        }
    }

    fn log_plugin_description(pad: &str, value: &PluginInstallationDescription) {
        logln(format!(
            "{pad}plugin name:       {}",
            format_id(&value.plugin_name)
        ));
        logln(format!(
            "{pad}plugin version:    {}",
            format_id(&value.plugin_version),
        ));
        logln(format!(
            "{pad}plugin parameters:    {}",
            format_id(&value.plugin_version),
        ));
        for (k, v) in &value.parameters {
            logln(format!("{pad}  - {}: {}", k, format_id(&v)));
        }
    }

    fn value_to_string(value: &ValueAndType) -> String {
        let tav: TypeAnnotatedValue = value.try_into().expect("Failed to convert value to string");
        print_type_annotated_value(&tav).expect("Failed to convert value to string")
    }
}

pub mod plugin {
    use crate::model::text::fmt::{
        format_id, format_main_id, format_message_highlight, log_table, FieldsBuilder,
        MessageWithFields, TextView,
    };
    use cli_table::Table;
    use golem_client::model::PluginInstallation;

    use crate::model::PluginDefinition;
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

    impl From<&PluginDefinition> for PluginDefinitionTableView {
        fn from(value: &PluginDefinition) -> Self {
            Self {
                name: value.name.clone(),
                version: value.version.clone(),
                description: value.description.clone(),
                homepage: value.homepage.clone(),
                typ: value.typ.clone(),
                scope: value.scope.clone(),
            }
        }
    }

    impl TextView for Vec<PluginDefinition> {
        fn log(&self) {
            log_table::<_, PluginDefinitionTableView>(self.as_slice())
        }
    }

    impl MessageWithFields for PluginDefinition {
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
                .field("Description", &self.description)
                .field("Homepage", &self.homepage)
                .field("Scope", &self.scope)
                .field("Type", &self.typ)
                .fmt_field_option(
                    "Validate URL",
                    &self.component_transformer_validate_url,
                    |f| f.to_string(),
                )
                .fmt_field_option(
                    "Transform URL",
                    &self.component_transformer_transform_url,
                    |f| f.to_string(),
                )
                .fmt_field_option(
                    "Component ID",
                    &self.oplog_processor_component_id,
                    format_id,
                )
                .fmt_field_option(
                    "Component Version",
                    &self.oplog_processor_component_version,
                    format_id,
                );

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

    // TODO: add component name to help with "multi-install"
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

    impl TextView for Vec<PluginInstallation> {
        fn log(&self) {
            log_table::<_, PluginInstallationTableView>(self.as_slice())
        }
    }
}

// Shared help messages
pub mod help {
    use crate::log::{logln, LogColorize};
    use crate::model::app::AppComponentName;
    use crate::model::component::render_type;
    use crate::model::text::fmt::{
        format_export, log_table, FieldsBuilder, MessageWithFields, TextView,
    };
    use cli_table::Table;
    use colored::Colorize;
    use golem_wasm_ast::analysis::AnalysedType;
    use indoc::indoc;
    use textwrap::WordSplitter;

    pub struct WorkerNameHelp;

    impl MessageWithFields for WorkerNameHelp {
        fn message(&self) -> String {
            "Accepted worker name formats:"
                .log_color_help_group()
                .to_string()
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            // NOTE: field descriptions - except for the last - are intentionally ending with and empty line
            fields.field(
                "<WORKER>",
                &indoc!(
                    "
                    Standalone worker name, usable when only one component is selected based on the
                    current application directory.

                    For ephemeral workers or for random worker name generation \"-\" can be used.

                    "
                ),
            );
            fields.field(
                "<COMPONENT>/<WORKER>",
                &indoc!(
                    "
                    Component specific worker name.

                    When used in an application (sub)directory then the given component name is fuzzy
                    matched against the application component names. If no matches are found, then
                    a the component name is used as is.

                    When used in a directory without application manifest(s), then the full component
                    name is expected.

                    "
                ),
            );
            fields.field(
                "<PROJECT>/<COMPONENT>/<WORKER>",
                &indoc!(
                    "
                    Project and component specific worker name.

                    Behaves the same as <COMPONENT>/<WORKER>, except it can refer to components in a
                    specific project.

                    "
                ),
            );
            fields.field(
                "<ACCOUNT>/<PROJECT>/<COMPONENT>/<WORKER>",
                &indoc!(
                    "
                    Account, project and component specific worker name.

                    Behaves the same as <COMPONENT>/<WORKER>, except it can refer to components in a
                    specific project owned by another account
                    "
                ),
            );

            fields.build()
        }

        fn indent_fields() -> bool {
            true
        }

        fn format_field_name(name: String) -> String {
            name.log_color_highlight().to_string()
        }
    }

    pub struct ComponentNameHelp;

    impl MessageWithFields for ComponentNameHelp {
        fn message(&self) -> String {
            "Accepted component name formats:"
                .log_color_help_group()
                .to_string()
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            // NOTE: field descriptions - except for the last - are intentionally ending with and empty line
            fields.field(
                "<COMPONENT>",
                &indoc!(
                    "
                    Standalone component name.

                    When used in an application (sub)directory then the given component name is fuzzy
                    matched against the application component names. If no matches are found, then
                    a the component name is used as is.

                    When used in a directory without application manifest(s), then the full component
                    name is expected.

                    "
                ),
            );
            fields.field(
                "<PROJECT>/<COMPONENT>",
                &indoc!(
                    "
                    Project specific component name.

                    Behaves the same as <COMPONENT>, except it can refer to components in a specific
                    project.

                    "
                ),
            );
            fields.field(
                "<ACCOUNT>/<PROJECT>/<COMPONENT>",
                &indoc!(
                    "
                    Account and Project specific component name.

                    Behaves the same as <COMPONENT>, except it can refer to components in a specific
                    project owned by another account.
                    "
                ),
            );

            fields.build()
        }

        fn indent_fields() -> bool {
            true
        }

        fn format_field_name(name: String) -> String {
            name.log_color_highlight().to_string()
        }
    }

    pub struct AvailableComponentNamesHelp(pub Vec<AppComponentName>);

    impl TextView for AvailableComponentNamesHelp {
        fn log(&self) {
            if self.0.is_empty() {
                logln(
                    "The application contains no components."
                        .log_color_warn()
                        .to_string(),
                );
                return;
            }

            logln(
                "Available application components:"
                    .bold()
                    .underline()
                    .to_string(),
            );
            for component_name in &self.0 {
                logln(format!("  - {}", component_name));
            }
            logln("");
        }
    }

    pub struct AvailableFunctionNamesHelp {
        pub component_name: String,
        pub function_names: Vec<String>,
    }

    impl TextView for AvailableFunctionNamesHelp {
        fn log(&self) {
            if self.function_names.is_empty() {
                logln(
                    format!(
                        "No functions are available for component {}.",
                        self.component_name.log_color_highlight()
                    )
                    .log_color_warn()
                    .to_string(),
                );
                return;
            }

            logln(
                format!(
                    "Available function names for component {}:",
                    self.component_name
                )
                .bold()
                .underline()
                .to_string(),
            );
            for function_name in &self.function_names {
                logln(format!("  - {}", format_export(function_name)));
            }
            logln("");
        }
    }

    pub struct ArgumentError {
        pub type_: Option<AnalysedType>,
        pub value: Option<String>,
        pub error: Option<String>,
    }

    // TODO: limit long values
    #[derive(Table)]
    pub struct ParameterErrorTable {
        #[table(title = "Parameter type")]
        pub parameter_type_: String,
        #[table(title = "Argument value")]
        pub argument_value: String,
        #[table(title = "Error")]
        pub error: String,
    }

    impl From<&ArgumentError> for ParameterErrorTable {
        fn from(value: &ArgumentError) -> Self {
            Self {
                parameter_type_: textwrap::wrap(
                    &value.type_.as_ref().map(render_type).unwrap_or_default(),
                    textwrap::Options::new(30).word_splitter(WordSplitter::NoHyphenation),
                )
                .join("\n"),
                argument_value: value.value.clone().unwrap_or_default(),
                error: value.error.clone().unwrap_or_default(),
            }
        }
    }

    pub struct ParameterErrorTableView(pub Vec<ArgumentError>);

    impl TextView for ParameterErrorTableView {
        fn log(&self) {
            log_table::<_, ParameterErrorTable>(self.0.as_slice());
        }
    }
}

pub mod account {
    use crate::model::text::fmt::*;
    use golem_cloud_client::model::{Account, Role};
    use serde::{Deserialize, Serialize};

    fn account_fields(account: &Account) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Account ID", &account.id, format_main_id)
            .fmt_field("E-mail", &account.email, format_id)
            .field("Name", &account.name);

        fields.build()
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct AccountGetView(pub Account);

    impl MessageWithFields for AccountGetView {
        fn message(&self) -> String {
            format!(
                "Got metadata for account {}",
                format_message_highlight(&self.0.id)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            account_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AccountNewView(pub Account);

    impl MessageWithFields for AccountNewView {
        fn message(&self) -> String {
            format!(
                "Created new account {}",
                format_message_highlight(&self.0.id)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            account_fields(&self.0)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AccountUpdateView(pub Account);

    impl MessageWithFields for AccountUpdateView {
        fn message(&self) -> String {
            format!("Updated account {}", format_message_highlight(&self.0.id))
        }

        fn fields(&self) -> Vec<(String, String)> {
            account_fields(&self.0)
        }
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    pub struct GrantGetView(pub Vec<Role>);

    impl TextView for GrantGetView {
        fn log(&self) {
            if self.0.is_empty() {
                println!("No roles granted")
            } else {
                println!("Granted roles:");
                for role in &self.0 {
                    println!("  - {}", role);
                }
            }
        }
    }
}

pub mod api_domain {
    use crate::model::text::fmt::*;
    use cli_table::Table;
    use golem_cloud_client::model::ApiDomain;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ApiDomainNewView(pub ApiDomain);

    impl MessageWithFields for ApiDomainNewView {
        fn message(&self) -> String {
            format!(
                "Created new API domain {}",
                format_message_highlight(&self.0.domain_name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Domain name", &self.0.domain_name, format_main_id)
                .fmt_field("Project ID", &self.0.project_id, format_id)
                .fmt_field_option("Created at", &self.0.created_at, |d| d.to_string())
                .fmt_field_optional(
                    "Name servers",
                    &self.0.name_servers,
                    !self.0.name_servers.is_empty(),
                    |ns| ns.join("\n"),
                );

            fields.build()
        }
    }

    #[derive(Table)]
    struct ApiDomainTableView {
        #[table(title = "Domain")]
        pub domain_name: String,
        #[table(title = "Project")]
        pub project_id: Uuid,
        #[table(title = "Servers")]
        pub name_servers: String,
    }

    impl From<&ApiDomain> for ApiDomainTableView {
        fn from(value: &ApiDomain) -> Self {
            ApiDomainTableView {
                domain_name: value.domain_name.to_string(),
                project_id: value.project_id,
                name_servers: value.name_servers.join("\n"),
            }
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ApiDomainListView(pub Vec<ApiDomain>);

    impl TextView for ApiDomainListView {
        fn log(&self) {
            log_table::<_, ApiDomainTableView>(&self.0);
        }
    }
}

pub mod certificate {
    use crate::model::text::fmt::*;
    use cli_table::Table;
    use golem_cloud_client::model::Certificate;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    fn certificate_fields(certificate: &Certificate) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Certificate ID", &certificate.id, format_main_id)
            .fmt_field("Domain name", &certificate.domain_name, format_main_id)
            .fmt_field("Project ID", &certificate.project_id, format_id)
            .fmt_field_option("Created at", &certificate.created_at, |d| d.to_string());

        fields.build()
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct CertificateNewView(pub Certificate);

    impl MessageWithFields for CertificateNewView {
        fn message(&self) -> String {
            format!(
                "Created new certificate {}",
                format_message_highlight(&self.0.domain_name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            certificate_fields(&self.0)
        }
    }

    #[derive(Table)]
    struct CertificateTableView {
        #[table(title = "Domain")]
        pub domain_name: String,
        #[table(title = "Certificate ID")]
        pub id: Uuid,
        #[table(title = "Project")]
        pub project_id: Uuid,
    }

    impl From<&Certificate> for CertificateTableView {
        fn from(value: &Certificate) -> Self {
            CertificateTableView {
                domain_name: value.domain_name.to_string(),
                id: value.id,
                project_id: value.project_id,
            }
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct CertificateListView(pub Vec<Certificate>);

    impl TextView for CertificateListView {
        fn log(&self) {
            log_table::<_, CertificateTableView>(&self.0);
        }
    }
}

pub mod project {
    use crate::model::project::ProjectView;
    use crate::model::text::fmt::*;
    use cli_table::Table;
    use golem_cloud_client::model::{Project, ProjectGrant, ProjectPolicy, ProjectType};
    use itertools::Itertools;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    fn project_fields(project: &ProjectView) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Project Name", &project.name.0, format_main_id)
            .fmt_field("Project ID", &project.project_id, format_main_id)
            .fmt_field("Account ID", &project.owner_account_id.0, format_id)
            .fmt_field("Environment ID", &project.default_environment_id, format_id)
            .field(
                "Default project",
                &(project.project_type == ProjectType::Default),
            )
            .field("Description", &project.description);

        fields.build()
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ProjectGetView(pub ProjectView);

    impl MessageWithFields for ProjectGetView {
        fn message(&self) -> String {
            format!(
                "Got metadata for project {}",
                format_message_highlight(&self.0.name.0)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            project_fields(&self.0)
        }
    }

    impl From<Project> for ProjectGetView {
        fn from(value: Project) -> Self {
            ProjectGetView(value.into())
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProjectCreatedView(pub ProjectView);

    impl From<Project> for ProjectCreatedView {
        fn from(value: Project) -> Self {
            ProjectCreatedView(value.into())
        }
    }

    impl MessageWithFields for ProjectCreatedView {
        fn message(&self) -> String {
            format!(
                "Created project {}",
                format_message_highlight(&self.0.name.0)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            project_fields(&self.0)
        }
    }

    #[derive(Table)]
    struct ProjectTableView {
        #[table(title = "Project ID")]
        pub project_id: Uuid,
        #[table(title = "Name")]
        pub name: String,
        #[table(title = "Description")]
        pub description: String,
    }

    impl From<&ProjectView> for ProjectTableView {
        fn from(value: &ProjectView) -> Self {
            ProjectTableView {
                project_id: value.project_id.0,
                name: value.name.0.clone(),
                description: textwrap::wrap(&value.description, 30).join("\n"),
            }
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ProjectListView(pub Vec<ProjectView>);

    impl From<Vec<Project>> for ProjectListView {
        fn from(value: Vec<Project>) -> Self {
            ProjectListView(value.into_iter().map(|v| v.into()).collect())
        }
    }

    impl TextView for ProjectListView {
        fn log(&self) {
            log_table::<_, ProjectTableView>(&self.0);
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ProjectGrantView(pub ProjectGrant);

    impl MessageWithFields for ProjectGrantView {
        fn message(&self) -> String {
            "Granted project".to_string()
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut field = FieldsBuilder::new();

            field
                .fmt_field("Project grant ID", &self.0.id, format_main_id)
                .fmt_field("Project ID", &self.0.data.grantor_project_id, format_id)
                .fmt_field("Account ID", &self.0.data.grantee_account_id, format_id)
                .fmt_field("Policy ID", &self.0.data.project_policy_id, format_id);

            field.build()
        }
    }

    fn project_policy_fields(policy: &ProjectPolicy) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Policy ID", &policy.id, format_main_id)
            .field("Policy name", &policy.name)
            .fmt_field_optional(
                "Actions",
                &policy.project_actions,
                !policy.project_actions.actions.is_empty(),
                |actions| {
                    actions
                        .actions
                        .iter()
                        .map(|a| format!("- {}", a))
                        .join("\n")
                },
            );

        fields.build()
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ProjectPolicyNewView(pub ProjectPolicy);

    impl MessageWithFields for ProjectPolicyNewView {
        fn message(&self) -> String {
            format!(
                "Created new project policy {}",
                format_message_highlight(&self.0.name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            project_policy_fields(&self.0)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ProjectPolicyGetView(pub ProjectPolicy);

    impl MessageWithFields for ProjectPolicyGetView {
        fn message(&self) -> String {
            format!(
                "Got metadata for project policy {}",
                format_message_highlight(&self.0.name)
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            project_policy_fields(&self.0)
        }
    }
}

pub mod token {
    use crate::model::text::fmt::*;
    use chrono::{DateTime, Utc};
    use cli_table::Table;
    use colored::Colorize;
    use golem_cloud_client::model::{Token, UnsafeToken};
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct TokenNewView(pub UnsafeToken);

    impl MessageWithFields for TokenNewView {
        fn message(&self) -> String {
            format!(
                "Created new token\n{}",
                format_warn("Save this token secret, you can't get this data later!")
            )
        }

        fn fields(&self) -> Vec<(String, String)> {
            let mut fields = FieldsBuilder::new();

            fields
                .fmt_field("Token ID", &self.0.data.id, format_main_id)
                .fmt_field("Account ID", &self.0.data.id, format_id)
                .field("Created at", &self.0.data.created_at)
                .field("Expires at", &self.0.data.expires_at)
                .fmt_field("Secret (SAVE THIS)", &self.0.secret.value, |s| {
                    s.to_string().bold().red().to_string()
                });

            fields.build()
        }
    }

    #[derive(Table)]
    struct TokenTableView {
        #[table(title = "ID")]
        pub id: Uuid,
        #[table(title = "Created at")]
        pub created_at: DateTime<Utc>,
        #[table(title = "Expires at")]
        pub expires_at: DateTime<Utc>,
        #[table(title = "Account")]
        pub account_id: String,
    }

    impl From<&Token> for TokenTableView {
        fn from(value: &Token) -> Self {
            TokenTableView {
                id: value.id,
                created_at: value.created_at,
                expires_at: value.expires_at,
                account_id: value.account_id.to_string(),
            }
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct TokenListView(pub Vec<Token>);

    impl TextView for TokenListView {
        fn log(&self) {
            log_table::<_, TokenTableView>(&self.0);
        }
    }
}
