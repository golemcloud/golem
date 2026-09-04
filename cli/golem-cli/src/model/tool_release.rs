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

use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::{Column, TextOutput, log_table, new_table_full_condensed};
use golem_common::model::environment_tool_grant::{
    EnvironmentToolGrantId, EnvironmentToolGrantWithDetails,
};
use golem_common::model::tool_release::{ToolRelease, ToolReleaseReference};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct ResolvedToolGrants {
    grants: BTreeMap<ToolReleaseReference, EnvironmentToolGrantWithDetails>,
}

impl ResolvedToolGrants {
    pub fn from_current(
        references: &[ToolReleaseReference],
        current: &[EnvironmentToolGrantWithDetails],
    ) -> Self {
        let grants = references
            .iter()
            .filter_map(|reference| {
                current
                    .iter()
                    .find(|grant| {
                        grant_release_matches_reference(grant, reference)
                            && grant_reference_mode_matches(grant, reference)
                    })
                    .or_else(|| {
                        current.iter().find(|grant| {
                            (grant.grant.protected || !grant.grant.automatic)
                                && grant_release_matches_reference(grant, reference)
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
        reference: &ToolReleaseReference,
    ) -> Option<&EnvironmentToolGrantWithDetails> {
        self.grants.get(reference)
    }

    pub fn contains_grant(&self, grant_id: EnvironmentToolGrantId) -> bool {
        self.grants.values().any(|grant| grant.grant.id == grant_id)
    }

    pub fn insert(
        &mut self,
        reference: ToolReleaseReference,
        grant: EnvironmentToolGrantWithDetails,
    ) {
        self.grants.insert(reference, grant);
    }

    pub fn find_automatic_reference_mode_mismatch<'a>(
        current: &'a [EnvironmentToolGrantWithDetails],
        reference: &ToolReleaseReference,
    ) -> Option<&'a EnvironmentToolGrantWithDetails> {
        current.iter().find(|grant| {
            grant.grant.automatic
                && !grant.grant.protected
                && grant_release_matches_reference(grant, reference)
                && !grant_reference_mode_matches(grant, reference)
        })
    }
}

fn grant_release_matches_reference(
    grant: &EnvironmentToolGrantWithDetails,
    reference: &ToolReleaseReference,
) -> bool {
    match reference {
        ToolReleaseReference::ById(reference) => grant.release.id == reference.release_id,
        ToolReleaseReference::ByCoordinates(reference) => {
            grant.release_owner.email == reference.account
                && grant.release.name == reference.name
                && grant.release.version == reference.version
        }
    }
}

fn grant_reference_mode_matches(
    grant: &EnvironmentToolGrantWithDetails,
    reference: &ToolReleaseReference,
) -> bool {
    grant.grant.follow_coordinates == matches!(reference, ToolReleaseReference::ByCoordinates(_))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReleaseView {
    pub release: ToolRelease,
}

impl StructuredOutput for ToolReleaseView {
    const KIND: &'static str = "tool.release";
}

impl TextOutput for ToolReleaseView {
    fn log(&self) {
        log_releases(std::slice::from_ref(&self.release));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReleaseListView {
    pub releases: Vec<ToolRelease>,
}

impl StructuredOutput for ToolReleaseListView {
    const KIND: &'static str = "tool.release.list";
}

impl TextOutput for ToolReleaseListView {
    fn log(&self) {
        log_releases(&self.releases);
    }
}

fn log_releases(releases: &[ToolRelease]) {
    let mut table = new_table_full_condensed(vec![
        Column::new("Release ID"),
        Column::new("Tool"),
        Column::new("Version"),
        Column::new("Lifecycle"),
        Column::new("Immutable"),
    ]);
    for release in releases {
        table.add_row(vec![
            release.id.to_string(),
            release.name.to_string(),
            release.version.clone(),
            release.lifecycle.to_string(),
            release.immutable.to_string(),
        ]);
    }
    log_table(table);
}
