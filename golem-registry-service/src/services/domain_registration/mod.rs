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

mod config;
mod errors;

use super::environment::{EnvironmentError, EnvironmentService};
use crate::repo::domain_registration::DomainRegistrationRepo;
use crate::repo::model::audit::ImmutableAuditFields;
use crate::repo::model::domain_registration::{
    DomainRegistrationRecord, DomainRegistrationRepoError,
};
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
pub use config::{
    AvailableDomainsConfig, DomainRegistrationConfig, RestrictedAvailableDomainsConfig,
};
pub use errors::DomainRegistrationError;
use golem_common::model::domain_registration::{
    Domain, DomainRegistration, DomainRegistrationCreation, DomainRegistrationId,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::EnvironmentAction;
use regex::Regex;
use std::sync::Arc;

pub struct DomainRegistrationService {
    domain_registration_repo: Arc<dyn DomainRegistrationRepo>,
    environment_service: Arc<EnvironmentService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    config: DomainRegistrationConfig,
}

impl DomainRegistrationService {
    pub fn new(
        domain_registration_repo: Arc<dyn DomainRegistrationRepo>,
        environment_service: Arc<EnvironmentService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
        config: DomainRegistrationConfig,
    ) -> Self {
        Self {
            domain_registration_repo,
            environment_service,
            registry_change_notifier,
            config,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: DomainRegistrationCreation,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DomainRegistrationError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateEnvironmentPluginGrant,
        )?;

        if !domain_available_to_provision(&data.domain, &self.config.available_domains) {
            return Err(DomainRegistrationError::DomainCannotBeProvisioned(
                data.domain,
            ));
        }

        let domain_registration = DomainRegistration {
            id: DomainRegistrationId::new(),
            environment_id,
            domain: data.domain.clone(),
        };

        let record = DomainRegistrationRecord::from_model(
            domain_registration,
            ImmutableAuditFields::new(auth.account_id().0),
        );

        let created_record = self
            .domain_registration_repo
            .create(record)
            .await
            .map_err(|err| match err {
                DomainRegistrationRepoError::DomainAlreadyExists => {
                    DomainRegistrationError::DomainAlreadyExists(data.domain)
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier);

        let created: DomainRegistration = created_record.into();

        Ok(created)
    }

    pub async fn delete(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        let (_, environment) = self
            .get_by_id_with_environment(domain_registration_id, auth)
            .await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteDomainRegistration,
        )?;

        let deleted_record = self
            .domain_registration_repo
            .delete(domain_registration_id.0, auth.account_id().0)
            .await?
            .ok_or(DomainRegistrationError::DomainRegistrationNotFound(
                domain_registration_id,
            ))?
            .signal_new_events_available(&self.registry_change_notifier);

        let deleted: DomainRegistration = deleted_record.into();

        Ok(deleted)
    }

    pub async fn get_by_id(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        Ok(self
            .get_by_id_with_environment(domain_registration_id, auth)
            .await?
            .0)
    }

    pub async fn get_in_environment(
        &self,
        environment: &Environment,
        domain: &Domain,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDomainRegistration,
        )
        .map_err(|_| DomainRegistrationError::DomainRegistrationByDomainNotFound(domain.clone()))?;

        let domain_registration: DomainRegistration = self
            .domain_registration_repo
            .get_in_environment(environment.id.0, &domain.0)
            .await?
            .ok_or(DomainRegistrationError::DomainRegistrationByDomainNotFound(
                domain.clone(),
            ))?
            .into();

        Ok(domain_registration)
    }

    pub fn validate_domain_for_http_api(
        &self,
        domain: &Domain,
    ) -> Result<(), DomainRegistrationError> {
        if domain_valid_for_http_api(&domain.0, &self.config.available_domains) {
            Ok(())
        } else {
            Err(DomainRegistrationError::DomainNotValidForHttpApi(
                domain.clone(),
            ))
        }
    }

    pub fn validate_domain_for_mcp(&self, domain: &Domain) -> Result<(), DomainRegistrationError> {
        if domain_valid_for_mcp(&domain.0, &self.config.available_domains) {
            Ok(())
        } else {
            Err(DomainRegistrationError::DomainNotValidForMcp(
                domain.clone(),
            ))
        }
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<DomainRegistration>, DomainRegistrationError> {
        // Optimally this is fetched together with the grant data instead of up front
        // see EnvironmentService::list_in_application for a better pattern
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DomainRegistrationError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDomainRegistration,
        )?;

        let domain_registrations: Vec<DomainRegistration> = self
            .domain_registration_repo
            .list_by_environment(environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(domain_registrations)
    }

    async fn get_by_id_with_environment(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: &AuthCtx,
    ) -> Result<(DomainRegistration, Environment), DomainRegistrationError> {
        let domain_registration: DomainRegistration = self
            .domain_registration_repo
            .get_by_id(domain_registration_id.0)
            .await?
            .ok_or(DomainRegistrationError::DomainRegistrationNotFound(
                domain_registration_id,
            ))?
            .into();

        let environment = self
            .environment_service
            .get(domain_registration.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    DomainRegistrationError::DomainRegistrationNotFound(domain_registration_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDomainRegistration,
        )
        .map_err(|_| DomainRegistrationError::DomainRegistrationNotFound(domain_registration_id))?;

        Ok((domain_registration, environment))
    }
}

fn domain_valid_for_http_api(domain: &str, config: &AvailableDomainsConfig) -> bool {
    match config {
        AvailableDomainsConfig::Unrestricted(_) => true,
        AvailableDomainsConfig::Restricted(restricted) => domain_matches_base(
            domain,
            &restricted.golem_apps_domain,
            restricted.allow_arbitary_subdomains,
        ),
    }
}

fn domain_valid_for_mcp(domain: &str, config: &AvailableDomainsConfig) -> bool {
    match config {
        AvailableDomainsConfig::Unrestricted(_) => true,
        AvailableDomainsConfig::Restricted(restricted) => domain_matches_base(
            domain,
            &restricted.golem_mcps_domain,
            restricted.allow_arbitary_subdomains,
        ),
    }
}

fn domain_matches_base(domain: &str, base: &str, allow_arbitrary_subdomains: bool) -> bool {
    let escaped = regex::escape(base);
    let pattern = if allow_arbitrary_subdomains {
        format!("^([^\\.]+\\.)+{escaped}$")
    } else {
        format!("^[^\\.]+\\.{escaped}$")
    };
    Regex::new(&pattern).unwrap().is_match(domain)
}

fn domain_available_to_provision(domain: &Domain, config: &AvailableDomainsConfig) -> bool {
    match config {
        AvailableDomainsConfig::Unrestricted(_) => true,
        AvailableDomainsConfig::Restricted(restricted) => {
            let allow_arbitrary = restricted.allow_arbitary_subdomains;
            let matches_apps =
                domain_matches_base(&domain.0, &restricted.golem_apps_domain, allow_arbitrary);
            let matches_mcps =
                domain_matches_base(&domain.0, &restricted.golem_mcps_domain, allow_arbitrary);
            matches_apps || matches_mcps
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use golem_common::model::Empty;
    use golem_common::model::domain_registration::Domain;

    use super::{
        AvailableDomainsConfig, RestrictedAvailableDomainsConfig, domain_available_to_provision,
        domain_valid_for_http_api, domain_valid_for_mcp,
    };

    fn domain(s: &str) -> Domain {
        Domain(s.to_string())
    }

    fn unrestricted() -> AvailableDomainsConfig {
        AvailableDomainsConfig::Unrestricted(Empty {})
    }

    fn restricted(
        apps_base: &str,
        mcps_base: &str,
        allow_arbitrary: bool,
    ) -> AvailableDomainsConfig {
        AvailableDomainsConfig::Restricted(RestrictedAvailableDomainsConfig {
            golem_apps_domain: apps_base.to_string(),
            golem_mcps_domain: mcps_base.to_string(),
            allow_arbitary_subdomains: allow_arbitrary,
        })
    }

    #[test]
    fn unrestricted_allows_any_domain() {
        let config = unrestricted();
        assert!(domain_available_to_provision(
            &domain("anything.example.com"),
            &config
        ));
        assert!(domain_available_to_provision(
            &domain("foo.bar.baz"),
            &config
        ));
        assert!(domain_available_to_provision(&domain("x"), &config));
    }

    #[test]
    fn restricted_no_arbitrary_allows_apps_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_available_to_provision(
            &domain("myapp.apps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_allows_mcps_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_available_to_provision(
            &domain("mymcp.mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_bare_apps_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_available_to_provision(
            &domain("apps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_bare_mcps_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_available_to_provision(
            &domain("mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_deep_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_available_to_provision(
            &domain("a.b.apps.golem.cloud"),
            &config
        ));
        assert!(!domain_available_to_provision(
            &domain("a.b.mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_different_base() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_available_to_provision(
            &domain("myapp.other.com"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_subdomain_with_dot_in_label() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_available_to_provision(
            &domain(".apps.golem.cloud"),
            &config
        ));
        assert!(!domain_available_to_provision(
            &domain(".mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_no_arbitrary_escapes_regex_special_chars_in_base() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_available_to_provision(
            &domain("myapp.appsXgolemYcloud"),
            &config
        ));
        assert!(!domain_available_to_provision(
            &domain("mymcp.mcpsXgolemYcloud"),
            &config
        ));
    }

    #[test]
    fn restricted_arbitrary_allows_apps_single_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_available_to_provision(
            &domain("myapp.apps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_arbitrary_allows_mcps_single_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_available_to_provision(
            &domain("mymcp.mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_arbitrary_allows_deep_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_available_to_provision(
            &domain("a.b.apps.golem.cloud"),
            &config
        ));
        assert!(domain_available_to_provision(
            &domain("x.y.z.apps.golem.cloud"),
            &config
        ));
        assert!(domain_available_to_provision(
            &domain("a.b.mcps.golem.cloud"),
            &config
        ));
        assert!(domain_available_to_provision(
            &domain("x.y.z.mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_arbitrary_rejects_bare_base_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(!domain_available_to_provision(
            &domain("apps.golem.cloud"),
            &config
        ));
        assert!(!domain_available_to_provision(
            &domain("mcps.golem.cloud"),
            &config
        ));
    }

    #[test]
    fn restricted_arbitrary_rejects_different_base() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(!domain_available_to_provision(
            &domain("myapp.other.com"),
            &config
        ));
    }

    #[test]
    fn restricted_arbitrary_escapes_regex_special_chars_in_base() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(!domain_available_to_provision(
            &domain("myapp.appsXgolemYcloud"),
            &config
        ));
        assert!(!domain_available_to_provision(
            &domain("mymcp.mcpsXgolemYcloud"),
            &config
        ));
    }

    #[test]
    fn http_api_valid_unrestricted() {
        let config = unrestricted();
        assert!(domain_valid_for_http_api("myapp.apps.golem.cloud", &config));
        assert!(domain_valid_for_http_api("mymcp.mcps.golem.cloud", &config));
        assert!(domain_valid_for_http_api("anything.example.com", &config));
    }

    #[test]
    fn http_api_valid_apps_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_valid_for_http_api("myapp.apps.golem.cloud", &config));
    }

    #[test]
    fn http_api_invalid_mcps_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_valid_for_http_api(
            "mymcp.mcps.golem.cloud",
            &config
        ));
    }

    #[test]
    fn http_api_invalid_unrelated_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_valid_for_http_api("myapp.other.com", &config));
    }

    #[test]
    fn http_api_valid_apps_deep_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_valid_for_http_api("a.b.apps.golem.cloud", &config));
    }

    #[test]
    fn http_api_invalid_apps_deep_subdomain_when_arbitrary_disallowed() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_valid_for_http_api("a.b.apps.golem.cloud", &config));
    }

    #[test]
    fn mcp_valid_unrestricted() {
        let config = unrestricted();
        assert!(domain_valid_for_mcp("mymcp.mcps.golem.cloud", &config));
        assert!(domain_valid_for_mcp("myapp.apps.golem.cloud", &config));
        assert!(domain_valid_for_mcp("anything.example.com", &config));
    }

    #[test]
    fn mcp_valid_mcps_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_valid_for_mcp("mymcp.mcps.golem.cloud", &config));
    }

    #[test]
    fn mcp_invalid_apps_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_valid_for_mcp("myapp.apps.golem.cloud", &config));
    }

    #[test]
    fn mcp_invalid_unrelated_domain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_valid_for_mcp("myapp.other.com", &config));
    }

    #[test]
    fn mcp_valid_mcps_deep_subdomain() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_valid_for_mcp("a.b.mcps.golem.cloud", &config));
    }

    #[test]
    fn mcp_invalid_mcps_deep_subdomain_when_arbitrary_disallowed() {
        let config = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_valid_for_mcp("a.b.mcps.golem.cloud", &config));
    }
}
