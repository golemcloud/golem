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
use super::release_grant_lifecycle::{
    ExistingGrantDecision, GrantMutationDecision, delete_grant_decision, existing_grant_decision,
    reconciliation_deletion_decision, release_is_admissible, restore_grant_decision,
};
use super::tool_middleware_release::{ToolMiddlewareReleaseError, ToolMiddlewareReleaseService};
use crate::repo::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantRepo, EnvironmentToolMiddlewareGrantRepoError,
};
use crate::repo::model::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantRecord, EnvironmentToolMiddlewareGrantWithDetailsRecord,
};
use golem_common::model::account::{AccountId, AccountSummary};
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentToolMiddlewareGrantResourcePattern,
    EnvironmentToolMiddlewareGrantVerb, PermissionTarget,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantCreation, EnvironmentToolMiddlewareGrantId,
    EnvironmentToolMiddlewareGrantReconciliation, EnvironmentToolMiddlewareGrantWithDetails,
    EnvironmentToolMiddlewareValidation, EnvironmentToolMiddlewareValidationResult,
};
use golem_common::model::tool_middleware::ToolMiddlewareName;
use golem_common::model::tool_middleware_release::{
    ToolMiddlewareRelease, ToolMiddlewareReleaseId, ToolMiddlewareReleaseReference,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ResolvedGrantedToolMiddlewareRelease {
    pub release: ToolMiddlewareRelease,
    pub owner: AccountSummary,
}

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentToolMiddlewareGrantError {
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Environment tool_middleware grant {0} not found")]
    EnvironmentToolMiddlewareGrantNotFound(EnvironmentToolMiddlewareGrantId),
    #[error("Referenced tool_middleware release not found")]
    ReferencedToolMiddlewareReleaseNotFound,
    #[error("Grant for this tool_middleware release already exists in this environment")]
    GrantAlreadyExists,
    #[error("Protected system tool_middleware grant {0} cannot be modified")]
    ProtectedToolGrant(EnvironmentToolMiddlewareGrantId),
    #[error("Administrator-managed tool_middleware grant {0} cannot be deleted automatically")]
    AdministratorManagedToolGrant(EnvironmentToolMiddlewareGrantId),
    #[error("Environment tool_middleware grant {0} is not deleted")]
    GrantNotDeleted(EnvironmentToolMiddlewareGrantId),
    #[error("Environment tool_middleware grant was modified concurrently")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentToolMiddlewareValidationError {
    #[error(transparent)]
    Grant(#[from] EnvironmentToolMiddlewareGrantError),
    #[error(transparent)]
    Publication(#[from] ToolMiddlewareReleaseError),
}

impl SafeDisplay for EnvironmentToolMiddlewareGrantError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            other => other.to_string(),
        }
    }
}

error_forwarding!(
    EnvironmentToolMiddlewareGrantError,
    EnvironmentError,
    ToolMiddlewareReleaseError
);

impl From<EnvironmentToolMiddlewareGrantRepoError> for EnvironmentToolMiddlewareGrantError {
    fn from(value: EnvironmentToolMiddlewareGrantRepoError) -> Self {
        match value {
            EnvironmentToolMiddlewareGrantRepoError::GrantAlreadyExists => Self::GrantAlreadyExists,
            EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification => {
                Self::ConcurrentModification
            }
            EnvironmentToolMiddlewareGrantRepoError::InternalError(error) => {
                Self::InternalError(error)
            }
        }
    }
}

pub struct EnvironmentToolMiddlewareGrantService {
    environment_tool_middleware_grant_repo: Arc<dyn EnvironmentToolMiddlewareGrantRepo>,
    environment_service: Arc<EnvironmentService>,
    tool_middleware_release_service: Arc<ToolMiddlewareReleaseService>,
}

impl EnvironmentToolMiddlewareGrantService {
    pub fn new(
        environment_tool_middleware_grant_repo: Arc<dyn EnvironmentToolMiddlewareGrantRepo>,
        environment_service: Arc<EnvironmentService>,
        tool_middleware_release_service: Arc<ToolMiddlewareReleaseService>,
    ) -> Self {
        Self {
            environment_tool_middleware_grant_repo,
            environment_service,
            tool_middleware_release_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        creation: EnvironmentToolMiddlewareGrantCreation,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolMiddlewareGrantWithDetails, EnvironmentToolMiddlewareGrantError>
    {
        let automatic = creation.automatic;
        let environment = self.get_environment(environment_id, auth).await?;
        let release = self
            .tool_middleware_release_service
            .resolve_user_grantable_reference(&creation.release)
            .await
            .map_err(|err| match err {
                ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound
                | ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(_) => {
                    EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound
                }
                other => other.into(),
            })?;
        let name = ToolMiddlewareName::try_from(release.release.tool_middleware_name.clone())
            .map_err(anyhow::Error::msg)?;
        authorize_environment_tool_middleware_grant_permission(
            auth,
            &environment,
            EnvironmentToolMiddlewareGrantVerb::Create,
            name,
        )
        .map_err(|_| {
            EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound
        })?;

        let release_id = ToolMiddlewareReleaseId(release.release.tool_middleware_release_id);
        let follow_coordinates = matches!(
            creation.release,
            ToolMiddlewareReleaseReference::ByCoordinates(_)
        );
        if !release_is_admissible(environment.version_check, release.release.immutable) {
            return Err(
                EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound,
            );
        }
        match self
            .environment_tool_middleware_grant_repo
            .create(EnvironmentToolMiddlewareGrantRecord::creation(
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
            Err(EnvironmentToolMiddlewareGrantRepoError::GrantAlreadyExists) => {
                let existing = self
                    .environment_tool_middleware_grant_repo
                    .get_by_environment_and_release(environment_id.0, release_id.0, true)
                    .await?
                    .ok_or(EnvironmentToolMiddlewareGrantError::GrantAlreadyExists)?;
                match existing_grant_decision(
                    existing.grant_deleted_at.is_some(),
                    existing.protected,
                    existing.automatic,
                    existing.follow_coordinates,
                    automatic,
                    follow_coordinates,
                ) {
                    ExistingGrantDecision::ReturnExisting => existing.try_into().map_err(Into::into),
                    ExistingGrantDecision::SetManagement => {
                        self.environment_tool_middleware_grant_repo
                            .set_management(
                                existing.environment_tool_middleware_grant_id,
                                environment_id.0,
                                release_id.0,
                                auth.actor_account_id().0,
                                automatic,
                                follow_coordinates,
                            )
                            .await?
                            .ok_or(EnvironmentToolMiddlewareGrantError::GrantAlreadyExists)?
                            .try_into()
                            .map_err(Into::into)
                    }
                    ExistingGrantDecision::Restore => self.environment_tool_middleware_grant_repo
                        .restore(
                            existing.environment_tool_middleware_grant_id,
                            environment_id.0,
                            release_id.0,
                            auth.actor_account_id().0,
                            automatic,
                            Some(follow_coordinates),
                        )
                        .await?
                        .ok_or(EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound)?
                        .try_into()
                        .map_err(Into::into),
                }
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn validate_tool_middlewares(
        &self,
        environment_id: EnvironmentId,
        validation: EnvironmentToolMiddlewareValidation,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolMiddlewareValidationResult, EnvironmentToolMiddlewareValidationError>
    {
        let environment = self.get_environment(environment_id, auth).await?;
        self.validate_reconciliation_for_environment(
            &environment,
            validation.grant_reconciliation,
            auth,
        )
        .await?;
        let publication_plan = self
            .tool_middleware_release_service
            .plan_publications(&environment, validation.publications, auth)
            .await?;
        Ok(EnvironmentToolMiddlewareValidationResult { publication_plan })
    }

    async fn validate_reconciliation_for_environment(
        &self,
        environment: &Environment,
        reconciliation: EnvironmentToolMiddlewareGrantReconciliation,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentToolMiddlewareGrantError> {
        for creation in reconciliation.creations {
            let release = self
                .tool_middleware_release_service
                .resolve_user_grantable_reference(&creation.release)
                .await
                .map_err(|err| match err {
                    ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound
                    | ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(_) => {
                        EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound
                    }
                    other => other.into(),
                })?;
            let name = ToolMiddlewareName::try_from(release.release.tool_middleware_name)
                .map_err(anyhow::Error::msg)?;
            authorize_environment_tool_middleware_grant_permission(
                auth,
                environment,
                EnvironmentToolMiddlewareGrantVerb::Create,
                name,
            )?;
        }
        for grant_id in reconciliation.deletions {
            let (record, grant_environment) = self
                .authorize(
                    grant_id,
                    false,
                    EnvironmentToolMiddlewareGrantVerb::Delete,
                    auth,
                )
                .await?;
            match reconciliation_deletion_decision(
                grant_environment.id == environment.id,
                record.automatic,
                record.protected,
            ) {
                GrantMutationDecision::NotReconciliable => {
                    return Err(
                        EnvironmentToolMiddlewareGrantError::EnvironmentToolMiddlewareGrantNotFound(
                            grant_id,
                        ),
                    );
                }
                GrantMutationDecision::Protected => {
                    return Err(EnvironmentToolMiddlewareGrantError::ProtectedToolGrant(
                        grant_id,
                    ));
                }
                GrantMutationDecision::Allow => {}
                GrantMutationDecision::AdministratorManaged | GrantMutationDecision::NotDeleted => {
                    unreachable!()
                }
            }
        }
        Ok(())
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<EnvironmentToolMiddlewareGrantWithDetails>, EnvironmentToolMiddlewareGrantError>
    {
        let environment = self.get_environment(environment_id, auth).await?;
        let mut result = Vec::new();
        for record in self
            .environment_tool_middleware_grant_repo
            .list_by_environment(environment_id.0)
            .await?
        {
            let name =
                ToolMiddlewareName::try_from(record.release.release.tool_middleware_name.clone())
                    .map_err(anyhow::Error::msg)?;
            if authorize_environment_tool_middleware_grant_permission(
                auth,
                &environment,
                EnvironmentToolMiddlewareGrantVerb::View,
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
        grant_id: EnvironmentToolMiddlewareGrantId,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolMiddlewareGrantWithDetails, EnvironmentToolMiddlewareGrantError>
    {
        let (record, _) = self
            .authorize(
                grant_id,
                false,
                EnvironmentToolMiddlewareGrantVerb::View,
                auth,
            )
            .await?;
        record.try_into().map_err(Into::into)
    }

    pub async fn delete(
        &self,
        grant_id: EnvironmentToolMiddlewareGrantId,
        automatic: bool,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentToolMiddlewareGrantError> {
        let (record, _) = self
            .authorize(
                grant_id,
                false,
                EnvironmentToolMiddlewareGrantVerb::Delete,
                auth,
            )
            .await?;
        match delete_grant_decision(record.protected, record.automatic, automatic) {
            GrantMutationDecision::Protected => {
                return Err(EnvironmentToolMiddlewareGrantError::ProtectedToolGrant(
                    grant_id,
                ));
            }
            GrantMutationDecision::AdministratorManaged => {
                return Err(
                    EnvironmentToolMiddlewareGrantError::AdministratorManagedToolGrant(grant_id),
                );
            }
            GrantMutationDecision::Allow => {}
            GrantMutationDecision::NotDeleted | GrantMutationDecision::NotReconciliable => {
                unreachable!()
            }
        }
        if !self
            .environment_tool_middleware_grant_repo
            .delete(grant_id.0, auth.actor_account_id().0, automatic)
            .await?
        {
            return Err(
                EnvironmentToolMiddlewareGrantError::EnvironmentToolMiddlewareGrantNotFound(
                    grant_id,
                ),
            );
        }
        Ok(())
    }

    pub async fn restore(
        &self,
        grant_id: EnvironmentToolMiddlewareGrantId,
        auth: &AuthCtx,
    ) -> Result<EnvironmentToolMiddlewareGrantWithDetails, EnvironmentToolMiddlewareGrantError>
    {
        let (record, _) = self
            .authorize(
                grant_id,
                true,
                EnvironmentToolMiddlewareGrantVerb::Restore,
                auth,
            )
            .await?;
        match restore_grant_decision(record.protected, record.grant_deleted_at.is_some()) {
            GrantMutationDecision::Protected => {
                return Err(EnvironmentToolMiddlewareGrantError::ProtectedToolGrant(
                    grant_id,
                ));
            }
            GrantMutationDecision::NotDeleted => {
                return Err(EnvironmentToolMiddlewareGrantError::GrantNotDeleted(
                    grant_id,
                ));
            }
            GrantMutationDecision::Allow => {}
            GrantMutationDecision::AdministratorManaged
            | GrantMutationDecision::NotReconciliable => unreachable!(),
        }
        self.environment_tool_middleware_grant_repo
            .restore(
                grant_id.0,
                record.environment_id,
                record.release.release.tool_middleware_release_id,
                auth.actor_account_id().0,
                false,
                None,
            )
            .await?
            .ok_or(EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound)?
            .try_into()
            .map_err(Into::into)
    }

    pub async fn resolve_active_references(
        &self,
        environment: &Environment,
        references: &[ToolMiddlewareReleaseReference],
        auth: &AuthCtx,
    ) -> Result<
        HashMap<ToolMiddlewareReleaseId, ToolMiddlewareRelease>,
        EnvironmentToolMiddlewareGrantError,
    > {
        let resolved = self
            .resolve_active_references_partial(environment, references, auth)
            .await?;
        if resolved.iter().any(Option::is_none) {
            return Err(
                EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound,
            );
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
        references: &[ToolMiddlewareReleaseReference],
        auth: &AuthCtx,
    ) -> Result<
        Vec<Option<ResolvedGrantedToolMiddlewareRelease>>,
        EnvironmentToolMiddlewareGrantError,
    > {
        let mut resolved_ids = Vec::with_capacity(references.len());
        for reference in references {
            match self
                .tool_middleware_release_service
                .resolve_published_reference(reference)
                .await
            {
                Ok(release) => resolved_ids.push(Some(release.release.tool_middleware_release_id)),
                Err(
                    ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound
                    | ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(_)
                    | ToolMiddlewareReleaseError::ParentAccountNotFound(_),
                ) => resolved_ids.push(None),
                Err(other) => return Err(other.into()),
            }
        }
        let ids = resolved_ids.iter().flatten().copied().collect::<Vec<_>>();
        let records = self
            .environment_tool_middleware_grant_repo
            .get_active_by_release_ids(environment.id.0, &ids)
            .await?;
        let mut by_id = HashMap::with_capacity(records.len());
        for record in records {
            let name =
                ToolMiddlewareName::try_from(record.release.release.tool_middleware_name.clone())
                    .map_err(anyhow::Error::msg)?;
            if authorize_environment_tool_middleware_grant_permission(
                auth,
                environment,
                EnvironmentToolMiddlewareGrantVerb::View,
                name,
            )
            .is_err()
            {
                continue;
            }
            let owner = record.release.owner();
            let release: ToolMiddlewareRelease = record.release.release.try_into()?;
            by_id.insert(
                release.id.0,
                ResolvedGrantedToolMiddlewareRelease { release, owner },
            );
        }
        Ok(resolved_ids
            .into_iter()
            .map(|id| id.and_then(|id| by_id.get(&id).cloned()))
            .collect())
    }

    pub async fn provision_protected(
        &self,
        environment_id: EnvironmentId,
        release_id: ToolMiddlewareReleaseId,
    ) -> Result<EnvironmentToolMiddlewareGrantWithDetails, EnvironmentToolMiddlewareGrantError>
    {
        self.tool_middleware_release_service
            .resolve_auto_grantable_system_release(release_id)
            .await
            .map_err(|_| {
                EnvironmentToolMiddlewareGrantError::ReferencedToolMiddlewareReleaseNotFound
            })?;
        let record = EnvironmentToolMiddlewareGrantRecord::creation(
            environment_id,
            release_id,
            true,
            true,
            false,
            AccountId::SYSTEM,
        );
        match self
            .environment_tool_middleware_grant_repo
            .create(record)
            .await
        {
            Ok(record) => record.try_into().map_err(Into::into),
            Err(EnvironmentToolMiddlewareGrantRepoError::GrantAlreadyExists) => {
                let existing = self
                    .environment_tool_middleware_grant_repo
                    .get_by_environment_and_release(environment_id.0, release_id.0, true)
                    .await?
                    .ok_or(EnvironmentToolMiddlewareGrantError::GrantAlreadyExists)?;
                if !existing.protected {
                    return Err(EnvironmentToolMiddlewareGrantError::GrantAlreadyExists);
                }
                if existing.grant_deleted_at.is_none() {
                    existing.try_into().map_err(Into::into)
                } else {
                    self.environment_tool_middleware_grant_repo
                        .restore_protected(
                            existing.environment_tool_middleware_grant_id,
                            environment_id.0,
                            release_id.0,
                            AccountId::SYSTEM.0,
                        )
                        .await?
                        .ok_or(EnvironmentToolMiddlewareGrantError::ConcurrentModification)?
                        .try_into()
                        .map_err(Into::into)
                }
            }
            Err(other) => Err(other.into()),
        }
    }

    async fn authorize(
        &self,
        grant_id: EnvironmentToolMiddlewareGrantId,
        include_deleted: bool,
        verb: EnvironmentToolMiddlewareGrantVerb,
        auth: &AuthCtx,
    ) -> Result<
        (EnvironmentToolMiddlewareGrantWithDetailsRecord, Environment),
        EnvironmentToolMiddlewareGrantError,
    > {
        let record = self
            .environment_tool_middleware_grant_repo
            .get_by_id(grant_id.0, include_deleted)
            .await?
            .ok_or(
                EnvironmentToolMiddlewareGrantError::EnvironmentToolMiddlewareGrantNotFound(
                    grant_id,
                ),
            )?;
        let environment = self
            .get_environment(EnvironmentId(record.environment_id), auth)
            .await
            .map_err(|_| {
                EnvironmentToolMiddlewareGrantError::EnvironmentToolMiddlewareGrantNotFound(
                    grant_id,
                )
            })?;
        let name =
            ToolMiddlewareName::try_from(record.release.release.tool_middleware_name.clone())
                .map_err(anyhow::Error::msg)?;
        authorize_environment_tool_middleware_grant_permission(
            auth,
            &environment,
            EnvironmentToolMiddlewareGrantVerb::View,
            name.clone(),
        )
        .map_err(|_| {
            EnvironmentToolMiddlewareGrantError::EnvironmentToolMiddlewareGrantNotFound(grant_id)
        })?;
        if verb != EnvironmentToolMiddlewareGrantVerb::View {
            authorize_environment_tool_middleware_grant_permission(auth, &environment, verb, name)?;
        }
        Ok((record, environment))
    }

    async fn get_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentToolMiddlewareGrantError> {
        self.environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(id) => {
                    EnvironmentToolMiddlewareGrantError::ParentEnvironmentNotFound(id)
                }
                other => other.into(),
            })
    }
}

fn authorize_environment_tool_middleware_grant_permission(
    auth: &AuthCtx,
    environment: &Environment,
    verb: EnvironmentToolMiddlewareGrantVerb,
    name: ToolMiddlewareName,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentToolMiddlewareGrant(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: EnvironmentToolMiddlewareGrantResourcePattern::Name(name),
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
mod tests;
