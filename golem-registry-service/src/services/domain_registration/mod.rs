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
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, DomainLabel, DomainNamePattern,
    EnvironmentDomainRegistrationResourcePattern, EnvironmentDomainRegistrationVerb,
    PermissionTarget,
};
use golem_common::model::domain_registration::{
    Domain, DomainRegistration, DomainRegistrationCreation, DomainRegistrationId,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
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

        authorize_domain_registration_permission(
            auth,
            &environment,
            Some(&data.domain),
            EnvironmentDomainRegistrationVerb::Create,
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
            ImmutableAuditFields::new(auth.actor_account_id().0),
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
        let (domain_registration, environment) = self
            .get_by_id_with_environment(domain_registration_id, auth)
            .await?;

        authorize_domain_registration_permission(
            auth,
            &environment,
            Some(&domain_registration.domain),
            EnvironmentDomainRegistrationVerb::Delete,
        )?;

        let deleted_record = self
            .domain_registration_repo
            .delete(domain_registration_id.0, auth.actor_account_id().0)
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
        authorize_domain_registration_permission(
            auth,
            environment,
            Some(domain),
            EnvironmentDomainRegistrationVerb::View,
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

        authorize_domain_registration_permission(
            auth,
            &environment,
            None,
            EnvironmentDomainRegistrationVerb::View,
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

        authorize_domain_registration_permission(
            auth,
            &environment,
            Some(&domain_registration.domain),
            EnvironmentDomainRegistrationVerb::View,
        )
        .map_err(|_| DomainRegistrationError::DomainRegistrationNotFound(domain_registration_id))?;

        Ok((domain_registration, environment))
    }

    pub fn validate_domain_for_http_api(
        &self,
        domain: &Domain,
    ) -> Result<(), DomainRegistrationError> {
        self.domain_matcher.validate_domain_for_http_api(domain)
    }

    pub fn validate_domain_for_mcp(&self, domain: &Domain) -> Result<(), DomainRegistrationError> {
        self.domain_matcher.validate_domain_for_mcp(domain)
    }
}

enum DomainMatcher {
    Unrestricted,
    Restricted {
        golem_apps_domain: String,
        golem_mcps_domain: String,
        valid_http_domain_regex: Regex,
        valid_mcp_domain_regex: Regex,
        allow_arbitrary_subdomains: bool,
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
                    golem_apps_domain: restricted.golem_apps_domain.clone(),
                    golem_mcps_domain: restricted.golem_mcps_domain.clone(),
                    valid_http_domain_regex: make_regex(&restricted.golem_apps_domain)?,
                    valid_mcp_domain_regex: make_regex(&restricted.golem_mcps_domain)?,
                    allow_arbitrary_subdomains: restricted.allow_arbitrary_subdomains,
                })
            }
        }
    }

    fn validate_domain_for_http_api(&self, domain: &Domain) -> Result<(), DomainRegistrationError> {
        match self {
            Self::Unrestricted => Ok(()),
            Self::Restricted {
                valid_http_domain_regex,
                golem_apps_domain,
                allow_arbitrary_subdomains,
                ..
            } => {
                if valid_http_domain_regex.is_match(&domain.0) {
                    Ok(())
                } else {
                    Err(DomainRegistrationError::DomainNotValidForHttpApi {
                        domain: domain.clone(),
                        available_domain: golem_apps_domain.clone(),
                        allow_arbitrary_subdomains: *allow_arbitrary_subdomains,
                    })
                }
            }
        }
    }

    fn validate_domain_for_mcp(&self, domain: &Domain) -> Result<(), DomainRegistrationError> {
        match self {
            Self::Unrestricted => Ok(()),
            Self::Restricted {
                valid_mcp_domain_regex,
                golem_mcps_domain,
                allow_arbitrary_subdomains,
                ..
            } => {
                if valid_mcp_domain_regex.is_match(&domain.0) {
                    Ok(())
                } else {
                    Err(DomainRegistrationError::DomainNotValidForMcp {
                        domain: domain.clone(),
                        available_domain: golem_mcps_domain.clone(),
                        allow_arbitrary_subdomains: *allow_arbitrary_subdomains,
                    })
                }
            }
        }
    }

    fn domain_available_to_provision(&self, domain: &Domain) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Restricted {
                valid_http_domain_regex: golem_apps_domain_regex,
                valid_mcp_domain_regex: golem_mcps_domain_regex,
                ..
            } => {
                golem_apps_domain_regex.is_match(&domain.0)
                    || golem_mcps_domain_regex.is_match(&domain.0)
            }
        }
    }
}

fn authorize_domain_registration_permission(
    auth: &AuthCtx,
    environment: &Environment,
    domain: Option<&Domain>,
    verb: EnvironmentDomainRegistrationVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentDomainRegistration(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: EnvironmentOwnerPattern::Environment {
                account: environment.owner_account_id.to_string(),
                application: environment.application_name.0.clone(),
                environment: environment.name.0.clone(),
            },
            resource: domain
                .map(|domain| {
                    EnvironmentDomainRegistrationResourcePattern::Domain(DomainNamePattern {
                        labels: domain
                            .0
                            .split('.')
                            .map(|label| DomainLabel(label.to_string()))
                            .collect(),
                    })
                })
                .unwrap_or(EnvironmentDomainRegistrationResourcePattern::Any),
        },
    ))
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
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("myapp.apps.golem.cloud"))
                .is_ok()
        );
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("mymcp.mcps.golem.cloud"))
                .is_ok()
        );
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("anything.example.com"))
                .is_ok()
        );
    }

    #[test]
    fn http_api_valid_apps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("myapp.apps.golem.cloud"))
                .is_ok()
        );
    }

    #[test]
    fn http_api_invalid_mcps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("mymcp.mcps.golem.cloud"))
                .is_err()
        );
    }

    #[test]
    fn http_api_invalid_unrelated_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("myapp.other.com"))
                .is_err()
        );
    }

    #[test]
    fn http_api_valid_apps_deep_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("a.b.apps.golem.cloud"))
                .is_ok()
        );
    }

    #[test]
    fn http_api_invalid_apps_deep_subdomain_when_arbitrary_disallowed() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_http_api(&domain("a.b.apps.golem.cloud"))
                .is_err()
        );
    }

    #[test]
    fn mcp_valid_unrestricted() {
        let domain_matcher = unrestricted();
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("mymcp.mcps.golem.cloud"))
                .is_ok()
        );
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("myapp.apps.golem.cloud"))
                .is_ok()
        );
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("anything.example.com"))
                .is_ok()
        );
    }

    #[test]
    fn mcp_valid_mcps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("mymcp.mcps.golem.cloud"))
                .is_ok()
        );
    }

    #[test]
    fn mcp_invalid_apps_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("myapp.apps.golem.cloud"))
                .is_err()
        );
    }

    #[test]
    fn mcp_invalid_unrelated_domain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("myapp.other.com"))
                .is_err()
        );
    }

    #[test]
    fn mcp_valid_mcps_deep_subdomain() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", true);
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("a.b.mcps.golem.cloud"))
                .is_ok()
        );
    }

    #[test]
    fn mcp_invalid_mcps_deep_subdomain_when_arbitrary_disallowed() {
        let domain_matcher = restricted("apps.golem.cloud", "mcps.golem.cloud", false);
        assert!(
            domain_matcher
                .validate_domain_for_mcp(&domain("a.b.mcps.golem.cloud"))
                .is_err()
        );
    }
}
