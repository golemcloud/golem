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

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Defines [`Class`], one verb enum for each class, and [`Verb`], which holds a verb together
/// with its class, so that no value can pair a verb with another class.
///
/// The binary encoding of a class and of a verb is its position in its list. Add new classes and
/// verbs at the end of their list, never in the middle. The catalog's golden file test fails on
/// any other change.
macro_rules! classes {
    ($(
        $(#[$class_doc:meta])*
        $class:ident ($class_name:literal) => $verb_enum:ident {
            $($verb:ident ($verb_name:literal)),+ $(,)?
        }
    ),+ $(,)?) => {
        /// A class of grants.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        #[cfg_attr(feature = "full", desert(evolution()))]
        pub enum Class {
            $(
                $(#[$class_doc])*
                #[serde(rename = $class_name)]
                $class,
            )+
        }

        impl Class {
            pub const ALL: &'static [Class] = &[$(Class::$class,)+];

            /// The class name, as a grant spells it.
            pub fn name(self) -> &'static str {
                match self {
                    $(Class::$class => $class_name,)+
                }
            }

            /// Every verb of the class.
            pub fn verbs(self) -> Vec<Verb> {
                match self {
                    $(Class::$class => $verb_enum::ALL
                        .iter()
                        .copied()
                        .map(Verb::$class)
                        .collect(),)+
                }
            }
        }

        $(
            #[doc = concat!("A verb of the `", $class_name, "` class.")]
            #[derive(
                Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
            )]
            #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
            #[cfg_attr(feature = "full", desert(evolution()))]
            pub enum $verb_enum {
                $(
                    #[serde(rename = $verb_name)]
                    $verb,
                )+
            }

            impl $verb_enum {
                pub const ALL: &'static [$verb_enum] = &[$($verb_enum::$verb,)+];

                /// The verb name, as a grant spells it.
                pub fn name(self) -> &'static str {
                    match self {
                        $($verb_enum::$verb => $verb_name,)+
                    }
                }
            }

            impl From<$verb_enum> for Verb {
                fn from(verb: $verb_enum) -> Self {
                    Verb::$class(verb)
                }
            }
        )+

        /// A verb together with its class.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        #[cfg_attr(feature = "full", desert(evolution()))]
        pub enum Verb {
            $(
                #[serde(rename = $class_name)]
                $class($verb_enum),
            )+
        }

        impl Verb {
            pub fn class(self) -> Class {
                match self {
                    $(Verb::$class(_) => Class::$class,)+
                }
            }

            /// The verb name, without the class.
            pub fn name(self) -> &'static str {
                match self {
                    $(Verb::$class(verb) => verb.name(),)+
                }
            }
        }
    };
}

classes! {
    Filesystem("filesystem") => FilesystemVerb {
        Read("read"), Write("write"), List("list"), Stat("stat"), Delete("delete"), Mount("mount"),
    },
    /// The owner is the account of the connecting workload. `prove-identity` must be named: `*`
    /// does not cover it.
    Network("network") => NetworkVerb {
        Connect("connect"), ProveIdentity("prove-identity"),
    },
    Oplog("oplog") => OplogVerb {
        Read("read"),
    },
    /// Environment secrets: `hold` and `reveal`, and the verbs that manage environment secrets.
    Secret("secret") => SecretVerb {
        Hold("hold"), Reveal("reveal"), View("view"), Create("create"), Update("update"),
        Delete("delete"),
    },
    /// Secrets that a principal owns. `update` is the only management verb.
    PrincipalSecret("principal-secret") => PrincipalSecretVerb {
        Hold("hold"), Reveal("reveal"), Update("update"),
    },
    Agent("agent") => AgentVerb {
        Create("create"), Invoke("invoke"), Exec("exec"), View("view"), Delete("delete"),
        Interrupt("interrupt"), Resume("resume"), UpdateRevision("update-revision"), Fork("fork"),
        Revert("revert"), ActivateProcessor("activate-processor"),
        DeactivateProcessor("deactivate-processor"), SimulateCrash("simulate-crash"),
        Limit("limit"), Replay("replay"), CloseWebhook("close-webhook"),
    },
    /// `interrupt`, `resume` and `close-webhook` act on a standalone invocation of the tool or
    /// on a webhook that the tool owns, never on the tool itself.
    Tool("tool") => ToolVerb {
        Invoke("invoke"), View("view"), Limit("limit"), Interrupt("interrupt"), Resume("resume"),
        CloseWebhook("close-webhook"),
    },
    Invocation("invocation") => InvocationVerb {
        Read("read"), Cancel("cancel"), Send("send"),
    },
    Promise("promise") => PromiseVerb {
        Send("send"),
    },
    Webhook("webhook") => WebhookVerb {
        Send("send"),
    },
    Artifact("artifact") => ArtifactVerb {
        Read("read"), Unpublish("unpublish"), View("view"),
    },
    /// `reserve`, and the resource management verbs of the 1.6 `environment.resource-definition`
    /// class, which merges into `quota`.
    Quota("quota") => QuotaVerb {
        Reserve("reserve"), View("view"), Create("create"), Update("update"), Delete("delete"),
        Restore("restore"),
    },
    Kv("kv") => KvVerb {
        Read("read"), Write("write"), Delete("delete"), List("list"),
    },
    Blob("blob") => BlobVerb {
        Read("read"), Write("write"), Delete("delete"), List("list"),
    },
    Rdbms("rdbms") => RdbmsVerb {
        Query("query"), Mutate("mutate"), Unanalyzed("unanalyzed"),
    },
    Card("card") => CardVerb {
        Install("install"), Revoke("revoke"), Inspect("inspect"),
    },
    Plan("plan") => PlanVerb {
        View("view"), Create("create"), Update("update"),
    },
    Account("account") => AccountVerb {
        View("view"), ViewPlan("view-plan"), Update("update"), Delete("delete"),
        SetPlan("set-plan"), Create("create"), ConfigureSignIn("configure-sign-in"),
        ViewAccountCountsReport("view-account-counts-report"),
        ViewAccountSummariesReport("view-account-summaries-report"),
        CreateApiTokens("create-api-tokens"), ViewApiTokens("view-api-tokens"),
        RevokeApiTokens("revoke-api-tokens"), SetOverrideExpiry("set-override-expiry"),
        SetSpendMode("set-spend-mode"), Limit("limit"), ViewInstallation("view-installation"),
    },
    AccountUsage("account-usage") => AccountUsageVerb {
        View("view"), Update("update"),
    },
    AccountPermissionShare("account.permission-share") => AccountPermissionShareVerb {
        View("view"), Create("create"), Update("update"), Delete("delete"),
    },
    /// A wildcard domain registration that an account holds.
    AccountDomainRegistration("account.domain-registration") => AccountDomainRegistrationVerb {
        View("view"), Create("create"), Delete("delete"),
    },
    Application("application") => ApplicationVerb {
        View("view"), Create("create"), Update("update"), Delete("delete"),
        AssertIdentity("assert-identity"),
    },
    Environment("environment") => EnvironmentVerb {
        View("view"), Create("create"), Update("update"), Delete("delete"), Deploy("deploy"),
        Rollback("rollback"), ViewDeployment("view-deployment"),
        ViewDeploymentPlan("view-deployment-plan"), ViewAgentTypes("view-agent-types"),
        ViewTools("view-tools"), WriteDeploymentRecord("write-deployment-record"),
    },
    EnvironmentDomainRegistration("environment.domain-registration")
        => EnvironmentDomainRegistrationVerb {
        View("view"), Create("create"), Delete("delete"),
    },
    EnvironmentInitialFiles("environment.initial-files") => EnvironmentInitialFilesVerb {
        View("view"), Update("update"), Delete("delete"), List("list"),
    },
    EnvironmentKvBucket("environment.kv-bucket") => EnvironmentKvBucketVerb {
        View("view"), Create("create"), Delete("delete"), Clear("clear"),
    },
    EnvironmentBlobBucket("environment.blob-bucket") => EnvironmentBlobBucketVerb {
        View("view"), Create("create"), Delete("delete"), Clear("clear"),
    },
    /// Component management only; callables never name components.
    Component("component") => ComponentVerb {
        View("view"), Create("create"), Update("update"), Delete("delete"),
    },
    Package("package") => PackageVerb {
        Publish("publish"), Read("read"), DePublish("de-publish"), Restore("restore"),
        Deprecate("deprecate"), AttachCard("attach-card"),
    },
    Domain("domain") => DomainVerb {
        Publish("publish"),
    },
    Protocol("protocol") => ProtocolVerb {
        Publish("publish"),
    },
}

impl Display for Class {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The text form `class : verb`.
impl Display for Verb {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} : {}", self.class().name(), self.name())
    }
}
