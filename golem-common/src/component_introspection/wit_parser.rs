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

use super::error::{
    AnalysisFailure, AnalysisResult, AnalysisWarning, InterfaceCouldNotBeAnalyzedWarning,
};
use super::metadata::Producers;
use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;
use wasm_metadata::{Payload, Version};
use wasmparser::KnownCustom;
use wit_parser::decoding::DecodedWasm;
use wit_parser::{Interface, PackageName, WorldItem};

/// A single top-level export of a component, identified by name only.
///
/// The component-metadata path only needs the exported interface/function
/// *names* (to build [`crate::base_model::component_metadata::KnownExports`]);
/// the full type information of the exports is intentionally not analysed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopLevelExport {
    Instance(ExportedInstance),
    Function(String),
}

/// An exported interface (instance) with its function names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedInstance {
    pub name: String,
    pub functions: Vec<String>,
}

pub struct WitAnalysisContext {
    wasm: DecodedWasm,
    metadata_name: Option<String>,
    metadata_version: Option<Version>,
    warnings: Rc<RefCell<Vec<AnalysisWarning>>>,
    linear_memories: Vec<wasmparser::MemoryType>,
    producers: Vec<Producers>,
}

impl WitAnalysisContext {
    pub fn new(component_bytes: &[u8]) -> AnalysisResult<WitAnalysisContext> {
        let wasm = wit_parser::decoding::decode(component_bytes).map_err(|err| {
            AnalysisFailure::failed(format!("Failed to decode WASM component: {err:#}"))
        })?;
        let payload = Payload::from_binary(component_bytes).map_err(|err| {
            AnalysisFailure::failed(format!("Failed to decode WASM component metadata: {err:#}"))
        })?;
        let metadata = payload.metadata();

        let (linear_memories, producers) = Self::extract_additional_information(component_bytes)?;

        Ok(Self {
            wasm,
            metadata_name: metadata.name.clone(),
            metadata_version: metadata.version.clone(),
            warnings: Rc::new(RefCell::new(Vec::new())),
            linear_memories,
            producers,
        })
    }

    /// Get all top-level exports from the component, identified by name only.
    pub fn get_top_level_exports(&self) -> AnalysisResult<Vec<TopLevelExport>> {
        let package_id = self.wasm.package();
        let resolve = self.wasm.resolve();

        let root_package =
            AnalysisFailure::fail_on_missing(resolve.packages.get(package_id), "root package")?;

        if root_package.worlds.len() > 1 {
            Err(AnalysisFailure::failed(
                "The component's root package must contains a single world",
            ))
        } else if root_package.worlds.is_empty() {
            Err(AnalysisFailure::failed(
                "The component's root package must contain a world",
            ))
        } else {
            let (_world_name, world_id) = root_package.worlds.iter().next().unwrap();
            let world = AnalysisFailure::fail_on_missing(resolve.worlds.get(*world_id), "world")?;

            let mut result = Vec::new();
            for (world_key, world_item) in &world.exports {
                let world_name = resolve.name_world_key(world_key);
                match world_item {
                    WorldItem::Interface { id, .. } => {
                        let interface = AnalysisFailure::fail_on_missing(
                            resolve.interfaces.get(*id),
                            "interface",
                        )?;

                        match self.analyse_interface(interface) {
                            Ok(instance) => result.push(TopLevelExport::Instance(instance)),
                            Err(failure) => {
                                self.warning(AnalysisWarning::InterfaceCouldNotBeAnalyzed(
                                    InterfaceCouldNotBeAnalyzedWarning {
                                        name: world_name,
                                        failure,
                                    },
                                ))
                            }
                        }
                    }
                    WorldItem::Function(function) => {
                        result.push(TopLevelExport::Function(function.name.clone()));
                    }
                    WorldItem::Type { .. } => {}
                }
            }

            Ok(result)
        }
    }

    /// Gets all the linear memories defined in the component
    pub fn linear_memories(&self) -> &[wasmparser::MemoryType] {
        &self.linear_memories
    }

    pub fn producers(&self) -> &[Producers] {
        &self.producers
    }

    /// Gets the component's root package name
    ///
    /// Note that the root package name (from the original WIT) gets lost when encoding the component,
    /// so here we use the metadata custom section's name and version fields in a Golem specific way
    /// to recover this information. These fields are set by Golem specific build tooling. If they are not
    /// present or not in a valid format, this function will return None.
    pub fn root_package_name(&self) -> Option<PackageName> {
        self.metadata_name.as_ref().and_then(|name| {
            let parts = name.split(':').collect::<Vec<_>>();

            if parts.len() == 2 {
                let namespace = parts[0].to_string();
                let name = parts[1].to_string();
                let version = self
                    .metadata_version
                    .as_ref()
                    .and_then(|version| semver::Version::from_str(&version.to_string()).ok());

                Some(PackageName {
                    namespace,
                    name,
                    version,
                })
            } else {
                None
            }
        })
    }

    /// Gets a binary WIT representation of the component's interface
    pub fn serialized_interface_only(&self) -> AnalysisResult<Vec<u8>> {
        let decoded_package = self.wasm.package();
        let bytes = wit_component::encode(self.wasm.resolve(), decoded_package).map_err(|err| {
            AnalysisFailure::failed(format!(
                "Failed to encode WASM component interface: {err:#}"
            ))
        })?;
        Ok(bytes)
    }

    pub fn warnings(&self) -> Vec<AnalysisWarning> {
        self.warnings.borrow().clone()
    }

    fn warning(&self, warning: AnalysisWarning) {
        self.warnings.borrow_mut().push(warning);
    }

    fn analyse_interface(&self, interface: &Interface) -> AnalysisResult<ExportedInstance> {
        let functions = interface
            .functions
            .iter()
            .map(|(_, function)| function.name.clone())
            .collect();
        let interface_name =
            AnalysisFailure::fail_on_missing(interface.name.clone(), "interface name")?;
        let package_id = AnalysisFailure::fail_on_missing(interface.package, "interface package")?;
        let package = AnalysisFailure::fail_on_missing(
            self.wasm.resolve().packages.get(package_id),
            "interface package",
        )?;

        Ok(ExportedInstance {
            name: package.name.interface_id(&interface_name),
            functions,
        })
    }

    fn extract_additional_information(
        component_bytes: &[u8],
    ) -> AnalysisResult<(Vec<wasmparser::MemoryType>, Vec<Producers>)> {
        let mut memories = Vec::new();
        let mut producers = Vec::new();

        for payload in wasmparser::Parser::new(0).parse_all(component_bytes) {
            let payload = payload.map_err(|err| AnalysisFailure::failed(err.to_string()))?;
            match payload {
                wasmparser::Payload::MemorySection(reader) => {
                    for memory_type in reader {
                        memories.push(
                            memory_type.map_err(|err| AnalysisFailure::failed(err.to_string()))?,
                        );
                    }
                }
                wasmparser::Payload::CustomSection(reader) => {
                    if let KnownCustom::Producers(_) = reader.as_known() {
                        let parsed_producers = wasm_metadata::Producers::from_bytes(
                            reader.data(),
                            reader.data_offset(),
                        )
                        .map_err(|err| AnalysisFailure::failed(err.to_string()))?;
                        producers.push(parsed_producers.into());
                    }
                }
                _ => {}
            }
        }

        Ok((memories, producers))
    }
}
