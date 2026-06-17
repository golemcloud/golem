// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agent_id_display::{SourceLanguage, render_type_for_language};
use crate::log::{LogColorize, LogIndent, logln};
use crate::model::component::show_exported_agent_constructors;
use crate::model::text::fmt::{
    Column, FieldsBuilder, MessageWithFields, MessageWithFieldsIndentMode, TextView, format_export,
    log_table, new_table_full,
};
use colored::Colorize;
use comfy_table::{Cell, CellAlignment, Color as ComfyColor};
use golem_client::model::ComponentDto;
use golem_common::model::agent::{
    AgentType, AgentTypeName, DeployedRegisteredAgentType, ElementSchema, ParsedAgentId,
};
use golem_common::model::component::ComponentName;
use indoc::indoc;
use std::path::PathBuf;

pub struct AgentNameHelp;

impl MessageWithFields for AgentNameHelp {
    fn message(&self) -> String {
        "Accepted agent ID formats:"
            .log_color_help_group()
            .to_string()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        // NOTE: field descriptions - except for the last - are intentionally ending with and empty line
        fields.field(
            "<AGENT>",
            &indoc!(
                "
                    Standalone agent ID.
                    "
            ),
        );
        fields.field(
            "<ENVIRONMENT>/<AGENT>",
            &indoc!(
                "
                        Environment specific agent ID.
                    "
            ),
        );
        fields.field(
            "<APPLICATION>/<ENVIRONMENT>/<AGENT>",
            &indoc!(
                "
                    Application, environment specific agent ID.
                    "
            ),
        );
        fields.field(
            "<ACCOUNT>/<APPLICATION>/<ENVIRONMENT>/<AGENT>",
            &indoc!(
                "
                    Account, application, environment specific agent ID.

                    Behaves the same as <APPLICATION>/<ENVIRONMENT>/<AGENT>, except it can
                    refer to any account.
                    "
            ),
        );

        fields.build()
    }

    fn indent_mode() -> MessageWithFieldsIndentMode {
        MessageWithFieldsIndentMode::IdentFields
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

    fn indent_mode() -> MessageWithFieldsIndentMode {
        MessageWithFieldsIndentMode::IdentFields
    }

    fn format_field_name(name: String) -> String {
        name.log_color_highlight().to_string()
    }
}

pub struct AvailableComponentNamesHelp(pub Vec<ComponentName>);

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
            logln(format!("  - {component_name}"));
        }
        logln("");
    }
}

pub struct AvailableFunctionNamesHelp {
    pub component_name: String,
    pub agent_name: Option<String>,
    pub function_names: Vec<String>,
}

impl AvailableFunctionNamesHelp {
    pub fn new_agent(
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        agent_type: &AgentType,
    ) -> Self {
        AvailableFunctionNamesHelp {
            component_name: component.component_name.0.clone(),
            agent_name: Some(agent_id.agent_type.0.clone()),
            function_names: agent_type.methods.iter().map(|m| m.name.clone()).collect(),
        }
    }
}

impl TextView for AvailableFunctionNamesHelp {
    fn log(&self) {
        if self.function_names.is_empty() {
            match &self.agent_name {
                Some(agent_name) => {
                    logln(
                        format!(
                            "No methods are available for agent {}.",
                            agent_name.underline()
                        )
                        .log_color_warn()
                        .to_string(),
                    );
                }
                None => {
                    logln(
                        format!(
                            "No functions are available for component {}.",
                            self.component_name.underline()
                        )
                        .log_color_warn()
                        .to_string(),
                    );
                }
            }
            return;
        }

        match &self.agent_name {
            Some(agent_name) => {
                logln(
                    format!("Available method names for agent {}:", agent_name)
                        .bold()
                        .underline()
                        .to_string(),
                );
            }
            None => {
                logln(
                    format!(
                        "Available function names for component {}:",
                        self.component_name
                    )
                    .bold()
                    .underline()
                    .to_string(),
                );
            }
        }
        for function_name in &self.function_names {
            logln(format!("  - {}", format_export(function_name)));
        }
        logln("");
    }
}

pub struct AvailableAgentConstructorsHelp {
    pub heading: String,
    pub empty_message: String,
    pub constructors: Vec<String>,
}

impl AvailableAgentConstructorsHelp {
    pub fn for_component(
        component: &ComponentDto,
        target_agent_type: Option<&AgentTypeName>,
    ) -> Self {
        let agent_types = component.metadata.agent_types();
        let constructors = if let Some(target_agent_type) = target_agent_type {
            agent_types
                .iter()
                .find(|agent_type| agent_type.type_name == *target_agent_type)
                .map(|agent_type| {
                    show_exported_agent_constructors(std::slice::from_ref(agent_type), true)
                })
                .unwrap_or_else(|| show_exported_agent_constructors(agent_types, true))
        } else {
            show_exported_agent_constructors(agent_types, true)
        };

        let component_name = &component.component_name.0;

        Self {
            heading: format!("Available agent constructors for component {component_name}:"),
            empty_message: format!(
                "No agent constructors are available for component {}.",
                component_name.log_color_highlight()
            ),
            constructors,
        }
    }

    pub fn for_deployed_agent_types(agent_types: &[DeployedRegisteredAgentType]) -> Self {
        let constructors = show_exported_agent_constructors(
            &agent_types
                .iter()
                .map(|agent_type| agent_type.agent_type.clone())
                .collect::<Vec<_>>(),
            true,
        );

        Self {
            heading: "Available deployed agent type constructors:".to_string(),
            empty_message: "No deployed agent type constructors are available.".to_string(),
            constructors,
        }
    }
}

impl TextView for AvailableAgentConstructorsHelp {
    fn log(&self) {
        if self.constructors.is_empty() {
            logln(self.empty_message.log_color_warn().to_string());
            return;
        }

        logln(self.heading.bold().underline().to_string());
        for constructor in &self.constructors {
            logln(format!("  - {}", format_export(constructor)));
        }
        logln("");
    }
}

pub struct ArgumentError {
    pub argument_index: usize,
    pub parameter_type: Option<ElementSchema>,
    pub value: Option<String>,
    pub error: Option<String>,
    pub source_language: SourceLanguage,
}

pub struct ParameterErrorTableView(pub Vec<ArgumentError>);

impl TextView for ParameterErrorTableView {
    fn log(&self) {
        let mut table = new_table_full(vec![
            Column::new("Arg #").fixed_right(),
            Column::new("Parameter type"),
            Column::new("Argument value"),
            Column::new("Error"),
        ]);

        for err in &self.0 {
            let has_error = err.error.is_some();

            table.add_row(vec![
                Cell::new(err.argument_index).set_alignment(CellAlignment::Right),
                Cell::new(
                    err.parameter_type
                        .as_ref()
                        .map(|parameter_type| {
                            render_parameter_type_for_language(&err.source_language, parameter_type)
                        })
                        .unwrap_or_default(),
                ),
                Cell::new(err.value.clone().unwrap_or_default()),
                Cell::new(err.error.clone().unwrap_or_default()).fg(if has_error {
                    ComfyColor::Red
                } else {
                    ComfyColor::Reset
                }),
            ]);
        }
        log_table(table);
    }
}

fn render_parameter_type_for_language(
    source_language: &SourceLanguage,
    parameter_type: &ElementSchema,
) -> String {
    match parameter_type {
        ElementSchema::ComponentModel(cm) => {
            // Adapt the legacy AnalysedType at the boundary.
            match golem_common::schema::adapters::analysed_type_to_schema_graph(&cm.element_type) {
                Ok(graph) => {
                    let root = graph.root.clone();
                    render_type_for_language(source_language, &graph, &root, true)
                }
                Err(_) => "<unknown>".to_string(),
            }
        }
        ElementSchema::UnstructuredText(text_descriptor) => {
            let mut result = "text".to_string();
            if let Some(restrictions) = &text_descriptor.restrictions {
                result.push('[');
                result.push_str(
                    &restrictions
                        .iter()
                        .map(|restriction| restriction.language_code.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                result.push(']');
            }
            result
        }
        ElementSchema::UnstructuredBinary(binary_descriptor) => {
            let mut result = "binary".to_string();
            if let Some(restrictions) = &binary_descriptor.restrictions {
                result.push('[');
                result.push_str(
                    &restrictions
                        .iter()
                        .map(|restriction| restriction.mime_type.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                result.push(']');
            }
            result
        }
    }
}

pub struct EnvironmentNameHelp;

impl MessageWithFields for EnvironmentNameHelp {
    fn message(&self) -> String {
        "Accepted environment and profile flags and environment variables:"
            .log_color_help_group()
            .to_string()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        // NOTE: field descriptions - except for the last - are intentionally ending with and empty line
        fields.field(
            "--environment <ENVIRONMENT_NAME>, -E <ENVIRONMENT_NAME>",
            &indoc! { "
                Selects an environment from the current application manifest.
                The application is searched based on the current directory.

                If no explicit flags or environment variables are used then the default environment
                is selected from the manifest.

                If there is no available application, then a more specific from has to be used.

                It is not compatible with the --cloud and --local flags.

            "},
        );
        fields.field(
            "--environment <APPLICATION_NAME>/<ENVIRONMENT_NAME>, -E <APPLICATION_NAME>/<ENVIRONMENT_NAME>",
            &indoc! { "
                Selects a server environment from a specific application. The used server is the
                current selected manifest environment or the selected profile if the CLI is used
                without a manifest.

            "},
        );
        fields.field(
            "--environment <ACCOUNT_EMAIL>/<APPLICATION_NAME>/<ENVIRONMENT_NAME>, -E <ACCOUNT_EMAIL>/<APPLICATION_NAME>/<ENVIRONMENT_NAME>",
            &indoc! { "
                Selects a server environment from a specific application owned by another account.
                The used server is the current selected manifest environment or the selected profile
                if the CLI is used without a manifest.

            "},
        );
        fields.field(
            "--local, -L",
            &indoc! { "
                When used with an application manifest then the environment is selected from the
                manifest. Without it it selects the built-in local profile.

            "},
        );
        fields.field(
            "--cloud, -C",
            &indoc! { "
                When used with an application manifest then the environment is selected from the
                manifest. Without it it selects the built-in cloud profile.

            "},
        );
        fields.field(
            "--profile",
            &indoc! { "
                Selects a different profile then the default one. Only effective when used without
                an application manifest.

            "},
        );
        fields.field(
            "GOLEM_ENVIRONMENT environment variable",
            &indoc! { "
                Alternative to the --environment flag.

            "},
        );
        fields.field(
            "GOLEM_PROFILE environment variable",
            &indoc! { "
                Alternative to the --profile flag.
            "},
        );

        fields.build()
    }

    fn indent_mode() -> MessageWithFieldsIndentMode {
        MessageWithFieldsIndentMode::IdentFields
    }

    fn format_field_name(name: String) -> String {
        name.log_color_highlight().to_string()
    }
}

pub enum AppNewNextStepsMode {
    NewApplication,
    ExistingApplication,
}

pub struct AppNewNextStepsHint {
    pub mode: AppNewNextStepsMode,
    pub app_dir: PathBuf,
    pub needs_switch_directory: bool,
    pub binary_name: String,
}

impl TextView for AppNewNextStepsHint {
    fn log(&self) {
        logln("");

        let title = match self.mode {
            AppNewNextStepsMode::NewApplication => "Next steps for your new application:",
            AppNewNextStepsMode::ExistingApplication => "Next steps for your updated application:",
        };

        logln(title.log_color_help_group().to_string());

        let _indent = LogIndent::new();

        logln("");

        if self.needs_switch_directory {
            logln(format!(
                "Switch to your application directory with {}",
                format!("'cd {}'", self.app_dir.display()).log_color_highlight()
            ));
            logln("");
        }

        logln(format!(
            "To build your application use {}",
            format!("'{} build'", self.binary_name).log_color_highlight()
        ));
        logln("");
        logln("To deploy them locally:");
        logln(format!(
            "  - start local server with {}",
            "'golem server run'".log_color_highlight()
        ));
        logln(format!(
            "  - use {} to deploy it",
            format!("'{} deploy'", self.binary_name).log_color_highlight()
        ));
        logln("");
    }
}
