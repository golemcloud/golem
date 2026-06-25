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

use super::class::card_permission_classes;
use super::class::*;
use super::owner::*;

macro_rules! define_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionPattern {
            $($variant(ClassPermissionPattern<$class>),)+
        }
    };
}

card_permission_classes!(define_permission_pattern);

macro_rules! define_permission_target {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionTarget {
            $($variant(ClassPermissionTarget<$class>),)+
        }
    };
}

card_permission_classes!(define_permission_target);

macro_rules! define_polymorphic_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicPermissionPattern {
            $($variant(PolymorphicClassPermissionPattern<$class>),)+
        }
    };
}

card_permission_classes!(define_polymorphic_permission_pattern);

macro_rules! define_polymorphic_manifest_permission_pattern {
    ($($variant:ident: $class:ty,)+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicManifestPermissionPattern {
            $($variant(PolymorphicManifestClassPermissionPattern<$class>),)+
        }
    };
}

card_permission_classes!(define_polymorphic_manifest_permission_pattern);

macro_rules! define_class_name_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! class_name_match {
            ($self:expr) => {
                match $self {
                    $(Self::$variant(_) => <$class as PermissionClass>::NAME,)+
                }
            };
        }
    };
}

card_permission_classes!(define_class_name_match);

macro_rules! define_same_variant_subsumes_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_subsumes_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (Self::$variant(a), Self::$variant(b)) => a.subsumes(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_subsumes_match);

macro_rules! define_same_variant_subsumes_target_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_subsumes_target_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (PermissionPattern::$variant(a), PermissionTarget::$variant(b)) => a.subsumes_target(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_subsumes_target_match);

macro_rules! define_same_variant_target_subsumes_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! same_variant_target_subsumes_match {
            ($left:expr, $right:expr) => {
                match ($left, $right) {
                    $(
                        (Self::$variant(a), Self::$variant(b)) => a.subsumes(b),
                    )+
                    _ => false,
                }
            };
        }
    };
}

card_permission_classes!(define_same_variant_target_subsumes_match);

macro_rules! define_recipient_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! recipient_match {
            ($self:expr) => {
                match $self {
                    $(
                        Self::$variant(pattern) => &pattern.recipient,
                    )+
                }
            };
        }
    };
}

card_permission_classes!(define_recipient_match);

macro_rules! define_permission_pattern_to_target_match {
    ($($variant:ident: $class:ty,)+) => {
        macro_rules! permission_pattern_to_target_match {
            ($self:expr) => {
                match $self {
                    $(
                        PermissionPattern::$variant(pattern) => PermissionTarget::$variant(ClassPermissionTarget::<$class> {
                            verb: pattern.verb,
                            owner: pattern.owner.clone(),
                            resource: pattern.resource.clone(),
                        }),
                    )+
                }
            };
        }
    };
}

card_permission_classes!(define_permission_pattern_to_target_match);

impl PermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        same_variant_subsumes_match!(self, other)
    }

    pub fn subsumes_target(&self, other: &PermissionTarget) -> bool {
        same_variant_subsumes_target_match!(self, other)
    }

    pub fn recipient(&self) -> &crate::model::card::recipient::RecipientPattern {
        recipient_match!(self)
    }

    pub fn render(&self) -> Result<String, String> {
        crate::model::card::render_permission(self)
    }

    pub(crate) fn to_target(&self) -> PermissionTarget {
        permission_pattern_to_target_match!(self)
    }
}

impl PermissionTarget {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        same_variant_target_subsumes_match!(self, other)
    }
}

impl PolymorphicPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn recipient(&self) -> &crate::model::card::recipient::RecipientPattern {
        recipient_match!(self)
    }

    pub fn render(&self) -> Result<String, String> {
        crate::model::card::render_polymorphic_permission(self)
    }

    pub fn into_manifest_concrete_recipient(self) -> PolymorphicManifestPermissionPattern {
        macro_rules! convert {
            ($($variant:ident: $class:ty,)+) => {
                match self {
                    $(PolymorphicPermissionPattern::$variant(pattern) => {
                        PolymorphicManifestPermissionPattern::$variant(PolymorphicManifestClassPermissionPattern::<$class> {
                            verb: pattern.verb,
                            owner: pattern.owner,
                            recipient: crate::model::card::recipient::PolymorphicRecipientPattern::Concrete(pattern.recipient),
                            resource: pattern.resource,
                        })
                    })+
                }
            };
        }

        card_permission_classes!(convert)
    }
}

pub(crate) fn render_class_permission<O, R, V>(
    class_name: &'static str,
    owner: &O,
    recipient: &crate::model::card::recipient::RecipientPattern,
    verb: Option<&V>,
    resource: &R,
) -> Result<String, String>
where
    O: RenderFragment,
    R: RenderFragment,
    V: RenderFragment,
{
    Ok(format!(
        "{}({}) @ {} : {} : {}",
        class_name,
        owner.render_fragment()?,
        recipient.render(),
        verb.map(RenderFragment::render_fragment)
            .transpose()?
            .unwrap_or_else(|| "*".to_string()),
        resource.render_fragment()?,
    ))
}

pub(crate) fn render_manifest_class_permission<O, R, V>(
    class_name: &'static str,
    owner: &O,
    recipient: &crate::model::card::recipient::PolymorphicRecipientPattern,
    verb: Option<&V>,
    resource: &R,
) -> Result<String, String>
where
    O: RenderFragment,
    R: RenderFragment,
    V: RenderFragment,
{
    Ok(format!(
        "{}({}) @ {} : {} : {}",
        class_name,
        owner.render_fragment()?,
        recipient.render(),
        verb.map(RenderFragment::render_fragment)
            .transpose()?
            .unwrap_or_else(|| "*".to_string()),
        resource.render_fragment()?,
    ))
}

pub(crate) trait RenderFragment {
    fn render_fragment(&self) -> Result<String, String>;
}

impl RenderFragment for PolymorphicEmptyOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(_) => Ok(String::new()),
        }
    }
}

impl RenderFragment for EmptyOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(String::new())
    }
}

impl RenderFragment for PolymorphicAccountOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(owner) => owner.render_fragment(),
            Self::Account => Ok("?account".to_string()),
        }
    }
}

impl RenderFragment for AccountOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Any => Ok("*".to_string()),
            Self::Account { account } => Ok(account.as_str().to_string()),
        }
    }
}

impl RenderFragment for PolymorphicApplicationOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(owner) => owner.render_fragment(),
            Self::AccountApplications => Ok("?account/*".to_string()),
            Self::AccountApplication { application } => Ok(format!("?account/{}", application.0)),
            Self::App => Ok("?app".to_string()),
        }
    }
}

impl RenderFragment for ApplicationOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::AnyApplications => Ok("*/*".to_string()),
            Self::AccountApplications { account } => Ok(format!("{}/*", account.as_str())),
            Self::Application {
                account,
                application,
            } => Ok(format!("{}/{}", account.as_str(), application.0)),
        }
    }
}

impl RenderFragment for PolymorphicEnvironmentOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(owner) => owner.render_fragment(),
            Self::AccountEnvironments => Ok("?account/*/*".to_string()),
            Self::AccountApplicationEnvironments { application } => {
                Ok(format!("?account/{}/*", application.0))
            }
            Self::AccountEnvironment {
                application,
                environment,
            } => Ok(format!("?account/{}/{}", application.0, environment.0)),
            Self::ApplicationEnvironments => Ok("?app/*".to_string()),
            Self::ApplicationEnvironment { environment } => Ok(format!("?app/{}", environment.0)),
            Self::Env => Ok("?env".to_string()),
        }
    }
}

impl RenderFragment for EnvironmentOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::AnyEnvironments => Ok("*/*/*".to_string()),
            Self::AccountEnvironments { account } => Ok(format!("{}/*/*", account.as_str())),
            Self::ApplicationEnvironments {
                account,
                application,
            } => Ok(format!("{}/{}/*", account.as_str(), application.0)),
            Self::Environment {
                account,
                application,
                environment,
            } => Ok(format!(
                "{}/{}/{}",
                account.as_str(),
                application.0,
                environment.0
            )),
        }
    }
}

impl RenderFragment for PolymorphicComponentOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(owner) => owner.render_fragment(),
            Self::AccountComponents => Ok("?account/*/*/*".to_string()),
            Self::AccountApplicationComponents { application } => {
                Ok(format!("?account/{}/*/*", application.0))
            }
            Self::AccountEnvironmentComponents {
                application,
                environment,
            } => Ok(format!("?account/{}/{}/*", application.0, environment.0)),
            Self::AccountComponent {
                application,
                environment,
                component,
            } => Ok(format!(
                "?account/{}/{}/{}",
                application.0, environment.0, component.0
            )),
            Self::ApplicationComponents => Ok("?app/*/*".to_string()),
            Self::ApplicationEnvironmentComponents { environment } => {
                Ok(format!("?app/{}/*", environment.0))
            }
            Self::ApplicationComponent {
                environment,
                component,
            } => Ok(format!("?app/{}/{}", environment.0, component.0)),
            Self::EnvComponents => Ok("?env/*".to_string()),
            Self::EnvComponent { component } => Ok(format!("?env/{}", component.0)),
            Self::Component => Ok("?component".to_string()),
        }
    }
}

impl RenderFragment for ComponentOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::AnyComponents => Ok("*/*/*/*".to_string()),
            Self::AccountComponents { account } => Ok(format!("{}/*/*/*", account.as_str())),
            Self::ApplicationComponents {
                account,
                application,
            } => Ok(format!("{}/{}/*/*", account.as_str(), application.0)),
            Self::EnvironmentComponents {
                account,
                application,
                environment,
            } => Ok(format!(
                "{}/{}/{}/*",
                account.as_str(),
                application.0,
                environment.0
            )),
            Self::Component {
                account,
                application,
                environment,
                component,
            } => Ok(format!(
                "{}/{}/{}/{}",
                account.as_str(),
                application.0,
                environment.0,
                component.0
            )),
        }
    }
}

fn render_agent_leaf(agent: &AgentOwnerLeafPattern) -> String {
    match agent {
        AgentOwnerLeafPattern::Agent(agent) => agent.clone(),
        AgentOwnerLeafPattern::AgentTypeWildcard(agent_type) => format!("{}(*)", agent_type.0),
    }
}

impl RenderFragment for PolymorphicAgentOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(owner) => owner.render_fragment(),
            Self::AccountAgents => Ok("?account/*/*/*/*".to_string()),
            Self::AccountApplicationAgents { application } => {
                Ok(format!("?account/{}/*/*/*", application.0))
            }
            Self::AccountEnvironmentAgents {
                application,
                environment,
            } => Ok(format!("?account/{}/{}/*/*", application.0, environment.0)),
            Self::AccountComponentAgents {
                application,
                environment,
                component,
            } => Ok(format!(
                "?account/{}/{}/{}/*",
                application.0, environment.0, component.0
            )),
            Self::AccountAgent {
                application,
                environment,
                component,
                agent,
            } => Ok(format!(
                "?account/{}/{}/{}/{}",
                application.0,
                environment.0,
                component.0,
                render_agent_leaf(agent)
            )),
            Self::ApplicationAgents => Ok("?app/*/*/*".to_string()),
            Self::ApplicationEnvironmentAgents { environment } => {
                Ok(format!("?app/{}/*/*", environment.0))
            }
            Self::ApplicationComponentAgents {
                environment,
                component,
            } => Ok(format!("?app/{}/{}/*", environment.0, component.0)),
            Self::ApplicationAgent {
                environment,
                component,
                agent,
            } => Ok(format!(
                "?app/{}/{}/{}",
                environment.0,
                component.0,
                render_agent_leaf(agent)
            )),
            Self::EnvAgents => Ok("?env/*/*".to_string()),
            Self::EnvComponentAgents { component } => Ok(format!("?env/{}/*", component.0)),
            Self::EnvAgent { component, agent } => {
                Ok(format!("?env/{}/{}", component.0, render_agent_leaf(agent)))
            }
            Self::ComponentAgents => Ok("?component/*".to_string()),
            Self::ComponentAgent { agent } => {
                Ok(format!("?component/{}", render_agent_leaf(agent)))
            }
            Self::Agent => Ok("?agent".to_string()),
        }
    }
}

impl RenderFragment for AgentOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::AnyAgents => Ok("*/*/*/*/*".to_string()),
            Self::AccountAgents { account } => Ok(format!("{}/*/*/*/*", account.as_str())),
            Self::ApplicationAgents {
                account,
                application,
            } => Ok(format!("{}/{}/*/*/*", account.as_str(), application.0)),
            Self::EnvironmentAgents {
                account,
                application,
                environment,
            } => Ok(format!(
                "{}/{}/{}/*/*",
                account.as_str(),
                application.0,
                environment.0
            )),
            Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            } => Ok(format!(
                "{}/{}/{}/{}/*",
                account.as_str(),
                application.0,
                environment.0,
                component.0
            )),
            Self::Agent {
                account,
                application,
                environment,
                component,
                agent,
            } => Ok(format!(
                "{}/{}/{}/{}/{}",
                account.as_str(),
                application.0,
                environment.0,
                component.0,
                render_agent_leaf(agent)
            )),
        }
    }
}

impl RenderFragment for PolymorphicToolOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Concrete(owner) => owner.render_fragment(),
            Self::AccountTools => Ok("?account/*/*/*/*".to_string()),
            Self::AccountApplicationTools { application } => {
                Ok(format!("?account/{}/*/*/*", application.0))
            }
            Self::AccountEnvironmentTools {
                application,
                environment,
            } => Ok(format!("?account/{}/{}/*/*", application.0, environment.0)),
            Self::AccountComponentTools {
                application,
                environment,
                component,
            } => Ok(format!(
                "?account/{}/{}/{}/*",
                application.0, environment.0, component.0
            )),
            Self::AccountTool {
                application,
                environment,
                component,
                tool,
            } => Ok(format!(
                "?account/{}/{}/{}/{}",
                application.0, environment.0, component.0, tool
            )),
            Self::ApplicationTools => Ok("?app/*/*/*".to_string()),
            Self::ApplicationEnvironmentTools { environment } => {
                Ok(format!("?app/{}/*/*", environment.0))
            }
            Self::ApplicationComponentTools {
                environment,
                component,
            } => Ok(format!("?app/{}/{}/*", environment.0, component.0)),
            Self::ApplicationTool {
                environment,
                component,
                tool,
            } => Ok(format!("?app/{}/{}/{}", environment.0, component.0, tool)),
            Self::EnvTools => Ok("?env/*/*".to_string()),
            Self::EnvComponentTools { component } => Ok(format!("?env/{}/*", component.0)),
            Self::EnvTool { component, tool } => Ok(format!("?env/{}/{}", component.0, tool)),
            Self::ComponentTools => Ok("?component/*".to_string()),
            Self::ComponentTool { tool } => Ok(format!("?component/{}", tool)),
        }
    }
}

impl RenderFragment for ToolOwnerPattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::AnyTools => Ok("*/*/*/*/*".to_string()),
            Self::AccountTools { account } => Ok(format!("{}/*/*/*/*", account.as_str())),
            Self::ApplicationTools {
                account,
                application,
            } => Ok(format!("{}/{}/*/*/*", account.as_str(), application.0)),
            Self::EnvironmentTools {
                account,
                application,
                environment,
            } => Ok(format!(
                "{}/{}/{}/*/*",
                account.as_str(),
                application.0,
                environment.0
            )),
            Self::ComponentTools {
                account,
                application,
                environment,
                component,
            } => Ok(format!(
                "{}/{}/{}/{}/*",
                account.as_str(),
                application.0,
                environment.0,
                component.0
            )),
            Self::Tool {
                account,
                application,
                environment,
                component,
                tool,
            } => Ok(format!(
                "{}/{}/{}/{}/{}",
                account.as_str(),
                application.0,
                environment.0,
                component.0,
                tool
            )),
        }
    }
}

macro_rules! render_verb {
    ($ty:ty { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl RenderFragment for $ty {
            fn render_fragment(&self) -> Result<String, String> {
                Ok(match self {
                    $(Self::$variant => $name,)+
                }.to_string())
            }
        }
    };
}

render_verb!(FilesystemVerb { Read => "read", Write => "write", List => "list", Stat => "stat", Delete => "delete" });
render_verb!(NetworkVerb { Connect => "connect" });
render_verb!(EnvVerb { Read => "read" });
render_verb!(OplogVerb { Read => "read" });
render_verb!(ConfigVerb { Read => "read" });
render_verb!(SecretVerb { Hold => "hold", Mint => "mint", Reveal => "reveal" });
render_verb!(AgentVerb { Invoke => "invoke", View => "view", Delete => "delete", Interrupt => "interrupt", Resume => "resume", UpdateRevision => "update-revision", Fork => "fork", Revert => "revert", CancelInvocation => "cancel-invocation", ActivatePlugin => "activate-plugin", DeactivatePlugin => "deactivate-plugin", Debug => "debug" });
render_verb!(ToolVerb { Invoke => "invoke" });
render_verb!(KvVerb { Read => "read", Write => "write", Delete => "delete", List => "list" });
render_verb!(BlobVerb { Read => "read", Write => "write", Delete => "delete", List => "list" });
render_verb!(RdbmsVerb { Query => "query", Mutate => "mutate" });
render_verb!(CardVerb { Derive => "derive", Revoke => "revoke", Inspect => "inspect", Install => "install" });
render_verb!(SystemVerb { CreateAccount => "create-account", ImpersonateUser => "impersonate-user", ViewDefaultPlan => "view-default-plan", ViewAccountSummariesReport => "view-account-summaries-report", ViewAccountCountsReport => "view-account-counts-report" });
render_verb!(PlanVerb { View => "view", Create => "create", Update => "update" });
render_verb!(AccountVerb { View => "view", Update => "update", Delete => "delete", SetPlan => "set-plan", ViewPlan => "view-plan" });
render_verb!(AccountUsageVerb { View => "view" });
render_verb!(AccountTokenVerb { View => "view", Create => "create", Delete => "delete" });
render_verb!(AccountPluginVerb { View => "view", Register => "register", Delete => "delete", Restore => "restore" });
render_verb!(ApplicationVerb { View => "view", Create => "create", Update => "update", Delete => "delete" });
render_verb!(EnvironmentVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Deploy => "deploy", Rollback => "rollback", ViewDeployment => "view-deployment", ViewDeploymentPlan => "view-deployment-plan", ViewAgentTypes => "view-agent-types", WriteDeploymentRecord => "write-deployment-record" });
render_verb!(EnvironmentPluginGrantVerb { View => "view", Create => "create", Delete => "delete" });
render_verb!(EnvironmentDomainRegistrationVerb { View => "view", Create => "create", Delete => "delete" });
render_verb!(EnvironmentSecuritySchemeVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Restore => "restore" });
render_verb!(EnvironmentHttpApiDeploymentVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Restore => "restore" });
render_verb!(EnvironmentMcpDeploymentVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Restore => "restore" });
render_verb!(EnvironmentAgentSecretVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Restore => "restore" });
render_verb!(EnvironmentResourceDefinitionVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Restore => "restore" });
render_verb!(EnvironmentRetryPolicyVerb { View => "view", Create => "create", Update => "update", Delete => "delete", Restore => "restore" });
render_verb!(ComponentVerb { View => "view", Create => "create", Update => "update", Delete => "delete" });
render_verb!(AccountOauth2IdentityVerb { View => "view", Link => "link", Unlink => "unlink" });
render_verb!(EnvironmentInitialFilesVerb { View => "view", Update => "update", Delete => "delete", List => "list" });
render_verb!(EnvironmentKvBucketVerb { View => "view", Create => "create", Delete => "delete", Clear => "clear" });
render_verb!(EnvironmentBlobBucketVerb { View => "view", Create => "create", Delete => "delete", Clear => "clear" });
render_verb!(AccountPermissionShareVerb { View => "view", Create => "create", Update => "update", Delete => "delete" });

impl RenderFragment for AccountResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(String::new())
    }
}
impl RenderFragment for ApplicationResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(String::new())
    }
}
impl RenderFragment for AccountUsageResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(String::new())
    }
}
impl RenderFragment for SystemResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(String::new())
    }
}

fn render_dot_segments<T>(segments: &[T], render: impl FnMut(&T) -> String) -> String {
    segments.iter().map(render).collect::<Vec<_>>().join(".")
}

fn render_slash_segments<T>(segments: &[T], render: impl FnMut(&T) -> String) -> String {
    format!(
        "/{}",
        segments.iter().map(render).collect::<Vec<_>>().join("/")
    )
}

impl RenderFragment for FilesystemResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        match self {
            Self::Path(path) => Ok(render_slash_segments(
                &path.segments,
                |segment| match segment {
                    FilesystemPathSegmentPattern::Literal(value) => value.clone(),
                    FilesystemPathSegmentPattern::Star => "*".to_string(),
                    FilesystemPathSegmentPattern::GlobStar => "**".to_string(),
                },
            )),
        }
    }
}

impl RenderFragment for NetworkResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::HostPort { host, ports } => match ports {
                PortPattern::Any => host.clone(),
                PortPattern::Single(port) => format!("{host}:{port}"),
                PortPattern::Range { start, end } => format!("{host}:{start}-{end}"),
            },
        })
    }
}

impl RenderFragment for EnvResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::VarName(name) => name.0.clone(),
        })
    }
}

impl RenderFragment for OplogResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Range { start, end } => [
                start.map(|value| format!("start={value}")),
                end.map(|value| format!("end={value}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(":"),
        })
    }
}

macro_rules! render_dot_key_resource {
    ($resource:ty, $path:ident, $segment:ident, $any:ident, $key_variant:ident) => {
        impl RenderFragment for $resource {
            fn render_fragment(&self) -> Result<String, String> {
                Ok(match self {
                    Self::$any => "*".to_string(),
                    Self::$key_variant($path) => {
                        render_dot_segments(&$path.segments, |segment| match segment {
                            $segment::Literal(value) => value.clone(),
                            $segment::Star => "*".to_string(),
                            $segment::GlobStar => "**".to_string(),
                        })
                    }
                })
            }
        }
    };
}

render_dot_key_resource!(
    ConfigResourcePattern,
    path,
    ConfigKeySegmentPattern,
    Any,
    Key
);
render_dot_key_resource!(
    SecretResourcePattern,
    path,
    SecretKeySegmentPattern,
    Any,
    Key
);
render_dot_key_resource!(
    EnvironmentAgentSecretResourcePattern,
    path,
    EnvironmentAgentSecretKeySegmentPattern,
    Any,
    Key
);

impl RenderFragment for AgentResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Method(name) => name.0.clone(),
            Self::OplogIndex(index) => index.to_string(),
            Self::InvocationId(AgentInvocationIdPattern::Uuid(id)) => id.to_string(),
            Self::InvocationId(AgentInvocationIdPattern::Identifier(id)) => id.0.clone(),
            Self::PluginName(name) => name.0.clone(),
        })
    }
}

impl RenderFragment for ToolResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::AnyInvocation => "*".to_string(),
            Self::Invocation(invocation) => {
                let command = invocation.command_path.as_ref().map(|path| {
                    path.iter()
                        .map(|part| part.0.clone())
                        .collect::<Vec<_>>()
                        .join("/")
                });
                let args = invocation
                    .args
                    .iter()
                    .map(render_tool_arg)
                    .collect::<Vec<_>>()
                    .join(" ");
                match (command, args.is_empty()) {
                    (Some(command), true) => command,
                    (Some(command), false) => format!("{command} {args}"),
                    (None, true) => "*".to_string(),
                    (None, false) => args,
                }
            }
        })
    }
}

fn render_tool_arg(arg: &ToolArgPattern) -> String {
    match arg {
        ToolArgPattern::ShortFlags { flags, value } => {
            let flags = flags.iter().collect::<String>();
            match value {
                Some(value) => format!("-{flags}={}", render_tool_value(value)),
                None => format!("-{flags}"),
            }
        }
        ToolArgPattern::LongFlag { name, value } => match value {
            Some(value) => format!("--{}={}", name.0, render_tool_value(value)),
            None => format!("--{}", name.0),
        },
        ToolArgPattern::Positional(value) => render_tool_value(value),
    }
}

fn render_tool_value(value: &ToolValuePattern) -> String {
    match value {
        ToolValuePattern::Literal(value) => value.0.clone(),
        ToolValuePattern::Star => "*".to_string(),
        ToolValuePattern::GlobStar => "**".to_string(),
    }
}

impl RenderFragment for KvResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        let Self::StoreKey { store, key_pattern } = self;
        Ok(format!("{store}.{key_pattern}"))
    }
}
impl RenderFragment for BlobResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        let Self::BucketKey {
            bucket,
            key_pattern,
        } = self;
        Ok(format!("{bucket}.{key_pattern}"))
    }
}
impl RenderFragment for RdbmsResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        let Self::Table {
            database,
            schema,
            table,
        } = self;
        Ok(format!("{database}.{schema}.{table}"))
    }
}
impl RenderFragment for CardResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::InstallTarget(target) => target.render(),
        })
    }
}
impl RenderFragment for PlanResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Plan(PlanIdPattern::Identifier(id)) => id.0.clone(),
            Self::Plan(PlanIdPattern::PlanId(id)) => id.to_string(),
        })
    }
}
impl RenderFragment for AccountTokenResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Token(id) => id.to_string(),
        })
    }
}
impl RenderFragment for AccountPluginResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Name(name) => name.0.clone(),
        })
    }
}
impl RenderFragment for AccountPermissionShareResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Name(name) => name.0.clone(),
        })
    }
}
impl RenderFragment for AccountOauth2IdentityResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Identity {
                provider,
                external_id,
            } => format!("{provider}/{external_id}"),
        })
    }
}
impl RenderFragment for EnvironmentResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Revision { revision } => format!("@rev={revision}"),
        })
    }
}

macro_rules! render_environment_named_resource {
    ($($ty:ty),+ $(,)?) => {$(
        impl RenderFragment for $ty {
            fn render_fragment(&self) -> Result<String, String> {
                Ok(match self { Self::Any => "*".to_string(), Self::Name(name) => name.0.clone() })
            }
        }
    )+};
}

render_environment_named_resource!(
    EnvironmentPluginGrantResourcePattern,
    EnvironmentSecuritySchemeResourcePattern,
    EnvironmentMcpDeploymentResourcePattern,
    EnvironmentResourceDefinitionResourcePattern,
    EnvironmentRetryPolicyResourcePattern,
    EnvironmentKvBucketResourcePattern,
    EnvironmentBlobBucketResourcePattern,
);

impl RenderFragment for EnvironmentDomainRegistrationResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Domain(domain) => domain
                .labels
                .iter()
                .map(|label| label.0.clone())
                .collect::<Vec<_>>()
                .join("."),
        })
    }
}
impl RenderFragment for EnvironmentHttpApiDeploymentResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::DomainPath { domain, path_glob } => format!("{domain}.{path_glob}"),
        })
    }
}
impl RenderFragment for ComponentResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        Ok(match self {
            Self::Any => "*".to_string(),
            Self::Revision { revision } => format!("@rev={revision}"),
        })
    }
}
impl RenderFragment for EnvironmentInitialFilesResourcePattern {
    fn render_fragment(&self) -> Result<String, String> {
        let Self::Path(path) = self;
        Ok(render_slash_segments(
            &path.segments,
            |segment| match segment {
                EnvironmentInitialFilesPathSegmentPattern::Literal(value) => value.clone(),
                EnvironmentInitialFilesPathSegmentPattern::Star => "*".to_string(),
                EnvironmentInitialFilesPathSegmentPattern::GlobStar => "**".to_string(),
            },
        ))
    }
}

impl PolymorphicManifestPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }

    pub fn into_concrete_recipient(
        self,
    ) -> Result<
        PolymorphicPermissionPattern,
        crate::model::card::recipient::PolymorphicRecipientPattern,
    > {
        macro_rules! manifest_into_concrete_recipient {
            ($($variant:ident: $class:ty,)+) => {
                match self {
                    $(
                        Self::$variant(pattern) => match pattern.recipient {
                            crate::model::card::recipient::PolymorphicRecipientPattern::Concrete(recipient) => {
                                Ok(PolymorphicPermissionPattern::$variant(PolymorphicClassPermissionPattern {
                                    verb: pattern.verb,
                                    owner: pattern.owner,
                                    recipient,
                                    resource: pattern.resource,
                                }))
                            }
                            recipient => Err(recipient),
                        },
                    )+
                }
            };
        }

        card_permission_classes!(manifest_into_concrete_recipient)
    }

    pub fn monomorphize_recipient(
        self,
        context: &crate::model::card::recipient::RecipientMonomorphizationContext,
    ) -> PolymorphicPermissionPattern {
        macro_rules! manifest_monomorphize_recipient {
            ($($variant:ident: $class:ty,)+) => {
                match self {
                    $(
                        Self::$variant(pattern) => PolymorphicPermissionPattern::$variant(PolymorphicClassPermissionPattern {
                            verb: pattern.verb,
                            owner: pattern.owner,
                            recipient: pattern.recipient.monomorphize(context),
                            resource: pattern.resource,
                        }),
                    )+
                }
            };
        }
        card_permission_classes!(manifest_monomorphize_recipient)
    }
}
