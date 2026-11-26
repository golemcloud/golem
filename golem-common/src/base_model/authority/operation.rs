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

use super::{
    AccountDomainRegistrationVerb, AccountUsageVerb, AccountVerb, AgentTarget, AgentVerb,
    ApplicationVerb, ArtifactVerb, BlobVerb, CardVerb, Class, DomainVerb,
    EnvironmentDomainRegistrationVerb, EnvironmentVerb, FilesystemVerb, InvocationVerb, KvVerb,
    NetworkVerb, OplogVerb, OwnerKind, PackageVerb, PrincipalSecretVerb, PromiseVerb, ProtocolVerb,
    QuotaVerb, RdbmsVerb, Resource, ResourceKind, SecretVerb, Target, TargetKind, ToolTarget,
    ToolVerb, Verb, WebhookVerb,
};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// The shape of one part of a catalog operation: its verb, the kind of its resource, the kinds of
/// target that it may name, and, for an entity target, the kinds of owner that the entity may
/// have. No owner kinds means any owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartShape {
    pub verb: Verb,
    pub resource: ResourceKind,
    pub targets: &'static [TargetKind],
    pub owners: &'static [OwnerKind],
}

/// Defines [`CatalogOperation`] and the parts of each operation.
///
/// The binary encoding of an operation is its position in this list. Add new operations at the
/// end of the list, never in the middle. The catalog's golden file test fails on any other change.
macro_rules! catalog {
    ($(
        $(#[$doc:meta])*
        $op:ident ($name:literal) => [
            $((
                $verb:expr,
                $resource:ident,
                [$($target:ident),+]
                $(owned by [$($owner:ident),+])?
            )),* $(,)?
        ]
    ),+ $(,)?) => {
        /// An operation of the catalog. Each operation has one or more parts, in the order in which
        /// `decide` checks them: parts whose targets are designated kinds first.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        #[cfg_attr(feature = "full", desert(evolution()))]
        pub enum CatalogOperation {
            $(
                $(#[$doc])*
                #[serde(rename = $name)]
                $op,
            )+
        }

        impl CatalogOperation {
            pub const ALL: &'static [CatalogOperation] = &[$(CatalogOperation::$op,)+];

            pub fn name(self) -> &'static str {
                match self {
                    $(CatalogOperation::$op => $name,)+
                }
            }

            /// The parts of the operation, in the order in which `decide` checks them.
            pub fn parts(self) -> &'static [PartShape] {
                match self {
                    $(CatalogOperation::$op => &[$(PartShape {
                        verb: $verb,
                        resource: ResourceKind::$resource,
                        targets: &[$(TargetKind::$target),+],
                        owners: &[$($(OwnerKind::$owner),+)?],
                    }),*],)+
                }
            }
        }
    };
}

catalog! {
    // Invocation: request authority.

    /// Fresh-agent creation. It is decided before the agent exists, so its agent target has no
    /// fingerprint.
    CreateAgent("create-agent") => [(Verb::Agent(AgentVerb::Create), None, [Agent])],
    InvokeAgent("invoke-agent") => [(Verb::Agent(AgentVerb::Invoke), Method, [Agent])],
    /// A standalone tool invocation.
    InvokeTool("invoke-tool") => [(Verb::Tool(ToolVerb::Invoke), Command, [Tool])],
    /// Hosted-exec admission: `agent : exec` on the hosting agent, then tool admission.
    HostedExec("hosted-exec") => [
        (Verb::Agent(AgentVerb::Exec), None, [Agent]),
        (Verb::Tool(ToolVerb::Invoke), Command, [Tool]),
    ],
    /// Retry: invocation `read` on the original, then request authority for the new request.
    RetryAgentInvocation("retry-agent-invocation") => [
        (Verb::Invocation(InvocationVerb::Read), None, [Invocation] owned by [Agent]),
        (Verb::Agent(AgentVerb::Invoke), Method, [Agent]),
    ],
    RetryToolInvocation("retry-tool-invocation") => [
        (Verb::Invocation(InvocationVerb::Read), None, [Invocation] owned by [Tool]),
        (Verb::Tool(ToolVerb::Invoke), Command, [Tool]),
    ],
    /// Invocation status, result and attach.
    ReadInvocation("read-invocation") => [
        (Verb::Invocation(InvocationVerb::Read), None, [Invocation]),
    ],
    CancelInvocation("cancel-invocation") => [
        (Verb::Invocation(InvocationVerb::Cancel), None, [Invocation]),
    ],
    /// Input to an invocation. Input to a tool invocation uses `send-tool-input` or
    /// `send-hosted-exec-input`, which repeat the invocation's admission.
    SendInvocationInput("send-invocation-input") => [
        (Verb::Invocation(InvocationVerb::Send), None, [Invocation]),
    ],
    /// Invocation listing: `scope`, then `filter` on each invocation of the owner.
    ListInvocations("list-invocations") => [
        (Verb::Invocation(InvocationVerb::Read), None, [Agent, Tool, Environment]),
    ],
    /// A2A task access and cancellation, on invocation grants.
    ReadTask("read-task") => [(Verb::Invocation(InvocationVerb::Read), None, [Task])],
    CancelTask("cancel-task") => [(Verb::Invocation(InvocationVerb::Cancel), None, [Task])],
    /// Input that continues an A2A task: invocation `read` on the task, then `send` on the task's
    /// pending input promise. No invocation grant implies the promise `send`.
    SendTaskInput("send-task-input") => [
        (Verb::Invocation(InvocationVerb::Read), None, [Task]),
        (Verb::Promise(PromiseVerb::Send), None, [Promise]),
    ],
    /// Durable Streams reads and appends, on invocation grants.
    ReadStream("read-stream") => [(Verb::Invocation(InvocationVerb::Read), None, [Stream])],
    AppendStream("append-stream") => [(Verb::Invocation(InvocationVerb::Send), None, [Stream])],
    SendToPromise("send-to-promise") => [(Verb::Promise(PromiseVerb::Send), None, [Promise])],
    SendToWebhook("send-to-webhook") => [(Verb::Webhook(WebhookVerb::Send), None, [Webhook])],
    ReadArtifact("read-artifact") => [(Verb::Artifact(ArtifactVerb::Read), None, [Artifact])],
    /// A static read of a tool's `File` config fields: tool-wide `invoke : *`.
    ReadToolFiles("read-tool-files") => [(Verb::Tool(ToolVerb::Invoke), All, [Tool])],

    // Management reads and controls.

    ViewAgent("view-agent") => [(Verb::Agent(AgentVerb::View), None, [Agent])],
    ReadAgentOplog("read-agent-oplog") => [(Verb::Oplog(OplogVerb::Read), None, [Agent])],
    /// The oplog of a standalone tool invocation, whose owner is the tool.
    ReadToolInvocationOplog("read-tool-invocation-oplog") => [
        (Verb::Oplog(OplogVerb::Read), None, [Invocation] owned by [Tool]),
    ],
    /// Agent listing: `scope`, then `filter` on each agent in this part's place.
    ListAgents("list-agents") => [(Verb::Agent(AgentVerb::View), None, [Environment])],
    DeleteAgent("delete-agent") => [(Verb::Agent(AgentVerb::Delete), None, [Agent])],
    InterruptAgent("interrupt-agent") => [(Verb::Agent(AgentVerb::Interrupt), None, [Agent])],
    ResumeAgent("resume-agent") => [(Verb::Agent(AgentVerb::Resume), None, [Agent])],
    UpdateAgentRevision("update-agent-revision") => [
        (Verb::Agent(AgentVerb::UpdateRevision), None, [Agent]),
    ],
    ForkAgent("fork-agent") => [(Verb::Agent(AgentVerb::Fork), None, [Agent])],
    RevertAgent("revert-agent") => [(Verb::Agent(AgentVerb::Revert), None, [Agent])],
    ActivateProcessor("activate-processor") => [
        (Verb::Agent(AgentVerb::ActivateProcessor), None, [Agent]),
    ],
    DeactivateProcessor("deactivate-processor") => [
        (Verb::Agent(AgentVerb::DeactivateProcessor), None, [Agent]),
    ],
    SimulateCrash("simulate-crash") => [(Verb::Agent(AgentVerb::SimulateCrash), None, [Agent])],
    /// Replay against new code.
    ReplayAgent("replay-agent") => [(Verb::Agent(AgentVerb::Replay), None, [Agent])],
    /// A control on a standalone tool invocation targets the invocation, owned by the tool.
    InterruptToolInvocation("interrupt-tool-invocation") => [
        (Verb::Tool(ToolVerb::Interrupt), None, [Invocation] owned by [Tool]),
    ],
    ResumeToolInvocation("resume-tool-invocation") => [
        (Verb::Tool(ToolVerb::Resume), None, [Invocation] owned by [Tool]),
    ],
    /// Webhook listing: `view` on the owner.
    ListAgentWebhooks("list-agent-webhooks") => [(Verb::Agent(AgentVerb::View), None, [Agent])],
    ListToolWebhooks("list-tool-webhooks") => [(Verb::Tool(ToolVerb::View), None, [Tool])],
    /// Webhook closing targets the webhook, together with its owner.
    CloseAgentWebhook("close-agent-webhook") => [
        (Verb::Agent(AgentVerb::CloseWebhook), None, [Webhook] owned by [Agent]),
    ],
    CloseToolWebhook("close-tool-webhook") => [
        (Verb::Tool(ToolVerb::CloseWebhook), None, [Webhook] owned by [Tool]),
    ],
    LimitAgent("limit-agent") => [(Verb::Agent(AgentVerb::Limit), None, [Agent])],
    LimitTool("limit-tool") => [(Verb::Tool(ToolVerb::Limit), None, [Tool])],
    LimitAccount("limit-account") => [(Verb::Account(AccountVerb::Limit), None, [Account])],
    /// Adding or removing a volume.
    AddVolume("add-volume") => [(Verb::Filesystem(FilesystemVerb::Mount), Path, [Agent, Tool])],
    RemoveVolume("remove-volume") => [
        (Verb::Filesystem(FilesystemVerb::Mount), Path, [Agent, Tool]),
    ],
    /// Management file reads.
    ListFiles("list-files") => [(Verb::Filesystem(FilesystemVerb::List), Path, [Agent, Tool])],
    ReadFile("read-file") => [(Verb::Filesystem(FilesystemVerb::Read), Path, [Agent, Tool])],
    /// A live-file read through a file route.
    ReadLiveFile("read-live-file") => [
        (Verb::Filesystem(FilesystemVerb::Read), Path, [Agent, Tool]),
    ],
    UnpublishArtifact("unpublish-artifact") => [
        (Verb::Artifact(ArtifactVerb::Unpublish), None, [Artifact]),
    ],
    /// Artifact listing by publisher.
    ListArtifacts("list-artifacts") => [(Verb::Artifact(ArtifactVerb::View), None, [Environment])],

    // Host functions. Filesystem, principal-secret and volume parts target the agent or tool
    // that owns them.

    FilesystemRead("filesystem-read") => [
        (Verb::Filesystem(FilesystemVerb::Read), Path, [Agent, Tool]),
    ],
    FilesystemWrite("filesystem-write") => [
        (Verb::Filesystem(FilesystemVerb::Write), Path, [Agent, Tool]),
    ],
    FilesystemList("filesystem-list") => [
        (Verb::Filesystem(FilesystemVerb::List), Path, [Agent, Tool]),
    ],
    FilesystemStat("filesystem-stat") => [
        (Verb::Filesystem(FilesystemVerb::Stat), Path, [Agent, Tool]),
    ],
    FilesystemDelete("filesystem-delete") => [
        (Verb::Filesystem(FilesystemVerb::Delete), Path, [Agent, Tool]),
    ],
    /// An outgoing connection. The owner is the account of the connecting workload.
    NetworkConnect("network-connect") => [
        (Verb::Network(NetworkVerb::Connect), Destination, [Account]),
    ],
    /// An identity proof attached at egress.
    ProveIdentity("prove-identity") => [
        (Verb::Network(NetworkVerb::ProveIdentity), Destination, [Account]),
    ],
    HoldSecret("hold-secret") => [(Verb::Secret(SecretVerb::Hold), Path, [Environment])],
    RevealSecret("reveal-secret") => [(Verb::Secret(SecretVerb::Reveal), Path, [Environment])],
    HoldPrincipalSecret("hold-principal-secret") => [
        (Verb::PrincipalSecret(PrincipalSecretVerb::Hold), Path, [Agent, Tool]),
    ],
    RevealPrincipalSecret("reveal-principal-secret") => [
        (Verb::PrincipalSecret(PrincipalSecretVerb::Reveal), Path, [Agent, Tool]),
    ],
    /// Reserving from a quota resource.
    ReserveQuota("reserve-quota") => [(Verb::Quota(QuotaVerb::Reserve), Name, [Environment])],
    KvRead("kv-read") => [(Verb::Kv(KvVerb::Read), Store, [Environment])],
    KvWrite("kv-write") => [(Verb::Kv(KvVerb::Write), Store, [Environment])],
    KvDelete("kv-delete") => [(Verb::Kv(KvVerb::Delete), Store, [Environment])],
    KvList("kv-list") => [(Verb::Kv(KvVerb::List), Store, [Environment])],
    BlobRead("blob-read") => [(Verb::Blob(BlobVerb::Read), Store, [Environment])],
    BlobWrite("blob-write") => [(Verb::Blob(BlobVerb::Write), Store, [Environment])],
    BlobDelete("blob-delete") => [(Verb::Blob(BlobVerb::Delete), Store, [Environment])],
    BlobList("blob-list") => [(Verb::Blob(BlobVerb::List), Store, [Environment])],
    RdbmsQuery("rdbms-query") => [(Verb::Rdbms(RdbmsVerb::Query), Database, [Environment])],
    RdbmsMutate("rdbms-mutate") => [(Verb::Rdbms(RdbmsVerb::Mutate), Database, [Environment])],
    RdbmsUnanalyzed("rdbms-unanalyzed") => [
        (Verb::Rdbms(RdbmsVerb::Unanalyzed), Database, [Environment]),
    ],

    // Secret management.

    ViewSecret("view-secret") => [(Verb::Secret(SecretVerb::View), Path, [Environment])],
    CreateSecret("create-secret") => [(Verb::Secret(SecretVerb::Create), Path, [Environment])],
    UpdateSecret("update-secret") => [(Verb::Secret(SecretVerb::Update), Path, [Environment])],
    DeleteSecret("delete-secret") => [(Verb::Secret(SecretVerb::Delete), Path, [Environment])],
    /// Changes only the declared destinations of promoted secrets.
    UpdatePrincipalSecret("update-principal-secret") => [
        (Verb::PrincipalSecret(PrincipalSecretVerb::Update), Path, [Agent, Tool]),
    ],

    // Accounts, applications, environments and deployment.

    /// Account creation under a parent account.
    CreateAccount("create-account") => [(Verb::Account(AccountVerb::Create), None, [Account])],
    /// Changing an account's display name.
    UpdateAccount("update-account") => [(Verb::Account(AccountVerb::Update), None, [Account])],
    ConfigureSignIn("configure-sign-in") => [
        (Verb::Account(AccountVerb::ConfigureSignIn), None, [Account]),
    ],
    ViewAccountCountsReport("view-account-counts-report") => [
        (Verb::Account(AccountVerb::ViewAccountCountsReport), None, [Account]),
    ],
    ViewAccountSummariesReport("view-account-summaries-report") => [
        (Verb::Account(AccountVerb::ViewAccountSummariesReport), None, [Account]),
    ],
    /// Creating an account's API tokens.
    CreateApiTokens("create-api-tokens") => [
        (Verb::Account(AccountVerb::CreateApiTokens), None, [Account]),
    ],
    ListApiTokens("list-api-tokens") => [
        (Verb::Account(AccountVerb::ViewApiTokens), All, [Account]),
    ],
    ViewApiToken("view-api-token") => [
        (Verb::Account(AccountVerb::ViewApiTokens), ApiToken, [Account]),
    ],
    RevokeApiToken("revoke-api-token") => [
        (Verb::Account(AccountVerb::RevokeApiTokens), ApiToken, [Account]),
    ],
    /// Setting or changing a resource override's expiry.
    SetOverrideExpiry("set-override-expiry") => [
        (Verb::Account(AccountVerb::SetOverrideExpiry), None, [Account]),
        (Verb::AccountUsage(AccountUsageVerb::Update), None, [Account]),
    ],
    /// Switching an account's spend mode; the resource is the mode.
    SetSpendMode("set-spend-mode") => [(Verb::Account(AccountVerb::SetSpendMode), Name, [Account])],
    /// `golem server status`.
    ViewInstallation("view-installation") => [
        (Verb::Account(AccountVerb::ViewInstallation), None, [Account]),
    ],
    CreateApplication("create-application") => [
        (Verb::Application(ApplicationVerb::Create), None, [Account]),
    ],
    CreateEnvironment("create-environment") => [
        (Verb::Environment(EnvironmentVerb::Create), None, [Application]),
    ],
    DeployEnvironment("deploy-environment") => [
        (Verb::Environment(EnvironmentVerb::Deploy), None, [Environment]),
    ],
    /// Host registration; the resource is the host.
    RegisterHost("register-host") => [
        (
            Verb::EnvironmentDomainRegistration(EnvironmentDomainRegistrationVerb::Create),
            Name,
            [Environment]
        ),
    ],
    /// A wildcard registration that an account holds; the resource is the domain.
    RegisterWildcardDomain("register-wildcard-domain") => [
        (Verb::AccountDomainRegistration(AccountDomainRegistrationVerb::Create), Name, [Account]),
    ],
    ListWildcardDomains("list-wildcard-domains") => [
        (Verb::AccountDomainRegistration(AccountDomainRegistrationVerb::View), All, [Account]),
    ],
    DeleteWildcardDomain("delete-wildcard-domain") => [
        (Verb::AccountDomainRegistration(AccountDomainRegistrationVerb::Delete), Name, [Account]),
    ],
    /// Publishing a domain for an environment; the resource is the host.
    PublishDomain("publish-domain") => [(Verb::Domain(DomainVerb::Publish), Name, [Environment])],
    /// Publishing a protocol for an environment; the resource is the protocol.
    PublishProtocol("publish-protocol") => [
        (Verb::Protocol(ProtocolVerb::Publish), Name, [Environment]),
    ],

    // Packages.

    PublishPackage("publish-package") => [(Verb::Package(PackageVerb::Publish), None, [Account])],
    ReadPackage("read-package") => [(Verb::Package(PackageVerb::Read), None, [Package])],
    DePublishPackage("de-publish-package") => [
        (Verb::Package(PackageVerb::DePublish), None, [Package]),
    ],
    RestorePackage("restore-package") => [(Verb::Package(PackageVerb::Restore), None, [Package])],
    DeprecatePackage("deprecate-package") => [
        (Verb::Package(PackageVerb::Deprecate), None, [Package]),
    ],
    AttachPackageCard("attach-package-card") => [
        (Verb::Package(PackageVerb::AttachCard), None, [Package]),
    ],

    // Cards. A `card` grant's owner is an environment, an account subtree, or one
    // entity.

    /// Installing a card into another pair; the resource is the destination pair.
    InstallCard("install-card") => [
        (
            Verb::Card(CardVerb::Install),
            Pair,
            [Environment, Account, Invocation, Promise, Webhook, Artifact]
        ),
    ],
    RevokeCard("revoke-card") => [
        (
            Verb::Card(CardVerb::Revoke),
            Card,
            [Environment, Account, Invocation, Promise, Webhook, Artifact]
        ),
    ],
    InspectCard("inspect-card") => [
        (
            Verb::Card(CardVerb::Inspect),
            Card,
            [Environment, Account, Invocation, Promise, Webhook, Artifact]
        ),
    ],
    /// Listing a card's tokens needs the right to revoke them.
    ListCardTokens("list-card-tokens") => [
        (
            Verb::Card(CardVerb::Revoke),
            Card,
            [Environment, Account, Invocation, Promise, Webhook, Artifact]
        ),
    ],

    // Invocation: request authority, appended (the binary encoding is the position).

    /// A methodless upsert, including the client ID accessor: `invoke` on any one method of the
    /// selected agent, or of the keyed agent if it does not exist yet. An upsert that calls a
    /// method uses `invoke-agent`.
    UpsertAgent("upsert-agent") => [(Verb::Agent(AgentVerb::Invoke), AnyOneMethod, [Agent])],
    /// Input to a standalone tool invocation, on the starting connection and every later one:
    /// invocation `send` on the invocation, then tool admission again, against the input
    /// sender's current authority.
    SendToolInput("send-tool-input") => [
        (Verb::Invocation(InvocationVerb::Send), None, [Invocation] owned by [Tool]),
        (Verb::Tool(ToolVerb::Invoke), Command, [Tool]),
    ],
    /// Input to a hosted tool invocation, on the starting connection and every later one:
    /// invocation `send` on the invocation, then hosted-exec admission again, against the input
    /// sender's current authority: `agent : exec` on the hosting agent, then `tool : invoke`.
    SendHostedExecInput("send-hosted-exec-input") => [
        (Verb::Invocation(InvocationVerb::Send), None, [Invocation] owned by [Agent]),
        (Verb::Agent(AgentVerb::Exec), None, [Agent]),
        (Verb::Tool(ToolVerb::Invoke), Command, [Tool]),
    ],
}

/// One part of an operation: a verb, the resource it asks for, and the target it names. The class
/// comes from the verb.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct OperationPart {
    pub verb: Verb,
    pub resource: Resource,
    pub target: Target,
}

impl OperationPart {
    pub fn new(verb: impl Into<Verb>, resource: Resource, target: Target) -> Self {
        Self {
            verb: verb.into(),
            resource,
            target,
        }
    }

    pub fn class(&self) -> Class {
        self.verb.class()
    }
}

/// One operation of the catalog with its parts. Every constructor, and every decoder, checks the
/// parts against the catalog, so an operation never misses a part, adds one, or names a target or
/// a resource of another kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "OperationRecord", into = "OperationRecord")]
pub struct Operation {
    name: CatalogOperation,
    parts: Vec<OperationPart>,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum OperationShapeError {
    #[error("{operation} has {expected} parts, not {actual}")]
    PartCount {
        operation: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("part {index} of {operation} must be {expected}, not {actual}")]
    Verb {
        operation: &'static str,
        index: usize,
        expected: Verb,
        actual: Verb,
    },
    #[error("part {index} of {operation} takes a resource of kind {expected:?}, not {actual:?}")]
    Resource {
        operation: &'static str,
        index: usize,
        expected: ResourceKind,
        actual: ResourceKind,
    },
    #[error("part {index} of {operation} cannot target {actual:?}")]
    Target {
        operation: &'static str,
        index: usize,
        actual: TargetKind,
    },
    #[error("part {index} of {operation} cannot target an entity whose owner is {actual:?}")]
    Owner {
        operation: &'static str,
        index: usize,
        actual: Option<OwnerKind>,
    },
}

impl Operation {
    /// An operation whose parts match the catalog, in the catalog's order.
    pub fn new(
        name: CatalogOperation,
        parts: Vec<OperationPart>,
    ) -> Result<Self, OperationShapeError> {
        let shapes = name.parts();
        if shapes.len() != parts.len() {
            return Err(OperationShapeError::PartCount {
                operation: name.name(),
                expected: shapes.len(),
                actual: parts.len(),
            });
        }
        for (index, (shape, part)) in shapes.iter().zip(&parts).enumerate() {
            if shape.verb != part.verb {
                return Err(OperationShapeError::Verb {
                    operation: name.name(),
                    index,
                    expected: shape.verb,
                    actual: part.verb,
                });
            }
            if shape.resource != part.resource.kind() {
                return Err(OperationShapeError::Resource {
                    operation: name.name(),
                    index,
                    expected: shape.resource,
                    actual: part.resource.kind(),
                });
            }
            if !shape.targets.contains(&part.target.kind()) {
                return Err(OperationShapeError::Target {
                    operation: name.name(),
                    index,
                    actual: part.target.kind(),
                });
            }
            if !shape.owners.is_empty() {
                let owner = part.target.owner_kind();
                if !owner.is_some_and(|owner| shape.owners.contains(&owner)) {
                    return Err(OperationShapeError::Owner {
                        operation: name.name(),
                        index,
                        actual: owner,
                    });
                }
            }
        }
        Ok(Self { name, parts })
    }

    /// An operation of one part, with the verb that the catalog gives it.
    pub fn single(
        name: CatalogOperation,
        resource: Resource,
        target: Target,
    ) -> Result<Self, OperationShapeError> {
        let verb = match name.parts() {
            [shape] => shape.verb,
            shapes => {
                return Err(OperationShapeError::PartCount {
                    operation: name.name(),
                    expected: shapes.len(),
                    actual: 1,
                });
            }
        };
        Self::new(name, vec![OperationPart::new(verb, resource, target)])
    }

    /// Hosted exec: `agent : exec` on the hosting agent, then `tool : invoke` on the tool.
    pub fn hosted_exec(
        agent: AgentTarget,
        tool: ToolTarget,
        command: String,
    ) -> Result<Self, OperationShapeError> {
        Self::new(
            CatalogOperation::HostedExec,
            vec![
                OperationPart::new(AgentVerb::Exec, Resource::None, Target::Agent(agent)),
                OperationPart::new(
                    ToolVerb::Invoke,
                    Resource::Command(command),
                    Target::Tool(tool),
                ),
            ],
        )
    }

    pub fn name(&self) -> CatalogOperation {
        self.name
    }

    pub fn parts(&self) -> &[OperationPart] {
        &self.parts
    }
}

impl Display for CatalogOperation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The stored form of an [`Operation`], before its parts are checked against the catalog.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
struct OperationRecord {
    name: CatalogOperation,
    parts: Vec<OperationPart>,
}

impl TryFrom<OperationRecord> for Operation {
    type Error = OperationShapeError;

    fn try_from(record: OperationRecord) -> Result<Self, Self::Error> {
        Operation::new(record.name, record.parts)
    }
}

impl From<Operation> for OperationRecord {
    fn from(operation: Operation) -> Self {
        OperationRecord {
            name: operation.name,
            parts: operation.parts,
        }
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for Operation {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&OperationRecord::from(self.clone()), context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for Operation {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let record: OperationRecord = desert_rust::BinaryDeserializer::deserialize(context)?;
        Operation::try_from(record)
            .map_err(|error| desert_rust::Error::DeserializationFailure(error.to_string()))
    }
}
