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

use crate::model::project::ProjectView;
use crate::model::text::fmt::*;
use cli_table::Table;
use golem_client::model::{Project, ProjectGrant, ProjectPolicy, ProjectType};
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
            |actions| actions.actions.iter().map(|a| format!("- {a}")).join("\n"),
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
