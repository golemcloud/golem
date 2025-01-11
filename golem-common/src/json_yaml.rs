use poem::http::StatusCode;
use poem::{Request, RequestBody};
use poem_openapi::__private::mime;
use poem_openapi::error::ParseRequestPayloadError;
use poem_openapi::payload::{ParsePayload, Payload};
use poem_openapi::registry::{MetaMediaType, MetaRequest, MetaSchemaRef, Registry};
use poem_openapi::types::{ParseFromJSON, ParseFromYAML, Type};
use poem_openapi::{ApiExtractor, ApiExtractorType, ExtractParamOptions};

// A poem type that supports json or yaml with explicit content type
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct JsonOrYaml<T>(pub T);

impl<T: Type> Payload for JsonOrYaml<T> {
    const CONTENT_TYPE: &'static str = "*/*";

    fn check_content_type(content_type: &str) -> bool {
        matches!(content_type.parse::<mime::Mime>(), Ok(content_type) if content_type.type_() == "application"
                && (content_type.subtype() == "yaml" || content_type.subtype() == "json"
                || content_type
                    .suffix()
                    .is_some_and(|v| v == "yaml" || v == "json")))
    }

    fn schema_ref() -> MetaSchemaRef {
        T::schema_ref()
    }

    #[allow(unused_variables)]
    fn register(registry: &mut Registry) {
        T::register(registry);
    }
}

// Explicit ApiExtractor than derived to help with multiple content types
impl<'a, T: ParseFromJSON + ParseFromYAML> ApiExtractor<'a> for JsonOrYaml<T> {
    const TYPES: &'static [ApiExtractorType] = &[ApiExtractorType::RequestObject];
    type ParamType = ();
    type ParamRawType = ();

    fn register(registry: &mut Registry) {
        <Self as Payload>::register(registry);
    }

    fn request_meta() -> Option<MetaRequest> {
        Some(MetaRequest {
            description: None,
            content: vec![
                MetaMediaType {
                    content_type: "application/json",
                    schema: <Self as Payload>::schema_ref(),
                },
                MetaMediaType {
                    content_type: "application/x-yaml",
                    schema: <Self as Payload>::schema_ref(),
                },
            ],
            required: <Self as ParsePayload>::IS_REQUIRED,
        })
    }

    async fn from_request(
        request: &'a Request,
        body: &mut RequestBody,
        _param_opts: ExtractParamOptions<Self::ParamType>,
    ) -> poem::Result<Self> {
        <Self as ParsePayload>::from_request(request, body).await
    }
}

impl<T: ParseFromYAML + ParseFromJSON> ParsePayload for JsonOrYaml<T> {
    const IS_REQUIRED: bool = true;

    async fn from_request(request: &Request, body: &mut RequestBody) -> poem::Result<Self> {
        let content_type = request
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();

        let body = body.take().map_err(|e| {
            poem::Error::from_string(
                format!("Missing request body {}", e),
                StatusCode::BAD_REQUEST,
            )
        })?;

        let bytes = body.into_bytes().await.map_err(|e| {
            poem::Error::from_string(
                format!("Failed to read request body {}", e),
                StatusCode::BAD_REQUEST,
            )
        })?;

        if content_type.contains("json") {
            let json_data = serde_json::from_slice(&bytes).map_err(|e| {
                poem::Error::from_string(
                    format!("Failed to read JSON data {}", e),
                    StatusCode::BAD_REQUEST,
                )
            })?;
            let value =
                T::parse_from_yaml(Some(json_data)).map_err(|err| ParseRequestPayloadError {
                    reason: err.into_message(),
                })?;
            Ok(Self(value))
        } else if content_type.contains("yaml") {
            let yaml_data = serde_yaml::from_slice(&bytes).map_err(|e| {
                poem::Error::from_string(
                    format!("Failed to read YAML data {}", e),
                    StatusCode::BAD_REQUEST,
                )
            })?;

            let value =
                T::parse_from_yaml(Some(yaml_data)).map_err(|err| ParseRequestPayloadError {
                    reason: err.into_message(),
                })?;
            Ok(Self(value))
        } else {
            Err(poem::Error::from_string(
                "Unsupported content type".to_string(),
                StatusCode::BAD_REQUEST,
            ))
        }
    }
}
