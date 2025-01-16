// Copyright 2024-2025 Golem Cloud
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

use crate::bindings::exports::wasi::filesystem::preopens::Descriptor;
use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::filesystem::preopens::get_directories;
use crate::durability::Durability;
use crate::wrappers::filesystem::types::WrappedDescriptor;
use crate::wrappers::SerializableError;
use std::path::Path;

impl crate::bindings::exports::wasi::filesystem::preopens::Guest for crate::Component {
    fn get_directories() -> Vec<(Descriptor, String)> {
        let durability = Durability::<Vec<String>, SerializableError>::new(
            "cli::preopens",
            "get_directories",
            DurableFunctionType::ReadLocal,
        );

        let current_dirs = get_directories();

        let names = {
            if durability.is_live() {
                let result: Vec<String> = current_dirs
                    .iter()
                    .map(|(_, name)| name.clone())
                    .collect::<Vec<_>>();
                durability.persist_infallible((), result)
            } else {
                durability.replay_infallible()
            }
        };

        // Filtering the current set of pre-opened directories by the serialized names
        let filtered = current_dirs
            .into_iter()
            .filter(|(_, name)| names.contains(name))
            .map(|(descriptor, name)| {
                let descriptor = Descriptor::new(WrappedDescriptor {
                    descriptor,
                    path: Path::new(&name).to_path_buf(),
                });
                (descriptor, name)
            })
            .collect::<Vec<_>>();

        if filtered.len() == names.len() {
            // All directories were found
            filtered
        } else {
            panic!("Not all previously available pre-opened directories were found")
        }
    }
}
