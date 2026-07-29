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

#[derive(Clone, Default)]
struct UniquenessIndex {
    agent_type_wrapper_name_sources: BTreeMap<String, BTreeSet<ComponentName>>,
    agent_type_name_sources: BTreeMap<AgentTypeName, BTreeSet<ComponentName>>,
    tool_name_sources: BTreeMap<String, BTreeSet<ComponentName>>,
}

impl UniquenessIndex {
    fn remove_component(&mut self, component_name: &ComponentName) {
        remove_component_from_index(&mut self.agent_type_wrapper_name_sources, component_name);
        remove_component_from_index(&mut self.agent_type_name_sources, component_name);
        remove_component_from_index(&mut self.tool_name_sources, component_name);
    }
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
        self.cache.remove(component_name).await;
        self.remove_uniqueness_entries(component_name).await;

        let wasm_path = wasm_path.to_path_buf();
        let enable_wasmtime_fs_cache = self.enable_wasmtime_fs_cache;

        let metadata = self
            .cache
            .get_or_insert_simple(component_name, async || {
                extract(wasm_path, enable_wasmtime_fs_cache).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.update_uniqueness(component_name, &metadata, true)
            .await?;

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

        self.update_uniqueness(component_name, &normalized, true)
            .await?;

        self.cache.remove(component_name).await;

        let metadata = self
            .cache
            .get_or_insert_simple(component_name, async || Ok(normalized))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

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

    async fn remove_uniqueness_entries(&self, component_name: &ComponentName) {
        self.uniqueness
            .write()
            .await
            .remove_component(component_name);
    }

    async fn update_uniqueness(
        &self,
        component_name: &ComponentName,
        metadata: &ExtractedComponentMetadata,
        replace_existing: bool,
    ) -> anyhow::Result<()> {
        let mut index_guard = self.uniqueness.write().await;
        let mut index = index_guard.clone();

        if replace_existing {
            index.remove_component(component_name);
        }

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

        // Only publish index changes after validation succeeds
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

        *index_guard = index;

        Ok(())
    }
}

fn remove_component_from_index<K: Ord>(
    index: &mut BTreeMap<K, BTreeSet<ComponentName>>,
    component_name: &ComponentName,
) {
    index.retain(|_, component_names| {
        component_names.remove(component_name);
        !component_names.is_empty()
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::{AgentMode, Snapshotting};
    use golem_common::schema::{AgentConstructorSchema, InputSchema, SchemaGraph};
    use test_r::test;

    #[test]
    async fn add_cached_component_metadata_replaces_uniqueness_entries() {
        let registry = ComponentMetadataRegistry::new(false);
        let first_component = ComponentName("app:first".to_string());
        let second_component = ComponentName("app:second".to_string());

        registry
            .add_cached_component_metadata(&first_component, metadata_with_agent_type("BarAgent"))
            .await
            .unwrap();
        registry
            .add_cached_component_metadata(&first_component, empty_metadata())
            .await
            .unwrap();
        registry
            .add_cached_component_metadata(&second_component, metadata_with_agent_type("BarAgent"))
            .await
            .unwrap();
    }

    #[test]
    async fn failed_add_cached_component_metadata_replace_keeps_previous_cached_metadata() {
        let registry = ComponentMetadataRegistry::new(false);
        let first_component = ComponentName("app:first".to_string());
        let second_component = ComponentName("app:second".to_string());

        registry
            .add_cached_component_metadata(&first_component, metadata_with_agent_type("FooAgent"))
            .await
            .unwrap();
        registry
            .add_cached_component_metadata(&second_component, metadata_with_agent_type("BarAgent"))
            .await
            .unwrap();

        assert!(
            registry
                .add_cached_component_metadata(
                    &first_component,
                    metadata_with_agent_type("BarAgent")
                )
                .await
                .is_err()
        );

        assert!(
            registry
                .add_cached_component_metadata(
                    &ComponentName("app:third".to_string()),
                    metadata_with_agent_type("FooAgent"),
                )
                .await
                .is_err()
        );
    }

    #[test]
    async fn get_or_extract_component_metadata_does_not_hide_required_extraction_failures_with_stale_cache()
     {
        let registry = ComponentMetadataRegistry::new(false);
        let component = ComponentName("app:component".to_string());

        registry
            .add_cached_component_metadata(&component, metadata_with_agent_type("OldAgent"))
            .await
            .unwrap();

        let result = registry
            .get_or_extract_component_metadata(
                &component,
                std::path::Path::new("/definitely/missing/current-component.wasm"),
            )
            .await;

        assert!(
            result.is_err(),
            "stale cached metadata was returned even though the current wasm could not be extracted: {result:?}"
        );
    }

    #[test]
    async fn failed_get_or_extract_component_metadata_replacement_drops_stale_uniqueness_entries() {
        let registry = ComponentMetadataRegistry::new(false);
        let stale_component = ComponentName("app:stale".to_string());
        let replacement_component = ComponentName("app:replacement".to_string());

        registry
            .add_cached_component_metadata(&stale_component, metadata_with_agent_type("OldAgent"))
            .await
            .unwrap();

        let result = registry
            .get_or_extract_component_metadata(
                &stale_component,
                std::path::Path::new("/definitely/missing/current-component.wasm"),
            )
            .await;
        assert!(result.is_err());

        registry
            .add_cached_component_metadata(
                &replacement_component,
                metadata_with_agent_type("OldAgent"),
            )
            .await
            .unwrap();
    }

    fn empty_metadata() -> ExtractedComponentMetadata {
        ExtractedComponentMetadata {
            agent_types: vec![],
            tools: vec![],
        }
    }

    fn metadata_with_agent_type(type_name: &str) -> ExtractedComponentMetadata {
        ExtractedComponentMetadata {
            agent_types: vec![AgentTypeSchema {
                type_name: AgentTypeName(type_name.to_string()),
                description: String::new(),
                source_language: String::new(),
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: InputSchema::parameters(vec![]),
                },
                methods: vec![],
                dependencies: vec![],
                mode: AgentMode::Ephemeral,
                http_mount: None,
                snapshotting: Snapshotting::Disabled(Empty {}),
                config: vec![],
            }],
            tools: vec![],
        }
    }
}
