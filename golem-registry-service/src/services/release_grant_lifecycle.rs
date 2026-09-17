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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReleaseLifecycle {
    Published,
    Superseded,
    DePublished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationDecision<I> {
    NoChange(I),
    Publish,
    ImmutableConflict,
    StrictFollowingGrantConflict,
    DePublishedConflict,
}

pub(crate) fn publication_decision<I>(
    existing: Option<(I, ReleaseLifecycle, bool)>,
    candidate_immutable: bool,
    strict_following_grant_exists: bool,
) -> PublicationDecision<I> {
    let Some((id, lifecycle, content_matches)) = existing else {
        return PublicationDecision::Publish;
    };
    match lifecycle {
        ReleaseLifecycle::DePublished => PublicationDecision::DePublishedConflict,
        ReleaseLifecycle::Superseded => PublicationDecision::Publish,
        ReleaseLifecycle::Published if content_matches => PublicationDecision::NoChange(id),
        ReleaseLifecycle::Published if candidate_immutable => {
            PublicationDecision::ImmutableConflict
        }
        ReleaseLifecycle::Published if strict_following_grant_exists => {
            PublicationDecision::StrictFollowingGrantConflict
        }
        ReleaseLifecycle::Published => PublicationDecision::Publish,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReleaseManagementDecision {
    Change,
    Protected,
    InvalidLifecycle,
}

pub(crate) fn release_management_decision(
    protected: bool,
    current: ReleaseLifecycle,
    required: ReleaseLifecycle,
) -> ReleaseManagementDecision {
    if protected {
        ReleaseManagementDecision::Protected
    } else if current != required {
        ReleaseManagementDecision::InvalidLifecycle
    } else {
        ReleaseManagementDecision::Change
    }
}

pub(crate) fn release_is_admissible(version_check: bool, immutable: bool) -> bool {
    !version_check || immutable
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExistingGrantDecision {
    ReturnExisting,
    SetManagement,
    Restore,
}

pub(crate) fn existing_grant_decision(
    deleted: bool,
    protected: bool,
    existing_automatic: bool,
    existing_follows_coordinates: bool,
    requested_automatic: bool,
    requested_follows_coordinates: bool,
) -> ExistingGrantDecision {
    if deleted {
        ExistingGrantDecision::Restore
    } else if protected
        || (requested_automatic && !existing_automatic)
        || (existing_automatic == requested_automatic
            && existing_follows_coordinates == requested_follows_coordinates)
    {
        ExistingGrantDecision::ReturnExisting
    } else {
        ExistingGrantDecision::SetManagement
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GrantMutationDecision {
    Allow,
    Protected,
    AdministratorManaged,
    NotDeleted,
    NotReconciliable,
}

pub(crate) fn reconciliation_deletion_decision(
    belongs_to_environment: bool,
    automatic: bool,
    protected: bool,
) -> GrantMutationDecision {
    if !belongs_to_environment || !automatic {
        GrantMutationDecision::NotReconciliable
    } else if protected {
        GrantMutationDecision::Protected
    } else {
        GrantMutationDecision::Allow
    }
}

pub(crate) fn delete_grant_decision(
    protected: bool,
    existing_automatic: bool,
    requested_automatic: bool,
) -> GrantMutationDecision {
    if protected {
        GrantMutationDecision::Protected
    } else if requested_automatic && !existing_automatic {
        GrantMutationDecision::AdministratorManaged
    } else {
        GrantMutationDecision::Allow
    }
}

pub(crate) fn restore_grant_decision(protected: bool, deleted: bool) -> GrantMutationDecision {
    if protected {
        GrantMutationDecision::Protected
    } else if !deleted {
        GrantMutationDecision::NotDeleted
    } else {
        GrantMutationDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn publication_policy_covers_lifecycle_and_strict_following_guards() {
        assert_eq!(
            publication_decision::<u8>(None, true, false),
            PublicationDecision::Publish
        );
        assert_eq!(
            publication_decision(Some((1, ReleaseLifecycle::Published, true)), true, true),
            PublicationDecision::NoChange(1)
        );
        assert_eq!(
            publication_decision(Some((1, ReleaseLifecycle::Published, false)), true, false),
            PublicationDecision::ImmutableConflict
        );
        assert_eq!(
            publication_decision(Some((1, ReleaseLifecycle::Published, false)), false, true),
            PublicationDecision::StrictFollowingGrantConflict
        );
        assert_eq!(
            publication_decision(Some((1, ReleaseLifecycle::Published, false)), false, false),
            PublicationDecision::Publish
        );
        assert_eq!(
            publication_decision(Some((1, ReleaseLifecycle::Superseded, true)), true, true),
            PublicationDecision::Publish
        );
        assert_eq!(
            publication_decision(Some((1, ReleaseLifecycle::DePublished, true)), false, false),
            PublicationDecision::DePublishedConflict
        );
    }

    #[test]
    fn grant_policy_preserves_manual_and_protected_management() {
        assert!(!release_is_admissible(true, false));
        assert!(release_is_admissible(false, false));
        assert_eq!(
            existing_grant_decision(false, true, true, false, false, true),
            ExistingGrantDecision::ReturnExisting
        );
        assert_eq!(
            existing_grant_decision(false, false, false, false, true, false),
            ExistingGrantDecision::ReturnExisting
        );
        assert_eq!(
            existing_grant_decision(false, false, true, false, false, true),
            ExistingGrantDecision::SetManagement
        );
        assert_eq!(
            existing_grant_decision(true, false, false, false, true, true),
            ExistingGrantDecision::Restore
        );
        assert_eq!(
            delete_grant_decision(true, true, false),
            GrantMutationDecision::Protected
        );
        assert_eq!(
            delete_grant_decision(false, false, true),
            GrantMutationDecision::AdministratorManaged
        );
        assert_eq!(
            restore_grant_decision(false, false),
            GrantMutationDecision::NotDeleted
        );
        assert_eq!(
            restore_grant_decision(false, true),
            GrantMutationDecision::Allow
        );
        assert_eq!(
            reconciliation_deletion_decision(true, false, false),
            GrantMutationDecision::NotReconciliable
        );
        assert_eq!(
            reconciliation_deletion_decision(true, true, true),
            GrantMutationDecision::Protected
        );
        assert_eq!(
            reconciliation_deletion_decision(true, true, false),
            GrantMutationDecision::Allow
        );
    }

    #[test]
    fn release_management_policy_requires_mutable_origin_and_expected_state() {
        assert_eq!(
            release_management_decision(
                true,
                ReleaseLifecycle::Published,
                ReleaseLifecycle::Published
            ),
            ReleaseManagementDecision::Protected
        );
        assert_eq!(
            release_management_decision(
                false,
                ReleaseLifecycle::DePublished,
                ReleaseLifecycle::Published
            ),
            ReleaseManagementDecision::InvalidLifecycle
        );
        assert_eq!(
            release_management_decision(
                false,
                ReleaseLifecycle::Published,
                ReleaseLifecycle::Published
            ),
            ReleaseManagementDecision::Change
        );
    }
}
