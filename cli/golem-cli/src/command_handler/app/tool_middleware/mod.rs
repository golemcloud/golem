// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use golem_common::model::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantId, EnvironmentToolMiddlewareGrantWithDetails,
};
use golem_common::model::tool_middleware_release::ToolMiddlewareReleaseReference;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
pub struct ResolvedToolMiddlewareGrants {
    grants: BTreeMap<ToolMiddlewareReleaseReference, EnvironmentToolMiddlewareGrantWithDetails>,
}

impl ResolvedToolMiddlewareGrants {
    pub fn from_current(
        references: &[ToolMiddlewareReleaseReference],
        current: &[EnvironmentToolMiddlewareGrantWithDetails],
    ) -> Self {
        let grants = references
            .iter()
            .filter_map(|reference| {
                current
                    .iter()
                    .find(|grant| {
                        matches_release(grant, reference) && matches_mode(grant, reference)
                    })
                    .or_else(|| {
                        current.iter().find(|grant| {
                            (grant.grant.protected || !grant.grant.automatic)
                                && matches_release(grant, reference)
                        })
                    })
                    .cloned()
                    .map(|grant| (reference.clone(), grant))
            })
            .collect();
        Self { grants }
    }

    pub fn get(
        &self,
        reference: &ToolMiddlewareReleaseReference,
    ) -> Option<&EnvironmentToolMiddlewareGrantWithDetails> {
        self.grants.get(reference)
    }

    pub fn contains_grant(&self, id: EnvironmentToolMiddlewareGrantId) -> bool {
        self.grants.values().any(|grant| grant.grant.id == id)
    }

    pub fn insert(
        &mut self,
        reference: ToolMiddlewareReleaseReference,
        grant: EnvironmentToolMiddlewareGrantWithDetails,
    ) {
        self.grants.insert(reference, grant);
    }
}

#[derive(Debug)]
pub struct ReconciliationPlan {
    pub creations: Vec<ToolMiddlewareReleaseReference>,
    pub updates: Vec<ToolMiddlewareReleaseReference>,
    pub deletions: Vec<EnvironmentToolMiddlewareGrantId>,
    pub retained_manual: Vec<EnvironmentToolMiddlewareGrantId>,
    pub retained_protected: Vec<EnvironmentToolMiddlewareGrantId>,
    pub resolved: ResolvedToolMiddlewareGrants,
}

impl ReconciliationPlan {
    pub fn build(
        desired: &[ToolMiddlewareReleaseReference],
        current: &[EnvironmentToolMiddlewareGrantWithDetails],
    ) -> Self {
        let resolved = ResolvedToolMiddlewareGrants::from_current(desired, current);
        let mut creations = Vec::new();
        let mut updates = Vec::new();
        let mut replaced = BTreeSet::new();
        for reference in desired {
            if resolved.get(reference).is_some() {
                continue;
            }
            if let Some(grant) = current.iter().find(|grant| {
                grant.grant.automatic
                    && !grant.grant.protected
                    && matches_release(grant, reference)
                    && !matches_mode(grant, reference)
            }) {
                updates.push(reference.clone());
                replaced.insert(grant.grant.id);
            } else {
                creations.push(reference.clone());
            }
        }
        let mut deletions = Vec::new();
        let mut retained_manual = Vec::new();
        let mut retained_protected = Vec::new();
        for grant in current {
            if resolved.contains_grant(grant.grant.id) {
                continue;
            }
            if replaced.contains(&grant.grant.id)
                || (grant.grant.automatic && !grant.grant.protected)
            {
                deletions.push(grant.grant.id);
            } else if grant.grant.protected {
                retained_protected.push(grant.grant.id);
            } else {
                retained_manual.push(grant.grant.id);
            }
        }
        Self {
            creations,
            updates,
            deletions,
            retained_manual,
            retained_protected,
            resolved,
        }
    }

    pub fn has_changes(&self) -> bool {
        !self.creations.is_empty() || !self.updates.is_empty() || !self.deletions.is_empty()
    }
    pub fn requires_access_changes(&self) -> bool {
        !self.creations.is_empty() || !self.updates.is_empty()
    }
    pub fn upserts(&self) -> impl Iterator<Item = &ToolMiddlewareReleaseReference> {
        self.creations.iter().chain(&self.updates)
    }
}

fn matches_release(
    grant: &EnvironmentToolMiddlewareGrantWithDetails,
    reference: &ToolMiddlewareReleaseReference,
) -> bool {
    match reference {
        ToolMiddlewareReleaseReference::ById(reference) => grant.release.id == reference.release_id,
        ToolMiddlewareReleaseReference::ByCoordinates(reference) => {
            grant.release_owner.email == reference.account
                && grant.release.name == reference.name
                && grant.release.version == reference.version
        }
    }
}

fn matches_mode(
    grant: &EnvironmentToolMiddlewareGrantWithDetails,
    reference: &ToolMiddlewareReleaseReference,
) -> bool {
    grant.grant.follow_coordinates
        == matches!(reference, ToolMiddlewareReleaseReference::ByCoordinates(_))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantExecutionDecision {
    ContinueReadOnly,
    StopAfterPlan,
    RejectStage,
    ConfirmApply,
}

pub fn grant_execution_decision(
    stage: bool,
    plan: bool,
    requires_access_changes: bool,
    has_changes: bool,
) -> GrantExecutionDecision {
    if stage && requires_access_changes {
        GrantExecutionDecision::RejectStage
    } else if plan && requires_access_changes {
        GrantExecutionDecision::StopAfterPlan
    } else if !stage && !plan && has_changes {
        GrantExecutionDecision::ConfirmApply
    } else {
        GrantExecutionDecision::ContinueReadOnly
    }
}

#[cfg(test)]
mod tests;
