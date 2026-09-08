// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1; see http://license.golem.cloud/LICENSE

use crate::base_model::account::{AccountId, AccountSummary};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool_middleware_release::{
    ToolMiddlewarePublication, ToolMiddlewarePublicationPlanEntry, ToolMiddlewareReleaseId,
    ToolMiddlewareReleaseMetadata, ToolMiddlewareReleaseReference,
};
use crate::{declare_enums, declare_structs, newtype_uuid};
use chrono::{DateTime, Utc};

newtype_uuid!(EnvironmentToolMiddlewareGrantId);
declare_enums! { pub enum EnvironmentToolMiddlewareGrantLifecycle { Active, Deleted } }
declare_structs! {
    pub struct EnvironmentToolMiddlewareGrant {
        pub id: EnvironmentToolMiddlewareGrantId, pub environment_id: EnvironmentId,
        pub tool_middleware_release_id: ToolMiddlewareReleaseId, pub protected: bool,
        pub automatic: bool, pub follow_coordinates: bool,
        pub lifecycle: EnvironmentToolMiddlewareGrantLifecycle,
        pub created_at: DateTime<Utc>, pub created_by: AccountId,
        pub state_changed_at: DateTime<Utc>, pub state_changed_by: AccountId,
    }
    pub struct EnvironmentToolMiddlewareGrantWithDetails { pub grant: EnvironmentToolMiddlewareGrant, pub release: ToolMiddlewareReleaseMetadata, pub release_owner: AccountSummary }
    pub struct EnvironmentToolMiddlewareGrantCreation { pub release: ToolMiddlewareReleaseReference, pub automatic: bool }
    pub struct EnvironmentToolMiddlewareGrantDeletion { pub automatic: bool }
    pub struct EnvironmentToolMiddlewareGrantReconciliation { pub creations: Vec<EnvironmentToolMiddlewareGrantCreation>, pub deletions: Vec<EnvironmentToolMiddlewareGrantId> }
    pub struct EnvironmentToolMiddlewareValidation { pub grant_reconciliation: EnvironmentToolMiddlewareGrantReconciliation, pub publications: Vec<ToolMiddlewarePublication> }
    pub struct EnvironmentToolMiddlewareValidationResult { pub publication_plan: Vec<ToolMiddlewarePublicationPlanEntry> }
}
