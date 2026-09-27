// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use chrono::{DateTime, Utc};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_import::{McpImport, McpImportSource};
use golem_mcp_import::oauth::AuthorizationServerMetadata;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpOAuthGrantKey {
    pub environment_id: Uuid,
    pub security_scheme_id: Uuid,
    pub security_scheme_revision: i64,
    pub credential_owner_account_id: Uuid,
    pub resource_url: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthFlowSecrets {
    pub pkce_verifier: String,
    pub session: McpOAuthSession,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpImportTarget {
    Deployed(McpImportSource),
    Declared {
        environment_id: EnvironmentId,
        import: McpImport,
    },
}

impl McpImportTarget {
    pub fn environment_id(&self) -> EnvironmentId {
        match self {
            Self::Deployed(source) => source.environment_id,
            Self::Declared { environment_id, .. } => *environment_id,
        }
    }
}

impl From<&McpImportSource> for McpImportTarget {
    fn from(source: &McpImportSource) -> Self {
        let mut source = source.clone();
        source.upstream_tool_name.clear();
        Self::Deployed(source)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthSession {
    pub target: McpImportTarget,
    pub authorized_by: Uuid,
    pub server: AuthorizationServerMetadata,
    pub scopes: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthTokens {
    pub session: McpOAuthSession,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

macro_rules! redacted_debug {
    ($type:ty, $name:literal) => {
        impl Debug for $type {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!($name, " { ..redacted.. }"))
            }
        }
    };
}

redacted_debug!(McpOAuthFlowSecrets, "McpOAuthFlowSecrets");
redacted_debug!(McpOAuthSession, "McpOAuthSession");
redacted_debug!(McpOAuthTokens, "McpOAuthTokens");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpOAuthGrantStatus {
    PendingConsent,
    Exchanging,
    Granted,
    Refreshing,
    ReauthorizationRequired,
    Revoked,
}

impl McpOAuthGrantStatus {
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "pending-consent" => Ok(Self::PendingConsent),
            "exchanging" => Ok(Self::Exchanging),
            "granted" => Ok(Self::Granted),
            "refreshing" => Ok(Self::Refreshing),
            "reauthorization-required" => Ok(Self::ReauthorizationRequired),
            "revoked" => Ok(Self::Revoked),
            other => anyhow::bail!("invalid MCP OAuth grant status: {other}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct McpOAuthAuthorization {
    pub key: McpOAuthGrantKey,
    pub generation: Uuid,
}

#[derive(Clone, Debug)]
pub struct McpOAuthExchangeClaim {
    pub key: McpOAuthGrantKey,
    pub generation: Uuid,
    pub flow: McpOAuthFlowSecrets,
}

#[derive(Clone, Debug)]
pub struct McpOAuthRefreshClaim {
    pub key: McpOAuthGrantKey,
    pub generation: Uuid,
    pub tokens: McpOAuthTokens,
}

#[derive(Clone, Debug)]
pub struct McpOAuthGrant {
    pub key: McpOAuthGrantKey,
    pub generation: Uuid,
    pub status: McpOAuthGrantStatus,
    pub tokens: Option<McpOAuthTokens>,
}
