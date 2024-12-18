use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use golem_wasm_ast::analysis::analysed_type::AnalysedType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAPISpec {
    pub openapi: String,
    pub info: Info,
    pub paths: HashMap<String, PathItem>,
    pub components: Option<Components>,
    pub security: Option<Vec<SecurityRequirement>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathItem {
    pub get: Option<Operation>,
    pub post: Option<Operation>,
    pub put: Option<Operation>,
    pub delete: Option<Operation>,
    pub options: Option<Operation>,  // Added this field
    pub parameters: Option<Vec<Parameter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub parameters: Option<Vec<Parameter>>,
    pub request_body: Option<RequestBody>,
    pub responses: HashMap<String, Response>,
    pub security: Option<Vec<SecurityRequirement>>,
    pub tags: Option<Vec<String>>,
}

impl Default for Operation {
    fn default() -> Self {
        Self {
            summary: None,
            description: None,
            parameters: None,
            request_body: None,
            responses: HashMap::new(),
            security: None,
            tags: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub r#in: ParameterLocation,
    pub description: Option<String>,
    pub required: Option<bool>,
    pub schema: Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParameterLocation {
    Query,
    Header,
    Path,
    Cookie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBody {
    pub description: Option<String>,
    pub content: HashMap<String, MediaType>,
    pub required: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub description: String,
    pub content: Option<HashMap<String, MediaType>>,
    pub headers: Option<HashMap<String, Header>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaType {
    pub schema: Schema,
    pub example: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Components {
    pub schemas: Option<HashMap<String, Schema>>,
    pub responses: Option<HashMap<String, Response>>,
    pub parameters: Option<HashMap<String, Parameter>>,
    pub security_schemes: Option<HashMap<String, SecurityScheme>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Schema {
    #[serde(rename = "object")]
    Object {
        properties: HashMap<String, Schema>,
        required: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        additional_properties: Option<Box<Schema>>,
    },
    #[serde(rename = "array")]
    Array {
        items: Box<Schema>,
    },
    #[serde(rename = "string")]
    String {
        format: Option<String>,
        enum_values: Option<Vec<String>>,
    },
    #[serde(rename = "number")]
    Number {
        format: Option<String>,
    },
    #[serde(rename = "integer")]
    Integer {
        format: Option<String>,
    },
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "$ref")]
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub description: Option<String>,
    pub schema: Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SecurityScheme {
    #[serde(rename = "apiKey")]
    ApiKey {
        name: String,
        r#in: SecuritySchemeLocation,
        description: Option<String>,
    },
    #[serde(rename = "http")]
    Http {
        scheme: String,
        bearer_format: Option<String>,
        description: Option<String>,
    },
    #[serde(rename = "oauth2")]
    OAuth2 {
        flows: OAuthFlows,
        description: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecuritySchemeLocation {
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthFlows {
    pub implicit: Option<OAuthFlow>,
    pub password: Option<OAuthFlow>,
    pub client_credentials: Option<OAuthFlow>,
    pub authorization_code: Option<OAuthFlow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthFlow {
    pub authorization_url: Option<String>,
    pub token_url: Option<String>,
    pub refresh_url: Option<String>,
    pub scopes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityRequirement(HashMap<String, Vec<String>>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BindingType {
    Default {
        input_type: AnalysedType,
        output_type: AnalysedType,
        options: Option<BindingOptions>,
    },
    FileServer {
        root_dir: String,
        options: Option<FileServerOptions>,
    },
    SwaggerUI {
        spec_path: String,
        options: Option<SwaggerUIOptions>,
    },
    Static {
        content_type: String,
        content: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingOptions {
    pub auth: Option<AuthConfig>,
    pub cache: Option<CacheConfig>,
    pub cors: Option<CorsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileServerOptions {
    pub index_files: Option<Vec<String>>,
    pub cache: Option<CacheConfig>,
    pub cors: Option<CorsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUIOptions {
    pub title: Option<String>,
    pub theme: Option<String>,
}
