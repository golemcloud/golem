#[cfg(test)]
mod tests {
    use crate::gateway_api_definition::http::{
        openapi_export::OpenApiExporter,
        rib_converter::RibConverter,
    };
    use golem_wasm_ast::analysis::{
        AnalysedType, NameTypePair, TypeBool, TypeEnum, TypeF64, TypeList, TypeRecord, TypeResult, TypeStr, TypeU64, TypeVariant,
    };
    use utoipa::{
        openapi::{
            security::{SecurityScheme, OAuth2, Flow, Scopes},
            schema::{Schema, SchemaType, Object as SchemaObject},
            OpenApi, Info, Components,
        },
        ToSchema,
    };
    use serde::{Serialize, Deserialize};
    use serde_json::json;
    use std::collections::{HashMap, BTreeMap};
    use rib::RibInputTypeInfo;

    // Example schema for documentation
    #[derive(Serialize, Deserialize, ToSchema)]
    #[schema(example = json!({
        "username": "john_doe",
        "age": 30,
        "is_active": true,
        "scores": [95.5, 87.3, 91.0]
    }))]
    struct UserProfile {
        username: String,
        age: u64,
        is_active: bool,
        scores: Vec<f64>,
    }

    #[derive(Serialize, Deserialize, ToSchema)]
    #[schema(example = json!(["admin", "user", "guest"]))]
    enum UserRole {
        Admin,
        User,
        Guest,
    }

    #[derive(Serialize, Deserialize, ToSchema)]
    #[schema(example = json!({
        "email": "user@example.com"
    }))]
    enum NotificationType {
        Email(String),
        Sms(String),
    }

    #[derive(Serialize, Deserialize, ToSchema)]
    #[schema(example = json!({
        "ok": "Operation successful",
        "err": null
    }))]
    struct OperationResult {
        ok: Option<String>,
        err: Option<String>,
    }

    #[test]
    fn test_openapi_exporter() {
        // Create OpenAPI document using the builder pattern
        let mut openapi = OpenApi::new()
            .info(Info::new("Original API", "0.1.0"));

        // Create OAuth2 flows
        let implicit_flow = Flow::Implicit {
            authorization_url: "https://auth.example.com/oauth2/authorize".to_string(),
            scopes: Scopes::from_iter([
                ("read".to_string(), "Read access".to_string())
            ]),
            refresh_url: None,
        };

        let auth_code_flow = Flow::AuthorizationCode {
            authorization_url: "https://auth.example.com/oauth2/authorize".to_string(),
            token_url: "https://auth.example.com/oauth2/token".to_string(),
            refresh_url: None,
            scopes: Scopes::from_iter([
                ("write".to_string(), "Write access".to_string())
            ]),
        };

        // Create OAuth2 configuration using builder methods
        let oauth2 = OAuth2::with_description(
            [implicit_flow, auth_code_flow],
            "OAuth 2.0 authentication"
        );

        // Add components with security scheme
        let components = Components::new()
            .security_scheme("oauth2", SecurityScheme::OAuth2(oauth2));
        
        openapi.components = Some(components);

        // Export the OpenAPI document
        let exported = OpenApiExporter::export_openapi(
            "test-api",
            "1.0.0",
            openapi,
        );

        // Verify the exported document
        assert_eq!(exported.info.title, "test-api API");
        assert_eq!(exported.info.version, "1.0.0");

        // Verify security schemes
        let components = exported.components.as_ref().expect("Components should be present");
        let schemes = components.security_schemes.as_ref().expect("Security schemes should be present");
        let oauth2 = schemes.get("oauth2").expect("OAuth2 scheme should be present");

        if let SecurityScheme::OAuth2(oauth2) = oauth2 {
            assert_eq!(oauth2.description.as_deref(), Some("OAuth 2.0 authentication"));

            // Verify implicit flow
            let implicit = oauth2.flows.get("implicit").expect("Implicit flow should be present");
            match implicit {
                Flow::Implicit { authorization_url, scopes, .. } => {
                    assert_eq!(authorization_url, "https://auth.example.com/oauth2/authorize");
                    assert!(scopes.contains_key("read"));
                }
                _ => panic!("Expected implicit flow"),
            }

            // Verify authorization code flow
            let auth_code = oauth2.flows.get("authorization_code").expect("Authorization code flow should be present");
            match auth_code {
                Flow::AuthorizationCode { authorization_url, token_url, scopes, .. } => {
                    assert_eq!(authorization_url, "https://auth.example.com/oauth2/authorize");
                    assert_eq!(token_url, "https://auth.example.com/oauth2/token");
                    assert!(scopes.contains_key("write"));
                }
                _ => panic!("Expected authorization code flow"),
            }
        }
    }

    #[test]
    fn test_complex_api_conversion() {
        // Define complex input types using RIB types
        let mut types = HashMap::new();
        
        // Create a complex record type for user profile
        let user_profile = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "username".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "age".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "is_active".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
                NameTypePair {
                    name: "scores".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::F64(TypeF64)),
                    }),
                },
            ],
        });

        // Create an enum for user role
        let user_role = AnalysedType::Enum(TypeEnum {
            cases: vec!["admin".to_string(), "user".to_string(), "guest".to_string()],
        });

        // Create a variant type for notification
        let notification = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameTypePair {
                    name: "email".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
                NameTypePair {
                    name: "sms".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
            ],
        });

        // Create a result type for operation status
        let operation_result = AnalysedType::Result(TypeResult {
            ok: Some(Box::new(AnalysedType::Str(TypeStr))),
            err: Some(Box::new(AnalysedType::Str(TypeStr))),
        });

        // Add all types to the input type map
        types.insert("user_profile".to_string(), user_profile);
        types.insert("user_role".to_string(), user_role);
        types.insert("notification".to_string(), notification);
        types.insert("operation_result".to_string(), operation_result);

        let input_type = RibInputTypeInfo { types };

        // Convert RIB types to OpenAPI schema
        let rib_converter = RibConverter;
        let schema = rib_converter.convert_input_type(&input_type).expect("Schema conversion should succeed");

        // Verify the converted schema using pattern matching
        match &schema {
            Schema::Object(obj) => {
                // Verify user profile schema
                let user_profile = obj.properties.get("user_profile").expect("User profile should exist");
                match user_profile {
                    Schema::Object(profile_obj) => {
                        assert_eq!(profile_obj.properties.len(), 4);
                        assert!(profile_obj.properties.contains_key("username"));
                        assert!(profile_obj.properties.contains_key("age"));
                        assert!(profile_obj.properties.contains_key("is_active"));
                        assert!(profile_obj.properties.contains_key("scores"));
                    }
                    _ => panic!("User profile should be an object type"),
                }

                // Verify user role schema
                let user_role = obj.properties.get("user_role").expect("User role should exist");
                match user_role {
                    Schema::Object(role_obj) => {
                        let enum_values = role_obj.enum_values.as_ref().expect("Enum values should exist");
                        assert_eq!(enum_values.len(), 3);
                        assert!(enum_values.contains(&serde_json::Value::String("admin".to_string())));
                        assert!(enum_values.contains(&serde_json::Value::String("user".to_string())));
                        assert!(enum_values.contains(&serde_json::Value::String("guest".to_string())));
                    }
                    _ => panic!("User role should be a string type with enum values"),
                }

                // Verify notification schema
                let notification = obj.properties.get("notification").expect("Notification should exist");
                match notification {
                    Schema::Object(notif_obj) => {
                        assert_eq!(notif_obj.properties.len(), 2);
                        assert!(notif_obj.properties.contains_key("email"));
                        assert!(notif_obj.properties.contains_key("sms"));
                    }
                    _ => panic!("Notification should be an object type"),
                }

                // Verify operation result schema
                let operation_result = obj.properties.get("operation_result").expect("Operation result should exist");
                match operation_result {
                    Schema::Object(result_obj) => {
                        assert_eq!(result_obj.properties.len(), 2);
                        assert!(result_obj.properties.contains_key("ok"));
                        assert!(result_obj.properties.contains_key("err"));
                    }
                    _ => panic!("Operation result should be an object type"),
                }
            }
            _ => panic!("Schema should be an object type"),
        }
    }
} 