use golem_wasm_ast::analysis::analysed_type::{
    case, f32, field, handle, list, record, result, result_err, str, tuple, u32, u64, unit_case,
    variant,
};
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysisContext,
};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use test_r::test;

test_r::enable!();

#[test]
fn exports_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![AnalysedExport::Instance(AnalysedInstance {
        name: "golem:it/api".to_string(),
        functions: vec![
            AnalysedFunction {
                name: "initialize-cart".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: str(),
                }],
                results: vec![],
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
                    ]),
                }],
                results: vec![],
            },
            AnalysedFunction {
                name: "remove-item".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "product-id".to_string(),
                    typ: str(),
                }],
                results: vec![],
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
                results: vec![],
            },
            AnalysedFunction {
                name: "checkout".to_string(),
                parameters: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: variant(vec![
                        case("error", str()),
                        case("success", record(vec![field("order-id", str())])),
                    ]),
                }],
            },
            AnalysedFunction {
                name: "get-cart-contents".to_string(),
                parameters: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: list(record(vec![
                        field("product-id", str()),
                        field("name", str()),
                        field("price", f32()),
                        field("quantity", u32()),
                    ])),
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
        functions: vec![
            AnalysedFunction {
                name: "read-file".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result(str(), str()),
                }],
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
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
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
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
            },
            AnalysedFunction {
                name: "delete-file".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
            },
            AnalysedFunction {
                name: "get-file-info".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result(
                        record(vec![
                            field(
                                "last-modified",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())]),
                            ),
                            field(
                                "last-accessed",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())]),
                            ),
                        ]),
                        str(),
                    ),
                }],
            },
            AnalysedFunction {
                name: "get-info".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result(
                        record(vec![
                            field(
                                "last-modified",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())]),
                            ),
                            field(
                                "last-accessed",
                                record(vec![field("seconds", u64()), field("nanoseconds", u32())]),
                            ),
                        ]),
                        str(),
                    ),
                }],
            },
            AnalysedFunction {
                name: "create-directory".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
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
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
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
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
            },
            AnalysedFunction {
                name: "remove-directory".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
            },
            AnalysedFunction {
                name: "remove-file".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
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
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result_err(str()),
                }],
            },
            AnalysedFunction {
                name: "hash".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "path".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: result(
                        record(vec![field("lower", u64()), field("upper", u64())]),
                        str(),
                    ),
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
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: record(vec![field("bidder-id", str())]),
                    }],
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
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: record(vec![field("auction-id", str())]),
                    }],
                },
                AnalysedFunction {
                    name: "get-auctions".to_string(),
                    parameters: vec![],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: list(record(vec![
                            field("auction-id", record(vec![field("auction-id", str())])),
                            field("name", str()),
                            field("description", str()),
                            field("limit-price", f32()),
                            field("expiration", u64()),
                        ],))
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
        functions: vec![
            AnalysedFunction {
                name: "[constructor]cart".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ]),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
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
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.checkout".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: variant(vec![
                        case("error", str()),
                        case("success", record(vec![field("order-id", str())])),
                    ]),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.get-cart-contents".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: list(record(vec![
                        field("product-id", str()),
                        field("name", str()),
                        field("price", f32()),
                        field("quantity", u32()),
                    ])),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
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
        functions: vec![
            AnalysedFunction {
                name: "[constructor]cart".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "user-id".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.add-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    },
                    AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ]),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.remove-item".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    },
                    AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                ],
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.update-item-quantity".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
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
                results: vec![],
            },
            AnalysedFunction {
                name: "[method]cart.checkout".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: variant(vec![
                        case("error", str()),
                        case("success", record(vec![field("order-id", str())])),
                    ]),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.get-cart-contents".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: list(record(vec![
                        field("product-id", str()),
                        field("name", str()),
                        field("price", f32()),
                        field("quantity", u32()),
                    ])),
                }],
            },
            AnalysedFunction {
                name: "[method]cart.merge-with".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    },
                    AnalysedFunctionParameter {
                        name: "other-cart".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
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
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: list(tuple(vec![str(), u64()])),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test2".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: u64(),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test3".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: u64(),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test4".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: tuple(vec![list(str()), list(tuple(vec![str(), str()]))]),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test5".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: list(u64()),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "bug-wasm-rpc-i32".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "in".to_string(),
                typ: variant(vec![unit_case("leaf")]),
            }],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: variant(vec![unit_case("leaf")]),
            }],
        }),
    ];

    pretty_assertions::assert_eq!(metadata, expected);
}

#[test]
fn exports_caller_component() {
    // NOTE: Same as caller_composed.wasm but not composed with the generated stub
    let source_bytes = include_bytes!("../wasm/caller.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();

    let state = AnalysisContext::new(component);
    let metadata = state.get_top_level_exports().unwrap();

    let expected = vec![
        AnalysedExport::Function(AnalysedFunction {
            name: "test1".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: list(tuple(vec![str(), u64()])),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test2".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: u64(),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test3".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: u64(),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test4".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: tuple(vec![list(str()), list(tuple(vec![str(), str()]))]),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "test5".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: list(u64()),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "bug-wasm-rpc-i32".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "in".to_string(),
                typ: variant(vec![unit_case("leaf")]),
            }],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: variant(vec![unit_case("leaf")]),
            }],
        }),
        AnalysedExport::Function(AnalysedFunction {
            name: "ephemeral-test1".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: list(tuple(vec![str(), str()])),
            }],
        }),
    ];

    pretty_assertions::assert_eq!(metadata, expected);
}
