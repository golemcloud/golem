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
use golem_client::model::{McpImportAuthorization, McpImportOAuthStatus};
use serde_derive::Serialize;
use std::fmt::{Debug, Formatter};

#[derive(Clone, Debug)]
pub struct McpDeploymentDeployProperties {
    pub agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
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
        fields
            .field("Authorization URL", &self.0.authorization_url)
            .field(
                "Complete with",
                &format!(
                    "golem api mcp-import complete {} --revision {} '<callback-url>'",
                    self.0.import_index, self.0.deployment_revision
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
                    .field("Deployment revision", &self.0.deployment_revision)
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
