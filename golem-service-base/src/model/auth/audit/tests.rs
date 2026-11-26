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
use crate::model::auth::Authorizer;
use crate::model::auth::surface::Surface;
use assert2::{assert, let_assert};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::TokenId;
use golem_common::model::authority::{
    AgentTarget, AuthorizationRequest, CardTokenId, CatalogOperation, Decision, Denial,
    DesignatedKind, EnvironmentTarget, GrantorCard, Grantors, Operation, PresentedAuthority,
    PresentedToken, Resource, Scope, Target, ToolTarget,
};
use golem_common::model::caller::{
    AccountAuthentication, AccountPrincipal, Caller, Claims, Pair, Principal, PrincipalRef,
    TokenProvenance, UserPrincipal,
};
use golem_common::model::card::CardId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::ToolName;
use golem_common::model::{AgentFingerprint, AgentId, Timestamp};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::test;
use uuid::Uuid;

const ISSUER: &str = "https://golem.example";
const SECRET_TOKEN_VALUE: &str = "card-token-secret-value-4f7d";
const CLAIM_EMAIL: &str = "person@claims.example";

/// Answers every decision the same way.
struct FixedAuthorizer(Decision);

#[async_trait]
impl Authorizer for FixedAuthorizer {
    async fn decide(&self, _request: &AuthorizationRequest) -> Decision {
        self.0.clone()
    }

    async fn filter(&self, _request: &AuthorizationRequest, targets: Vec<Target>) -> Vec<Target> {
        targets
    }

    async fn scope(&self, _request: &AuthorizationRequest) -> Scope {
        Scope::default()
    }
}

/// Keeps every record it accepts.
#[derive(Default)]
struct RecordingSink(Mutex<Vec<AuditRecord>>);

impl RecordingSink {
    fn records(&self) -> Vec<AuditRecord> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuditSink for RecordingSink {
    async fn record(&self, record: AuditRecord) -> Result<(), AuditSinkError> {
        self.0.lock().unwrap().push(record);
        Ok(())
    }
}

/// Refuses every record, as a sink whose storage is down.
struct FailingSink;

#[async_trait]
impl AuditSink for FailingSink {
    async fn record(&self, _record: AuditRecord) -> Result<(), AuditSinkError> {
        Err(AuditSinkError::Unavailable("storage is down".to_string()))
    }
}

fn environment_id() -> EnvironmentId {
    EnvironmentId(Uuid::from_u128(0xe0))
}

fn agent_target() -> AgentTarget {
    AgentTarget {
        environment_id: environment_id(),
        agent_type: AgentTypeName("Report".to_string()),
        agent_id: AgentId::from_parts(7, 0x71),
        fingerprint: Some(AgentFingerprint(Uuid::from_u128(0xf1))),
    }
}

fn account(id: u128, authentication: AccountAuthentication) -> Principal {
    Principal::Account(AccountPrincipal {
        issuer: ISSUER.to_string(),
        account_id: AccountId(Uuid::from_u128(id)),
        authentication,
    })
}

fn person_with_claims() -> Principal {
    Principal::User(UserPrincipal {
        pair: Pair::new("https://idp.example/", "person-1"),
        claims: Claims::from_json(format!(r#"{{"sub":"person-1","email":"{CLAIM_EMAIL}"}}"#))
            .unwrap(),
        authenticated_at: "2026-11-24T10:00:00.000Z".parse::<Timestamp>().unwrap(),
    })
}

fn account_caller() -> Caller {
    Caller::entry(account(0xa, AccountAuthentication::ApiToken))
}

fn read_oplog(caller: Caller) -> AuthorizationRequest {
    AuthorizationRequest::new(
        caller,
        Operation::single(
            CatalogOperation::ReadAgentOplog,
            Resource::None,
            Target::Agent(agent_target()),
        )
        .unwrap(),
    )
}

fn invoke(caller: Caller) -> AuthorizationRequest {
    AuthorizationRequest::new(
        caller,
        Operation::single(
            CatalogOperation::InvokeAgent,
            Resource::Method("run".to_string()),
            Target::Agent(agent_target()),
        )
        .unwrap(),
    )
}

fn hosted_exec() -> Operation {
    Operation::hosted_exec(
        agent_target(),
        ToolTarget {
            environment_id: environment_id(),
            tool_name: ToolName::try_from("grep").unwrap(),
        },
        "grep".to_string(),
    )
    .unwrap()
}

fn grantors() -> Grantors {
    Grantors {
        cards: vec![GrantorCard {
            card_id: CardId(Uuid::from_u128(0xc1)),
            ancestors: vec![CardId(Uuid::from_u128(0xc0))],
            presented_by: Some(CardTokenId(Uuid::from_u128(0x7c))),
        }],
    }
}

fn looks_unknown() -> Decision {
    Decision::Deny(Denial::LooksUnknown {
        kind: DesignatedKind::Agent,
    })
}

fn rest() -> AuditContext {
    AuditContext::new(Surface::Rest).authenticated_by(TokenId(Uuid::from_u128(0x70)))
}

fn audited(decision: Decision, sink: Arc<dyn AuditSink>) -> AuditedAuthorizer {
    AuditedAuthorizer::new(
        Arc::new(FixedAuthorizer(decision)),
        sink,
        Duration::from_secs(5),
    )
}

/// The current value of one counter series in the default registry, or 0 before its first use.
fn counter(name: &str, labels: &[(&str, &str)]) -> f64 {
    prometheus::gather()
        .into_iter()
        .filter(|family| family.name() == name)
        .flat_map(|family| family.get_metric().to_vec())
        .find(|metric| {
            labels.iter().all(|(key, value)| {
                metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == *key && label.value() == *value)
            })
        })
        .map(|metric| metric.get_counter().value())
        .unwrap_or(0.0)
}

#[test]
async fn an_allowed_management_read_writes_one_record_before_it_answers() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(Decision::Allow(grantors()), sink.clone());

    let decision = authorizer
        .decide(&read_oplog(account_caller()), &rest())
        .await;

    assert!(decision == Ok(Decision::Allow(grantors())));
    let records = sink.records();
    assert!(records.len() == 1);
    assert!(records[0].outcome == AuditOutcome::Allowed);
    assert!(records[0].operation == "read-agent-oplog");
}

#[test]
async fn an_allowed_invocation_writes_no_record() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(Decision::Allow(grantors()), sink.clone());

    let decision = authorizer.decide(&invoke(account_caller()), &rest()).await;

    assert!(decision == Ok(Decision::Allow(grantors())));
    assert!(sink.records().is_empty());
}

#[test]
async fn a_denials_only_audit_writes_no_record_for_an_allowed_management_read() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(Decision::Allow(grantors()), sink.clone()).denials_only();

    let decision = authorizer
        .decide(&read_oplog(account_caller()), &rest())
        .await;

    assert!(decision == Ok(Decision::Allow(grantors())));
    assert!(sink.records().is_empty());
}

#[test]
async fn a_denials_only_audit_answers_an_allowed_read_also_when_the_sink_fails() {
    let authorizer = audited(Decision::Allow(grantors()), Arc::new(FailingSink)).denials_only();

    let decision = authorizer
        .decide(&read_oplog(account_caller()), &rest())
        .await;

    assert!(decision == Ok(Decision::Allow(grantors())));
}

#[test]
async fn a_denials_only_audit_still_records_a_denial() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(looks_unknown(), sink.clone()).denials_only();

    let decision = authorizer
        .decide(&read_oplog(account_caller()), &rest())
        .await;

    assert!(decision == Ok(looks_unknown()));
    let records = sink.records();
    assert!(records.len() == 1);
    assert!(records[0].outcome != AuditOutcome::Allowed);
}

#[test]
async fn a_denied_invocation_writes_one_record_with_the_real_target() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(looks_unknown(), sink.clone());

    let decision = authorizer.decide(&invoke(account_caller()), &rest()).await;

    assert!(decision == Ok(looks_unknown()));
    let records = sink.records();
    assert!(records.len() == 1);
    let record = &records[0];
    assert!(
        record.outcome
            == AuditOutcome::LooksUnknown {
                kind: "agent".to_string()
            }
    );
    assert!(record.failed_part == Some(0));
    let target = &record.parts[0].target;
    assert!(target.kind == "agent");
    assert!(target.id == agent_target().agent_id.as_uuid().to_string());
    assert!(target.name.as_deref() == Some("Report"));
    assert!(target.fingerprint == Some(AgentFingerprint(Uuid::from_u128(0xf1))));
    assert!(target.environment_id == Some(environment_id()));
}

#[test]
async fn a_record_has_every_field_of_the_contract() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(Decision::Allow(grantors()), sink.clone());
    let caller = Caller {
        original: account(0xa, AccountAuthentication::ApiToken),
        immediate: account(0xb, AccountAuthentication::SignIn),
        tokens: vec![TokenProvenance {
            id: Uuid::from_u128(0x7d),
            maker: Some(PrincipalRef::new(Pair::new(ISSUER, "account:1"))),
        }],
    };

    authorizer
        .decide(&read_oplog(caller), &rest())
        .await
        .unwrap();

    let record = sink.records().remove(0);
    assert!(record.version == AUDIT_RECORD_VERSION);
    assert!(!record.event_id.is_nil());
    assert!(record.surface == "rest");
    assert!(record.operation == "read-agent-oplog");
    assert!(record.parts.len() == 1);
    assert!(record.parts[0].class == "oplog");
    assert!(record.parts[0].verb == "read");
    // The acting pair is the immediate caller unless the request names another principal.
    let immediate_pair = format!(
        "{}",
        Pair::new(ISSUER, format!("account:{}", Uuid::from_u128(0xb)))
    );
    assert!(record.principal.pair.as_deref() == Some(immediate_pair.as_str()));
    assert!(record.caller.immediate.pair.as_deref() == Some(immediate_pair.as_str()));
    assert!(record.caller.immediate.authentication.as_deref() == Some("sign-in"));
    assert!(record.caller.original.kind == "account");
    assert!(record.caller.original.authentication.as_deref() == Some("api-token"));
    assert!(record.caller.tokens.len() == 1);
    assert!(record.caller.tokens[0].id == Uuid::from_u128(0x7d));
    assert!(record.authenticated_by == Some(TokenId(Uuid::from_u128(0x70))));
    assert!(record.grantors.len() == 1);
    assert!(record.grantors[0].card_id == CardId(Uuid::from_u128(0xc1)));
    assert!(record.grantors[0].ancestors == vec![CardId(Uuid::from_u128(0xc0))]);
    assert!(record.grantors[0].presented_by == Some(CardTokenId(Uuid::from_u128(0x7c))));
}

#[test]
async fn a_record_holds_no_presented_token_value_and_no_claim() {
    let sink = Arc::new(RecordingSink::default());
    let authorizer = audited(looks_unknown(), sink.clone());
    let request = invoke(Caller::entry(person_with_claims())).with_presented(
        PresentedAuthority::none().with_token(PresentedToken::new(SECRET_TOKEN_VALUE)),
    );

    authorizer.decide(&request, &rest()).await.unwrap();

    let json = serde_json::to_string(&sink.records()[0]).unwrap();
    assert!(!json.contains(SECRET_TOKEN_VALUE));
    assert!(!json.contains(CLAIM_EMAIL));
    assert!(
        json.contains("person-1"),
        "the pair still names the person: {json}"
    );
}

#[test]
async fn with_a_failing_sink_an_allowed_management_read_is_unavailable() {
    let authorizer = audited(Decision::Allow(grantors()), Arc::new(FailingSink));

    let decision = authorizer
        .decide(&read_oplog(account_caller()), &rest())
        .await;

    let_assert!(Err(unavailable) = decision);
    assert!(unavailable.retry_after == Duration::from_secs(5));
}

#[test]
async fn with_a_failing_sink_an_allowed_invocation_still_runs() {
    let authorizer = audited(Decision::Allow(grantors()), Arc::new(FailingSink));

    let decision = authorizer.decide(&invoke(account_caller()), &rest()).await;

    assert!(decision == Ok(Decision::Allow(grantors())));
}

#[test]
async fn with_a_failing_sink_a_denial_stays_a_denial_and_the_lost_record_is_counted() {
    let authorizer = audited(looks_unknown(), Arc::new(FailingSink));
    let context = AuditContext::new(Surface::Mcp);
    let lost_before = counter("golem_audit_records_lost_total", &[("surface", "mcp")]);

    let decision = authorizer.decide(&invoke(account_caller()), &context).await;

    assert!(decision == Ok(looks_unknown()));
    let lost_after = counter("golem_audit_records_lost_total", &[("surface", "mcp")]);
    assert!(lost_after == lost_before + 1.0);
}

#[test]
async fn a_denial_counts_by_the_class_of_its_failing_part_and_its_surface() {
    let authorizer = audited(
        Decision::Deny(Denial::Forbidden {
            missing_grant: None,
        }),
        Arc::new(RecordingSink::default()),
    );
    let request = AuthorizationRequest::new(account_caller(), hosted_exec());
    let context = AuditContext::new(Surface::Attach);
    let labels = [("class", "tool"), ("surface", "attach")];
    let before = counter("golem_authorization_denials_total", &labels);

    authorizer.decide(&request, &context).await.unwrap();

    assert!(counter("golem_authorization_denials_total", &labels) == before + 1.0);
}

#[test]
fn the_failing_part_of_a_hosted_exec_follows_the_shape_of_its_denial() {
    let operation = hosted_exec();

    let agent_denied = Denial::LooksUnknown {
        kind: DesignatedKind::Agent,
    };
    let tool_denied = Denial::Forbidden {
        missing_grant: None,
    };
    let sign_in = Denial::SignInRequired { scheme: None };

    assert!(failed_part(&operation, &agent_denied) == Some(0));
    assert!(failed_part(&operation, &tool_denied) == Some(1));
    assert!(failed_part(&operation, &sign_in) == None);
}

#[test]
fn a_one_part_operation_always_fails_on_its_part() {
    let operation = Operation::single(
        CatalogOperation::ListAgents,
        Resource::None,
        Target::Environment(EnvironmentTarget {
            account_id: AccountId(Uuid::from_u128(0xacc0)),
            application_id: ApplicationId(Uuid::from_u128(0xa0)),
            environment_id: environment_id(),
        }),
    )
    .unwrap();

    assert!(failed_part(&operation, &Denial::SignInRequired { scheme: None }) == Some(0));
}

#[test]
fn invocations_and_host_calls_are_recorded_only_when_denied() {
    for operation in [
        CatalogOperation::CreateAgent,
        CatalogOperation::InvokeAgent,
        CatalogOperation::UpsertAgent,
        CatalogOperation::HostedExec,
        CatalogOperation::SendToolInput,
        CatalogOperation::SendHostedExecInput,
        CatalogOperation::ReadInvocation,
        CatalogOperation::ReadToolFiles,
        CatalogOperation::FilesystemRead,
        CatalogOperation::NetworkConnect,
        CatalogOperation::RdbmsUnanalyzed,
    ] {
        assert!(!audited_when_allowed(operation), "{operation}");
    }
    for operation in [
        CatalogOperation::ViewAgent,
        CatalogOperation::InspectAgentConfig,
        CatalogOperation::InspectAgentEnvVars,
        CatalogOperation::InspectToolConfig,
        CatalogOperation::InspectToolEnvVars,
        CatalogOperation::ReadAgentOplog,
        CatalogOperation::InterruptAgent,
        CatalogOperation::ReadFile,
        CatalogOperation::DeleteSecret,
        CatalogOperation::RevokeApiToken,
        CatalogOperation::ViewInstallation,
        CatalogOperation::DeployEnvironment,
        CatalogOperation::ReadPackage,
        CatalogOperation::InspectCard,
    ] {
        assert!(audited_when_allowed(operation), "{operation}");
    }
    // 22 invocation operations and 23 host-function operations; every other operation is a
    // management read or control.
    let not_audited = CatalogOperation::ALL
        .iter()
        .filter(|operation| !audited_when_allowed(**operation))
        .count();
    assert!(not_audited == 45);
}

#[test]
async fn the_authorization_metrics_are_registered_with_the_golem_prefix() {
    let authorizer = audited(looks_unknown(), Arc::new(FailingSink));
    authorizer
        .decide(&invoke(account_caller()), &rest())
        .await
        .unwrap();

    let names = prometheus::gather()
        .into_iter()
        .map(|family| family.name().to_string())
        .collect::<std::collections::HashSet<_>>();
    for expected in [
        "golem_authorization_decisions_total",
        "golem_authorization_denials_total",
        "golem_audit_records_lost_total",
    ] {
        assert!(names.contains(expected), "missing metric {expected}");
    }
}
