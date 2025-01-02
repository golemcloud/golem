#[cfg(test)]
mod complex_wit_type_validation_tests {
    use golem_wasm_ast::analysis::{
        AnalysedType, TypeBool, TypeStr, TypeU32, TypeVariant, TypeRecord, TypeList,
        NameOptionTypePair, NameTypePair, TypeOption, TypeResult, AnalysedExport,
        TypeS8, TypeU8, TypeS16, TypeU16, TypeS32, TypeS64, TypeU64, TypeF32, TypeF64,
        TypeChr, TypeTuple, TypeFlags,
    };
    use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
    use golem_wasm_rpc::{ValueAndType, Value};
    use utoipa::openapi::Schema;
    use serde_json;
    use valico::json_schema;
    use rib::{self, RibInput, LiteralValue};

    fn validate_json_against_schema(json: &serde_json::Value, schema: &Schema) -> bool {
        let schema_json = serde_json::to_value(schema).unwrap();
        let mut scope = json_schema::Scope::new();
        let schema = scope.compile_and_return(schema_json, false).unwrap();
        schema.validate(json).is_valid()
    }

    #[test]
    fn test_deeply_nested_variant_record_list() {
        let converter = RibConverter;

        // Create a deeply nested type:
        // Variant {
        //   Record {
        //     list: List<Variant {
        //       Record {
        //         flags: List<Bool>,
        //         value: U32
        //       }
        //     }>,
        //     name: String
        //   }
        // }
        let inner_record_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "flags".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Bool(TypeBool)),
                    }),
                },
                NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        };

        let inner_variant_type = TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Data".to_string(),
                    typ: Some(AnalysedType::Record(inner_record_type)),
                },
            ],
        };

        let outer_record_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "list".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Variant(inner_variant_type)),
                    }),
                },
                NameTypePair {
                    name: "name".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        };

        let outer_variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Container".to_string(),
                    typ: Some(AnalysedType::Record(outer_record_type)),
                },
            ],
        });

        // Generate schema
        let schema = converter.convert_type(&outer_variant_type).unwrap();

        // Test valid complex structure
        let json = serde_json::json!({
            "discriminator": "Container",
            "value": {
                "Container": {
                    "list": [
                        {
                            "discriminator": "Data",
                            "value": {
                                "Data": {
                                    "flags": [true, false, true],
                                    "value": 42
                                }
                            }
                        }
                    ],
                    "name": "test"
                }
            }
        });

        assert!(validate_json_against_schema(&json, &schema));

        // Verify round-trip through TypeAnnotatedValue
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json, &outer_variant_type).unwrap();
        let round_trip_json = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&round_trip_json, &schema));
    }

    #[test]
    fn test_nested_variants() {
        let converter = RibConverter;

        // Create nested variants:
        // Variant {
        //   Variant {
        //     Option<Variant {
        //       Result<U32, String>
        //     }>
        //   }
        // }
        let result_type = TypeResult {
            ok: Some(Box::new(AnalysedType::U32(TypeU32))),
            err: Some(Box::new(AnalysedType::Str(TypeStr))),
        };

        let inner_variant_type = TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Success".to_string(),
                    typ: Some(AnalysedType::Result(result_type)),
                },
            ],
        };

        let middle_variant_type = TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Inner".to_string(),
                    typ: Some(AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::Variant(inner_variant_type)),
                    })),
                },
            ],
        };

        let outer_variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Outer".to_string(),
                    typ: Some(AnalysedType::Variant(middle_variant_type)),
                },
            ],
        });

        // Generate schema
        let schema = converter.convert_type(&outer_variant_type).unwrap();

        // Test valid nested structure
        let json = serde_json::json!({
            "discriminator": "Outer",
            "value": {
                "Outer": {
                    "discriminator": "Inner",
                    "value": {
                        "Inner": {
                            "value": {
                                "discriminator": "Success",
                                "value": {
                                    "Success": {
                                        "ok": 42
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        assert!(validate_json_against_schema(&json, &schema));

        // Verify round-trip through TypeAnnotatedValue
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json, &outer_variant_type).unwrap();
        let round_trip_json = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&round_trip_json, &schema));
    }

    #[test]
    fn test_complex_record_nesting() {
        let converter = RibConverter;

        // Create deeply nested records:
        // Record {
        //   data: Record {
        //     items: List<Record {
        //       flags: List<Bool>,
        //       meta: Record {
        //         id: U32,
        //         name: String
        //       }
        //     }>
        //   }
        // }
        let meta_record_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "name".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        };

        let item_record_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "flags".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Bool(TypeBool)),
                    }),
                },
                NameTypePair {
                    name: "meta".to_string(),
                    typ: AnalysedType::Record(meta_record_type),
                },
            ],
        };

        let data_record_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "items".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Record(item_record_type)),
                    }),
                },
            ],
        };

        let root_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "data".to_string(),
                    typ: AnalysedType::Record(data_record_type),
                },
            ],
        });

        // Generate schema
        let schema = converter.convert_type(&root_type).unwrap();

        // Test valid nested structure
        let json = serde_json::json!({
            "data": {
                "items": [
                    {
                        "flags": [true, false],
                        "meta": {
                            "id": 1,
                            "name": "item1"
                        }
                    },
                    {
                        "flags": [false, true],
                        "meta": {
                            "id": 2,
                            "name": "item2"
                        }
                    }
                ]
            }
        });

        assert!(validate_json_against_schema(&json, &schema));

        // Verify round-trip through TypeAnnotatedValue
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json, &root_type).unwrap();
        let round_trip_json = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&round_trip_json, &schema));
    }

    #[test]
    fn test_rib_script_compilation_and_evaluation() {
        let converter = RibConverter;

        // Create a complex type for testing
        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "flag".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
            ],
        });

        // Generate schema
        let schema = converter.convert_type(&record_type).unwrap();

        // Create a Rib script that constructs a value of this type
        let rib_script = r#"{ value = 42, flag = true }"#;
        let expr = rib::from_string(rib_script).unwrap();
        
        // Compile the Rib script
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        // Evaluate the compiled Rib script
        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        // Convert the result to JSON
        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &record_type).unwrap();
        let json_value = annotated_value.to_json_value();

        // Validate the JSON against the schema
        assert!(validate_json_against_schema(&json_value, &schema));
    }

    #[test]
    fn test_worker_gateway_json_rendering() {
        let converter = RibConverter;

        // Create a complex nested type that mimics a typical Worker Gateway response
        let response_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::Variant(TypeVariant {
                        cases: vec![
                            NameOptionTypePair {
                                name: "Success".to_string(),
                                typ: Some(AnalysedType::Record(TypeRecord {
                                    fields: vec![
                                        NameTypePair {
                                            name: "data".to_string(),
                                            typ: AnalysedType::List(TypeList {
                                                inner: Box::new(AnalysedType::Record(TypeRecord {
                                                    fields: vec![
                                                        NameTypePair {
                                                            name: "id".to_string(),
                                                            typ: AnalysedType::U32(TypeU32),
                                                        },
                                                        NameTypePair {
                                                            name: "name".to_string(),
                                                            typ: AnalysedType::Str(TypeStr),
                                                        },
                                                    ],
                                                })),
                                            }),
                                        },
                                    ],
                                })),
                            },
                            NameOptionTypePair {
                                name: "Error".to_string(),
                                typ: Some(AnalysedType::Str(TypeStr)),
                            },
                        ],
                    }),
                },
                NameTypePair {
                    name: "metadata".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "timestamp".to_string(),
                                    typ: AnalysedType::U32(TypeU32),
                                },
                            ],
                        })),
                    }),
                },
            ],
        });

        // Generate schema
        let schema = converter.convert_type(&response_type).unwrap();

        // Create a Rib script that constructs a response value
        let rib_script = r#"{
            status = Success({
                data = [
                    { id = 1, name = "item1" },
                    { id = 2, name = "item2" }
                ]
            }),
            metadata = Some({ timestamp = 1234567890 })
        }"#;
        let expr = rib::from_string(rib_script).unwrap();
        
        // Compile and evaluate
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        // Convert to JSON using Worker Gateway's JSON rendering
        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &response_type).unwrap();
        let json_value = annotated_value.to_json_value();

        // Validate against schema
        assert!(validate_json_against_schema(&json_value, &schema));

        // Verify specific JSON structure
        assert_eq!(
            json_value["status"]["discriminator"].as_str().unwrap(),
            "Success"
        );
        assert_eq!(
            json_value["status"]["value"]["Success"]["data"][0]["id"].as_u64().unwrap(),
            1
        );
        assert_eq!(
            json_value["metadata"]["value"]["timestamp"].as_u64().unwrap(),
            1234567890
        );

        // Test error case
        let error_script = r#"{
            status = Error("Something went wrong"),
            metadata = None
        }"#;
        let error_expr = rib::from_string(error_script).unwrap();
        let error_compiled = rib::compile_with_limited_globals(
            &error_expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let error_result = tokio_test::block_on(async {
            rib::interpret(&error_compiled.byte_code, &rib_input, worker_invoke_function).await
        }).unwrap();

        let literal = error_result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &response_type).unwrap();
        let error_json = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&error_json, &schema));
        assert_eq!(
            error_json["status"]["discriminator"].as_str().unwrap(),
            "Error"
        );
        assert_eq!(
            error_json["status"]["value"]["Error"].as_str().unwrap(),
            "Something went wrong"
        );
    }

    #[test]
    fn test_all_primitive_types() {
        let converter = RibConverter;

        // Test all integer types
        let test_cases: Vec<(AnalysedType, &str, serde_json::Value)> = vec![
            (AnalysedType::S8(TypeS8), "1", serde_json::json!(1)),
            (AnalysedType::U8(TypeU8), "1", serde_json::json!(1)),
            (AnalysedType::S16(TypeS16), "1", serde_json::json!(1)),
            (AnalysedType::U16(TypeU16), "1", serde_json::json!(1)),
            (AnalysedType::S32(TypeS32), "1", serde_json::json!(1)),
            (AnalysedType::U32(TypeU32), "1", serde_json::json!(1)),
            (AnalysedType::S64(TypeS64), "1", serde_json::json!(1)),
            (AnalysedType::U64(TypeU64), "1", serde_json::json!(1)),
            (AnalysedType::F32(TypeF32), "1.0", serde_json::json!(1.0)),
            (AnalysedType::F64(TypeF64), "1.0", serde_json::json!(1.0)),
        ];

        for (typ, rib_value, expected) in test_cases {
            let schema = converter.convert_type(&typ).unwrap();
            
            // Create and compile Rib script
            let expr = rib::from_string(rib_value).unwrap();
            let exports: Vec<AnalysedExport> = vec![];
            let compiled = rib::compile_with_limited_globals(
                &expr,
                &exports,
                Some(vec!["request".to_string()]),
            ).unwrap();

            // Evaluate
            let rib_input = RibInput::default();
            let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
                Box::pin(async { 
                    Ok(ValueAndType::new(
                        Value::Option(None),
                        AnalysedType::Bool(TypeBool)
                    ))
                })
            });

            let result = tokio_test::block_on(async {
                rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
            }).unwrap();

            // Convert to JSON and verify
            let literal = result.get_literal().unwrap();
            let json_value = match literal {
                LiteralValue::Bool(b) => serde_json::json!(b),
                LiteralValue::Num(n) => serde_json::json!(n.to_string()),
                LiteralValue::String(s) => serde_json::json!(s),
            };
            let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &typ).unwrap();
            let json_value = annotated_value.to_json_value();

            assert!(validate_json_against_schema(&json_value, &schema));
            assert_eq!(json_value, expected);

            // Test invalid values for each type
            let invalid_json = match &typ {
                AnalysedType::Bool(_) => serde_json::json!(42),
                AnalysedType::S8(_) | AnalysedType::U8(_) | AnalysedType::S16(_) | AnalysedType::U16(_) |
                AnalysedType::S32(_) | AnalysedType::U32(_) | AnalysedType::S64(_) | AnalysedType::U64(_) => 
                    serde_json::json!("not a number"),
                AnalysedType::F32(_) | AnalysedType::F64(_) => serde_json::json!("not a float"),
                AnalysedType::Chr(_) | AnalysedType::Str(_) => serde_json::json!(42),
                _ => continue,
            };
            assert!(!validate_json_against_schema(&invalid_json, &schema));
        }

        // Test char
        let char_type = AnalysedType::Chr(TypeChr);
        let schema = converter.convert_type(&char_type).unwrap();
        let expr = rib::from_string("'a'").unwrap();
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &char_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));

        // Test string
        let string_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&string_type).unwrap();
        let expr = rib::from_string("\"hello\"").unwrap();
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &string_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));

        // Test bool
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type).unwrap();
        let expr = rib::from_string("true").unwrap();
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &bool_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));
    }

    #[test]
    fn test_complex_composite_types() {
        let converter = RibConverter;

        // Test tuple containing variant and list
        let tuple_type = AnalysedType::Tuple(TypeTuple {
            items: vec![
                AnalysedType::Variant(TypeVariant {
                    cases: vec![
                        NameOptionTypePair {
                            name: "A".to_string(),
                            typ: Some(AnalysedType::U32(TypeU32)),
                        },
                        NameOptionTypePair {
                            name: "B".to_string(),
                            typ: Some(AnalysedType::Str(TypeStr)),
                        },
                    ],
                }),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Bool(TypeBool)),
                }),
            ],
        });

        let schema = converter.convert_type(&tuple_type).unwrap();
        let rib_script = r#"(A(42), [true, false])"#;
        let expr = rib::from_string(rib_script).unwrap();
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &tuple_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));

        // Test flags
        let flags_type = AnalysedType::Flags(TypeFlags {
            names: vec![
                "READ".to_string(),
                "WRITE".to_string(),
                "EXECUTE".to_string(),
            ],
        });

        let schema = converter.convert_type(&flags_type).unwrap();
        let rib_script = r#"{ READ = true, WRITE = false, EXECUTE = true }"#;
        let expr = rib::from_string(rib_script).unwrap();
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &flags_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));

        // Test list of options
        let list_of_options_type = AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::U32(TypeU32)),
            })),
        });

        let schema = converter.convert_type(&list_of_options_type).unwrap();
        let rib_script = r#"[Some(1), None, Some(2)]"#;
        let expr = rib::from_string(rib_script).unwrap();
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &list_of_options_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));

        // Test variant containing result containing option
        let complex_variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Success".to_string(),
                    typ: Some(AnalysedType::Result(TypeResult {
                        ok: Some(Box::new(AnalysedType::Option(TypeOption {
                            inner: Box::new(AnalysedType::U32(TypeU32)),
                        }))),
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    })),
                },
            ],
        });

        let schema = converter.convert_type(&complex_variant_type).unwrap();
        let rib_script = r#"Success(Ok(Some(42)))"#;
        let expr = rib::from_string(rib_script).unwrap();
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &complex_variant_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));
    }

    #[test]
    fn test_comprehensive_tuple_validation() {
        let converter = RibConverter;

        // Test empty tuple
        let empty_tuple_type = AnalysedType::Tuple(TypeTuple {
            items: vec![],
        });
        let schema = converter.convert_type(&empty_tuple_type).unwrap();
        let json = serde_json::json!([]);
        assert!(validate_json_against_schema(&json, &schema));

        // Test tuple with primitive types
        let primitive_tuple_type = AnalysedType::Tuple(TypeTuple {
            items: vec![
                AnalysedType::U32(TypeU32),
                AnalysedType::Str(TypeStr),
                AnalysedType::Bool(TypeBool),
                AnalysedType::F64(TypeF64),
            ],
        });
        let schema = converter.convert_type(&primitive_tuple_type).unwrap();
        let json = serde_json::json!([42, "hello", true, 3.14]);
        assert!(validate_json_against_schema(&json, &schema));

        // Test tuple with complex nested types
        let complex_tuple_type = AnalysedType::Tuple(TypeTuple {
            items: vec![
                // List of integers
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::U32(TypeU32)),
                }),
                // Option of string
                AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
                // Record with two fields
                AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "x".to_string(),
                            typ: AnalysedType::U32(TypeU32),
                        },
                        NameTypePair {
                            name: "y".to_string(),
                            typ: AnalysedType::U32(TypeU32),
                        },
                    ],
                }),
            ],
        });
        let schema = converter.convert_type(&complex_tuple_type).unwrap();
        let json = serde_json::json!([
            [1, 2, 3],
            { "value": "optional" },
            { "x": 10, "y": 20 }
        ]);
        assert!(validate_json_against_schema(&json, &schema));

        // Test tuple with variant
        let variant_tuple_type = AnalysedType::Tuple(TypeTuple {
            items: vec![
                AnalysedType::Variant(TypeVariant {
                    cases: vec![
                        NameOptionTypePair {
                            name: "Number".to_string(),
                            typ: Some(AnalysedType::U32(TypeU32)),
                        },
                        NameOptionTypePair {
                            name: "Text".to_string(),
                            typ: Some(AnalysedType::Str(TypeStr)),
                        },
                    ],
                }),
                AnalysedType::U32(TypeU32),
            ],
        });
        let schema = converter.convert_type(&variant_tuple_type).unwrap();
        let json = serde_json::json!([
            {
                "discriminator": "Number",
                "value": { "Number": 42 }
            },
            123
        ]);
        assert!(validate_json_against_schema(&json, &schema));

        // Verify invalid tuple schemas
        let invalid_json = serde_json::json!({});  // Object instead of array
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        let invalid_json = serde_json::json!([1, 2, 3]);  // Wrong number of elements
        assert!(!validate_json_against_schema(&invalid_json, &schema));
    }

    #[test]
    fn test_comprehensive_flags_validation() {
        let converter = RibConverter;

        // Test empty flags
        let empty_flags_type = AnalysedType::Flags(TypeFlags {
            names: vec![],
        });
        let schema = converter.convert_type(&empty_flags_type).unwrap();
        let json = serde_json::json!({});
        assert!(validate_json_against_schema(&json, &schema));

        // Test simple flags
        let simple_flags_type = AnalysedType::Flags(TypeFlags {
            names: vec![
                "READ".to_string(),
                "WRITE".to_string(),
                "EXECUTE".to_string(),
            ],
        });
        let schema = converter.convert_type(&simple_flags_type).unwrap();

        // Test all combinations
        let json = serde_json::json!({
            "READ": true,
            "WRITE": true,
            "EXECUTE": true
        });
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!({
            "READ": true,
            "WRITE": false,
            "EXECUTE": true
        });
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!({
            "READ": false,
            "WRITE": false,
            "EXECUTE": false
        });
        assert!(validate_json_against_schema(&json, &schema));

        // Test flags with special characters and longer names
        let special_flags_type = AnalysedType::Flags(TypeFlags {
            names: vec![
                "SUPER_USER_ACCESS".to_string(),
                "SYSTEM_ADMIN_RIGHTS".to_string(),
                "DATABASE_READ_WRITE".to_string(),
                "API_MANAGEMENT".to_string(),
            ],
        });
        let schema = converter.convert_type(&special_flags_type).unwrap();
        let json = serde_json::json!({
            "SUPER_USER_ACCESS": true,
            "SYSTEM_ADMIN_RIGHTS": false,
            "DATABASE_READ_WRITE": true,
            "API_MANAGEMENT": false
        });
        assert!(validate_json_against_schema(&json, &schema));

        // Test invalid flags schemas
        let invalid_json = serde_json::json!([]);  // Array instead of object
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        let invalid_json = serde_json::json!({
            "INVALID_FLAG": true,  // Unknown flag
            "READ": true
        });
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        let invalid_json = serde_json::json!({
            "READ": "true"  // String instead of boolean
        });
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        // Test flags with Rib script evaluation
        let flags_type = AnalysedType::Flags(TypeFlags {
            names: vec![
                "READ".to_string(),
                "WRITE".to_string(),
                "EXECUTE".to_string(),
            ],
        });
        let schema = converter.convert_type(&flags_type).unwrap();
        
        let rib_script = r#"{ READ = true, WRITE = false, EXECUTE = true }"#;
        let expr = rib::from_string(rib_script).unwrap();
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &flags_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));
    }

    #[test]
    fn test_deeply_nested_options_and_results() {
        let converter = RibConverter;

        // Create a deeply nested type:
        // Option<Result<Option<List<Result<Option<U32>, String>>>, String>>
        let inner_result_type = TypeResult {
            ok: Some(Box::new(AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::U32(TypeU32)),
            }))),
            err: Some(Box::new(AnalysedType::Str(TypeStr))),
        };

        let list_type = TypeList {
            inner: Box::new(AnalysedType::Result(inner_result_type)),
        };

        let nested_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::List(list_type)),
                }))),
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            })),
        });

        let schema = converter.convert_type(&nested_type).unwrap();

        // Test successful case with all values present
        let json = serde_json::json!({
            "value": {
                "ok": {
                    "value": [
                        { "ok": { "value": 42 } },
                        { "err": "inner error" },
                        { "ok": { "value": null } },
                        { "ok": { "value": 100 } }
                    ]
                }
            }
        });
        assert!(validate_json_against_schema(&json, &schema));

        // Test with null at different levels
        let json = serde_json::json!({ "value": null });  // Top-level Option is None
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!({
            "value": {
                "ok": { "value": [] }  // Empty list
            }
        });
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!({
            "value": {
                "err": "top level error"  // Result is Err
            }
        });
        assert!(validate_json_against_schema(&json, &schema));

        // Test with Rib script
        let rib_script = r#"Some(Ok(Some([Ok(Some(42)), Err("error"), Ok(None)])))"#;
        let expr = rib::from_string(rib_script).unwrap();
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &nested_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));
    }

    #[test]
    fn test_list_of_complex_variants() {
        let converter = RibConverter;

        // Create a complex variant type:
        // List<Variant {
        //   Simple,
        //   WithData(Record {
        //     id: U32,
        //     data: Option<List<Result<String, U32>>>
        //   }),
        //   Nested(Variant {
        //     First(U32),
        //     Second(String)
        //   })
        // }>
        let inner_result_type = TypeResult {
            ok: Some(Box::new(AnalysedType::Str(TypeStr))),
            err: Some(Box::new(AnalysedType::U32(TypeU32))),
        };

        let record_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "data".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::List(TypeList {
                            inner: Box::new(AnalysedType::Result(inner_result_type)),
                        })),
                    }),
                },
            ],
        };

        let nested_variant = TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "First".to_string(),
                    typ: Some(AnalysedType::U32(TypeU32)),
                },
                NameOptionTypePair {
                    name: "Second".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
            ],
        };

        let variant_type = TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Simple".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "WithData".to_string(),
                    typ: Some(AnalysedType::Record(record_type)),
                },
                NameOptionTypePair {
                    name: "Nested".to_string(),
                    typ: Some(AnalysedType::Variant(nested_variant)),
                },
            ],
        };

        let list_type = AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Variant(variant_type)),
        });

        let schema = converter.convert_type(&list_type).unwrap();

        // Test with various combinations
        let json = serde_json::json!([
            {
                "discriminator": "Simple",
                "value": { "Simple": null }
            },
            {
                "discriminator": "WithData",
                "value": {
                    "WithData": {
                        "id": 42,
                        "data": {
                            "value": [
                                { "ok": "success" },
                                { "err": 404 },
                                { "ok": "another success" }
                            ]
                        }
                    }
                }
            },
            {
                "discriminator": "Nested",
                "value": {
                    "Nested": {
                        "discriminator": "First",
                        "value": { "First": 123 }
                    }
                }
            },
            {
                "discriminator": "WithData",
                "value": {
                    "WithData": {
                        "id": 43,
                        "data": { "value": null }  // Option is None
                    }
                }
            }
        ]);
        assert!(validate_json_against_schema(&json, &schema));

        // Test empty list
        let json = serde_json::json!([]);
        assert!(validate_json_against_schema(&json, &schema));

        // Test invalid cases
        let invalid_json = serde_json::json!([
            {
                "discriminator": "Invalid",  // Invalid discriminator
                "value": null
            }
        ]);
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        let invalid_json = serde_json::json!([
            {
                "discriminator": "WithData",
                "value": {
                    "WithData": {
                        "id": "not a number",  // Wrong type for id
                        "data": null
                    }
                }
            }
        ]);
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        // Test with Rib script
        let rib_script = r#"[
            Simple,
            WithData({ id = 42, data = Some([Ok("success"), Err(404)]) }),
            Nested(First(123))
        ]"#;
        let expr = rib::from_string(rib_script).unwrap();
        let exports: Vec<AnalysedExport> = vec![];
        let compiled = rib::compile_with_limited_globals(
            &expr,
            &exports,
            Some(vec!["request".to_string()]),
        ).unwrap();

        let rib_input = RibInput::default();
        let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
            Box::pin(async { 
                Ok(ValueAndType::new(
                    Value::Option(None),
                    AnalysedType::Bool(TypeBool)
                ))
            })
        });

        let result = tokio_test::block_on(async {
            rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function).await
        }).unwrap();

        let literal = result.get_literal().unwrap();
        let json_value = match literal {
            LiteralValue::Bool(b) => serde_json::json!(b),
            LiteralValue::Num(n) => serde_json::json!(n.to_string()),
            LiteralValue::String(s) => serde_json::json!(s),
        };
        let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &list_type).unwrap();
        let json_value = annotated_value.to_json_value();
        assert!(validate_json_against_schema(&json_value, &schema));
    }

    #[test]
    fn test_edge_cases_and_invalid_json() {
        let converter = RibConverter;

        // Test case 1: Deeply nested empty structures
        let empty_nested_type = AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::List(TypeList {
                            inner: Box::new(AnalysedType::U32(TypeU32)),
                        })),
                    })),
                })),
            })),
        });

        let schema = converter.convert_type(&empty_nested_type).unwrap();

        // Valid empty structures
        let json = serde_json::json!([]);  // Empty outer list
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!([{ "value": null }]);  // List with one None option
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!([
            { "value": [] },  // Empty inner list
            { "value": [{ "value": null }] },  // Inner list with None
            { "value": [{ "value": [] }] }  // Inner list with empty innermost list
        ]);
        assert!(validate_json_against_schema(&json, &schema));

        // Invalid structures
        let invalid_json = serde_json::json!(null);  // null instead of array
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        let invalid_json = serde_json::json!([null]);  // null instead of option object
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        // Test case 2: Mixed optional and required fields in record
        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "required".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "optional".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    }),
                },
            ],
        });

        let schema = converter.convert_type(&record_type).unwrap();

        // Valid cases
        let json = serde_json::json!({
            "required": 42,
            "optional": { "value": "present" }
        });
        assert!(validate_json_against_schema(&json, &schema));

        let json = serde_json::json!({
            "required": 42,
            "optional": { "value": null }
        });
        assert!(validate_json_against_schema(&json, &schema));

        // Invalid cases
        let invalid_json = serde_json::json!({
            "optional": { "value": "missing required" }
        });
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        let invalid_json = serde_json::json!({
            "required": null,  // null not allowed for required field
            "optional": { "value": "present" }
        });
        assert!(!validate_json_against_schema(&invalid_json, &schema));

        // Test case 3: Complex Rib script edge cases
        let complex_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Empty".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "Data".to_string(),
                    typ: Some(AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Option(TypeOption {
                            inner: Box::new(AnalysedType::U32(TypeU32)),
                        })),
                    })),
                },
            ],
        });

        let schema = converter.convert_type(&complex_type).unwrap();

        // Test various Rib scripts
        let test_scripts = vec![
            (r#"Empty"#, true),
            (r#"Data([])"#, true),
            (r#"Data([Some(42), None, Some(0)])"#, true),
            (r#"Data([Some(1), Some(2), Some(3)])"#, true),
        ];

        for (script, should_validate) in test_scripts {
            let expr = rib::from_string(script).unwrap();
            let exports: Vec<AnalysedExport> = vec![];
            let compiled = rib::compile_with_limited_globals(
                &expr,
                &exports,
                Some(vec!["request".to_string()]),
            ).unwrap();

            let rib_input = RibInput::default();
            let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
                Box::pin(async { 
                    Ok(ValueAndType::new(
                        Value::Option(None),
                        AnalysedType::Bool(TypeBool)
                    ))
                })
            });

            let result = tokio_test::block_on(async {
                rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
            }).unwrap();

            let literal = result.get_literal().unwrap();
            let json_value = match literal {
                LiteralValue::Bool(b) => serde_json::json!(b),
                LiteralValue::Num(n) => serde_json::json!(n.to_string()),
                LiteralValue::String(s) => serde_json::json!(s),
            };
            let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &complex_type).unwrap();
            let json_value = annotated_value.to_json_value();
            assert_eq!(validate_json_against_schema(&json_value, &schema), should_validate);

            // Verify the structure if it should validate
            if should_validate {
                assert_eq!(json_value["discriminator"], "Complex");
                assert!(json_value["value"]["Complex"]["metadata"].is_array());
                
                if let Some(data) = json_value["value"]["Complex"]["data"]["value"].as_object() {
                    match data.keys().next().unwrap().as_str() {
                        "ListCase" => {
                            let list = data["ListCase"].as_array().unwrap();
                            for item in list {
                                assert!(item.get("ok").is_some() || item.get("err").is_some());
                            }
                        },
                        "RecordCase" => {
                            let record = &data["RecordCase"];
                            assert!(record["flags"].is_object());
                            assert!(record["value"].is_number());
                        },
                        _ => panic!("Unexpected variant case"),
                    }
                }
            }
        }

        // Test invalid cases
        let invalid_scripts = vec![
            // Invalid flags
            (r#"Complex({
                data = Some(RecordCase({
                    flags = { A = true, B = "invalid", C = true },
                    value = 42
                })),
                metadata = ["test"]
            })"#, false),
            // Invalid result type
            (r#"Complex({
                data = Some(ListCase([Ok(42), Err("invalid")])),
                metadata = ["test"]
            })"#, false),
            // Missing required field
            (r#"Complex({
                data = Some(RecordCase({
                    flags = { A = true, B = false, C = true }
                })),
                metadata = ["test"]
            })"#, false),
        ];

        for (script, should_validate) in invalid_scripts {
            let expr = rib::from_string(script).unwrap();
            let exports: Vec<AnalysedExport> = vec![];
            let compiled = rib::compile_with_limited_globals(
                &expr,
                &exports,
                Some(vec!["request".to_string()]),
            ).unwrap();

            let rib_input = RibInput::default();
            let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
                Box::pin(async { 
                    Ok(ValueAndType::new(
                        Value::Option(None),
                        AnalysedType::Bool(TypeBool)
                    ))
                })
            });

            let result = tokio_test::block_on(async {
                rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function).await
            }).unwrap();

            let literal = result.get_literal().unwrap();
            let json_value = match literal {
                LiteralValue::Bool(b) => serde_json::json!(b),
                LiteralValue::Num(n) => serde_json::json!(n.to_string()),
                LiteralValue::String(s) => serde_json::json!(s),
            };
            let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &complex_type).unwrap();
            let json_value = annotated_value.to_json_value();
            assert_eq!(validate_json_against_schema(&json_value, &schema), should_validate);
        }
    }

    #[test]
    fn test_exhaustive_wit_type_combinations() {
        let converter = RibConverter;

        // Test all primitive types with their Rib script representations, including edge cases
        let primitive_test_cases: Vec<(AnalysedType, &str, serde_json::Value)> = vec![
            // Integer types with edge cases
            (AnalysedType::S8(TypeS8), "-128", serde_json::json!(-128)), // min i8
            (AnalysedType::S8(TypeS8), "127", serde_json::json!(127)),   // max i8
            (AnalysedType::U8(TypeU8), "0", serde_json::json!(0)),       // min u8
            (AnalysedType::U8(TypeU8), "255", serde_json::json!(255)),   // max u8
            (AnalysedType::S16(TypeS16), "-32768", serde_json::json!(-32768)), // min i16
            (AnalysedType::S16(TypeS16), "32767", serde_json::json!(32767)),   // max i16
            (AnalysedType::U16(TypeU16), "0", serde_json::json!(0)),           // min u16
            (AnalysedType::U16(TypeU16), "65535", serde_json::json!(65535)),   // max u16
            (AnalysedType::S32(TypeS32), "-2147483648", serde_json::json!(-2147483648)), // min i32
            (AnalysedType::S32(TypeS32), "2147483647", serde_json::json!(2147483647)),   // max i32
            (AnalysedType::U32(TypeU32), "0", serde_json::json!(0)),                     // min u32
            (AnalysedType::U32(TypeU32), "4294967295", serde_json::json!("4294967295")),   // max u32
            (AnalysedType::S64(TypeS64), "-9223372036854775808", serde_json::json!("-9223372036854775808")), // min i64
            (AnalysedType::S64(TypeS64), "9223372036854775807", serde_json::json!("9223372036854775807")),   // max i64
            (AnalysedType::U64(TypeU64), "0", serde_json::json!(0)),                                         // min u64
            (AnalysedType::U64(TypeU64), "18446744073709551615", serde_json::json!("18446744073709551615")), // max u64
            
            // Float types with special values
            (AnalysedType::F32(TypeF32), "0.0", serde_json::json!(0.0)),
            (AnalysedType::F32(TypeF32), "3.4028235e38", serde_json::json!("3.4028235e38")), // max f32
            (AnalysedType::F32(TypeF32), "-3.4028235e38", serde_json::json!("-3.4028235e38")), // min f32
            (AnalysedType::F64(TypeF64), "0.0", serde_json::json!(0.0)),
            (AnalysedType::F64(TypeF64), "1.7976931348623157e308", serde_json::json!("1.7976931348623157e308")), // max f64
            (AnalysedType::F64(TypeF64), "-1.7976931348623157e308", serde_json::json!("-1.7976931348623157e308")), // min f64
            
            // Other primitives with special cases
            (AnalysedType::Bool(TypeBool), "true", serde_json::json!(true)),
            (AnalysedType::Bool(TypeBool), "false", serde_json::json!(false)),
            (AnalysedType::Chr(TypeChr), "'a'", serde_json::json!("a")),
            (AnalysedType::Chr(TypeChr), "'\\n'", serde_json::json!("\n")), // escape sequence
            (AnalysedType::Chr(TypeChr), "'\\t'", serde_json::json!("\t")), // escape sequence
            (AnalysedType::Chr(TypeChr), "'\\''", serde_json::json!("'")), // escaped quote
            (AnalysedType::Str(TypeStr), "\"hello\"", serde_json::json!("hello")),
            (AnalysedType::Str(TypeStr), "\"\"", serde_json::json!("")), // empty string
            (AnalysedType::Str(TypeStr), "\"\\\"escaped\\\"\"", serde_json::json!("\"escaped\"")), // escaped quotes
            (AnalysedType::Str(TypeStr), "\"hello\\nworld\"", serde_json::json!("hello\nworld")), // newline
        ];

        // Test each primitive type
        for (typ, rib_value, expected) in primitive_test_cases {
            let schema = converter.convert_type(&typ).unwrap();
            let expr = rib::from_string(rib_value).unwrap();
            let exports: Vec<AnalysedExport> = vec![];
            let compiled = rib::compile_with_limited_globals(
                &expr,
                &exports,
                Some(vec!["request".to_string()]),
            ).unwrap();

            let rib_input = RibInput::default();
            let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
                Box::pin(async { 
                    Ok(ValueAndType::new(
                        Value::Option(None),
                        AnalysedType::Bool(TypeBool)
                    ))
                })
            });

            let result = tokio_test::block_on(async {
                rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
            }).unwrap();

            let literal = result.get_literal().unwrap();
            let json_value = match literal {
                LiteralValue::Bool(b) => serde_json::json!(b),
                LiteralValue::Num(n) => serde_json::json!(n.to_string()),
                LiteralValue::String(s) => serde_json::json!(s),
            };
            let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &typ).unwrap();
            let json_value = annotated_value.to_json_value();
            assert!(validate_json_against_schema(&json_value, &schema));
            assert_eq!(json_value, expected);

            // Test invalid values for each type
            let invalid_json = match &typ {
                AnalysedType::Bool(_) => serde_json::json!(42),
                AnalysedType::S8(_) | AnalysedType::U8(_) | AnalysedType::S16(_) | AnalysedType::U16(_) |
                AnalysedType::S32(_) | AnalysedType::U32(_) | AnalysedType::S64(_) | AnalysedType::U64(_) => 
                    serde_json::json!("not a number"),
                AnalysedType::F32(_) | AnalysedType::F64(_) => serde_json::json!("not a float"),
                AnalysedType::Chr(_) | AnalysedType::Str(_) => serde_json::json!(42),
                _ => continue,
            };
            assert!(!validate_json_against_schema(&invalid_json, &schema));
        }

        // Test complex nested types
        
        // Test 1: Deeply nested variants
        // Variant {
        //   Record {
        //     data: Option<Variant {
        //       List<Result<String, U32>>,
        //       Record { flags: Flags, value: U32 }
        //     }>,
        //     metadata: List<String>
        //   }
        // }
        let flags_type = AnalysedType::Flags(TypeFlags {
            names: vec!["A".to_string(), "B".to_string(), "C".to_string()],
        });

        let inner_record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "flags".to_string(),
                    typ: flags_type,
                },
                NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        });

        let inner_variant_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "ListCase".to_string(),
                    typ: Some(AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Result(TypeResult {
                            ok: Some(Box::new(AnalysedType::Str(TypeStr))),
                            err: Some(Box::new(AnalysedType::U32(TypeU32))),
                        })),
                    })),
                },
                NameOptionTypePair {
                    name: "RecordCase".to_string(),
                    typ: Some(inner_record_type),
                },
            ],
        });

        let outer_record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "data".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(inner_variant_type),
                    }),
                },
                NameTypePair {
                    name: "metadata".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    }),
                },
            ],
        });

        let complex_type = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Complex".to_string(),
                    typ: Some(outer_record_type),
                },
            ],
        });

        let schema = converter.convert_type(&complex_type).unwrap();

        // Test with both variant cases
        let test_scripts = vec![
            // Test ListCase
            (r#"Complex({
                data = Some(ListCase([Ok("success"), Err(404), Ok("another")])),
                metadata = ["info1", "info2"]
            })"#, true),
            // Test RecordCase
            (r#"Complex({
                data = Some(RecordCase({
                    flags = { A = true, B = false, C = true },
                    value = 42
                })),
                metadata = ["test"]
            })"#, true),
            // Test with None
            (r#"Complex({
                data = None,
                metadata = []
            })"#, true),
        ];

        for (script, should_validate) in test_scripts {
            let expr = rib::from_string(script).unwrap();
            let exports: Vec<AnalysedExport> = vec![];
            let compiled = rib::compile_with_limited_globals(
                &expr,
                &exports,
                Some(vec!["request".to_string()]),
            ).unwrap();

            let rib_input = RibInput::default();
            let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
                Box::pin(async { 
                    Ok(ValueAndType::new(
                        Value::Option(None),
                        AnalysedType::Bool(TypeBool)
                    ))
                })
            });

            let result = tokio_test::block_on(async {
                rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function.clone()).await
            }).unwrap();

            let literal = result.get_literal().unwrap();
            let json_value = match literal {
                LiteralValue::Bool(b) => serde_json::json!(b),
                LiteralValue::Num(n) => serde_json::json!(n.to_string()),
                LiteralValue::String(s) => serde_json::json!(s),
            };
            let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &complex_type).unwrap();
            let json_value = annotated_value.to_json_value();
            assert_eq!(validate_json_against_schema(&json_value, &schema), should_validate);

            // Verify the structure if it should validate
            if should_validate {
                assert_eq!(json_value["discriminator"], "Complex");
                assert!(json_value["value"]["Complex"]["metadata"].is_array());
                
                if let Some(data) = json_value["value"]["Complex"]["data"]["value"].as_object() {
                    match data.keys().next().unwrap().as_str() {
                        "ListCase" => {
                            let list = data["ListCase"].as_array().unwrap();
                            for item in list {
                                assert!(item.get("ok").is_some() || item.get("err").is_some());
                            }
                        },
                        "RecordCase" => {
                            let record = &data["RecordCase"];
                            assert!(record["flags"].is_object());
                            assert!(record["value"].is_number());
                        },
                        _ => panic!("Unexpected variant case"),
                    }
                }
            }
        }

        // Test invalid cases
        let invalid_scripts = vec![
            // Invalid flags
            (r#"Complex({
                data = Some(RecordCase({
                    flags = { A = true, B = "invalid", C = true },
                    value = 42
                })),
                metadata = ["test"]
            })"#, false),
            // Invalid result type
            (r#"Complex({
                data = Some(ListCase([Ok(42), Err("invalid")])),
                metadata = ["test"]
            })"#, false),
            // Missing required field
            (r#"Complex({
                data = Some(RecordCase({
                    flags = { A = true, B = false, C = true }
                })),
                metadata = ["test"]
            })"#, false),
        ];

        for (script, should_validate) in invalid_scripts {
            let expr = rib::from_string(script).unwrap();
            let exports: Vec<AnalysedExport> = vec![];
            let compiled = rib::compile_with_limited_globals(
                &expr,
                &exports,
                Some(vec!["request".to_string()]),
            ).unwrap();

            let rib_input = RibInput::default();
            let worker_invoke_function = std::sync::Arc::new(|_: String, _: Vec<ValueAndType>| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ValueAndType, String>> + Send>> {
                Box::pin(async { 
                    Ok(ValueAndType::new(
                        Value::Option(None),
                        AnalysedType::Bool(TypeBool)
                    ))
                })
            });

            let result = tokio_test::block_on(async {
                rib::interpret(&compiled.byte_code, &rib_input, worker_invoke_function).await
            }).unwrap();

            let literal = result.get_literal().unwrap();
            let json_value = match literal {
                LiteralValue::Bool(b) => serde_json::json!(b),
                LiteralValue::Num(n) => serde_json::json!(n.to_string()),
                LiteralValue::String(s) => serde_json::json!(s),
            };
            let annotated_value = TypeAnnotatedValue::parse_with_type(&json_value, &complex_type).unwrap();
            let json_value = annotated_value.to_json_value();
            assert_eq!(validate_json_against_schema(&json_value, &schema), should_validate);
        }
    }
} 