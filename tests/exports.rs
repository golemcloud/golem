use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType, AnalysisContext,
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

#[test]
fn exports_auction_registry_composed_component() {
    let source_bytes = include_bytes!("../wasm/auction_registry_composed.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    println!("{:?}", metadata);
    // [Instance(AnalysedInstance { name: "auction:registry/api", funcs: [AnalysedFunction { name: "create-bidder", params: [AnalysedFunctionParameter { name: "name", typ: Str }, AnalysedFunctionParameter { name: "address", typ: Str }], results: [AnalysedFunctionResult { name: None, typ: Record([("bidder-id", Str)]) }] }, AnalysedFunction { name: "create-auction", params: [AnalysedFunctionParameter { name: "name", typ: Str }, AnalysedFunctionParameter { name: "description", typ: Str }, AnalysedFunctionParameter { name: "limit-price", typ: F32 }, AnalysedFunctionParameter { name: "expiration", typ: U64 }], results: [AnalysedFunctionResult { name: None, typ: Record([("auction-id", Str)]) }] }, AnalysedFunction { name: "get-auctions", params: [], results: [AnalysedFunctionResult { name: None, typ: List(Record([("auction-id", Record([("auction-id", Str)])), ("name", Str), ("description", Str), ("limit-price", F32), ("expiration", U64)])) }] }] })]

    pretty_assertions::assert_eq!(
        metadata,
        vec![AnalysedExport::Instance(AnalysedInstance {
            name: "auction:registry/api".to_string(),
            funcs: vec![
                AnalysedFunction {
                    name: "create-bidder".to_string(),
                    params: vec![
                        AnalysedFunctionParameter {
                            name: "name".to_string(),
                            typ: AnalysedType::Str,
                        },
                        AnalysedFunctionParameter {
                            name: "address".to_string(),
                            typ: AnalysedType::Str,
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::Record(vec![(
                            "bidder-id".to_string(),
                            AnalysedType::Str
                        )]),
                    }],
                },
                AnalysedFunction {
                    name: "create-auction".to_string(),
                    params: vec![
                        AnalysedFunctionParameter {
                            name: "name".to_string(),
                            typ: AnalysedType::Str,
                        },
                        AnalysedFunctionParameter {
                            name: "description".to_string(),
                            typ: AnalysedType::Str,
                        },
                        AnalysedFunctionParameter {
                            name: "limit-price".to_string(),
                            typ: AnalysedType::F32,
                        },
                        AnalysedFunctionParameter {
                            name: "expiration".to_string(),
                            typ: AnalysedType::U64,
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::Record(vec![(
                            "auction-id".to_string(),
                            AnalysedType::Str
                        )]),
                    }],
                },
                AnalysedFunction {
                    name: "get-auctions".to_string(),
                    params: vec![],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::List(Box::new(AnalysedType::Record(vec![
                            (
                                "auction-id".to_string(),
                                AnalysedType::Record(vec![(
                                    "auction-id".to_string(),
                                    AnalysedType::Str
                                )])
                            ),
                            ("name".to_string(), AnalysedType::Str),
                            ("description".to_string(), AnalysedType::Str),
                            ("limit-price".to_string(), AnalysedType::F32),
                            ("expiration".to_string(), AnalysedType::U64),
                        ]))),
                    }],
                },
            ],
        })]
    );
}

#[test]
fn exports_shopping_cart_resource_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        funcs: vec![
            AnalysedFunction {
                name: "[constructor]cart".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Resource {
                        id: AnalysedResourceId { value: 0 },
                        resource_mode: AnalysedResourceMode::Owned,
                    },
                }],
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: AnalysedType::Record(vec![
                            ("product-id".to_string(), AnalysedType::Str),
                            ("name".to_string(), AnalysedType::Str),
                            ("price".to_string(), AnalysedType::F32),
                            ("quantity".to_string(), AnalysedType::U32),
                        ]),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
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
                name: "[method]cart.checkout".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Resource {
                        id: AnalysedResourceId { value: 0 },
                        resource_mode: AnalysedResourceMode::Borrowed,
                    },
                }],
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
                name: "[method]cart.get-cart-contents".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Resource {
                        id: AnalysedResourceId { value: 0 },
                        resource_mode: AnalysedResourceMode::Borrowed,
                    },
                }],
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
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                ],
                results: vec![],
            },
        ],
    })];

    pretty_assertions::assert_eq!(metadata, expected);
}

#[test]
fn exports_shopping_cart_resource_versioned_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource-versioned.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api@1.2.3".to_string(),
        funcs: vec![
            AnalysedFunction {
                name: "[constructor]cart".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: AnalysedType::Str,
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Resource {
                        id: AnalysedResourceId { value: 0 },
                        resource_mode: AnalysedResourceMode::Owned,
                    },
                }],
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: AnalysedType::Record(vec![
                            ("product-id".to_string(), AnalysedType::Str),
                            ("name".to_string(), AnalysedType::Str),
                            ("price".to_string(), AnalysedType::F32),
                            ("quantity".to_string(), AnalysedType::U32),
                        ]),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str,
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
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
                name: "[method]cart.checkout".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Resource {
                        id: AnalysedResourceId { value: 0 },
                        resource_mode: AnalysedResourceMode::Borrowed,
                    },
                }],
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
                name: "[method]cart.get-cart-contents".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Resource {
                        id: AnalysedResourceId { value: 0 },
                        resource_mode: AnalysedResourceMode::Borrowed,
                    },
                }],
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
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: AnalysedType::Resource {
                            id: AnalysedResourceId { value: 0 },
                            resource_mode: AnalysedResourceMode::Borrowed,
                        },
                    },
                ],
                results: vec![],
            },
        ],
    })];

    pretty_assertions::assert_eq!(metadata, expected);
}
