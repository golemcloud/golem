// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::log::{logln, LogColorize};
use crate::model::component::render_type;
use crate::model::text::fmt::{
    format_export, log_table, FieldsBuilder, MessageWithFields, MessageWithFieldsIndentMode,
    TextView,
};
use cli_table::Table;
use colored::Colorize;
use golem_client::model::ComponentDto;
use golem_common::model::agent::{AgentId, AgentType};
use golem_common::model::component::ComponentName;
use golem_wasm::analysis::AnalysedType;
use indoc::indoc;
use textwrap::WordSplitter;

pub struct WorkerNameHelp;

impl MessageWithFields for WorkerNameHelp {
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
                    Standalone agent name, usable when only one component is selected based on the
                    current application directory.
                    "
            ),
        );
        fields.field(
            "<COMPONENT>/<AGENT>",
            &indoc!(
                    "
                        Component specific agent name.

                        When used in an application (sub)directory then the given component name is fuzzy
                        matched against the application component names. If no matches are found, then
                        a the component name is used as is.

                        When used in a directory without application manifest(s), then the full component
                        name is expected.

                    "
                ),
        );
        fields.field(
            "<ENVIRONMENT/<COMPONENT>/<AGENT>",
            &indoc!(
                "
                    Environment and component specific agent name.

                    Behaves the same as <COMPONENT>/<AGENT>, except it can refer to components in a
                    specific environment.

                    "
            ),
        );
        fields.field(
            "<APPLICATION>/<ENVIRONMENT/<COMPONENT>/<AGENT>",
            &indoc!(
                "
                    Application, environment and component specific agent name.

                    It can refer to components in a specific application and environment, always
                    expects the component name to be fully qualified.
                    "
            ),
        );
        fields.field(
            "<ACCOUNT>/<APPLICATION>/<ENVIRONMENT/<COMPONENT>/<AGENT>",
            &indoc!(
                "
                    Account, application, environment and component specific agent name.

                    Behaves the same as <APPLICATION>/<ENVIRONMENT/<COMPONENT>/<AGENT>, except it can
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
    pub fn new_agent(component: &ComponentDto, agent_id: &AgentId, agent_type: &AgentType) -> Self {
        AvailableFunctionNamesHelp {
            component_name: component.component_name.0.clone(),
            agent_name: Some(agent_id.wrapper_agent_type().to_string()),
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
