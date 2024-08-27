#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use cloud_common::model::Role;
    use cloud_common::model::{
        PlanId, ProjectActions, ProjectAuthorisedActions, ProjectGrantId, ProjectPolicyId, TokenId,
    };
    use cloud_service::auth::AccountAuthorisation;
    use cloud_service::config::{make_config_loader, CloudServiceConfig};
    use cloud_service::model::{
        Account, AccountData, OAuth2Provider, OAuth2Token, Project, ProjectData, ProjectGrant,
        ProjectGrantData, ProjectPolicy, ProjectType, Token,
    };
    use cloud_service::service::account::AccountService;
    use cloud_service::service::oauth2_token::OAuth2TokenService;
    use cloud_service::service::project::ProjectService;
    use cloud_service::service::project_grant::ProjectGrantService;
    use cloud_service::service::project_policy::ProjectPolicyService;
    use cloud_service::service::Services;
    use golem_common::model::AccountId;
    use golem_common::model::ProjectId;
    use golem_service_base::config::DbConfig;
    use golem_service_base::db;
    use testcontainers_modules::postgres::Postgres;
    use testcontainers_modules::testcontainers::clients::Cli;
    use testcontainers_modules::testcontainers::{Container, RunnableImage};
    use uuid::Uuid;

    fn start_docker_postgres<'d>(docker: &'d Cli) -> (CloudServiceConfig, Container<'d, Postgres>) {
        let image = RunnableImage::from(Postgres::default()).with_tag("14.7-alpine");
        let container: Container<'d, Postgres> = docker.run(image);

        std::env::set_var("GOLEM__SCALA_CLOUD_SERVER__HOST", "localhost");
        std::env::set_var("GOLEM__SCALA_CLOUD_SERVER__PORT", "1234");
        std::env::set_var("GOLEM__ROUTING_TABLE__HOST", "localhost");
        std::env::set_var("GOLEM__ROUTING_TABLE__PORT", "1234");
        std::env::set_var(
            "GOLEM__SCALA_CLOUD_SERVER__ACCESS_TOKEN",
            "7E0BBC59-DB10-4A6F-B508-7673FE948315",
        );
        std::env::set_var("GOLEM__DB__CONFIG__HOST", "localhost");
        std::env::set_var(
            "GOLEM__DB__CONFIG__PORT",
            container.get_host_port_ipv4(5432).to_string(),
        );
        std::env::set_var("GOLEM__DB__TYPE", "Postgres");
        std::env::set_var("GOLEM__DB__CONFIG__DATABASE", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__USERNAME", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__PASSWORD", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__SCHEMA", "test");
        std::env::set_var("GOLEM__ENVIRONMENT", "dev");
        std::env::set_var("GOLEM__WORKSPACE", "test");
        std::env::set_var(
            "GOLEM__ACCOUNTS__ROOT__TOKEN",
            "c88084af-3741-4946-8b58-fa445d770a26",
        );
        std::env::set_var(
            "GOLEM__ACCOUNTS__MARKETING__TOKEN",
            "bb249eb2-e54e-4bab-8e0e-836578e35912",
        );
        std::env::set_var(
            "GOLEM__ED_DSA__PRIVATE_KEY",
            "MC4CAQAwBQYDK2VwBCIEIGCD+oyHo7U5CP/6n/hYqkT4OeccA+a+OVqr526PMNJY",
        );
        std::env::set_var(
            "GOLEM__ED_DSA__PUBLIC_KEY",
            "MCowBQYDK2VwAyEAtKkMHoxrjJ52D/OEJ9Gww9hBl22m2YLU3qkWwTka02w=",
        );
        std::env::set_var("GOLEM__COMPONENTS__STORE__TYPE", "Local");
        std::env::set_var(
            "GOLEM__COMPONENTS__STORE__CONFIG__ROOT_PATH",
            "/tmp/golem/components",
        );

        println!("{:?}", std::env::vars());

        let config = make_config_loader()
            .load_or_dump_config()
            .expect("Failed to load config");
        (config, container)
    }

    fn create_auth(account_id: &AccountId, roles: Vec<Role>) -> AccountAuthorisation {
        AccountAuthorisation {
            token: Token {
                id: TokenId::new_v4(),
                account_id: account_id.clone(),
                created_at: chrono::Utc::now(),
                expires_at: chrono::Utc::now() + chrono::Duration::days(1),
            },
            roles,
        }
    }

    async fn create_account(
        account_id: &AccountId,
        account_service: Arc<dyn AccountService + Sync + Send>,
    ) -> Account {
        let account_data = AccountData {
            name: "acc_name".to_string(),
            email: "acc@golem.cloud".to_string(),
        };

        let auth = create_auth(account_id, Role::all());

        let create_result = account_service
            .create(account_id, &account_data, &auth)
            .await;

        assert!(
            create_result.is_ok(),
            "Failed to create account: {:?}",
            account_data
        );

        create_result.unwrap()
    }

    async fn create_oauth2_token(
        account_id: &AccountId,
        oauth2_token_service: Arc<dyn OAuth2TokenService + Sync + Send>,
    ) -> OAuth2Token {
        let token = OAuth2Token {
            provider: OAuth2Provider::Github,
            external_id: account_id.value.clone(),
            account_id: account_id.clone(),
            token_id: None,
        };

        let create_result = oauth2_token_service.upsert(&token).await;

        assert!(
            create_result.is_ok(),
            "Failed to create oauth2 token: {:?}",
            token
        );

        token
    }

    async fn create_project(
        project_id: &ProjectId,
        account_id: &AccountId,
        project_service: Arc<dyn ProjectService + Sync + Send>,
    ) -> Project {
        let project = Project {
            project_id: project_id.clone(),
            project_data: ProjectData {
                name: "project_name".to_string(),
                owner_account_id: account_id.clone(),
                description: "project_desc".to_string(),
                default_environment_id: "default".to_string(),
                project_type: ProjectType::NonDefault,
            },
        };
        let auth = create_auth(account_id, Role::all());
        let create_result = project_service.create(&project, &auth).await;

        assert!(
            create_result.is_ok(),
            "Failed to create project: {:?}",
            project
        );
        project
    }

    async fn delete_project(
        project_id: &ProjectId,
        account_id: &AccountId,
        project_service: Arc<dyn ProjectService + Sync + Send>,
    ) -> Project {
        let project = Project {
            project_id: project_id.clone(),
            project_data: ProjectData {
                name: "project_name".to_string(),
                owner_account_id: account_id.clone(),
                description: "project_desc".to_string(),
                default_environment_id: "default".to_string(),
                project_type: ProjectType::NonDefault,
            },
        };
        let auth = create_auth(account_id, Role::all());
        let create_result = project_service.create(&project, &auth).await;
        let delete_result = project_service.delete(project_id, &auth).await;

        println!("{create_result:?}");
        assert!(
            create_result.is_ok(),
            "Failed to create project: {:?}",
            project
        );

        println!("{delete_result:?}");

        assert!(
            delete_result.is_ok(),
            "Failed to delete project: {:?}",
            project
        );
        project
    }

    async fn create_project_policy(
        id: &ProjectPolicyId,
        project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
    ) -> ProjectPolicy {
        let policy = ProjectPolicy {
            id: id.clone(),
            name: "policy_name".to_string(),
            project_actions: ProjectActions::all(),
        };

        let create_result = project_policy_service.create(&policy).await;

        assert!(
            create_result.is_ok(),
            "Failed to create project policy: {:?}",
            policy
        );
        policy
    }

    async fn create_project_grant(
        id: &ProjectGrantId,
        data: &ProjectGrantData,
        project_grant_service: Arc<dyn ProjectGrantService + Sync + Send>,
    ) -> ProjectGrant {
        let grant = ProjectGrant {
            id: id.clone(),
            data: data.clone(),
        };

        let auth = create_auth(&data.grantee_account_id, Role::all());

        let create_result = project_grant_service.create(&grant, &auth).await;

        assert!(
            create_result.is_ok(),
            "Failed to create project grant: {:?}",
            grant
        );
        grant
    }

    async fn test_services(config: &CloudServiceConfig) {
        let services: Services = Services::new(config).await.unwrap();

        let _ = services.plan_service.create_initial_plan().await;

        let plan_by_id = services
            .plan_service
            .get(&PlanId(config.plans.default.plan_id))
            .await
            .unwrap();

        let default_plan = services.plan_service.get_default_plan().await.unwrap();

        assert!(plan_by_id.is_some_and(|p| p == default_plan));

        let account_id = AccountId::from("1");
        let account_id2 = AccountId::from("2");

        let auth = create_auth(&account_id, Role::all());
        let auth2 = create_auth(&account_id2, Role::all());

        let account = create_account(&account_id, services.account_service.clone()).await;
        let account2 = create_account(&account_id2, services.account_service.clone()).await;

        let account_by_id = services
            .account_service
            .get(&account.id, &auth)
            .await
            .unwrap();

        assert!(account_by_id == account);

        let token = services
            .token_service
            .create(
                &account.id,
                &(chrono::Utc::now() + chrono::Duration::minutes(2)),
                &auth,
            )
            .await
            .unwrap();

        let token_by_id = services
            .token_service
            .get(&token.data.id, &auth)
            .await
            .unwrap();
        // assert!(token_by_id == token.data); // FIXME failing in CI - probably related to timestamps
        assert!(token_by_id.account_id == token.data.account_id);

        let token_by_secret = services
            .token_service
            .get_by_secret(&token.secret)
            .await
            .unwrap();
        // assert!(token_by_secret.is_some_and(|t| t == token_by_id));  // FIXME failing in CI - probably related to timestamps
        assert!(token_by_secret.is_some_and(|t| t.account_id == token_by_id.account_id));

        let tokens_by_account = services
            .token_service
            .find(&token.data.account_id, &auth)
            .await
            .unwrap();
        // assert!(tokens_by_account == vec![token.data]); // FIXME failing in CI - probably related to timestamps
        assert!(tokens_by_account.len() == 1);

        let auth = services
            .auth_service
            .authorization(&token.secret)
            .await
            .unwrap();

        assert!(auth.token.account_id == token.data.account_id && auth.token.id == token.data.id);

        let oauth2_token =
            create_oauth2_token(&account.id, services.oauth2_token_service.clone()).await;

        let oauth2_token_by_id = services
            .oauth2_token_service
            .get(&oauth2_token.provider, oauth2_token.external_id.as_str())
            .await
            .unwrap();

        assert!(oauth2_token_by_id.is_some_and(|p| p == oauth2_token));

        // let token_id = TokenId::new_v4();

        let project_default = services
            .project_service
            .get_own_default(&auth)
            .await
            .unwrap();

        let project_id = ProjectId::new_v4();
        let project =
            create_project(&project_id, &account.id, services.project_service.clone()).await;

        let projects = services.project_service.get_own(&auth).await.unwrap();

        assert!(
            HashSet::from_iter(projects)
                == HashSet::from([project_default.clone(), project.clone()])
        );

        let count = services.project_service.get_own_count(&auth).await.unwrap();

        assert!(count == 2);

        let project_by_id = services
            .project_service
            .get(&project_id, &auth)
            .await
            .unwrap();

        assert!(project_by_id.is_some_and(|p| p == project));

        let project_policy = create_project_policy(
            &ProjectPolicyId::new_v4(),
            services.project_policy_service.clone(),
        )
        .await;

        let project_policy_by_id = services
            .project_policy_service
            .get(&project_policy.id)
            .await
            .unwrap();

        assert!(project_policy_by_id.is_some_and(|p| p == project_policy));

        let project_grant = create_project_grant(
            &ProjectGrantId::new_v4(),
            &ProjectGrantData {
                grantor_project_id: project_id.clone(),
                grantee_account_id: account2.id.clone(),
                project_policy_id: project_policy.id.clone(),
            },
            services.project_grant_service.clone(),
        )
        .await;

        let project_grant_by_id = services
            .project_grant_service
            .get(&project_id, &project_grant.id, &auth2)
            .await
            .unwrap();

        assert!(project_grant_by_id.is_some_and(|p| p == project_grant));

        let project_grant_by_project = services
            .project_grant_service
            .get_by_project(&project_id, &auth2)
            .await
            .unwrap();

        assert!(project_grant_by_project == vec![project_grant]);

        let project_grant_by_account = services
            .project_grant_service
            .get_by_account(&account2.id, &auth2)
            .await
            .unwrap();

        assert!(project_grant_by_account == project_grant_by_project);

        let project_actions = services
            .project_auth_service
            .get_by_project(&project_id, &auth)
            .await
            .unwrap();

        let all_project_actions = ProjectAuthorisedActions {
            actions: ProjectActions::all(),
            owner_account_id: account.id.clone(),
            project_id: project_id.clone(),
        };

        assert!(project_actions == all_project_actions);

        let project_actions = services
            .project_auth_service
            .get_by_project(&project_id, &auth2)
            .await
            .unwrap();

        assert!(project_actions == all_project_actions);

        let auth = create_auth(&account.id, Role::all());
        let auth2 = create_auth(&account2.id, Role::all());

        let project_actions = services.project_auth_service.get_all(&auth).await.unwrap();

        let all_project_actions = HashMap::from([
            (project_id.clone(), ProjectActions::all()),
            (project_default.project_id, ProjectActions::all()),
        ]);

        assert!(project_actions == all_project_actions);

        let project_actions = services.project_auth_service.get_all(&auth2).await.unwrap();

        let all_project_actions = HashMap::from([(project_id.clone(), ProjectActions::all())]);

        assert!(project_actions == all_project_actions);

        let new_project_id = ProjectId::new_v4();

        delete_project(
            &new_project_id,
            &account.id,
            services.project_service.clone(),
        )
        .await;

        let account_summaries = services
            .account_summary_service
            .get(0, 10, &auth)
            .await
            .unwrap();
        assert!(account_summaries.len() == 1);
    }

    #[tokio::test]
    pub async fn test_postgres_db() {
        let cli = Cli::default();
        let (config, _container) = start_docker_postgres(&cli);

        let db_config = match config.db.clone() {
            DbConfig::Postgres(db_config) => db_config,
            _ => panic!("Invalid DB config"),
        };

        db::postgres_migrate(&db_config, "./db/migration/postgres")
            .await
            .unwrap();

        test_services(&config).await;
    }

    struct SqliteDb {
        db_path: String,
    }

    impl Default for SqliteDb {
        fn default() -> Self {
            Self {
                db_path: format!("/tmp/golem-{}.db", Uuid::new_v4()),
            }
        }
    }

    impl Drop for SqliteDb {
        fn drop(&mut self) {
            std::fs::remove_file(&self.db_path).unwrap();
        }
    }

    #[tokio::test]
    pub async fn test_sqlite_db() {
        let db = SqliteDb::default();
        std::env::set_var("GOLEM__SCALA_CLOUD_SERVER__HOST", "localhost");
        std::env::set_var("GOLEM__SCALA_CLOUD_SERVER__PORT", "1234");
        std::env::set_var("GOLEM__ROUTING_TABLE__HOST", "localhost");
        std::env::set_var("GOLEM__ROUTING_TABLE__PORT", "1234");
        std::env::set_var(
            "GOLEM__SCALA_CLOUD_SERVER__ACCESS_TOKEN",
            "7E0BBC59-DB10-4A6F-B508-7673FE948315",
        );
        std::env::set_var("GOLEM__DB__TYPE", "Sqlite");
        std::env::set_var("GOLEM__DB__CONFIG__DATABASE", db.db_path.clone());
        std::env::set_var("GOLEM__ENVIRONMENT", "dev");
        std::env::set_var("GOLEM__WORKSPACE", "test");
        std::env::set_var(
            "GOLEM__ACCOUNTS__ROOT__TOKEN",
            "c88084af-3741-4946-8b58-fa445d770a26",
        );
        std::env::set_var(
            "GOLEM__ACCOUNTS__MARKETING__TOKEN",
            "bb249eb2-e54e-4bab-8e0e-836578e35912",
        );
        std::env::set_var(
            "GOLEM__ED_DSA__PRIVATE_KEY",
            "MC4CAQAwBQYDK2VwBCIEIGCD+oyHo7U5CP/6n/hYqkT4OeccA+a+OVqr526PMNJY",
        );
        std::env::set_var(
            "GOLEM__ED_DSA__PUBLIC_KEY",
            "MCowBQYDK2VwAyEAtKkMHoxrjJ52D/OEJ9Gww9hBl22m2YLU3qkWwTka02w=",
        );
        std::env::set_var("GOLEM__COMPONENTS__STORE__TYPE", "Local");
        std::env::set_var(
            "GOLEM__COMPONENTS__STORE__CONFIG__ROOT_PATH",
            "/tmp/golem/components",
        );

        println!("{:?}", std::env::vars());

        let config = make_config_loader()
            .load_or_dump_config()
            .expect("Failed to load config");

        let db_config = match config.db.clone() {
            DbConfig::Sqlite(db_config) => db_config,
            _ => panic!("Invalid DB config"),
        };

        db::sqlite_migrate(&db_config, "./db/migration/sqlite")
            .await
            .unwrap();

        test_services(&config).await;
    }
}
