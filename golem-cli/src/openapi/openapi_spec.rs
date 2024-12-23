use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::Error;
use convert_case::{Case, Casing};
use tailcall_valid::{Valid, Validator};
use crate::wit_config::config::WitConfig;
use crate::openapi::handle_files::handle_types;

#[derive(Default)]
pub struct Unresolved;

#[derive(Default)]
pub struct Resolved;

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiSpec<Resolved = Unresolved> {
    pub openapi: Option<String>,
    pub info: Option<Info>,
    pub paths: Option<HashMap<String, PathItem>>,
    pub components: Option<Components>,
    pub servers: Option<Vec<Server>>,

    #[serde(skip)]
    pub _marker: std::marker::PhantomData<Resolved>,
}

impl<T> OpenApiSpec<T> {
    pub fn resolve_ref(&self, reference: &str) -> Option<&Schema> {
        if let Some(components) = &self.components {
            if reference.starts_with("#/components/schemas/") {
                let name = reference.trim_start_matches("#/components/schemas/");
                let name = name.to_case(Case::Kebab);
                return components.schemas.as_ref()?.get(&name);
            }
        }
        None
    }
}


#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub title: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub url: Option<String>,
    pub description: Option<String>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PathItem {
    pub get: Option<Operation>,
    pub post: Option<Operation>,
    pub put: Option<Operation>,
    pub delete: Option<Operation>,
    pub patch: Option<Operation>,
    pub options: Option<Operation>,
    pub head: Option<Operation>,
    pub trace: Option<Operation>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub tags: Option<Vec<String>>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub operation_id: Option<String>,
    pub parameters: Option<Vec<Parameter>>,
    pub request_body: Option<RequestBody>,
    pub responses: Option<Responses>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    pub name: Option<String>,
    #[serde(rename = "in")] // in is a reserved keyword in Rust
    pub in_: Option<String>,
    pub required: Option<bool>,
    pub schema: Option<Schema>,
    pub description: Option<String>,
    pub example: Option<serde_json::Value>,
    pub examples: Option<HashMap<String, Example>>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    pub content: Option<HashMap<String, MediaType>>,
    pub description: Option<String>,
    pub required: Option<bool>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaType {
    pub schema: Option<Schema>,
    pub example: Option<serde_json::Value>,
    pub examples: Option<HashMap<String, Example>>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Responses {
    #[serde(flatten)]
    pub responses: Option<HashMap<String, Response>>,
    pub default: Option<Response>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub description: Option<String>,
    pub content: Option<HashMap<String, MediaType>>,
    pub headers: Option<HashMap<String, Header>>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub description: Option<String>,
    pub schema: Option<Schema>,
    pub example: Option<serde_json::Value>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    pub schemas: Option<HashMap<String, Schema>>,
    pub responses: Option<HashMap<String, Response>>,
    pub parameters: Option<HashMap<String, Parameter>>,
    pub examples: Option<HashMap<String, Example>>,
    pub request_bodies: Option<HashMap<String, RequestBody>>,
    pub headers: Option<HashMap<String, Header>>,
    pub security_schemes: Option<HashMap<String, SecurityScheme>>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub value: Option<serde_json::Value>,
    pub external_value: Option<String>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    #[serde(rename = "$ref")]
    pub ref_: Option<String>,
    pub title: Option<String>,
    pub multiple_of: Option<f64>,
    pub maximum: Option<f64>,
    pub exclusive_maximum: Option<bool>,
    pub minimum: Option<f64>,
    pub exclusive_minimum: Option<bool>,
    pub max_length: Option<u32>,
    pub min_length: Option<u32>,
    pub pattern: Option<String>,
    pub max_items: Option<u32>,
    pub min_items: Option<u32>,
    pub unique_items: Option<bool>,
    pub max_properties: Option<u32>,
    pub min_properties: Option<u32>,
    pub required: Option<Vec<String>>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub not: Option<Box<Schema>>,
    pub all_of: Option<Vec<Schema>>,
    pub one_of: Option<Vec<Schema>>,
    pub any_of: Option<Vec<Schema>>,
    pub items: Option<Box<Schema>>,
    pub properties: Option<HashMap<String, Schema>>,
    pub additional_properties: Option<Box<Schema>>,
    pub description: Option<String>,
    pub format: Option<String>,
    pub default: Option<serde_json::Value>,
    pub nullable: Option<bool>,
    pub discriminator: Option<Discriminator>,
    pub read_only: Option<bool>,
    pub write_only: Option<bool>,
    pub xml: Option<XML>,
    pub example: Option<serde_json::Value>,
    pub deprecated: Option<bool>,
}


#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Discriminator {
    pub property_name: Option<String>,
    pub mapping: Option<HashMap<String, String>>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityScheme {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub description: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "in")]
    pub in_: Option<String>,
    pub scheme: Option<String>,
    pub bearer_format: Option<String>,
    pub flows: Option<OAuthFlows>,
    pub open_id_connect_url: Option<String>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlows {
    pub implicit: Option<OAuthFlow>,
    pub password: Option<OAuthFlow>,
    pub client_credentials: Option<OAuthFlow>,
    pub authorization_code: Option<OAuthFlow>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlow {
    pub authorization_url: Option<String>,
    pub token_url: Option<String>,
    pub refresh_url: Option<String>,
    pub scopes: Option<HashMap<String, String>>,
}

#[derive(Serialize, Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct XML {
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub prefix: Option<String>,
    pub attribute: Option<bool>,
    pub wrapped: Option<bool>,
}

impl OpenApiSpec<Resolved> {
    pub fn to_config(&self) -> Valid<WitConfig, Error, Error> {
        Valid::succeed(WitConfig::default())
            .and_then(|config| handle_types(config, &self))
    }
}

impl OpenApiSpec<Unresolved> {
    pub fn into_config(self) -> Valid<WitConfig, Error, Error> {
        self.into_resolved()
            .to_config()
    }
    pub fn into_resolved(self) -> OpenApiSpec<Resolved> {
        OpenApiSpec {
            openapi: self.openapi,
            info: self.info,
            paths: self.paths,
            components: self.components.map(|components| Components {
                schemas: components.schemas.map(|schemas| {
                    schemas.into_iter().map(|(k, v)| (k.to_case(Case::Kebab), Self::process_schema(v))).collect()
                }),
                responses: components.responses,
                parameters: components.parameters,
                examples: components.examples,
                request_bodies: components.request_bodies,
                headers: components.headers,
                security_schemes: components.security_schemes,
            }),
            servers: self.servers,
            _marker: Default::default(),
        }
    }

    // TODO: it's a poor and unsafe implementation,
    // maybe add Marker for all structs here and implement into_resolve for all of them.
    fn process_schema(mut schema: Schema) -> Schema {
        schema.ref_ = schema.ref_.map(|t| t.to_case(Case::Kebab));
        schema.type_ = schema.type_.map(|t| t.to_case(Case::Kebab));
        schema.properties = schema.properties.map(|properties| {
            properties.into_iter().map(|(k, v)| (k.to_case(Case::Kebab), Self::process_schema(v))).collect()
        });

        schema
    }
}

#[cfg(test)]
mod test {
    use tailcall_valid::Validator;
    use wit_parser::Resolve;
    use crate::openapi::openapi_spec::OpenApiSpec;

    #[test]
    fn test_from_openapi() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi/fixtures/openapi_todos.yaml");
        let content = std::fs::read_to_string(path).unwrap();

        let y: OpenApiSpec = serde_yaml::from_str(&content).unwrap();
        let mut res = y.into_config().to_result();
        let v = res.as_mut().unwrap();
        v.package = "api:todos@1.0.0".to_string();
        let mut resolve = Resolve::new();

        assert!(resolve.push_str("foox.wit", &v.to_wit()).is_ok());

        insta::assert_snapshot!(v.to_wit());
    }
}
