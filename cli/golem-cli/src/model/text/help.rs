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
use crate::model::app::AppComponentName;
use crate::model::component::render_type;
use crate::model::text::fmt::{
    format_export, log_table, FieldsBuilder, MessageWithFields, MessageWithFieldsIndentMode,
    TextView,
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
            logln(format!("  - {component_name}"));
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
