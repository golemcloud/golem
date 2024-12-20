use crate::api::{
    definition::types::{ApiDefinition, Route, HttpMethod, BindingType},
    openapi::{OpenAPIConverter, Schema, PathItem, Operation},
};
use std::collections::HashMap;

fn create_test_route(path: &str, method: HttpMethod, input: &str, output: &str) -> Route {
    Route {
        path: path.to_string(),
        method,
        description: "Test route".to_string(),
        template_name: "test".to_string(),
        binding: BindingType::Default {
            input_type: input.to_string(),
            output_type: output.to_string(),
            function_name: "test_function".to_string(),
        },
    }
}

#[test]
fn test_primitive_type_conversion() {
    let types = [
        ("string", Schema::String { format: None, enum_values: None }),
        ("i32", Schema::Integer { format: None }),
        ("i64", Schema::Integer { format: None }),
        ("f32", Schema::Number { format: None }),
        ("f64", Schema::Number { format: None }),
        ("bool", Schema::Boolean),
    ];

    for (wit_type, expected_schema) in types {
        let route = create_test_route(
            "/test",
            HttpMethod::Get,
            wit_type,
            wit_type,
        );
        let api = ApiDefinition {
            id: "test".to_string(),
            name: "Test API".to_string(),
            version: "1.0".to_string(),
            description: "Test API".to_string(),
            routes: vec![route],
        };

        let spec = OpenAPIConverter::convert(&api);
        let path_item = spec.paths.get("/test").unwrap();
        if let Some(operation) = &path_item.get {
            if let Some(request_body) = &operation.request_body {
                let schema = &request_body.content["application/json"].schema;
                assert_eq!(&expected_schema, schema);
            }
        }
    }
}

#[test]
fn test_complex_type_conversion() {
    let route = create_test_route(
        "/complex",
        HttpMethod::Post,
        "record{name: string, age: i32, tags: list<string>}",
        "record{id: string, data: record{value: f64, valid: bool}}",
    );

    let api = ApiDefinition {
        id: "test".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API".to_string(),
        routes: vec![route],
    };

    let spec = OpenAPIConverter::convert(&api);
    let path_item = spec.paths.get("/complex").unwrap();
    
    // Verify request body schema
    if let Some(operation) = &path_item.post {
        let schema = &operation.request_body.as_ref().unwrap().content["application/json"].schema;
        match schema {
            Schema::Object { properties, .. } => {
                assert!(properties.contains_key("name"));
                assert!(properties.contains_key("age"));
                assert!(properties.contains_key("tags"));
                
                match &properties["tags"] {
                    Schema::Array { items } => {
                        assert!(matches!(**items, Schema::String { .. }));
                    },
                    _ => panic!("Expected array schema for tags"),
                }
            },
            _ => panic!("Expected object schema"),
        }
    }

    // Verify response schema
    if let Some(operation) = &path_item.post {
        let schema = &operation.responses["200"]
            .content.as_ref().unwrap()["application/json"].schema;
        
        match schema {
            Schema::Object { properties, .. } => {
                assert!(properties.contains_key("id"));
                assert!(properties.contains_key("data"));
                
                match &properties["data"] {
                    Schema::Object { properties, .. } => {
                        assert!(properties.contains_key("value"));
                        assert!(properties.contains_key("valid"));
                    },
                    _ => panic!("Expected object schema for data"),
                }
            },
            _ => panic!("Expected object schema"),
        }
    }
}

#[test]
fn test_path_parameters() {
    let route = create_test_route(
        "/users/{id}/posts/{postId}",
        HttpMethod::Get,
        "string",
        "string",
    );

    let api = ApiDefinition {
        id: "test".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API".to_string(),
        routes: vec![route],
    };

    let spec = OpenAPIConverter::convert(&api);
    let path_item = spec.paths.get("/users/{id}/posts/{postId}").unwrap();
    
    if let Some(parameters) = &path_item.parameters {
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name, "id");
        assert_eq!(parameters[1].name, "postId");
        assert!(parameters.iter().all(|p| p.required == Some(true)));
    } else {
        panic!("Expected path parameters");
    }
}

#[test]
fn test_cors_headers() {
    let route = create_test_route(
        "/test",
        HttpMethod::Get,
        "string",
        "string",
    );

    let api = ApiDefinition {
        id: "test".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API".to_string(),
        routes: vec![route],
    };

    let spec = OpenAPIConverter::convert(&api);
    let path_item = spec.paths.get("/test").unwrap();
    
    // Verify CORS options
    if let Some(options) = &path_item.options {
        let response = &options.responses["200"];
        let headers = response.headers.as_ref().unwrap();
        
        assert!(headers.contains_key("Access-Control-Allow-Origin"));
        assert!(headers.contains_key("Access-Control-Allow-Methods"));
        assert!(headers.contains_key("Access-Control-Allow-Headers"));
    } else {
        panic!("Expected OPTIONS operation");
    }
}

#[test]
fn test_file_server_binding() {
    let route = Route {
        path: "/files/{path}".to_string(),
        method: HttpMethod::Get,
        description: "Serve files".to_string(),
        template_name: "files".to_string(),
        binding: BindingType::FileServer {
            root_dir: "/static".to_string(),
        },
    };

    let api = ApiDefinition {
        id: "test".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API".to_string(),
        routes: vec![route],
    };

    let spec = OpenAPIConverter::convert(&api);
    let path_item = spec.paths.get("/files/{path}").unwrap();
    
    if let Some(operation) = &path_item.get {
        let response = &operation.responses["200"];
        let content = response.content.as_ref().unwrap();
        assert!(content.contains_key("*/*"));
        
        let schema = &content["*/*"].schema;
        assert!(matches!(schema, Schema::String { format: Some(f), .. } if f == "binary"));
    } else {
        panic!("Expected GET operation");
    }
}

#[test]
fn test_swagger_ui_binding() {
    let route = Route {
        path: "/docs".to_string(),
        method: HttpMethod::Get,
        description: "API Documentation".to_string(),
        template_name: "docs".to_string(),
        binding: BindingType::SwaggerUI {
            spec_path: "/api/openapi/my-api/v1".to_string(),
        },
    };

    let api = ApiDefinition {
        id: "my-api".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API".to_string(),
        routes: vec![route],
    };

    let spec = OpenAPIConverter::convert(&api);
    let path_item = spec.paths.get("/docs").unwrap();
    
    // Verify SwaggerUI route is converted correctly
    if let Some(operation) = &path_item.get {
        assert_eq!(
            operation.summary,
            Some("API Documentation".to_string())
        );
        assert!(operation.responses.contains_key("200"));
    } else {
        panic!("Expected GET operation for SwaggerUI");
    }
}
