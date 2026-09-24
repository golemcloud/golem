// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use golem_common::model::mcp_import::McpImportSource;
use golem_mcp_import::tool::{Diagnostic, ProjectedTool};
use serde::{Deserialize, Serialize};

/// A complete dynamic observation. It contains projection and replay inputs,
/// never credentials. An empty tools list is a successful observation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpImportObservation {
    pub source: McpImportSource,
    pub protocol_version: String,
    pub tools: Vec<ProjectedTool>,
    pub diagnostics: Vec<Diagnostic>,
}

impl McpImportObservation {
    /// Internal snapshots contain validated, bounded projections whose encoded
    /// mapping trees can exceed serde's default depth. This is not an upstream
    /// MCP response decoder: upstream limits apply before projection.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        stacker::maybe_grow(2 << 20, 64 << 20, || {
            let mut decoder = serde_json::Deserializer::from_slice(bytes);
            decoder.disable_recursion_limit();
            let observation = Self::deserialize(&mut decoder)?;
            decoder.end()?;
            Ok(observation)
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        stacker::maybe_grow(2 << 20, 64 << 20, || serde_json::to_vec(self))
    }
}
