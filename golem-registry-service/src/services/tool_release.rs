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

use super::account::{AccountError, AccountService};
use crate::repo::model::tool_release::{
    TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED, TOOL_RELEASE_LIFECYCLE_PUBLISHED,
    TOOL_RELEASE_LIFECYCLE_SUPERSEDED, TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM, ToolReleaseRecord,
    ToolReleaseWithOwnerRecord,
};
use crate::repo::tool_release::{ToolReleaseRepo, ToolReleaseRepoError};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    AccountToolReleaseResourcePattern, AccountToolReleaseVerb, ClassPermissionTarget,
    PermissionTarget,
};
use golem_common::model::diff;
use golem_common::model::environment::Environment;
use golem_common::model::tool::{RegisteredTool, TOOL_METADATA_WIT_VERSION, ToolName, ToolSource};
use golem_common::model::tool_release::{
    SystemToolAvailability, SystemToolReleaseProvision, ToolPublication, ToolPublicationPlanAction,
    ToolPublicationPlanEntry, ToolRelease, ToolReleaseId, ToolReleaseLifecycle, ToolReleaseOrigin,
    ToolReleaseReference, tool_metadata_digest,
};
use golem_common::schema::tool::Tool;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ToolReleaseError {
    #[error("Tool release {0} not found")]
    ToolReleaseNotFound(ToolReleaseId),
    #[error("Tool release not found")]
    ReferencedToolReleaseNotFound,
    #[error("Parent account {0} not found")]
    ParentAccountNotFound(AccountId),
    #[error("Tool {0} selected for publication is not implemented by this deployment")]
    PublicationToolNotFound(ToolName),
    #[error("Tool {0} was selected for publication more than once")]
    DuplicatePublication(ToolName),
    #[error("Tool {0} is not owned by the publishing environment's account")]
    PublicationOwnerMismatch(ToolName),
    #[error("Tool {0} cannot be published from a host source")]
    PublicationHostSource(ToolName),
    #[error("Tool release coordinate already exists with different immutable metadata")]
    ImmutableReleaseConflict,
    #[error("A de-published tool release must be restored explicitly before publication")]
    DePublishedReleaseRequiresExplicitRestore,
    #[error("Only a published tool release can be de-published")]
    ToolReleaseNotPublished,
    #[error("Only a de-published tool release can be restored")]
    ToolReleaseNotDePublished,
    #[error("Protected system tool releases cannot be modified")]
    ProtectedToolRelease,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ToolReleaseError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            other => other.to_string(),
        }
    }
}

error_forwarding!(ToolReleaseError, AccountError, ToolReleaseRepoError);

pub struct ToolReleaseService {
    tool_release_repo: Arc<dyn ToolReleaseRepo>,
    account_service: Arc<AccountService>,
    builtin_tool_owner_account_id: AccountId,
}

enum PublicationAssessment {
    NoChange(ToolReleaseId),
    Publish,
    ImmutableConflict,
    StrictFollowingGrantConflict,
    DePublishedConflict,
}

impl ToolReleaseService {
    pub fn new(
        tool_release_repo: Arc<dyn ToolReleaseRepo>,
        account_service: Arc<AccountService>,
        builtin_tool_owner_account_id: AccountId,
    ) -> Self {
        Self {
            tool_release_repo,
            account_service,
            builtin_tool_owner_account_id,
        }
    }

    pub fn prepare_publications(
        &self,
        environment: &Environment,
        registered_tools: &BTreeMap<ToolName, RegisteredTool>,
        publish_tools: &[ToolName],
        auth: &AuthCtx,
    ) -> Result<Vec<ToolReleaseRecord>, ToolReleaseError> {
        let mut seen = BTreeSet::new();
        let mut records = Vec::with_capacity(publish_tools.len());

        for name in publish_tools {
            Self::validate_publication_selection(environment, name, &mut seen, auth)?;
            let tool = registered_tools
                .get(name)
                .ok_or_else(|| ToolReleaseError::PublicationToolNotFound(name.clone()))?;
            if tool.owner_account_id != environment.owner_account_id {
                return Err(ToolReleaseError::PublicationOwnerMismatch(name.clone()));
            }
            if !matches!(tool.source, ToolSource::Component { .. }) {
                return Err(ToolReleaseError::PublicationHostSource(name.clone()));
            }
            records.push(ToolReleaseRecord::from_registered_tool(
                tool,
                environment.version_check,
                auth.actor_account_id(),
            )?);
        }

        Ok(records)
    }

    pub async fn plan_publications(
        &self,
        environment: &Environment,
        publications: Vec<ToolPublication>,
        auth: &AuthCtx,
    ) -> Result<Vec<ToolPublicationPlanEntry>, ToolReleaseError> {
        let mut seen = BTreeSet::new();
        let mut entries = Vec::with_capacity(publications.len());
        for publication in publications {
            Self::validate_publication_selection(environment, &publication.name, &mut seen, auth)?;
            if publication.definition.name() != Some(publication.name.as_str()) {
                return Err(ToolReleaseError::PublicationToolNotFound(publication.name));
            }
            let metadata_digest =
                tool_metadata_digest(TOOL_METADATA_WIT_VERSION, &publication.definition)?;
            let assessment = self
                .assess_publication(
                    environment.owner_account_id,
                    &publication.name,
                    &publication.definition,
                    TOOL_METADATA_WIT_VERSION,
                    metadata_digest,
                    environment.version_check,
                )
                .await?;
            let (action, reason) = match assessment {
                PublicationAssessment::NoChange(_) => {
                    (ToolPublicationPlanAction::NoChange, None)
                }
                PublicationAssessment::Publish => (ToolPublicationPlanAction::Publish, None),
                PublicationAssessment::ImmutableConflict => (
                    ToolPublicationPlanAction::Conflict,
                    Some(
                        "this coordinate already has different content; use a new version or disable versionCheck for this environment"
                            .to_string(),
                    ),
                ),
                PublicationAssessment::StrictFollowingGrantConflict => (
                    ToolPublicationPlanAction::Conflict,
                    Some(
                        "this coordinate is followed by a grant in a version-checked environment; use a new version"
                            .to_string(),
                    ),
                ),
                PublicationAssessment::DePublishedConflict => (
                    ToolPublicationPlanAction::Conflict,
                    Some(
                        "this release is de-published; restore it explicitly before deploying"
                            .to_string(),
                    ),
                ),
            };
            entries.push(ToolPublicationPlanEntry {
                action,
                name: publication.name.to_string(),
                version: publication.definition.version,
                reason,
            });
        }
        Ok(entries)
    }

    pub async fn publications_need_change(
        &self,
        candidates: &mut [ToolReleaseRecord],
    ) -> Result<bool, ToolReleaseError> {
        let mut changed = false;
        for candidate in candidates {
            let name =
                ToolName::try_from(candidate.tool_name.clone()).map_err(anyhow::Error::msg)?;
            match self
                .assess_publication(
                    AccountId(candidate.owner_account_id),
                    &name,
                    candidate.tool_definition.value(),
                    &candidate.metadata_version,
                    candidate.metadata_digest.into(),
                    candidate.immutable,
                )
                .await?
            {
                PublicationAssessment::NoChange(existing_id) => {
                    candidate.tool_release_id = existing_id.0;
                }
                PublicationAssessment::Publish => changed = true,
                PublicationAssessment::ImmutableConflict
                | PublicationAssessment::StrictFollowingGrantConflict => {
                    return Err(ToolReleaseError::ImmutableReleaseConflict);
                }
                PublicationAssessment::DePublishedConflict => {
                    return Err(ToolReleaseError::DePublishedReleaseRequiresExplicitRestore);
                }
            }
        }
        Ok(changed)
    }

    fn validate_publication_selection(
        environment: &Environment,
        name: &ToolName,
        seen: &mut BTreeSet<ToolName>,
        auth: &AuthCtx,
    ) -> Result<(), ToolReleaseError> {
        if !seen.insert(name.clone()) {
            return Err(ToolReleaseError::DuplicatePublication(name.clone()));
        }
        authorize_account_tool_release_permission(
            auth,
            &environment.owner_account_email,
            AccountToolReleaseVerb::Publish,
            name.clone(),
        )?;
        Ok(())
    }

    async fn assess_publication(
        &self,
        owner_account_id: AccountId,
        name: &ToolName,
        definition: &Tool,
        metadata_version: &str,
        metadata_digest: diff::Hash,
        immutable: bool,
    ) -> Result<PublicationAssessment, ToolReleaseError> {
        let Some(existing) = self
            .tool_release_repo
            .get_by_coordinates(owner_account_id.0, name.as_str(), &definition.version)
            .await?
        else {
            return Ok(PublicationAssessment::Publish);
        };
        match existing.release.lifecycle {
            TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED => Ok(PublicationAssessment::DePublishedConflict),
            TOOL_RELEASE_LIFECYCLE_SUPERSEDED => Ok(PublicationAssessment::Publish),
            TOOL_RELEASE_LIFECYCLE_PUBLISHED => {
                let content_matches = existing.release.tool_definition.value() == definition
                    && existing.release.metadata_version == metadata_version
                    && diff::Hash::from(existing.release.metadata_digest) == metadata_digest;
                if content_matches {
                    Ok(PublicationAssessment::NoChange(ToolReleaseId(
                        existing.release.tool_release_id,
                    )))
                } else if immutable {
                    Ok(PublicationAssessment::ImmutableConflict)
                } else if self
                    .tool_release_repo
                    .strict_following_grant_exists(existing.release.tool_release_id)
                    .await?
                {
                    Ok(PublicationAssessment::StrictFollowingGrantConflict)
                } else {
                    Ok(PublicationAssessment::Publish)
                }
            }
            lifecycle => Err(anyhow::anyhow!("unknown tool release lifecycle {lifecycle}").into()),
        }
    }

    pub async fn get(
        &self,
        release_id: ToolReleaseId,
        auth: &AuthCtx,
    ) -> Result<ToolRelease, ToolReleaseError> {
        let record = self.get_record(release_id).await?;
        authorize_account_tool_release_permission(
            auth,
            &AccountEmail::new(&record.owner_account_email),
            AccountToolReleaseVerb::View,
            ToolName::try_from(record.release.tool_name.clone()).map_err(anyhow::Error::msg)?,
        )
        .map_err(|_| ToolReleaseError::ToolReleaseNotFound(release_id))?;
        record.release.try_into().map_err(Into::into)
    }

    pub async fn list_in_account(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<ToolRelease>, ToolReleaseError> {
        let account =
            self.account_service
                .get(account_id, auth)
                .await
                .map_err(|err| match err {
                    AccountError::AccountNotFound(id) => {
                        ToolReleaseError::ParentAccountNotFound(id)
                    }
                    other => other.into(),
                })?;

        let mut releases = Vec::new();
        for record in self.tool_release_repo.list_by_owner(account_id.0).await? {
            let name =
                ToolName::try_from(record.release.tool_name.clone()).map_err(anyhow::Error::msg)?;
            if authorize_account_tool_release_permission(
                auth,
                &account.email,
                AccountToolReleaseVerb::View,
                name,
            )
            .is_ok()
            {
                releases.push(record.release.try_into()?);
            }
        }
        Ok(releases)
    }

    pub async fn de_publish(
        &self,
        release_id: ToolReleaseId,
        auth: &AuthCtx,
    ) -> Result<ToolRelease, ToolReleaseError> {
        let record = self
            .authorize_management(release_id, AccountToolReleaseVerb::DePublish, auth)
            .await?;
        if record.release.origin == TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM {
            return Err(ToolReleaseError::ProtectedToolRelease);
        }
        if record.release.lifecycle != TOOL_RELEASE_LIFECYCLE_PUBLISHED {
            return Err(ToolReleaseError::ToolReleaseNotPublished);
        }
        self.tool_release_repo
            .de_publish(release_id.0, auth.actor_account_id().0)
            .await?
            .ok_or(ToolReleaseError::ToolReleaseNotFound(release_id))?
            .release
            .try_into()
            .map_err(Into::into)
    }

    pub async fn restore(
        &self,
        release_id: ToolReleaseId,
        auth: &AuthCtx,
    ) -> Result<ToolRelease, ToolReleaseError> {
        let record = self
            .authorize_management(release_id, AccountToolReleaseVerb::Restore, auth)
            .await?;
        if record.release.origin == TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM {
            return Err(ToolReleaseError::ProtectedToolRelease);
        }
        if record.release.lifecycle != TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED {
            return Err(ToolReleaseError::ToolReleaseNotDePublished);
        }
        self.tool_release_repo
            .restore(release_id.0, auth.actor_account_id().0)
            .await?
            .ok_or(ToolReleaseError::ToolReleaseNotFound(release_id))?
            .release
            .try_into()
            .map_err(Into::into)
    }

    pub(crate) async fn resolve_published_reference(
        &self,
        reference: &ToolReleaseReference,
    ) -> Result<ToolReleaseWithOwnerRecord, ToolReleaseError> {
        let (record, allows_superseded) = match reference {
            ToolReleaseReference::ById(reference) => (
                self.tool_release_repo
                    .get_by_id(reference.release_id.0)
                    .await?,
                true,
            ),
            ToolReleaseReference::ByCoordinates(reference) => {
                let account = self
                    .account_service
                    .get_by_email(reference.account.as_str(), &AuthCtx::System)
                    .await
                    .map_err(|_| ToolReleaseError::ReferencedToolReleaseNotFound)?;
                (
                    self.tool_release_repo
                        .get_by_coordinates(
                            account.id.0,
                            reference.name.as_str(),
                            &reference.version,
                        )
                        .await?,
                    false,
                )
            }
        };
        let record = record.ok_or(ToolReleaseError::ReferencedToolReleaseNotFound)?;

        let release: ToolRelease = record.release.clone().try_into()?;
        if release.lifecycle != ToolReleaseLifecycle::Published
            && !(allows_superseded && release.lifecycle == ToolReleaseLifecycle::Superseded)
        {
            return Err(ToolReleaseError::ReferencedToolReleaseNotFound);
        }
        Ok(record)
    }

    pub(crate) async fn resolve_user_grantable_reference(
        &self,
        reference: &ToolReleaseReference,
    ) -> Result<ToolReleaseWithOwnerRecord, ToolReleaseError> {
        let record = self.resolve_published_reference(reference).await?;
        let release: ToolRelease = record.release.clone().try_into()?;
        if !is_user_grantable(release.origin, release.system_availability) {
            return Err(ToolReleaseError::ReferencedToolReleaseNotFound);
        }
        Ok(record)
    }

    pub(crate) async fn resolve_auto_grantable_system_release(
        &self,
        release_id: ToolReleaseId,
    ) -> Result<ToolReleaseWithOwnerRecord, ToolReleaseError> {
        let record = self.get_record(release_id).await?;
        let release: ToolRelease = record.release.clone().try_into()?;
        if release.lifecycle != ToolReleaseLifecycle::Published
            || release.origin != ToolReleaseOrigin::ProtectedSystem
            || !matches!(
                release.system_availability,
                Some(SystemToolAvailability::AutoGranted | SystemToolAvailability::Ambient)
            )
        {
            return Err(ToolReleaseError::ReferencedToolReleaseNotFound);
        }
        Ok(record)
    }

    pub async fn provision_system_release(
        &self,
        provision: SystemToolReleaseProvision,
    ) -> Result<ToolRelease, ToolReleaseError> {
        let candidate = ToolReleaseRecord::from_system_provision(
            self.builtin_tool_owner_account_id,
            provision,
            AccountId::SYSTEM,
        )?;
        match self.tool_release_repo.create(candidate.clone()).await {
            Ok(record) => record.release.try_into().map_err(Into::into),
            Err(ToolReleaseRepoError::CoordinateAlreadyExists) => {
                let existing = self
                    .tool_release_repo
                    .get_by_coordinates(
                        candidate.owner_account_id,
                        &candidate.tool_name,
                        &candidate.tool_version,
                    )
                    .await?
                    .ok_or(ToolReleaseError::ImmutableReleaseConflict)?;
                if !existing.release.immutable_fields_match(&candidate) {
                    return Err(ToolReleaseError::ImmutableReleaseConflict);
                }
                existing.release.try_into().map_err(Into::into)
            }
            Err(other) => Err(other.into()),
        }
    }

    async fn authorize_management(
        &self,
        release_id: ToolReleaseId,
        verb: AccountToolReleaseVerb,
        auth: &AuthCtx,
    ) -> Result<ToolReleaseWithOwnerRecord, ToolReleaseError> {
        let record = self.get_record(release_id).await?;
        let name =
            ToolName::try_from(record.release.tool_name.clone()).map_err(anyhow::Error::msg)?;
        authorize_account_tool_release_permission(
            auth,
            &AccountEmail::new(&record.owner_account_email),
            AccountToolReleaseVerb::View,
            name.clone(),
        )
        .map_err(|_| ToolReleaseError::ToolReleaseNotFound(release_id))?;
        authorize_account_tool_release_permission(
            auth,
            &AccountEmail::new(&record.owner_account_email),
            verb,
            name,
        )?;
        Ok(record)
    }

    async fn get_record(
        &self,
        release_id: ToolReleaseId,
    ) -> Result<ToolReleaseWithOwnerRecord, ToolReleaseError> {
        self.tool_release_repo
            .get_by_id(release_id.0)
            .await?
            .ok_or(ToolReleaseError::ToolReleaseNotFound(release_id))
    }
}

fn is_user_grantable(
    origin: ToolReleaseOrigin,
    availability: Option<SystemToolAvailability>,
) -> bool {
    origin == ToolReleaseOrigin::Ordinary || availability == Some(SystemToolAvailability::Grantable)
}

fn authorize_account_tool_release_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    verb: AccountToolReleaseVerb,
    name: ToolName,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::AccountToolRelease(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: AccountOwnerPattern::Account {
                account: account_email.clone(),
            },
            resource: AccountToolReleaseResourcePattern::Name(name),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::is_user_grantable;
    use golem_common::model::tool_release::{SystemToolAvailability, ToolReleaseOrigin};
    use test_r::test;

    #[test]
    fn user_grants_accept_ordinary_and_grantable_system_releases_only() {
        assert!(is_user_grantable(ToolReleaseOrigin::Ordinary, None));
        assert!(is_user_grantable(
            ToolReleaseOrigin::ProtectedSystem,
            Some(SystemToolAvailability::Grantable)
        ));
        assert!(!is_user_grantable(
            ToolReleaseOrigin::ProtectedSystem,
            Some(SystemToolAvailability::AutoGranted)
        ));
        assert!(!is_user_grantable(
            ToolReleaseOrigin::ProtectedSystem,
            Some(SystemToolAvailability::Ambient)
        ));
    }
}
