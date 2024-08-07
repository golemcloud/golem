use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType, AnalysisContext, NameOptionTypePair, NameTypePair, TypeF32, TypeHandle, TypeList, TypeRecord, TypeResult, TypeStr, TypeTuple, TypeU32, TypeU64, TypeVariant};
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
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "add-item".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "item".to_string(),
                    typ: AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair { name: "product-id".to_string(), typ: AnalysedType::Str(TypeStr) },
                            NameTypePair { name: "name".to_string(), typ: AnalysedType::Str(TypeStr) },
                            NameTypePair { name: "price".to_string(), typ: AnalysedType::F32(TypeF32) },
                            NameTypePair { name: "quantity".to_string(), typ: AnalysedType::U32(TypeU32) },
                        ]
                    }),
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "remove-item".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "product-id".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "update-item-quantity".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "checkout".to_string(),
                params: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Variant(TypeVariant {
                        cases: vec![
                            NameOptionTypePair { name: "error".to_string(), typ: Some(AnalysedType::Str(TypeStr)) },
                            NameOptionTypePair {
                                name: "success".to_string(),
                                typ:
                                Some(AnalysedType::Record(TypeRecord {
                                    fields: vec![
                                        NameTypePair { name: "order-id".to_string(), typ: AnalysedType::Str(TypeStr) },
                                    ]
                                })),
                            },
                        ]
                    }),
                }],
            },
            AnalysedFunction {
                name: "get-cart-contents".to_string(),
                params: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair { name: "product-id".to_string(), typ: AnalysedType::Str(TypeStr) },
                                NameTypePair { name: "name".to_string(), typ: AnalysedType::Str(TypeStr) },
                                NameTypePair { name: "price".to_string(), typ: AnalysedType::F32(TypeF32) },
                                NameTypePair { name: "quantity".to_string(), typ: AnalysedType::U32(TypeU32) },
                            ]
                        }))
                    }),
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
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: Some(Box::new(AnalysedType::Str(TypeStr))),
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "write-file".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "path".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "contents".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "write-file-direct".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "path".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "contents".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "delete-file".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "get-file-info".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "last-modified".to_string(),
                                    typ: AnalysedType::Record(TypeRecord {
                                        fields: vec![
                                            NameTypePair { name: "seconds".to_string(), typ: AnalysedType::U64(TypeU64) },
                                            NameTypePair { name: "nanoseconds".to_string(), typ: AnalysedType::U32(TypeU32) },
                                        ]
                                    }),
                                },
                                NameTypePair {
                                    name: "last-accessed".to_string(),
                                    typ: AnalysedType::Record(TypeRecord {
                                        fields: vec![
                                            NameTypePair {
                                                name: "seconds".to_string(),
                                                typ: AnalysedType::U64(TypeU64),
                                            },
                                            NameTypePair { name: "nanoseconds".to_string(), typ: AnalysedType::U32(TypeU32) },
                                        ]
                                    }),
                                },
                            ]
                        }))),
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "get-info".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "last-modified".to_string(),
                                    typ: AnalysedType::Record(TypeRecord {
                                        fields: vec![
                                            NameTypePair { name: "seconds".to_string(), typ: AnalysedType::U64(TypeU64) },
                                            NameTypePair { name: "nanoseconds".to_string(), typ: AnalysedType::U32(TypeU32) },
                                        ]
                                    }),
                                },
                                NameTypePair {
                                    name: "last-accessed".to_string(),
                                    typ: AnalysedType::Record(TypeRecord {
                                        fields: vec![
                                            NameTypePair { name: "seconds".to_string(), typ: AnalysedType::U64(TypeU64) },
                                            NameTypePair { name: "nanoseconds".to_string(), typ: AnalysedType::U32(TypeU32) },
                                        ]
                                    }),
                                },
                            ]
                        }))),
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "create-directory".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "create-link".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "create-sym-link".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "remove-directory".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "remove-file".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "rename-file".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
                }],
            },
            AnalysedFunction {
                name: "hash".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Result(TypeResult {
                        ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair { name: "lower".to_string(), typ: AnalysedType::U64(TypeU64) },
                                NameTypePair { name: "upper".to_string(), typ: AnalysedType::U64(TypeU64) },
                            ]
                        }))),
                        err: Some(Box::new(AnalysedType::Str(TypeStr))),
                    }),
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
                            typ: AnalysedType::Str(TypeStr),
                        },
                        AnalysedFunctionParameter {
                            name: "address".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![(
                                NameTypePair { name: "bidder-id".to_string(), typ: AnalysedType::Str(TypeStr) }
                            )]
                        }),
                    }],
                },
                AnalysedFunction {
                    name: "create-auction".to_string(),
                    params: vec![
                        AnalysedFunctionParameter {
                            name: "name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        AnalysedFunctionParameter {
                            name: "description".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        AnalysedFunctionParameter {
                            name: "limit-price".to_string(),
                            typ: AnalysedType::F32(TypeF32),
                        },
                        AnalysedFunctionParameter {
                            name: "expiration".to_string(),
                            typ: AnalysedType::U64(TypeU64),
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "auction-id".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                }
                            ]
                        }),
                    }],
                },
                AnalysedFunction {
                    name: "get-auctions".to_string(),
                    params: vec![],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::List(TypeList {
                            inner: Box::new(AnalysedType::Record(TypeRecord {
                                fields: vec![
                                    NameTypePair {
                                        name: "auction-id".to_string(),
                                        typ: AnalysedType::Record(TypeRecord {
                                            fields: vec![
                                                NameTypePair {
                                                    name: "auction-id".to_string(),
                                                    typ: AnalysedType::Str(TypeStr),
                                                }
                                            ]
                                        }),
                                    },
                                    NameTypePair {
                                        name: "name".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    },
                                    NameTypePair {
                                        name: "description".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    },
                                    NameTypePair {
                                        name: "limit-price".to_string(),
                                        typ: AnalysedType::F32(TypeF32),
                                    },
                                    NameTypePair {
                                        name: "expiration".to_string(),
                                        typ: AnalysedType::U64(TypeU64),
                                    },
                                ],
                            }))
                        }),
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
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: AnalysedResourceId(0),
                        mode: AnalysedResourceMode::Owned,
                    }),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: AnalysedType::Record(
                            TypeRecord {
                                fields: vec![
                                    NameTypePair {
                                        name: "product-id".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    },
                                    NameTypePair {
                                        name: "name".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    },
                                    NameTypePair {
                                        name: "price".to_string(),
                                        typ: AnalysedType::F32(TypeF32),
                                    },
                                    NameTypePair {
                                        name: "quantity".to_string(),
                                        typ: AnalysedType::U32(TypeU32),
                                    },
                                ],
                            },
                        ),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.checkout".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: AnalysedResourceId(0),
                        mode: AnalysedResourceMode::Borrowed,
                    }),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Variant(TypeVariant {
                        cases: vec![
                            NameOptionTypePair {
                                name: "error".to_string(),
                                typ: Some(AnalysedType::Str(TypeStr)),
                            },
                            NameOptionTypePair {
                                name: "success".to_string(),
                                typ: Some(AnalysedType::Record(TypeRecord {
                                    fields: vec![
                                        NameTypePair {
                                            name: "order-id".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        },
                                    ],
                                })),
                            },
                        ]
                    }),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.get-cart-contents".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: AnalysedResourceId(0),
                        mode: AnalysedResourceMode::Borrowed,
                    }),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "product-id".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                },
                                NameTypePair {
                                    name: "price".to_string(),
                                    typ: AnalysedType::F32(TypeF32),
                                },
                                NameTypePair {
                                    name: "quantity".to_string(),
                                    typ: AnalysedType::U32(TypeU32),
                                },
                            ],
                        }))
                    }),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
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
                    typ: AnalysedType::Str(TypeStr),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: AnalysedResourceId(0),
                        mode: AnalysedResourceMode::Owned,
                    }),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "product-id".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                },
                                NameTypePair {
                                    name: "price".to_string(),
                                    typ: AnalysedType::F32(TypeF32),
                                },
                                NameTypePair {
                                    name: "quantity".to_string(),
                                    typ: AnalysedType::U32(TypeU32),
                                },
                            ],
                        }, ),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.checkout".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: AnalysedResourceId(0),
                        mode: AnalysedResourceMode::Borrowed,
                    }),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::Variant(TypeVariant {
                        cases: vec![
                            NameOptionTypePair {
                                name: "error".to_string(),
                                typ: Some(AnalysedType::Str(TypeStr)),
                            },
                            NameOptionTypePair {
                                name: "success".to_string(),
                                typ: Some(AnalysedType::Record(TypeRecord {
                                    fields: vec![
                                        NameTypePair {
                                            name: "order-id".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        },
                                    ],
                                })),
                            },
                        ]
                    }),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.get-cart-contents".to_string(),
                params: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: AnalysedResourceId(0),
                        mode: AnalysedResourceMode::Borrowed,
                    }),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "product-id".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Str(TypeStr),
                                },
                                NameTypePair {
                                    name: "price".to_string(),
                                    typ: AnalysedType::F32(TypeF32),
                                },
                                NameTypePair {
                                    name: "quantity".to_string(),
                                    typ: AnalysedType::U32(TypeU32),
                                },
                            ],
                        }))
                    }),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                params: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Borrowed,
                        }),
                    },
                ],
                results: vec![],
            },
        ],
    })];

    pretty_assertions::assert_eq!(metadata, expected);
}

#[test]
fn exports_caller_composed_component() {
    let source_bytes = include_bytes!("../wasm/caller_composed.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![
        AnalysedExport::Function(AnalysedFunction {
            name: "test1".to_string(),
            params: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Tuple(TypeTuple {
                        items: vec![
                            AnalysedType::Str(TypeStr),
                            AnalysedType::U64(TypeU64),
                        ]
                    }))
                }),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test2".to_string(),
            params: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::U64(TypeU64),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test3".to_string(),
            params: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::U64(TypeU64),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test4".to_string(),
            params: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::Tuple(TypeTuple {
                    items: vec![
                        AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                        AnalysedType::List(TypeList {
                            inner: Box::new(AnalysedType::Tuple(TypeTuple {
                                items: vec![
                                    AnalysedType::Str(TypeStr),
                                    AnalysedType::Str(TypeStr),
                                ]
                            }))
                        }),
                    ]
                }),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test5".to_string(),
            params: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::U64(TypeU64)) }),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "bug-wasm-rpc-i32".to_string(),
            params: vec![AnalysedFunctionParameter {
                name: "in".to_string(),
                typ: AnalysedType::Variant(TypeVariant { cases: vec![NameOptionTypePair { name: "leaf".to_string(), typ: None }] }),
            }],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::Variant(TypeVariant { cases: vec![NameOptionTypePair { name: "leaf".to_string(), typ: None }] }),
            }],
        }),
    ];

    pretty_assertions::assert_eq!(metadata, expected);
}
