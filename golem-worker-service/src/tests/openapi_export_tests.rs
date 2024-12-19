use crate::api::*;
use crate::test_utils::*;

#[tokio::test]
async fn test_openapi_export_and_client() {
    // Set up test API definition
    let api = setup_test_api();
    
    // Export OpenAPI spec
    let converter = OpenAPIConverter::new();
    let spec = converter.convert(&api).expect("Failed to convert to OpenAPI");
    
    // Validate the spec
    validate_openapi(&spec).expect("Invalid OpenAPI spec");
    
    // Test metadata preservation
    assert_eq!(spec.info.title, api.name);
    assert_eq!(spec.info.description.as_deref(), Some(api.description.as_str()));
    assert_eq!(spec.info.version, api.version);
    
    // Test security schemes
    if let Some(security) = &api.security {
        assert!(spec.components.is_some());
        let components = spec.components.as_ref().unwrap();
        assert!(!components.security_schemes.is_empty());
    }
}

#[tokio::test]
async fn test_openapi_full_lifecycle() {
    // Setup test API
    let api = setup_test_api();
    
    // Test converter
    let mut converter = CachedOpenAPIConverter::new();
    let spec = converter.convert(&api).expect("Failed to convert API");
    
    // Validate generated spec
    assert_eq!(spec.info.title, api.name);
    assert_eq!(spec.info.version, api.version);
    assert_eq!(spec.info.description.as_deref(), Some(api.description.as_str()));

    // Test caching
    let cached_spec = converter.convert(&api).expect("Failed to get cached spec");
    assert_eq!(spec, cached_spec);

    // Test security schemes
    if let Some(security) = api.security {
        let components = spec.components.expect("Missing components");
        assert!(!components.security_schemes.is_empty());
        
        for (name, scheme) in security.iter() {
            let converted = components.security_schemes.get(name).expect("Missing scheme");
            match scheme {
                SecurityScheme::ApiKey { .. } => {
                    assert!(matches!(converted, SecuritySchemeData::ApiKey { .. }));
                },
                SecurityScheme::OAuth2 { .. } => {
                    assert!(matches!(converted, SecuritySchemeData::OAuth2 { .. }));
                },
            }
        }
    }
}

#[tokio::test]
async fn test_swagger_ui_binding() {
    let binding = BindingType::SwaggerUI {
        spec_path: "/api/openapi.json".to_string(),
        theme: None,
    };

    let handler = binding.create_handler();
    let response = handler.handle(test_request()).await;
    
    assert!(response.status().is_success());
    assert!(response.headers().contains_key("content-type"));
    // More assertions...
}

fn setup_test_api() -> ApiDefinition {
    // ...existing code...
}
