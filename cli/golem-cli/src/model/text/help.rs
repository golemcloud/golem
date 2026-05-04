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
use crate::model::text::fmt::{
    Column, FieldsBuilder, MessageWithFields, MessageWithFieldsIndentMode, TextView, format_export,
    log_table, new_table,
};
use colored::Colorize;
use golem_client::model::ComponentDto;
use golem_common::model::agent::{AgentType, ParsedAgentId};
use golem_common::model::component::ComponentName;
use golem_wasm::analysis::AnalysedType;
use indoc::indoc;
use std::path::PathBuf;

pub struct AgentNameHelp;

impl MessageWithFields for AgentNameHelp {
    fn message(&self) -> String {
        "Accepted agent name formats:"
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
                    Standalone agent name.
                    "
            ),
        );
        fields.field(
            "<ENVIRONMENT>/<AGENT>",
            &indoc!(
                "
                        Environment specific agent name.
                    "
            ),
        );
        fields.field(
            "<APPLICATION>/<ENVIRONMENT>/<AGENT>",
            &indoc!(
                "
                    Application, environment specific agent name.
                    "
            ),
        );
        fields.field(
            "<ACCOUNT>/<APPLICATION>/<ENVIRONMENT>/<AGENT>",
            &indoc!(
                "
                    Account, application, environment specific agent name.

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
    pub component_name: String,
    pub constructors: Vec<String>,
}

impl TextView for AvailableAgentConstructorsHelp {
    fn log(&self) {
        if self.constructors.is_empty() {
            logln(
                format!(
                    "No agent constructors are available for component {}.",
                    self.component_name.log_color_highlight()
                )
                .log_color_warn()
                .to_string(),
            );
            return;
        }

        logln(
            format!(
                "Available agent constructors for component {}:",
                self.component_name
            )
            .bold()
            .underline()
            .to_string(),
        );
        for constructor in &self.constructors {
            logln(format!("  - {}", format_export(constructor)));
        }
        logln("");
    }
}

pub struct ArgumentError {
    pub type_: Option<AnalysedType>,
    pub value: Option<String>,
    pub error: Option<String>,
    pub source_language: SourceLanguage,
}

pub struct ParameterErrorTableView(pub Vec<ArgumentError>);

impl TextView for ParameterErrorTableView {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Parameter type"),
            Column::new("Argument value"),
            Column::new("Error"),
        ]);
        for err in &self.0 {
            table.add_row(vec![
                err.type_
                    .as_ref()
                    .map(|t| render_type_for_language(&err.source_language, t, true))
                    .unwrap_or_default(),
                err.value.clone().unwrap_or_default(),
                err.error.clone().unwrap_or_default(),
            ]);
        }
        log_table(table);
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
