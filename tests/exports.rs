use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedInstance, AnalysedType, AnalysisContext,
};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;

#[test]
fn exports_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        funcs: vec![
            AnalysedFunction {
                name: "initialize-cart".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "add-item".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "item".to_string(),
                    typ: AnalysedType::Record(vec![
                        ("product-id".to_string(), AnalysedType::Str),
                        ("name".to_string(), AnalysedType::Str),
                        ("price".to_string(), AnalysedType::F32),
                        ("quantity".to_string(), AnalysedType::U32),
                    ]),
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "remove-item".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "product-id".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "update-item-quantity".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str,
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: AnalysedType::U32,
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "checkout".to_string(),
                params: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Variant(vec![
                        ("error".to_string(), Some(AnalysedType::Str)),
                        (
                            "success".to_string(),
                            Some(AnalysedType::Record(vec![(
                                "order-id".to_string(),
                                AnalysedType::Str,
                            )])),
                        ),
                    ]),
                }],
            },
            AnalysedFunction {
                name: "get-cart-contents".to_string(),
                params: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::List(Box::new(AnalysedType::Record(vec![
                        ("product-id".to_string(), AnalysedType::Str),
                        ("name".to_string(), AnalysedType::Str),
                        ("price".to_string(), AnalysedType::F32),
                        ("quantity".to_string(), AnalysedType::U32),
                    ]))),
                }],
            },
        ],
    })];

    pretty_assertions::assert_eq!(metadata, expected);
}

#[test]
fn exports_file_service_component() {
    let source_bytes = include_bytes!("../wasm/file-service.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        funcs: vec![
            AnalysedFunction {
                name: "read-file".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: Some(Box::new(AnalysedType::Str)),
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "write-file".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "path".to_string(),
                        typ: AnalysedType::Str,
                    },
                    AnalysedFunctionParameter {
                        name: "contents".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "write-file-direct".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "path".to_string(),
                        typ: AnalysedType::Str,
                    },
                    AnalysedFunctionParameter {
                        name: "contents".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "delete-file".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "get-file-info".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: Some(Box::new(AnalysedType::Record(vec![
                            (
                                "last-modified".to_string(),
                                AnalysedType::Record(vec![
                                    ("seconds".to_string(), AnalysedType::U64),
                                    ("nanoseconds".to_string(), AnalysedType::U32),
                                ]),
                            ),
                            (
                                "last-accessed".to_string(),
                                AnalysedType::Record(vec![
                                    ("seconds".to_string(), AnalysedType::U64),
                                    ("nanoseconds".to_string(), AnalysedType::U32),
                                ]),
                            ),
                        ]))),
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "get-info".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: Some(Box::new(AnalysedType::Record(vec![
                            (
                                "last-modified".to_string(),
                                AnalysedType::Record(vec![
                                    ("seconds".to_string(), AnalysedType::U64),
                                    ("nanoseconds".to_string(), AnalysedType::U32),
                                ]),
                            ),
                            (
                                "last-accessed".to_string(),
                                AnalysedType::Record(vec![
                                    ("seconds".to_string(), AnalysedType::U64),
                                    ("nanoseconds".to_string(), AnalysedType::U32),
                                ]),
                            ),
                        ]))),
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "create-directory".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "create-link".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: AnalysedType::Str,
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "create-sym-link".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: AnalysedType::Str,
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "remove-directory".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "remove-file".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "rename-file".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: AnalysedType::Str,
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
            AnalysedFunction {
                name: "hash".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result {
                        ok: Some(Box::new(AnalysedType::Record(vec![
                            ("lower".to_string(), AnalysedType::U64),
                            ("upper".to_string(), AnalysedType::U64),
                        ]))),
                        error: Some(Box::new(AnalysedType::Str)),
                    },
                }],
            },
        ],
    })];

    pretty_assertions::assert_eq!(metadata, expected);
}
