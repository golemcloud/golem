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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardAlgebraError {
    InvalidOwnerPath(String),
    InvalidRecipientPath(String),
    DerivationNotSubsumed { grant: PatternGrant },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct OwnerPathPattern(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct RecipientPathPattern(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum VerbPattern {
    Any,
    Exact(String),
}

impl VerbPattern {
    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ResourcePattern {
    Any,
    Empty,
    Exact(String),
    Glob(String),
    NetworkHostPort {
        host: String,
        ports: PortPattern,
    },
    OplogRange {
        start: Option<u64>,
        end: Option<u64>,
    },
}

impl ResourcePattern {
    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Empty, Self::Empty) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (
                Self::NetworkHostPort {
                    host: ah,
                    ports: ap,
                },
                Self::NetworkHostPort {
                    host: bh,
                    ports: bp,
                },
            ) => glob_subsumes(ah, bh) && ap.subsumes(bp),
            (
                Self::OplogRange {
                    start: as_,
                    end: ae,
                },
                Self::OplogRange { start: bs, end: be },
            ) => range_subsumes(*as_, *ae, *bs, *be),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PortPattern {
    Any,
    Single(u16),
    Range { start: u16, end: u16 },
}

impl PortPattern {
    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Single(a), Self::Single(b)) => a == b,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Single(b),
            ) => as_ <= b && b <= ae,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Range { start: bs, end: be },
            ) => as_ <= bs && be <= ae,
            (Self::Single(_), Self::Any | Self::Range { .. }) => false,
            (Self::Range { .. }, Self::Any) => false,
        }
    }
}

macro_rules! define_permission_patterns {
    ($(
        $variant:ident($pattern:ident, $verb:ident, $resource:ident) => $class_name:literal
    ),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
            #[cfg_attr(feature = "full", desert(transparent))]
            pub struct $verb(pub VerbPattern);

            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
            #[cfg_attr(feature = "full", desert(transparent))]
            pub struct $resource(pub ResourcePattern);

            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
            pub struct $pattern {
                pub verb: $verb,
                pub resource: $resource,
            }
        )+

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionPattern {
            $($variant($pattern)),+
        }

        impl PermissionPattern {
            pub fn class_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $class_name),+
                }
            }

            pub fn subsumes(&self, other: &Self) -> bool {
                match (self, other) {
                    $((Self::$variant(a), Self::$variant(b)) => {
                        a.verb.0.subsumes(&b.verb.0) && a.resource.0.subsumes(&b.resource.0)
                    }),+,
                    _ => false,
                }
            }
        }
    };
}

define_permission_patterns! {
    Filesystem(FilesystemPermissionPattern, FilesystemVerb, FilesystemResourcePattern) => "filesystem",
    Network(NetworkPermissionPattern, NetworkVerb, NetworkResourcePattern) => "network",
    Env(EnvPermissionPattern, EnvVerb, EnvResourcePattern) => "env",
    Oplog(OplogPermissionPattern, OplogVerb, OplogResourcePattern) => "oplog",
    Config(ConfigPermissionPattern, ConfigVerb, ConfigResourcePattern) => "config",
    Secret(SecretPermissionPattern, SecretVerb, SecretResourcePattern) => "secret",
    Agent(AgentPermissionPattern, AgentVerb, AgentResourcePattern) => "agent",
    Tool(ToolPermissionPattern, ToolVerb, ToolResourcePattern) => "tool",
    Kv(KvPermissionPattern, KvVerb, KvResourcePattern) => "kv",
    Blob(BlobPermissionPattern, BlobVerb, BlobResourcePattern) => "blob",
    Rdbms(RdbmsPermissionPattern, RdbmsVerb, RdbmsResourcePattern) => "rdbms",
    Card(CardPermissionPattern, CardVerb, CardResourcePattern) => "card",
    System(SystemPermissionPattern, SystemVerb, SystemResourcePattern) => "system",
    Plan(PlanPermissionPattern, PlanVerb, PlanResourcePattern) => "plan",
    Account(AccountPermissionPattern, AccountVerb, AccountResourcePattern) => "account",
    AccountUsage(AccountUsagePermissionPattern, AccountUsageVerb, AccountUsageResourcePattern) => "account.usage",
    AccountToken(AccountTokenPermissionPattern, AccountTokenVerb, AccountTokenResourcePattern) => "account.token",
    AccountPlugin(AccountPluginPermissionPattern, AccountPluginVerb, AccountPluginResourcePattern) => "account.plugin",
    Application(ApplicationPermissionPattern, ApplicationVerb, ApplicationResourcePattern) => "application",
    Environment(EnvironmentPermissionPattern, EnvironmentVerb, EnvironmentResourcePattern) => "environment",
    EnvironmentShare(EnvironmentSharePermissionPattern, EnvironmentShareVerb, EnvironmentShareResourcePattern) => "environment.share",
    EnvironmentPluginGrant(EnvironmentPluginGrantPermissionPattern, EnvironmentPluginGrantVerb, EnvironmentPluginGrantResourcePattern) => "environment.plugin-grant",
    EnvironmentDomainRegistration(EnvironmentDomainRegistrationPermissionPattern, EnvironmentDomainRegistrationVerb, EnvironmentDomainRegistrationResourcePattern) => "environment.domain-registration",
    EnvironmentSecurityScheme(EnvironmentSecuritySchemePermissionPattern, EnvironmentSecuritySchemeVerb, EnvironmentSecuritySchemeResourcePattern) => "environment.security-scheme",
    EnvironmentHttpApiDeployment(EnvironmentHttpApiDeploymentPermissionPattern, EnvironmentHttpApiDeploymentVerb, EnvironmentHttpApiDeploymentResourcePattern) => "environment.http-api-deployment",
    EnvironmentMcpDeployment(EnvironmentMcpDeploymentPermissionPattern, EnvironmentMcpDeploymentVerb, EnvironmentMcpDeploymentResourcePattern) => "environment.mcp-deployment",
    EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern, EnvironmentAgentSecretVerb, EnvironmentAgentSecretResourcePattern) => "environment.agent-secret",
    EnvironmentResourceDefinition(EnvironmentResourceDefinitionPermissionPattern, EnvironmentResourceDefinitionVerb, EnvironmentResourceDefinitionResourcePattern) => "environment.resource-definition",
    EnvironmentRetryPolicy(EnvironmentRetryPolicyPermissionPattern, EnvironmentRetryPolicyVerb, EnvironmentRetryPolicyResourcePattern) => "environment.retry-policy",
    Component(ComponentPermissionPattern, ComponentVerb, ComponentResourcePattern) => "component",
    AccountOauth2Identity(AccountOauth2IdentityPermissionPattern, AccountOauth2IdentityVerb, AccountOauth2IdentityResourcePattern) => "account.oauth2-identity",
    EnvironmentInitialFiles(EnvironmentInitialFilesPermissionPattern, EnvironmentInitialFilesVerb, EnvironmentInitialFilesResourcePattern) => "environment.initial-files",
    EnvironmentKvBucket(EnvironmentKvBucketPermissionPattern, EnvironmentKvBucketVerb, EnvironmentKvBucketResourcePattern) => "environment.kv-bucket",
    EnvironmentBlobBucket(EnvironmentBlobBucketPermissionPattern, EnvironmentBlobBucketVerb, EnvironmentBlobBucketResourcePattern) => "environment.blob-bucket",
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PatternGrant {
    pub owner: OwnerPathPattern,
    pub recipient: RecipientPathPattern,
    pub permission: PermissionPattern,
}

impl PatternGrant {
    pub fn subsumes(&self, other: &Self) -> Result<bool, CardAlgebraError> {
        Ok(self.owner.subsumes(&other.owner)?
            && self.recipient.subsumes(&other.recipient)?
            && self.permission.subsumes(&other.permission))
    }

    pub fn applies_to_recipient(
        &self,
        holder: &RecipientPathPattern,
    ) -> Result<bool, CardAlgebraError> {
        self.recipient.matches_holder(holder)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
    pub polymorphic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantSurface {
    pub positive: Vec<PatternGrant>,
    pub negative: Vec<PatternGrant>,
}

impl GrantSurface {
    pub fn allows(&self, request: &PatternGrant) -> Result<bool, CardAlgebraError> {
        let granted = self
            .positive
            .iter()
            .any(|grant| grant.subsumes(request).unwrap_or(false));
        if !granted {
            return Ok(false);
        }

        let denied = self
            .negative
            .iter()
            .any(|grant| grant.subsumes(request).unwrap_or(false));
        Ok(!denied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSurface {
    lower: GrantSurface,
    upper: Vec<GrantSurface>,
}

impl EffectiveSurface {
    pub fn from_cards(
        cards: &[Card],
        holder: &RecipientPathPattern,
    ) -> Result<Self, CardAlgebraError> {
        let mut lower_positive = Vec::new();
        let mut lower_negative = Vec::new();
        let mut upper = Vec::new();

        for card in cards {
            lower_positive.extend(filter_by_recipient(&card.lower_positive, holder)?);
            lower_negative.extend(filter_by_recipient(&card.lower_negative, holder)?);

            let upper_positive = filter_by_recipient(&card.upper_positive, holder)?;
            let upper_negative = filter_by_recipient(&card.upper_negative, holder)?;
            if !upper_positive.is_empty() || !upper_negative.is_empty() {
                upper.push(GrantSurface {
                    positive: upper_positive,
                    negative: upper_negative,
                });
            }
        }

        Ok(Self {
            lower: GrantSurface {
                positive: lower_positive,
                negative: lower_negative,
            },
            upper,
        })
    }

    pub fn authorize(&self, request: &PatternGrant) -> Result<bool, CardAlgebraError> {
        if !self.lower.allows(request)? {
            return Ok(false);
        }

        for surface in &self.upper {
            if !surface.allows(request)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    pub fn validates_derivation(
        parent_cards: &[Card],
        holder: &RecipientPathPattern,
        child_lower: &[PatternGrant],
        child_upper: &[PatternGrant],
    ) -> Result<(), CardAlgebraError> {
        for grant in child_lower {
            if !union_lower_allows(parent_cards, holder, grant)? {
                return Err(CardAlgebraError::DerivationNotSubsumed {
                    grant: grant.clone(),
                });
            }
        }

        for grant in child_upper {
            if !union_upper_allows(parent_cards, holder, grant)? {
                return Err(CardAlgebraError::DerivationNotSubsumed {
                    grant: grant.clone(),
                });
            }
        }

        Ok(())
    }
}

impl OwnerPathPattern {
    pub fn subsumes(&self, other: &Self) -> Result<bool, CardAlgebraError> {
        let left = parse_path(&self.0).map_err(CardAlgebraError::InvalidOwnerPath)?;
        let right = parse_path(&other.0).map_err(CardAlgebraError::InvalidOwnerPath)?;
        Ok(path_subsumes(&left, &right))
    }
}

impl RecipientPathPattern {
    pub fn subsumes(&self, other: &Self) -> Result<bool, CardAlgebraError> {
        let left = parse_recipient(&self.0)?;
        let right = parse_recipient(&other.0)?;
        Ok(path_subsumes(&left, &right))
    }

    pub fn matches_holder(&self, holder: &Self) -> Result<bool, CardAlgebraError> {
        let pattern = parse_recipient(&self.0)?;
        let holder = parse_recipient(&holder.0)?;
        Ok(pattern.len() <= holder.len() && path_subsumes(&pattern, &holder[..pattern.len()]))
    }
}

fn filter_by_recipient(
    grants: &[PatternGrant],
    holder: &RecipientPathPattern,
) -> Result<Vec<PatternGrant>, CardAlgebraError> {
    grants
        .iter()
        .filter_map(|grant| match grant.applies_to_recipient(holder) {
            Ok(true) => Some(Ok(grant.clone())),
            Ok(false) => None,
            Err(err) => Some(Err(err)),
        })
        .collect()
}

fn union_lower_allows(
    cards: &[Card],
    holder: &RecipientPathPattern,
    grant: &PatternGrant,
) -> Result<bool, CardAlgebraError> {
    for card in cards {
        let surface = GrantSurface {
            positive: filter_by_recipient(&card.lower_positive, holder)?,
            negative: filter_by_recipient(&card.lower_negative, holder)?,
        };
        if surface.allows(grant)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn union_upper_allows(
    cards: &[Card],
    holder: &RecipientPathPattern,
    grant: &PatternGrant,
) -> Result<bool, CardAlgebraError> {
    for card in cards {
        let positive = filter_by_recipient(&card.upper_positive, holder)?;
        let negative = filter_by_recipient(&card.upper_negative, holder)?;
        if positive.is_empty() && negative.is_empty() {
            return Ok(true);
        }
        if (GrantSurface { positive, negative }).allows(grant)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_recipient(path: &str) -> Result<Vec<&str>, CardAlgebraError> {
    let parsed = if path == "*" {
        vec!["*", "*", "*", "*", "*"]
    } else {
        parse_path(path).map_err(CardAlgebraError::InvalidRecipientPath)?
    };

    match parsed.len() {
        1 | 2 | 5 => {
            if parsed.len() == 1 && parsed[0].contains('(') {
                Err(CardAlgebraError::InvalidRecipientPath(path.to_string()))
            } else {
                Ok(parsed)
            }
        }
        _ => Err(CardAlgebraError::InvalidRecipientPath(path.to_string())),
    }
}

fn parse_path(path: &str) -> Result<Vec<&str>, String> {
    if path.is_empty() {
        Ok(Vec::new())
    } else if path.split('/').any(str::is_empty) {
        Err(path.to_string())
    } else {
        Ok(path.split('/').collect())
    }
}

fn path_subsumes(left: &[&str], right: &[&str]) -> bool {
    let max_len = left.len().max(right.len());
    for idx in 0..max_len {
        let left = left.get(idx).copied().unwrap_or("*");
        let right = right.get(idx).copied().unwrap_or("*");
        if !segment_subsumes(left, right) {
            return false;
        }
    }
    true
}

fn segment_subsumes(left: &str, right: &str) -> bool {
    left == "*" || left == right
}

fn glob_subsumes(left: &str, right: &str) -> bool {
    left == "**" || left == "*" || left == right
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    if pattern == "**" || pattern == "*" {
        true
    } else if let Some(prefix) = pattern.strip_suffix("**") {
        value.starts_with(prefix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
}

fn range_subsumes(
    left_start: Option<u64>,
    left_end: Option<u64>,
    right_start: Option<u64>,
    right_end: Option<u64>,
) -> bool {
    left_start.unwrap_or(0) <= right_start.unwrap_or(0)
        && right_end.unwrap_or(u64::MAX) <= left_end.unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn fs(
        owner: &str,
        recipient: &str,
        verb: VerbPattern,
        resource: ResourcePattern,
    ) -> PatternGrant {
        PatternGrant {
            owner: OwnerPathPattern(owner.to_string()),
            recipient: RecipientPathPattern(recipient.to_string()),
            permission: PermissionPattern::Filesystem(FilesystemPermissionPattern {
                verb: FilesystemVerb(verb),
                resource: FilesystemResourcePattern(resource),
            }),
        }
    }

    fn card(lower_positive: Vec<PatternGrant>, upper_positive: Vec<PatternGrant>) -> Card {
        Card {
            card_id: Uuid::new_v4(),
            parent_ids: Vec::new(),
            lower_positive,
            lower_negative: Vec::new(),
            upper_positive,
            upper_negative: Vec::new(),
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
            polymorphic: false,
        }
    }

    #[test]
    fn owner_truncation_subsumes_trailing_segments() {
        let broad = OwnerPathPattern("acme/shop".to_string());
        let narrow = OwnerPathPattern("acme/shop/prod/cart/agent".to_string());

        assert!(broad.subsumes(&narrow).unwrap());
        assert!(!narrow.subsumes(&broad).unwrap());
    }

    #[test]
    fn recipient_depths_are_validated() {
        let invalid = RecipientPathPattern("acme/shop/prod".to_string());
        let valid = RecipientPathPattern("acme/shop/prod/cart/agent".to_string());

        assert!(invalid.subsumes(&valid).is_err());
        assert!(
            RecipientPathPattern("agent(*)".to_string())
                .subsumes(&valid)
                .is_err()
        );
    }

    #[test]
    fn glob_resource_subsumes_concrete_resource() {
        let broad = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Glob("/data/**".to_string()),
        );
        let narrow = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Exact("/data/item.json".to_string()),
        );

        assert!(broad.subsumes(&narrow).unwrap());
        assert!(!narrow.subsumes(&broad).unwrap());
    }

    #[test]
    fn effective_surface_requires_lower_and_all_upper_bounds() {
        let holder = RecipientPathPattern("acme/shop/prod/cart/agent".to_string());
        let read_all = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Glob("/data/**".to_string()),
        );
        let read_secret = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Exact("/data/secret.txt".to_string()),
        );
        let read_public = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Exact("/data/public.txt".to_string()),
        );

        let lower = card(vec![read_all], Vec::new());
        let ceiling = card(Vec::new(), vec![read_public.clone()]);
        let surface = EffectiveSurface::from_cards(&[lower, ceiling], &holder).unwrap();

        assert!(surface.authorize(&read_public).unwrap());
        assert!(!surface.authorize(&read_secret).unwrap());
    }

    #[test]
    fn derivation_must_be_subsumed_by_parent_union() {
        let holder = RecipientPathPattern("acme/shop/prod/cart/agent".to_string());
        let parent_grant = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Glob("/data/**".to_string()),
        );
        let child_grant = fs(
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Exact("/data/file.txt".to_string()),
        );
        let denied_child = fs(
            "other/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            VerbPattern::Exact("read".to_string()),
            ResourcePattern::Exact("/data/file.txt".to_string()),
        );

        let parent = card(vec![parent_grant], Vec::new());

        assert!(
            EffectiveSurface::validates_derivation(
                std::slice::from_ref(&parent),
                &holder,
                std::slice::from_ref(&child_grant),
                &[]
            )
            .is_ok()
        );
        assert!(
            EffectiveSurface::validates_derivation(&[parent], &holder, &[denied_child], &[])
                .is_err()
        );
    }
}
