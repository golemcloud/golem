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

use golem_common::model::agent::AgentTypeName;
use std::collections::BTreeMap;

use crate::model::cli_output::StructuredOutput;
use crate::model::masking::Masked;
use crate::model::text_format::{FieldsBuilder, MessageWithFields, format_message_highlight};
use golem_client::model::McpImportTools;
use serde_derive::Serialize;
use std::fmt::{Debug, Formatter};

#[derive(Clone, Debug)]
pub struct McpDeploymentDeployProperties {
    pub agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
}

#[derive(Clone, Debug, Serialize)]
pub struct McpImportToolsView(pub McpImportTools);

impl StructuredOutput for McpImportToolsView {
    const KIND: &'static str = "api.mcp-import.tools";
}

impl Masked for McpImportToolsView {}

impl MessageWithFields for McpImportToolsView {
    fn message(&self) -> String {
        "MCP import tools (before native and earlier-import precedence)".into()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        fields
            .field("Environment ID", &self.0.environment_id)
            .field("Deployment revision", &self.0.deployment_revision)
            .field("Import index", &self.0.import_index)
            .field("Protocol version", &self.0.protocol_version)
            .field(
                "Tools",
                &self
                    .0
                    .tools
                    .iter()
                    .filter_map(|tool| tool.definition.name())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        for diagnostic in &self.0.diagnostics {
            fields.field(
                "Warning",
                &format!("{}: {}", diagnostic.upstream_name, diagnostic.reason),
            );
        }
        fields.build()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportAuthorization {
    pub authorization_url: String,
    pub import_index: u32,
    pub deployment_revision: Option<u64>,
}

impl From<golem_client::model::McpImportAuthorization> for McpImportAuthorization {
    fn from(value: golem_client::model::McpImportAuthorization) -> Self {
        Self {
            authorization_url: value.authorization_url,
            import_index: value.import_index,
            deployment_revision: Some(value.deployment_revision),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportOAuthStatus {
    pub environment_id: uuid::Uuid,
    pub import_index: u32,
    pub deployment_revision: Option<u64>,
    pub security_scheme: String,
    pub status: String,
}

impl From<golem_client::model::McpImportOAuthStatus> for McpImportOAuthStatus {
    fn from(value: golem_client::model::McpImportOAuthStatus) -> Self {
        Self {
            environment_id: value.environment_id,
            import_index: value.import_index,
            deployment_revision: Some(value.deployment_revision),
            security_scheme: value.security_scheme,
            status: value.status,
        }
    }
}

impl McpImportOAuthStatus {
    pub fn declared(
        value: golem_client::model::DeclaredMcpImportOAuthStatus,
        import_index: u32,
    ) -> Self {
        Self {
            environment_id: value.environment_id,
            import_index,
            deployment_revision: None,
            security_scheme: value.security_scheme,
            status: value.status,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct McpImportAuthorizeView(pub McpImportAuthorization);

impl Debug for McpImportAuthorizeView {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpImportAuthorizeView([REDACTED])")
    }
}

impl Masked for McpImportAuthorizeView {}

impl MessageWithFields for McpImportAuthorizeView {
    fn message(&self) -> String {
        "MCP import authorization started".into()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        let target = self
            .0
            .deployment_revision
            .map(|revision| format!("--revision {revision}"))
            .unwrap_or_else(|| "--manifest".into());
        fields
            .field("Authorization URL", &self.0.authorization_url)
            .field(
                "Complete with",
                &format!(
                    "golem api mcp-import complete {} {target} '<callback-url>'",
                    self.0.import_index
                ),
            );
        fields.build()
    }
}

impl StructuredOutput for McpImportAuthorizeView {
    const KIND: &'static str = "api.mcp-import.authorize";
}

#[derive(Debug, Clone, Serialize)]
pub struct McpImportCompleteView(pub McpImportOAuthStatus);

impl StructuredOutput for McpImportCompleteView {
    const KIND: &'static str = "api.mcp-import.complete";
}

#[derive(Debug, Clone, Serialize)]
pub struct McpImportStatusView(pub McpImportOAuthStatus);

impl StructuredOutput for McpImportStatusView {
    const KIND: &'static str = "api.mcp-import.status";
}

#[derive(Debug, Clone, Serialize)]
pub struct McpImportDisconnectView(pub McpImportOAuthStatus);

impl StructuredOutput for McpImportDisconnectView {
    const KIND: &'static str = "api.mcp-import.disconnect";
}

macro_rules! mcp_import_status_view {
    ($name:ident, $message:literal) => {
        impl Masked for $name {}

        impl MessageWithFields for $name {
            fn message(&self) -> String {
                $message.into()
            }

            fn fields(&self) -> Vec<(String, String)> {
                let mut fields = FieldsBuilder::new();
                fields
                    .field("Environment ID", &self.0.environment_id)
                    .field(
                        "Target",
                        &self
                            .0
                            .deployment_revision
                            .map(|revision| format!("Deployment {revision}"))
                            .unwrap_or_else(|| "Manifest declaration".into()),
                    )
                    .field("Import index", &self.0.import_index)
                    .field("Security scheme", &self.0.security_scheme)
                    .fmt_field("Status", &self.0.status, format_message_highlight);
                fields.build()
            }
        }
    };
}

mcp_import_status_view!(McpImportCompleteView, "MCP import authorization completed");
mcp_import_status_view!(McpImportStatusView, "MCP import authorization status");
mcp_import_status_view!(
    McpImportDisconnectView,
    "MCP import authorization disconnected"
);

#[derive(Clone, Debug)]
pub struct McpDeploymentAgentOptions {
    pub security_scheme: Option<String>,
}

impl McpDeploymentAgentOptions {
    pub fn to_diffable(&self) -> golem_common::model::diff::McpDeploymentAgentOptions {
        golem_common::model::diff::McpDeploymentAgentOptions {
            security_scheme: self.security_scheme.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn mcp_authorize_completion_command_preserves_target() {
        for (revision, expected) in [(None, "--manifest"), (Some(42), "--revision 42")] {
            let view = McpImportAuthorizeView(McpImportAuthorization {
                authorization_url: "https://provider.example/authorize?state=private".into(),
                import_index: 3,
                deployment_revision: revision,
            });
            let fields = view.fields();
            assert_eq!(
                fields[1].1,
                format!("golem api mcp-import complete 3 {expected} '<callback-url>'")
            );
            assert!(!format!("{view:?}").contains("private"));
            let value = serde_json::to_value(&view).unwrap();
            assert_eq!(value["deploymentRevision"], serde_json::json!(revision));
            assert_eq!(value["importIndex"], 3);
        }
    }
}
