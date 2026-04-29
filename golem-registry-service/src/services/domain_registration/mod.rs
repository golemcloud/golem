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
    domain_matcher: DomainMatcher,
}

impl DomainRegistrationService {
    pub fn new(
        domain_registration_repo: Arc<dyn DomainRegistrationRepo>,
        environment_service: Arc<EnvironmentService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
        config: DomainRegistrationConfig,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            domain_registration_repo,
            environment_service,
            registry_change_notifier,
            domain_matcher: DomainMatcher::new(&config.available_domains)?,
        })
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

        if !self
            .domain_matcher
            .domain_available_to_provision(&data.domain)
        {
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
        if self.domain_matcher.domain_valid_for_http_api(domain) {
            Ok(())
        } else {
            Err(DomainRegistrationError::DomainNotValidForHttpApi(
                domain.clone(),
            ))
        }
    }

    pub fn validate_domain_for_mcp(&self, domain: &Domain) -> Result<(), DomainRegistrationError> {
        if self.domain_matcher.domain_valid_for_mcp(domain) {
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

enum DomainMatcher {
    Unrestricted,
    Restricted {
        golem_apps_domain_regex: Regex,
        golem_mcps_domain_regex: Regex,
    },
}

impl DomainMatcher {
    fn new(config: &AvailableDomainsConfig) -> anyhow::Result<Self> {
        match config {
            AvailableDomainsConfig::Unrestricted(_) => Ok(Self::Unrestricted),
            AvailableDomainsConfig::Restricted(restricted) => {
                let make_regex = |base| {
                    let escaped = regex::escape(base);
                    let pattern = if restricted.allow_arbitrary_subdomains {
                        format!("^([^\\.]+\\.)+{escaped}$")
                    } else {
                        format!("^[^\\.]+\\.{escaped}$")
                    };
                    Regex::new(&pattern)
                };

                Ok(Self::Restricted {
                    golem_apps_domain_regex: make_regex(&restricted.golem_apps_domain)?,
                    golem_mcps_domain_regex: make_regex(&restricted.golem_mcps_domain)?,
                })
            }
        }
    }

    fn domain_valid_for_http_api(&self, domain: &Domain) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Restricted {
                golem_apps_domain_regex,
                ..
            } => golem_apps_domain_regex.is_match(&domain.0),
        }
    }

    fn domain_valid_for_mcp(&self, domain: &Domain) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Restricted {
                golem_mcps_domain_regex,
                ..
            } => golem_mcps_domain_regex.is_match(&domain.0),
        }
    }

    fn domain_available_to_provision(&self, domain: &Domain) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Restricted {
                golem_apps_domain_regex,
                golem_mcps_domain_regex,
            } => {
                golem_apps_domain_regex.is_match(&domain.0)
                    || golem_mcps_domain_regex.is_match(&domain.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use golem_common::model::Empty;
    use golem_common::model::domain_registration::Domain;

    use super::{AvailableDomainsConfig, DomainMatcher, RestrictedAvailableDomainsConfig};

    fn domain(s: &str) -> Domain {
        Domain(s.to_string())
    }

    fn unrestricted() -> DomainMatcher {
        DomainMatcher::new(&AvailableDomainsConfig::Unrestricted(Empty {})).unwrap()
    }

    fn restricted(apps_base: &str, mcps_base: &str, allow_arbitrary: bool) -> DomainMatcher {
        DomainMatcher::new(&AvailableDomainsConfig::Restricted(
            RestrictedAvailableDomainsConfig {
                golem_apps_domain: apps_base.to_string(),
                golem_mcps_domain: mcps_base.to_string(),
                allow_arbitrary_subdomains: allow_arbitrary,
            },
        ))
        .unwrap()
    }

    #[test]
    fn unrestricted_allows_any_domain() {
        let domain_matcher = unrestricted();
        assert!(domain_matcher.domain_available_to_provision(&domain("anything.example.com")));
        assert!(domain_matcher.domain_available_to_provision(&domain("foo.bar.baz")));
        assert!(domain_matcher.domain_available_to_provision(&domain("x")));
    }

    #[test]
    fn restricted_no_arbitrary_allows_apps_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_matcher.domain_available_to_provision(&domain("myapp.apps.golem.cloud")));
    }

    #[test]
    fn restricted_no_arbitrary_allows_mcps_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_matcher.domain_available_to_provision(&domain("mymcp.mcps.golem.cloud")));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_bare_apps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_available_to_provision(&domain("apps.golem.cloud")));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_bare_mcps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_available_to_provision(&domain("mcps.golem.cloud")));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_deep_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_available_to_provision(&domain("a.b.apps.golem.cloud")));
        assert!(!domain_matcher.domain_available_to_provision(&domain("a.b.mcps.golem.cloud")));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_different_base() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_available_to_provision(&domain("myapp.other.com")));
    }

    #[test]
    fn restricted_no_arbitrary_rejects_subdomain_with_dot_in_label() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_available_to_provision(&domain(".apps.golem.cloud")));
        assert!(!domain_matcher.domain_available_to_provision(&domain(".mcps.golem.cloud")));
    }

    #[test]
    fn restricted_no_arbitrary_escapes_regex_special_chars_in_base() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_available_to_provision(&domain("myapp.appsXgolemYcloud")));
        assert!(!domain_matcher.domain_available_to_provision(&domain("mymcp.mcpsXgolemYcloud")));
    }

    #[test]
    fn restricted_arbitrary_allows_apps_single_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_matcher.domain_available_to_provision(&domain("myapp.apps.golem.cloud")));
    }

    #[test]
    fn restricted_arbitrary_allows_mcps_single_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_matcher.domain_available_to_provision(&domain("mymcp.mcps.golem.cloud")));
    }

    #[test]
    fn restricted_arbitrary_allows_deep_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_matcher.domain_available_to_provision(&domain("a.b.apps.golem.cloud")));
        assert!(domain_matcher.domain_available_to_provision(&domain("x.y.z.apps.golem.cloud")));
        assert!(domain_matcher.domain_available_to_provision(&domain("a.b.mcps.golem.cloud")));
        assert!(domain_matcher.domain_available_to_provision(&domain("x.y.z.mcps.golem.cloud")));
    }

    #[test]
    fn restricted_arbitrary_rejects_bare_base_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(!domain_matcher.domain_available_to_provision(&domain("apps.golem.cloud")));
        assert!(!domain_matcher.domain_available_to_provision(&domain("mcps.golem.cloud")));
    }

    #[test]
    fn restricted_arbitrary_rejects_different_base() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(!domain_matcher.domain_available_to_provision(&domain("myapp.other.com")));
    }

    #[test]
    fn restricted_arbitrary_escapes_regex_special_chars_in_base() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(!domain_matcher.domain_available_to_provision(&domain("myapp.appsXgolemYcloud")));
        assert!(!domain_matcher.domain_available_to_provision(&domain("mymcp.mcpsXgolemYcloud")));
    }

    #[test]
    fn http_api_valid_unrestricted() {
        let domain_matcher = unrestricted();
        assert!(domain_matcher.domain_valid_for_http_api(&domain("myapp.apps.golem.cloud")));
        assert!(domain_matcher.domain_valid_for_http_api(&domain("mymcp.mcps.golem.cloud")));
        assert!(domain_matcher.domain_valid_for_http_api(&domain("anything.example.com")));
    }

    #[test]
    fn http_api_valid_apps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_matcher.domain_valid_for_http_api(&domain("myapp.apps.golem.cloud")));
    }

    #[test]
    fn http_api_invalid_mcps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_valid_for_http_api(&domain("mymcp.mcps.golem.cloud")));
    }

    #[test]
    fn http_api_invalid_unrelated_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_valid_for_http_api(&domain("myapp.other.com")));
    }

    #[test]
    fn http_api_valid_apps_deep_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_matcher.domain_valid_for_http_api(&domain("a.b.apps.golem.cloud")));
    }

    #[test]
    fn http_api_invalid_apps_deep_subdomain_when_arbitrary_disallowed() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_valid_for_http_api(&domain("a.b.apps.golem.cloud")));
    }

    #[test]
    fn mcp_valid_unrestricted() {
        let domain_matcher = unrestricted();
        assert!(domain_matcher.domain_valid_for_mcp(&domain("mymcp.mcps.golem.cloud")));
        assert!(domain_matcher.domain_valid_for_mcp(&domain("myapp.apps.golem.cloud")));
        assert!(domain_matcher.domain_valid_for_mcp(&domain("anything.example.com")));
    }

    #[test]
    fn mcp_valid_mcps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(domain_matcher.domain_valid_for_mcp(&domain("mymcp.mcps.golem.cloud")));
    }

    #[test]
    fn mcp_invalid_apps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_valid_for_mcp(&domain("myapp.apps.golem.cloud")));
    }

    #[test]
    fn mcp_invalid_unrelated_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_valid_for_mcp(&domain("myapp.other.com")));
    }

    #[test]
    fn mcp_valid_mcps_deep_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(domain_matcher.domain_valid_for_mcp(&domain("a.b.mcps.golem.cloud")));
    }

    #[test]
    fn mcp_invalid_mcps_deep_subdomain_when_arbitrary_disallowed() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(!domain_matcher.domain_valid_for_mcp(&domain("a.b.mcps.golem.cloud")));
    }
}
