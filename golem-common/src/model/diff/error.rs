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

use std::sync::Arc;

#[derive(Debug, Clone, thiserror::Error)]
pub enum DiffError {
    #[error("JSON serialization failed during '{operation}': {source}")]
    SerdeJson {
        operation: &'static str,
        source: Arc<serde_json::Error>,
    },
    #[error("YAML serialization failed during '{operation}': {source}")]
    SerdeYaml {
        operation: &'static str,
        source: Arc<serde_yaml::Error>,
    },
    #[error("Vector diff input is not sorted for '{side}' side")]
    VecInputNotSorted { side: &'static str },
    #[error("Vector diff invariant violation at '{phase}'")]
    VecStateInvariantViolation { phase: &'static str },
    #[error("Map diff invariant violation at '{phase}'")]
    MapStateInvariantViolation { phase: &'static str },
    #[error("Set diff invariant violation at '{phase}'")]
    SetStateInvariantViolation { phase: &'static str },
    #[error(
        "Typed config entry JSON conversion failed during '{operation}' for '{path}': {reason}"
    )]
    TypedConfigJsonConversion {
        operation: &'static str,
        path: String,
        reason: String,
    },
}

impl DiffError {
    pub fn serde_json(operation: &'static str, source: serde_json::Error) -> Self {
        Self::SerdeJson {
            operation,
            source: Arc::new(source),
        }
    }

    pub fn serde_yaml(operation: &'static str, source: serde_yaml::Error) -> Self {
        Self::SerdeYaml {
            operation,
            source: Arc::new(source),
        }
    }
}
