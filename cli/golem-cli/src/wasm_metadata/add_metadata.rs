use crate::wasm_metadata::{rewrite_wasm, Producers};
use anyhow::Result;
use std::fmt::Debug;

/// Add metadata (module name, producers) to a WebAssembly file.
///
/// Supports both core WebAssembly modules and components. In components,
/// metadata will be added to the outermost component.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct AddMetadata {
    /// Add a module or component name to the names section
    pub name: AddMetadataField<String>,

    /// Add a programming language to the producers section
    pub language: Vec<(String, String)>,

    /// Add a tool and its version to the producers section
    pub processed_by: Vec<(String, String)>,

    /// Add an SDK and its version to the producers section
    pub sdk: Vec<(String, String)>,

    /// Contact details of the people or organization responsible,
    /// encoded as a freeform string.
    pub authors: AddMetadataField<crate::wasm_metadata::Authors>,

    /// A human-readable description of the binary
    pub description: AddMetadataField<crate::wasm_metadata::Description>,

    /// License(s) under which contained software is distributed as an SPDX License Expression.
    pub licenses: AddMetadataField<crate::wasm_metadata::Licenses>,

    /// URL to get source code for building the image
    pub source: AddMetadataField<crate::wasm_metadata::Source>,

    /// URL to find more information on the binary
    pub homepage: AddMetadataField<crate::wasm_metadata::Homepage>,

    /// Source control revision identifier for the packaged software.
    pub revision: AddMetadataField<crate::wasm_metadata::Revision>,

    /// Version of the packaged software
    pub version: AddMetadataField<crate::wasm_metadata::Version>,
}

impl AddMetadata {
    /// Process a WebAssembly binary. Supports both core WebAssembly modules, and WebAssembly
    /// components. The module and component will have, at very least, an empty name and producers
    /// section created.
    pub fn to_wasm(&self, input: &[u8]) -> Result<Vec<u8>> {
        let add_producers = Producers::from_meta(self);
        rewrite_wasm(self, &add_producers, input)
    }
}

#[derive(Debug, Clone)]
pub enum AddMetadataField<T: Debug + Clone> {
    Keep,
    Clear,
    Set(T),
}

impl<T: Debug + Clone> AddMetadataField<T> {
    pub fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}

impl<T: Debug + Clone> Default for AddMetadataField<T> {
    fn default() -> Self {
        Self::Keep
    }
}
