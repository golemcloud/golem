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

//! Error and warning types for component introspection.

use std::fmt::{Display, Formatter};

pub type AnalysisResult<A> = Result<A, AnalysisFailure>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisFailure {
    pub reason: String,
}

impl AnalysisFailure {
    pub fn failed(message: impl Into<String>) -> AnalysisFailure {
        AnalysisFailure {
            reason: message.into(),
        }
    }

    pub fn fail_on_missing<T>(value: Option<T>, description: impl AsRef<str>) -> AnalysisResult<T> {
        match value {
            Some(value) => Ok(value),
            None => Err(AnalysisFailure::failed(format!(
                "Missing {}",
                description.as_ref()
            ))),
        }
    }
}

impl Display for AnalysisFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceCouldNotBeAnalyzedWarning {
    pub name: String,
    pub failure: AnalysisFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisWarning {
    InterfaceCouldNotBeAnalyzed(InterfaceCouldNotBeAnalyzedWarning),
}

impl Display for AnalysisWarning {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisWarning::InterfaceCouldNotBeAnalyzed(warning) => {
                write!(
                    f,
                    "Interface could not be analyzed: {} {}",
                    warning.name, warning.failure.reason
                )
            }
        }
    }
}
