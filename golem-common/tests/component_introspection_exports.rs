use golem_common::component_introspection::wit_parser::WitAnalysisContext;
use golem_common::component_introspection::{ExportedInstance, TopLevelExport};
use test_r::test;

test_r::enable!();

fn instance(exports: &[TopLevelExport]) -> &ExportedInstance {
    assert_eq!(exports.len(), 1, "expected a single top-level export");
    match &exports[0] {
        TopLevelExport::Instance(instance) => instance,
        TopLevelExport::Function(name) => {
            panic!("expected an instance export, got function {name}")
        }
    }
}

#[test]
fn exports_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");

    let exports = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let instance = instance(&exports);
    assert_eq!(instance.name, "golem:it/api");
    assert_eq!(
        instance.functions,
        vec![
            "initialize-cart".to_string(),
            "add-item".to_string(),
            "remove-item".to_string(),
            "update-item-quantity".to_string(),
            "checkout".to_string(),
            "get-cart-contents".to_string(),
        ]
    );
}

#[test]
fn exports_file_service_component() {
    let source_bytes = include_bytes!("../wasm/file-service.wasm");

    let exports = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let instance = instance(&exports);
    assert_eq!(instance.name, "golem:it/api");
    assert_eq!(
        instance.functions,
        vec![
            "read-file".to_string(),
            "write-file".to_string(),
            "write-file-direct".to_string(),
            "delete-file".to_string(),
            "get-file-info".to_string(),
            "get-info".to_string(),
            "create-directory".to_string(),
            "create-link".to_string(),
            "create-sym-link".to_string(),
            "remove-directory".to_string(),
            "remove-file".to_string(),
            "rename-file".to_string(),
            "hash".to_string(),
        ]
    );
}

#[test]
fn exports_auction_registry_composed_component() {
    let source_bytes = include_bytes!("../wasm/auction_registry_composed.wasm");

    let exports = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let instance = instance(&exports);
    assert_eq!(instance.name, "auction:registry/api");
    assert_eq!(
        instance.functions,
        vec![
            "create-bidder".to_string(),
            "create-auction".to_string(),
            "get-auctions".to_string(),
        ]
    );
}

#[test]
fn exports_shopping_cart_resource_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource.wasm");
    let exports = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let instance = instance(&exports);
    assert_eq!(instance.name, "golem:it/api");
    assert_eq!(
        instance.functions,
        vec![
            "[constructor]cart".to_string(),
            "[method]cart.add-item".to_string(),
            "[method]cart.remove-item".to_string(),
            "[method]cart.update-item-quantity".to_string(),
            "[method]cart.checkout".to_string(),
            "[method]cart.get-cart-contents".to_string(),
            "[method]cart.merge-with".to_string(),
        ]
    );
}

#[test]
fn exports_shopping_cart_resource_versioned_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource-versioned.wasm");
    let exports = WitAnalysisContext::new(source_bytes)
        .unwrap()
        .get_top_level_exports()
        .unwrap();

    let instance = instance(&exports);
    assert_eq!(instance.name, "golem:it/api@1.2.3");
    assert_eq!(
        instance.functions,
        vec![
            "[constructor]cart".to_string(),
            "[method]cart.add-item".to_string(),
            "[method]cart.remove-item".to_string(),
            "[method]cart.update-item-quantity".to_string(),
            "[method]cart.checkout".to_string(),
            "[method]cart.get-cart-contents".to_string(),
            "[method]cart.merge-with".to_string(),
        ]
    );
}
