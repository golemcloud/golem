#[cfg(test)]
mod openapi_converter_tests {
    use utoipa::openapi::{
        Components,
        HttpMethod,
        Info,
        Object,
        OpenApi,
        PathItem,
        PathsBuilder,
        Schema,
        SecurityRequirement,
        Server,
        Tag,
        path::OperationBuilder,
        Response,
        security::{SecurityScheme, ApiKeyValue, ApiKey},
    };
    use golem_worker_service_base::gateway_api_definition::http::openapi_converter::OpenApiConverter;

    fn create_test_info() -> Info {
        Info::new("Test API", "1.0.0")
    }

    #[test]
    fn test_merge_openapi_paths() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with a GET endpoint
        let base_paths = PathsBuilder::new();
        let get_op = OperationBuilder::new()
            .summary(Some("Base GET operation".to_string()))
            .build();
        let path_item = PathItem::new(HttpMethod::Get, get_op);
        let base = OpenApi::new(create_test_info(), base_paths.path("/api/v1/resource", path_item));

        // Create other OpenAPI with a POST endpoint
        let other_paths = PathsBuilder::new();
        let post_op = OperationBuilder::new()
            .summary(Some("Other POST operation".to_string()))
            .build();
        let path_item = PathItem::new(HttpMethod::Post, post_op);
        let other = OpenApi::new(create_test_info(), other_paths.path("/api/v1/other-resource", path_item));

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify paths were merged correctly
        assert!(merged.paths.paths.contains_key("/api/v1/resource"));
        assert!(merged.paths.paths.contains_key("/api/v1/other-resource"));
    }

    #[test]
    fn test_merge_openapi_components() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with a schema component
        let mut base = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut base_components = Components::new();
        base_components.schemas.insert(
            "BaseSchema".to_string(),
            Schema::Object(Object::new()).into()
        );
        base.components = Some(base_components);

        // Create other OpenAPI with a different schema component
        let mut other = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut other_components = Components::new();
        other_components.schemas.insert(
            "OtherSchema".to_string(),
            Schema::Object(Object::new()).into()
        );
        other.components = Some(other_components);

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify components were merged correctly
        let components = merged.components.unwrap();
        assert!(components.schemas.contains_key("BaseSchema"));
        assert!(components.schemas.contains_key("OtherSchema"));
    }

    #[test]
    fn test_merge_openapi_security() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with security requirement
        let mut base = OpenApi::new(create_test_info(), PathsBuilder::new());
        let base_security = SecurityRequirement::new("BaseAuth", vec!["read", "write"]);
        base.security = Some(vec![base_security]);

        // Create other OpenAPI with different security requirement
        let mut other = OpenApi::new(create_test_info(), PathsBuilder::new());
        let other_security = SecurityRequirement::new("OtherAuth", vec!["read"]);
        other.security = Some(vec![other_security]);

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify security requirements were merged correctly
        let security = merged.security.unwrap();
        assert_eq!(security.len(), 2);
        
        // Since we can't directly compare security requirements, we'll just verify
        // that both security requirements are present in the merged result
        let has_base_auth = security.iter().any(|s| {
            s == &SecurityRequirement::new("BaseAuth", vec!["read", "write"])
        });
        let has_other_auth = security.iter().any(|s| {
            s == &SecurityRequirement::new("OtherAuth", vec!["read"])
        });
        
        assert!(has_base_auth, "BaseAuth security requirement should be present");
        assert!(has_other_auth, "OtherAuth security requirement should be present");
    }

    #[test]
    fn test_merge_openapi_tags_and_servers() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with tag and server
        let mut base = OpenApi::new(create_test_info(), PathsBuilder::new());
        base.tags = Some(vec![Tag::new("base-tag")]);
        base.servers = Some(vec![Server::new("/base")]);

        // Create other OpenAPI with different tag and server
        let mut other = OpenApi::new(create_test_info(), PathsBuilder::new());
        other.tags = Some(vec![Tag::new("other-tag")]);
        other.servers = Some(vec![Server::new("/other")]);

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify tags were merged correctly
        let tags = merged.tags.unwrap();
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().any(|t| t.name == "base-tag"));
        assert!(tags.iter().any(|t| t.name == "other-tag"));

        // Verify servers were merged correctly
        let servers = merged.servers.unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.iter().any(|s| s.url == "/base"));
        assert!(servers.iter().any(|s| s.url == "/other"));
    }

    #[test]
    fn test_merge_openapi_with_overlapping_paths() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with a GET endpoint
        let base_paths = PathsBuilder::new();
        let get_op = OperationBuilder::new()
            .summary(Some("Base GET operation".to_string()))
            .build();
        let path_item = PathItem::new(HttpMethod::Get, get_op);
        let base = OpenApi::new(create_test_info(), base_paths.path("/api/v1/resource", path_item));

        // Create other OpenAPI with a POST endpoint for the same path
        let other_paths = PathsBuilder::new();
        let post_op = OperationBuilder::new()
            .summary(Some("Other POST operation".to_string()))
            .build();
        let path_item = PathItem::new(HttpMethod::Post, post_op);
        let other = OpenApi::new(create_test_info(), other_paths.path("/api/v1/resource", path_item));

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify the path was merged correctly with both operations
        let path = merged.paths.paths.get("/api/v1/resource").unwrap();
        assert!(path.get.is_some(), "GET operation should be preserved");
        assert!(path.post.is_some(), "POST operation should be added");
    }

    #[test]
    fn test_merge_openapi_empty_components() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with no components
        let base = OpenApi::new(create_test_info(), PathsBuilder::new());

        // Create other OpenAPI with components
        let mut other = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut components = Components::new();
        components.schemas.insert(
            "TestSchema".to_string(),
            Schema::Object(Object::new()).into()
        );
        other.components = Some(components);

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify components were added correctly
        let components = merged.components.unwrap();
        assert!(components.schemas.contains_key("TestSchema"));
    }

    #[test]
    fn test_merge_openapi_response_components() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with a response component
        let mut base = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut base_components = Components::new();
        let base_response = Response::new("Base response description");
        base_components.responses.insert(
            "BaseResponse".to_string(),
            base_response.into()
        );
        base.components = Some(base_components);

        // Create other OpenAPI with a different response component
        let mut other = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut other_components = Components::new();
        let other_response = Response::new("Other response description");
        other_components.responses.insert(
            "OtherResponse".to_string(),
            other_response.into()
        );
        other.components = Some(other_components);

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify response components were merged correctly
        let components = merged.components.unwrap();
        assert!(components.responses.contains_key("BaseResponse"), "Base response should be present");
        assert!(components.responses.contains_key("OtherResponse"), "Other response should be present");
    }

    #[test]
    fn test_merge_openapi_security_schemes() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI with a security scheme
        let mut base = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut base_components = Components::new();
        let base_scheme = SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Base-Key")));
        base_components.security_schemes.insert(
            "BaseScheme".to_string(),
            base_scheme
        );
        base.components = Some(base_components);

        // Create other OpenAPI with a different security scheme
        let mut other = OpenApi::new(create_test_info(), PathsBuilder::new());
        let mut other_components = Components::new();
        let other_scheme = SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Other-Key")));
        other_components.security_schemes.insert(
            "OtherScheme".to_string(),
            other_scheme
        );
        other.components = Some(other_components);

        // Merge the OpenAPI specs
        let merged = OpenApiConverter::merge_openapi(base, other);

        // Verify security schemes were merged correctly
        let components = merged.components.unwrap();
        assert!(components.security_schemes.contains_key("BaseScheme"), "Base security scheme should be present");
        assert!(components.security_schemes.contains_key("OtherScheme"), "Other security scheme should be present");
    }
} 