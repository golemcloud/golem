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

use super::environment::{EnvironmentError, EnvironmentService};
use super::tool_release::{ToolReleaseError, ToolReleaseService};
use crate::repo::environment_tool_grant::{
    EnvironmentToolGrantRepo, EnvironmentToolGrantRepoError,
};
use crate::repo::model::environment_tool_grant::{
    EnvironmentToolGrantRecord, EnvironmentToolGrantWithDetailsRecord,
};
use golem_common::model::account::{AccountId, AccountSummary};
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentToolGrantResourcePattern, EnvironmentToolGrantVerb,
    PermissionTarget,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::environment_tool_grant::{
    EnvironmentToolGrantCreation, EnvironmentToolGrantId, EnvironmentToolGrantReconciliation,
    EnvironmentToolGrantWithDetails,
};
use golem_common::model::tool::ToolName;
use golem_common::model::tool_release::{ToolRelease, ToolReleaseId, ToolReleaseReference};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ResolvedGrantedToolRelease {
    pub release: ToolRelease,
    pub owner: AccountSummary,
}

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentToolGrantError {
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Environment tool grant {0} not found")]
    EnvironmentToolGrantNotFound(EnvironmentToolGrantId),
    #[error("Referenced tool release not found")]
    ReferencedToolReleaseNotFound,
    #[error("Grant for this tool release already exists in this environment")]
    GrantAlreadyExists,
    #[error("Protected system tool grant {0} cannot be modified")]
    ProtectedToolGrant(EnvironmentToolGrantId),
    #[error("Administrator-managed tool grant {0} cannot be deleted automatically")]
    AdministratorManagedToolGrant(EnvironmentToolGrantId),
    #[error("Environment tool grant {0} is not deleted")]
    GrantNotDeleted(EnvironmentToolGrantId),
    #[error("Environment tool grant was modified concurrently")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentToolGrantError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            other => other.to_string(),
        }
    }
}

error_forwarding!(
    EnvironmentToolGrantError,
    EnvironmentError,
    ToolReleaseError
);

impl From<EnvironmentToolGrantRepoError> for EnvironmentToolGrantError {
    fn from(value: EnvironmentToolGrantRepoError) -> Self {
        match value {
            EnvironmentToolGrantRepoError::GrantAlreadyExists => Self::GrantAlreadyExists,
            EnvironmentToolGrantRepoError::ConcurrentModification => Self::ConcurrentModification,
            EnvironmentToolGrantRepoError::InternalError(error) => Self::InternalError(error),
        }
    }
}

pub struct EnvironmentToolGrantService {
    environment_tool_grant_repo: Arc<dyn EnvironmentToolGrantRepo>,
    environment_service: Arc<EnvironmentService>,
    tool_release_service: Arc<ToolReleaseService>,
}

impl EnvironmentToolGrantService {
    pub fn new(
        environment_tool_grant_repo: Arc<dyn EnvironmentToolGrantRepo>,
        environment_service: Arc<EnvironmentService>,
        tool_release_service: Arc<ToolReleaseService>,
    ) -> Self {
        Self {
            environment_tool_grant_repo,
            environment_service,
            tool_release_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        creation: EnvironmentToolGrantCreation,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolGrantWithDetails, EnvironmentToolGrantError> {
        self.create_with_provenance(environment_id, creation, false, auth)
            .await
    }

    pub async fn create_automatic(
        &self,
        environment_id: EnvironmentId,
        creation: EnvironmentToolGrantCreation,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolGrantWithDetails, EnvironmentToolGrantError> {
        self.create_with_provenance(environment_id, creation, true, auth)
            .await
    }

    pub async fn validate_reconciliation(
        &self,
        environment_id: EnvironmentId,
        reconciliation: EnvironmentToolGrantReconciliation,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentToolGrantError> {
        let environment = self.get_environment(environment_id, auth).await?;
        for creation in reconciliation.creations {
            let release = self
                .tool_release_service
                .resolve_user_grantable_reference(&creation.release)
                .await
                .map_err(|err| match err {
                    ToolReleaseError::ReferencedToolReleaseNotFound
                    | ToolReleaseError::ToolReleaseNotFound(_) => {
                        EnvironmentToolGrantError::ReferencedToolReleaseNotFound
                    }
                    other => other.into(),
                })?;
            let name = ToolName::try_from(release.release.tool_name).map_err(anyhow::Error::msg)?;
            authorize_environment_tool_grant_permission(
                auth,
                &environment,
                EnvironmentToolGrantVerb::Create,
                name,
            )?;
        }
        for grant_id in reconciliation.deletions {
            let (record, grant_environment) = self
                .authorize(grant_id, false, EnvironmentToolGrantVerb::Delete, auth)
                .await?;
            if grant_environment.id != environment_id || !record.automatic {
                return Err(EnvironmentToolGrantError::EnvironmentToolGrantNotFound(
                    grant_id,
                ));
            }
            if record.protected {
                return Err(EnvironmentToolGrantError::ProtectedToolGrant(grant_id));
            }
        }
        Ok(())
    }

    async fn create_with_provenance(
        &self,
        environment_id: EnvironmentId,
        creation: EnvironmentToolGrantCreation,
        automatic: bool,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolGrantWithDetails, EnvironmentToolGrantError> {
        let environment = self.get_environment(environment_id, auth).await?;
        let release = self
            .tool_release_service
            .resolve_user_grantable_reference(&creation.release)
            .await
            .map_err(|err| match err {
                ToolReleaseError::ReferencedToolReleaseNotFound
                | ToolReleaseError::ToolReleaseNotFound(_) => {
                    EnvironmentToolGrantError::ReferencedToolReleaseNotFound
                }
                other => other.into(),
            })?;
        let name =
            ToolName::try_from(release.release.tool_name.clone()).map_err(anyhow::Error::msg)?;
        authorize_environment_tool_grant_permission(
            auth,
            &environment,
            EnvironmentToolGrantVerb::Create,
            name,
        )
        .map_err(|_| EnvironmentToolGrantError::ReferencedToolReleaseNotFound)?;

        let release_id = ToolReleaseId(release.release.tool_release_id);
        let follow_coordinates = matches!(creation.release, ToolReleaseReference::ByCoordinates(_));
        if !release.release.immutable && environment.version_check {
            return Err(EnvironmentToolGrantError::ReferencedToolReleaseNotFound);
        }
        match self
            .environment_tool_grant_repo
            .create(EnvironmentToolGrantRecord::creation(
                environment_id,
                release_id,
                false,
                automatic,
                follow_coordinates,
                auth.actor_account_id(),
            ))
            .await
        {
            Ok(record) => record.try_into().map_err(Into::into),
            Err(EnvironmentToolGrantRepoError::GrantAlreadyExists) => {
                let existing = self
                    .environment_tool_grant_repo
                    .get_by_environment_and_release(environment_id.0, release_id.0, true)
                    .await?
                    .ok_or(EnvironmentToolGrantError::GrantAlreadyExists)?;
                if existing.grant_deleted_at.is_none() {
                    if existing.protected
                        || (automatic && !existing.automatic)
                        || (existing.automatic == automatic
                            && existing.follow_coordinates == follow_coordinates)
                    {
                        existing.try_into().map_err(Into::into)
                    } else {
                        self.environment_tool_grant_repo
                            .set_management(
                                existing.environment_tool_grant_id,
                                environment_id.0,
                                release_id.0,
                                auth.actor_account_id().0,
                                automatic,
                                follow_coordinates,
                            )
                            .await?
                            .ok_or(EnvironmentToolGrantError::GrantAlreadyExists)?
                            .try_into()
                            .map_err(Into::into)
                    }
                } else {
                    self.environment_tool_grant_repo
                        .restore(
                            existing.environment_tool_grant_id,
                            environment_id.0,
                            release_id.0,
                            auth.actor_account_id().0,
                            automatic,
                            Some(follow_coordinates),
                        )
                        .await?
                        .ok_or(EnvironmentToolGrantError::ReferencedToolReleaseNotFound)?
                        .try_into()
                        .map_err(Into::into)
                }
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<EnvironmentToolGrantWithDetails>, EnvironmentToolGrantError> {
        let environment = self.get_environment(environment_id, auth).await?;
        let mut result = Vec::new();
        for record in self
            .environment_tool_grant_repo
            .list_by_environment(environment_id.0)
            .await?
        {
            let name = ToolName::try_from(record.release.release.tool_name.clone())
                .map_err(anyhow::Error::msg)?;
            if authorize_environment_tool_grant_permission(
                auth,
                &environment,
                EnvironmentToolGrantVerb::View,
                name,
            )
            .is_ok()
            {
                result.push(record.try_into()?);
            }
        }
        Ok(result)
    }

    pub async fn get(
        &self,
        grant_id: EnvironmentToolGrantId,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolGrantWithDetails, EnvironmentToolGrantError> {
        let (record, _) = self
            .authorize(grant_id, false, EnvironmentToolGrantVerb::View, auth)
            .await?;
        record.try_into().map_err(Into::into)
    }

    pub async fn delete(
        &self,
        grant_id: EnvironmentToolGrantId,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentToolGrantError> {
        self.delete_with_provenance(grant_id, false, auth).await
    }

    pub async fn delete_automatic(
        &self,
        grant_id: EnvironmentToolGrantId,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentToolGrantError> {
        self.delete_with_provenance(grant_id, true, auth).await
    }

    async fn delete_with_provenance(
        &self,
        grant_id: EnvironmentToolGrantId,
        automatic_only: bool,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentToolGrantError> {
        let (record, _) = self
            .authorize(grant_id, false, EnvironmentToolGrantVerb::Delete, auth)
            .await?;
        if record.protected {
            return Err(EnvironmentToolGrantError::ProtectedToolGrant(grant_id));
        }
        if automatic_only && !record.automatic {
            return Err(EnvironmentToolGrantError::AdministratorManagedToolGrant(
                grant_id,
            ));
        }
        if !self
            .environment_tool_grant_repo
            .delete(grant_id.0, auth.actor_account_id().0, automatic_only)
            .await?
        {
            return Err(EnvironmentToolGrantError::EnvironmentToolGrantNotFound(
                grant_id,
            ));
        }
        Ok(())
    }

    pub async fn restore(
        &self,
        grant_id: EnvironmentToolGrantId,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolGrantWithDetails, EnvironmentToolGrantError> {
        let (record, _) = self
            .authorize(grant_id, true, EnvironmentToolGrantVerb::Restore, auth)
            .await?;
        if record.protected {
            return Err(EnvironmentToolGrantError::ProtectedToolGrant(grant_id));
        }
        if record.grant_deleted_at.is_none() {
            return Err(EnvironmentToolGrantError::GrantNotDeleted(grant_id));
        }
        self.environment_tool_grant_repo
            .restore(
                grant_id.0,
                record.environment_id,
                record.release.release.tool_release_id,
                auth.actor_account_id().0,
                false,
                None,
            )
            .await?
            .ok_or(EnvironmentToolGrantError::ReferencedToolReleaseNotFound)?
            .try_into()
            .map_err(Into::into)
    }

    pub async fn resolve_active_references(
        &self,
        environment: &Environment,
        references: &[ToolReleaseReference],
        auth: &AuthCtx,
    ) -> Result<HashMap<ToolReleaseId, ToolRelease>, EnvironmentToolGrantError> {
        let resolved = self
            .resolve_active_references_partial(environment, references, auth)
            .await?;
        if resolved.iter().any(Option::is_none) {
            return Err(EnvironmentToolGrantError::ReferencedToolReleaseNotFound);
        }
        Ok(resolved
            .into_iter()
            .flatten()
            .map(|resolved| (resolved.release.id, resolved.release))
            .collect())
    }

    pub async fn resolve_active_references_partial(
        &self,
        environment: &Environment,
        references: &[ToolReleaseReference],
        auth: &AuthCtx,
    ) -> Result<Vec<Option<ResolvedGrantedToolRelease>>, EnvironmentToolGrantError> {
        let mut resolved_ids = Vec::with_capacity(references.len());
        for reference in references {
            match self
                .tool_release_service
                .resolve_published_reference(reference)
                .await
            {
                Ok(release) => resolved_ids.push(Some(release.release.tool_release_id)),
                Err(
                    ToolReleaseError::ReferencedToolReleaseNotFound
                    | ToolReleaseError::ToolReleaseNotFound(_)
                    | ToolReleaseError::ParentAccountNotFound(_),
                ) => resolved_ids.push(None),
                Err(other) => return Err(other.into()),
            }
        }
        let ids = resolved_ids.iter().flatten().copied().collect::<Vec<_>>();
        let records = self
            .environment_tool_grant_repo
            .get_active_by_release_ids(environment.id.0, &ids)
            .await?;
        let mut by_id = HashMap::with_capacity(records.len());
        for record in records {
            let name = ToolName::try_from(record.release.release.tool_name.clone())
                .map_err(anyhow::Error::msg)?;
            if authorize_environment_tool_grant_permission(
                auth,
                environment,
                EnvironmentToolGrantVerb::View,
                name,
            )
            .is_err()
            {
                continue;
            }
            let owner = record.release.owner();
            let release: ToolRelease = record.release.release.try_into()?;
            by_id.insert(release.id.0, ResolvedGrantedToolRelease { release, owner });
        }
        Ok(resolved_ids
            .into_iter()
            .map(|id| id.and_then(|id| by_id.get(&id).cloned()))
            .collect())
    }

    pub async fn provision_protected(
        &self,
        environment_id: EnvironmentId,
        release_id: ToolReleaseId,
    ) -> Result<EnvironmentToolGrantWithDetails, EnvironmentToolGrantError> {
        self.tool_release_service
            .resolve_auto_grantable_system_release(release_id)
            .await
            .map_err(|_| EnvironmentToolGrantError::ReferencedToolReleaseNotFound)?;
        let record = EnvironmentToolGrantRecord::creation(
            environment_id,
            release_id,
            true,
            true,
            false,
            AccountId::SYSTEM,
        );
        match self.environment_tool_grant_repo.create(record).await {
            Ok(record) => record.try_into().map_err(Into::into),
            Err(EnvironmentToolGrantRepoError::GrantAlreadyExists) => {
                let existing = self
                    .environment_tool_grant_repo
                    .get_active_by_release_ids(environment_id.0, &[release_id.0])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or(EnvironmentToolGrantError::GrantAlreadyExists)?;
                if !existing.protected {
                    return Err(EnvironmentToolGrantError::GrantAlreadyExists);
                }
                existing.try_into().map_err(Into::into)
            }
            Err(other) => Err(other.into()),
        }
    }

    async fn authorize(
        &self,
        grant_id: EnvironmentToolGrantId,
        include_deleted: bool,
        verb: EnvironmentToolGrantVerb,
        auth: &AuthCtx,
    ) -> Result<(EnvironmentToolGrantWithDetailsRecord, Environment), EnvironmentToolGrantError>
    {
        let record = self
            .environment_tool_grant_repo
            .get_by_id(grant_id.0, include_deleted)
            .await?
            .ok_or(EnvironmentToolGrantError::EnvironmentToolGrantNotFound(
                grant_id,
            ))?;
        let environment = self
            .get_environment(EnvironmentId(record.environment_id), auth)
            .await
            .map_err(|_| EnvironmentToolGrantError::EnvironmentToolGrantNotFound(grant_id))?;
        let name = ToolName::try_from(record.release.release.tool_name.clone())
            .map_err(anyhow::Error::msg)?;
        authorize_environment_tool_grant_permission(
            auth,
            &environment,
            EnvironmentToolGrantVerb::View,
            name.clone(),
        )
        .map_err(|_| EnvironmentToolGrantError::EnvironmentToolGrantNotFound(grant_id))?;
        if verb != EnvironmentToolGrantVerb::View {
            authorize_environment_tool_grant_permission(auth, &environment, verb, name)?;
        }
        Ok((record, environment))
    }

    async fn get_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentToolGrantError> {
        self.environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(id) => {
                    EnvironmentToolGrantError::ParentEnvironmentNotFound(id)
                }
                other => other.into(),
            })
    }
}

fn authorize_environment_tool_grant_permission(
    auth: &AuthCtx,
    environment: &Environment,
    verb: EnvironmentToolGrantVerb,
    name: ToolName,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentToolGrant(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: EnvironmentToolGrantResourcePattern::Name(name),
        },
    ))
}

fn environment_owner(environment: &Environment) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: environment.owner_account_email.clone(),
        application: environment.application_name.clone(),
        environment: environment.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::account::AccountId;
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::card::{EffectiveSurface, GrantSurface};
    use golem_common::model::environment::{EnvironmentName, EnvironmentRevision};
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
            version_check: false,
            security_overrides: false,
            owner_account_id: AccountId::new(),
            owner_account_email: golem_common::model::account::AccountEmail::new(
                "owner@example.com",
            ),
            current_deployment: None,
        }
    }

    fn view_permission(environment: &Environment, name: ToolName) -> PermissionTarget {
        PermissionTarget::EnvironmentToolGrant(ClassPermissionTarget {
            verb: Some(EnvironmentToolGrantVerb::View),
            owner: environment_owner(environment),
            resource: EnvironmentToolGrantResourcePattern::Name(name),
        })
    }

    #[test]
    fn name_scoped_view_permission_authorizes_only_that_granted_tool() {
        let environment = test_environment();
        let permitted = ToolName::try_from("search").unwrap();
        let denied = ToolName::try_from("payments").unwrap();
        let auth = AuthCtx::agent_with_effective_surface(
            environment.owner_account_id,
            environment.owner_account_email.clone(),
            EffectiveSurface {
                source_card_ids: Vec::new(),
                lower: vec![GrantSurface {
                    positive: vec![view_permission(&environment, permitted.clone())],
                    negative: Vec::new(),
                }],
                upper: Vec::new(),
            },
        );

        assert!(
            authorize_environment_tool_grant_permission(
                &auth,
                &environment,
                EnvironmentToolGrantVerb::View,
                permitted,
            )
            .is_ok()
        );
        assert!(
            authorize_environment_tool_grant_permission(
                &auth,
                &environment,
                EnvironmentToolGrantVerb::View,
                denied,
            )
            .is_err()
        );
    }
}
