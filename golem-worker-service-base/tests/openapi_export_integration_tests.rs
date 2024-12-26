#[cfg(test)]
mod openapi_export_integration_tests {
    use golem_wasm_ast::analysis::{
        AnalysedType, TypeBool, TypeStr, TypeU32, TypeVariant, TypeRecord, TypeList,
        NameOptionTypePair, NameTypePair,
    };
    use golem_worker_service_base::gateway_api_definition::http::{
        openapi_export::{OpenApiExporter, OpenApiFormat},
        rib_converter::RibConverter,
    };
    use utoipa::openapi::{
        Components, HttpMethod, Info, OpenApi, PathItem, PathsBuilder, Schema,
        path::OperationBuilder, response::ResponseBuilder,
        request_body::RequestBodyBuilder,
        content::Content,
    };
    use serde_json::Value;

    fn create_complex_api() -> OpenApi {
        // Create a complex record type for the request body
        let request_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "name".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "flags".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Bool(TypeBool)),
                    }),
                },
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::Variant(TypeVariant {
                        cases: vec![
                            NameOptionTypePair {
                                name: "Active".to_string(),
                                typ: None,
                            },
                            NameOptionTypePair {
                                name: "Inactive".to_string(),
                                typ: Some(AnalysedType::Str(TypeStr)),
                            },
                        ],
                    }),
                },
            ],
        });

        // Convert types to OpenAPI schemas
        let converter = RibConverter;
        let request_schema = converter.convert_type(&request_type).unwrap();

        // Create request body content
        let content = Content::new(Some(request_schema.clone()));

        // Create request body
        let request_body = RequestBodyBuilder::new()
            .content("application/json", content)
            .build();

        // Create response
        let response = ResponseBuilder::new()
            .description("Successful response")
            .content("application/json", Content::new(Some(Schema::Object(Default::default()))))
            .build();

        // Create API paths
        let paths = PathsBuilder::new();
        let post_op = OperationBuilder::new()
            .summary(Some("Create complex entity".to_string()))
            .response("200", response)
            .request_body(Some(request_body))
            .build();
        let path_item = PathItem::new(HttpMethod::Post, post_op);
        
        // Create components
        let mut components = Components::new();
        components.schemas.insert(
            "ComplexRequest".to_string(),
            request_schema.into()
        );

        // Build final OpenAPI spec
        let mut openapi = OpenApi::new(
            Info::new("Complex API Test", "1.0.0"),
            paths.path("/api/v1/complex", path_item),
        );
        openapi.components = Some(components);
        
        openapi
    }

    #[test]
    fn test_complex_api_export() {
        let exporter = OpenApiExporter;
        let openapi = create_complex_api();

        // Test JSON export
        let json_format = OpenApiFormat { json: true };
        let exported_json = exporter.export_openapi(
            "complex-api",
            "1.0.0",
            openapi.clone(),
            &json_format,
        );

        // Validate JSON structure
        let json_value: Value = serde_json::from_str(&exported_json).unwrap();
        assert_eq!(json_value["info"]["title"], "complex-api API");
        assert_eq!(json_value["info"]["version"], "1.0.0");
        assert!(json_value["paths"]["/api/v1/complex"]["post"]["requestBody"].is_object());
        assert!(json_value["components"]["schemas"]["ComplexRequest"].is_object());

        // Test YAML export
        let yaml_format = OpenApiFormat { json: false };
        let exported_yaml = exporter.export_openapi(
            "complex-api",
            "1.0.0",
            openapi,
            &yaml_format,
        );

        // Basic YAML validation
        assert!(exported_yaml.contains("title: complex-api API"));
        assert!(exported_yaml.contains("version: '1.0.0'"));
        assert!(exported_yaml.contains("/api/v1/complex:"));
        assert!(exported_yaml.contains("ComplexRequest:"));
    }

    #[test]
    fn test_export_path_generation() {
        let api_id = "test-api";
        let version = "2.0.0";
        let path = OpenApiExporter::get_export_path(api_id, version);
        assert_eq!(path, "/v1/api/definitions/test-api/version/2.0.0/export");
    }
} 