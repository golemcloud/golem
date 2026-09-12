// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use crate::services::tool_release::ToolReleaseService;
use golem_common::model::account::AccountSummary;
use golem_common::model::deployment::{
    DeploymentPlanAmbientToolEntry, ToolMetadataVersion, ToolVersion,
};
use golem_common::model::tool::{
    HostToolId, ToolBindingInput, ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::model::tool_release::ToolRelease;
use golem_common::model::tool_release::{
    SystemToolAvailability, SystemToolReleaseProvision, ToolReleaseOrigin, tool_source_digest,
};
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

/// Exact registry-visible half of a native implementation registration.
#[derive(Clone)]
pub struct NativeToolDescriptor {
    pub definition: golem_native_tool::NativeToolDefinition,
    pub release_name: ToolName,
    pub provision: ToolProvisionConfig,
    pub environment_binding: ToolBindingInput,
}

/// Native definitions compiled into the production registry binary.
pub fn compiled_native_tools() -> Vec<NativeToolDescriptor> {
    Vec::new()
}

/// Registry-side source of truth for native descriptors. It deliberately has no dependency on
/// executor registration; production starts with an empty inventory and tests/products inject it.
#[derive(Clone, Default)]
pub struct NativeToolCatalog {
    entries: Arc<RwLock<Vec<AmbientToolDeployment>>>,
}

#[derive(Clone)]
pub struct AmbientToolDeployment {
    pub release: ToolRelease,
    pub owner: AccountSummary,
    pub provision: ToolProvisionConfig,
    pub environment_binding: ToolBindingInput,
}

impl NativeToolCatalog {
    pub fn active(&self) -> Vec<AmbientToolDeployment> {
        self.entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn plan_entries(&self) -> Vec<DeploymentPlanAmbientToolEntry> {
        self.active()
            .into_iter()
            .map(|ambient| DeploymentPlanAmbientToolEntry {
                release_id: ambient.release.id,
                name: ambient.release.name,
                version: ToolVersion(ambient.release.version),
                source_digest: tool_source_digest(&ambient.release.source),
                owner_account_id: ambient.release.owner_account_id,
                owner_account_email: ambient.owner.email,
                metadata_version: ToolMetadataVersion(ambient.release.metadata_version),
                metadata_digest: ambient.release.metadata_digest,
                definition: ambient.release.definition,
                provision: ambient.provision,
                environment_binding: ambient.environment_binding,
            })
            .collect()
    }

    pub async fn provision(
        &self,
        descriptors: Vec<NativeToolDescriptor>,
        owner: AccountSummary,
        releases: &ToolReleaseService,
    ) -> anyhow::Result<()> {
        let mut names = BTreeSet::new();
        let mut active = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            descriptor
                .definition
                .validate()
                .map_err(anyhow::Error::msg)?;
            if !names.insert(descriptor.release_name.clone()) {
                anyhow::bail!(
                    "native catalog has multiple active versions for {}",
                    descriptor.release_name
                );
            }
            let release = SystemToolReleaseProvision {
                name: descriptor.release_name,
                version: descriptor.definition.tool.version.clone(),
                source: ToolSource::Host {
                    host_tool_id: HostToolId::try_from(descriptor.definition.id.clone())
                        .map_err(anyhow::Error::msg)?,
                    implementation_version: descriptor.definition.implementation_version,
                },
                definition: descriptor.definition.tool,
                metadata_version: descriptor.definition.metadata_version,
                availability: SystemToolAvailability::Ambient,
            };
            let release = releases.provision_system_release(release).await?;
            if release.origin != ToolReleaseOrigin::ProtectedSystem {
                anyhow::bail!("native catalog release was not provisioned as protected system");
            }
            active.push(AmbientToolDeployment {
                release,
                owner: owner.clone(),
                provision: descriptor.provision,
                environment_binding: descriptor.environment_binding,
            });
        }
        *self
            .entries
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = active;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
