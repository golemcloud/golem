// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::model::component_metadata::{
    ComponentMetadata, ComponentProcessingError, LinearMemory, RawComponentMetadata,
};

pub fn process_component(data: &[u8]) -> Result<ComponentMetadata, ComponentProcessingError> {
    let raw_component_metadata = RawComponentMetadata::analyse_component(data)?;

    let producers = raw_component_metadata
        .producers
        .into_iter()
        .map(|producers| producers.into())
        .collect::<Vec<_>>();

    let exports = raw_component_metadata
        .exports
        .into_iter()
        .collect::<Vec<_>>();

    let memories = raw_component_metadata
        .memories
        .into_iter()
        .map(LinearMemory::from)
        .collect();

    Ok(ComponentMetadata {
        exports,
        producers,
        memories,
    })
}
