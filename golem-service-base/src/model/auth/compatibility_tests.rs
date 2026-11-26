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

use super::*;
use golem_common::base_model::caller::{
    AccountAuthentication, AccountPrincipal, Caller, Principal,
};
use golem_common::base_model::{AgentFingerprint, AgentId};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::authority::{
    AgentTarget, AuthorizationRequest, CatalogOperation, Decision, Denial, DesignatedKind,
    Operation, Resource, Target,
};
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, ClassPermissionTarget, EffectiveSurface, PartChecks,
    PermissionTarget, parse_permission,
};
use golem_common::model::environment::EnvironmentId;
use test_r::test;
use uuid::Uuid;

const OWNER: &str = "acme/shop/prod/cart";

fn agent(n: u128) -> AgentTarget {
    AgentTarget {
        environment_id: EnvironmentId(Uuid::from_u128(0xe1)),
        agent_type: AgentTypeName("Cart".to_string()),
        agent_id: AgentId::from_parts(7, n),
        fingerprint: Some(AgentFingerprint(Uuid::from_u128(0xf0 + n))),
    }
}

fn agent_check(owner: &str, verb: AgentVerb, resource: AgentResourcePattern) -> PermissionTarget {
    PermissionTarget::Agent(ClassPermissionTarget {
        owner: AgentOwnerPattern::parse(owner).unwrap(),
        verb: Some(verb),
        resource,
    })
}

/// The 1.6 check `agent : <verb> : <resource>` on agent `n`.
fn check(n: u128, verb: AgentVerb, resource: AgentResourcePattern) -> PermissionTarget {
    agent_check(
        &format!("{OWNER}/{}", AgentId::from_parts(7, n)),
        verb,
        resource,
    )
}

fn method(name: &str) -> AgentResourcePattern {
    AgentResourcePattern::Method(golem_common::model::card::AgentMethodName(name.to_string()))
}

fn user(grants: &[&str]) -> AuthCtx {
    let grants = grants
        .iter()
        .map(|text| parse_permission(text).unwrap())
        .collect::<Vec<_>>();
    let surface = EffectiveSurface::from_grants(
        &grants,
        &[],
        &[],
        &[],
        &RecipientPattern::parse("acme").unwrap(),
    )
    .unwrap();
    AuthCtx::agent_with_effective_surface(
        AccountId(Uuid::from_u128(1)),
        golem_common::model::account::AccountEmail::new("acme@example.com".to_string()),
        surface,
    )
}

fn caller() -> Caller {
    Caller::entry(Principal::Account(AccountPrincipal {
        issuer: "golem".to_string(),
        account_id: AccountId(Uuid::from_u128(1)),
        authentication: AccountAuthentication::ApiToken,
    }))
}

fn request(
    operation: CatalogOperation,
    resource: Resource,
    target: AgentTarget,
) -> AuthorizationRequest {
    AuthorizationRequest::new(
        caller(),
        Operation::single(operation, resource, Target::Agent(target)).unwrap(),
    )
}

fn invoke(n: u128) -> AuthorizationRequest {
    request(
        CatalogOperation::InvokeAgent,
        Resource::Method("increment".to_string()),
        agent(n),
    )
}

#[test]
async fn the_system_context_is_allowed_without_checks() {
    let authorizer =
        CompatibilityAuthorizer::new(AuthCtx::System, vec![PartChecks::Checks(Vec::new())]);
    assert!(matches!(
        authorizer.decide(&invoke(1)).await,
        Decision::Allow(_)
    ));
}

#[test]
async fn decide_answers_from_the_checks_of_the_call_site() {
    let auth = user(&["agent(acme/shop/prod/cart/*) @ acme : invoke : increment"]);
    let allowed = CompatibilityAuthorizer::new(
        auth.clone(),
        vec![vec![check(1, AgentVerb::Invoke, method("increment"))].into()],
    );
    assert!(matches!(
        allowed.decide(&invoke(1)).await,
        Decision::Allow(_)
    ));

    let denied = CompatibilityAuthorizer::new(
        auth,
        vec![vec![check(1, AgentVerb::Invoke, method("decrement"))].into()],
    );
    assert_eq!(
        denied.decide(&invoke(1)).await,
        Decision::Deny(Denial::LooksUnknown {
            kind: DesignatedKind::Agent
        })
    );
}

#[test]
async fn filter_keeps_the_rows_whose_checks_allow_and_drops_rows_without_checks() {
    let auth = user(&[&format!(
        "agent({OWNER}/{}) @ acme : view :",
        AgentId::from_parts(7, 1)
    )]);
    let authorizer = CompatibilityAuthorizer::new(auth, vec![]).with_row_checks([
        (
            Target::Agent(agent(1)),
            vec![check(1, AgentVerb::View, AgentResourcePattern::Empty)],
        ),
        (
            Target::Agent(agent(2)),
            vec![check(2, AgentVerb::View, AgentResourcePattern::Empty)],
        ),
    ]);
    let listing = AuthorizationRequest::new(
        caller(),
        Operation::single(
            CatalogOperation::ViewAgent,
            Resource::None,
            Target::Agent(agent(1)),
        )
        .unwrap(),
    );
    let kept = authorizer
        .filter(
            &listing,
            vec![
                Target::Agent(agent(1)),
                Target::Agent(agent(2)),
                Target::Agent(agent(3)),
            ],
        )
        .await;
    assert_eq!(kept, vec![Target::Agent(agent(1))]);
}

#[test]
async fn scope_names_the_agent_types_whose_checks_allow() {
    let auth = user(&[&format!("agent({OWNER}/*) @ acme : view :")]);
    let authorizer = CompatibilityAuthorizer::new(auth, vec![]).with_type_checks([
        (
            AgentTypeName("Cart".to_string()),
            vec![agent_check(
                &format!("{OWNER}/*"),
                AgentVerb::View,
                AgentResourcePattern::Empty,
            )],
        ),
        (
            AgentTypeName("Billing".to_string()),
            vec![agent_check(
                "acme/shop/prod/billing/*",
                AgentVerb::View,
                AgentResourcePattern::Empty,
            )],
        ),
    ]);
    let listing = AuthorizationRequest::new(
        caller(),
        Operation::single(
            CatalogOperation::ListAgents,
            Resource::None,
            Target::Environment(golem_common::model::authority::EnvironmentTarget {
                account_id: AccountId(Uuid::from_u128(1)),
                application_id: golem_common::model::application::ApplicationId(Uuid::from_u128(2)),
                environment_id: EnvironmentId(Uuid::from_u128(0xe1)),
            }),
        )
        .unwrap(),
    );
    let scope = authorizer.scope(&listing).await;
    assert!(!scope.whole_environment);
    assert_eq!(scope.agent_types, vec![AgentTypeName("Cart".to_string())]);
}

#[test]
fn the_entry_caller_is_the_account_in_both_roles() {
    assert_eq!(user(&[]).entry_caller("golem"), caller());
}

#[test]
async fn under_impersonation_the_admin_is_the_caller_and_the_target_account_decides() {
    let admin = AccountId(Uuid::from_u128(9));
    let surface = EffectiveSurface::from_grants(
        &[parse_permission("agent(acme/shop/prod/cart/*) @ acme : invoke : increment").unwrap()],
        &[],
        &[],
        &[],
        &RecipientPattern::parse("acme").unwrap(),
    )
    .unwrap();
    let auth = AuthCtx::admin_impersonation(
        admin,
        AccountId(Uuid::from_u128(1)),
        golem_common::model::account::AccountEmail::new("acme@example.com".to_string()),
        std::collections::BTreeSet::new(),
        golem_common::model::plan::PlanId(Uuid::from_u128(3)),
        surface,
    );

    let operation = invoke(1).operation;
    let request = auth.entry_request("golem", operation.clone());
    assert_eq!(
        request.caller,
        Caller::entry(Principal::Account(AccountPrincipal {
            issuer: "golem".to_string(),
            account_id: admin,
            authentication: AccountAuthentication::ApiToken,
        }))
    );
    assert!(request.principal.same_record(&caller().immediate));
    // The caller that the request passes on to the agent stays the target account.
    assert_eq!(auth.entry_caller("golem"), caller());
    let authorizer = CompatibilityAuthorizer::new(
        auth,
        vec![vec![check(1, AgentVerb::Invoke, method("increment"))].into()],
    );
    assert!(matches!(
        authorizer.decide(&request).await,
        Decision::Allow(_)
    ));
}

#[test]
fn without_impersonation_the_account_asks_on_its_own_authority() {
    let request = user(&[]).entry_request("golem", invoke(1).operation);
    assert_eq!(request.caller, caller());
    assert!(request.principal.same_record(&caller().immediate));
}

#[test]
async fn todays_error_is_the_error_of_the_first_refused_check() {
    let auth = user(&["agent(acme/shop/prod/cart/*) @ acme : invoke : increment"]);
    let refused = check(1, AgentVerb::Invoke, method("decrement"));
    let authorizer = CompatibilityAuthorizer::new(auth.clone(), vec![vec![refused.clone()].into()]);
    let request = invoke(1);
    assert!(matches!(
        authorizer.decide(&request).await,
        Decision::Deny(_)
    ));
    assert_eq!(
        authorizer.todays_error(&request).to_string(),
        auth.authorize_permission(&refused).unwrap_err().to_string()
    );
}

#[test]
async fn todays_error_names_a_check_that_does_not_belong_to_its_part() {
    let auth = user(&["agent(acme/shop/prod/cart/*) @ acme : * : *"]);
    let delete = check(1, AgentVerb::Delete, AgentResourcePattern::Empty);
    let authorizer = CompatibilityAuthorizer::new(auth, vec![vec![delete.clone()].into()]);
    assert_eq!(
        authorizer.todays_error(&invoke(1)).to_string(),
        AuthorizationError::PermissionNotAllowed(Box::new(delete)).to_string()
    );
}
