use golem_wasm_ast::analysis::analysed_type::{
    case, f32, field, handle, list, record, result, result_err, str, tuple, u32, u64, unit_case,
    variant,
};
use golem_wasm_ast::analysis::wit_parser::WitAnalysisContext;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedInstance, AnalysedResourceId, AnalysedResourceMode,
};
use test_r::test;

test_r::enable!();

#[test]
fn exports_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");

    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        functions: vec![
            AnalysedFunction {
                name: "initialize-cart".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: str(),
                }],
                result: None,
            },
            AnalysedFunction {
                name: "add-item".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "item".to_string(),
                    typ: record(vec![
                        field("product-id", str()),
                        field("name", str()),
                        field("price", f32()),
                        field("quantity", u32()),
                    ])
                    .named("product-item")
                    .owned("golem:it/api"),
                }],
                result: None,
            },
            AnalysedFunction {
                name: "remove-item".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "product-id".to_string(),
                    typ: str(),
                }],
                result: None,
            },
            AnalysedFunction {
                name: "update-item-quantity".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: u32(),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "checkout".to_string(),
                parameters: vec![],
                result: Some(AnalysedFunctionResult {
                    typ: variant(vec![
                        case("error", str()),
                        case(
                            "success",
                            record(vec![field("order-id", str())])
                                .named("order-confirmation")
                                .owned("golem:it/api"),
                        ),
                    ])
                    .named("checkout-result")
                    .owned("golem:it/api"),
                }),
            },
            AnalysedFunction {
                name: "get-cart-contents".to_string(),
                parameters: vec![],
                result: Some(AnalysedFunctionResult {
                    typ: list(
                        record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])
                        .named("product-item")
                        .owned("golem:it/api"),
                    ),
                }),
            },
        ],
    })];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}

#[test]
fn exports_file_service_component() {
    let source_bytes = include_bytes!("../wasm/file-service.wasm");

    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        functions: vec![
            AnalysedFunction {
                name: "read-file".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result(str(), str()),
                }),
            },
            AnalysedFunction {
                name: "write-file".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "path".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "contents".to_string(),
                        typ: str(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "write-file-direct".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "path".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "contents".to_string(),
                        typ: str(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "delete-file".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "get-file-info".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result(
                        record(vec![
                            field(
                                "last-modified",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())])
                                    .named("datetime")
                                    .owned("wasi:clocks@0.2.0/wall-clock"),
                            ),
                            field(
                                "last-accessed",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())])
                                    .named("datetime")
                                    .owned("wasi:clocks@0.2.0/wall-clock"),
                            ),
                        ])
                        .named("file-info")
                        .owned("golem:it/api"),
                        str(),
                    ),
                }),
            },
            AnalysedFunction {
                name: "get-info".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result(
                        record(vec![
                            field(
                                "last-modified",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())])
                                    .named("datetime")
                                    .owned("wasi:clocks@0.2.0/wall-clock"),
                            ),
                            field(
                                "last-accessed",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())])
                                    .named("datetime")
                                    .owned("wasi:clocks@0.2.0/wall-clock"),
                            ),
                        ])
                        .named("file-info")
                        .owned("golem:it/api"),
                        str(),
                    ),
                }),
            },
            AnalysedFunction {
                name: "create-directory".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "create-link".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: str(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "create-sym-link".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: str(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "remove-directory".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "remove-file".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "rename-file".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "source".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "destination".to_string(),
                        typ: str(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: result_err(str()),
                }),
            },
            AnalysedFunction {
                name: "hash".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: result(
                        record(vec![field("lower", u64()), field("upper", u64())])
                            .named("metadata-hash-value")
                            .owned("wasi:filesystem@0.2.0/types"),
                        str(),
                    ),
                }),
            },
        ],
    })];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}

#[test]
fn exports_auction_registry_composed_component() {
    let source_bytes = include_bytes!("../wasm/auction_registry_composed.wasm");

    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "auction:registry/api".to_string(),
        functions: vec![
            AnalysedFunction {
                name: "create-bidder".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "name".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "address".to_string(),
                        typ: str(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: record(vec![field("bidder-id", str())])
                        .named("bidder-id")
                        .owned("auction:auction/api"),
                }),
            },
            AnalysedFunction {
                name: "create-auction".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "name".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "description".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "limit-price".to_string(),
                        typ: f32(),
                    },
                    AnalysedFunctionParameter {
                        name: "expiration".to_string(),
                        typ: u64(),
                    },
                ],
                result: Some(AnalysedFunctionResult {
                    typ: record(vec![field("auction-id", str())])
                        .named("auction-id")
                        .owned("auction:auction/api"),
                }),
            },
            AnalysedFunction {
                name: "get-auctions".to_string(),
                parameters: vec![],
                result: Some(AnalysedFunctionResult {
                    typ: list(
                        record(vec![
                            field(
                                "auction-id",
                                record(vec![field("auction-id", str())])
                                    .named("auction-id")
                                    .owned("auction:auction/api"),
                            ),
                            field("name", str()),
                            field("description", str()),
                            field("limit-price", f32()),
                            field("expiration", u64()),
                        ])
                        .named("auction")
                        .owned("auction:auction/api"),
                    ),
                }),
            },
        ],
    })];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}

#[test]
fn exports_shopping_cart_resource_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource.wasm");
    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        functions: vec![
            AnalysedFunction {
                name: "[constructor]cart".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned)
                        .named("cart")
                        .owned("golem:it/api"),
                }),
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])
                        .named("product-item")
                        .owned("golem:it/api"),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: u32(),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "[method]cart.checkout".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                        .named("cart")
                        .owned("golem:it/api"),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: variant(vec![
                        case("error", str()),
                        case(
                            "success",
                            record(vec![field("order-id", str())])
                                .named("order-confirmation")
                                .owned("golem:it/api"),
                        ),
                    ])
                    .named("checkout-result")
                    .owned("golem:it/api"),
                }),
            },
            AnalysedFunction {
                name: "[method]cart.get-cart-contents".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                        .named("cart")
                        .owned("golem:it/api"),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: list(
                        record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])
                        .named("product-item")
                        .owned("golem:it/api"),
                    ),
                }),
            },
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it/api"),
                    },
                ],
                result: None,
            },
        ],
    })];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}

#[test]
fn exports_shopping_cart_resource_versioned_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource-versioned.wasm");
    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api@1.2.3".to_string(),
        functions: vec![
            AnalysedFunction {
                name: "[constructor]cart".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned)
                        .named("cart")
                        .owned("golem:it@1.2.3/api"),
                }),
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it@1.2.3/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])
                        .named("product-item")
                        .owned("golem:it@1.2.3/api"),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it@1.2.3/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it@1.2.3/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "quantity".to_string(),
                        typ: u32(),
                    },
                ],
                result: None,
            },
            AnalysedFunction {
                name: "[method]cart.checkout".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                        .named("cart")
                        .owned("golem:it@1.2.3/api"),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: variant(vec![
                        case("error", str()),
                        case(
                            "success",
                            record(vec![field("order-id", str())])
                                .named("order-confirmation")
                                .owned("golem:it@1.2.3/api"),
                        ),
                    ])
                    .named("checkout-result")
                    .owned("golem:it@1.2.3/api"),
                }),
            },
            AnalysedFunction {
                name: "[method]cart.get-cart-contents".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                        .named("cart")
                        .owned("golem:it@1.2.3/api"),
                }],
                result: Some(AnalysedFunctionResult {
                    typ: list(
                        record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])
                        .named("product-item")
                        .owned("golem:it@1.2.3/api"),
                    ),
                }),
            },
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it@1.2.3/api"),
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed)
                            .named("cart")
                            .owned("golem:it@1.2.3/api"),
                    },
                ],
                result: None,
            },
        ],
    })];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}

#[test]
fn exports_caller_composed_component() {
    let source_bytes = include_bytes!("../wasm/caller_composed.wasm");
    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![
        AnalysedExport::Function(AnalysedFunction {
            name: "test1".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: list(tuple(vec![str(), u64()])),
            }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test2".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: u64() }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test3".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: u64() }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test4".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: tuple(vec![list(str()), list(tuple(vec![str(), str()]))]),
            }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test5".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: list(u64()) }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "bug-wasm-rpc-i32".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "in".to_string(),
                typ: variant(vec![unit_case("leaf")])
                    .named("timeline-node")
                    .owned("root"),
            }],
            result: Some(AnalysedFunctionResult {
                typ: variant(vec![unit_case("leaf")])
                    .named("timeline-node")
                    .owned("root"),
            }),
        }),
    ];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}

#[test]
fn exports_caller_component() {
    // NOTE: Same as caller_composed.wasm but not composed with the generated stub
    let source_bytes = include_bytes!("../wasm/caller.wasm");
    let wit_parser_metadata = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let expected = vec![
        AnalysedExport::Function(AnalysedFunction {
            name: "test1".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: list(tuple(vec![str(), u64()])),
            }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test2".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: u64() }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test3".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: u64() }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test4".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: tuple(vec![list(str()), list(tuple(vec![str(), str()]))]),
            }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test5".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: list(u64()) }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "bug-wasm-rpc-i32".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "in".to_string(),
                typ: variant(vec![unit_case("leaf")])
                    .named("timeline-node")
                    .owned("rpc:counters/api"),
            }],
            result: Some(AnalysedFunctionResult {
                typ: variant(vec![unit_case("leaf")])
                    .named("timeline-node")
                    .owned("rpc:counters/api"),
            }),
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "ephemeral-test1".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: list(tuple(vec![str(), str()])),
            }),
        }),
    ];

    pretty_assertions::assert_eq!(wit_parser_metadata, expected);
}
