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

use crate::log::LogColorize;
use anyhow::bail;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::component::ComponentName;
use golem_common::schema::AgentTypeSchema;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ComponentMetadataRegistry {
    cache: Cache<ComponentName, (), ExtractedComponentMetadata, Arc<anyhow::Error>>,
    uniqueness: tokio::sync::RwLock<UniquenessIndex>,
    enable_wasmtime_fs_cache: bool,
}

#[derive(Default)]
struct UniquenessIndex {
    agent_type_wrapper_name_sources: BTreeMap<String, BTreeSet<ComponentName>>,
    agent_type_name_sources: BTreeMap<AgentTypeName, BTreeSet<ComponentName>>,
    tool_name_sources: BTreeMap<String, BTreeSet<ComponentName>>,
}

impl ComponentMetadataRegistry {
    pub fn new(enable_wasmtime_fs_cache: bool) -> Self {
        Self {
            cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_metadata",
            ),
            uniqueness: tokio::sync::RwLock::new(UniquenessIndex::default()),
            enable_wasmtime_fs_cache,
        }
    }

    pub async fn get_or_extract_component_metadata(
        &self,
        component_name: &ComponentName,
        wasm_path: &Path,
    ) -> anyhow::Result<ExtractedComponentMetadata> {
        let wasm_path = wasm_path.to_path_buf();
        let enable_wasmtime_fs_cache = self.enable_wasmtime_fs_cache;

        let metadata = self
            .cache
            .get_or_insert_simple(component_name, async || {
                extract(wasm_path, enable_wasmtime_fs_cache).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.validate_uniqueness(component_name, &metadata).await?;

        Ok(metadata)
    }

    pub async fn add_cached_component_metadata(
        &self,
        component_name: &ComponentName,
        metadata: ExtractedComponentMetadata,
    ) -> anyhow::Result<ExtractedComponentMetadata> {
        let normalized = ExtractedComponentMetadata {
            agent_types: AgentTypeSchema::normalized_vec(metadata.agent_types),
            tools: metadata.tools,
        };

        self.cache.remove(component_name).await;

        let metadata = self
            .cache
            .get_or_insert_simple(component_name, async || Ok(normalized))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.validate_uniqueness(component_name, &metadata).await?;

        Ok(metadata)
    }

    pub async fn get_all_extracted_agent_type_names(&self) -> Vec<AgentTypeName> {
        self.uniqueness
            .read()
            .await
            .agent_type_name_sources
            .keys()
            .cloned()
            .collect()
    }

    async fn validate_uniqueness(
        &self,
        component_name: &ComponentName,
        metadata: &ExtractedComponentMetadata,
    ) -> anyhow::Result<()> {
        let mut index = self.uniqueness.write().await;

        // Validate first, before mutating the index
        for agent_type in &metadata.agent_types {
            let wrapper_name = agent_type.type_name.0.clone();
            if let Some(existing) = index.agent_type_wrapper_name_sources.get(&wrapper_name)
                && !existing.contains(component_name)
            {
                let mut all = existing.clone();
                all.insert(component_name.clone());
                bail!(
                    "Wrapper agent type name {} is defined by multiple components: {}",
                    wrapper_name.log_color_highlight(),
                    all.iter()
                        .map(|s| s.as_str().log_color_highlight())
                        .join(", ")
                );
            }

            if let Some(existing) = index.agent_type_name_sources.get(&agent_type.type_name)
                && !existing.contains(component_name)
            {
                let mut all = existing.clone();
                all.insert(component_name.clone());
                bail!(
                    "Agent type name {} is defined by multiple components: {}",
                    agent_type.type_name.as_str().log_color_highlight(),
                    all.iter()
                        .map(|s| s.as_str().log_color_highlight())
                        .join(", ")
                );
            }
        }

        for tool in &metadata.tools {
            let Some(tool_name) = tool.name() else {
                continue;
            };
            if let Some(existing) = index.tool_name_sources.get(tool_name)
                && !existing.contains(component_name)
            {
                let mut all = existing.clone();
                all.insert(component_name.clone());
                bail!(
                    "Tool name {} is defined by multiple components: {}",
                    tool_name.log_color_highlight(),
                    all.iter()
                        .map(|s| s.as_str().log_color_highlight())
                        .join(", ")
                );
            }
        }

        // Only mutate after validation succeeds
        for agent_type in &metadata.agent_types {
            index
                .agent_type_wrapper_name_sources
                .entry(agent_type.type_name.0.clone())
                .or_default()
                .insert(component_name.clone());

            index
                .agent_type_name_sources
                .entry(agent_type.type_name.clone())
                .or_default()
                .insert(component_name.clone());
        }

        for tool in &metadata.tools {
            if let Some(tool_name) = tool.name() {
                index
                    .tool_name_sources
                    .entry(tool_name.to_string())
                    .or_default()
                    .insert(component_name.clone());
            }
        }

        Ok(())
    }
}

async fn extract(
    wasm_path: PathBuf,
    enable_wasmtime_fs_cache: bool,
) -> Result<ExtractedComponentMetadata, Arc<anyhow::Error>> {
    let metadata = crate::model::agent::extraction::extract_component_metadata(
        &wasm_path,
        enable_wasmtime_fs_cache,
    )
    .await
    .map_err(Arc::new)?;
    Ok(ExtractedComponentMetadata {
        agent_types: AgentTypeSchema::normalized_vec(metadata.agent_types),
        tools: metadata.tools,
    })
}
