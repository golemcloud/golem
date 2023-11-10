use pretty_assertions::assert_eq;
use wasm_ast::component::Component;
use wasm_ast::metadata::{Metadata, Producers, ProducersField, VersionedName};
use wasm_ast::IgnoreAllButMetadata;

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
                        version: "0.14.0".to_string(),
                    },
                    VersionedName {
                        name: "cargo-component".to_string(),
                        version: "0.1.0 (e57d1d1 2023-08-31 wasi:134dddc)".to_string(),
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
                            version: "1.72.1 (d5c2e9c34 2023-09-13)".to_string(),
                        },
                        VersionedName {
                            name: "clang".to_string(),
                            version: "15.0.6".to_string(),
                        },
                        VersionedName {
                            name: "wit-component".to_string(),
                            version: "0.14.0".to_string(),
                        },
                        VersionedName {
                            name: "wit-bindgen-rust".to_string(),
                            version: "0.11.0".to_string(),
                        },
                    ],
                },
            ],
        }),
        registry_metadata: None,
    };

    assert_eq!(inner_module_metadata, expected);
}
