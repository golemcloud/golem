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

use golem_common::model::authority::{
    AccountTarget, ApplicationTarget, EnvironmentTarget, Grantors, Operation, OperationShapeError,
    Target,
};
use golem_common::model::card::PermissionTarget;
use golem_common::model::golem_issuer::GolemIssuer;
use golem_service_base::model::auth::audit::DecisionAudit;
use golem_service_base::model::auth::surface::Surface;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError, decide_with_todays_checks};
use tracing::error;

/// Decides the registry's operations that the decision catalog names, from the card check that
/// each call site makes today, so that the move to `decide` changes no answer. Each decision is
/// audited; the registry decides only for its REST API.
#[derive(Clone, Debug)]
pub struct Decisions {
    golem_issuer: GolemIssuer,
    audit: DecisionAudit,
}

impl Decisions {
    pub fn new(golem_issuer: GolemIssuer, audit: DecisionAudit) -> Self {
        Self {
            golem_issuer,
            audit,
        }
    }

    /// Decides `operation` for the caller of `auth` from today's `check`. A denial is the error
    /// that today's check gives. An operation that the catalog refuses is a defect of its call
    /// site: it is logged, and today's check answers, without an audit record.
    pub async fn decide(
        &self,
        auth: &AuthCtx,
        operation: Result<Operation, OperationShapeError>,
        check: PermissionTarget,
    ) -> Result<Grantors, AuthorizationError> {
        self.decide_checks(auth, operation, vec![check]).await
    }

    /// Decides `operation`, whose one part today's code checks with each of `checks` in turn. A
    /// denial is the error of the first check that refuses, as today.
    pub async fn decide_checks(
        &self,
        auth: &AuthCtx,
        operation: Result<Operation, OperationShapeError>,
        checks: Vec<PermissionTarget>,
    ) -> Result<Grantors, AuthorizationError> {
        match operation {
            Ok(operation) => {
                let request = auth.entry_request(self.golem_issuer.to_string(), operation);
                decide_with_todays_checks(
                    auth,
                    request,
                    vec![checks.into()],
                    &self.audit,
                    Surface::Rest,
                )
                .await
            }
            Err(shape) => {
                error!("Registry operation has no valid decision request: {shape}");
                for check in &checks {
                    auth.authorize_permission(check)?;
                }
                Ok(Grantors::default())
            }
        }
    }
}

pub fn account_target(account_id: golem_common::model::account::AccountId) -> Target {
    Target::Account(AccountTarget { account_id })
}

pub fn application_target(
    account_id: golem_common::model::account::AccountId,
    application_id: golem_common::model::application::ApplicationId,
) -> Target {
    Target::Application(ApplicationTarget {
        account_id,
        application_id,
    })
}

pub fn environment_target(
    account_id: golem_common::model::account::AccountId,
    application_id: golem_common::model::application::ApplicationId,
    environment_id: golem_common::model::environment::EnvironmentId,
) -> Target {
    Target::Environment(EnvironmentTarget {
        account_id,
        application_id,
        environment_id,
    })
}

#[cfg(test)]
mod tests {
    use super::{account_target, application_target, environment_target};
    use crate::services::{
        account, agent_secret, application, card, deployment, domain_registration, environment,
        reports, token,
    };
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent_secret::CanonicalAgentSecretPath;
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::TokenId;
    use golem_common::model::authority::{
        CatalogOperation, Operation, OperationShapeError, Resource,
    };
    use golem_common::model::card::owner::EnvironmentOwnerPattern;
    use golem_common::model::card::{
        AccountTokenResourcePattern, AccountTokenVerb, AccountVerb, ApplicationVerb, CardId,
        CardVerb, EnvironmentAgentSecretVerb, EnvironmentDomainRegistrationVerb, EnvironmentVerb,
        PermissionTarget, SystemVerb, check_belongs_to_part,
    };
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{
        Environment, EnvironmentId, EnvironmentName, EnvironmentRevision,
    };
    use test_r::test;

    fn test_environment() -> Environment {
        Environment {
            id: EnvironmentId::new(),
            revision: EnvironmentRevision::INITIAL,
            application_id: ApplicationId::new(),
            application_name: ApplicationName::try_from("app").unwrap(),
            name: EnvironmentName::try_from("dev").unwrap(),
            diff_model_version: 0,
            compatibility_check: false,
            tool_compatibility_mode: Default::default(),
            version_check: false,
            security_overrides: false,
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            current_deployment: None,
        }
    }

    /// Each operation that a call site of the registry decides, with the check that the call site
    /// passes for it today. A call site that decides a new operation adds its pair here.
    fn call_site_pairs() -> Vec<(
        &'static str,
        Result<Operation, OperationShapeError>,
        PermissionTarget,
    )> {
        let environment = test_environment();
        let email = environment.owner_account_email.clone();
        let account = account_target(environment.owner_account_id);
        let application =
            application_target(environment.owner_account_id, environment.application_id);
        let environment_of = || {
            environment_target(
                environment.owner_account_id,
                environment.application_id,
                environment.id,
            )
        };
        let owner = EnvironmentOwnerPattern::Environment {
            account: email.clone(),
            application: environment.application_name.clone(),
            environment: environment.name.clone(),
        };
        let token_id = TokenId::new();
        let card_id = CardId::new();
        let single = Operation::single;

        let mut pairs = vec![
            (
                "update an account",
                single(
                    CatalogOperation::UpdateAccount,
                    Resource::None,
                    account.clone(),
                ),
                account::account_permission_target(&email, AccountVerb::Update),
            ),
            (
                "create an account",
                single(
                    CatalogOperation::CreateAccount,
                    Resource::None,
                    account.clone(),
                ),
                account::system_permission_target(SystemVerb::CreateAccount),
            ),
            (
                "create an application",
                single(
                    CatalogOperation::CreateApplication,
                    Resource::None,
                    account.clone(),
                ),
                application::application_permission_target(
                    &email,
                    &environment.application_name,
                    ApplicationVerb::Create,
                ),
            ),
            (
                "create an environment",
                single(
                    CatalogOperation::CreateEnvironment,
                    Resource::None,
                    application,
                ),
                environment::environment_permission_target(
                    &email,
                    &environment.application_name,
                    &environment.name,
                    EnvironmentVerb::Create,
                ),
            ),
            (
                "deploy an environment",
                single(
                    CatalogOperation::DeployEnvironment,
                    Resource::None,
                    environment_of(),
                ),
                deployment::environment_check(&environment, EnvironmentVerb::Deploy),
            ),
            (
                "register a host",
                single(
                    CatalogOperation::RegisterHost,
                    Resource::Name("api.example.com".to_string()),
                    environment_of(),
                ),
                domain_registration::domain_registration_check(
                    owner.clone(),
                    Some(&Domain("api.example.com".to_string())),
                    EnvironmentDomainRegistrationVerb::Create,
                ),
            ),
            (
                "view an API token, list the API tokens",
                single(
                    CatalogOperation::ViewApiToken,
                    Resource::ApiToken(token_id),
                    account.clone(),
                ),
                token::account_token_permission_target(
                    &email,
                    AccountTokenVerb::View,
                    AccountTokenResourcePattern::Token(token_id),
                ),
            ),
            (
                "create an API token",
                single(
                    CatalogOperation::CreateApiTokens,
                    Resource::None,
                    account.clone(),
                ),
                token::account_token_permission_target(
                    &email,
                    AccountTokenVerb::Create,
                    AccountTokenResourcePattern::Any,
                ),
            ),
            (
                "revoke an API token, its view check",
                single(
                    CatalogOperation::RevokeApiToken,
                    Resource::ApiToken(token_id),
                    account.clone(),
                ),
                token::account_token_permission_target(
                    &email,
                    AccountTokenVerb::View,
                    AccountTokenResourcePattern::Token(token_id),
                ),
            ),
            (
                "revoke an API token, its delete check",
                single(
                    CatalogOperation::RevokeApiToken,
                    Resource::ApiToken(token_id),
                    account.clone(),
                ),
                token::account_token_permission_target(
                    &email,
                    AccountTokenVerb::Delete,
                    AccountTokenResourcePattern::Token(token_id),
                ),
            ),
            (
                "get a card",
                single(
                    CatalogOperation::InspectCard,
                    Resource::Card(card_id),
                    account.clone(),
                ),
                card::card_check(&email, CardVerb::Inspect),
            ),
            (
                "revoke a card",
                single(
                    CatalogOperation::RevokeCard,
                    Resource::Card(card_id),
                    account.clone(),
                ),
                card::card_check(&email, CardVerb::Revoke),
            ),
            (
                "view the account summaries report",
                single(
                    CatalogOperation::ViewAccountSummariesReport,
                    Resource::None,
                    account.clone(),
                ),
                reports::system_permission_target(SystemVerb::ViewAccountSummariesReport),
            ),
            (
                "view the account counts report",
                single(
                    CatalogOperation::ViewAccountCountsReport,
                    Resource::None,
                    account,
                ),
                reports::system_permission_target(SystemVerb::ViewAccountCountsReport),
            ),
        ];

        // The secret call sites pass these verbs; each one decides the operation of its verb.
        let key = CanonicalAgentSecretPath(vec!["db".to_string(), "password".to_string()]);
        for (site, verb) in [
            ("create a secret", EnvironmentAgentSecretVerb::Create),
            ("update a secret", EnvironmentAgentSecretVerb::Update),
            ("delete a secret", EnvironmentAgentSecretVerb::Delete),
            (
                "get a secret, list the secrets",
                EnvironmentAgentSecretVerb::View,
            ),
        ] {
            pairs.push((
                site,
                single(
                    agent_secret::secret_operation(verb),
                    Resource::Path(key.0.join("/")),
                    environment_of(),
                ),
                agent_secret::agent_secret_check(owner.clone(), &key, verb),
            ));
        }
        pairs
    }

    /// A check that is not a check of its part would deny every caller of its call site.
    #[test]
    fn every_call_site_check_belongs_to_its_part() {
        for (site, operation, check) in call_site_pairs() {
            let operation = operation.unwrap_or_else(|error| panic!("{site}: {error}"));
            for part in operation.parts() {
                assert!(
                    check_belongs_to_part(part.verb, &check),
                    "{site}: {check:?} is not a check of {:?}",
                    part.verb
                );
            }
        }
    }

    /// A revoke is one decision with today's two checks. A caller that may not see the token
    /// gets "token not found", as for an unknown token; one that may see it but not revoke it
    /// gets a 403; one that may do both is allowed. These are today's answers.
    /// An audit sink that stores nothing and never fails, for the decision tests.
    struct KeepsNoRecord;

    #[async_trait::async_trait]
    impl golem_service_base::model::auth::audit::AuditSink for KeepsNoRecord {
        async fn record(
            &self,
            _record: golem_service_base::model::auth::audit::AuditRecord,
        ) -> Result<(), golem_service_base::model::auth::audit::AuditSinkError> {
            Ok(())
        }
    }

    #[test]
    async fn a_revoke_is_one_decision_with_todays_answers() {
        use crate::services::token::{TokenError, decide_revoke};
        use golem_common::model::card::recipient::RecipientPattern;
        use golem_common::model::card::{EffectiveSurface, parse_permission};
        use golem_service_base::model::auth::AuthCtx;

        let account_id = AccountId::new();
        let email = AccountEmail::new("acme");
        let token_id = TokenId::new();
        let decisions = super::Decisions::new(
            "https://golem.test".parse().unwrap(),
            golem_service_base::model::auth::audit::DecisionAudit::new(
                std::sync::Arc::new(KeepsNoRecord),
                std::time::Duration::from_secs(5),
            ),
        );
        let view = format!("account.token(acme) @ acme : view : {token_id}");
        let delete = format!("account.token(acme) @ acme : delete : {token_id}");
        let today = |auth: &AuthCtx| {
            let check = |verb| {
                token::account_token_permission_target(
                    &email,
                    verb,
                    AccountTokenResourcePattern::Token(token_id),
                )
            };
            if auth
                .authorize_permission(&check(AccountTokenVerb::View))
                .is_err()
            {
                "not found"
            } else if auth
                .authorize_permission(&check(AccountTokenVerb::Delete))
                .is_err()
            {
                "forbidden"
            } else {
                "allowed"
            }
        };
        for (grants, expected) in [
            (vec![], "not found"),
            (vec![view.clone()], "forbidden"),
            (vec![view.clone(), delete.clone()], "allowed"),
        ] {
            let grants = grants
                .iter()
                .map(|grant| parse_permission(grant).unwrap())
                .collect::<Vec<_>>();
            let surface = EffectiveSurface::from_grants(
                &grants,
                &[],
                &[],
                &[],
                &RecipientPattern::parse("acme").unwrap(),
            )
            .unwrap();
            let auth = AuthCtx::agent_with_effective_surface(account_id, email.clone(), surface);
            let answer = match decide_revoke(&decisions, &auth, account_id, &email, token_id).await
            {
                Ok(()) => "allowed",
                Err(TokenError::TokenNotFound(id)) if id == token_id => "not found",
                Err(TokenError::Unauthorized(_)) => "forbidden",
                Err(other) => panic!("{other}"),
            };
            assert_eq!(answer, expected, "{grants:?}");
            assert_eq!(answer, today(&auth), "{grants:?}");
        }
    }
}
