#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, RouteRequest};
    use crate::gateway_binding::{GatewayBinding, WorkerBinding};
    use golem_common::model::ComponentId;
    use golem_service_base::model::VersionedComponentId;
    use uuid::Uuid;
    use rib::Expr;
    use openapiv3;
    use indexmap;
    use serde_json;
    use golem_wasm_ast::analysis;

    #[test]
    fn test_from_http_api_definition_response_data_with_routes() {
        // Create a test HttpApiDefinitionResponseData with routes
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };

        let worker_name_expr = Expr::from_text("let user:string = request.path.user;\n\"worker-${user}\"").unwrap();
        let response_mapping_expr = Expr::from_text("let result = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200u64, body: result}").unwrap();

        let route = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/{user}/get-cart-contents").unwrap(),
            method: MethodPattern::Get,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(worker_name_expr),
                component_id: versioned_component_id,
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(response_mapping_expr),
            }),
            security: None,
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("shopping-cart".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let result = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data);
        assert!(result.is_ok(), "Failed to convert: {:?}", result.err());
        
        let openapi = result.unwrap().0;
        
        // Verify basic structure
        assert_eq!(openapi.openapi, "3.0.0");
        assert_eq!(openapi.info.title, "shopping-cart");
        assert_eq!(openapi.info.version, "0.1.0");
        
        // Verify extensions
        let id_ext = openapi.extensions.get("x-golem-api-definition-id").unwrap();
        let version_ext = openapi.extensions.get("x-golem-api-definition-version").unwrap();
        
        assert_eq!(id_ext.as_str().unwrap(), "shopping-cart");
        assert_eq!(version_ext.as_str().unwrap(), "0.1.0");
        
        // Verify paths
        let path = openapi.paths.paths.get("/v0.1.0/{user}/get-cart-contents").unwrap();
        let path_item = path.as_item().unwrap();
        assert!(path_item.get.is_some(), "GET operation not found");
        
        let get_op = path_item.get.as_ref().unwrap();
        
        // Verify parameters
        assert_eq!(get_op.parameters.len(), 1, "Should have 1 path parameter");
        let param = get_op.parameters[0].as_item().unwrap();
        if let openapiv3::Parameter::Path { parameter_data, .. } = param {
            assert_eq!(parameter_data.name, "user");
            assert!(parameter_data.required);
        } else {
            panic!("Expected path parameter");
        }
        
        // Verify binding
        let binding = get_op.extensions.get("x-golem-api-gateway-binding").unwrap();
        let binding_obj = binding.as_object().unwrap();
        
        assert_eq!(binding_obj.get("binding-type").unwrap().as_str().unwrap(), "default");
        assert_eq!(
            binding_obj.get("component-id").unwrap().as_str().unwrap(),
            "6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd"
        );
        assert_eq!(binding_obj.get("component-version").unwrap().as_u64().unwrap(), 0);
        
        // Verify worker name expression is properly formatted
        let worker_name = binding_obj.get("worker-name").unwrap().as_str().unwrap();
        assert!(worker_name.contains("let user:string = request.path.user;"));
        assert!(worker_name.contains("\"worker-${user}\""));
        assert!(!worker_name.contains('/'), "Worker name should not contain slash");
        
        // Verify response mapping expression is properly formatted
        let response = binding_obj.get("response").unwrap().as_str().unwrap();
        assert!(response.contains("let result = golem:shoppingcart/api.{get-cart-contents}();"));
        assert!(response.contains("{status: 200u64, body: result}"));
        
        // Verify responses
        let responses = &get_op.responses.responses;
        assert!(responses.contains_key(&openapiv3::StatusCode::Code(200)));
        
        // Serialize to JSON and verify
        let serialized = serde_json::to_string_pretty(&openapi).unwrap();
        assert!(serialized.contains(r#""openapi": "3.0.0""#));
        assert!(serialized.contains(r#""title": "shopping-cart""#));
        assert!(serialized.contains(r#""version": "0.1.0""#));
        assert!(serialized.contains(r#""x-golem-api-definition-id": "shopping-cart""#));
        assert!(serialized.contains(r#""x-golem-api-definition-version": "0.1.0""#));
        assert!(serialized.contains(r#""binding-type": "default""#));
        assert!(serialized.contains(r#""component-id": "6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd""#));
    }

    #[test]
    fn test_from_http_api_definition_response_data_basic() {
        // Create a mock HttpApiDefinitionResponseData
        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("shopping-cart".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: Vec::new(),
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let result = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data);
        assert!(result.is_ok(), "Failed to convert: {:?}", result.err());
        
        let openapi = result.unwrap().0;
        
        // Verify basic structure
        assert_eq!(openapi.openapi, "3.0.0");
        assert_eq!(openapi.info.title, "shopping-cart");
        assert_eq!(openapi.info.version, "0.1.0");
        
        // Verify extensions
        let id_ext = openapi.extensions.get("x-golem-api-definition-id").unwrap();
        let version_ext = openapi.extensions.get("x-golem-api-definition-version").unwrap();
        
        assert_eq!(id_ext.as_str().unwrap(), "shopping-cart");
        assert_eq!(version_ext.as_str().unwrap(), "0.1.0");
        
        // Serialize to JSON and verify
        let serialized = serde_json::to_string_pretty(&openapi).unwrap();
        assert!(serialized.contains(r#""openapi": "3.0.0""#));
        assert!(serialized.contains(r#""title": "shopping-cart""#));
        assert!(serialized.contains(r#""version": "0.1.0""#));
        assert!(serialized.contains(r#""x-golem-api-definition-id": "shopping-cart""#));
        assert!(serialized.contains(r#""x-golem-api-definition-version": "0.1.0""#));
    }

    #[test]
    fn test_request_body_schema_generation() {
        // Test with a POST route that takes a request body
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };
        
        // Create an expression that uses request.body
        let worker_name_expr = Expr::from_text("\"worker-cart\"").unwrap();
        let response_mapping_expr = Expr::from_text("let product = request.body;\n{status: 201u64, body: {success: true, id: product.id}}").unwrap();

        // Create type information for request.body
        let mut response_mapping_input = golem_wasm_ast::analysis::types::TypeContainer::default();
        let record_fields = vec![
            golem_wasm_ast::analysis::Field {
                name: "id".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {}),
            },
            golem_wasm_ast::analysis::Field {
                name: "name".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {}),
            },
            golem_wasm_ast::analysis::Field {
                name: "price".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::F32(golem_wasm_ast::analysis::TypeF32 {}),
            },
        ];
        
        let body_type = golem_wasm_ast::analysis::AnalysedType::Record(
            golem_wasm_ast::analysis::TypeRecord { fields: record_fields }
        );
        
        let request_fields = vec![
            golem_wasm_ast::analysis::Field {
                name: "body".to_string(),
                typ: body_type,
            }
        ];
        
        let request_type = golem_wasm_ast::analysis::AnalysedType::Record(
            golem_wasm_ast::analysis::TypeRecord { fields: request_fields }
        );
        
        response_mapping_input.types.insert("request".to_string(), request_type);

        // Create route with request body and response mapping
        let route = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/products").unwrap(),
            method: MethodPattern::Post,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(worker_name_expr),
                component_id: versioned_component_id,
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(response_mapping_expr),
                response_mapping_input: Some(response_mapping_input),
                response_mapping_output: None,
            }),
            security: None,
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("product-service".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let openapi = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data).unwrap().0;
        
        // Get the POST operation
        let path = openapi.paths.paths.get("/v0.1.0/products").unwrap();
        let path_item = path.as_item().unwrap();
        let post_op = path_item.post.as_ref().unwrap();
        
        // Verify request body exists and has the correct structure
        let request_body = post_op.request_body.as_ref().unwrap().as_item().unwrap();
        assert!(request_body.required, "POST request body should be required");
        
        // Verify content type exists
        let content = request_body.content.get("application/json").unwrap();
        let schema = content.schema.as_ref().unwrap().as_item().unwrap();
        
        // Check the schema structure matches our expected request body
        if let openapiv3::SchemaKind::Type(openapiv3::Type::Object(obj)) = &schema.schema_kind {
            assert!(obj.properties.contains_key("id"), "Schema should contain 'id' property");
            assert!(obj.properties.contains_key("name"), "Schema should contain 'name' property");
            assert!(obj.properties.contains_key("price"), "Schema should contain 'price' property");
            
            // Check the types of properties
            let price_schema = obj.properties.get("price").unwrap().as_item().unwrap();
            if let openapiv3::SchemaKind::Type(openapiv3::Type::Number(num)) = &price_schema.schema_kind {
                assert_eq!(num.format, openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::NumberFormat::Float));
            } else {
                panic!("Expected 'price' to be a number with float format");
            }
        } else {
            panic!("Expected object schema for request body");
        }
    }

    #[test]
    fn test_response_schema_type_strictness() {
        // Test proper type conversion for response schema
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };
        
        let worker_name_expr = Expr::from_text("\"worker-cart\"").unwrap();
        let response_mapping_expr = Expr::from_text("let cart_items = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200u64, body: cart_items}").unwrap();

        // Create response output type information with specific numeric types
        let item_fields = vec![
            golem_wasm_ast::analysis::Field {
                name: "product-id".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {}),
            },
            golem_wasm_ast::analysis::Field {
                name: "name".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {}),
            },
            golem_wasm_ast::analysis::Field {
                name: "price".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::F32(golem_wasm_ast::analysis::TypeF32 {}),
            },
            golem_wasm_ast::analysis::Field {
                name: "quantity".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::U32(golem_wasm_ast::analysis::TypeU32 {}),
            }
        ];
        
        let item_type = golem_wasm_ast::analysis::AnalysedType::Record(
            golem_wasm_ast::analysis::TypeRecord { fields: item_fields }
        );
        
        let body_type = golem_wasm_ast::analysis::AnalysedType::List(
            golem_wasm_ast::analysis::TypeList { inner: Box::new(item_type) }
        );
        
        let response_fields = vec![
            golem_wasm_ast::analysis::Field {
                name: "body".to_string(),
                typ: body_type,
            },
            golem_wasm_ast::analysis::Field {
                name: "status".to_string(),
                typ: golem_wasm_ast::analysis::AnalysedType::U64(golem_wasm_ast::analysis::TypeU64 {}),
            }
        ];
        
        let response_type = golem_wasm_ast::analysis::AnalysedType::Record(
            golem_wasm_ast::analysis::TypeRecord { fields: response_fields }
        );
        
        let response_mapping_output = crate::gateway_binding::ResponseMappingOutput {
            analysed_type: response_type,
        };

        let route = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/cart-items").unwrap(),
            method: MethodPattern::Get,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(worker_name_expr),
                component_id: versioned_component_id,
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(response_mapping_expr),
                response_mapping_input: None,
                response_mapping_output: Some(response_mapping_output),
            }),
            security: None,
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("cart-service".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let openapi = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data).unwrap().0;
        
        // Get the GET operation
        let path = openapi.paths.paths.get("/v0.1.0/cart-items").unwrap();
        let path_item = path.as_item().unwrap();
        let get_op = path_item.get.as_ref().unwrap();
        
        // Verify response exists
        let response = get_op.responses.responses.get(&openapiv3::StatusCode::Code(200)).unwrap().as_item().unwrap();
        let content = response.content.get("application/json").unwrap();
        let schema = content.schema.as_ref().unwrap().as_item().unwrap();
        
        // Verify schema structure matches our response mapping output type
        if let openapiv3::SchemaKind::Type(openapiv3::Type::Object(obj)) = &schema.schema_kind {
            assert!(obj.properties.contains_key("body"), "Schema should contain 'body' property");
            assert!(obj.properties.contains_key("status"), "Schema should contain 'status' property");
            
            // Verify body is an array
            let body_schema = obj.properties.get("body").unwrap().as_item().unwrap();
            if let openapiv3::SchemaKind::Type(openapiv3::Type::Array(array)) = &body_schema.schema_kind {
                // Verify array item structure
                let item_schema = array.items.as_ref().unwrap().as_item().unwrap();
                if let openapiv3::SchemaKind::Type(openapiv3::Type::Object(item_obj)) = &item_schema.schema_kind {
                    // Verify correct type for quantity (u32)
                    let quantity_schema = item_obj.properties.get("quantity").unwrap().as_item().unwrap();
                    if let openapiv3::SchemaKind::Type(openapiv3::Type::Integer(int)) = &quantity_schema.schema_kind {
                        assert_eq!(int.format, openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32));
                        assert_eq!(int.minimum, Some(0));
                    } else {
                        panic!("Expected 'quantity' to be an integer");
                    }
                    
                    // Verify correct type for price (f32)
                    let price_schema = item_obj.properties.get("price").unwrap().as_item().unwrap();
                    if let openapiv3::SchemaKind::Type(openapiv3::Type::Number(num)) = &price_schema.schema_kind {
                        assert_eq!(num.format, openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::NumberFormat::Float));
                    } else {
                        panic!("Expected 'price' to be a number with float format");
                    }
                } else {
                    panic!("Expected object schema for array items");
                }
            } else {
                panic!("Expected array schema for body");
            }

            // Verify status is properly typed as integer
            let status_schema = obj.properties.get("status").unwrap().as_item().unwrap();
            if let openapiv3::SchemaKind::Type(openapiv3::Type::Integer(int)) = &status_schema.schema_kind {
                assert_eq!(int.format, openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int64));
                assert_eq!(int.minimum, Some(0));
            } else {
                panic!("Expected 'status' to be an integer");
            }
        } else {
            panic!("Expected object schema for response");
        }
    }

    #[test]
    fn test_path_parameters_with_different_types() {
        // Test path parameters with different types from worker_name_input
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };
        
        let worker_name_expr = Expr::from_text("let user = request.path.user; let id = request.path.id;\n\"worker-${user}-${id}\"").unwrap();
        let response_mapping_expr = Expr::from_text("let id = request.path.id;\n{status: 200u64, body: {id: id}}").unwrap();

        // Create type information for path parameters
        let mut worker_name_input = golem_wasm_ast::analysis::types::TypeContainer::default();
        
        // Set user as string and id as number
        worker_name_input.types.insert(
            "request.path.user".to_string(), 
            golem_wasm_ast::analysis::AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {})
        );
        
        worker_name_input.types.insert(
            "request.path.id".to_string(), 
            golem_wasm_ast::analysis::AnalysedType::U32(golem_wasm_ast::analysis::TypeU32 {})
        );

        let route = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/{user}/items/{id}").unwrap(),
            method: MethodPattern::Get,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(worker_name_expr),
                component_id: versioned_component_id,
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(response_mapping_expr),
                worker_name_input: Some(worker_name_input),
                response_mapping_input: None,
                response_mapping_output: None,
            }),
            security: None,
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("user-items-service".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let openapi = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data).unwrap().0;
        
        // Get the GET operation
        let path = openapi.paths.paths.get("/v0.1.0/{user}/items/{id}").unwrap();
        let path_item = path.as_item().unwrap();
        let get_op = path_item.get.as_ref().unwrap();
        
        // Verify parameters with correct types
        assert_eq!(get_op.parameters.len(), 2, "Should have 2 path parameters");
        
        // Create a map of parameters by name for easier testing
        let mut params = std::collections::HashMap::new();
        for param in &get_op.parameters {
            let param_item = param.as_item().unwrap();
            if let openapiv3::Parameter::Path { parameter_data, .. } = param_item {
                params.insert(parameter_data.name.clone(), parameter_data);
            }
        }
        
        // Check 'user' parameter is string type
        let user_param = params.get("user").unwrap();
        if let openapiv3::ParameterSchemaOrContent::Schema(schema_ref) = &user_param.format {
            let schema = schema_ref.as_item().unwrap();
            if let openapiv3::SchemaKind::Type(openapiv3::Type::String(_)) = &schema.schema_kind {
                // String type is correct
            } else {
                panic!("Expected 'user' parameter to be string type");
            }
        }
        
        // Check 'id' parameter is integer type
        let id_param = params.get("id").unwrap();
        if let openapiv3::ParameterSchemaOrContent::Schema(schema_ref) = &id_param.format {
            let schema = schema_ref.as_item().unwrap();
            if let openapiv3::SchemaKind::Type(openapiv3::Type::Integer(int_type)) = &schema.schema_kind {
                assert_eq!(int_type.format, openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32));
                assert_eq!(int_type.minimum, Some(0));
            } else {
                panic!("Expected 'id' parameter to be integer type");
            }
        }
    }

    #[test]
    fn test_cors_preflight_response_formatting() {
        // Create a test with a CORS preflight route
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        
        // Create an OPTIONS route with CORS preflight binding
        use crate::gateway_middleware::HttpCors;
        
        // Create a custom CORS configuration
        let cors = HttpCors::new(
            "*", 
            "GET, POST, PUT, DELETE, OPTIONS",
            "Content-Type, Authorization",
            Some("X-Request-ID"),
            Some(true),
            Some(8400),
        );
        
        let route = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/api/resource").unwrap(),
            method: MethodPattern::Options,
            binding: GatewayBinding::CorsPreflight(cors),
            security: None,
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("api-with-cors".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let result = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data);
        assert!(result.is_ok(), "Failed to convert: {:?}", result.err());
        
        let openapi = result.unwrap().0;
        
        // Get the OPTIONS operation
        let path = openapi.paths.paths.get("/v0.1.0/api/resource").unwrap();
        let path_item = path.as_item().unwrap();
        let options_op = path_item.options.as_ref().unwrap();
        
        // Verify CORS binding
        let binding = options_op.extensions.get("x-golem-api-gateway-binding").unwrap();
        let binding_obj = binding.as_object().unwrap();
        
        // Check binding type
        assert_eq!(binding_obj.get("binding-type").unwrap().as_str().unwrap(), "cors-preflight");
        
        // Check response format
        let response = binding_obj.get("response").unwrap().as_str().unwrap();
        
        // Verify the response has the expected format
        assert!(response.starts_with("|\n{"), "CORS response should start with a YAML pipe and opening brace");
        
        // Verify all expected headers are present with proper formatting
        assert!(response.contains("Access-Control-Allow-Origin: \"*\""), "Missing Allow-Origin header");
        assert!(response.contains("Access-Control-Allow-Methods: \"GET, POST, PUT, DELETE, OPTIONS\""), "Missing Allow-Methods header");
        assert!(response.contains("Access-Control-Allow-Headers: \"Content-Type, Authorization\""), "Missing Allow-Headers header");
        assert!(response.contains("Access-Control-Expose-Headers: \"X-Request-ID\""), "Missing Expose-Headers header");
        assert!(response.contains("Access-Control-Allow-Credentials: true"), "Missing Allow-Credentials header");
        assert!(response.contains("Access-Control-Max-Age: 8400u64"), "Missing Max-Age header");
        
        // Check that the last header doesn't have a trailing comma
        let max_age_line = response.lines()
            .find(|line| line.contains("Access-Control-Max-Age"))
            .unwrap();
        assert!(!max_age_line.ends_with(","), "Last header should not have a trailing comma");
        
        // Check for proper indentation (2 spaces)
        let header_lines = response.lines()
            .filter(|line| line.contains("Access-Control-"))
            .collect::<Vec<_>>();
        for line in header_lines {
            assert!(line.starts_with("  Access-Control-"), "Header lines should be indented with 2 spaces");
        }
    }

    #[test]
    fn test_route_level_security() {
        // Test with a route that has security
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };

        let worker_name_expr = Expr::from_text("\"secured-worker\"").unwrap();
        let response_mapping_expr = Expr::from_text("{status: 200u64, body: {success: true}}").unwrap();

        // Create a route with security
        let route = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/secured").unwrap(),
            method: MethodPattern::Get,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(worker_name_expr),
                component_id: versioned_component_id,
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(response_mapping_expr),
                response_mapping_input: None,
                response_mapping_output: None,
                worker_name_input: None,
            }),
            security: Some("api_key".to_string()),
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("secured-api".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let openapi = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data).unwrap().0;
        
        // Get the GET operation for the secured route
        let path = openapi.paths.paths.get("/v0.1.0/secured").unwrap();
        let path_item = path.as_item().unwrap();
        let get_op = path_item.get.as_ref().unwrap();
        
        // Verify operation has security
        assert!(get_op.security.is_some(), "Operation should have security");
        
        // Verify security scheme name
        let security = get_op.security.as_ref().unwrap();
        assert_eq!(security.len(), 1, "Should have exactly one security requirement");
        
        let sec_req = &security[0];
        assert!(sec_req.contains_key("api_key"), "Security requirement should contain api_key");
        
        // Verify the security requirement list for api_key is empty (no scopes)
        let scopes = sec_req.get("api_key").unwrap();
        assert!(scopes.is_empty(), "Security requirement scopes should be empty");
    }

    #[test]
    fn test_security_schema_handling() {
        // Create a test with multiple routes having different security requirements
        let uuid = Uuid::parse_str("6e01ef2f-ee1f-4ac1-82d7-78959ef74bbd").unwrap();
        let component_id = ComponentId(uuid);
        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };

        // Create routes with different security requirements
        let route1 = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/secured1").unwrap(),
            method: MethodPattern::Get,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(Expr::from_text("\"worker1\"").unwrap()),
                component_id: versioned_component_id.clone(),
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(Expr::from_text("{status: 200u64, body: {success: true}}").unwrap()),
                response_mapping_input: None,
                response_mapping_output: None,
                worker_name_input: None,
            }),
            security: Some("api_key".to_string()),
            cors: None,
        };

        let route2 = RouteRequest {
            path: AllPathPatterns::parse("/v0.1.0/secured2").unwrap(),
            method: MethodPattern::Post,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(Expr::from_text("\"worker2\"").unwrap()),
                component_id: versioned_component_id.clone(),
                idempotency_key: None,
                response_mapping: crate::gateway_binding::ResponseMapping(Expr::from_text("{status: 201u64, body: {created: true}}").unwrap()),
                response_mapping_input: None,
                response_mapping_output: None,
                worker_name_input: None,
            }),
            security: Some("oauth2".to_string()),
            cors: None,
        };

        let response_data = crate::api::HttpApiDefinitionResponseData {
            id: crate::gateway_api_definition::ApiDefinitionId("secured-api".to_string()),
            version: crate::gateway_api_definition::ApiVersion("0.1.0".to_string()),
            routes: vec![route1, route2],
            draft: true,
            created_at: None,
        };
        
        // Convert to OpenAPI
        let openapi = OpenApiHttpApiDefinitionRequest::from_http_api_definition_response_data(&response_data).unwrap().0;
        
        // Verify components section exists
        assert!(openapi.components.is_some(), "Components section should exist");
        
        // Verify security schemes
        let components = openapi.components.unwrap();
        assert_eq!(components.security_schemes.len(), 2, "Should have 2 security schemes");
        
        // Verify api_key security scheme
        let api_key_scheme = components.security_schemes.get("api_key").unwrap();
        if let openapiv3::SecurityScheme::APIKey { location, name, description } = api_key_scheme {
            assert_eq!(*location, openapiv3::APIKeyLocation::Header);
            assert_eq!(name, "Authorization");
            assert!(description.is_some());
            assert!(description.as_ref().unwrap().contains("api_key"));
        } else {
            panic!("Expected api_key security scheme");
        }
        
        // Verify oauth2 security scheme
        let oauth2_scheme = components.security_schemes.get("oauth2").unwrap();
        if let openapiv3::SecurityScheme::APIKey { location, name, description } = oauth2_scheme {
            assert_eq!(*location, openapiv3::APIKeyLocation::Header);
            assert_eq!(name, "Authorization");
            assert!(description.is_some());
            assert!(description.as_ref().unwrap().contains("oauth2"));
        } else {
            panic!("Expected oauth2 security scheme");
        }
        
        // Verify global security
        assert!(openapi.security.is_some(), "Global security should exist");
        let global_security = openapi.security.unwrap();
        assert_eq!(global_security.len(), 2, "Should have 2 global security requirements");
        
        // Verify individual operation security
        let path1 = openapi.paths.paths.get("/v0.1.0/secured1").unwrap();
        let path_item1 = path1.as_item().unwrap();
        let get_op = path_item1.get.as_ref().unwrap();
        assert!(get_op.security.is_some(), "GET operation should have security");
        let get_security = get_op.security.as_ref().unwrap();
        assert_eq!(get_security.len(), 1, "GET operation should have 1 security requirement");
        assert!(get_security[0].contains_key("api_key"), "GET operation should require api_key");
        
        let path2 = openapi.paths.paths.get("/v0.1.0/secured2").unwrap();
        let path_item2 = path2.as_item().unwrap();
        let post_op = path_item2.post.as_ref().unwrap();
        assert!(post_op.security.is_some(), "POST operation should have security");
        let post_security = post_op.security.as_ref().unwrap();
        assert_eq!(post_security.len(), 1, "POST operation should have 1 security requirement");
        assert!(post_security[0].contains_key("oauth2"), "POST operation should require oauth2");
    }
}