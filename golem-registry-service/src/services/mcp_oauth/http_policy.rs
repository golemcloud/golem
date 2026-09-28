// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::McpOAuthError;
use crate::services::account_usage::AccountUsageService;
use crate::services::deployment::authorize_environment_permission;
use crate::services::security_scheme::authorize_security_scheme_permission;
use golem_common::model::account::AccountId;
use golem_common::model::card::network_target::http_target;
use golem_common::model::card::{EnvironmentSecuritySchemeVerb, EnvironmentVerb};
use golem_common::model::environment::Environment;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_mcp_import::transport::TransportError;
use golem_mcp_import::transport::sender::HttpPolicy;
use golem_service_base::model::auth::AuthCtx;
use http::Uri;
use std::sync::Arc;

pub struct McpHttpPolicy {
    runtime_auth: Option<AuthCtx>,
    owner: AccountId,
    usage: Arc<AccountUsageService>,
}

impl McpHttpPolicy {
    /// The trusted caller captures the effective surface at its live boundary.
    pub fn runtime(
        auth: AuthCtx,
        environment: &Environment,
        usage: Arc<AccountUsageService>,
    ) -> Result<Self, McpOAuthError> {
        if !matches!(auth, AuthCtx::Agent(_)) {
            return Err(TransportError::Denied.into());
        }
        if auth.access_account_id() != environment.owner_account_id {
            return Err(McpOAuthError::OwnerMismatch);
        }
        Ok(Self {
            owner: auth.access_account_id(),
            runtime_auth: Some(auth),
            usage,
        })
    }

    /// Consent is a scheme-administration operation, not an agent network call.
    /// Provider discovery still validates every URL independently of this policy.
    pub fn operator(
        auth: &AuthCtx,
        environment: &Environment,
        scheme: &SecuritySchemeName,
        usage: Arc<AccountUsageService>,
    ) -> Result<Self, McpOAuthError> {
        authorize_security_scheme_permission(
            auth,
            environment,
            Some(scheme),
            EnvironmentSecuritySchemeVerb::Update,
        )?;
        Ok(Self {
            runtime_auth: None,
            owner: environment.owner_account_id,
            usage,
        })
    }

    pub fn inspection(
        auth: &AuthCtx,
        environment: &Environment,
        usage: Arc<AccountUsageService>,
    ) -> Result<Self, McpOAuthError> {
        for verb in [EnvironmentVerb::View, EnvironmentVerb::ViewDeployment] {
            authorize_environment_permission(auth, environment, verb)
                .map_err(|_| McpOAuthError::ImportNotFound)?;
        }
        authorize_environment_permission(auth, environment, EnvironmentVerb::ViewTools)?;
        Ok(Self {
            runtime_auth: None,
            owner: environment.owner_account_id,
            usage,
        })
    }

    pub fn preview(
        auth: &AuthCtx,
        environment: &Environment,
        usage: Arc<AccountUsageService>,
    ) -> Result<Self, McpOAuthError> {
        authorize_environment_permission(auth, environment, EnvironmentVerb::View)
            .map_err(|_| McpOAuthError::ImportNotFound)?;
        authorize_environment_permission(auth, environment, EnvironmentVerb::Deploy)?;
        authorize_environment_permission(auth, environment, EnvironmentVerb::ViewTools)?;
        Ok(Self {
            runtime_auth: None,
            owner: environment.owner_account_id,
            usage,
        })
    }

    /// Check cached observations and coalesced fetch admission without charging
    /// another request. Only the fetch owner admits actual network attempts.
    pub fn authorize(&self, target: &Uri) -> Result<(), McpOAuthError> {
        let target = http_target(&target.to_string()).map_err(|_| TransportError::Denied)?;
        if let Some(auth) = &self.runtime_auth {
            auth.authorize_permission(&target.permission)
                .map_err(|_| TransportError::Denied)?;
        }
        Ok(())
    }
}

impl HttpPolicy for McpHttpPolicy {
    type Error = McpOAuthError;

    async fn admit(&mut self, target: &Uri) -> Result<(), Self::Error> {
        self.authorize(target)?;
        self.usage.record_http_call(self.owner).await?;
        Ok(())
    }
}
