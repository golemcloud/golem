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

use super::deployment::DeploymentRevision;
use super::diff::Hash;
use super::environment::EnvironmentId;
use super::security_scheme::SecuritySchemeName;
use super::validate_lower_kebab_case_identifier;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use url::Url;

pub const MCP_IMPORT_BRIDGE_HOST_TOOL_ID: &str = "mcp-import";
pub const MCP_IMPORT_BRIDGE_IMPLEMENTATION_VERSION: &str = "1";
pub const PROTOCOL_VERSION: &str = "2026-07-28";
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[PROTOCOL_VERSION];

pub fn mcp_import_bridge_source() -> super::tool::ToolSource {
    super::tool::ToolSource::Host {
        host_tool_id: MCP_IMPORT_BRIDGE_HOST_TOOL_ID
            .to_string()
            .try_into()
            .expect("valid bridge identifier"),
        implementation_version: MCP_IMPORT_BRIDGE_IMPLEMENTATION_VERSION.to_string(),
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpImportDeployment {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<McpImportAuthInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<SecuritySchemeName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpImportAuthInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<McpImportBasicAuth>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpImportBasicAuth {
    pub user: String,
    pub password: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct McpImport {
    pub url: String,
    pub auth: Option<McpImportAuth>,
    pub security_scheme: Option<SecuritySchemeName>,
    pub prefix: Option<String>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct McpImportAuth {
    pub kind: McpInlineCredentialKind,
    pub credential_digest: Hash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub enum McpInlineCredentialKind {
    Bearer,
    Basic,
}

#[derive(Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum McpImportCredential {
    Bearer { token: String },
    Basic { user: String, password: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct McpImportSource {
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub import_index: u32,
    pub upstream_tool_name: String,
}

impl Debug for McpImportDeployment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpImportDeployment { auth: [REDACTED], .. }")
    }
}
impl Debug for McpImportAuthInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpImportAuthInput([REDACTED])")
    }
}
impl Debug for McpImportBasicAuth {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpImportBasicAuth([REDACTED])")
    }
}
impl Debug for McpImportCredential {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpImportCredential([REDACTED])")
    }
}

fn has_header_controls(value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_control())
}

fn digest_credential(environment_id: EnvironmentId, fields: &[&str]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"golem:mcp-import:inline-credential:v1");
    hasher.update(environment_id.0.as_bytes());
    for field in fields {
        hasher.update(&(field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    Hash::new(hasher.finalize())
}

impl McpImportDeployment {
    pub fn into_parts(
        self,
        environment_id: EnvironmentId,
    ) -> Result<(McpImport, Option<McpImportCredential>), String> {
        if self.auth.is_some() && self.security_scheme.is_some() {
            return Err("auth and securityScheme are mutually exclusive".into());
        }
        if self.include.is_some() && self.exclude.is_some() {
            return Err("include and exclude are mutually exclusive".into());
        }
        if has_header_controls(&self.url) {
            return Err("MCP import URL must contain no ASCII control characters".into());
        }
        let parsed = Url::parse(&self.url).map_err(|_| "MCP import URL is invalid".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "MCP import URL must be HTTP(S), have a host, and contain no userinfo or fragment"
                    .into(),
            );
        }
        if let Some(prefix) = &self.prefix {
            validate_lower_kebab_case_identifier("MCP import prefix", prefix)?;
        }
        if let Some(scheme) = &self.security_scheme {
            SecuritySchemeName::try_from(scheme.0.clone())?;
        }
        if let Some(version) = &self.version
            && !SUPPORTED_PROTOCOL_VERSIONS.contains(&version.as_str())
        {
            return Err("unsupported MCP protocol version".into());
        }

        let (auth, credential) = match self.auth {
            None => (None, None),
            Some(input) => match (input.bearer, input.basic) {
                (Some(token), None) => {
                    if has_header_controls(&token) {
                        return Err(
                            "MCP bearer credential contains header control characters".into()
                        );
                    }
                    (
                        Some(McpImportAuth {
                            kind: McpInlineCredentialKind::Bearer,
                            credential_digest: digest_credential(environment_id, &[&token]),
                        }),
                        Some(McpImportCredential::Bearer { token }),
                    )
                }
                (None, Some(basic)) => {
                    if basic.user.contains(':')
                        || has_header_controls(&basic.user)
                        || has_header_controls(&basic.password)
                    {
                        return Err("MCP basic credential is invalid".into());
                    }
                    let digest = digest_credential(environment_id, &[&basic.user, &basic.password]);
                    (
                        Some(McpImportAuth {
                            kind: McpInlineCredentialKind::Basic,
                            credential_digest: digest,
                        }),
                        Some(McpImportCredential::Basic {
                            user: basic.user,
                            password: basic.password,
                        }),
                    )
                }
                _ => return Err("auth must specify exactly one of bearer or basic".into()),
            },
        };
        Ok((
            McpImport {
                url: self.url,
                auth,
                security_scheme: self.security_scheme,
                prefix: self.prefix,
                include: self.include,
                exclude: self.exclude,
                version: self.version,
            },
            credential,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn deployment() -> McpImportDeployment {
        McpImportDeployment {
            url: "http://internal.example/mcp".to_string(),
            auth: Some(McpImportAuthInput {
                bearer: Some("very-secret-token".to_string()),
                basic: None,
            }),
            security_scheme: None,
            prefix: Some("upstream-tools".to_string()),
            include: Some(vec!["read-*".to_string()]),
            exclude: None,
            version: Some(PROTOCOL_VERSION.to_string()),
        }
    }

    #[test]
    fn splits_credentials_and_keeps_descriptor_secret_free() {
        let (descriptor, credential) = deployment().into_parts(EnvironmentId::new()).unwrap();
        assert!(matches!(
            credential,
            Some(McpImportCredential::Bearer { .. })
        ));
        let json = serde_json::to_string(&descriptor).unwrap();
        assert!(!json.contains("very-secret-token"));
        assert!(!format!("{:?}", deployment()).contains("very-secret-token"));
        assert!(!format!("{credential:?}").contains("very-secret-token"));
    }

    #[test]
    fn credential_digest_is_scoped_and_length_prefixed() {
        let environment = EnvironmentId::new();
        let basic = |user: &str, password: &str| McpImportDeployment {
            auth: Some(McpImportAuthInput {
                bearer: None,
                basic: Some(McpImportBasicAuth {
                    user: user.into(),
                    password: password.into(),
                }),
            }),
            ..deployment()
        };
        let digest = |value: McpImportDeployment, environment| {
            value
                .into_parts(environment)
                .unwrap()
                .0
                .auth
                .unwrap()
                .credential_digest
        };
        assert_ne!(
            digest(basic("a", "bc"), environment),
            digest(basic("ab", "c"), environment)
        );
        assert_ne!(
            digest(basic("a", "bc"), environment),
            digest(basic("a", "bc"), EnvironmentId::new())
        );
        let mut rotated = deployment();
        rotated.auth.as_mut().unwrap().bearer = Some("another-token".into());
        assert_ne!(
            digest(deployment(), environment),
            digest(rotated, environment)
        );
    }

    #[test]
    fn rejects_invalid_declarations_without_echoing_secrets() {
        let invalid = [
            (
                "userinfo",
                McpImportDeployment {
                    url: "https://user:password@example.com/mcp".into(),
                    ..deployment()
                },
            ),
            (
                "fragment",
                McpImportDeployment {
                    url: "https://example.com/mcp#fragment".into(),
                    ..deployment()
                },
            ),
            (
                "prefix",
                McpImportDeployment {
                    prefix: Some("Bad_Prefix".into()),
                    ..deployment()
                },
            ),
            (
                "filters",
                McpImportDeployment {
                    exclude: Some(vec![]),
                    ..deployment()
                },
            ),
            (
                "version",
                McpImportDeployment {
                    version: Some("bad\rvalue".into()),
                    ..deployment()
                },
            ),
            (
                "credential",
                McpImportDeployment {
                    auth: Some(McpImportAuthInput {
                        bearer: Some("secret\nvalue".into()),
                        basic: None,
                    }),
                    ..deployment()
                },
            ),
            (
                "auth choice",
                McpImportDeployment {
                    auth: Some(McpImportAuthInput {
                        bearer: Some("very-secret-token".into()),
                        basic: Some(McpImportBasicAuth {
                            user: "u".into(),
                            password: "p".into(),
                        }),
                    }),
                    ..deployment()
                },
            ),
            (
                "scheme/auth",
                McpImportDeployment {
                    security_scheme: Some(SecuritySchemeName("oauth".into())),
                    ..deployment()
                },
            ),
        ];
        for (case, value) in invalid {
            let error = value.into_parts(EnvironmentId::new()).unwrap_err();
            assert!(
                !error.contains("very-secret-token") && !error.contains("secret\nvalue"),
                "{case}: {error}"
            );
        }
        let colon_user = McpImportDeployment {
            auth: Some(McpImportAuthInput {
                bearer: None,
                basic: Some(McpImportBasicAuth {
                    user: "user:name".into(),
                    password: "hidden".into(),
                }),
            }),
            ..deployment()
        };
        assert!(colon_user.into_parts(EnvironmentId::new()).is_err());
    }

    #[test]
    fn mcp_import_rejects_url_with_embedded_ascii_control_characters() {
        let input = McpImportDeployment {
            url: "https://example.com/m\tcp".into(),
            auth: None,
            security_scheme: None,
            prefix: None,
            include: None,
            exclude: None,
            version: None,
        };

        assert!(input.into_parts(EnvironmentId::new()).is_err());
    }

    #[test]
    fn accepts_supported_and_default_protocol_versions() {
        for version in [Some(PROTOCOL_VERSION.to_string()), None] {
            let input = McpImportDeployment {
                version: version.clone(),
                ..deployment()
            };
            let (import, _) = input.into_parts(EnvironmentId::new()).unwrap();
            assert_eq!(import.version, version);
        }
    }

    #[test]
    fn rejects_invalid_protocol_versions_without_echoing_them() {
        for version in ["2025-06-18", "", "secret\rvalue"] {
            let input = McpImportDeployment {
                version: Some(version.to_string()),
                ..deployment()
            };
            let error = input.into_parts(EnvironmentId::new()).unwrap_err();
            assert_eq!(error, "unsupported MCP protocol version");
            if !version.is_empty() {
                assert!(!error.contains(version));
            }
        }
    }
}
