// Copyright 2024 Golem Cloud
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

use crate::cloud::model::Role;
use crate::model::text::TextFormat;
use chrono::{DateTime, Utc};
use cli_table::{print_stdout, Table, WithTitle};
use colored::Colorize;
use golem_cloud_client::model::{
    Account, Project, ProjectGrant, ProjectPolicy, Token, UnsafeToken,
};
use golem_cloud_worker_client::model::{ApiDomain, Certificate};
use indoc::printdoc;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn print_account(account: &Account, action: &str) {
    printdoc!(
        "
        Account{action} with id {} for name {} with email {}.
        ",
        account.id,
        account.name,
        account.email,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountViewGet(pub Account);

impl TextFormat for AccountViewGet {
    fn print(&self) {
        print_account(&self.0, "")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountViewAdd(pub Account);

impl TextFormat for AccountViewAdd {
    fn print(&self) {
        print_account(&self.0, " created")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountViewUpdate(pub Account);

impl TextFormat for AccountViewUpdate {
    fn print(&self) {
        print_account(&self.0, " updated")
    }
}

impl TextFormat for Project {
    fn print(&self) {
        printdoc!(
            r#"
            Project "{}" with id {}.
            Description: "{}".
            Owner: {}, environment: {}, type: {}
            "#,
            self.project_data.name,
            self.project_id,
            self.project_data.description,
            self.project_data.owner_account_id,
            self.project_data.default_environment_id,
            self.project_data.project_type,
        )
    }
}

#[derive(Table)]
struct ProjectListView {
    #[table(title = "ID")]
    pub id: Uuid,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Description")]
    pub description: String,
}

impl From<&Project> for ProjectListView {
    fn from(value: &Project) -> Self {
        ProjectListView {
            id: value.project_id,
            name: value.project_data.name.to_string(),
            description: textwrap::wrap(&value.project_data.description, 30).join("\n"),
        }
    }
}

impl TextFormat for Vec<Project> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(ProjectListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

impl TextFormat for Vec<Role> {
    fn print(&self) {
        println!(
            "Available roles: {}.",
            self.iter().map(|r| r.to_string()).join(", ")
        )
    }
}

impl TextFormat for UnsafeToken {
    fn print(&self) {
        printdoc!(
            "
            New token created with id {} and expiration date {}.
            Please save this token secret, you can't get this data later:
            {}
            ",
            self.data.id,
            self.data.expires_at,
            self.secret.value.to_string().bold()
        )
    }
}

#[derive(Table)]
struct TokenListView {
    #[table(title = "ID")]
    pub id: Uuid,
    #[table(title = "Created at")]
    pub created_at: DateTime<Utc>,
    #[table(title = "Expires at")]
    pub expires_at: DateTime<Utc>,
    #[table(title = "Account")]
    pub account_id: String,
}

impl From<&Token> for TokenListView {
    fn from(value: &Token) -> Self {
        TokenListView {
            id: value.id,
            created_at: value.created_at,
            expires_at: value.expires_at,
            account_id: value.account_id.to_string(),
        }
    }
}

impl TextFormat for Vec<Token> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(TokenListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

impl TextFormat for ProjectGrant {
    fn print(&self) {
        printdoc!(
            "
            Project grant {}.
            Account: {}.
            Project: {}.
            Policy: {}
            ",
            self.id,
            self.data.grantee_account_id,
            self.data.grantor_project_id,
            self.data.project_policy_id,
        )
    }
}

impl TextFormat for ProjectPolicy {
    fn print(&self) {
        printdoc!(
            "
            Project policy {} with id {}.
            Actions: {}.
            ",
            self.name,
            self.id,
            self.project_actions
                .actions
                .iter()
                .map(|a| a.to_string())
                .join(", ")
        )
    }
}

impl TextFormat for Certificate {
    fn print(&self) {
        printdoc!(
            "
            Certificate with id {} for domain {} on project {}.
            ",
            self.id,
            self.domain_name,
            self.project_id
        )
    }
}

#[derive(Table)]
struct CertificateListView {
    #[table(title = "Domain")]
    pub domain_name: String,
    #[table(title = "ID")]
    pub id: Uuid,
    #[table(title = "Project")]
    pub project_id: Uuid,
}

impl From<&Certificate> for CertificateListView {
    fn from(value: &Certificate) -> Self {
        CertificateListView {
            domain_name: value.domain_name.to_string(),
            id: value.id,
            project_id: value.project_id,
        }
    }
}

impl TextFormat for Vec<Certificate> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(CertificateListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

impl TextFormat for ApiDomain {
    fn print(&self) {
        printdoc!(
            "
            Domain {} on project {}.
            Servers: {}.
            ",
            self.domain_name,
            self.project_id,
            self.name_servers.join(", ")
        )
    }
}

#[derive(Table)]
struct DomainListView {
    #[table(title = "Domain")]
    pub domain_name: String,
    #[table(title = "Project")]
    pub project_id: Uuid,
    #[table(title = "Servers")]
    pub name_servers: String,
}

impl From<&ApiDomain> for DomainListView {
    fn from(value: &ApiDomain) -> Self {
        DomainListView {
            domain_name: value.domain_name.to_string(),
            project_id: value.project_id,
            name_servers: value.name_servers.join("\n"),
        }
    }
}

impl TextFormat for Vec<ApiDomain> {
    fn print(&self) {
        print_stdout(
            self.iter()
                .map(DomainListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}
