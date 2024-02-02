use golem_wasm_ast::component::Component;
use golem_wasm_ast::metadata::{Metadata, Producers, ProducersField, VersionedName};
use golem_wasm_ast::IgnoreAllButMetadata;
use pretty_assertions::assert_eq;

#[test]
fn metadata_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();
    let metadata = component.get_metadata().unwrap();

    let expected = Metadata {
        name: None,
        producers: Some(Producers {
            fields: vec![ProducersField {
                name: "processed-by".to_string(),
                values: vec![
                    VersionedName {
                        name: "wit-component".to_string(),
                        version: "0.20.0".to_string(),
                    },
                    VersionedName {
                        name: "cargo-component".to_string(),
                        version: "0.7.0 (wasi:ab5a448)".to_string(),
                    },
                ],
            }],
        }),
        registry_metadata: None,
    };

    assert_eq!(metadata, expected);

    let inner_module_metadata = component.modules().first().unwrap().get_metadata().unwrap();

    let expected = Metadata {
        name: None,
        producers: Some(Producers {
            fields: vec![
                ProducersField {
                    name: "language".to_string(),
                    values: vec![VersionedName {
                        name: "Rust".to_string(),
                        version: "".to_string(),
                    }],
                },
                ProducersField {
                    name: "processed-by".to_string(),
                    values: vec![
                        VersionedName {
                            name: "rustc".to_string(),
                            version: "1.75.0 (82e1608df 2023-12-21)".to_string(),
                        },
                        VersionedName {
                            name: "clang".to_string(),
                            version: "16.0.4 (https://github.com/llvm/llvm-project ae42196bc493ffe877a7e3dff8be32035dea4d07)".to_string(),
                        },
                        VersionedName {
                            name: "wit-component".to_string(),
                            version: "0.18.2".to_string(),
                        },
                        VersionedName {
                            name: "wit-bindgen-rust".to_string(),
                            version: "0.16.0".to_string(),
                        },
                    ],
                },
            ],
        }),
        registry_metadata: None,
    };

    assert_eq!(inner_module_metadata, expected);
}

#[test]
fn get_all_producers_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
    let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();
    let producers = component.get_all_producers();

    let expected = vec![
        Producers {
            fields: vec![ProducersField {
                name: "processed-by".to_string(),
                values: vec![
                    VersionedName {
                        name: "wit-component".to_string(),
                        version: "0.20.0".to_string(),
                    },
                    VersionedName {
                        name: "cargo-component".to_string(),
                        version: "0.7.0 (wasi:ab5a448)".to_string(),
                    },
                ],
            }],
        },
        Producers {
            fields: vec![
                ProducersField {
                    name: "language".to_string(),
                    values: vec![VersionedName {
                        name: "Rust".to_string(),
                        version: "".to_string(),
                    }],
                },
                ProducersField {
                    name: "processed-by".to_string(),
                    values: vec![
                        VersionedName {
                            name: "rustc".to_string(),
                            version: "1.75.0 (82e1608df 2023-12-21)".to_string(),
                        },
                        VersionedName {
                            name: "clang".to_string(),
                            version: "16.0.4 (https://github.com/llvm/llvm-project ae42196bc493ffe877a7e3dff8be32035dea4d07)".to_string(),
                        },
                        VersionedName {
                            name: "wit-component".to_string(),
                            version: "0.18.2".to_string(),
                        },
                        VersionedName {
                            name: "wit-bindgen-rust".to_string(),
                            version: "0.16.0".to_string(),
                        },
                    ],
                },
            ],
        },
        Producers {
            fields: vec![
                ProducersField {
                    name: "language".to_string(),
                    values: vec![VersionedName {
                        name: "Rust".to_string(),
                        version: "".to_string(),
                    }],
                },
                ProducersField {
                    name: "processed-by".to_string(),
                    values: vec![VersionedName {
                        name: "rustc".to_string(),
                        version: "1.75.0 (82e1608df 2023-12-21)".to_string(),
                    }],
                },
            ],
        },
        Producers {
            fields: vec![ProducersField {
                name: "processed-by".to_string(),
                values: vec![VersionedName {
                    name: "wit-component".to_string(),
                    version: "0.20.0".to_string(),
                }],
            }],
        },
        Producers {
            fields: vec![ProducersField {
                name: "processed-by".to_string(),
                values: vec![VersionedName {
                    name: "wit-component".to_string(),
                    version: "0.20.0".to_string(),
                }],
            }],
        },
    ];

    assert_eq!(producers, expected);
}
