use crate::Tracing;
use assert2::assert;
use golem_client::model::{Provider, SecuritySchemeData};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_api_security_scheme(deps: &EnvBasedTestDependencies) {
    let security_scheme = new_security_scheme();

    let created_security_scheme = deps
        .worker_service()
        .create_api_security_scheme(security_scheme.clone())
        .await
        .unwrap();

    assert!(created_security_scheme == security_scheme);
}

#[test]
#[tracing::instrument]
async fn get_api_security_scheme(deps: &EnvBasedTestDependencies) {
    let security_scheme = new_security_scheme();

    deps.worker_service()
        .create_api_security_scheme(security_scheme.clone())
        .await
        .unwrap();

    let get_result = deps
        .worker_service()
        .get_api_security_scheme(&security_scheme.scheme_identifier)
        .await
        .unwrap();

    assert!(get_result == security_scheme);
}

fn new_security_scheme() -> SecuritySchemeData {
    SecuritySchemeData {
        provider_type: Provider::Google,
        scheme_identifier: format!("security-scheme-{}", Uuid::new_v4()),
        client_id: "client_id".to_string(),
        client_secret: "super_secret".to_string(),
        redirect_url: "http://localhost/redirect-url".to_string(),
        scopes: vec!["custom-scope-1".to_string(), "custom-scope-2".to_string()],
    }
}
