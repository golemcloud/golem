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

use std::fmt::{Display};

use golem_common::component_metadata::{ComponentProcessingError, RawComponentMetadata};

use golem_service_base::model::{ComponentMetadata, LinearMemory};

pub fn process_component(data: &[u8]) -> Result<ComponentMetadata, ComponentProcessingError> {
    let raw_component_metadata = RawComponentMetadata::from_data(data)?;

    let producers = raw_component_metadata
        .producers
        .into_iter()
        .map(|producers| producers.into())
        .collect::<Vec<_>>();

    let exports = raw_component_metadata
        .exports
        .into_iter()
        .map(|export| export.into())
        .collect::<Vec<_>>();

    let memories = raw_component_metadata
        .memories
        .into_iter()
        .map(|mem| LinearMemory::from(mem))
        .collect();

    Ok(ComponentMetadata {
        exports,
        producers,
        memories,
    })
}



