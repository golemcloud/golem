pub mod account {
    use golem_cli::model::text::fmt::*;
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
    pub struct AccountAddView(pub Account);

    impl MessageWithFields for AccountAddView {
        fn message(&self) -> String {
            format!("Added account {}", format_message_highlight(&self.0.id))
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

    #[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
    pub struct GrantGetView(pub Vec<Role>);

    impl TextFormat for GrantGetView {
        fn print(&self) {
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
    use cli_table::Table;
    use golem_cli::model::text::fmt::*;
    use golem_cloud_client::model::ApiDomain;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ApiDomainAddView(pub ApiDomain);

    impl MessageWithFields for ApiDomainAddView {
        fn message(&self) -> String {
            format!(
                "Added API domain {}",
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

    impl TextFormat for ApiDomainListView {
        fn print(&self) {
            print_table::<_, ApiDomainTableView>(&self.0);
        }
    }
}

pub mod certificate {
    use cli_table::Table;
    use golem_cli::model::text::fmt::*;
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
    pub struct CertificateAddView(pub Certificate);

    impl MessageWithFields for CertificateAddView {
        fn message(&self) -> String {
            format!(
                "Added certificate {}",
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

    impl TextFormat for CertificateListView {
        fn print(&self) {
            print_table::<_, CertificateTableView>(&self.0);
        }
    }
}

pub mod project {
    use crate::cloud::model::ProjectView;
    use cli_table::Table;
    use golem_cli::model::text::fmt::*;
    use golem_cloud_client::model::{Project, ProjectGrant, ProjectPolicy, ProjectType};
    use itertools::Itertools;
    use serde::{Deserialize, Serialize};

    fn project_fields(project: &ProjectView) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Project URN", &project.project_urn, format_main_id)
            .fmt_field("Account ID", &project.owner_account_id, format_id)
            .fmt_field("Environment ID", &project.default_environment_id, format_id)
            .field("Name ", &project.name)
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
                format_message_highlight(&self.0.name)
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
    pub struct ProjectAddView(pub ProjectView);

    impl From<Project> for ProjectAddView {
        fn from(value: Project) -> Self {
            ProjectAddView(value.into())
        }
    }

    impl MessageWithFields for ProjectAddView {
        fn message(&self) -> String {
            format!("Added project {}", format_message_highlight(&self.0.name))
        }

        fn fields(&self) -> Vec<(String, String)> {
            project_fields(&self.0)
        }
    }

    #[derive(Table)]
    struct ProjectTableView {
        #[table(title = "Project URN")]
        pub project_urn: String,
        #[table(title = "Name")]
        pub name: String,
        #[table(title = "Description")]
        pub description: String,
    }

    impl From<&ProjectView> for ProjectTableView {
        fn from(value: &ProjectView) -> Self {
            ProjectTableView {
                project_urn: value.project_urn.to_string(),
                name: value.name.clone(),
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

    impl TextFormat for ProjectListView {
        fn print(&self) {
            print_table::<_, ProjectTableView>(&self.0);
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ProjectShareView(pub ProjectGrant);

    impl MessageWithFields for ProjectShareView {
        fn message(&self) -> String {
            "Shared project".to_string()
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
    pub struct ProjectPolicyAddView(pub ProjectPolicy);

    impl MessageWithFields for ProjectPolicyAddView {
        fn message(&self) -> String {
            format!(
                "Added project policy {}",
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
    use chrono::{DateTime, Utc};
    use cli_table::Table;
    use colored::Colorize;
    use golem_cli::model::text::fmt::*;
    use golem_cloud_client::model::{Token, UnsafeToken};
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct UnsafeTokenView(pub UnsafeToken);

    impl MessageWithFields for UnsafeTokenView {
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

    impl TextFormat for TokenListView {
        fn print(&self) {
            print_table::<_, TokenTableView>(&self.0);
        }
    }
}
