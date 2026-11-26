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

//! The audit of authorization decisions.
//!
//! [`AuditedAuthorizer`] wraps the service's [`Authorizer`] and is the only producer of audit
//! records: no entry point writes one of its own. It records every denial, and every allowed
//! operation that is not an invocation or a host call (a management read or control). The record
//! of an allowed operation is written before the operation runs, so no data leaves Golem and no
//! control takes effect without its record. A denial stays a denial when its record cannot be
//! written; the lost record is counted.
//!
//! A listing of agents writes its record through `decide` on the listing operation, once for
//! each agent type that it scans; the rows that the caller may not see are dropped without a
//! record.
//!
//! Calls that never reach `decide` write no record:
//! - system calls;
//! - calls on an agent that has no type (the owner agent of a standalone tool call, an oplog
//!   processor), and the lookup of a standalone tool, which `authorize_typeless_owner_with_1_6_check`
//!   and the new owner agent's check answer with the 1.6 card model;
//! - the session-resume and Durable Streams attachment paths behind `AgentAccess::unclassified`,
//!   which keep today's check until their operations are named;
//! - the description of a component's baseline tool (`authorize_component_agents`), which names no
//!   agent, and tool calls whose tool name is not a valid tool name.

#[cfg(test)]
mod tests;

use crate::metrics::authorization as metrics;
use crate::model::auth::surface::Surface;
use crate::model::auth::{AuthCtx, Authorizer};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::AgentFingerprint;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::TokenId;
use golem_common::model::authority::{
    AuthorizationRequest, CardTokenId, CatalogOperation, Decision, Denial, DesignatedKind,
    Destination, GrantorCard, InvocationOwner, InvocationTarget, Operation, OperationPart, Owner,
    PromiseOwner, Resource, Scope, Target, TargetKind,
};
use golem_common::model::caller::{AccountAuthentication, Caller, Principal, TokenProvenance};
use golem_common::model::card::CardId;
use golem_common::model::environment::EnvironmentId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

/// The version of [`AuditRecord`]'s serialized form.
pub const AUDIT_RECORD_VERSION: u32 = 1;

/// Where a decision was asked, and the token that authenticated the request.
#[derive(Debug, Clone)]
pub struct AuditContext {
    pub surface: Surface,
    pub authenticated_by: Option<TokenId>,
}

impl AuditContext {
    pub fn new(surface: Surface) -> Self {
        Self {
            surface,
            authenticated_by: None,
        }
    }

    /// The context of a request that `auth` describes, asked at `surface`.
    pub fn for_request(auth: &AuthCtx, surface: Surface) -> Self {
        Self {
            surface,
            authenticated_by: auth.authenticated_by(),
        }
    }

    pub fn authenticated_by(mut self, token: TokenId) -> Self {
        self.authenticated_by = Some(token);
        self
    }
}

/// How a service audits its decisions: the sink that stores the records, and how long a client
/// waits before it retries an operation whose record could not be written.
#[derive(Clone)]
pub struct DecisionAudit {
    sink: Arc<dyn AuditSink>,
    retry_after: Duration,
    record_allowed: bool,
}

impl DecisionAudit {
    pub fn new(sink: Arc<dyn AuditSink>, retry_after: Duration) -> Self {
        Self {
            sink,
            retry_after,
            record_allowed: true,
        }
    }

    /// This audit, but it records only denials. For a decision about a target that does not
    /// exist: an allowed caller learns only that the target is missing, so nothing is read or
    /// changed, and no record is written.
    pub fn denials_only(&self) -> Self {
        Self {
            record_allowed: false,
            ..self.clone()
        }
    }

    /// `inner`, with every decision audited and counted.
    pub fn authorizer(&self, inner: Arc<dyn Authorizer>) -> AuditedAuthorizer {
        let authorizer = AuditedAuthorizer::new(inner, self.sink.clone(), self.retry_after);
        if self.record_allowed {
            authorizer
        } else {
            authorizer.denials_only()
        }
    }
}

impl std::fmt::Debug for DecisionAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionAudit")
            .field("retry_after", &self.retry_after)
            .field("record_allowed", &self.record_allowed)
            .finish_non_exhaustive()
    }
}

/// An allowed operation whose audit record could not be written. The operation must not run;
/// the caller may retry after `retry_after`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("audit unavailable, retry after {retry_after:?}")]
pub struct AuditUnavailable {
    pub retry_after: Duration,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AuditSinkError {
    #[error("audit storage unavailable: {0}")]
    Unavailable(String),
}

/// Stores audit records durably.
#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Returns only after the record is stored, or fails.
    async fn record(&self, record: AuditRecord) -> Result<(), AuditSinkError>;
}

/// The service's [`Authorizer`], with every decision audited and counted.
pub struct AuditedAuthorizer {
    inner: Arc<dyn Authorizer>,
    sink: Arc<dyn AuditSink>,
    retry_after: Duration,
    record_allowed: bool,
}

impl AuditedAuthorizer {
    pub fn new(
        inner: Arc<dyn Authorizer>,
        sink: Arc<dyn AuditSink>,
        retry_after: Duration,
    ) -> Self {
        Self {
            inner,
            sink,
            retry_after,
            record_allowed: true,
        }
    }

    /// This authorizer, but it records only denials (see [`DecisionAudit::denials_only`]). It
    /// still counts every decision.
    pub fn denials_only(mut self) -> Self {
        self.record_allowed = false;
        self
    }

    /// Decides the request and writes its audit record before it answers.
    pub async fn decide(
        &self,
        request: &AuthorizationRequest,
        context: &AuditContext,
    ) -> Result<Decision, AuditUnavailable> {
        let decision = self.inner.decide(request).await;
        count(&request.operation, &decision, context.surface);

        let needs_record = match &decision {
            Decision::Allow(_) => {
                self.record_allowed && audited_when_allowed(request.operation.name())
            }
            Decision::Deny(_) => true,
        };
        if !needs_record {
            return Ok(decision);
        }

        let record = AuditRecord::new(request, &decision, context);
        let event_id = record.event_id;
        match self.sink.record(record).await {
            Ok(()) => Ok(decision),
            Err(error) => match decision {
                Decision::Deny(_) => {
                    metrics::record_lost_audit_record(context.surface.label());
                    warn!(%event_id, %error, "The audit record of a denial was lost");
                    Ok(decision)
                }
                Decision::Allow(_) => {
                    warn!(
                        %event_id, %error,
                        "Refusing an allowed operation because its audit record cannot be written"
                    );
                    Err(AuditUnavailable {
                        retry_after: self.retry_after,
                    })
                }
            },
        }
    }

    pub async fn filter(
        &self,
        request: &AuthorizationRequest,
        targets: Vec<Target>,
    ) -> Vec<Target> {
        self.inner.filter(request, targets).await
    }

    pub async fn scope(&self, request: &AuthorizationRequest) -> Scope {
        self.inner.scope(request).await
    }
}

fn count(operation: &Operation, decision: &Decision, surface: Surface) {
    match decision {
        Decision::Allow(_) => {
            for part in operation.parts() {
                metrics::record_decision(part.class().name(), "allowed");
            }
        }
        Decision::Deny(denial) => {
            let parts = operation.parts();
            let class = failed_part(operation, denial)
                .and_then(|index| parts.get(index))
                .or(parts.first())
                .map(|part| part.class().name())
                .unwrap_or("unknown");
            metrics::record_decision(class, AuditOutcome::from(denial).label());
            metrics::record_denial(class, surface.label());
        }
    }
}

/// Whether an allowed operation needs an audit record. Invocations and host calls are recorded
/// only when they are denied; every other operation is a management read or control.
pub fn audited_when_allowed(operation: CatalogOperation) -> bool {
    use CatalogOperation::*;
    match operation {
        // Invocations.
        CreateAgent | InvokeAgent | UpsertAgent | InvokeTool | HostedExec
        | RetryAgentInvocation | RetryToolInvocation | ReadInvocation | CancelInvocation
        | SendInvocationInput | SendToolInput | SendHostedExecInput | ListInvocations
        | ReadTask | CancelTask | SendTaskInput | ReadStream | AppendStream | SendToPromise
        | SendToWebhook | ReadArtifact | ReadToolFiles => false,
        // Host calls.
        FilesystemRead
        | FilesystemWrite
        | FilesystemList
        | FilesystemStat
        | FilesystemDelete
        | NetworkConnect
        | ProveIdentity
        | HoldSecret
        | RevealSecret
        | HoldPrincipalSecret
        | RevealPrincipalSecret
        | ReserveQuota
        | KvRead
        | KvWrite
        | KvDelete
        | KvList
        | BlobRead
        | BlobWrite
        | BlobDelete
        | BlobList
        | RdbmsQuery
        | RdbmsMutate
        | RdbmsUnanalyzed => false,
        // Management reads and controls.
        ViewAgent
        | InspectAgentConfig
        | InspectAgentEnvVars
        | InspectToolConfig
        | InspectToolEnvVars
        | ReadAgentOplog
        | ReadToolInvocationOplog
        | ListAgents
        | DeleteAgent
        | InterruptAgent
        | ResumeAgent
        | UpdateAgentRevision
        | ForkAgent
        | RevertAgent
        | ActivateProcessor
        | DeactivateProcessor
        | SimulateCrash
        | ReplayAgent
        | InterruptToolInvocation
        | ResumeToolInvocation
        | ListAgentWebhooks
        | ListToolWebhooks
        | CloseAgentWebhook
        | CloseToolWebhook
        | LimitAgent
        | LimitTool
        | LimitAccount
        | AddVolume
        | RemoveVolume
        | ListFiles
        | ReadFile
        | ReadLiveFile
        | UnpublishArtifact
        | ListArtifacts => true,
        ViewSecret | CreateSecret | UpdateSecret | DeleteSecret | UpdatePrincipalSecret => true,
        CreateAccount
        | UpdateAccount
        | ConfigureSignIn
        | ViewAccountCountsReport
        | ViewAccountSummariesReport
        | CreateApiTokens
        | ListApiTokens
        | ViewApiToken
        | RevokeApiToken
        | SetOverrideExpiry
        | SetSpendMode
        | ViewInstallation
        | CreateApplication
        | CreateEnvironment
        | DeployEnvironment
        | RegisterHost
        | RegisterWildcardDomain
        | ListWildcardDomains
        | DeleteWildcardDomain
        | PublishDomain
        | PublishProtocol => true,
        PublishPackage | ReadPackage | DePublishPackage | RestorePackage | DeprecatePackage
        | AttachPackageCard => true,
        InstallCard | RevokeCard | InspectCard | ListCardTokens => true,
    }
}

/// The index of the part whose denial the decision answers, from the shape of the denial: a
/// denial that looks like an unknown ID belongs to the part with that kind of target, and a
/// `403` to the only part whose target is not such a kind. `None` when the shape fits several
/// parts.
pub fn failed_part(operation: &Operation, denial: &Denial) -> Option<usize> {
    let parts = operation.parts();
    if parts.len() == 1 {
        return Some(0);
    }
    match denial {
        Denial::LooksUnknown { kind } => parts
            .iter()
            .position(|part| part.target.kind().designated_kind() == Some(*kind)),
        Denial::Forbidden { .. } => {
            let mut candidates = parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part.target.kind().designated_kind().is_none());
            match (candidates.next(), candidates.next()) {
                (Some((index, _)), None) => Some(index),
                _ => None,
            }
        }
        Denial::SignInRequired { .. } => None,
    }
}

/// One audit record. It is its own serialized form: it never stores a request or a decision as
/// they are, so it holds no presented token value and no claim of any principal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    pub version: u32,
    /// Stable across retries of the same record.
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub surface: String,
    pub operation: String,
    /// The operation's parts, each with its target. For a file read, the part's resource is the
    /// path, and the principal is the reader.
    pub parts: Vec<AuditPart>,
    pub outcome: AuditOutcome,
    /// The part whose denial the outcome is, when its shape tells.
    pub failed_part: Option<usize>,
    /// The principal whose authority decided.
    pub principal: AuditPrincipal,
    pub caller: AuditCaller,
    /// The token that authenticated the request.
    pub authenticated_by: Option<TokenId>,
    /// The cards that covered an allowed operation.
    pub grantors: Vec<AuditGrantor>,
}

impl AuditRecord {
    pub fn new(
        request: &AuthorizationRequest,
        decision: &Decision,
        context: &AuditContext,
    ) -> Self {
        let operation = &request.operation;
        let (outcome, failed_part, grantors) = match decision {
            Decision::Allow(grantors) => (
                AuditOutcome::Allowed,
                None,
                grantors.cards.iter().map(AuditGrantor::from).collect(),
            ),
            Decision::Deny(denial) => (
                AuditOutcome::from(denial),
                failed_part(operation, denial),
                Vec::new(),
            ),
        };
        Self {
            version: AUDIT_RECORD_VERSION,
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            surface: context.surface.label().to_string(),
            operation: operation.name().name().to_string(),
            parts: operation.parts().iter().map(AuditPart::from).collect(),
            outcome,
            failed_part,
            principal: AuditPrincipal::from(&request.principal),
            caller: AuditCaller::from(&request.caller),
            authenticated_by: context.authenticated_by,
            grantors,
        }
    }

    /// The account of the first part's target, where the target or one of its owners names it.
    pub fn target_account_id(&self) -> Option<AccountId> {
        self.parts
            .first()
            .and_then(|part| part.target.find(|target| target.account_id))
    }

    /// The environment of the first part's target, where the target or one of its owners
    /// names it.
    pub fn target_environment_id(&self) -> Option<EnvironmentId> {
        self.parts
            .first()
            .and_then(|part| part.target.find(|target| target.environment_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "outcome"
)]
pub enum AuditOutcome {
    Allowed,
    SignInRequired,
    LooksUnknown { kind: String },
    Forbidden { missing_grant: Option<String> },
}

impl AuditOutcome {
    /// A stable, bounded label for metrics and the staging table.
    pub fn label(&self) -> &'static str {
        match self {
            AuditOutcome::Allowed => "allowed",
            AuditOutcome::SignInRequired => "sign-in-required",
            AuditOutcome::LooksUnknown { .. } => "looks-unknown",
            AuditOutcome::Forbidden { .. } => "forbidden",
        }
    }
}

impl From<&Denial> for AuditOutcome {
    fn from(denial: &Denial) -> Self {
        match denial {
            Denial::SignInRequired { .. } => AuditOutcome::SignInRequired,
            Denial::LooksUnknown { kind } => AuditOutcome::LooksUnknown {
                kind: designated_kind_label(*kind).to_string(),
            },
            Denial::Forbidden { missing_grant } => AuditOutcome::Forbidden {
                missing_grant: missing_grant.as_ref().map(|grant| grant.0.clone()),
            },
        }
    }
}

fn designated_kind_label(kind: DesignatedKind) -> &'static str {
    match kind {
        DesignatedKind::Agent => "agent",
        DesignatedKind::Invocation => "invocation",
        DesignatedKind::Task => "task",
        DesignatedKind::Stream => "stream",
        DesignatedKind::Promise => "promise",
        DesignatedKind::Webhook => "webhook",
        DesignatedKind::Artifact => "artifact",
    }
}

/// A principal by its pair, its kind and how it authenticated; never its claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditPrincipal {
    pub kind: String,
    /// `None` for an anonymous principal.
    pub pair: Option<String>,
    /// How an account authenticated.
    pub authentication: Option<String>,
}

impl From<&Principal> for AuditPrincipal {
    fn from(principal: &Principal) -> Self {
        let authentication = match principal {
            Principal::Account(account) => Some(
                match account.authentication {
                    AccountAuthentication::ApiToken => "api-token",
                    AccountAuthentication::BootstrapToken => "bootstrap-token",
                    AccountAuthentication::SignIn => "sign-in",
                }
                .to_string(),
            ),
            _ => None,
        };
        Self {
            kind: principal.kind().as_str().to_string(),
            pair: principal.pair().map(|pair| pair.to_string()),
            authentication,
        }
    }
}

/// The caller: the principal that started the chain, the one that made this call, and the
/// tokens presented at the entry point with their makers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditCaller {
    pub original: AuditPrincipal,
    pub immediate: AuditPrincipal,
    pub tokens: Vec<AuditTokenProvenance>,
}

impl From<&Caller> for AuditCaller {
    fn from(caller: &Caller) -> Self {
        Self {
            original: AuditPrincipal::from(&caller.original),
            immediate: AuditPrincipal::from(&caller.immediate),
            tokens: caller
                .tokens
                .iter()
                .map(AuditTokenProvenance::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTokenProvenance {
    pub id: Uuid,
    /// The pair of the token's maker.
    pub maker: Option<String>,
}

impl From<&TokenProvenance> for AuditTokenProvenance {
    fn from(token: &TokenProvenance) -> Self {
        Self {
            id: token.id,
            maker: token.maker.as_ref().map(|maker| maker.pair().to_string()),
        }
    }
}

/// A covering card, with its full ancestor list and the card token that presented it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditGrantor {
    pub card_id: CardId,
    pub ancestors: Vec<CardId>,
    pub presented_by: Option<CardTokenId>,
}

impl From<&GrantorCard> for AuditGrantor {
    fn from(card: &GrantorCard) -> Self {
        Self {
            card_id: card.card_id,
            ancestors: card.ancestors.clone(),
            presented_by: card.presented_by,
        }
    }
}

/// One part of the operation: the class and verb that it checks, its resource and its target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditPart {
    pub class: String,
    pub verb: String,
    pub resource: AuditResource,
    pub target: AuditTarget,
}

impl From<&OperationPart> for AuditPart {
    fn from(part: &OperationPart) -> Self {
        Self {
            class: part.class().name().to_string(),
            verb: part.verb.name().to_string(),
            resource: AuditResource::from(&part.resource),
            target: AuditTarget::from(&part.target),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum AuditResource {
    None,
    All,
    Method {
        name: String,
    },
    Command {
        name: String,
    },
    Path {
        path: String,
    },
    HostPort {
        host: String,
        port: Option<u16>,
    },
    Socket {
        address: String,
        port: u16,
    },
    Name {
        name: String,
    },
    Store {
        store: String,
        key: String,
    },
    Database {
        database: String,
        schema: Option<String>,
        table: Option<String>,
    },
    Card {
        card_id: CardId,
    },
    Pair {
        pair: String,
    },
    ApiToken {
        token_id: TokenId,
    },
    /// Any one method of the agent, as a methodless upsert asks for.
    AnyOneMethod,
}

impl From<&Resource> for AuditResource {
    fn from(resource: &Resource) -> Self {
        match resource {
            Resource::None => AuditResource::None,
            Resource::All => AuditResource::All,
            Resource::Method(name) => AuditResource::Method { name: name.clone() },
            Resource::Command(name) => AuditResource::Command { name: name.clone() },
            Resource::Path(path) => AuditResource::Path { path: path.clone() },
            Resource::Destination(Destination::HostPort { host, port }) => {
                AuditResource::HostPort {
                    host: host.clone(),
                    port: *port,
                }
            }
            Resource::Destination(Destination::Socket { address, port, .. }) => {
                AuditResource::Socket {
                    address: address.0.to_string(),
                    port: *port,
                }
            }
            Resource::Name(name) => AuditResource::Name { name: name.clone() },
            Resource::Store { store, key } => AuditResource::Store {
                store: store.clone(),
                key: key.clone(),
            },
            Resource::Database {
                database,
                schema,
                table,
            } => AuditResource::Database {
                database: database.clone(),
                schema: schema.clone(),
                table: table.clone(),
            },
            Resource::Card(card_id) => AuditResource::Card { card_id: *card_id },
            Resource::Pair(pair) => AuditResource::Pair {
                pair: pair.to_string(),
            },
            Resource::ApiToken(token_id) => AuditResource::ApiToken {
                token_id: *token_id,
            },
            Resource::AnyOneMethod => AuditResource::AnyOneMethod,
        }
    }
}

/// A target: its kind, its own ID, and the IDs that place it, with its owner for an entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTarget {
    pub kind: String,
    pub id: String,
    /// The agent type of an agent.
    pub name: Option<String>,
    /// `None` for an agent that the request creates.
    pub fingerprint: Option<AgentFingerprint>,
    pub environment_id: Option<EnvironmentId>,
    pub application_id: Option<ApplicationId>,
    pub account_id: Option<AccountId>,
    pub owner: Option<Box<AuditTarget>>,
}

impl AuditTarget {
    fn new(kind: TargetKind, id: String) -> Self {
        Self {
            kind: target_kind_label(kind).to_string(),
            id,
            name: None,
            fingerprint: None,
            environment_id: None,
            application_id: None,
            account_id: None,
            owner: None,
        }
    }

    fn owned_by(mut self, owner: AuditTarget) -> Self {
        self.owner = Some(Box::new(owner));
        self
    }

    /// The first value that the target or one of its owners gives.
    fn find<T>(&self, get: impl Fn(&AuditTarget) -> Option<T> + Copy) -> Option<T> {
        get(self).or_else(|| self.owner.as_ref().and_then(|owner| owner.find(get)))
    }

    fn of_invocation(invocation: &InvocationTarget) -> Self {
        let owner = match &invocation.owner {
            InvocationOwner::Agent(agent) => AuditTarget::from(&Target::Agent(agent.clone())),
            InvocationOwner::Tool(tool) => AuditTarget::from(&Target::Tool(tool.clone())),
        };
        AuditTarget::new(TargetKind::Invocation, invocation.invocation_id.to_string())
            .owned_by(owner)
    }

    fn of_owner(owner: &Owner) -> Self {
        match owner {
            Owner::Agent(agent) => AuditTarget::from(&Target::Agent(agent.clone())),
            Owner::Tool(tool) => AuditTarget::from(&Target::Tool(tool.clone())),
            Owner::Invocation(invocation) => AuditTarget::of_invocation(invocation),
            Owner::Environment(environment) => {
                AuditTarget::from(&Target::Environment(environment.clone()))
            }
            Owner::Account(account) => AuditTarget::from(&Target::Account(account.clone())),
        }
    }
}

impl From<&Target> for AuditTarget {
    fn from(target: &Target) -> Self {
        match target {
            Target::Agent(agent) => Self {
                name: Some(agent.agent_type.to_string()),
                fingerprint: agent.fingerprint,
                environment_id: Some(agent.environment_id),
                ..AuditTarget::new(TargetKind::Agent, agent.agent_id.as_uuid().to_string())
            },
            Target::Tool(tool) => Self {
                environment_id: Some(tool.environment_id),
                ..AuditTarget::new(TargetKind::Tool, tool.tool_name.to_string())
            },
            Target::Invocation(invocation) => AuditTarget::of_invocation(invocation),
            Target::Stream(stream) => {
                AuditTarget::new(TargetKind::Stream, stream.stream_id.to_string())
                    .owned_by(AuditTarget::of_invocation(&stream.owner))
            }
            Target::Promise(promise) => {
                let owner = match &promise.owner {
                    PromiseOwner::Agent(agent) => AuditTarget::from(&Target::Agent(agent.clone())),
                    PromiseOwner::Invocation(invocation) => AuditTarget::of_invocation(invocation),
                };
                AuditTarget::new(TargetKind::Promise, promise.id.0.clone()).owned_by(owner)
            }
            Target::Task(entity)
            | Target::Webhook(entity)
            | Target::Artifact(entity)
            | Target::Package(entity) => AuditTarget::new(target.kind(), entity.id.0.clone())
                .owned_by(AuditTarget::of_owner(&entity.owner)),
            Target::Account(account) => Self {
                account_id: Some(account.account_id),
                ..AuditTarget::new(TargetKind::Account, account.account_id.to_string())
            },
            Target::Application(application) => Self {
                account_id: Some(application.account_id),
                application_id: Some(application.application_id),
                ..AuditTarget::new(
                    TargetKind::Application,
                    application.application_id.to_string(),
                )
            },
            Target::Environment(environment) => Self {
                account_id: Some(environment.account_id),
                application_id: Some(environment.application_id),
                environment_id: Some(environment.environment_id),
                ..AuditTarget::new(
                    TargetKind::Environment,
                    environment.environment_id.to_string(),
                )
            },
        }
    }
}

fn target_kind_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Agent => "agent",
        TargetKind::Tool => "tool",
        TargetKind::Invocation => "invocation",
        TargetKind::Task => "task",
        TargetKind::Stream => "stream",
        TargetKind::Promise => "promise",
        TargetKind::Webhook => "webhook",
        TargetKind::Artifact => "artifact",
        TargetKind::Package => "package",
        TargetKind::Account => "account",
        TargetKind::Application => "application",
        TargetKind::Environment => "environment",
    }
}
