#[cfg(test)]
mod tests {
    use chrono::Utc;
    use cloud_common::model::TokenSecret;
    use cloud_worker_service::model::{ApiDomain, Certificate, CertificateRequest, DomainRequest};
    use cloud_worker_service::repo::api_certificate::{ApiCertificateRepo, DbApiCertificateRepo};
    use cloud_worker_service::repo::api_domain::{ApiDomainRepo, DbApiDomainRepo};
    use cloud_worker_service::service::api_certificate::{
        CertificateManager, CertificateManagerNoop, CertificateService, CertificateServiceDefault,
    };
    use cloud_worker_service::service::api_domain::{
        ApiDomainService, ApiDomainServiceDefault, InMemoryRegisterDomain, RegisterDomain,
    };
    use cloud_worker_service::service::auth::{AuthService, CloudAuthCtx, CloudAuthServiceNoop};
    use golem_common::model::ProjectId;
    use golem_service_base::config::{DbPostgresConfig, DbSqliteConfig};
    use golem_service_base::db;
    use std::sync::Arc;
    use testcontainers::clients::Cli;
    use testcontainers::{Container, RunnableImage};
    use testcontainers_modules::postgres::Postgres;
    use uuid::Uuid;

    fn start_docker_postgres<'d>(docker: &'d Cli) -> (DbPostgresConfig, Container<'d, Postgres>) {
        let image = RunnableImage::from(Postgres::default()).with_tag("14.7-alpine");
        let container: Container<'d, Postgres> = docker.run(image);

        let config = DbPostgresConfig {
            host: "localhost".to_string(),
            port: container.get_host_port_ipv4(5432),
            database: "postgres".to_string(),
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            schema: Some("test".to_string()),
            max_connections: 10,
        };

        (config, container)
    }

    struct SqliteDb {
        db_path: String,
    }

    impl Default for SqliteDb {
        fn default() -> Self {
            Self {
                db_path: format!("/tmp/golem-worker-{}.db", uuid::Uuid::new_v4()),
            }
        }
    }

    impl Drop for SqliteDb {
        fn drop(&mut self) {
            std::fs::remove_file(&self.db_path).unwrap();
        }
    }

    #[tokio::test]
    pub async fn test_with_sqlite_db() {
        let db = SqliteDb::default();
        let db_config = DbSqliteConfig {
            database: db.db_path.clone(),
            max_connections: 10,
        };

        db::sqlite_migrate(&db_config, "./db/migration/sqlite")
            .await
            .unwrap();

        let db_pool = db::create_sqlite_pool(&db_config).await.unwrap();

        let api_certificate_repo: Arc<dyn ApiCertificateRepo + Sync + Send> =
            Arc::new(DbApiCertificateRepo::new(db_pool.clone().into()));
        let certificate_manager: Arc<dyn CertificateManager + Sync + Send> =
            Arc::new(CertificateManagerNoop::default());

        let auth_service: Arc<dyn AuthService + Sync + Send> =
            Arc::new(CloudAuthServiceNoop::default());

        let certificate_service: Arc<dyn CertificateService + Sync + Send> =
            Arc::new(CertificateServiceDefault::new(
                auth_service.clone(),
                certificate_manager.clone(),
                api_certificate_repo.clone(),
            ));

        let api_domain_repo: Arc<dyn ApiDomainRepo + Sync + Send> =
            Arc::new(DbApiDomainRepo::new(db_pool.clone().into()));

        let domain_register_service: Arc<dyn RegisterDomain + Sync + Send> =
            Arc::new(InMemoryRegisterDomain::default());

        let domain_service: Arc<dyn ApiDomainService + Sync + Send> =
            Arc::new(ApiDomainServiceDefault::new(
                auth_service.clone(),
                domain_register_service.clone(),
                api_domain_repo.clone(),
            ));

        test_certificate_service(certificate_service).await;
        test_domain_service(domain_service).await;
    }

    #[tokio::test]
    pub async fn test_with_postgres_db() {
        let cli = Cli::default();
        let (db_config, _container) = start_docker_postgres(&cli);

        db::postgres_migrate(&db_config, "./db/migration/postgres")
            .await
            .unwrap();

        let db_pool = db::create_postgres_pool(&db_config).await.unwrap();

        let api_certificate_repo: Arc<dyn ApiCertificateRepo + Sync + Send> =
            Arc::new(DbApiCertificateRepo::new(db_pool.clone().into()));
        let certificate_manager: Arc<dyn CertificateManager + Sync + Send> =
            Arc::new(CertificateManagerNoop::default());

        let auth_service: Arc<dyn AuthService + Sync + Send> =
            Arc::new(CloudAuthServiceNoop::default());

        let certificate_service: Arc<dyn CertificateService + Sync + Send> =
            Arc::new(CertificateServiceDefault::new(
                auth_service.clone(),
                certificate_manager.clone(),
                api_certificate_repo.clone(),
            ));

        let api_domain_repo: Arc<dyn ApiDomainRepo + Sync + Send> =
            Arc::new(DbApiDomainRepo::new(db_pool.clone().into()));

        let domain_register_service: Arc<dyn RegisterDomain + Sync + Send> =
            Arc::new(InMemoryRegisterDomain::default());

        let domain_service: Arc<dyn ApiDomainService + Sync + Send> =
            Arc::new(ApiDomainServiceDefault::new(
                auth_service.clone(),
                domain_register_service.clone(),
                api_domain_repo.clone(),
            ));

        test_certificate_service(certificate_service).await;
        test_domain_service(domain_service).await;
    }

    async fn test_certificate_service(
        certificate_service: Arc<dyn CertificateService + Sync + Send>,
    ) {
        let auth_ctx = CloudAuthCtx::new(TokenSecret::new(Uuid::new_v4()));

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let certificate_request1 = CertificateRequest {
            project_id: project_id.clone(),
            domain_name: "*.golem.test1".to_string(),
            certificate_body: "body1".to_string(),
            certificate_private_key: "key1".to_string(),
        };

        let certificate_request2 = CertificateRequest {
            project_id: project_id.clone(),
            domain_name: "*.golem.test2".to_string(),
            certificate_body: "body2".to_string(),
            certificate_private_key: "key2".to_string(),
        };

        let certificate1 = certificate_service
            .create(&certificate_request1, &auth_ctx)
            .await
            .unwrap();

        let certificate2 = certificate_service
            .create(&certificate_request2, &auth_ctx)
            .await
            .unwrap();

        let certificate_id1 = certificate1.id.clone();
        let certificate_id2 = certificate2.id.clone();

        let certificate1_result1 = certificate_service
            .get(project_id.clone(), Some(certificate_id1.clone()), &auth_ctx)
            .await
            .unwrap();

        let certificate2_result1 = certificate_service
            .get(project_id.clone(), Some(certificate_id2.clone()), &auth_ctx)
            .await
            .unwrap();

        let certificate_result2 = certificate_service
            .get(project_id.clone(), None, &auth_ctx)
            .await
            .unwrap();

        certificate_service
            .delete(&project_id, &certificate_id1, &auth_ctx)
            .await
            .unwrap();

        let certificate1_result3 = certificate_service
            .get(project_id.clone(), Some(certificate_id1.clone()), &auth_ctx)
            .await
            .unwrap_or(vec![]);

        let certificate_result3 = certificate_service
            .get(project_id.clone(), None, &auth_ctx)
            .await
            .unwrap();

        certificate_service
            .delete(&project_id, &certificate_id2, &auth_ctx)
            .await
            .unwrap();

        let certificate2_result3 = certificate_service
            .get(project_id.clone(), Some(certificate_id2.clone()), &auth_ctx)
            .await
            .unwrap();

        let certificate_result4 = certificate_service
            .get(project_id, None, &auth_ctx)
            .await
            .unwrap();

        assert!(contains_certificates(
            certificate1_result1,
            vec![certificate1.clone()]
        ));
        assert!(contains_certificates(
            certificate2_result1,
            vec![certificate2.clone()]
        ));
        assert_eq!(certificate_result2.len(), 2);

        assert!(certificate1_result3.is_empty());
        assert!(certificate2_result3.is_empty());
        assert!(contains_certificates(
            certificate_result3,
            vec![certificate2]
        ));
        assert!(certificate_result4.is_empty());
    }

    fn contains_certificates(result: Vec<Certificate>, expected: Vec<Certificate>) -> bool {
        for value in expected.into_iter() {
            if !result.iter().any(|c| {
                c.id == value.id
                    && c.project_id == value.project_id
                    && c.domain_name == value.domain_name
                    && c.created_at
                        .map(|c| c.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                        == value
                            .created_at
                            .map(|c| c.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            }) {
                return false;
            }
        }

        true
    }

    async fn test_domain_service(domain_service: Arc<dyn ApiDomainService + Sync + Send>) {
        let auth_ctx = CloudAuthCtx::new(TokenSecret::new(Uuid::new_v4()));

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let domain_name = "my-domain.com".to_string();

        let domain_request = DomainRequest {
            project_id: project_id.clone(),
            domain_name: domain_name.clone(),
        };

        let domain = domain_service
            .create_or_update(&domain_request, &auth_ctx)
            .await
            .unwrap();

        let expected = ApiDomain::new(
            &domain_request,
            vec![],
            domain.created_at.unwrap_or(Utc::now()),
        );

        assert_eq!(domain, expected);

        let result = domain_service.get(&project_id, &auth_ctx).await.unwrap();

        domain_service
            .delete(&project_id, domain_name.as_str(), &auth_ctx)
            .await
            .unwrap();

        let result2 = domain_service.get(&project_id, &auth_ctx).await.unwrap();

        assert!(!result.is_empty());
        assert_eq!(result[0].domain_name, domain.domain_name);
        assert_eq!(result[0].name_servers, domain.name_servers);
        assert!(result2.is_empty());
    }
}
