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
use super::release_grant_lifecycle::{
    PublicationDecision, ReleaseLifecycle, ReleaseManagementDecision, publication_decision,
    release_management_decision,
};
use crate::repo::model::tool_middleware_release::{
    TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED, TOOL_RELEASE_LIFECYCLE_PUBLISHED,
    TOOL_RELEASE_LIFECYCLE_SUPERSEDED, TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM,
    ToolMiddlewareReleaseRecord, ToolMiddlewareReleaseWithOwnerRecord,
};
use crate::repo::tool_middleware_release::{
    ToolMiddlewareReleaseRepo, ToolMiddlewareReleaseRepoError,
};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    AccountToolMiddlewareReleaseResourcePattern, AccountToolMiddlewareReleaseVerb,
    ClassPermissionTarget, PermissionTarget,
};
use golem_common::model::diff;
use golem_common::model::environment::Environment;
use golem_common::model::tool_middleware::{
    RegisteredToolMiddleware, TOOL_MIDDLEWARE_METADATA_WIT_VERSION, ToolMiddlewareName,
    ToolMiddlewareSource,
};
use golem_common::model::tool_middleware_release::{
    ToolMiddlewarePublication, ToolMiddlewarePublicationPlanAction,
    ToolMiddlewarePublicationPlanEntry, ToolMiddlewareRelease, ToolMiddlewareReleaseId,
    ToolMiddlewareReleaseLifecycle, ToolMiddlewareReleaseOrigin, ToolMiddlewareReleaseReference,
    tool_middleware_metadata_digest,
};
use golem_common::schema::tool::ToolMiddleware;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ToolMiddlewareReleaseError {
    #[error("ToolMiddleware release {0} not found")]
    ToolMiddlewareReleaseNotFound(ToolMiddlewareReleaseId),
    #[error("ToolMiddleware release not found")]
    ReferencedToolMiddlewareReleaseNotFound,
    #[error("Parent account {0} not found")]
    ParentAccountNotFound(AccountId),
    #[error("ToolMiddleware {0} selected for publication is not implemented by this deployment")]
    PublicationToolNotFound(ToolMiddlewareName),
    #[error("ToolMiddleware {0} was selected for publication more than once")]
    DuplicatePublication(ToolMiddlewareName),
    #[error("ToolMiddleware {0} is not owned by the publishing environment's account")]
    PublicationOwnerMismatch(ToolMiddlewareName),
    #[error("ToolMiddleware {0} cannot be published from a host source")]
    PublicationHostSource(ToolMiddlewareName),
    #[error("ToolMiddleware release coordinate already exists with different immutable metadata")]
    ImmutableReleaseConflict,
    #[error(
        "A de-published tool_middleware release must be restored explicitly before publication"
    )]
    DePublishedReleaseRequiresExplicitRestore,
    #[error("Only a published tool_middleware release can be de-published")]
    ToolMiddlewareReleaseNotPublished,
    #[error("Only a de-published tool_middleware release can be restored")]
    ToolMiddlewareReleaseNotDePublished,
    #[error("Protected system tool_middleware releases cannot be modified")]
    ProtectedToolMiddlewareRelease,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ToolMiddlewareReleaseError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            other => other.to_string(),
        }
    }
}

error_forwarding!(
    ToolMiddlewareReleaseError,
    AccountError,
    ToolMiddlewareReleaseRepoError
);

pub struct ToolMiddlewareReleaseService {
    tool_middleware_release_repo: Arc<dyn ToolMiddlewareReleaseRepo>,
    account_service: Arc<AccountService>,
}

type ToolMiddlewarePublicationAssessment = PublicationDecision<ToolMiddlewareReleaseId>;

impl ToolMiddlewareReleaseService {
    pub fn new(
        tool_middleware_release_repo: Arc<dyn ToolMiddlewareReleaseRepo>,
        account_service: Arc<AccountService>,
    ) -> Self {
        Self {
            tool_middleware_release_repo,
            account_service,
        }
    }

    pub fn prepare_publications(
        &self,
        environment: &Environment,
        registered_tool_middlewares: &BTreeMap<ToolMiddlewareName, RegisteredToolMiddleware>,
        publish_tool_middlewares: &[ToolMiddlewareName],
        auth: &AuthCtx,
    ) -> Result<Vec<ToolMiddlewareReleaseRecord>, ToolMiddlewareReleaseError> {
        let mut seen = BTreeSet::new();
        let mut records = Vec::with_capacity(publish_tool_middlewares.len());

        for name in publish_tool_middlewares {
            Self::validate_publication_selection(environment, name, &mut seen, auth)?;
            let tool_middleware = registered_tool_middlewares
                .get(name)
                .ok_or_else(|| ToolMiddlewareReleaseError::PublicationToolNotFound(name.clone()))?;
            if tool_middleware.owner_account_id != environment.owner_account_id {
                return Err(ToolMiddlewareReleaseError::PublicationOwnerMismatch(
                    name.clone(),
                ));
            }
            if !matches!(
                tool_middleware.source,
                ToolMiddlewareSource::Component { .. }
            ) {
                return Err(ToolMiddlewareReleaseError::PublicationHostSource(
                    name.clone(),
                ));
            }
            records.push(
                ToolMiddlewareReleaseRecord::from_registered_tool_middleware(
                    tool_middleware,
                    environment.version_check,
                    auth.actor_account_id(),
                )?,
            );
        }

        Ok(records)
    }

    pub async fn plan_publications(
        &self,
        environment: &Environment,
        publications: Vec<ToolMiddlewarePublication>,
        auth: &AuthCtx,
    ) -> Result<Vec<ToolMiddlewarePublicationPlanEntry>, ToolMiddlewareReleaseError> {
        let mut seen = BTreeSet::new();
        let mut entries = Vec::with_capacity(publications.len());
        for publication in publications {
            Self::validate_publication_selection(environment, &publication.name, &mut seen, auth)?;
            if publication.definition.name != publication.name.as_str() {
                return Err(ToolMiddlewareReleaseError::PublicationToolNotFound(
                    publication.name,
                ));
            }
            let metadata_digest = tool_middleware_metadata_digest(
                TOOL_MIDDLEWARE_METADATA_WIT_VERSION,
                &publication.definition,
            )?;
            let assessment = self
                .assess_publication(
                    environment.owner_account_id,
                    &publication.name,
                    &publication.definition,
                    TOOL_MIDDLEWARE_METADATA_WIT_VERSION,
                    metadata_digest,
                    environment.version_check,
                )
                .await?;
            let (action, reason) = match assessment {
                ToolMiddlewarePublicationAssessment::NoChange(_) => {
                    (ToolMiddlewarePublicationPlanAction::NoChange, None)
                }
                ToolMiddlewarePublicationAssessment::Publish => (ToolMiddlewarePublicationPlanAction::Publish, None),
                ToolMiddlewarePublicationAssessment::ImmutableConflict => (
                    ToolMiddlewarePublicationPlanAction::Conflict,
                    Some(
                        "this coordinate already has different content; use a new version or disable versionCheck for this environment"
                            .to_string(),
                    ),
                ),
                ToolMiddlewarePublicationAssessment::StrictFollowingGrantConflict => (
                    ToolMiddlewarePublicationPlanAction::Conflict,
                    Some(
                        "this coordinate is followed by a grant in a version-checked environment; use a new version"
                            .to_string(),
                    ),
                ),
                ToolMiddlewarePublicationAssessment::DePublishedConflict => (
                    ToolMiddlewarePublicationPlanAction::Conflict,
                    Some(
                        "this release is de-published; restore it explicitly before deploying"
                            .to_string(),
                    ),
                ),
            };
            entries.push(ToolMiddlewarePublicationPlanEntry {
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
        candidates: &mut [ToolMiddlewareReleaseRecord],
    ) -> Result<bool, ToolMiddlewareReleaseError> {
        let mut changed = false;
        for candidate in candidates {
            let name = ToolMiddlewareName::try_from(candidate.tool_middleware_name.clone())
                .map_err(anyhow::Error::msg)?;
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
                ToolMiddlewarePublicationAssessment::NoChange(existing_id) => {
                    candidate.tool_middleware_release_id = existing_id.0;
                }
                ToolMiddlewarePublicationAssessment::Publish => changed = true,
                ToolMiddlewarePublicationAssessment::ImmutableConflict
                | ToolMiddlewarePublicationAssessment::StrictFollowingGrantConflict => {
                    return Err(ToolMiddlewareReleaseError::ImmutableReleaseConflict);
                }
                ToolMiddlewarePublicationAssessment::DePublishedConflict => {
                    return Err(
                        ToolMiddlewareReleaseError::DePublishedReleaseRequiresExplicitRestore,
                    );
                }
            }
        }
        Ok(changed)
    }

    fn validate_publication_selection(
        environment: &Environment,
        name: &ToolMiddlewareName,
        seen: &mut BTreeSet<ToolMiddlewareName>,
        auth: &AuthCtx,
    ) -> Result<(), ToolMiddlewareReleaseError> {
        if !seen.insert(name.clone()) {
            return Err(ToolMiddlewareReleaseError::DuplicatePublication(
                name.clone(),
            ));
        }
        authorize_account_tool_middleware_release_permission(
            auth,
            &environment.owner_account_email,
            AccountToolMiddlewareReleaseVerb::Publish,
            name.clone(),
        )?;
        Ok(())
    }

    async fn assess_publication(
        &self,
        owner_account_id: AccountId,
        name: &ToolMiddlewareName,
        definition: &ToolMiddleware,
        metadata_version: &str,
        metadata_digest: diff::Hash,
        immutable: bool,
    ) -> Result<ToolMiddlewarePublicationAssessment, ToolMiddlewareReleaseError> {
        let Some(existing) = self
            .tool_middleware_release_repo
            .get_by_coordinates(owner_account_id.0, name.as_str(), &definition.version)
            .await?
        else {
            return Ok(publication_decision(None, immutable, false));
        };
        let lifecycle = match existing.release.lifecycle {
            TOOL_RELEASE_LIFECYCLE_PUBLISHED => ReleaseLifecycle::Published,
            TOOL_RELEASE_LIFECYCLE_SUPERSEDED => ReleaseLifecycle::Superseded,
            TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED => ReleaseLifecycle::DePublished,
            lifecycle => {
                return Err(anyhow::anyhow!(
                    "unknown tool_middleware release lifecycle {lifecycle}"
                )
                .into());
            }
        };
        let content_matches = existing.release.tool_definition.value() == definition
            && existing.release.metadata_version == metadata_version
            && diff::Hash::from(existing.release.metadata_digest) == metadata_digest;
        let strict_following_grant_exists = lifecycle == ReleaseLifecycle::Published
            && !content_matches
            && !immutable
            && self
                .tool_middleware_release_repo
                .strict_following_grant_exists(existing.release.tool_middleware_release_id)
                .await?;
        Ok(publication_decision(
            Some((
                ToolMiddlewareReleaseId(existing.release.tool_middleware_release_id),
                lifecycle,
                content_matches,
            )),
            immutable,
            strict_following_grant_exists,
        ))
    }

    pub async fn get(
        &self,
        release_id: ToolMiddlewareReleaseId,
        auth: &AuthCtx,
    ) -> Result<ToolMiddlewareRelease, ToolMiddlewareReleaseError> {
        let record = self.get_record(release_id).await?;
        authorize_account_tool_middleware_release_permission(
            auth,
            &AccountEmail::new(&record.owner_account_email),
            AccountToolMiddlewareReleaseVerb::View,
            ToolMiddlewareName::try_from(record.release.tool_middleware_name.clone())
                .map_err(anyhow::Error::msg)?,
        )
        .map_err(|_| ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(release_id))?;
        record.release.try_into().map_err(Into::into)
    }

    pub async fn list_in_account(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<ToolMiddlewareRelease>, ToolMiddlewareReleaseError> {
        let account =
            self.account_service
                .get(account_id, auth)
                .await
                .map_err(|err| match err {
                    AccountError::AccountNotFound(id) => {
                        ToolMiddlewareReleaseError::ParentAccountNotFound(id)
                    }
                    other => other.into(),
                })?;

        let mut releases = Vec::new();
        for record in self
            .tool_middleware_release_repo
            .list_by_owner(account_id.0)
            .await?
        {
            let name = ToolMiddlewareName::try_from(record.release.tool_middleware_name.clone())
                .map_err(anyhow::Error::msg)?;
            if authorize_account_tool_middleware_release_permission(
                auth,
                &account.email,
                AccountToolMiddlewareReleaseVerb::View,
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
        release_id: ToolMiddlewareReleaseId,
        auth: &AuthCtx,
    ) -> Result<ToolMiddlewareRelease, ToolMiddlewareReleaseError> {
        let record = self
            .authorize_management(
                release_id,
                AccountToolMiddlewareReleaseVerb::DePublish,
                auth,
            )
            .await?;
        match release_management_decision(
            record.release.origin == TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM,
            release_lifecycle(record.release.lifecycle)?,
            ReleaseLifecycle::Published,
        ) {
            ReleaseManagementDecision::Protected => {
                return Err(ToolMiddlewareReleaseError::ProtectedToolMiddlewareRelease);
            }
            ReleaseManagementDecision::InvalidLifecycle => {
                return Err(ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotPublished);
            }
            ReleaseManagementDecision::Change => {}
        }
        self.tool_middleware_release_repo
            .de_publish(release_id.0, auth.actor_account_id().0)
            .await?
            .ok_or(ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(
                release_id,
            ))?
            .release
            .try_into()
            .map_err(Into::into)
    }

    pub async fn restore(
        &self,
        release_id: ToolMiddlewareReleaseId,
        auth: &AuthCtx,
    ) -> Result<ToolMiddlewareRelease, ToolMiddlewareReleaseError> {
        let record = self
            .authorize_management(release_id, AccountToolMiddlewareReleaseVerb::Restore, auth)
            .await?;
        match release_management_decision(
            record.release.origin == TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM,
            release_lifecycle(record.release.lifecycle)?,
            ReleaseLifecycle::DePublished,
        ) {
            ReleaseManagementDecision::Protected => {
                return Err(ToolMiddlewareReleaseError::ProtectedToolMiddlewareRelease);
            }
            ReleaseManagementDecision::InvalidLifecycle => {
                return Err(ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotDePublished);
            }
            ReleaseManagementDecision::Change => {}
        }
        self.tool_middleware_release_repo
            .restore(release_id.0, auth.actor_account_id().0)
            .await?
            .ok_or(ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(
                release_id,
            ))?
            .release
            .try_into()
            .map_err(Into::into)
    }

    pub(crate) async fn resolve_published_reference(
        &self,
        reference: &ToolMiddlewareReleaseReference,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseError> {
        let (record, allows_superseded) = match reference {
            ToolMiddlewareReleaseReference::ById(reference) => (
                self.tool_middleware_release_repo
                    .get_by_id(reference.release_id.0)
                    .await?,
                true,
            ),
            ToolMiddlewareReleaseReference::ByCoordinates(reference) => {
                let account = self
                    .account_service
                    .get_by_email(reference.account.as_str(), &AuthCtx::System)
                    .await
                    .map_err(|_| {
                        ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound
                    })?;
                (
                    self.tool_middleware_release_repo
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
        let record =
            record.ok_or(ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound)?;

        let release: ToolMiddlewareRelease = record.release.clone().try_into()?;
        if release.lifecycle != ToolMiddlewareReleaseLifecycle::Published
            && !(allows_superseded
                && release.lifecycle == ToolMiddlewareReleaseLifecycle::Superseded)
        {
            return Err(ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound);
        }
        Ok(record)
    }

    pub(crate) async fn resolve_user_grantable_reference(
        &self,
        reference: &ToolMiddlewareReleaseReference,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseError> {
        let record = self.resolve_published_reference(reference).await?;
        let release: ToolMiddlewareRelease = record.release.clone().try_into()?;
        if release.origin != ToolMiddlewareReleaseOrigin::Ordinary {
            return Err(ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound);
        }
        Ok(record)
    }

    pub(crate) async fn resolve_auto_grantable_system_release(
        &self,
        release_id: ToolMiddlewareReleaseId,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseError> {
        let record = self.get_record(release_id).await?;
        let release: ToolMiddlewareRelease = record.release.clone().try_into()?;
        if release.lifecycle != ToolMiddlewareReleaseLifecycle::Published
            || release.origin != ToolMiddlewareReleaseOrigin::ProtectedSystem
        {
            return Err(ToolMiddlewareReleaseError::ReferencedToolMiddlewareReleaseNotFound);
        }
        Ok(record)
    }

    async fn authorize_management(
        &self,
        release_id: ToolMiddlewareReleaseId,
        verb: AccountToolMiddlewareReleaseVerb,
        auth: &AuthCtx,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseError> {
        let record = self.get_record(release_id).await?;
        let name = ToolMiddlewareName::try_from(record.release.tool_middleware_name.clone())
            .map_err(anyhow::Error::msg)?;
        authorize_account_tool_middleware_release_permission(
            auth,
            &AccountEmail::new(&record.owner_account_email),
            AccountToolMiddlewareReleaseVerb::View,
            name.clone(),
        )
        .map_err(|_| ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(release_id))?;
        authorize_account_tool_middleware_release_permission(
            auth,
            &AccountEmail::new(&record.owner_account_email),
            verb,
            name,
        )?;
        Ok(record)
    }

    async fn get_record(
        &self,
        release_id: ToolMiddlewareReleaseId,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseError> {
        self.tool_middleware_release_repo
            .get_by_id(release_id.0)
            .await?
            .ok_or(ToolMiddlewareReleaseError::ToolMiddlewareReleaseNotFound(
                release_id,
            ))
    }
}

fn release_lifecycle(value: i16) -> Result<ReleaseLifecycle, ToolMiddlewareReleaseError> {
    match value {
        TOOL_RELEASE_LIFECYCLE_PUBLISHED => Ok(ReleaseLifecycle::Published),
        TOOL_RELEASE_LIFECYCLE_SUPERSEDED => Ok(ReleaseLifecycle::Superseded),
        TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED => Ok(ReleaseLifecycle::DePublished),
        value => Err(anyhow::anyhow!("unknown tool_middleware release lifecycle {value}").into()),
    }
}

fn authorize_account_tool_middleware_release_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    verb: AccountToolMiddlewareReleaseVerb,
    name: ToolMiddlewareName,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::AccountToolMiddlewareRelease(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: AccountOwnerPattern::Account {
                account: account_email.clone(),
            },
            resource: AccountToolMiddlewareReleaseResourcePattern::Name(name),
        },
    ))
}
