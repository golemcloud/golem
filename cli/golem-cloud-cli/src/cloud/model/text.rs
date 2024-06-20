use crate::cloud::model::Role;
use chrono::{DateTime, Utc};
use cli_table::{print_stdout, Table, WithTitle};
use colored::Colorize;
use golem_cli::model::text::TextFormat;
use golem_cloud_client::model::{
    Account, Project, ProjectGrant, ProjectPolicy, Token, UnsafeToken,
};
use golem_cloud_client::model::{ApiDomain, Certificate};
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectView(pub Project);

impl TextFormat for ProjectView {
    fn print(&self) {
        printdoc!(
            r#"
            Project "{}" with id {}.
            Description: "{}".
            Owner: {}, environment: {}, type: {}
            "#,
            self.0.project_data.name,
            self.0.project_id,
            self.0.project_data.description,
            self.0.project_data.owner_account_id,
            self.0.project_data.default_environment_id,
            self.0.project_data.project_type,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectVecView(pub Vec<Project>);

impl TextFormat for ProjectVecView {
    fn print(&self) {
        print_stdout(
            self.0
                .iter()
                .map(ProjectListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleVecView(pub Vec<Role>);

impl TextFormat for RoleVecView {
    fn print(&self) {
        println!(
            "Available roles: {}.",
            self.0.iter().map(|r| r.to_string()).join(", ")
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnsafeTokenView(pub UnsafeToken);

impl TextFormat for UnsafeTokenView {
    fn print(&self) {
        printdoc!(
            "
            New token created with id {} and expiration date {}.
            Please save this token secret, you can't get this data later:
            {}
            ",
            self.0.data.id,
            self.0.data.expires_at,
            self.0.secret.value.to_string().bold()
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

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenVecView(pub Vec<Token>);

impl TextFormat for TokenVecView {
    fn print(&self) {
        print_stdout(
            self.0
                .iter()
                .map(TokenListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectGrantView(pub ProjectGrant);

impl TextFormat for ProjectGrantView {
    fn print(&self) {
        printdoc!(
            "
            Project grant {}.
            Account: {}.
            Project: {}.
            Policy: {}
            ",
            self.0.id,
            self.0.data.grantee_account_id,
            self.0.data.grantor_project_id,
            self.0.data.project_policy_id,
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectPolicyView(pub ProjectPolicy);

impl TextFormat for ProjectPolicyView {
    fn print(&self) {
        printdoc!(
            "
            Project policy {} with id {}.
            Actions: {}.
            ",
            self.0.name,
            self.0.id,
            self.0
                .project_actions
                .actions
                .iter()
                .map(|a| a.to_string())
                .join(", ")
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificateView(pub Certificate);

impl TextFormat for CertificateView {
    fn print(&self) {
        printdoc!(
            "
            Certificate with id {} for domain {} on project {}.
            ",
            self.0.id,
            self.0.domain_name,
            self.0.project_id
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

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificateVecView(pub Vec<Certificate>);

impl TextFormat for CertificateVecView {
    fn print(&self) {
        print_stdout(
            self.0
                .iter()
                .map(CertificateListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiDomainView(pub ApiDomain);

impl TextFormat for ApiDomainView {
    fn print(&self) {
        printdoc!(
            "
            Domain {} on project {}.
            Servers: {}.
            ",
            self.0.domain_name,
            self.0.project_id,
            self.0.name_servers.join(", ")
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiDomainVecView(pub Vec<ApiDomain>);

impl TextFormat for ApiDomainVecView {
    fn print(&self) {
        print_stdout(
            self.0
                .iter()
                .map(DomainListView::from)
                .collect::<Vec<_>>()
                .with_title(),
        )
        .unwrap()
    }
}
