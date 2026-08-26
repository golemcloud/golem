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

use crate::validation::ValidationBuilder;
use golem_common::model::component::ComponentName;
use golem_common::schema::tool::Tool;
use serde::Serialize;
use std::cmp::Ordering;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolValidationPhase {
    StructuralMetadata,
    DeclarationDiscoveryIdentity,
    BindingReferences,
    LocalResolution,
    BindingSemantics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolValidationSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolValidationCode {
    InvalidDeclaration,
    InvalidName,
    ReservedMiddleware,
    DuplicateDeclaration,
    InvalidDefinition,
    DuplicateImplementation,
    MissingDeclaration,
    MissingImplementation,
    UnknownToolReference,
    UnknownAgentReference,
    VersionMismatch,
    EnvironmentAgentVersionMismatch,
    AccountMismatch,
    InvalidParameters,
    InvalidProvision,
    InvalidSecretScope,
    RevealableScopeNarrowed,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEntityPath {
    pub entity_kind: String,
    pub entity_name: String,
    pub field_path: String,
}

impl ToolEntityPath {
    pub fn tool(tool_name: impl ToString, field_path: impl Into<String>) -> Self {
        Self {
            entity_kind: "tool".to_string(),
            entity_name: tool_name.to_string(),
            field_path: field_path.into(),
        }
    }

    pub fn agent(agent_name: impl ToString, field_path: impl Into<String>) -> Self {
        Self {
            entity_kind: "agent".to_string(),
            entity_name: agent_name.to_string(),
            field_path: field_path.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolValidationIssue {
    pub phase: ToolValidationPhase,
    pub severity: ToolValidationSeverity,
    pub path: ToolEntityPath,
    pub source: Option<PathBuf>,
    pub code: ToolValidationCode,
    pub message: String,
}

impl Ord for ToolValidationIssue {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.phase,
            &self.path.entity_kind,
            &self.path.entity_name,
            &self.source,
            &self.path.field_path,
            self.code,
            self.severity,
            &self.message,
        )
            .cmp(&(
                other.phase,
                &other.path.entity_kind,
                &other.path.entity_name,
                &other.source,
                &other.path.field_path,
                other.code,
                other.severity,
                &other.message,
            ))
    }
}

impl PartialOrd for ToolValidationIssue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ToolValidationIssue {
    pub fn error(
        phase: ToolValidationPhase,
        code: ToolValidationCode,
        path: ToolEntityPath,
        source: Option<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            severity: ToolValidationSeverity::Error,
            path,
            source,
            code,
            message: message.into(),
        }
    }

    pub fn warning(
        phase: ToolValidationPhase,
        code: ToolValidationCode,
        path: ToolEntityPath,
        source: Option<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            severity: ToolValidationSeverity::Warning,
            path,
            source,
            code,
            message: message.into(),
        }
    }

    pub fn render(&self) -> String {
        format!("[{:?}/{:?}] {}", self.phase, self.code, self.message)
    }
}

pub fn add_tool_issues(
    validation: &mut ValidationBuilder,
    issues: impl IntoIterator<Item = ToolValidationIssue>,
) {
    let mut issues = issues.into_iter().collect::<Vec<_>>();
    issues.sort();
    for issue in issues {
        let mut context = vec![
            (
                "entity",
                format!("{} {}", issue.path.entity_kind, issue.path.entity_name),
            ),
            ("field", issue.path.field_path.clone()),
        ];
        if let Some(source) = &issue.source {
            context.insert(0, ("source", source.display().to_string()));
        }
        validation.with_context(context, |validation| match issue.severity {
            ToolValidationSeverity::Warning => validation.add_warn(issue.render()),
            ToolValidationSeverity::Error => validation.add_error(issue.render()),
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ToolImplementationSource {
    Component { component_name: ComponentName },
}

impl ToolImplementationSource {
    pub fn local_component_name(&self) -> Option<&ComponentName> {
        match self {
            Self::Component { component_name } => Some(component_name),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveredToolImplementation {
    pub definition: Tool,
    pub implementation: ToolImplementationSource,
    pub diagnostic_source: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::{ToolEntityPath, ToolValidationCode, ToolValidationIssue, ToolValidationPhase};
    use std::path::PathBuf;
    use test_r::test;

    #[test]
    fn validation_issue_order_uses_source_before_field_path() {
        let mut issues = [
            ToolValidationIssue::error(
                ToolValidationPhase::BindingReferences,
                ToolValidationCode::UnknownToolReference,
                ToolEntityPath::tool("grep", "a.field"),
                Some(PathBuf::from("z.yaml")),
                "z",
            ),
            ToolValidationIssue::error(
                ToolValidationPhase::BindingReferences,
                ToolValidationCode::UnknownToolReference,
                ToolEntityPath::tool("grep", "z.field"),
                Some(PathBuf::from("a.yaml")),
                "a",
            ),
        ];

        issues.sort();

        assert_eq!(issues[0].source, Some(PathBuf::from("a.yaml")));
        assert_eq!(issues[1].source, Some(PathBuf::from("z.yaml")));
    }
}
